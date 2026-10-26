//! p2pcli 命令行：让正在运行的 P2PCLI 发文件、拉文件、在别的设备上跑命令、列设备、看收到的文件。
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
#[command(name = "p2pcli", version, about = "让正在运行的 P2PCLI 发文件、拉文件、在别的设备上跑命令、列设备、看收到的文件")]
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
        /// 放到对方电脑上的这个目录（不存在会建，同名文件会被盖掉）；不写就放进对方的收件目录。
        /// 要对方点过同意、勾了受信，或者这边连过它并记住了密码
        #[arg(long)]
        to: Option<String>,
    },
    /// 把另一台设备上的文件或文件夹拉到本机，同名文件会被盖掉
    Pull {
        /// 设备名（可以只写一部分）、9 位设备码或公钥开头几位
        device: String,
        /// 对方电脑上的路径，可以写多个；`~` 开头或相对路径从对方的用户目录算起
        #[arg(required = true)]
        paths: Vec<String>,
        /// 放到本机的这个目录，不存在会建；不写就是当前目录
        #[arg(long)]
        to: Option<PathBuf>,
    },
    /// 在另一台设备上跑一条命令，输出原样回来，退出码就是命令的退出码（超时 124，P2PCLI 自己出错 255）。
    /// 命令交给对方的 shell 去解析，要引号自己写好，用法同 ssh。命令的输入是空的，不能交互
    Exec {
        /// 设备名（可以只写一部分）、9 位设备码或公钥开头几位
        device: String,
        /// 命令在对方电脑上的哪个目录跑，默认是对方的用户目录
        #[arg(long)]
        cwd: Option<String>,
        /// 跑超过这么多秒就结束它
        #[arg(long)]
        timeout: Option<u64>,
        /// 要跑的命令，前面加 -- 隔开
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
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
    // 跑命令时 p2pcli 自己出错用 255，和 ssh 一样，免得和命令自己的退出码混起来
    let failed = if matches!(cli.cmd, Cmd::Exec { .. }) { ExitCode::from(255) } else { ExitCode::FAILURE };
    match run(cli).await {
        Ok(code) => code,
        Err(e) => {
            if json {
                println!("{}", serde_json::json!({ "error": format!("{e:#}") }));
            } else {
                eprintln!("p2pcli：{e:#}");
            }
            failed
        }
    }
}

async fn run(cli: Cli) -> Result<ExitCode> {
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
        Cmd::Send { device, paths, to } => {
            // 软件的当前目录和这里不一样，先换成绝对路径；不存在的这里就报
            let mut absolute = Vec::with_capacity(paths.len());
            for p in &paths {
                let abs = std::path::absolute(p)?;
                if !abs.exists() {
                    bail!("找不到 {}", p.display());
                }
                absolute.push(abs);
            }
            transfer(&dir, Request::Send { target: device, paths: absolute, to }, Verb::Send, cli.json).await?
        }
        Cmd::Pull { device, paths, to } => {
            let to = std::path::absolute(to.unwrap_or_else(|| PathBuf::from(".")))?;
            transfer(&dir, Request::Pull { target: device, paths, to }, Verb::Pull, cli.json).await?
        }
        Cmd::Exec { device, cwd, timeout, command } => {
            let request = Request::Exec { target: device, command: command.join(" "), cwd, timeout_secs: timeout };
            return exec(&dir, request, cli.json).await;
        }
    }
    Ok(ExitCode::SUCCESS)
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

#[derive(Clone, Copy, PartialEq)]
enum Verb {
    Send,
    Pull,
}

/// 发文件或拉文件：盯着进度，结束时打印结果
async fn transfer(dir: &Path, request: Request, verb: Verb, json: bool) -> Result<()> {
    let tty = !json && std::io::stderr().is_terminal();
    let started = Instant::now();
    let (mut done, mut err) = (None::<Transfer>, None);
    let (mut waiting_since, mut hinted) = (None::<Instant>, false);
    control::request(dir, &request, |reply| match reply {
        Reply::Progress { transfer } => match transfer.state {
            // 连接中也提示：用终端连接认证的操作，对方可能要点同意
            TransferState::Waiting | TransferState::Connecting => {
                let since = *waiting_since.get_or_insert_with(Instant::now);
                if !json && !hinted && since.elapsed() >= WAITING_HINT_AFTER {
                    eprintln!("等 {} 同意…", transfer.peer_name);
                    hinted = true;
                }
            }
            TransferState::Running if tty => {
                eprint!("\r\x1b[K{}", progress_line(&transfer, started.elapsed(), verb));
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
        bail!(t.error.unwrap_or_else(|| if verb == Verb::Pull { "拉取失败".into() } else { "发送失败".into() }));
    }
    match verb {
        Verb::Send => println!(
            "已发到 {}，{} 个文件，{}，用时 {:.1} 秒。在对方电脑上的位置：",
            t.peer_name,
            t.files,
            size(t.total),
            started.elapsed().as_secs_f32()
        ),
        Verb::Pull => println!(
            "已从 {} 拉到本机，{} 个文件，{}，用时 {:.1} 秒。在本机的位置：",
            t.peer_name,
            t.files,
            size(t.total),
            started.elapsed().as_secs_f32()
        ),
    }
    for p in &t.paths {
        println!("{p}");
    }
    Ok(())
}

/// 在对方电脑上跑命令：输出随到随打印，退出码带回来
async fn exec(dir: &Path, request: Request, json: bool) -> Result<ExitCode> {
    let (mut stdout, mut stderr) = (std::io::stdout(), std::io::stderr());
    let (mut out_buf, mut err_buf) = (Vec::<u8>::new(), Vec::<u8>::new());
    let (mut done, mut err, mut write_err, mut hinted) = (None, None, None::<std::io::Error>, false);
    control::request_while(dir, &request, |reply| match reply {
        Reply::Status { message } => {
            if !json && !hinted {
                eprintln!("{message}…（对方可能要点同意）");
                hinted = true;
            }
            true
        }
        Reply::ExecOut { stderr: is_err, data } => {
            let bytes = control::decode_output(&data);
            if json {
                if is_err { err_buf.extend_from_slice(&bytes) } else { out_buf.extend_from_slice(&bytes) }
                return true;
            }
            let written = if is_err {
                stderr.write_all(&bytes).and_then(|_| stderr.flush())
            } else {
                stdout.write_all(&bytes).and_then(|_| stdout.flush())
            };
            // 写不出去（比如管道另一头关了）就别再等了，断开让对方也停掉命令
            match written {
                Ok(()) => true,
                Err(e) => {
                    write_err = Some(e);
                    false
                }
            }
        }
        Reply::ExecDone { code, signal, timed_out } => {
            done = Some((code, signal, timed_out));
            true
        }
        Reply::Error { message } => {
            err = Some(message);
            true
        }
        _ => true,
    })
    .await?;
    if let Some(e) = write_err {
        bail!("输出写不出去：{e}");
    }
    if let Some(message) = err {
        bail!(message);
    }
    let (code, signal, timed_out) = done.ok_or_else(|| anyhow!("P2PCLI 没有回结果，命令可能没跑完"))?;
    if json {
        print_json(&serde_json::json!({
            "stdout": String::from_utf8_lossy(&out_buf),
            "stderr": String::from_utf8_lossy(&err_buf),
            "code": code,
            "signal": signal,
            "timed_out": timed_out,
        }));
    } else if timed_out {
        eprintln!("p2pcli：命令超时，已被结束");
    }
    Ok(ExitCode::from(exit_code(code, signal, timed_out)))
}

/// 命令的退出情况换成本进程的退出码：超时 124，被信号结束 128 加信号编号，其余原样
fn exit_code(code: Option<i32>, signal: Option<i32>, timed_out: bool) -> u8 {
    if timed_out {
        return 124;
    }
    match (code, signal) {
        (Some(c), _) if (0..=255).contains(&c) => c as u8,
        (Some(_), _) => 1,
        (None, Some(s)) => (128 + s).clamp(1, 254) as u8,
        (None, None) => 1,
    }
}

fn progress_line(t: &Transfer, elapsed: Duration, verb: Verb) -> String {
    let percent = (t.done * 100).checked_div(t.total).unwrap_or(100);
    let speed = t.done as f64 / elapsed.as_secs_f64().max(0.1);
    let what = if verb == Verb::Pull { "拉取中" } else { "发送中" };
    format!("{what} {percent}%  {} / {}  {}/s", size(t.done), size(t.total), size(speed as u64))
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
