//! 牵线服务器同时开 UDP 和 WebSocket：一台设备走 WebSocket，一台走 UDP，互相用设备码连，
//! 发包通知也要能跨两种通道送到。

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use p2pcli_core::host::{BoxFuture, Decision, HostDelegate, HostEvent, IncomingRequest};
use p2pcli_core::node::{Node, NodeConfig};
use p2pcli_core::v6;

struct Allow;

impl HostDelegate for Allow {
    fn approve(&self, _req: IncomingRequest) -> BoxFuture<Decision> {
        Box::pin(async { Decision::Allow { trust: false } })
    }
    fn event(&self, _ev: HostEvent) {}
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("p2pcli-ws-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

async fn start(tag: &str, server: String) -> Arc<Node> {
    let node = Node::start(
        NodeConfig {
            data_dir: temp_dir(tag),
            device_name: Some(tag.into()),
            server: Some(server),
            offline: true,
            ..Default::default()
        },
        Arc::new(Allow),
    )
    .await
    .unwrap();
    node.host.set_password("abc234");
    node
}

async fn code_of(node: &Node) -> String {
    let mut code = node.rdv.as_ref().unwrap().watch_code();
    let c = tokio::time::timeout(Duration::from_secs(15), code.wait_for(|c| c.is_some()))
        .await
        .expect("登记超时")
        .unwrap()
        .clone()
        .unwrap();
    // 等第一次报地址
    tokio::time::sleep(Duration::from_millis(500)).await;
    c
}

#[tokio::test(flavor = "multi_thread")]
async fn ws_and_udp_devices_find_each_other() {
    // 有局域网 IPv6 时顺带测发包通知经 WebSocket 送达
    unsafe { std::env::set_var("P2PCLI_IPV6_ULA", "1") };

    let srv_dir = temp_dir("srv");
    let state = p2pcli_server::State::open(&srv_dir.join("devices.db")).unwrap();
    let srv_ep = p2pcli_server::bind(iroh::SecretKey::generate(), 0, vec![]).await.unwrap();
    let udp_port = srv_ep.bound_sockets().iter().find(|s| s.is_ipv4()).unwrap().port();
    let quic_server = format!("{}@127.0.0.1:{udp_port}", srv_ep.id());
    tokio::spawn(p2pcli_server::serve(srv_ep.clone(), state.clone()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ws_server = format!("ws://127.0.0.1:{}", listener.local_addr().unwrap().port());
    tokio::spawn(p2pcli_server::serve_ws(listener, state));

    let ws_dev = start("wsdev", ws_server).await;
    let udp_dev = start("udpdev", quic_server).await;
    let ws_code = code_of(&ws_dev).await;
    let udp_code = code_of(&udp_dev).await;
    assert_ne!(ws_code, udp_code);

    // UDP 那台连 WebSocket 那台
    let r = udp_dev.connect_code(&ws_code, Some("abc234".into())).await.expect("UDP→WS 应该连上");
    assert_eq!(r.info.name, "wsdev");
    r.close();
    let has_v6 = !v6::local_candidates(&ws_dev.endpoint).is_empty();
    if has_v6 {
        for _ in 0..20 {
            if ws_dev.punches.load(Ordering::Relaxed) > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(ws_dev.punches.load(Ordering::Relaxed), 1, "发包通知没经 WebSocket 送到");
    }

    // WebSocket 那台连 UDP 那台
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let r = ws_dev.connect_code(&udp_code, Some("abc234".into())).await.expect("WS→UDP 应该连上");
    assert_eq!(r.info.name, "udpdev");
    r.close();
    if has_v6 {
        assert_eq!(udp_dev.punches.load(Ordering::Relaxed), 1, "发包通知没经 UDP 送到");
    }

    // 查不存在的码
    let e = udp_dev.connect_code("111111111", None).await.err().expect("应该报没有这个设备码");
    assert!(e.to_string().contains("没有这个设备码"), "{e}");

    ws_dev.shutdown().await;
    udp_dev.shutdown().await;
    srv_ep.close().await;
    for tag in ["srv", "wsdev", "udpdev"] {
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!("p2pcli-ws-{tag}-{}", std::process::id())));
    }
}

/// 鉴权不对的连接要被拒
#[tokio::test(flavor = "multi_thread")]
async fn ws_rejects_bad_auth() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let srv_dir = temp_dir("srv2");
    let state = p2pcli_server::State::open(&srv_dir.join("devices.db")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(p2pcli_server::serve_ws(listener, state));

    let url = format!("ws://127.0.0.1:{port}/v1/ws");
    // 没带鉴权
    assert!(tokio_tungstenite::connect_async(url.as_str()).await.is_err());
    // 签的是别的主机名
    let key = iroh::SecretKey::generate();
    let mut req = url.as_str().into_client_request().unwrap();
    let h = p2pcli_core::rdv_ws::auth_header(&key, "evil.example.com", p2pcli_core::store::now_secs());
    req.headers_mut().insert(p2pcli_core::rdv_ws::AUTH_HEADER, h.parse().unwrap());
    assert!(tokio_tungstenite::connect_async(req).await.is_err());
    // 正确的能连上
    let mut req = url.as_str().into_client_request().unwrap();
    let h = p2pcli_core::rdv_ws::auth_header(&key, &format!("127.0.0.1:{port}"), p2pcli_core::store::now_secs());
    req.headers_mut().insert(p2pcli_core::rdv_ws::AUTH_HEADER, h.parse().unwrap());
    assert!(tokio_tungstenite::connect_async(req).await.is_ok());
    let _ = std::fs::remove_dir_all(srv_dir);
}
