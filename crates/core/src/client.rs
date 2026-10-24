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
        } => Ok(RemoteHost {
            conn,
            info: RemoteInfo {
                endpoint_id,
                name,
                platform,
                user,
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

    async fn session_request(&self, msg: Msg) -> Result<Msg> {
        let (mut send, mut recv) = self.conn.open_bi().await?;
        write_msg(&mut send, &msg).await?;
        let reply = timeout(Duration::from_secs(20), read_msg(&mut recv)).await??;
        if let Msg::Error { message } = reply { bail!("{message}"); }
        Ok(reply)
    }

    pub fn path(&self) -> PathInfo {
        path::current(&self.conn)
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
