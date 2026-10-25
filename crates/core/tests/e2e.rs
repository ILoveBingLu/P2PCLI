//! 同一进程里起两台设备，走真的 iroh 连接，把配对、弹窗、终端整条链路跑一遍。

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iroh::{EndpointAddr, TransportAddr};
use p2pcli_core::client::{ConnectError, TermEvent, asks_cursor_position};
use p2pcli_core::files::TransferState;
use p2pcli_core::host::{BoxFuture, Decision, FileRequest, HostDelegate, HostEvent, IncomingRequest};
use p2pcli_core::node::{Node, NodeConfig};
use p2pcli_core::proto::RejectCode;

struct TestDelegate {
    decision: Mutex<Decision>,
    requests: Mutex<Vec<IncomingRequest>>,
    events: Mutex<Vec<HostEvent>>,
    accept_files: Mutex<bool>,
    file_requests: Mutex<Vec<FileRequest>>,
}

impl TestDelegate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            decision: Mutex::new(Decision::Deny),
            requests: Mutex::new(Vec::new()),
            events: Mutex::new(Vec::new()),
            accept_files: Mutex::new(false),
            file_requests: Mutex::new(Vec::new()),
        })
    }
    fn set(&self, d: Decision) {
        *self.decision.lock().unwrap() = d;
    }
    fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

impl HostDelegate for TestDelegate {
    fn approve(&self, req: IncomingRequest) -> BoxFuture<Decision> {
        self.requests.lock().unwrap().push(req);
        let d = *self.decision.lock().unwrap();
        Box::pin(async move { d })
    }
    fn event(&self, ev: HostEvent) {
        self.events.lock().unwrap().push(ev);
    }
    fn approve_files(&self, req: FileRequest) -> BoxFuture<bool> {
        self.file_requests.lock().unwrap().push(req);
        let ok = *self.accept_files.lock().unwrap();
        Box::pin(async move { ok })
    }
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("p2pcli-e2e-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// 本机回环地址，不依赖外网和中继
fn local_addr(node: &Node) -> EndpointAddr {
    let addrs = node
        .endpoint
        .bound_sockets()
        .into_iter()
        .filter(|s| s.is_ipv4())
        .map(|s| TransportAddr::Ip(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), s.port())));
    EndpointAddr::from_parts(node.id(), addrs)
}

fn rejected(r: Result<impl Sized, ConnectError>) -> RejectCode {
    match r {
        Err(ConnectError::Rejected(c)) => c,
        Err(ConnectError::Other(e)) => panic!("应该被拒，实际出错：{e:#}"),
        Ok(_) => panic!("应该被拒，实际连上了"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn pair_approve_and_run_terminal() {
    let host_dir = temp_dir("host");
    let guest_dir = temp_dir("guest");
    let delegate = TestDelegate::new();
    let host = Node::start(
        NodeConfig {
            data_dir: host_dir.clone(),
            device_name: Some("被连机".into()),
            offline: true,
            ..Default::default()
        },
        delegate.clone(),
    )
    .await
    .unwrap();
    let guest = Node::start(
        NodeConfig {
            data_dir: guest_dir.clone(),
            device_name: Some("连接机".into()),
            offline: true,
            ..Default::default()
        },
        TestDelegate::new(),
    )
    .await
    .unwrap();
    host.host.set_password("abc234");
    let addr = local_addr(&host);

    // 不带密码、密码错、主人拒绝，三种都要被拒
    assert_eq!(rejected(guest.connect_addr(addr.clone(), None).await), RejectCode::NeedPassword);
    assert_eq!(
        rejected(guest.connect_addr(addr.clone(), Some("xxxxxx".into())).await),
        RejectCode::BadPassword
    );
    assert_eq!(delegate.request_count(), 0, "密码不对不该弹窗");
    delegate.set(Decision::Deny);
    assert_eq!(
        rejected(guest.connect_addr(addr.clone(), Some("ABC234".into())).await),
        RejectCode::Denied
    );
    assert_eq!(delegate.request_count(), 1);

    // 主人同意，开终端跑命令
    delegate.set(Decision::Allow { trust: false });
    let remote = guest
        .connect_addr(addr.clone(), Some("abc234".into()))
        .await
        .expect("应该连上");
    assert_eq!(remote.info.name, "被连机");
    assert_eq!(delegate.requests.lock().unwrap()[1].name, "连接机");
    let (mut input, mut output) = remote.open_ephemeral_terminal(100, 30).await.unwrap();
    assert!(!output.shell.is_empty());
    input.resize(120, 40).await.unwrap();
    #[cfg(windows)]
    input.write(b"echo hi-$(2+3); exit 3\r").await.unwrap();
    #[cfg(not(windows))]
    input.write(b"echo hi-$((2+3)); exit 3\r").await.unwrap();
    let mut text = String::new();
    let code = loop {
        match tokio::time::timeout(Duration::from_secs(20), output.next())
            .await
            .expect("终端没反应")
            .unwrap()
        {
            Some(TermEvent::Output(d)) => {
                if asks_cursor_position(&d) {
                    input.write(b"\x1b[1;1R").await.unwrap();
                }
                text.push_str(&String::from_utf8_lossy(&d))
            }
            Some(TermEvent::Exit(c)) => break c,
            None => panic!("没收到退出码"),
        }
    };
    assert!(text.contains("hi-5"), "输出：{text}");
    assert_eq!(code, Some(3));
    assert_eq!(host.host.sessions().len(), 1);

    // 主人点断开，连接方要收到
    let session = host.host.sessions()[0].session;
    assert!(host.host.kick(session));
    let reason = tokio::time::timeout(Duration::from_secs(10), remote.closed())
        .await
        .expect("断开没传到连接方");
    assert!(reason.contains("断开"), "原因：{reason}");

    // 勾了受信之后，下次不带密码也不弹窗
    delegate.set(Decision::Allow { trust: true });
    let r = guest.connect_addr(addr.clone(), Some("abc234".into())).await.unwrap();
    r.close();
    let before = delegate.request_count();
    let r = guest
        .connect_addr(addr.clone(), None)
        .await
        .expect("受信设备应该直接连上");
    assert_eq!(delegate.request_count(), before, "受信设备不该再弹窗");
    r.close();

    // 撤掉受信，又要密码
    assert!(host.host.trust.remove(&guest.id().to_string()).unwrap());
    assert_eq!(rejected(guest.connect_addr(addr, None).await), RejectCode::NeedPassword);

    host.shutdown().await;
    guest.shutdown().await;
    let _ = std::fs::remove_dir_all(host_dir);
    let _ = std::fs::remove_dir_all(guest_dir);
}

/// 只给 IPv6 地址也能连上，线路显示为 IPv6 直连。
/// 两条路都在时选 IPv6 由 path::Ipv6First 保证；本机回环延迟低于 10 毫秒，
/// iroh 不会再打洞开第二条路，所以这里测不了“二选一”，要跨公网才测得到。
#[tokio::test(flavor = "multi_thread")]
async fn connects_over_ipv6() {
    use p2pcli_core::path::PathKind;
    use std::net::Ipv6Addr;

    let host_dir = temp_dir("v6host");
    let guest_dir = temp_dir("v6guest");
    let delegate = TestDelegate::new();
    delegate.set(Decision::Allow { trust: false });
    let host = Node::start(
        NodeConfig {
            data_dir: host_dir.clone(),
            offline: true,
            ..Default::default()
        },
        delegate,
    )
    .await
    .unwrap();
    let guest = Node::start(
        NodeConfig {
            data_dir: guest_dir.clone(),
            offline: true,
            ..Default::default()
        },
        TestDelegate::new(),
    )
    .await
    .unwrap();
    host.host.set_password("abc234");
    let Some(v6) = host.endpoint.bound_sockets().into_iter().find(|s| s.is_ipv6()) else {
        eprintln!("本机没有 IPv6 监听，跳过");
        return;
    };
    let addr = EndpointAddr::from_parts(
        host.id(),
        [TransportAddr::Ip(SocketAddr::new(Ipv6Addr::LOCALHOST.into(), v6.port()))],
    );
    let remote = guest.connect_addr(addr, Some("abc234".into())).await.unwrap();
    assert_eq!(remote.path().kind, PathKind::Ipv6, "线路：{:?}", remote.path());
    let (mut input, mut output) = remote.open_ephemeral_terminal(80, 24).await.unwrap();
    #[cfg(windows)]
    input.write(b"echo v6-$(3+4); exit\r").await.unwrap();
    #[cfg(not(windows))]
    input.write(b"echo v6-$((3+4)); exit\r").await.unwrap();
    let mut text = String::new();
    while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_secs(20), output.next()).await.unwrap() {
        match ev {
            TermEvent::Output(d) => {
                if asks_cursor_position(&d) {
                    input.write(b"\x1b[1;1R").await.unwrap();
                }
                text.push_str(&String::from_utf8_lossy(&d));
            }
            TermEvent::Exit(_) => break,
        }
    }
    assert!(text.contains("v6-7"), "输出：{text}");
    remote.close();
    host.shutdown().await;
    guest.shutdown().await;
    let _ = std::fs::remove_dir_all(host_dir);
    let _ = std::fs::remove_dir_all(guest_dir);
}


#[tokio::test(flavor = "multi_thread")]
async fn apply_name_without_replacing_endpoint_or_host() {
    let delegate = TestDelegate::new();
    delegate.set(Decision::Allow { trust: false });
    let cfg = NodeConfig {
        data_dir: temp_dir("live-config"),
        device_name: Some("before".into()),
        offline: true,
        ..Default::default()
    };
    let node = Node::start(cfg.clone(), delegate.clone()).await.unwrap();
    node.host.set_password("abc234");
    let guest = Node::start(NodeConfig {
        data_dir: temp_dir("live-config-guest"), offline: true, ..Default::default()
    }, TestDelegate::new()).await.unwrap();
    let remote = guest.connect_addr(local_addr(&node), Some("abc234".into())).await.unwrap();
    let sockets = node.endpoint.bound_sockets();
    let mut updated = cfg.clone();
    updated.device_name = Some("after".into());
    assert!(!node.transport_changed(&updated));
    let next = node.prepare_update(updated, delegate.clone()).await.unwrap();
    node.stop_rendezvous().await;
    next.activate();
    assert!(Arc::ptr_eq(&node.host, &next.host));
    assert!(Arc::ptr_eq(&node.recent, &next.recent));
    assert_eq!(next.endpoint.bound_sockets(), sockets);
    assert!(tokio::time::timeout(Duration::from_millis(100), remote.closed()).await.is_err());
    let new_remote = guest.connect_addr(local_addr(&next), Some("abc234".into())).await.unwrap();
    assert_eq!(new_remote.info.name, "after");
    let mut invalid = cfg;
    invalid.proxy = Some("socks5://127.0.0.1:7890".into());
    assert!(next.prepare_update(invalid, delegate).await.is_err());
    assert!(tokio::time::timeout(Duration::from_millis(100), remote.closed()).await.is_err());
    new_remote.close();
    remote.close();
    guest.shutdown().await;
    next.shutdown().await;
}

async fn read_until(input: &mut p2pcli_core::client::TermInput, output: &mut p2pcli_core::client::TermOutput, marker: &str) {
    tokio::time::timeout(Duration::from_secs(25), async {
        let mut text = String::new();
        while let Some(ev) = output.next().await.unwrap() {
            match ev {
                TermEvent::Output(data) => {
                    if asks_cursor_position(&data) { input.write(b"\x1b[1;1R").await.unwrap(); }
                    text.push_str(&String::from_utf8_lossy(&data));
                    if text.contains(marker) { return; }
                }
                TermEvent::Exit(code) => panic!("attachment ended before {marker}: {code:?}"),
            }
        }
        panic!("stream ended before {marker}");
    }).await.expect("persistent terminal did not respond");
}

/// Explicit opt-in: this starts a real detached tmux/psmux server on the test machine.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires tmux on macOS/Linux or psmux on Windows"]
async fn persistent_session_survives_disconnect_and_host_restart() {
    let delegate = TestDelegate::new();
    delegate.set(Decision::Allow { trust: false });
    let cfg = NodeConfig {
        data_dir: temp_dir("persistent-host"), offline: true,
        session_program: std::env::var_os("P2PCLI_TEST_SESSION_PROGRAM").map(std::path::PathBuf::from),
        ..Default::default()
    };
    let host = Node::start(cfg.clone(), delegate.clone()).await.unwrap();
    host.host.set_password("abc234");
    let guest = Node::start(NodeConfig { data_dir: temp_dir("persistent-guest"), offline: true, ..Default::default() }, TestDelegate::new()).await.unwrap();
    let remote = guest.connect_addr(local_addr(&host), Some("abc234".into())).await.unwrap();
    let (mut input, mut output) = remote.open_terminal(100, 30).await.unwrap();
    let id = output.session_id.clone();
    #[cfg(windows)]
    input.write(b"$p2pcliRetained = 41; Write-Output ('ready-' + (20+22))\r").await.unwrap();
    #[cfg(not(windows))]
    input.write(b"p2pcliRetained=41; printf 'ready-%s\\n' $((20+22))\r").await.unwrap();
    read_until(&mut input, &mut output, "ready-42").await;
    input.close(); // Closing the tab only detaches.
    remote.close(); // A lost network connection must not kill the shell either.
    drop((input, output, remote));
    host.shutdown().await;
    let restarted = Node::start(cfg, delegate).await.unwrap();
    restarted.host.set_password("abc234");
    let remote = guest.connect_addr(local_addr(&restarted), Some("abc234".into())).await.unwrap();
    assert!(remote.list_terminals().await.unwrap().iter().any(|s| s.id == id));
    assert!(restarted.host.terminals.list(&"0".repeat(64)).await.unwrap().is_empty());
    assert!(restarted.host.terminals.end(&"0".repeat(64), &id).await.is_err());
    let (mut input, mut output) = remote.attach_terminal(Some(id.clone()), 100, 30).await.unwrap();
    #[cfg(windows)]
    input.write(b"Write-Output ('retained-' + ($p2pcliRetained+1))\r").await.unwrap();
    #[cfg(not(windows))]
    input.write(b"printf 'retained-%s\\n' $((p2pcliRetained+1))\r").await.unwrap();
    read_until(&mut input, &mut output, "retained-42").await;
    remote.end_terminal(id.clone()).await.unwrap();
    assert!(!remote.list_terminals().await.unwrap().iter().any(|s| s.id == id));
    remote.close();
    guest.shutdown().await;
    restarted.shutdown().await;
}

/// 会话里跑命令行 AI：终端连接的会话列表和在线探测都要认出是 Claude Code，状态跟着屏幕变。
/// 用一个改名叫 claude 的小脚本假装 AI，依次显示干活、等确认、空闲三种屏幕。要真的起 tmux，所以默认不跑
#[cfg(not(windows))]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires tmux on macOS/Linux"]
async fn agent_in_session_is_recognized_with_its_state() {
    use p2pcli_core::agents::AgentState;

    let delegate = TestDelegate::new();
    delegate.set(Decision::Allow { trust: false });
    let host = Node::start(NodeConfig {
        data_dir: temp_dir("agent-host"), offline: true,
        session_program: std::env::var_os("P2PCLI_TEST_SESSION_PROGRAM").map(std::path::PathBuf::from),
        ..Default::default()
    }, delegate).await.unwrap();
    host.host.set_password("abc234");
    let guest = Node::start(NodeConfig { data_dir: temp_dir("agent-guest"), offline: true, ..Default::default() }, TestDelegate::new()).await.unwrap();
    let remote = guest.connect_addr(local_addr(&host), Some("abc234".into())).await.unwrap();
    let (mut input, output) = remote.open_terminal(100, 30).await.unwrap();
    let id = output.session_id.clone();
    let script = "printf '\\033[2J\\033[H✻ Pondering… (2s)\\n'; sleep 6; \
        printf '\\033[2J\\033[HDo you want to proceed?\\n❯ 1. Yes\\n  2. No\\nEsc to cancel\\n'; sleep 6; \
        printf '\\033[2J\\033[H✻ Worked for 3s\\n❯ \\n'; sleep 60";
    input.write(format!("(exec -a claude sh -c \"{script}\")\r").as_bytes()).await.unwrap();

    // 等会话列表里这个会话的 AI 变成想要的状态；顺带确认在线探测也带着同样的信息
    let conn = p2pcli_core::presence::connect(&guest.endpoint, local_addr(&host)).await.unwrap();
    for want in [AgentState::Working, AgentState::Waiting, AgentState::Idle] {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let sessions = remote.list_terminals().await.unwrap();
                let agent = sessions.iter().find(|s| s.id == id).and_then(|s| s.agent.clone());
                if let Some(agent) = agent && agent.state == want {
                    assert_eq!((agent.kind.as_str(), agent.name.as_str()), ("claude", "Claude Code"));
                    let probed = p2pcli_core::presence::ask_sessions(&conn).await.unwrap().unwrap();
                    let probed = probed.iter().find(|s| s.id == id).unwrap();
                    assert!(probed.details.is_none(), "探测不该带目录和程序");
                    assert_eq!(probed.agent.as_ref().map(|a| a.kind.as_str()), Some("claude"), "探测要带上 AI 的种类");
                    return;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }).await.unwrap_or_else(|_| panic!("会话里的 AI 没有变成 {want:?}"));
    }
    remote.end_terminal(id).await.unwrap();
    remote.close();
    guest.shutdown().await;
    host.shutdown().await;
}

/// 在线探测：配对过的能拿会话列表；没配对的只知道在线和延迟；旧版本认得出来，不当成离线；对方关了 ping 就不通
#[tokio::test(flavor = "multi_thread")]
async fn presence_probe_paired_unpaired_and_old_version() {
    use p2pcli_core::presence::{self, ProbeError};

    let delegate = TestDelegate::new();
    let start = |dir: std::path::PathBuf, name: &str, d: Arc<TestDelegate>| {
        let name = name.to_string();
        async move {
            Node::start(NodeConfig { data_dir: dir, device_name: Some(name), offline: true, ..Default::default() }, d)
                .await
                .unwrap()
        }
    };
    let host = start(temp_dir("presence-host"), "被连机", delegate.clone()).await;
    let paired = start(temp_dir("presence-paired"), "配对过的", TestDelegate::new()).await;
    let stranger = start(temp_dir("presence-stranger"), "陌生的", TestDelegate::new()).await;
    host.host.set_password("abc234");
    let addr = local_addr(&host);

    // 同意过一次，就记成配对设备
    delegate.set(Decision::Allow { trust: false });
    let remote = paired.connect_addr(addr.clone(), Some("abc234".into())).await.expect("应该连上");
    remote.close();
    assert!(host.host.paired.contains(&paired.id()), "同意过的设备应该记成配对");
    let requests = delegate.request_count();

    // 配对过的：拿得到会话列表，也有线路和延迟，而且不再弹窗
    let conn = presence::connect(&paired.endpoint, addr.clone()).await.expect("探测应该连上");
    let sessions = presence::ask_sessions(&conn).await.unwrap();
    assert!(sessions.is_some(), "配对过的设备应该拿到会话列表");
    assert!(sessions.unwrap().iter().all(|s| s.details.is_none()), "探测不该带目录和程序");
    let path = p2pcli_core::path::current(&conn);
    assert!(path.rtt_ms.is_some(), "应该测得出延迟：{path:?}");
    assert_eq!(delegate.request_count(), requests, "探测不该弹窗");
    // ping 和会话列表同时问，互不耽误
    let (a, b) = tokio::join!(presence::ping(&conn), presence::ask_sessions(&conn));
    a.expect("ping 应该通");
    b.expect("会话列表应该拿得到");
    let paired_conn = conn;

    // 没配对过的：只回“没配对”，延迟照样有
    let conn = presence::connect(&stranger.endpoint, addr.clone()).await.expect("探测应该连上");
    assert_eq!(presence::ask_sessions(&conn).await.unwrap(), None);
    assert!(p2pcli_core::path::current(&conn).rtt_ms.is_some());
    presence::ping(&conn).await.expect("没配对的也能 ping，只是测延迟");
    assert_eq!(delegate.request_count(), requests, "探测不该弹窗");

    // 旧版本：只认终端协议，探测要认出是旧版本，不是离线
    let old = iroh::Endpoint::builder(iroh::endpoint::presets::N0)
        .alpns(vec![p2pcli_core::ALPN_TERM.to_vec()])
        .clear_address_lookup()
        .relay_mode(iroh::RelayMode::Disabled)
        .bind()
        .await
        .unwrap();
    let old_accept = old.clone();
    tokio::spawn(async move {
        while let Some(incoming) = old_accept.accept().await {
            tokio::spawn(async move {
                if let Ok(a) = incoming.accept() {
                    let _ = a.await;
                }
            });
        }
    });
    let old_addr = EndpointAddr::from_parts(
        old.id(),
        old.bound_sockets()
            .into_iter()
            .filter(|s| s.is_ipv4())
            .map(|s| TransportAddr::Ip(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), s.port()))),
    );
    match presence::connect(&paired.endpoint, old_addr).await {
        Err(ProbeError::OldVersion) => {}
        Err(ProbeError::Unreachable(e)) => panic!("旧版本被当成连不上：{e}"),
        Ok(_) => panic!("旧版本不该连上探测协议"),
    }

    old.close().await;
    // 被连方关了：ping 要在超时内报错，不能一直卡着
    host.shutdown().await;
    let started = std::time::Instant::now();
    assert!(presence::ping(&paired_conn).await.is_err(), "对方关了 ping 不该通");
    assert!(started.elapsed() < Duration::from_secs(5), "ping 卡太久：{:?}", started.elapsed());
    for n in [paired, stranger] {
        n.shutdown().await;
    }
}

/// 传文件：配对过的直接收，文件夹和权限原样，重名不覆盖；断点续传接得上，半截坏了查得出来；陌生设备要主人同意
#[tokio::test(flavor = "multi_thread")]
async fn file_transfer_paired_resume_and_stranger() {
    use p2pcli_core::files::Direction;

    let delegate = TestDelegate::new();
    let host_dir = temp_dir("files-host");
    let inbox = host_dir.join("inbox");
    let host = Node::start(
        NodeConfig { data_dir: host_dir.clone(), device_name: Some("收件机".into()), offline: true, inbox_dir: Some(inbox.clone()), ..Default::default() },
        delegate.clone(),
    )
    .await
    .unwrap();
    let start = |tag: &str, name: &str| {
        let (dir, name) = (temp_dir(tag), name.to_string());
        async move {
            Node::start(NodeConfig { data_dir: dir, device_name: Some(name), offline: true, ..Default::default() }, TestDelegate::new())
                .await
                .unwrap()
        }
    };
    let sender = start("files-sender", "发件机").await;
    let stranger = start("files-stranger", "陌生机").await;
    host.host.set_password("abc234");
    let addr = local_addr(&host);
    delegate.set(Decision::Allow { trust: false });
    sender.connect_addr(addr.clone(), Some("abc234".into())).await.expect("应该连上").close();

    // 要发的东西：一个文件，一个带子文件夹、空文件、可执行文件的文件夹
    let src = temp_dir("files-src");
    std::fs::create_dir_all(src.join("proj/sub")).unwrap();
    let big: Vec<u8> = (0..3_000_000u32).map(|i| (i.wrapping_mul(2654435761) >> 24) as u8).collect();
    std::fs::write(src.join("a.txt"), "你好，文件").unwrap();
    std::fs::write(src.join("proj/big.bin"), &big).unwrap();
    std::fs::write(src.join("proj/sub/empty.txt"), b"").unwrap();
    std::fs::write(src.join("proj/run.sh"), "#!/bin/sh\necho hi\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(src.join("proj/run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let t = sender.send_files(addr.clone(), "收件机", vec![src.join("a.txt"), src.join("proj")]).await;
    assert_eq!(t.state, TransferState::Done, "配对过的应该直接收：{:?}", t.error);
    assert!(delegate.file_requests.lock().unwrap().is_empty(), "配对过的不该弹窗");
    assert_eq!((t.files, t.total, t.done), (4, big.len() as u64 + 15 + 18, big.len() as u64 + 15 + 18));
    let dest = inbox.join("发件机");
    assert_eq!(t.paths, [dest.join("a.txt").display().to_string(), dest.join("proj").display().to_string()]);
    assert_eq!(std::fs::read_to_string(dest.join("a.txt")).unwrap(), "你好，文件");
    assert_eq!(std::fs::read(dest.join("proj/big.bin")).unwrap(), big);
    assert_eq!(std::fs::read(dest.join("proj/sub/empty.txt")).unwrap(), b"");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_ne!(std::fs::metadata(dest.join("proj/run.sh")).unwrap().permissions().mode() & 0o111, 0, "可执行权限要保留");
    }
    let received = host.files.transfers();
    assert_eq!(received[0].direction, Direction::Receive);
    assert_eq!(received[0].state, TransferState::Done);
    assert_eq!(host.files.inbox(10)[0].from_name, "发件机");
    assert!(delegate.events.lock().unwrap().iter().any(|e| matches!(e, HostEvent::FilesReceived { files: 4, .. })));

    // 再发一次：不覆盖，加 (1)
    let t = sender.send_files(addr.clone(), "收件机", vec![src.join("a.txt"), src.join("proj")]).await;
    assert_eq!(t.state, TransferState::Done, "{:?}", t.error);
    assert_eq!(t.paths, [dest.join("a (1).txt").display().to_string(), dest.join("proj (1)").display().to_string()]);

    // 续传：收件方已经有前一半，接着发后一半
    let only = src.join("proj/big.bin");
    let modified = std::fs::metadata(&only).unwrap().modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let partial = |path: &str| {
        let mut h = blake3::Hasher::new();
        h.update(sender.id().as_bytes());
        h.update(path.as_bytes());
        h.update(&(big.len() as u64).to_le_bytes());
        h.update(&modified.to_le_bytes());
        inbox.join(".partial").join(&h.finalize().to_hex()[..32])
    };
    std::fs::write(partial("big.bin"), &big[..1_000_000]).unwrap();
    let t = sender.send_files(addr.clone(), "收件机", vec![only.clone()]).await;
    assert_eq!(t.state, TransferState::Done, "{:?}", t.error);
    assert_eq!(std::fs::read(dest.join("big.bin")).unwrap(), big);
    assert!(!partial("big.bin").exists(), "收完半截文件要挪走");

    // 半截文件坏了：校验不过就报错并丢掉半截，再发一次就好
    let mut bad = big[..1_000_000].to_vec();
    bad[10] ^= 0xff;
    std::fs::write(partial("big.bin"), &bad).unwrap();
    let t = sender.send_files(addr.clone(), "收件机", vec![only.clone()]).await;
    assert_eq!(t.state, TransferState::Failed);
    assert!(t.error.as_deref().unwrap_or_default().contains("校验没通过"), "{:?}", t.error);
    assert!(!partial("big.bin").exists(), "坏的半截要删掉");
    let t = sender.send_files(addr.clone(), "收件机", vec![only.clone()]).await;
    assert_eq!(t.state, TransferState::Done, "{:?}", t.error);
    assert_eq!(std::fs::read(dest.join("big (1).bin")).unwrap(), big);

    // 陌生设备：弹窗问，不同意就不收
    let t = stranger.send_files(addr.clone(), "收件机", vec![src.join("a.txt")]).await;
    assert_eq!(t.state, TransferState::Failed);
    assert_eq!(t.error.as_deref(), Some("对方拒绝接收"));
    assert_eq!(delegate.file_requests.lock().unwrap().len(), 1);
    assert_eq!(delegate.file_requests.lock().unwrap()[0].name, "陌生机");
    *delegate.accept_files.lock().unwrap() = true;
    let t = stranger.send_files(addr.clone(), "收件机", vec![src.join("a.txt")]).await;
    assert_eq!(t.state, TransferState::Done, "{:?}", t.error);
    assert!(inbox.join("陌生机/a.txt").exists());

    // 发不存在的文件：本机就报错，不连对方
    let t = sender.send_files(addr.clone(), "收件机", vec![src.join("没有这个")]).await;
    assert_eq!(t.state, TransferState::Failed);
    assert!(t.error.unwrap().contains("找不到"));

    for n in [host, sender, stranger] {
        n.shutdown().await;
    }
    let _ = std::fs::remove_dir_all(&src);
}
