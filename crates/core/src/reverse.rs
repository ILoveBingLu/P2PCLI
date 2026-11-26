//! 反向端口转发：把本机（连接方）的端口接到对方电脑的 127.0.0.1 上。对方电脑上的程序连那个端口，
//! 就连到了本机的服务——比如本机的代理（借网给没有代理的对方），或者本机正在调的 Webhook、接口。
//!
//! 被连方不往连接方开流，所以这样做：连接方开一条一直连着的控制流（[`Msg::ReverseForward`]），
//! 被连方在自己的 127.0.0.1 上监听；每来一个连接，被连方在控制流上发 [`Msg::ReverseIncoming`]，
//! 连接方连好本机的目标，再开一条新流（[`Msg::ReverseConnect`]）接上，两边原样对拷。
//! 控制流断了（连接方停掉、连接断开），被连方就关掉监听。
//!
//! 借网：转的是本机的代理时，被连方给这条连接之后新开的终端、后台会话和远程执行加上
//! HTTP_PROXY、HTTPS_PROXY、ALL_PROXY，不碰对方系统的全局代理，也不影响对方电脑上别的程序。

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::timeout;

use crate::client::RemoteHost;
use crate::proto::{Msg, read_msg, write_msg};

/// 被连方收到连接以后，等连接方接上的最长时间
const ACCEPT_WAIT: Duration = Duration::from_secs(15);

/// 借给对方的代理是哪种：决定环境变量里写 http:// 还是 socks5h://
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyKind {
    Http,
    Socks5,
}

/// 借网时给对方新开的终端加的环境变量
pub fn proxy_env(kind: ProxyKind, port: u16) -> Vec<(String, String)> {
    let url = match kind {
        ProxyKind::Http => format!("http://127.0.0.1:{port}"),
        // socks5h：域名也交给代理解析，对方连不上外网的 DNS 时也能用
        ProxyKind::Socks5 => format!("socks5h://127.0.0.1:{port}"),
    };
    let no_proxy = "localhost,127.0.0.1,::1".to_string();
    // Windows 的环境变量不分大小写，只写大写的
    let names: &[&str] = if cfg!(windows) {
        &["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"]
    } else {
        &["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy"]
    };
    let mut env: Vec<(String, String)> = names.iter().map(|n| (n.to_string(), url.clone())).collect();
    env.push(("NO_PROXY".into(), no_proxy.clone()));
    if !cfg!(windows) {
        env.push(("no_proxy".into(), no_proxy));
    }
    env
}

// ---------------------------------------------------------------- 被连方

/// 一个反向转发在被连方这边等着接的连接
struct Listening {
    remote: iroh::EndpointId,
    next: AtomicU64,
    pending: Mutex<HashMap<u64, TcpStream>>,
}

static LISTENING: Mutex<Option<HashMap<u64, Weak<Listening>>>> = Mutex::new(None);

struct Registered(u64);

impl Drop for Registered {
    fn drop(&mut self) {
        if let Some(m) = LISTENING.lock().unwrap().as_mut() {
            m.remove(&self.0);
        }
    }
}

/// 被连方：在 127.0.0.1 上监听，来了连接就在控制流上告诉连接方，直到控制流断开。
/// listening 在开始监听后调一次（借网时记下代理的环境变量）
pub async fn serve<S, R>(remote: iroh::EndpointId, port: u16, send: &mut S, recv: &mut R, listening: impl FnOnce(u16)) -> Result<()>
where
    S: AsyncWrite + Unpin,
    R: AsyncRead + Unpin,
{
    // 只听回环地址：对方局域网里别的电脑连不到
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
        .await
        .map_err(|_| anyhow!("对方电脑上这个端口已经被占用了"))?;
    let port = listener.local_addr()?.port();
    let token = loop {
        let t: u64 = rand::random();
        if t != 0 {
            break t;
        }
    };
    let state = Arc::new(Listening { remote, next: AtomicU64::new(1), pending: Mutex::new(HashMap::new()) });
    LISTENING.lock().unwrap().get_or_insert_with(HashMap::new).insert(token, Arc::downgrade(&state));
    let _registered = Registered(token);
    write_msg(send, &Msg::ReverseListening { port, token }).await?;
    listening(port);
    tracing::info!(port, "开始反向端口转发");

    let mut probe = [0u8; 1];
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((tcp, _)) = accepted else { continue };
                let _ = tcp.set_nodelay(true);
                let id = state.next.fetch_add(1, Ordering::Relaxed);
                state.pending.lock().unwrap().insert(id, tcp);
                write_msg(send, &Msg::ReverseIncoming { id }).await?;
                // 连接方一直没接上：关掉这个连接
                let weak = Arc::downgrade(&state);
                tokio::spawn(async move {
                    tokio::time::sleep(ACCEPT_WAIT).await;
                    if let Some(s) = weak.upgrade() {
                        s.pending.lock().unwrap().remove(&id);
                    }
                });
            }
            // 连接方不再往控制流里写东西；读到结束就是它停了
            n = recv.read(&mut probe) => {
                if matches!(n, Ok(0) | Err(_)) {
                    break;
                }
            }
        }
    }
    tracing::info!(port, "停止反向端口转发");
    Ok(())
}

/// 被连方：连接方接上了某个连接，拿出等着的那个 TCP 连接
pub fn take(remote: iroh::EndpointId, token: u64, id: u64) -> Result<TcpStream> {
    let state = LISTENING.lock().unwrap().as_ref().and_then(|m| m.get(&token)).and_then(Weak::upgrade);
    let state = match state {
        Some(s) if s.remote == remote => s,
        _ => bail!("这个反向端口转发已经停了"),
    };
    state.pending.lock().unwrap().remove(&id).ok_or_else(|| anyhow!("这个连接已经断了"))
}

// ---------------------------------------------------------------- 连接方

/// 连接方：一个正在进行的反向转发。丢掉它就停，对方关掉监听，正在转的连接也一起断掉
pub struct ReverseForward {
    /// 对方电脑上监听的端口
    pub remote_port: u16,
    /// 本机的目标，主机:端口
    pub target: String,
    pub proxy: Option<ProxyKind>,
    connections: Arc<AtomicUsize>,
    task: JoinHandle<()>,
}

impl ReverseForward {
    /// 让对方在它的 127.0.0.1:remote_port（0 是随便挑一个空闲的）上监听，连过来的都接到本机的 target。
    /// proxy 有值时 target 是本机的代理，对方给这条连接之后新开的终端加上代理环境变量
    pub async fn start(remote: Arc<RemoteHost>, remote_port: u16, target: String, proxy: Option<ProxyKind>) -> Result<ReverseForward> {
        if remote.is_local() {
            bail!("这是本机，不用反向转发");
        }
        remote.require_reverse_forward()?;
        let target = target.trim().to_string();
        if target.is_empty() {
            bail!("没写本机要接到哪个端口");
        }
        // 先试连一次本机的目标：没开就直接说
        timeout(Duration::from_secs(5), TcpStream::connect(&target))
            .await
            .map_err(|_| anyhow!("连不上本机的 {target}"))?
            .with_context(|| format!("连不上本机的 {target}"))?;
        // 直连优先：刚连上还在中继上的话先等一会儿直连，等不到就走中继（界面上会提示慢）
        let path = remote.wait_direct(crate::path::DIRECT_WAIT).await;
        if !path.kind.is_direct() {
            tracing::info!(?path.kind, "反向转发开始时还没有直连，先走中继");
        }
        let (send, mut recv) = remote.open_stream(&Msg::ReverseForward { port: remote_port, proxy }).await?;
        let (port, token) = match timeout(Duration::from_secs(15), read_msg(&mut recv)).await.map_err(|_| anyhow!("对方没有响应"))?? {
            Msg::ReverseListening { port, token } => (port, token),
            Msg::Error { message } => bail!("{message}"),
            other => bail!("意外的回复：{other:?}"),
        };
        let connections = Arc::new(AtomicUsize::new(0));
        let task = tokio::spawn(run(remote, send, recv, token, target.clone(), connections.clone()));
        tracing::info!(remote_port = port, %target, ?proxy, "开始反向端口转发");
        Ok(ReverseForward { remote_port: port, target, proxy, connections, task })
    }

    /// 正在转的连接数
    pub fn connections(&self) -> usize {
        self.connections.load(Ordering::Relaxed)
    }

    /// 控制流断了（对方停了、连接断了）
    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }
}

impl Drop for ReverseForward {
    fn drop(&mut self) {
        self.task.abort();
        tracing::info!(remote_port = self.remote_port, target = %self.target, "停止反向端口转发");
    }
}

/// 控制流：对方每来一个连接，连好本机的目标，开一条新流接上。任务被停掉时，JoinSet 一起丢掉，正在转的连接都断开
async fn run(remote: Arc<RemoteHost>, send: iroh::endpoint::SendStream, mut recv: iroh::endpoint::RecvStream, token: u64, target: String, connections: Arc<AtomicUsize>) {
    // 控制流的发送端留着：丢掉的话对方读到结束，就当停了
    let _send = send;
    let mut running = JoinSet::new();
    loop {
        tokio::select! {
            msg = read_msg(&mut recv) => match msg {
                Ok(Msg::ReverseIncoming { id }) => {
                    let (remote, target, connections) = (remote.clone(), target.clone(), connections.clone());
                    running.spawn(async move {
                        connections.fetch_add(1, Ordering::Relaxed);
                        if let Err(e) = connect(&remote, token, id, &target).await {
                            tracing::debug!(%target, "反向转发的一个连接没接上：{e:#}");
                        }
                        connections.fetch_sub(1, Ordering::Relaxed);
                    });
                }
                Ok(_) => {}
                Err(_) => break,
            },
            Some(_) = running.join_next() => {}
        }
    }
}

async fn connect(remote: &RemoteHost, token: u64, id: u64, target: &str) -> Result<()> {
    let tcp = timeout(Duration::from_secs(5), TcpStream::connect(target)).await.map_err(|_| anyhow!("连本机的目标超时"))??;
    let _ = tcp.set_nodelay(true);
    let (send, mut recv) = remote.open_stream(&Msg::ReverseConnect { token, id }).await?;
    match timeout(Duration::from_secs(15), read_msg(&mut recv)).await.map_err(|_| anyhow!("对方没有响应"))?? {
        Msg::PortOpened => {}
        Msg::Error { message } => bail!("{message}"),
        other => bail!("意外的回复：{other:?}"),
    }
    crate::ports::pipe(tcp, tokio::io::join(recv, send)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_for_proxy() {
        let env = proxy_env(ProxyKind::Http, 4321);
        assert!(env.iter().any(|(k, v)| k == "HTTPS_PROXY" && v == "http://127.0.0.1:4321"));
        assert!(env.iter().any(|(k, v)| k == "NO_PROXY" && v.contains("127.0.0.1")));
        let env = proxy_env(ProxyKind::Socks5, 1);
        assert!(env.iter().any(|(k, v)| k == "ALL_PROXY" && v == "socks5h://127.0.0.1:1"));
    }
}
