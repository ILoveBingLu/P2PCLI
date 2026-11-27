//! 开发测试用命令行：一台跑 host，另一台 connect 过去跑一条命令。

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use p2pcli_core::client::{ConnectError, RemoteHost, TermEvent, asks_cursor_position};
use p2pcli_core::host::{BoxFuture, Decision, FileRequest, HostDelegate, HostEvent, IncomingRequest, ScreenDecision, ScreenRequest};
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
    /// 关掉局域网发现（默认开，也让别人看到本机）
    #[arg(long, global = true)]
    no_lan: bool,
    /// 局域网里只看别人，不让别人看到本机
    #[arg(long, global = true)]
    lan_hidden: bool,
    /// 收到的文件放哪，默认下载文件夹里的 P2PCLI
    #[arg(long, global = true)]
    inbox: Option<PathBuf>,
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

/// 别人要看本机画面时怎么办
#[derive(Clone, Copy, ValueEnum, Default)]
enum ScreenMode {
    /// 不给看（默认）
    #[default]
    No,
    /// 只给看，不给操作
    View,
    /// 给看也给操作
    Control,
}

#[derive(Subcommand)]
enum Cmd {
    /// 等别人来连
    Host {
        #[arg(long)]
        password: Option<String>,
        #[arg(long, value_enum, default_value = "ask")]
        approve: Approve,
        /// 让别人看本机画面（用真实的抓屏和硬件编码）
        #[arg(long, value_enum, default_value = "no")]
        screen: ScreenMode,
        /// 画面不抓屏幕，用程序生成的 60 Hz 动态画面（照样走 GPU 转换和硬件编码），测网络这一段
        #[arg(long)]
        synthetic: bool,
    },
    /// 看对方画面（只看不操作）：收画面、回确认，每秒打印帧率、码率、线路
    Screen {
        /// 9 位设备码、局域网设备的公钥，或 <公钥>@<ip:端口>
        target: String,
        #[arg(long)]
        password: Option<String>,
        /// 看多少秒
        #[arg(long, default_value_t = 20)]
        seconds: u64,
        /// sharp、balanced、smooth
        #[arg(long, default_value = "balanced")]
        quality: String,
        /// 把收到的 H.264 存成 Annex B 文件，用来验证能不能解码
        #[arg(long)]
        save: Option<PathBuf>,
        /// 请对方用 H.265（对方编不了会自己退回 H.264）
        #[arg(long)]
        h265: bool,
        /// 请对方用 4:4:4（对方做不到会自己退回 4:2:0）
        #[arg(long)]
        yuv444: bool,
        /// 假装本机屏幕的刷新率（Hz）：对方帧率不超过它。0 是不报，对方只看自己的屏幕
        #[arg(long, default_value_t = 0)]
        hz: u32,
    },
    /// 网络诊断：IPv6、NAT 类型、中继
    Diag,
    /// 列出连接历史里各台设备的在线、线路、延迟、会话列表（后台探测的结果）
    Devices {
        /// 等多少秒再列
        #[arg(long, default_value_t = 8)]
        wait: u64,
        /// 列完不退出，每 2 秒再列一次，看延迟和在线变化
        #[arg(long)]
        watch: bool,
    },
    /// 列出局域网里发现的 P2PCLI 设备
    Nearby {
        /// 等多少秒再列
        #[arg(long, default_value_t = 5)]
        wait: u64,
    },
    /// 连过去跑一条命令，打印输出，退出码跟对面一致
    Connect {
        /// 9 位设备码、局域网设备的公钥，或 <公钥>@<ip:端口>
        target: String,
        #[arg(long)]
        password: Option<String>,
        #[arg(long)]
        run: String,
        /// 跑完后再观察多少秒线路（看打洞有没有从中继切到直连）
        #[arg(long, default_value_t = 0)]
        watch: u64,
        /// 跑之前把本机代理借给对方（主机:端口，写 socks5:// 开头是 SOCKS5），对方跑的命令走它
        #[arg(long)]
        borrow_proxy: Option<String>,
    },
}

struct CliDelegate {
    approve: Approve,
    screen: ScreenMode,
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

    fn approve_screen(&self, req: ScreenRequest) -> BoxFuture<ScreenDecision> {
        let d = match self.screen {
            ScreenMode::No => ScreenDecision { allow: false, control: false },
            ScreenMode::View => ScreenDecision { allow: true, control: false },
            ScreenMode::Control => ScreenDecision { allow: true, control: true },
        };
        eprintln!("【看画面请求】{}（{}）想要操作={}，决定 {d:?}", req.name, req.platform, req.control);
        Box::pin(async move { d })
    }

    fn approve_files(&self, req: FileRequest) -> BoxFuture<bool> {
        eprintln!("【收文件请求】{}（{}）{} 个文件 {} 字节：{}", req.name, req.platform, req.files, req.total, req.items.join("、"));
        let approve = self.approve;
        Box::pin(async move {
            let ok = match approve {
                Approve::Yes | Approve::Trust => true,
                Approve::No => false,
                Approve::Ask => tokio::task::spawn_blocking(|| {
                    eprint!("收吗？y 收 / 其他不收：");
                    let _ = std::io::stderr().flush();
                    let mut line = String::new();
                    let _ = std::io::stdin().lock().read_line(&mut line);
                    line.trim() == "y"
                })
                .await
                .unwrap_or(false),
            };
            eprintln!("【决定】{}", if ok { "收" } else { "不收" });
            ok
        })
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
    let (approve, screen) = match &cli.cmd {
        Cmd::Host { approve, screen, .. } => (*approve, *screen),
        _ => (Approve::No, ScreenMode::No),
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
            proxy: None,
            lan: !cli.no_lan,
            lan_visible: !cli.no_lan && !cli.lan_hidden,
            session_program: None,
            inbox_dir: cli.inbox.clone(),
            app_version: None,
            platform: None,
            agent_commands: Default::default(),
            file_lanes: None,
            custom_transport: None,
        },
        Arc::new(CliDelegate { approve, screen }),
    )
    .await?;
    let synthetic = matches!(&cli.cmd, Cmd::Host { synthetic: true, .. });
    if !matches!(screen, ScreenMode::No) {
        match if synthetic { p2pcli_screen::synthetic_backend() } else { p2pcli_screen::backend() } {
            Some(b) => {
                eprintln!("远程画面：屏幕 {:?}", b.displays());
                node.host.set_screen_backend(b);
            }
            None => eprintln!("这个系统不支持远程画面"),
        }
    }

    match cli.cmd {
        Cmd::Screen { target, password, seconds, quality, save, h265, yuv444, hz } => {
            let remote = match connect_target(&node, &target, password).await {
                Ok(r) => r,
                Err(ConnectError::Rejected(c)) => bail!("被拒：{}", c.describe()),
                Err(e) => bail!("{e}"),
            };
            eprintln!("已连上 {}（{}），线路：{:?}", remote.info.name, remote.info.platform, remote.path());
            // 刚连上可能还在中继上，等打洞换成直连
            for _ in 0..20 {
                if matches!(remote.path().kind, p2pcli_core::path::PathKind::Ipv4 | p2pcli_core::path::PathKind::Ipv6) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            let quality = match quality.as_str() {
                "sharp" => p2pcli_core::screen::Quality::Sharp,
                "smooth" => p2pcli_core::screen::Quality::Smooth,
                _ => p2pcli_core::screen::Quality::Balanced,
            };
            watch_screen(&remote, &node.endpoint, quality, seconds, save, h265, yuv444, hz).await?;
            remote.close();
            node.shutdown().await;
            Ok(())
        }
        Cmd::Host { password, .. } => {
            if let Some(p) = password {
                node.host.set_password(&p);
            }
            eprintln!("本机公钥：{}", node.id());
            eprintln!("配对密码：{}", node.host.password());
            // 开本机控制口，p2pcli 命令行能通过它发文件
            let shared = node.clone();
            match p2pcli_core::control::serve(&cli.data_dir, Arc::new(move || shared.clone())).await {
                Ok(()) => eprintln!("收件目录：{}", node.files.inbox_root().display()),
                Err(e) => eprintln!("控制口没起来：{e:#}"),
            }
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
        Cmd::Devices { wait, watch } => {
            tokio::time::sleep(Duration::from_secs(wait)).await;
            let started = Instant::now();
            loop {
                for d in node.devices() {
                    let path = d.path.map(|p| format!("{:?} {}ms {}", p.kind, p.rtt_ms.unwrap_or(0), p.remote.unwrap_or_default())).unwrap_or_default();
                    let sessions: Vec<String> = d.sessions.iter().map(|s| format!("{}{}", &s.id[..8], if s.attached { "(在用)" } else { "" })).collect();
                    println!(
                        "{:>5.1}s  {}  {:?}  {}  配对={:?}  会话[{}]{}",
                        started.elapsed().as_secs_f32(), &d.endpoint_id[..10], d.state, path, d.paired, sessions.join(","),
                        if d.sessions_live { "" } else { "（记住的）" }
                    );
                }
                if !watch {
                    break;
                }
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                    _ = tokio::signal::ctrl_c() => break,
                }
            }
            node.shutdown().await;
            Ok(())
        }
        Cmd::Nearby { wait } => {
            tokio::time::sleep(Duration::from_secs(wait)).await;
            let list = node.nearby();
            if list.is_empty() {
                println!("局域网里没发现别的 P2PCLI 设备");
            }
            for d in list {
                println!("{}  {}  {}", d.endpoint_id, d.platform, d.name);
            }
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
            borrow_proxy,
        } => {
            let started = Instant::now();
            let res = if normalize_code(&target).is_some() {
                node.connect_code(&target, password).await
            } else if let Ok(id) = target.trim().parse::<iroh::EndpointId>() {
                node.connect_endpoint(id, password).await
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
            let remote = Arc::new(remote);
            let _lend = match borrow_proxy {
                Some(proxy) => {
                    let kind = if proxy.starts_with("socks5") { p2pcli_core::reverse::ProxyKind::Socks5 } else { p2pcli_core::reverse::ProxyKind::Http };
                    let target = proxy.split("://").last().unwrap_or(&proxy).to_string();
                    let f = p2pcli_core::reverse::ReverseForward::start(remote.clone(), 0, target, Some(kind)).await?;
                    eprintln!("已把本机代理借给对方：对方的 127.0.0.1:{}", f.remote_port);
                    Some(f)
                }
                None => None,
            };
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

async fn connect_target(node: &Node, target: &str, password: Option<String>) -> Result<RemoteHost, ConnectError> {
    if normalize_code(target).is_some() {
        node.connect_code(target, password).await
    } else if let Ok(id) = target.trim().parse::<iroh::EndpointId>() {
        node.connect_endpoint(id, password).await
    } else {
        node.connect_addr(parse_server(target).map_err(ConnectError::Other)?, password).await
    }
}

/// 只看不操作：收画面、马上回确认，每秒打印一行
#[allow(clippy::too_many_arguments)]
async fn watch_screen(remote: &RemoteHost, endpoint: &iroh::Endpoint, quality: p2pcli_core::screen::Quality, seconds: u64, save: Option<PathBuf>, h265: bool, yuv444: bool, hz: u32) -> Result<()> {
    use p2pcli_core::screen::{HostMsg, ScreenEvent, ScreenOpen, ViewerMsg};
    let started = Instant::now();
    let (info, mut input, mut output) = remote.open_screen(ScreenOpen {
        display: None,
        quality,
        control: false,
        codecs: if h265 { vec![p2pcli_core::screen::VideoCodec::H265, p2pcli_core::screen::VideoCodec::H264] } else { vec![] },
        color_tags: true,
        wide_gamut: false,
        yuv444,
        // 只看不操作，剪贴板不互通
        clipboard: false,
        refresh_hz: hz,
        keyboard: None,
        translate_keys: false,
    }).await?;
    // 帧间隔：看稳不稳。整段的都留着，最后算分位数；每秒打印这一秒最长的和超过 25 毫秒的次数
    let mut last_frame: Option<Instant> = None;
    let mut gaps: Vec<f64> = Vec::new();
    let (mut sec_max, mut sec_slow) = (0f64, 0u32);
    let mut host_stats: Option<(u32, u32, u32)> = None;
    let mut last_encoder = None;
    // 本机发出去的（确认）丢了多少：反方向丢包会让对方以为网络堵了
    let mut sent_lost = { let s = remote.stats(endpoint); (s.sent_packets, s.lost_packets) };
    // 单独的任务读画面：读一帧不能被超时打断，不然读了一半的字节就丢了，后面全错位
    let (ev_tx, mut ev_rx) = tokio::sync::mpsc::unbounded_channel();
    let reader = tokio::spawn(async move {
        loop {
            let ev = output.next().await;
            let end = !matches!(ev, Ok(Some(_)));
            if ev_tx.send(ev).is_err() || end {
                break;
            }
        }
    });
    eprintln!("开始看画面，用时 {} 毫秒：屏幕 {:?}，当前 {}", started.elapsed().as_millis(), info.displays, info.display);
    let mut file = match &save { Some(p) => Some(std::fs::File::create(p)?), None => None };
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let (mut frames, mut bytes, mut keys, mut total_frames, mut total_bytes) = (0u32, 0usize, 0u32, 0u64, 0usize);
    let mut first_frame = None;
    let mut tick = Instant::now();
    let mut asked_key = false;
    while Instant::now() < deadline {
        // 第 5 秒要一次关键帧：屏幕不动时被连方也要能给出来
        if !asked_key && started.elapsed() > Duration::from_secs(5) {
            asked_key = true;
            eprintln!("要一个关键帧");
            input.send(&ViewerMsg::KeyFrame).await?;
        }
        let ev = match tokio::time::timeout(Duration::from_millis(500), ev_rx.recv()).await {
            Ok(Some(r)) => r?,
            Ok(None) => break,
            Err(_) => None,
        };
        match ev {
            Some(ScreenEvent::Frame { seq, key, data, .. }) => {
                input.send(&ViewerMsg::Ack { seq }).await?;
                let now = Instant::now();
                if let Some(prev) = last_frame {
                    let gap = (now - prev).as_secs_f64() * 1000.0;
                    gaps.push(gap);
                    sec_max = sec_max.max(gap);
                    sec_slow += u32::from(gap > 25.0);
                    if gap > 100.0 {
                        eprintln!("      停了 {gap:.0} ms（第 {seq} 帧，{} KB{}）", data.len() / 1024, if key { "，关键帧" } else { "" });
                    }
                }
                last_frame = Some(now);
                first_frame.get_or_insert_with(|| started.elapsed());
                frames += 1;
                total_frames += 1;
                bytes += data.len();
                total_bytes += data.len();
                keys += u32::from(key);
                if let Some(f) = &mut file {
                    let mut d = &data[..];
                    while d.len() >= 4 {
                        let n = u32::from_be_bytes(d[..4].try_into()?) as usize;
                        f.write_all(&[0, 0, 0, 1])?;
                        f.write_all(&d[4..4 + n])?;
                        d = &d[4 + n..];
                    }
                }
            }
            Some(ScreenEvent::Config(c)) => {
                eprintln!("解码配置：{} {}x{}", c.codec, c.width, c.height);
                if let Some(f) = &mut file {
                    use base64::Engine as _;
                    let rec = base64::engine::general_purpose::STANDARD.decode(&c.description)?;
                    let mut sets: Vec<&[u8]> = Vec::new();
                    if c.codec.starts_with("hvc1") {
                        let mut i = 23;
                        for _ in 0..rec[22] {
                            let n = u16::from_be_bytes([rec[i + 1], rec[i + 2]]) as usize;
                            i += 3;
                            for _ in 0..n {
                                let len = u16::from_be_bytes([rec[i], rec[i + 1]]) as usize;
                                sets.push(&rec[i + 2..i + 2 + len]);
                                i += 2 + len;
                            }
                        }
                    } else {
                        let sl = u16::from_be_bytes([rec[6], rec[7]]) as usize;
                        let pl = u16::from_be_bytes([rec[9 + sl], rec[10 + sl]]) as usize;
                        sets.push(&rec[8..8 + sl]);
                        sets.push(&rec[11 + sl..11 + sl + pl]);
                    }
                    for p in sets {
                        f.write_all(&[0, 0, 0, 1])?;
                        f.write_all(p)?;
                    }
                }
            }
            Some(ScreenEvent::Host(HostMsg::Stats { bitrate, fps, target_fps, encoder, encode_us })) => {
                host_stats = Some((fps, bitrate, target_fps));
                if encoder != last_encoder {
                    eprintln!("对方编码器：{encoder:?}，这一秒每帧编 {:.1} ms", encode_us as f64 / 1000.0);
                    last_encoder = encoder;
                }
            }
            // 指针的消息太多，不打印
            Some(ScreenEvent::Host(HostMsg::Cursor { .. } | HostMsg::CursorShape { .. })) => {}
            Some(ScreenEvent::Host(m)) => eprintln!("对方：{m:?}"),
            None => {}
        }
        if tick.elapsed() >= Duration::from_secs(1) {
            let secs = tick.elapsed().as_secs_f64();
            let path = remote.path();
            let now_stats = remote.stats(endpoint);
            let (sent, lost) = (now_stats.sent_packets - sent_lost.0, now_stats.lost_packets - sent_lost.1);
            sent_lost = (now_stats.sent_packets, now_stats.lost_packets);
            eprintln!("      本机发出 {sent} 包、丢 {lost} 包");
            eprintln!(
                "{:>4.0}s  {:>3} 帧/秒  {:>6.2} Mbps  关键帧 {}  最长间隔 {:>4.0} ms  超 25 ms {:>2} 次  对方 {:?}  线路 {:?} {}ms",
                started.elapsed().as_secs_f64(), (frames as f64 / secs).round(), bytes as f64 * 8.0 / secs / 1e6, keys,
                sec_max, sec_slow, host_stats.map(|(f, b, t)| format!("{f}/{t} 帧 {:.1} Mbps", b as f64 / 1e6)).unwrap_or_default(),
                path.kind, path.rtt_ms.unwrap_or(0)
            );
            (frames, bytes, keys, sec_max, sec_slow) = (0, 0, 0, 0.0, 0);
            tick = Instant::now();
        }
    }
    eprintln!(
        "一共 {total_frames} 帧，{:.1} MB，第一帧用时 {:?}",
        total_bytes as f64 / 1e6,
        first_frame.unwrap_or_default()
    );
    if !gaps.is_empty() {
        let mut sorted = gaps.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let pct = |p: f64| sorted[((sorted.len() as f64 - 1.0) * p) as usize];
        eprintln!(
            "帧间隔：中位 {:.1} ms、95% {:.1} ms、99% {:.1} ms、最长 {:.0} ms；超过 25 ms {} 次、50 ms {} 次、100 ms {} 次",
            pct(0.5), pct(0.95), pct(0.99), sorted.last().unwrap(),
            gaps.iter().filter(|&&g| g > 25.0).count(), gaps.iter().filter(|&&g| g > 50.0).count(), gaps.iter().filter(|&&g| g > 100.0).count()
        );
    }
    input.close().await;
    reader.abort();
    Ok(())
}

async fn run_once(remote: &RemoteHost, cmd: &str) -> Result<Option<u32>> {
    let (mut input, mut output) = remote.open_ephemeral_terminal(120, 40).await?;
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
