//! p2pcli 命令行：让正在运行的 P2PCLI 发文件、列设备、看收到的文件。
//! 给人用，也给 AI 用：加 --json 输出好解析的结果；出错时退出码不是 0。

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Result, anyhow, bail};
use clap::{Parser, Subcommand};
use p2pcli_core::control::{self, Reply, Request};
use p2pcli_core::files::{InboxItem, Transfer, TransferState};
use p2pcli_core::node::KnownDevice;
use p2pcli_core::presence::PresenceState;

/// 等对方同意超过这么久才提示，配对过的设备一般马上就收
const WAITING_HINT_AFTER: Duration = Duration::from_millis(1500);

#[derive(Parser)]
#[command(name = "p2pcli", version, about = "让正在运行的 P2PCLI 发文件、列设备、看收到的文件")]
struct Cli {
    /// 输出 JSON，方便程序和 AI 读
    #[arg(long, global = true)]
    json: bool,
    /// P2PCLI 的数据目录，一般不用填；也可以用环境变量 P2PCLI_DATA_DIR
    #[arg(long, global = true, hide = true)]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 列出知道的设备：在不在线、延迟、发文件要不要对方同意
    Devices,
    /// 把文件或文件夹发给另一台设备，对方收好后打印文件在对方电脑上的路径
    Send {
        /// 设备名（可以只写一部分）、9 位设备码或公钥开头几位
        device: String,
        /// 要发的文件或文件夹，可以写多个
        #[arg(required = true)]
        paths: Vec<PathBuf>,
    },
    /// 看最近收到的文件
    Inbox {
        /// 最多列几条
        #[arg(short = 'n', long, default_value_t = 10)]
        limit: usize,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let json = cli.json;
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            if json {
                println!("{}", serde_json::json!({ "error": format!("{e:#}") }));
            } else {
                eprintln!("p2pcli：{e:#}");
            }
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
    let dir = cli.data_dir.or_else(control::data_dir).ok_or_else(|| anyhow!("找不到 P2PCLI 的数据目录"))?;
    match cli.cmd {
        Cmd::Devices => {
            let devices = ask(&dir, Request::Devices, |r| match r {
                Reply::Devices { devices } => Some(devices),
                _ => None,
            })
            .await?;
            if cli.json {
                print_json(&devices);
            } else {
                print_devices(&devices);
            }
        }
        Cmd::Inbox { limit } => {
            let (items, root) = ask(&dir, Request::Inbox { limit }, |r| match r {
                Reply::Inbox { items, dir } => Some((items, dir)),
                _ => None,
            })
            .await?;
            if cli.json {
                print_json(&serde_json::json!({ "dir": root, "items": items }));
            } else {
                print_inbox(&items, &root);
            }
        }
        Cmd::Send { device, paths } => send(&dir, device, paths, cli.json).await?,
    }
    Ok(())
}

/// 发一个请求，取第一条想要的回复；软件回了错误就报错
async fn ask<T>(dir: &Path, req: Request, pick: impl Fn(Reply) -> Option<T>) -> Result<T> {
    let (mut out, mut err) = (None, None);
    control::request(dir, &req, |reply| match reply {
        Reply::Error { message } => err = Some(message),
        other if out.is_none() => out = pick(other),
        _ => {}
    })
    .await?;
    if let Some(message) = err {
        bail!(message);
    }
    out.ok_or_else(|| anyhow!("P2PCLI 没有回结果"))
}

async fn send(dir: &Path, device: String, paths: Vec<PathBuf>, json: bool) -> Result<()> {
    // 软件的当前目录和这里不一样，先换成绝对路径；不存在的这里就报
    let mut absolute = Vec::with_capacity(paths.len());
    for p in &paths {
        let abs = std::path::absolute(p)?;
        if !abs.exists() {
            bail!("找不到 {}", p.display());
        }
        absolute.push(abs);
    }
    let tty = !json && std::io::stderr().is_terminal();
    let started = Instant::now();
    let (mut done, mut err) = (None::<Transfer>, None);
    let (mut waiting_since, mut hinted) = (None::<Instant>, false);
    control::request(dir, &Request::Send { target: device, paths: absolute }, |reply| match reply {
        Reply::Progress { transfer } => match transfer.state {
            TransferState::Waiting => {
                let since = *waiting_since.get_or_insert_with(Instant::now);
                if !json && !hinted && since.elapsed() >= WAITING_HINT_AFTER {
                    eprintln!("等 {} 同意接收…", transfer.peer_name);
                    hinted = true;
                }
            }
            TransferState::Running if tty => {
                eprint!("\r\x1b[K{}", progress_line(&transfer, started.elapsed()));
                let _ = std::io::stderr().flush();
            }
            _ => {}
        },
        Reply::Done { transfer } => done = Some(transfer),
        Reply::Error { message } => err = Some(message),
        _ => {}
    })
    .await?;
    if tty {
        eprint!("\r\x1b[K");
    }
    if let Some(message) = err {
        bail!(message);
    }
    let t = done.ok_or_else(|| anyhow!("P2PCLI 没有回结果"))?;
    if json {
        print_json(&t);
        if t.state != TransferState::Done {
            let _ = std::io::stdout().flush();
            std::process::exit(1);
        }
        return Ok(());
    }
    if t.state != TransferState::Done {
        bail!(t.error.unwrap_or_else(|| "发送失败".into()));
    }
    println!(
        "已发到 {}，{} 个文件，{}，用时 {:.1} 秒。在对方电脑上的位置：",
        t.peer_name,
        t.files,
        size(t.total),
        started.elapsed().as_secs_f32()
    );
    for p in &t.paths {
        println!("{p}");
    }
    Ok(())
}

fn progress_line(t: &Transfer, elapsed: Duration) -> String {
    let percent = (t.done * 100).checked_div(t.total).unwrap_or(100);
    let speed = t.done as f64 / elapsed.as_secs_f64().max(0.1);
    format!("发送中 {percent}%  {} / {}  {}/s", size(t.done), size(t.total), size(speed as u64))
}

fn print_devices(devices: &[KnownDevice]) {
    if devices.is_empty() {
        println!("还没有设备，先在 P2PCLI 里连一台");
        return;
    }
    for d in devices {
        let state = match d.state {
            Some(PresenceState::Online) => match &d.path {
                Some(p) => match p.rtt_ms {
                    Some(ms) => format!("在线 · {} · {ms} ms", p.kind.label()),
                    None => format!("在线 · {}", p.kind.label()),
                },
                None => "在线".into(),
            },
            Some(PresenceState::Offline) => "离线".into(),
            Some(PresenceState::OldVersion) => "在线，但版本太旧，要先更新".into(),
            Some(PresenceState::Checking) | None => "状态未知".into(),
        };
        let mut line = format!("{}  {}  {state}", d.name, platform(&d.platform));
        if d.lan {
            line.push_str(" · 局域网");
        }
        match d.paired {
            Some(true) => line.push_str("  发文件直接收"),
            Some(false) => line.push_str("  发文件可能要对方同意"),
            None => {}
        }
        if let Some(code) = &d.code {
            line.push_str(&format!("  设备码 {code}"));
        }
        line.push_str(&format!("  公钥 {}", &d.endpoint_id[..8.min(d.endpoint_id.len())]));
        println!("{line}");
    }
}

fn print_inbox(items: &[InboxItem], root: &str) {
    println!("收件目录：{root}");
    if items.is_empty() {
        println!("还没收到过文件");
    }
    for item in items {
        println!("{}  来自 {}  {} 个文件 {}", ago(item.at), item.from_name, item.files, size(item.total));
        for p in &item.paths {
            println!("  {p}");
        }
    }
}

fn print_json<T: serde::Serialize>(value: &T) {
    println!("{}", serde_json::to_string_pretty(value).unwrap_or_default());
}

fn platform(p: &str) -> &str {
    match p {
        "macos" => "macOS",
        "windows" => "Windows",
        "linux" => "Linux",
        other => other,
    }
}

fn size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

fn ago(at: u64) -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    match now.saturating_sub(at) {
        s if s < 60 => "刚刚".into(),
        s if s < 3600 => format!("{} 分钟前", s / 60),
        s if s < 86400 => format!("{} 小时前", s / 3600),
        s => format!("{} 天前", s / 86400),
    }
}
