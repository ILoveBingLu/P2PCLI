//! 开发测试用命令行：一台跑 host，另一台 connect 过去跑一条命令。

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use p2pcli_core::client::{ConnectError, RemoteHost, TermEvent, asks_cursor_position};
use p2pcli_core::host::{BoxFuture, Decision, HostDelegate, HostEvent, IncomingRequest};
use p2pcli_core::node::{Node, NodeConfig};
use p2pcli_core::rendezvous::{format_code, normalize_code, parse_server};

#[derive(Parser)]
struct Cli {
    #[arg(long, global = true, default_value = "./p2pcli-dev-data")]
    data_dir: PathBuf,
    /// 牵线服务器：<公钥>@<ip:端口> 走 UDP，wss://域名 走 WebSocket；传空字符串表示不用
    #[arg(long, global = true, default_value = p2pcli_core::rendezvous::DEFAULT_SERVER)]
    server: String,
    #[arg(long, global = true)]
    name: Option<String>,
    /// 只走中继，测兜底
    #[arg(long, global = true)]
    relay_only: bool,
    /// 只绑这个本机 IP，测公网打洞时用来绕开隧道
    #[arg(long, global = true)]
    bind_ip: Option<std::net::IpAddr>,
    /// 自建中继地址，可写多个
    #[arg(long, global = true)]
    relay: Vec<String>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, ValueEnum)]
enum Approve {
    /// 在命令行里问
    Ask,
    Yes,
    No,
    Trust,
}

#[derive(Subcommand)]
enum Cmd {
    /// 等别人来连
    Host {
        #[arg(long)]
        password: Option<String>,
        #[arg(long, value_enum, default_value = "ask")]
        approve: Approve,
    },
    /// 网络诊断：IPv6、NAT 类型、中继
    Diag,
    /// 连过去跑一条命令，打印输出，退出码跟对面一致
    Connect {
        /// 9 位设备码，或 <公钥>@<ip:端口>
        target: String,
        #[arg(long)]
        password: Option<String>,
        #[arg(long)]
        run: String,
        /// 跑完后再观察多少秒线路（看打洞有没有从中继切到直连）
        #[arg(long, default_value_t = 0)]
        watch: u64,
    },
}

struct CliDelegate {
    approve: Approve,
}

impl HostDelegate for CliDelegate {
    fn approve(&self, req: IncomingRequest) -> BoxFuture<Decision> {
        eprintln!("【连接请求】{}（{}）{}", req.name, req.platform, req.remote);
        let approve = self.approve;
        Box::pin(async move {
            let d = match approve {
                Approve::Yes => Decision::Allow { trust: false },
                Approve::Trust => Decision::Allow { trust: true },
                Approve::No => Decision::Deny,
                Approve::Ask => tokio::task::spawn_blocking(|| {
                    eprint!("允许吗？y 允许 / t 允许并记住 / 其他拒绝：");
                    let _ = std::io::stderr().flush();
                    let mut line = String::new();
                    let _ = std::io::stdin().lock().read_line(&mut line);
                    match line.trim() {
                        "y" => Decision::Allow { trust: false },
                        "t" => Decision::Allow { trust: true },
                        _ => Decision::Deny,
                    }
                })
                .await
                .unwrap_or(Decision::Deny),
            };
            eprintln!("【决定】{d:?}");
            d
        })
    }

    fn event(&self, ev: HostEvent) {
        eprintln!("【事件】{}", serde_json_line(&ev));
    }
}

fn serde_json_line(ev: &HostEvent) -> String {
    format!("{ev:?}")
}

fn serde_json_pretty(d: &p2pcli_core::diag::Diagnosis) -> String {
    format!("{d:#?}")
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "p2pcli_core=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    let approve = match &cli.cmd {
        Cmd::Host { approve, .. } => *approve,
        _ => Approve::No,
    };
    let node = Node::start(
        NodeConfig {
            data_dir: cli.data_dir.clone(),
            device_name: cli.name.clone(),
            server: Some(cli.server.clone()).filter(|s| !s.trim().is_empty()),
            relay_only: cli.relay_only,
            bind_ip: cli.bind_ip,
            relays: cli.relay.clone(),
            offline: false,
        },
        Arc::new(CliDelegate { approve }),
    )
    .await?;

    match cli.cmd {
        Cmd::Host { password, .. } => {
            if let Some(p) = password {
                node.host.set_password(&p);
            }
            eprintln!("本机公钥：{}", node.id());
            eprintln!("临时密码：{}", node.host.password());
            if let Some(rdv) = &node.rdv {
                let mut code = rdv.watch_code();
                let got = tokio::time::timeout(Duration::from_secs(30), code.wait_for(|c| c.is_some())).await;
                match got {
                    Ok(Ok(c)) => eprintln!("设备码：{}", format_code(c.as_deref().unwrap())),
                    _ => eprintln!("牵线服务器没连上：{:?}", rdv.status()),
                }
            }
            match tokio::time::timeout(Duration::from_secs(15), node.ticket()).await {
                Ok(Ok(t)) => eprintln!("直连地址：{t}"),
                _ => eprintln!("直连地址暂时拿不到"),
            }
            tokio::signal::ctrl_c().await?;
            node.shutdown().await;
            Ok(())
        }
        Cmd::Diag => {
            // 给中继探测一点时间
            let _ = tokio::time::timeout(Duration::from_secs(10), node.endpoint.online()).await;
            tokio::time::sleep(Duration::from_secs(2)).await;
            let d = p2pcli_core::diag::diagnose(&node).await;
            println!("直连条件：{}（{}）", d.rating, d.summary);
            for n in &d.notes {
                println!("- {n}");
            }
            eprintln!("{}", serde_json_pretty(&d));
            node.shutdown().await;
            Ok(())
        }
        Cmd::Connect {
            target,
            password,
            run,
            watch,
        } => {
            let started = Instant::now();
            let res = if normalize_code(&target).is_some() {
                node.connect_code(&target, password).await
            } else {
                node.connect_addr(parse_server(&target)?, password).await
            };
            let remote = match res {
                Ok(r) => r,
                Err(ConnectError::Rejected(c)) => {
                    eprintln!("被拒：{}", c.describe());
                    std::process::exit(20);
                }
                Err(e) => bail!("{e}"),
            };
            eprintln!(
                "已连上 {}（{}，用户 {}），用时 {} 毫秒，线路：{:?}",
                remote.info.name,
                remote.info.platform,
                remote.info.user,
                started.elapsed().as_millis(),
                remote.path()
            );
            let code = run_once(&remote, &run).await?;
            for i in 0..watch {
                tokio::time::sleep(Duration::from_secs(1)).await;
                eprintln!("第 {} 秒线路：{:?}", i + 1, remote.path());
            }
            remote.close();
            node.shutdown().await;
            std::process::exit(code.unwrap_or(255) as i32);
        }
    }
}

async fn run_once(remote: &RemoteHost, cmd: &str) -> Result<Option<u32>> {
    let (mut input, mut output) = remote.open_terminal(120, 40).await?;
    eprintln!("对面 shell：{}", output.shell);
    input.write(format!("{cmd}\r").as_bytes()).await?;
    input.write(b"exit\r").await?;
    let mut stdout = std::io::stdout();
    loop {
        match tokio::time::timeout(Duration::from_secs(60), output.next()).await {
            Err(_) => bail!("60 秒没输出"),
            Ok(r) => match r? {
                Some(TermEvent::Output(d)) => {
                    if asks_cursor_position(&d) {
                        input.write(b"\x1b[1;1R").await?;
                    }
                    stdout.write_all(&d)?;
                    stdout.flush()?;
                }
                Some(TermEvent::Exit(c)) => return Ok(c),
                None => return Ok(None),
            },
        }
    }
}
