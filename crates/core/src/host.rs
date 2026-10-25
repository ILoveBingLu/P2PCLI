//! 被连方：校验配对密码，弹窗等主人点同意，然后按需开终端。

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use iroh::EndpointId;
use iroh::endpoint::{Connection, RecvStream, SendStream};
use serde::Serialize;
use tokio::time::timeout;
use tracing::{debug, info, warn};

use crate::PROTO_VERSION;
use crate::identity::DeviceInfo;
use crate::proto::{Msg, RejectCode, TerminalSession, read_frame, read_json, read_msg, write_json, write_msg};
use crate::pty::{serve_session, serve_terminal};
use crate::sessions::SessionManager;
use crate::presence::{PresenceReq, PresenceResp};
use crate::store::{TrustStore, write_private};

/// 弹窗最多等多久
pub const APPROVE_TIMEOUT: Duration = Duration::from_secs(60);
/// 同一台设备连续错几次锁住
const MAX_FAILS_PER_PEER: u32 = 5;
const PEER_LOCK: Duration = Duration::from_secs(10 * 60);
/// 所有设备加起来一段时间内错太多次，整体暂停密码校验（防换钥匙爆破）
const GLOBAL_WINDOW: Duration = Duration::from_secs(10 * 60);
const GLOBAL_MAX_FAILS: usize = 30;
/// 配对密码字符：去掉 0 o 1 l i 这些容易看错的
const PASSWORD_ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
const PASSWORD_LEN: usize = 6;

#[derive(Debug, Clone, Serialize)]
pub struct IncomingRequest {
    pub id: u64,
    pub remote: String,
    pub name: String,
    pub platform: String,
}

/// 没配对过的设备要发文件过来，问主人收不收
#[derive(Debug, Clone, Serialize)]
pub struct FileRequest {
    pub id: u64,
    pub remote: String,
    pub name: String,
    pub platform: String,
    pub files: usize,
    pub total: u64,
    /// 前几项的名字（文件或文件夹）
    pub items: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// trust 为真时记成受信设备，以后免密码免弹窗
    Allow { trust: bool },
    Deny,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostEvent {
    /// 弹窗还开着，但对面走了或超时了，界面要关掉弹窗
    RequestCancelled { id: u64 },
    Connected {
        session: u64,
        remote: String,
        name: String,
        platform: String,
        trusted: bool,
    },
    Disconnected { session: u64 },
    TerminalOpened { session: u64 },
    TerminalClosed { session: u64 },
    /// 收到一批文件，已经放好
    FilesReceived {
        from: String,
        from_name: String,
        files: usize,
        total: u64,
        dir: String,
        paths: Vec<String>,
    },
}

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// 界面层实现：弹窗问主人、显示谁连着本机
pub trait HostDelegate: Send + Sync + 'static {
    fn approve(&self, req: IncomingRequest) -> BoxFuture<Decision>;
    fn event(&self, ev: HostEvent);
    /// 没配对过的设备发文件来，问主人收不收；没有界面的默认不收
    fn approve_files(&self, _req: FileRequest) -> BoxFuture<bool> {
        Box::pin(async { false })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionSummary {
    pub session: u64,
    pub remote: String,
    pub name: String,
    pub platform: String,
    pub terminals: u32,
    pub since: u64,
}

struct SessionEntry {
    conn: Connection,
    summary: SessionSummary,
}

#[derive(Default)]
struct Limiter {
    peers: HashMap<EndpointId, (u32, Option<Instant>)>,
    global: VecDeque<Instant>,
}

impl Limiter {
    fn locked(&mut self, id: &EndpointId) -> bool {
        let now = Instant::now();
        while self
            .global
            .front()
            .is_some_and(|t| now.duration_since(*t) > GLOBAL_WINDOW)
        {
            self.global.pop_front();
        }
        if self.global.len() >= GLOBAL_MAX_FAILS {
            return true;
        }
        match self.peers.get(id) {
            Some((_, Some(until))) if *until > now => true,
            Some((_, Some(_))) => {
                self.peers.remove(id);
                false
            }
            _ => false,
        }
    }

    fn fail(&mut self, id: &EndpointId) {
        let now = Instant::now();
        self.global.push_back(now);
        let e = self.peers.entry(*id).or_insert((0, None));
        e.0 += 1;
        if e.0 >= MAX_FAILS_PER_PEER {
            e.1 = Some(now + PEER_LOCK);
        }
    }

    fn success(&mut self, id: &EndpointId) {
        self.peers.remove(id);
    }
}

pub fn generate_password() -> String {
    (0..PASSWORD_LEN)
        .map(|_| PASSWORD_ALPHABET[rand::random_range(0..PASSWORD_ALPHABET.len())] as char)
        .collect()
}

/// 定长比较，不因提前退出泄露猜对了几位
fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// 对面报的名字要显示在弹窗里，去掉控制字符并限长
pub(crate) fn clean_name(s: &str) -> String {
    let s: String = s.chars().filter(|c| !c.is_control()).take(48).collect();
    if s.trim().is_empty() {
        "未命名设备".into()
    } else {
        s
    }
}

pub struct Host {
    me: RwLock<DeviceInfo>,
    pub terminals: SessionManager,
    password: RwLock<String>,
    /// 固定了的配对密码存在这里，重启也不变；没固定就不存，每次启动换新的
    password_file: PathBuf,
    password_fixed: AtomicBool,
    pub trust: TrustStore,
    /// 同意过一次的设备：能在后台探测延迟、看自己的会话列表，开终端照常要密码和同意
    pub paired: TrustStore,
    limiter: Mutex<Limiter>,
    sessions: Mutex<HashMap<u64, SessionEntry>>,
    next_id: AtomicU64,
    delegate: Arc<dyn HostDelegate>,
}

impl Host {
    pub fn new(me: DeviceInfo, data_dir: &Path, local_key: &str, session_program: Option<PathBuf>, delegate: Arc<dyn HostDelegate>) -> Self {
        let password_file = data_dir.join("pairing_password.txt");
        let fixed = std::fs::read_to_string(&password_file)
            .ok()
            .map(|p| p.trim().to_lowercase())
            .filter(|p| !p.is_empty());
        Self {
            me: RwLock::new(me),
            terminals: SessionManager::new(local_key, session_program),
            password_fixed: AtomicBool::new(fixed.is_some()),
            password: RwLock::new(fixed.unwrap_or_else(generate_password)),
            password_file,
            trust: TrustStore::open(data_dir),
            paired: TrustStore::open_file(data_dir, "paired.json"),
            limiter: Mutex::new(Limiter::default()),
            sessions: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            delegate,
        }
    }

    pub fn set_info(&self, me: DeviceInfo) { *self.me.write().unwrap() = me; }

    pub fn password(&self) -> String {
        self.password.read().unwrap().clone()
    }

    pub fn regenerate_password(&self) -> String {
        let p = generate_password();
        *self.password.write().unwrap() = p.clone();
        if self.password_fixed()
            && let Err(e) = write_private(&self.password_file, p.as_bytes())
        {
            warn!("保存固定的配对密码失败：{e:#}");
        }
        p
    }

    pub fn password_fixed(&self) -> bool {
        self.password_fixed.load(Ordering::Relaxed)
    }

    /// 固定配对密码：存进数据目录，重启也不变；取消固定就删掉，下次启动换新的
    pub fn set_password_fixed(&self, fixed: bool) -> Result<()> {
        if fixed {
            write_private(&self.password_file, self.password().as_bytes())?;
        } else if let Err(e) = std::fs::remove_file(&self.password_file)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            return Err(e.into());
        }
        self.password_fixed.store(fixed, Ordering::Relaxed);
        Ok(())
    }

    pub fn set_password(&self, p: &str) {
        *self.password.write().unwrap() = p.trim().to_lowercase();
    }

    pub fn sessions(&self) -> Vec<SessionSummary> {
        let mut v: Vec<_> = self
            .sessions
            .lock()
            .unwrap()
            .values()
            .map(|e| e.summary.clone())
            .collect();
        v.sort_by_key(|s| s.session);
        v
    }

    /// 主人点断开
    pub fn kick(&self, session: u64) -> bool {
        match self.sessions.lock().unwrap().get(&session) {
            Some(e) => {
                e.conn.close(1u32.into(), "对方断开了连接".as_bytes());
                true
            }
            None => false,
        }
    }

    pub fn kick_all(&self) {
        for e in self.sessions.lock().unwrap().values() {
            e.conn.close(1u32.into(), "对方断开了连接".as_bytes());
        }
    }

    fn next(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// 弹窗问主人收不收这批文件；没人点、对方走了都算不收
    pub async fn ask_files(&self, mut req: FileRequest, conn: &Connection) -> bool {
        req.id = self.next();
        let id = req.id;
        tokio::select! {
            ok = self.delegate.approve_files(req) => ok,
            _ = tokio::time::sleep(APPROVE_TIMEOUT) => {
                self.delegate.event(HostEvent::RequestCancelled { id });
                false
            }
            _ = conn.closed() => {
                self.delegate.event(HostEvent::RequestCancelled { id });
                false
            }
        }
    }

    pub fn notify(&self, ev: HostEvent) {
        self.delegate.event(ev);
    }

    fn check_password(&self, remote: &EndpointId, given: Option<&str>) -> Option<RejectCode> {
        let mut lim = self.limiter.lock().unwrap();
        if lim.locked(remote) {
            return Some(RejectCode::Locked);
        }
        let given = match given.map(|g| g.trim().to_lowercase()) {
            Some(g) if !g.is_empty() => g,
            _ => return Some(RejectCode::NeedPassword),
        };
        if same(&given, &self.password.read().unwrap()) {
            lim.success(remote);
            None
        } else {
            lim.fail(remote);
            Some(RejectCode::BadPassword)
        }
    }

    /// 回答在线探测：不弹窗、不开终端。配对过的设备能拿到自己的会话列表（只有编号、状态和里面 AI 的种类、状态，不带目录和程序），
    /// 没配对过的只回一句“没配对”，对方从这次握手里也就知道在线和延迟
    pub async fn handle_presence(self: Arc<Self>, conn: Connection) {
        let remote = conn.remote_id();
        let owner = remote.to_string();
        // 每个请求一条流，分开处理：读会话列表慢一点也不耽误 ping
        while let Ok((mut send, mut recv)) = conn.accept_bi().await {
            let host = self.clone();
            let owner = owner.clone();
            tokio::spawn(async move {
                let Ok(Ok(req)) = timeout(Duration::from_secs(10), read_json::<_, PresenceReq>(&mut recv)).await else {
                    return;
                };
                let resp = match req {
                    PresenceReq::Ping => PresenceResp::Pong,
                    PresenceReq::Sessions if host.trust.contains(&remote) || host.paired.contains(&remote) => {
                        match host.terminals.list(&owner).await {
                            Ok(list) => PresenceResp::Sessions {
                                sessions: list.into_iter().map(|s| TerminalSession { details: None, ..s }).collect(),
                            },
                            Err(e) => {
                                debug!("探测时读会话列表失败：{e:#}");
                                PresenceResp::Sessions { sessions: Vec::new() }
                            }
                        }
                    }
                    PresenceReq::Sessions => PresenceResp::NotPaired,
                };
                if write_json(&mut send, &resp).await.is_ok() {
                    let _ = send.finish();
                }
            });
        }
    }

    /// 处理一条进来的连接，直到对面走
    pub async fn handle(self: Arc<Self>, conn: Connection) {
        let remote = conn.remote_id();
        if let Err(e) = self.handle_inner(&conn, remote).await {
            debug!(remote = %remote.fmt_short(), "连接结束：{e:#}");
        }
    }

    async fn handle_inner(self: &Arc<Self>, conn: &Connection, remote: EndpointId) -> Result<()> {
        let (mut send, mut recv) = timeout(Duration::from_secs(15), conn.accept_bi()).await??;
        let Msg::Hello {
            v,
            name,
            platform,
            password,
        } = timeout(Duration::from_secs(10), read_msg(&mut recv)).await??
        else {
            bail!("开场消息不对");
        };
        let name = clean_name(&name);
        let platform = clean_name(&platform);
        if v != PROTO_VERSION {
            return reject(conn, &mut send, RejectCode::Version).await;
        }

        let trusted = self.trust.contains(&remote);
        if !trusted {
            if let Some(code) = self.check_password(&remote, password.as_deref()) {
                info!(remote = %remote.fmt_short(), ?code, "拒绝连接");
                return reject(conn, &mut send, code).await;
            }
            let id = self.next();
            let req = IncomingRequest {
                id,
                remote: remote.to_string(),
                name: name.clone(),
                platform: platform.clone(),
            };
            let decision = tokio::select! {
                d = self.delegate.approve(req) => Some(d),
                _ = tokio::time::sleep(APPROVE_TIMEOUT) => None,
                _ = conn.closed() => {
                    self.delegate.event(HostEvent::RequestCancelled { id });
                    return Ok(());
                }
            };
            match decision {
                None => {
                    self.delegate.event(HostEvent::RequestCancelled { id });
                    return reject(conn, &mut send, RejectCode::Timeout).await;
                }
                Some(Decision::Deny) => return reject(conn, &mut send, RejectCode::Denied).await,
                Some(Decision::Allow { trust }) => {
                    if trust && let Err(e) = self.trust.add(&remote, &name, &platform) {
                        warn!("记受信设备失败：{e:#}");
                    }
                }
            }
        }

        // 同意过（包括受信设备直接进来的）就记成配对过的设备，之后能在后台看延迟和会话列表
        if !self.paired.contains(&remote)
            && let Err(e) = self.paired.add(&remote, &name, &platform)
        {
            warn!("记配对设备失败：{e:#}");
        }

        let me = self.me.read().unwrap().clone();
        write_msg(
            &mut send,
            &Msg::Welcome {
                name: me.name,
                platform: me.platform,
                user: me.user,
            },
        )
        .await?;

        let session = self.next();
        self.sessions.lock().unwrap().insert(
            session,
            SessionEntry {
                conn: conn.clone(),
                summary: SessionSummary {
                    session,
                    remote: remote.to_string(),
                    name: name.clone(),
                    platform: platform.clone(),
                    terminals: 0,
                    since: crate::store::now_secs(),
                },
            },
        );
        info!(session, remote = %remote.fmt_short(), %name, trusted, "已连接");
        self.delegate.event(HostEvent::Connected {
            session,
            remote: remote.to_string(),
            name,
            platform,
            trusted,
        });

        // 控制流关了就算对面走了；读放单独任务，免得和 accept_bi 抢
        let mut control = tokio::spawn(async move {
            while let Ok(Some(_)) = read_frame(&mut recv).await {}
        });
        loop {
            tokio::select! {
                bi = conn.accept_bi() => match bi {
                    Ok((s, r)) => {
                        let me = self.clone();
                        tokio::spawn(async move { me.terminal(session, remote.to_string(), s, r).await });
                    }
                    Err(_) => break,
                },
                _ = &mut control => break,
            }
        }
        control.abort();
        self.sessions.lock().unwrap().remove(&session);
        conn.close(0u32.into(), b"bye");
        info!(session, "已断开");
        self.delegate.event(HostEvent::Disconnected { session });
        Ok(())
    }

    async fn terminal(self: Arc<Self>, session: u64, owner: String, mut send: SendStream, mut recv: RecvStream) {
        let request = match timeout(Duration::from_secs(10), read_msg(&mut recv)).await {
            Ok(Ok(msg)) => msg,
            _ => return,
        };
        let result: Result<()> = async {
            match request {
                Msg::ListTerminals => {
                    let sessions = self.terminals.list(&owner).await?;
                    write_msg(&mut send, &Msg::Terminals { sessions }).await?;
                }
                Msg::EndTerminal { session_id } => {
                    self.terminals.end(&owner, &session_id).await?;
                    write_msg(&mut send, &Msg::TerminalEnded).await?;
                }
                Msg::Open { cols, rows, session_id, persistent } => {
                    let attachment = if persistent {
                        Some(self.terminals.attach(&owner, session_id.as_deref(), cols, rows).await?)
                    } else {
                        if session_id.is_some() { bail!("临时终端不能恢复后台会话"); }
                        None
                    };
                    self.bump_terminals(session, 1);
                    self.delegate.event(HostEvent::TerminalOpened { session });
                    let result = match attachment {
                        Some((cmd, id)) => serve_session(recv, &mut send, cols, rows, cmd, id).await,
                        None => serve_terminal(recv, &mut send, cols, rows).await,
                    };
                    self.bump_terminals(session, -1);
                    self.delegate.event(HostEvent::TerminalClosed { session });
                    result?;
                }
                _ => bail!("不支持的会话操作"),
            }
            Ok(())
        }.await;
        if let Err(e) = result {
            warn!(session, %owner, "会话操作失败：{e:#}");
            let _ = write_msg(&mut send, &Msg::Error { message: format!("{e:#}") }).await;
        }
        let _ = send.finish();
    }

    fn bump_terminals(&self, session: u64, delta: i32) {
        if let Some(e) = self.sessions.lock().unwrap().get_mut(&session) {
            e.summary.terminals = (e.summary.terminals as i32 + delta).max(0) as u32;
        }
    }
}

/// 回一个拒绝，等对面读完再关
async fn reject(conn: &Connection, send: &mut SendStream, code: RejectCode) -> Result<()> {
    write_msg(send, &Msg::Reject { code }).await?;
    let _ = send.finish();
    let _ = timeout(Duration::from_secs(3), conn.closed()).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::SecretKey;

    #[test]
    fn password_shape() {
        let p = generate_password();
        assert_eq!(p.len(), PASSWORD_LEN);
        assert!(p.bytes().all(|b| PASSWORD_ALPHABET.contains(&b)));
    }

    #[test]
    fn limiter_locks_after_fails() {
        let mut l = Limiter::default();
        let id = SecretKey::generate().public();
        for _ in 0..MAX_FAILS_PER_PEER {
            assert!(!l.locked(&id));
            l.fail(&id);
        }
        assert!(l.locked(&id));
        let other = SecretKey::generate().public();
        assert!(!l.locked(&other));
    }

    #[test]
    fn global_lock() {
        let mut l = Limiter::default();
        for _ in 0..GLOBAL_MAX_FAILS {
            l.fail(&SecretKey::generate().public());
        }
        assert!(l.locked(&SecretKey::generate().public()));
    }

    #[test]
    fn compare() {
        assert!(same("abc234", "abc234"));
        assert!(!same("abc234", "abc235"));
        assert!(!same("abc", "abc234"));
        assert_eq!(clean_name("a\u{1b}[31mb"), "a[31mb");
        assert_eq!(clean_name("  "), "未命名设备");
    }
}
