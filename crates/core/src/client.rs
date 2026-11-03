//! 连接方：连上对方、开终端、收发字节。

use std::fmt;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use bytes::Bytes;
use iroh::endpoint::{Connection, PathId, RecvStream, SendStream};
use iroh::{Endpoint, EndpointAddr};
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncWrite, DuplexStream, ReadBuf};
use tokio::time::timeout;

use crate::collaboration::model::{Reply, Request};
use crate::host::{APPROVE_TIMEOUT, Host};
use crate::identity::DeviceInfo;
use crate::path::{self, PathInfo};
use crate::proto::{AGENT_MESSAGE_V1, COLLABORATION_V1, DIR_PICKER_V1, Frame, Msg, RejectCode, TerminalSession, read_frame, read_msg, write_data, write_msg};
use crate::pty::{serve_session, serve_terminal};
use crate::{ALPN_TERM, PROTO_VERSION};

#[derive(Debug)]
pub enum ConnectError {
    Rejected(RejectCode),
    Other(anyhow::Error),
}

impl fmt::Display for ConnectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConnectError::Rejected(c) => f.write_str(c.describe()),
            ConnectError::Other(e) => write!(f, "{e:#}"),
        }
    }
}

impl std::error::Error for ConnectError {}

impl From<anyhow::Error> for ConnectError {
    fn from(e: anyhow::Error) -> Self {
        ConnectError::Other(e)
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RemoteInfo {
    pub endpoint_id: String,
    pub name: String,
    pub platform: String,
    pub user: String,
    /// 对方的软件版本、系统、会话管理器；旧版本不告诉
    pub version: Option<String>,
    pub os: Option<String>,
    pub multiplexer: Option<String>,
}

/// 一台已经同意连接的设备：远程的，或者本机自己
pub struct RemoteHost {
    link: Link,
    pub info: RemoteInfo,
}

enum Link {
    /// 走网络的连接
    Net {
        conn: Connection,
        /// 控制流一直开着；关掉它对面就当我们走了
        _control: SendStream,
        capabilities: Vec<String>,
    },
    /// 本机自己：不走网络，直接用本机的会话管理器，终端和会话的行为和远程设备完全一样
    Local {
        host: Arc<Host>,
        /// 会话归在这个名下；用本机的公钥，重启以后还能认回自己的后台会话
        owner: String,
    },
}

pub async fn connect(
    endpoint: &Endpoint,
    addr: EndpointAddr,
    me: &DeviceInfo,
    password: Option<String>,
) -> Result<RemoteHost, ConnectError> {
    let conn = dial(endpoint, addr).await?;
    handshake(conn, me, password).await
}

/// 只建 QUIC 连接，不发开场消息
pub async fn dial(endpoint: &Endpoint, addr: EndpointAddr) -> Result<Connection> {
    timeout(Duration::from_secs(30), endpoint.connect(addr, ALPN_TERM))
        .await
        .map_err(|_| anyhow!("30 秒内没连上对方，对方可能不在线"))?
        .map_err(|e| anyhow!("连不上对方：{e}"))
}

/// 在建好的连接上报身份、带密码，等对方点同意
pub async fn handshake(
    conn: Connection,
    me: &DeviceInfo,
    password: Option<String>,
) -> Result<RemoteHost, ConnectError> {
    let endpoint_id = conn.remote_id().to_string();
    let (mut send, mut recv) = conn.open_bi().await.map_err(|e| anyhow!("{e}"))?;
    write_msg(
        &mut send,
        &Msg::Hello {
            v: PROTO_VERSION,
            name: me.name.clone(),
            platform: me.platform.clone(),
            password,
            version: me.version.clone(),
            os: me.os.clone(),
            capabilities: vec![COLLABORATION_V1.into(), AGENT_MESSAGE_V1.into(), DIR_PICKER_V1.into()],
        },
    )
    .await?;
    // 对方要点同意，等得比弹窗超时长一点
    let reply = timeout(APPROVE_TIMEOUT + Duration::from_secs(15), read_msg(&mut recv))
        .await
        .map_err(|_| anyhow!("对方一直没有响应"))??;
    match reply {
        Msg::Welcome {
            name,
            platform,
            user,
            version,
            os,
            multiplexer,
            capabilities,
        } => Ok(RemoteHost {
            info: RemoteInfo {
                endpoint_id,
                name,
                platform,
                user,
                version,
                os,
                multiplexer,
            },
            link: Link::Net { conn, _control: send, capabilities },
        }),
        Msg::Reject { code } => {
            conn.close(0u32.into(), b"rejected");
            Err(ConnectError::Rejected(code))
        }
        other => Err(anyhow!("对方回了意外的消息：{other:?}").into()),
    }
}

/// 本机内部通道的缓冲大小：终端输出慢慢读也不会丢，写满了就等
const LOCAL_PIPE: usize = 256 * 1024;

impl RemoteHost {
    /// 本机自己：不走网络，直接用本机的会话管理器，所以会话、终端、AI 识别和远程设备一模一样
    pub async fn local(host: Arc<Host>, me: &DeviceInfo, endpoint_id: String) -> RemoteHost {
        let multiplexer = host.terminals.describe().await;
        RemoteHost {
            info: RemoteInfo {
                endpoint_id: endpoint_id.clone(),
                name: me.name.clone(),
                platform: me.platform.clone(),
                user: me.user.clone(),
                version: me.version.clone(),
                os: me.os.clone(),
                multiplexer,
            },
            link: Link::Local { host, owner: endpoint_id },
        }
    }

    /// 是本机自己，不是远程设备
    pub fn is_local(&self) -> bool {
        matches!(self.link, Link::Local { .. })
    }

    /// 对面的公钥；本机自己就是本机的公钥
    pub fn peer_id(&self) -> iroh::EndpointId {
        match &self.link {
            Link::Net { conn, .. } => conn.remote_id(),
            Link::Local { .. } => self.info.endpoint_id.parse().expect("本机公钥一定能解析"),
        }
    }

    /// 网络连接；本机自己没有
    fn conn(&self) -> Result<&Connection> {
        match &self.link {
            Link::Net { conn, .. } => Ok(conn),
            Link::Local { .. } => bail!("本机自己没有这条通道"),
        }
    }

    pub async fn open_terminal(&self, cols: u16, rows: u16) -> Result<(TermInput, TermOutput)> {
        self.attach_terminal(None, cols, rows).await
    }

    pub async fn attach_terminal(&self, session_id: Option<String>, cols: u16, rows: u16) -> Result<(TermInput, TermOutput)> {
        self.open(true, session_id, cols, rows).await
    }

    /// One-shot CLI commands retain the shell's exit code and do not leave background sessions.
    pub async fn open_ephemeral_terminal(&self, cols: u16, rows: u16) -> Result<(TermInput, TermOutput)> {
        self.open(false, None, cols, rows).await
    }

    async fn open(&self, persistent: bool, session_id: Option<String>, cols: u16, rows: u16) -> Result<(TermInput, TermOutput)> {
        let conn = match &self.link {
            Link::Net { conn, .. } => conn,
            Link::Local { host, owner } => return open_local(host, owner, persistent, session_id, cols, rows).await,
        };
        let (mut send, mut recv) = conn.open_bi().await?;
        write_msg(&mut send, &Msg::Open { persistent, cols, rows, session_id }).await?;
        match timeout(Duration::from_secs(35), read_msg(&mut recv)).await?? {
            Msg::Opened { shell, session_id } => Ok((
                TermInput { send: Some(TermWrite::Quic(send)) },
                TermOutput { recv: TermRead::Quic(recv), shell, session_id },
            )),
            Msg::Error { message } => bail!("对方开终端失败：{message}"),
            other => bail!("意外的回复：{other:?}"),
        }
    }

    pub async fn list_terminals(&self) -> Result<Vec<TerminalSession>> {
        if let Link::Local { host, owner } = &self.link {
            return host.terminals.list(owner).await;
        }
        match self.session_request(Msg::ListTerminals).await? {
            Msg::Terminals { sessions } => Ok(sessions),
            other => bail!("读取会话失败：{other:?}"),
        }
    }

    pub async fn end_terminal(&self, session_id: String) -> Result<()> {
        if let Link::Local { host, owner } = &self.link {
            return host.terminals.end(owner, &session_id).await;
        }
        match self.session_request(Msg::EndTerminal { session_id }).await? {
            Msg::TerminalEnded => Ok(()),
            other => bail!("结束会话失败：{other:?}"),
        }
    }

    pub async fn installed_agents(&self, refresh: bool) -> Result<Vec<crate::launchers::InstalledAgent>> {
        if let Link::Local { host, .. } = &self.link {
            return host.terminals.installed_agents(refresh).await;
        }
        match self.session_request(Msg::ListInstalledAgents { refresh }).await? {
            Msg::InstalledAgents { agents } => Ok(agents),
            other => bail!("检测 AI 失败（对方可能需要更新）：{other:?}"),
        }
    }

    pub async fn launch_agent(&self, session_id: String, kind: String, allow_bypass: bool) -> Result<()> {
        if let Link::Local { host, owner } = &self.link {
            return host.terminals.launch_agent(owner, &session_id, &kind, allow_bypass).await;
        }
        match self.session_request(Msg::LaunchAgent { session_id, kind, allow_bypass }).await? {
            Msg::AgentLaunched => Ok(()),
            other => bail!("启动 AI 失败：{other:?}"),
        }
    }

    pub async fn choose_agent_option(&self, session_id: String, approval_id: String, option_id: usize) -> Result<()> {
        if let Link::Local { host, owner } = &self.link {
            return host.terminals.choose_agent_option(owner, &session_id, &approval_id, option_id).await;
        }
        match self.session_request(Msg::ChooseAgentOption { session_id, approval_id, option_id }).await? {
            Msg::AgentOptionChosen => Ok(()),
            other => bail!("提交选项失败：{other:?}"),
        }
    }

    pub async fn send_agent_message(&self, session_id: String, kind: String, request_id: String, text: String) -> Result<()> {
        if let Link::Local { host, owner } = &self.link {
            return host.terminals.send_agent_message(owner, &session_id, &kind, &request_id, &text).await;
        }
        if let Link::Net { capabilities, .. } = &self.link
            && !capabilities.iter().any(|capability| capability == AGENT_MESSAGE_V1) {
            bail!("对方尚不支持自动接入，请更新对方 P2PCLI");
        }
        match self.session_request(Msg::SendAgentMessage { session_id, kind, request_id, text }).await? {
            Msg::AgentMessageSent => Ok(()),
            other => bail!("接入请求未确认：{other:?}"),
        }
    }

    /// 列出对方电脑上某个目录的子目录；路径空着是用户目录
    pub async fn list_dir(&self, path: String) -> Result<crate::dirs::DirListing> {
        if self.is_local() {
            return crate::dirs::list(&path).await;
        }
        self.require_dir_picker()?;
        match self.session_request(Msg::ListDir { path }).await? {
            Msg::DirListing { listing } => Ok(listing),
            other => bail!("读取目录失败：{other:?}"),
        }
    }

    /// 会话停在空闲提示符时切到这个目录
    pub async fn change_dir(&self, session_id: String, path: String) -> Result<()> {
        if let Link::Local { host, owner } = &self.link {
            return host.terminals.change_dir(owner, &session_id, &path).await;
        }
        self.require_dir_picker()?;
        match self.session_request(Msg::ChangeDir { session_id, path }).await? {
            Msg::DirChanged => Ok(()),
            other => bail!("切换目录失败：{other:?}"),
        }
    }

    fn require_dir_picker(&self) -> Result<()> {
        if let Link::Net { capabilities, .. } = &self.link
            && !capabilities.iter().any(|c| c == DIR_PICKER_V1)
        {
            bail!("对方的 P2PCLI 版本较旧，不支持选择目录，请更新对方的 P2PCLI");
        }
        Ok(())
    }

    async fn session_request(&self, msg: Msg) -> Result<Msg> {
        let conn = self.conn()?;
        // 超时要覆盖整个请求，不能只包住 read：open_bi/write 挂住同样会让前端一直等待。
        timeout(Duration::from_secs(20), async {
            let (mut send, mut recv) = conn.open_bi().await?;
            write_msg(&mut send, &msg).await?;
            let reply = read_msg(&mut recv).await?;
            if let Msg::Error { message } = reply { bail!("{message}"); }
            Ok(reply)
        })
        .await
        .map_err(|_| anyhow!("会话请求超时"))?
    }

    /// 协作有独立的请求预算，给事件长轮询留出时间；旧设备不会收到新增消息。
    pub async fn collaborate(&self, request: Request) -> Result<Reply> {
        self.require_collaboration()?;
        match &self.link {
            Link::Local { host, owner } => host.collaborate(owner.clone(), request).await,
            Link::Net { conn, .. } => {
                timeout(Duration::from_secs(35), async {
                    let (mut send, mut recv) = conn.open_bi().await?;
                    write_msg(&mut send, &Msg::CollabRequest { request }).await?;
                    send.finish()?;
                    match read_msg(&mut recv).await? {
                        Msg::CollabReply { reply } => Ok(reply),
                        Msg::Error { message } => bail!("{message}"),
                        _ => bail!("意外的协作回复类型"),
                    }
                })
                .await
                .map_err(|_| anyhow!("协作请求超时；操作可能已提交，重试时请使用同一操作 ID"))?
            }
        }
    }

    fn require_collaboration(&self) -> Result<()> {
        if let Link::Net { capabilities, .. } = &self.link
            && !capabilities.iter().any(|c| c == COLLABORATION_V1)
        {
            bail!("unsupported_capability: 对方不支持 collaboration_v1，请更新对方的 P2PCLI");
        }
        Ok(())
    }

    /// 开一条新流并先发一条控制消息，后面怎么走由这条消息决定（跑命令、拉文件、推文件）
    pub async fn open_stream(&self, first: &Msg) -> Result<(SendStream, RecvStream)> {
        if matches!(first, Msg::CollabRequest { .. } | Msg::CollabReply { .. }) {
            self.require_collaboration()?;
        }
        let conn = self.conn()?;
        timeout(Duration::from_secs(20), async {
            let (mut send, recv) = conn.open_bi().await?;
            write_msg(&mut send, first).await?;
            Ok((send, recv))
        })
        .await
        .map_err(|_| anyhow!("对方没有响应"))?
    }

    /// 在对方电脑上跑一条命令，不开终端；用返回的流收输出和退出码。
    /// 流被丢掉就等于取消，对方会结束这条命令。
    pub async fn exec(&self, command: &str, cwd: Option<String>, timeout_secs: Option<u64>) -> Result<ExecStream> {
        let (send, recv) = self.open_stream(&Msg::Exec { command: command.to_string(), cwd, timeout_secs }).await?;
        Ok(ExecStream { _send: send, recv, seen: false })
    }

    /// 这条连接现在用的拥塞控制算法（"cubic" 或 "bbr3"），测试和排查用；本机自己没有
    pub fn congestion_algorithm(&self) -> Option<&'static str> {
        match &self.link {
            Link::Net { conn, .. } => conn.congestion_state(PathId::ZERO).and_then(crate::quic::algorithm),
            Link::Local { .. } => None,
        }
    }

    /// 连接已经断了；本机自己不会断
    pub fn is_closed(&self) -> bool {
        match &self.link {
            Link::Net { conn, .. } => conn.close_reason().is_some(),
            Link::Local { .. } => false,
        }
    }

    pub fn path(&self) -> PathInfo {
        match &self.link {
            Link::Net { conn, .. } => path::current(conn),
            Link::Local { .. } => PathInfo { kind: path::PathKind::Local, rtt_ms: None, remote: None },
        }
    }

    pub fn stats(&self, endpoint: &iroh::Endpoint) -> path::ConnStats {
        match &self.link {
            Link::Net { conn, .. } => path::stats(conn, endpoint),
            Link::Local { .. } => path::ConnStats::local(),
        }
    }

    pub fn close(&self) {
        if let Link::Net { conn, .. } = &self.link {
            conn.close(0u32.into(), b"bye");
        }
    }

    /// 等连接断开，返回原因；本机自己不会断，一直等下去
    pub async fn closed(&self) -> String {
        match &self.link {
            Link::Net { conn, .. } => describe_close(conn.closed().await),
            Link::Local { .. } => std::future::pending().await,
        }
    }
}

/// 在本机开一个终端：起一对内存里的管道，当作“连接”的两头，服务端那头直接交给和远程设备同一套的终端服务
async fn open_local(
    host: &Arc<Host>,
    owner: &str,
    persistent: bool,
    session_id: Option<String>,
    cols: u16,
    rows: u16,
) -> Result<(TermInput, TermOutput)> {
    let attachment = if persistent {
        match host.terminals.attach(owner, session_id.as_deref(), cols, rows).await {
            Ok(attachment) => Some(attachment),
            // 没装 tmux、psmux 这类会话管理器：新开的退回普通终端，照样能用，只是软件关了就没了；
            // 要接回指定的后台会话就不能糊弄，照实报错
            Err(e) if session_id.is_none() => {
                tracing::warn!("本机用不了后台会话，改开普通终端：{e:#}");
                None
            }
            Err(e) => return Err(e),
        }
    } else {
        None
    };
    let (client_w, host_r) = tokio::io::duplex(LOCAL_PIPE);
    let (mut host_w, client_r) = tokio::io::duplex(LOCAL_PIPE);
    tokio::spawn(async move {
        let result = match attachment {
            Some((cmd, id)) => serve_session(host_r, &mut host_w, cols, rows, cmd, id).await,
            None => serve_terminal(host_r, &mut host_w, cols, rows).await,
        };
        if let Err(e) = result {
            let _ = write_msg(&mut host_w, &Msg::Error { message: format!("{e:#}") }).await;
        }
    });
    let mut recv = TermRead::Local(client_r);
    match timeout(Duration::from_secs(35), read_msg(&mut recv)).await?? {
        Msg::Opened { shell, session_id } => Ok((
            TermInput { send: Some(TermWrite::Local(client_w)) },
            TermOutput { recv, shell, session_id },
        )),
        Msg::Error { message } => bail!("开本机终端失败：{message}"),
        other => bail!("意外的回复：{other:?}"),
    }
}

fn describe_close(e: iroh::endpoint::ConnectionError) -> String {
    use iroh::endpoint::ConnectionError as E;
    match e {
        E::ApplicationClosed(c) => {
            let reason = String::from_utf8_lossy(&c.reason).to_string();
            if reason.is_empty() || reason == "bye" {
                "连接已关闭".into()
            } else {
                reason
            }
        }
        E::LocallyClosed => "连接已关闭".into(),
        E::TimedOut => "网络中断，连接超时".into(),
        other => format!("连接断开：{other}"),
    }
}

/// 终端输入这一头：远程设备是 QUIC 流，本机自己是内存管道
enum TermWrite {
    Quic(SendStream),
    Local(DuplexStream),
}

impl AsyncWrite for TermWrite {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            TermWrite::Quic(s) => Pin::new(s).poll_write(cx, buf).map_err(std::io::Error::other),
            TermWrite::Local(s) => Pin::new(s).poll_write(cx, buf),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            TermWrite::Quic(s) => Pin::new(s).poll_flush(cx),
            TermWrite::Local(s) => Pin::new(s).poll_flush(cx),
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            TermWrite::Quic(s) => Pin::new(s).poll_shutdown(cx),
            TermWrite::Local(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

/// 终端输出这一头
enum TermRead {
    Quic(RecvStream),
    Local(DuplexStream),
}

impl AsyncRead for TermRead {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            TermRead::Quic(s) => Pin::new(s).poll_read(cx, buf),
            TermRead::Local(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

pub struct TermInput {
    send: Option<TermWrite>,
}

impl TermInput {
    fn stream(&mut self) -> Result<&mut TermWrite> {
        self.send.as_mut().ok_or_else(|| anyhow!("终端输入已经关了"))
    }

    pub async fn write(&mut self, data: &[u8]) -> Result<()> {
        write_data(self.stream()?, data).await
    }

    pub async fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        write_msg(self.stream()?, &Msg::Resize { cols, rows }).await
    }

    /// 关闭当前附着；后台会话继续运行。
    pub fn close(&mut self) {
        if let Some(TermWrite::Quic(s)) = &mut self.send {
            let _ = s.finish();
        }
        // 本机自己的内存管道，丢掉写的这头对面就读到结尾
        self.send = None;
    }
}

/// Windows 的伪终端启动时会问光标位置（ESC[6n），不回答就卡住。
/// 界面里 xterm.js 会自动回答；没有界面的调用方（命令行、测试）看到它要回 ESC[1;1R。
pub fn asks_cursor_position(output: &[u8]) -> bool {
    output.windows(4).any(|w| w == b"\x1b[6n")
}

#[derive(Debug, PartialEq)]
pub enum TermEvent {
    Output(Bytes),
    Exit(Option<u32>),
}

pub struct TermOutput {
    recv: TermRead,
    pub shell: String,
    pub session_id: String,
}

impl TermOutput {
    /// 下一段输出；None 表示流结束
    pub async fn next(&mut self) -> Result<Option<TermEvent>> {
        loop {
            return match read_frame(&mut self.recv).await? {
                Some(Frame::Data(d)) => Ok(Some(TermEvent::Output(d))),
                Some(Frame::Msg(Msg::Exit { code })) => Ok(Some(TermEvent::Exit(code))),
                Some(Frame::Msg(Msg::Error { message })) => bail!("{message}"),
                Some(Frame::Msg(_)) => continue,
                None => Ok(None),
            };
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum ExecEvent {
    Stdout(Bytes),
    Stderr(Bytes),
    /// 命令结束：退出码；被信号结束时是信号编号；超时被结束时 timed_out 为真
    Exit { code: Option<i32>, signal: Option<i32>, timed_out: bool },
}

/// 一条正在对方电脑上跑的命令。命令的输入是空的；丢掉这个值会取消命令。
pub struct ExecStream {
    /// 留着不关：丢掉它流就被重置，对方据此结束命令
    _send: SendStream,
    recv: RecvStream,
    seen: bool,
}

impl ExecStream {
    /// 下一段输出或者退出码；收到 Exit 以后不要再调
    pub async fn next(&mut self) -> Result<ExecEvent> {
        loop {
            let frame = read_frame(&mut self.recv).await.map_err(|e| self.broken(Some(e)))?;
            match frame {
                Some(Frame::Data(d)) => {
                    self.seen = true;
                    match d.first() {
                        Some(&crate::exec::STDOUT) => return Ok(ExecEvent::Stdout(d.slice(1..))),
                        Some(&crate::exec::STDERR) => return Ok(ExecEvent::Stderr(d.slice(1..))),
                        _ => continue,
                    }
                }
                Some(Frame::Msg(Msg::ExecExit { code, signal, timed_out })) => return Ok(ExecEvent::Exit { code, signal, timed_out }),
                Some(Frame::Msg(Msg::Error { message })) => bail!("{message}"),
                Some(Frame::Msg(_)) => continue,
                None => return Err(self.broken(None)),
            }
        }
    }

    fn broken(&self, cause: Option<anyhow::Error>) -> anyhow::Error {
        if self.seen {
            anyhow!("连接中断，没收到命令的结果")
        } else {
            let cause = cause.map(|e| format!("（{e:#}）")).unwrap_or_default();
            anyhow!("对方没有回应，它的 P2PCLI 可能版本太旧、还不支持远程执行，请更新{cause}")
        }
    }
}

impl Drop for ExecStream {
    fn drop(&mut self) {
        // 明确重置发送端：对方在这条流上读到错误就知道要取消命令
        let _ = self._send.reset(0u32.into());
    }
}

#[cfg(test)]
mod collaboration_tests {
    use super::*;
    use crate::collaboration::model::{Action, Response, Role};
    use crate::host::{BoxFuture, Decision, HostDelegate, HostEvent, IncomingRequest};
    use crate::node::{Node, NodeConfig};

    struct Allow;

    impl HostDelegate for Allow {
        fn approve(&self, _: IncomingRequest) -> BoxFuture<Decision> {
            Box::pin(async { Decision::Allow { trust: false } })
        }
        fn event(&self, _: HostEvent) {}
    }

    fn config() -> NodeConfig {
        NodeConfig {
            data_dir: std::env::temp_dir().join(format!("p2pcli-collab-net-{:032x}", rand::random::<u128>())),
            offline: true,
            bind_ip: Some(std::net::Ipv4Addr::LOCALHOST.into()),
            ..Default::default()
        }
    }

    fn address(endpoint: &Endpoint) -> EndpointAddr {
        EndpointAddr::from_parts(
            endpoint.id(),
            endpoint.bound_sockets().into_iter().map(iroh::TransportAddr::Ip),
        )
    }

    fn response(reply: Reply) -> Response {
        assert!(reply.error.is_none(), "{:?}", reply.error);
        reply.response.expect("successful reply has a response")
    }

    #[tokio::test]
    async fn legacy_peer_receives_no_collaboration_stream() {
        let server = Endpoint::builder(iroh::endpoint::presets::N0)
            .alpns(vec![ALPN_TERM.to_vec()])
            .clear_address_lookup()
            .relay_mode(iroh::RelayMode::Disabled)
            .bind_addr((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap()
            .bind().await.unwrap();
        let client = Endpoint::builder(iroh::endpoint::presets::N0)
            .clear_address_lookup()
            .relay_mode(iroh::RelayMode::Disabled)
            .bind_addr((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap()
            .bind().await.unwrap();
        let server_endpoint = server.clone();
        let peer = tokio::spawn(async move {
            let conn = server_endpoint.accept().await.unwrap().await.unwrap();
            let (mut send, mut recv) = conn.accept_bi().await.unwrap();
            assert!(matches!(read_msg(&mut recv).await.unwrap(), Msg::Hello { .. }));
            crate::proto::write_json(&mut send, &serde_json::json!({
                "t": "welcome", "name": "legacy", "platform": "macos", "user": "test"
            })).await.unwrap();
            // The first subsequent stream must still be an existing operation.
            let (mut response, mut request) = conn.accept_bi().await.unwrap();
            assert_eq!(read_msg(&mut request).await.unwrap(), Msg::ListTerminals);
            write_msg(&mut response, &Msg::Terminals { sessions: vec![] }).await.unwrap();
            response.finish().unwrap();
            (conn, send, recv)
        });
        let remote = connect(&client, address(&server), &DeviceInfo::local(None), None).await.unwrap();
        let request = Request::new(None, Action::ProjectsList);
        let error = remote.collaborate(request.clone()).await.unwrap_err();
        assert!(error.to_string().contains("unsupported_capability"));
        assert!(remote.open_stream(&Msg::CollabRequest { request }).await.is_err());
        assert!(timeout(Duration::from_secs(5), remote.list_terminals()).await.unwrap().unwrap().is_empty());
        let _control = peer.await.unwrap();
        client.close().await;
        server.close().await;
    }

    #[tokio::test]
    async fn authenticated_collaboration_and_local_routing() {
        let host_config = config();
        let guest_config = config();
        let host = Node::start(host_config.clone(), Arc::new(Allow)).await.unwrap();
        let guest = Node::start(guest_config.clone(), Arc::new(Allow)).await.unwrap();
        let create = Request::new(None, Action::ProjectCreate { name: "network test".into() });
        let created = host.collaborate(None, create.clone()).await.unwrap();
        let project = match response(created.clone()) {
            Response::Project { project } => project,
            other => panic!("unexpected response: {other:?}"),
        };
        assert_eq!(project.coordinator_id, host.id().to_string());
        for target in ["", "  ", "local", &host.id().to_string()] {
            assert_eq!(host.collaborate(Some(target), create.clone()).await.unwrap(), created);
        }
        let local = host.local_remote().await;
        let read = Request::new(Some(project.id.clone()), Action::ProjectGet);
        assert_eq!(local.collaborate(read.clone()).await.unwrap(), created);

        let remote = Arc::new(guest.connect_addr(
            address(&host.endpoint), Some(host.host.password()),
        ).await.unwrap());
        let denied = remote.collaborate(read.clone()).await.unwrap();
        assert_eq!(denied.error.unwrap().code, "forbidden");
        response(host.collaborate(None, Request::new(Some(project.id.clone()), Action::MemberSet {
            device_id: guest.id().to_string(), role: Role::Viewer, revoked: false,
        })).await.unwrap());
        let read = Request::new(Some(project.id.clone()), Action::ProjectGet);
        assert_eq!(remote.collaborate(read.clone()).await.unwrap(), created);
        // Adopting a connection does not add a device-directory entry.
        guest.recent.upsert(crate::store::RecentDevice {
            code: "123456789".into(),
            endpoint_id: host.id().to_string(),
            name: remote.info.name.clone(),
            platform: remote.info.platform.clone(),
            sort_at: 0,
            last_used: crate::store::now_secs(),
        }).unwrap();
        guest.adopt_remote(remote.clone());
        assert_eq!(guest.collaborate(Some(&host.id().to_string()), read.clone()).await.unwrap(), created);

        // Revocation must apply to an already authenticated, reused connection.
        response(host.collaborate(None, Request::new(Some(project.id.clone()), Action::MemberSet {
            device_id: guest.id().to_string(), role: Role::Viewer, revoked: true,
        })).await.unwrap());
        let denied = remote.collaborate(read).await.unwrap();
        assert_eq!(denied.error.unwrap().code, "forbidden");
        assert!(!remote.is_closed());

        let updated = host.prepare_update(host_config.clone(), Arc::new(Allow)).await.unwrap();
        assert!(Arc::ptr_eq(&host.host.collaboration, &updated.host.collaboration));
        let epoch = host.host.collaboration.delivery_epoch.load(std::sync::atomic::Ordering::SeqCst);
        host.stop_rendezvous().await;
        assert_eq!(host.host.collaboration.delivery_epoch.load(std::sync::atomic::Ordering::SeqCst), epoch + 1);
        updated.activate();
        assert_eq!(updated.host.collaboration.delivery_epoch.load(std::sync::atomic::Ordering::SeqCst), epoch + 2);
        assert_eq!(updated.collaborate(None, create).await.unwrap(), created);
        guest.shutdown().await;
        updated.shutdown().await;
        let _ = std::fs::remove_dir_all(host_config.data_dir);
        let _ = std::fs::remove_dir_all(guest_config.data_dir);
    }

    #[tokio::test]
    async fn host_rejects_collaboration_without_negotiation() {
        let host_config = config();
        let host = Node::start(host_config.clone(), Arc::new(Allow)).await.unwrap();
        let client = Endpoint::builder(iroh::endpoint::presets::N0)
            .clear_address_lookup()
            .relay_mode(iroh::RelayMode::Disabled)
            .bind_addr((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap()
            .bind().await.unwrap();
        let conn = dial(&client, address(&host.endpoint)).await.unwrap();
        let (mut control_send, mut control_recv) = conn.open_bi().await.unwrap();
        write_msg(&mut control_send, &Msg::Hello {
            v: PROTO_VERSION, name: "legacy".into(), platform: "test".into(),
            password: Some(host.host.password()), version: None, os: None,
            capabilities: vec![],
        }).await.unwrap();
        let Msg::Welcome { capabilities, .. } = read_msg(&mut control_recv).await.unwrap() else {
            panic!("expected welcome");
        };
        assert!(capabilities.is_empty());
        let (mut send, mut recv) = conn.open_bi().await.unwrap();
        write_msg(&mut send, &Msg::CollabRequest {
            request: Request::new(None, Action::ProjectsList),
        }).await.unwrap();
        assert!(matches!(
            read_msg(&mut recv).await.unwrap(),
            Msg::Error { message } if message.contains("unsupported_capability")
        ));
        let (mut send, mut recv) = conn.open_bi().await.unwrap();
        write_msg(&mut send, &Msg::Exec {
            command: "exit 0".into(), cwd: None, timeout_secs: Some(5),
        }).await.unwrap();
        assert!(matches!(
            timeout(Duration::from_secs(10), read_msg(&mut recv)).await.unwrap().unwrap(),
            Msg::ExecExit { code: Some(0), timed_out: false, .. }
        ));
        client.close().await;
        host.shutdown().await;
        let _ = std::fs::remove_dir_all(host_config.data_dir);
    }

    #[tokio::test]
    async fn storage_initialization_failure_is_returned() {
        let config = config();
        std::fs::create_dir_all(config.data_dir.join("collaboration.sqlite")).unwrap();
        let result = Node::start(config.clone(), Arc::new(Allow)).await;
        let error = match result {
            Err(error) => error,
            Ok(node) => {
                node.shutdown().await;
                panic!("invalid database path must prevent startup");
            }
        };
        assert!(format!("{error:#}").contains("打开协作存储失败"));
        let _ = std::fs::remove_dir_all(config.data_dir);
    }
}
