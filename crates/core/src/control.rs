//! 本机控制口：命令行工具 p2pcli 通过它让正在运行的 P2PCLI 干活（列设备、发文件、拉文件、在对方电脑上跑命令、看收件）。
//! Mac 和 Linux 是数据目录下的 Unix socket，权限只给本人；Windows 是按数据目录起名的命名管道。
//! 一问多答：命令行发一行 JSON 请求，软件回若干行 JSON，最后一行是结果或错误。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tracing::{debug, warn};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;

use crate::client::ExecEvent;
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
    /// 路径要是绝对路径，命令行和软件的当前目录不一样。
    /// to 是对方电脑上的目录：写了就放到那里（同名盖掉），不写放进对方的收件目录
    Send {
        target: String,
        paths: Vec<PathBuf>,
        #[serde(default)]
        to: Option<String>,
    },
    /// 把对方电脑上的文件拉到本机的 to 目录（绝对路径，同名盖掉）；paths 是对方电脑上的路径
    Pull { target: String, paths: Vec<String>, to: PathBuf },
    /// 在对方电脑上跑一条命令；命令行断开就取消它
    Exec {
        target: String,
        command: String,
        #[serde(default)]
        cwd: Option<String>,
        #[serde(default)]
        timeout_secs: Option<u64>,
    },
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
    /// 还在连对方（对方可能要点同意），每秒报一次，命令行好给出提示
    Status { message: String },
    /// 命令的一段输出，原始字节的 base64
    ExecOut { stderr: bool, data: String },
    /// 命令结束了
    ExecDone {
        code: Option<i32>,
        signal: Option<i32>,
        timed_out: bool,
    },
    Error { message: String },
}

/// 命令输出的一段，装进 JSON 的样子
pub fn encode_output(data: &[u8]) -> String {
    B64.encode(data)
}

pub fn decode_output(data: &str) -> Vec<u8> {
    B64.decode(data).unwrap_or_default()
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
    let mut reader = BufReader::new(r);
    let mut line = String::new();
    (&mut reader).take(1 << 20).read_line(&mut line).await?;
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
        Request::Send { target, paths, to } => {
            let started = node.find_device(&target).and_then(|d| match to {
                Some(dest) => node.start_put(&d, paths, dest),
                None => node.start_send(&d, paths),
            });
            match started {
                Ok(id) => follow_transfer(&node, &id, &mut w).await?,
                Err(e) => return send(&mut w, &Reply::Error { message: format!("{e:#}") }).await,
            }
        }
        Request::Pull { target, paths, to } => match node.find_device(&target).and_then(|d| node.start_pull(&d, paths, to)) {
            Ok(id) => follow_transfer(&node, &id, &mut w).await?,
            Err(e) => return send(&mut w, &Reply::Error { message: format!("{e:#}") }).await,
        },
        Request::Exec { target, command, cwd, timeout_secs } => {
            exec(&node, &mut reader, &mut w, &target, &command, cwd, timeout_secs).await?
        }
    }
    let _ = w.shutdown().await;
    Ok(())
}

/// 盯一次传输到结束，进度一行行报给命令行
async fn follow_transfer<W: AsyncWrite + Unpin>(node: &Node, id: &str, w: &mut W) -> Result<()> {
    let mut last = None;
    let mut sent_at = std::time::Instant::now();
    loop {
        let Some(t) = node.files.transfer(id) else {
            return send(w, &Reply::Error { message: "传输记录不见了".into() }).await;
        };
        if t.finished() {
            return send(w, &Reply::Done { transfer: t }).await;
        }
        // 有变化就报；没变化每秒也报一次，命令行好知道还在等对方同意
        if last.as_ref() != Some(&t) || sent_at.elapsed() >= Duration::from_secs(1) {
            // 命令行中途走了也不停，文件照发
            send(w, &Reply::Progress { transfer: t.clone() }).await?;
            last = Some(t);
            sent_at = std::time::Instant::now();
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// 在对方电脑上跑命令，输出随到随转。命令行断开（读端读到头）就取消：丢掉流，对方结束命令
async fn exec<R, W>(node: &Arc<Node>, reader: &mut R, w: &mut W, target: &str, command: &str, cwd: Option<String>, timeout_secs: Option<u64>) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let device = match node.find_device(target) {
        Ok(d) => d,
        Err(e) => return send(w, &Reply::Error { message: format!("{e:#}") }).await,
    };
    // 连对方可能要等它点同意，期间每秒报一次
    let connect = node.remote_for(&device);
    tokio::pin!(connect);
    let remote = loop {
        tokio::select! {
            r = &mut connect => break r,
            _ = tokio::time::sleep(Duration::from_secs(1)) => {
                send(w, &Reply::Status { message: format!("正在连接 {}", device.name) }).await?;
            }
        }
    };
    let remote = match remote {
        Ok(r) => r,
        Err(e) => return send(w, &Reply::Error { message: format!("{e:#}") }).await,
    };
    let mut stream = match remote.exec(command, cwd, timeout_secs).await {
        Ok(s) => s,
        Err(e) => return send(w, &Reply::Error { message: format!("{e:#}") }).await,
    };
    let mut gone = std::pin::pin!(wait_closed(reader));
    loop {
        let event = tokio::select! {
            event = stream.next() => event,
            _ = &mut gone => {
                debug!("命令行走了，取消远程命令");
                return Ok(());
            }
        };
        match event {
            Ok(ExecEvent::Stdout(d)) => send(w, &Reply::ExecOut { stderr: false, data: encode_output(&d) }).await?,
            Ok(ExecEvent::Stderr(d)) => send(w, &Reply::ExecOut { stderr: true, data: encode_output(&d) }).await?,
            Ok(ExecEvent::Exit { code, signal, timed_out }) => {
                return send(w, &Reply::ExecDone { code, signal, timed_out }).await;
            }
            Err(e) => return send(w, &Reply::Error { message: format!("{e:#}") }).await,
        }
    }
}

/// 等对面关掉：命令行发完请求以后不会再发东西，读到头或出错就是它走了
async fn wait_closed<R: AsyncRead + Unpin>(r: &mut R) {
    let mut buf = [0u8; 64];
    loop {
        match r.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
    }
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
    request_while(data_dir, req, |reply| {
        on_reply(reply);
        true
    })
    .await
}

/// 同上，但 on_reply 返回 false 就不再等了，直接断开（软件那头据此取消还在跑的命令）
pub async fn request_while(data_dir: &Path, req: &Request, mut on_reply: impl FnMut(Reply) -> bool) -> Result<()> {
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
        if !on_reply(reply) {
            break;
        }
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
