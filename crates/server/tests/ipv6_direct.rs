//! 牵线服务器、被连方、连接方都在本进程：验证 IPv6 直连的整条流程。
//! 手头没有公网 IPv6，用 P2PCLI_IPV6_ULA 把局域网 fd 开头的地址当公网地址用。

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use p2pcli_core::host::{BoxFuture, Decision, HostDelegate, HostEvent, IncomingRequest};
use p2pcli_core::node::{Node, NodeConfig};
use p2pcli_core::path::PathKind;
use p2pcli_core::v6;

struct Allow;

impl HostDelegate for Allow {
    fn approve(&self, _req: IncomingRequest) -> BoxFuture<Decision> {
        Box::pin(async { Decision::Allow { trust: false } })
    }
    fn event(&self, _ev: HostEvent) {}
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("p2pcli-v6-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[tokio::test(flavor = "multi_thread")]
async fn ipv6_first_dial_with_punch() {
    // 测试进程里只有这一个测试，设环境变量不会和别的线程抢
    unsafe { std::env::set_var("P2PCLI_IPV6_ULA", "1") };

    let srv_dir = temp_dir("srv");
    let state = p2pcli_server::State::open(&srv_dir.join("devices.db")).unwrap();
    let key = iroh::SecretKey::generate();
    let srv_ep = p2pcli_server::bind(key, 0, vec![]).await.unwrap();
    let port = srv_ep.bound_sockets().iter().find(|s| s.is_ipv4()).unwrap().port();
    let server = format!("{}@127.0.0.1:{port}", srv_ep.id());
    tokio::spawn(p2pcli_server::serve(srv_ep.clone(), state));

    let start = |tag: &'static str, name: &'static str| {
        let server = server.clone();
        async move {
            Node::start(
                NodeConfig {
                    data_dir: temp_dir(tag),
                    device_name: Some(name.into()),
                    server: Some(server),
                    offline: true,
                    ..Default::default()
                },
                Arc::new(Allow),
            )
            .await
            .unwrap()
        }
    };
    let host = start("host", "被连机").await;
    let guest = start("guest", "连接机").await;
    let cleanup = || {
        for tag in ["srv", "host", "guest"] {
            let _ = std::fs::remove_dir_all(
                std::env::temp_dir().join(format!("p2pcli-v6-{tag}-{}", std::process::id())),
            );
        }
    };
    if v6::local_candidates(&host.endpoint).is_empty() {
        eprintln!("本机没有可用的 IPv6 地址，跳过");
        host.shutdown().await;
        guest.shutdown().await;
        cleanup();
        return;
    }
    host.host.set_password("abc234");

    // 等被连方登记并报上地址
    let rdv = host.rdv.clone().unwrap();
    let mut code = rdv.watch_code();
    let code = tokio::time::timeout(Duration::from_secs(10), code.wait_for(|c| c.is_some()))
        .await
        .expect("登记超时")
        .unwrap()
        .clone()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let remote = guest
        .connect_code(&code, Some("abc234".into()))
        .await
        .expect("应该连上");
    let path = remote.path();
    assert_eq!(path.kind, PathKind::Ipv6, "线路：{path:?}");
    assert!(
        path.remote.as_deref().is_some_and(|r| r.starts_with("[f") || r.starts_with("[2")),
        "应该是直连的 IPv6 地址：{path:?}"
    );
    // 被连方收到了服务器转来的发包请求
    for _ in 0..20 {
        if host.punches.load(Ordering::Relaxed) > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(host.punches.load(Ordering::Relaxed), 1, "被连方没收到发包请求");

    // 终端能用
    let (mut input, mut output) = remote.open_terminal(80, 24).await.unwrap();
    input.write(b"echo v6-direct-ok; exit\r").await.unwrap();
    let mut text = String::new();
    while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_secs(20), output.next()).await.unwrap() {
        match ev {
            p2pcli_core::client::TermEvent::Output(d) => {
                if p2pcli_core::client::asks_cursor_position(&d) {
                    input.write(b"\x1b[1;1R").await.unwrap();
                }
                text.push_str(&String::from_utf8_lossy(&d));
            }
            p2pcli_core::client::TermEvent::Exit(_) => break,
        }
    }
    assert!(text.contains("v6-direct-ok"), "输出：{text}");

    remote.close();
    host.shutdown().await;
    guest.shutdown().await;
    srv_ep.close().await;
    cleanup();
}
