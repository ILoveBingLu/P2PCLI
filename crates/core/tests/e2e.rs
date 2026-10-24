//! 同一进程里起两台设备，走真的 iroh 连接，把配对、弹窗、终端整条链路跑一遍。

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iroh::{EndpointAddr, TransportAddr};
use p2pcli_core::client::{ConnectError, TermEvent, asks_cursor_position};
use p2pcli_core::host::{BoxFuture, Decision, HostDelegate, HostEvent, IncomingRequest};
use p2pcli_core::node::{Node, NodeConfig};
use p2pcli_core::proto::RejectCode;

struct TestDelegate {
    decision: Mutex<Decision>,
    requests: Mutex<Vec<IncomingRequest>>,
    events: Mutex<Vec<HostEvent>>,
}

impl TestDelegate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            decision: Mutex::new(Decision::Deny),
            requests: Mutex::new(Vec::new()),
            events: Mutex::new(Vec::new()),
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
