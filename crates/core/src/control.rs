//! 本机控制口：命令行工具 p2pcli 通过它让正在运行的 P2PCLI 干活（列设备、发文件、看收件）。
//! Mac 和 Linux 是数据目录下的 Unix socket，权限只给本人；Windows 是按数据目录起名的命名管道。
//! 一问多答：命令行发一行 JSON 请求，软件回若干行 JSON，最后一行是结果或错误。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tracing::{debug, warn};

use crate::files::{InboxItem, Transfer};
use crate::node::{KnownDevice, Node};

/// 桌面端的应用标识，数据目录按它起名，要和 tauri.conf.json 里的 identifier 一致
pub const APP_IDENTIFIER: &str = "io.github.ilovebinglu.p2pcli";

/// 取当前在用的节点：桌面端改了设置会换节点，每个请求现取
pub type NodeSource = Arc<dyn Fn() -> Arc<Node> + Send + Sync>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Devices,
    /// 路径要是绝对路径，命令行和软件的当前目录不一样
    Send { target: String, paths: Vec<PathBuf> },
    Inbox { limit: usize },
    Transfers,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Reply {
    Devices { devices: Vec<KnownDevice> },
    /// 发文件的进度，最后一条换成 Done
    Progress { transfer: Transfer },
    Done { transfer: Transfer },
    Inbox { items: Vec<InboxItem>, dir: String },
    Transfers { transfers: Vec<Transfer> },
    Error { message: String },
}

/// 桌面端的数据目录；P2PCLI_DATA_DIR 可以指定别的（一台机器开两份测试时用）
pub fn data_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("P2PCLI_DATA_DIR") {
        return Some(PathBuf::from(dir));
    }
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    let base = if cfg!(target_os = "macos") {
        home()?.join("Library/Application Support")
    } else if cfg!(windows) {
        PathBuf::from(std::env::var_os("APPDATA")?)
    } else {
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| home().map(|h| h.join(".local/share")))?
    };
    Some(base.join(APP_IDENTIFIER))
}

#[cfg(unix)]
fn socket_path(data_dir: &Path) -> PathBuf {
    data_dir.join("control.sock")
}

#[cfg(windows)]
fn pipe_name(data_dir: &Path) -> String {
    let key = data_dir.to_string_lossy().to_lowercase();
    format!(r"\\.\pipe\p2pcli-{}", &blake3::hash(key.as_bytes()).to_hex()[..16])
}

/// 起本机控制口，在后台一直收请求
#[cfg(unix)]
pub async fn serve(data_dir: &Path, node: NodeSource) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let path = socket_path(data_dir);
    // 上次没删掉的 socket 文件：连得上说明还有一份在跑，不抢；连不上就删掉重建
    if path.exists() {
        if tokio::net::UnixStream::connect(&path).await.is_ok() {
            return Err(anyhow!("已经有一份 P2PCLI 在用这个数据目录"));
        }
        let _ = std::fs::remove_file(&path);
    }
    let listener = tokio::net::UnixListener::bind(&path).with_context(|| format!("起控制口失败：{}", path.display()))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let node = node.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle(stream, node).await {
                            debug!("控制口请求出错：{e:#}");
                        }
                    });
                }
                Err(e) => {
                    warn!("控制口出错：{e}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    });
    Ok(())
}

/// 起本机控制口，在后台一直收请求
#[cfg(windows)]
pub async fn serve(data_dir: &Path, node: NodeSource) -> Result<()> {
    use tokio::net::windows::named_pipe::ServerOptions;
    let name = pipe_name(data_dir);
    let mut server = ServerOptions::new()
        .first_pipe_instance(true)
        .create(&name)
        .context("起控制口失败，可能已经有一份 P2PCLI 在用这个数据目录")?;
    tokio::spawn(async move {
        loop {
            let connected = server.connect().await;
            // 先建好下一个管道再处理这一个，中间来的连接不会扑空
            let next = match ServerOptions::new().create(&name) {
                Ok(next) => next,
                Err(e) => return warn!("控制口建不了新管道：{e}"),
            };
            let client = std::mem::replace(&mut server, next);
            if let Err(e) = connected {
                debug!("控制口连接失败：{e}");
                continue;
            }
            let node = node.clone();
            tokio::spawn(async move {
                if let Err(e) = handle(client, node).await {
                    debug!("控制口请求出错：{e:#}");
                }
            });
        }
    });
    Ok(())
}

async fn handle<S: AsyncRead + AsyncWrite + Unpin>(stream: S, node: NodeSource) -> Result<()> {
    let (r, mut w) = tokio::io::split(stream);
    let mut line = String::new();
    BufReader::new(r).take(1 << 20).read_line(&mut line).await?;
    let req: Request = match serde_json::from_str(line.trim()) {
        Ok(req) => req,
        Err(e) => {
            let message = format!("请求看不懂：{e}。命令行和 P2PCLI 的版本可能不一致");
            return send(&mut w, &Reply::Error { message }).await;
        }
    };
    let node = node();
    match req {
        Request::Devices => send(&mut w, &Reply::Devices { devices: node.known_devices() }).await?,
        Request::Inbox { limit } => {
            let dir = node.files.inbox_root().display().to_string();
            send(&mut w, &Reply::Inbox { items: node.files.inbox(limit), dir }).await?
        }
        Request::Transfers => send(&mut w, &Reply::Transfers { transfers: node.files.transfers() }).await?,
        Request::Send { target, paths } => {
            let id = match node.find_device(&target).and_then(|d| node.start_send(&d, paths)) {
                Ok(id) => id,
                Err(e) => return send(&mut w, &Reply::Error { message: format!("{e:#}") }).await,
            };
            let mut last = None;
            let mut sent_at = std::time::Instant::now();
            loop {
                let Some(t) = node.files.transfer(&id) else {
                    return send(&mut w, &Reply::Error { message: "传输记录不见了".into() }).await;
                };
                if t.finished() {
                    send(&mut w, &Reply::Done { transfer: t }).await?;
                    break;
                }
                // 有变化就报；没变化每秒也报一次，命令行好知道还在等对方同意
                if last.as_ref() != Some(&t) || sent_at.elapsed() >= Duration::from_secs(1) {
                    // 命令行中途走了也不停，文件照发
                    send(&mut w, &Reply::Progress { transfer: t.clone() }).await?;
                    last = Some(t);
                    sent_at = std::time::Instant::now();
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
    }
    let _ = w.shutdown().await;
    Ok(())
}

async fn send<W: AsyncWrite + Unpin>(w: &mut W, reply: &Reply) -> Result<()> {
    let mut line = serde_json::to_vec(reply)?;
    line.push(b'\n');
    w.write_all(&line).await?;
    w.flush().await?;
    Ok(())
}

/// 命令行这头：发一个请求，每收到一行回复调一次 on_reply
pub async fn request(data_dir: &Path, req: &Request, mut on_reply: impl FnMut(Reply)) -> Result<()> {
    let stream = connect(data_dir).await?;
    let (r, mut w) = tokio::io::split(stream);
    let mut line = serde_json::to_vec(req)?;
    line.push(b'\n');
    w.write_all(&line).await?;
    w.flush().await?;
    let mut lines = BufReader::new(r).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let reply = serde_json::from_str(&line).context("P2PCLI 回的消息看不懂，命令行和 P2PCLI 的版本可能不一致")?;
        on_reply(reply);
    }
    Ok(())
}

fn not_running() -> anyhow::Error {
    anyhow!("P2PCLI 没在运行，先打开 P2PCLI")
}

#[cfg(unix)]
async fn connect(data_dir: &Path) -> Result<tokio::net::UnixStream> {
    tokio::net::UnixStream::connect(socket_path(data_dir)).await.map_err(|_| not_running())
}

#[cfg(windows)]
async fn connect(data_dir: &Path) -> Result<tokio::net::windows::named_pipe::NamedPipeClient> {
    use tokio::net::windows::named_pipe::ClientOptions;
    /// 管道都在忙
    const ERROR_PIPE_BUSY: i32 = 231;
    let name = pipe_name(data_dir);
    for _ in 0..50 {
        match ClientOptions::new().open(&name) {
            Ok(client) => return Ok(client),
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => tokio::time::sleep(Duration::from_millis(100)).await,
            Err(_) => return Err(not_running()),
        }
    }
    Err(anyhow!("P2PCLI 忙不过来，稍后再试"))
}
