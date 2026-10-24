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
    let (mut input, mut output) = remote.open_terminal(100, 30).await.unwrap();
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
    let (mut input, mut output) = remote.open_terminal(80, 24).await.unwrap();
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
