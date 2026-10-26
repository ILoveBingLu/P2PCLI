//! 连接方：连上对方、开终端、收发字节。

use std::fmt;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use bytes::Bytes;
use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::{Endpoint, EndpointAddr};
use serde::Serialize;
use tokio::time::timeout;

use crate::host::APPROVE_TIMEOUT;
use crate::identity::DeviceInfo;
use crate::path::{self, PathInfo};
use crate::proto::{Frame, Msg, RejectCode, TerminalSession, read_frame, read_msg, write_data, write_msg};
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

/// 一台已经同意连接的对方设备
pub struct RemoteHost {
    conn: Connection,
    pub info: RemoteInfo,
    /// 控制流一直开着；关掉它对面就当我们走了
    _control: SendStream,
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
        } => Ok(RemoteHost {
            conn,
            info: RemoteInfo {
                endpoint_id,
                name,
                platform,
                user,
                version,
                os,
                multiplexer,
            },
            _control: send,
        }),
        Msg::Reject { code } => {
            conn.close(0u32.into(), b"rejected");
            Err(ConnectError::Rejected(code))
        }
        other => Err(anyhow!("对方回了意外的消息：{other:?}").into()),
    }
}

impl RemoteHost {
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
        let (mut send, mut recv) = self.conn.open_bi().await?;
        write_msg(&mut send, &Msg::Open { persistent, cols, rows, session_id }).await?;
        match timeout(Duration::from_secs(35), read_msg(&mut recv)).await?? {
            Msg::Opened { shell, session_id } => Ok((TermInput { send }, TermOutput { recv, shell, session_id })),
            Msg::Error { message } => bail!("对方开终端失败：{message}"),
            other => bail!("意外的回复：{other:?}"),
        }
    }

    pub async fn list_terminals(&self) -> Result<Vec<TerminalSession>> {
        match self.session_request(Msg::ListTerminals).await? {
            Msg::Terminals { sessions } => Ok(sessions),
            other => bail!("读取会话失败：{other:?}"),
        }
    }

    pub async fn end_terminal(&self, session_id: String) -> Result<()> {
        match self.session_request(Msg::EndTerminal { session_id }).await? {
            Msg::TerminalEnded => Ok(()),
            other => bail!("结束会话失败：{other:?}"),
        }
    }

    pub async fn installed_agents(&self, refresh: bool) -> Result<Vec<crate::launchers::InstalledAgent>> {
        match self.session_request(Msg::ListInstalledAgents { refresh }).await? {
            Msg::InstalledAgents { agents } => Ok(agents),
            other => bail!("检测 AI 失败（对方可能需要更新）：{other:?}"),
        }
    }

    pub async fn launch_agent(&self, session_id: String, kind: String, allow_bypass: bool) -> Result<()> {
        match self.session_request(Msg::LaunchAgent { session_id, kind, allow_bypass }).await? {
            Msg::AgentLaunched => Ok(()),
            other => bail!("启动 AI 失败：{other:?}"),
        }
    }

    pub async fn choose_agent_option(&self, session_id: String, approval_id: String, option_id: usize) -> Result<()> {
        match self.session_request(Msg::ChooseAgentOption { session_id, approval_id, option_id }).await? {
            Msg::AgentOptionChosen => Ok(()),
            other => bail!("提交选项失败：{other:?}"),
        }
    }

    async fn session_request(&self, msg: Msg) -> Result<Msg> {
        // 超时要覆盖整个请求，不能只包住 read：open_bi/write 挂住同样会让前端一直等待。
        timeout(Duration::from_secs(20), async {
            let (mut send, mut recv) = self.conn.open_bi().await?;
            write_msg(&mut send, &msg).await?;
            let reply = read_msg(&mut recv).await?;
            if let Msg::Error { message } = reply { bail!("{message}"); }
            Ok(reply)
        })
        .await
        .map_err(|_| anyhow!("会话请求超时"))?
    }

    /// 开一条新流并先发一条控制消息，后面怎么走由这条消息决定（跑命令、拉文件、推文件）
    pub async fn open_stream(&self, first: &Msg) -> Result<(SendStream, RecvStream)> {
        timeout(Duration::from_secs(20), async {
            let (mut send, recv) = self.conn.open_bi().await?;
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

    /// 连接已经断了
    pub fn is_closed(&self) -> bool {
        self.conn.close_reason().is_some()
    }

    pub fn path(&self) -> PathInfo {
        path::current(&self.conn)
    }

    pub fn stats(&self, endpoint: &iroh::Endpoint) -> path::ConnStats {
        path::stats(&self.conn, endpoint)
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    pub fn close(&self) {
        self.conn.close(0u32.into(), b"bye");
    }

    /// 等连接断开，返回原因
    pub async fn closed(&self) -> String {
        describe_close(self.conn.closed().await)
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

pub struct TermInput {
    send: SendStream,
}

impl TermInput {
    pub async fn write(&mut self, data: &[u8]) -> Result<()> {
        write_data(&mut self.send, data).await
    }

    pub async fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        write_msg(&mut self.send, &Msg::Resize { cols, rows }).await
    }

    /// 关闭当前附着；后台会话继续运行。
    pub fn close(&mut self) {
        let _ = self.send.finish();
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
    recv: RecvStream,
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
