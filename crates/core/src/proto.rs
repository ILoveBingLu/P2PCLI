//! 帧格式：1 字节类型 + 4 字节长度（大端）+ 内容。
//! 类型 1 是 JSON 控制消息，类型 2 是终端原始字节（输入或输出）。

use anyhow::{Context, Result, bail};
use bytes::Bytes;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const KIND_MSG: u8 = 1;
const KIND_DATA: u8 = 2;
/// 单帧上限，防对面发超大长度把内存吃光
pub const MAX_FRAME: usize = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectCode {
    /// 没带密码，也不是受信设备
    NeedPassword,
    BadPassword,
    /// 对方在弹窗里点了拒绝
    Denied,
    /// 弹窗没人点
    Timeout,
    /// 密码错太多次，暂时锁住
    Locked,
    Version,
}

impl RejectCode {
    pub fn describe(self) -> &'static str {
        match self {
            RejectCode::NeedPassword => "需要输入临时密码",
            RejectCode::BadPassword => "临时密码不对",
            RejectCode::Denied => "对方拒绝了连接",
            RejectCode::Timeout => "对方没有响应连接请求",
            RejectCode::Locked => "密码错误次数太多，请稍后再试",
            RejectCode::Version => "两边软件版本不兼容，请更新",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Msg {
    /// 连接方开场：我是谁，带的密码
    Hello {
        v: u32,
        name: String,
        platform: String,
        password: Option<String>,
    },
    /// 被连方同意
    Welcome {
        name: String,
        platform: String,
        user: String,
    },
    Reject {
        code: RejectCode,
    },
    /// 开一个终端
    Open {
        persistent: bool,
        session_id: Option<String>,
        cols: u16,
        rows: u16,
    },
    Opened {
        shell: String,
        session_id: String,
    },
    ListTerminals,
    Terminals { sessions: Vec<TerminalSession> },
    EndTerminal { session_id: String },
    TerminalEnded,
    Resize {
        cols: u16,
        rows: u16,
    },
    /// 终端里的程序退出了
    Exit {
        code: Option<u32>,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TerminalSession {
    pub id: String,
    pub created_at: u64,
    pub attached: bool,
}

#[derive(Debug, PartialEq)]
pub enum Frame {
    Msg(Msg),
    Data(Bytes),
}

pub async fn write_msg<W: AsyncWrite + Unpin>(w: &mut W, msg: &Msg) -> Result<()> {
    write_json(w, msg).await
}

/// 牵线协议也用同一种帧，只是消息类型不同
pub async fn write_json<W: AsyncWrite + Unpin, T: Serialize>(w: &mut W, value: &T) -> Result<()> {
    let body = serde_json::to_vec(value)?;
    write_frame(w, KIND_MSG, &body).await
}

pub async fn read_json<R: AsyncRead + Unpin, T: DeserializeOwned>(r: &mut R) -> Result<T> {
    let mut head = [0u8; 5];
    r.read_exact(&mut head).await.context("连接已关闭")?;
    let len = u32::from_be_bytes(head[1..].try_into().unwrap()) as usize;
    if head[0] != KIND_MSG || len > MAX_FRAME {
        bail!("帧格式不对");
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await?;
    Ok(serde_json::from_slice(&body)?)
}

pub async fn write_data<W: AsyncWrite + Unpin>(w: &mut W, data: &[u8]) -> Result<()> {
    write_frame(w, KIND_DATA, data).await
}

async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, kind: u8, body: &[u8]) -> Result<()> {
    if body.len() > MAX_FRAME {
        bail!("帧太大：{} 字节", body.len());
    }
    let mut head = [0u8; 5];
    head[0] = kind;
    head[1..].copy_from_slice(&(body.len() as u32).to_be_bytes());
    w.write_all(&head).await?;
    w.write_all(body).await?;
    Ok(())
}

/// 读一帧；对面正常关流返回 None
pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> Result<Option<Frame>> {
    let mut head = [0u8; 5];
    match r.read_exact(&mut head).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_be_bytes(head[1..].try_into().unwrap()) as usize;
    if len > MAX_FRAME {
        bail!("帧太大：{len} 字节");
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await.context("帧内容没读完连接就断了")?;
    match head[0] {
        KIND_MSG => Ok(Some(Frame::Msg(serde_json::from_slice(&body)?))),
        KIND_DATA => Ok(Some(Frame::Data(Bytes::from(body)))),
        k => bail!("未知帧类型 {k}"),
    }
}

/// 读一条控制消息，读到原始字节或流结束都算错
pub async fn read_msg<R: AsyncRead + Unpin>(r: &mut R) -> Result<Msg> {
    match read_frame(r).await? {
        Some(Frame::Msg(m)) => Ok(m),
        Some(Frame::Data(_)) => bail!("这里应该是控制消息，收到的是终端数据"),
        None => bail!("连接已关闭"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn roundtrip() {
        let (mut a, mut b) = tokio::io::duplex(64 * 1024);
        let hello = Msg::Hello {
            v: 1,
            name: "测试机".into(),
            platform: "macos".into(),
            password: Some("abc123".into()),
        };
        write_msg(&mut a, &hello).await.unwrap();
        write_data(&mut a, "你好\r\n".as_bytes()).await.unwrap();
        drop(a);
        assert_eq!(read_frame(&mut b).await.unwrap(), Some(Frame::Msg(hello)));
        assert_eq!(
            read_frame(&mut b).await.unwrap(),
            Some(Frame::Data(Bytes::from("你好\r\n")))
        );
        assert_eq!(read_frame(&mut b).await.unwrap(), None);
    }

    #[tokio::test]
    async fn rejects_oversized() {
        let (mut a, mut b) = tokio::io::duplex(64);
        tokio::spawn(async move {
            let mut head = [KIND_DATA, 0, 0, 0, 0];
            head[1..].copy_from_slice(&((MAX_FRAME as u32) + 1).to_be_bytes());
            a.write_all(&head).await.unwrap();
        });
        assert!(read_frame(&mut b).await.is_err());
    }
}
