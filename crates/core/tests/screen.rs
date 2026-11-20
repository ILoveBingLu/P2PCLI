//! 远程画面整条链路：同一进程里起两台设备，走真的 iroh 连接（本机回环，算直连）。
//! 抓屏和编码换成假的后端，只验协议：同意、解码配置、关键帧、确认节奏、码率、键鼠事件、停止。
#![cfg(feature = "host")]

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use iroh::{EndpointAddr, TransportAddr};
use p2pcli_core::host::{BoxFuture, Decision, HostDelegate, HostEvent, IncomingRequest, ScreenDecision, ScreenRequest};
use p2pcli_core::node::{Node, NodeConfig};
use p2pcli_core::screen::{
    CaptureOptions, DisplayInfo, EncodedFrame, InputEvent, Quality, ScreenBackend, ScreenCapture, ScreenEvent, ScreenOpen,
    VideoConfig, ViewerMsg,
};
use tokio::time::timeout;

struct Delegate {
    screen: Mutex<ScreenDecision>,
    screen_requests: Mutex<Vec<ScreenRequest>>,
    events: Mutex<Vec<HostEvent>>,
}

impl HostDelegate for Delegate {
    fn approve(&self, _: IncomingRequest) -> BoxFuture<Decision> {
        Box::pin(async { Decision::Allow { trust: false } })
    }
    fn event(&self, ev: HostEvent) {
        self.events.lock().unwrap().push(ev);
    }
    fn approve_screen(&self, req: ScreenRequest) -> BoxFuture<ScreenDecision> {
        self.screen_requests.lock().unwrap().push(req);
        let d = *self.screen.lock().unwrap();
        Box::pin(async move { d })
    }
}

#[derive(Default)]
struct Fake {
    /// 能单独读指针
    cursor: bool,
    /// 抓屏时要不要把指针画进画面
    show_cursor: Mutex<Option<bool>>,
    cursor_reads: AtomicUsize,
    produced: AtomicUsize,
    keyframes: AtomicUsize,
    bitrate: AtomicU32,
    inputs: Mutex<Vec<(String, InputEvent)>>,
}

struct FakeBackend(Arc<Fake>);
struct FakeCapture {
    fake: Arc<Fake>,
    key: Arc<std::sync::atomic::AtomicBool>,
}

impl ScreenBackend for FakeBackend {
    fn displays(&self) -> Result<Vec<DisplayInfo>> {
        Ok(vec![
            DisplayInfo { id: "1".into(), name: "主屏".into(), width: 1920, height: 1080, primary: true },
            DisplayInfo { id: "2".into(), name: "副屏".into(), width: 1280, height: 1024, primary: false },
        ])
    }
    fn check_capture(&self) -> Result<()> {
        Ok(())
    }
    fn check_control(&self) -> Result<()> {
        Ok(())
    }
    fn start(&self, opts: CaptureOptions) -> Result<Arc<dyn ScreenCapture>> {
        self.0.bitrate.store(opts.bitrate, Ordering::Relaxed);
        *self.0.show_cursor.lock().unwrap() = Some(opts.show_cursor);
        Ok(Arc::new(FakeCapture { fake: self.0.clone(), key: Arc::new(true.into()) }))
    }
    fn inject(&self, display: &str, ev: &InputEvent) -> Result<()> {
        self.0.inputs.lock().unwrap().push((display.to_string(), ev.clone()));
        Ok(())
    }
    fn cursor(&self, _display: &str) -> Option<p2pcli_core::screen::CursorInfo> {
        if !self.0.cursor {
            return None;
        }
        // 指针一直往右挪，样子不变
        let n = self.0.cursor_reads.fetch_add(1, Ordering::Relaxed);
        Some(p2pcli_core::screen::CursorInfo {
            x: (n % 100) as f64 / 100.0,
            y: 0.5,
            visible: true,
            shape: Some(Arc::new(p2pcli_core::screen::CursorShape {
                id: 7, width: 2, height: 2, hot_x: 0.0, hot_y: 0.0, scale: 1.0, rgba: "AAAAAAAAAAAAAAAAAAAAAA==".into(),
            })),
        })
    }
}

impl ScreenCapture for FakeCapture {
    fn next_frame(&self) -> BoxFuture<Result<EncodedFrame>> {
        let fake = self.fake.clone();
        let key = self.key.swap(false, Ordering::Relaxed);
        Box::pin(async move {
            // 60 帧每秒的屏幕一直在变
            tokio::time::sleep(Duration::from_millis(16)).await;
            fake.produced.fetch_add(1, Ordering::Relaxed);
            if key {
                fake.keyframes.fetch_add(1, Ordering::Relaxed);
            }
            Ok(EncodedFrame {
                key,
                ts_us: 0,
                data: vec![if key { 0x65 } else { 0x41 }; if key { 60_000 } else { 8_000 }],
                config: key.then(|| VideoConfig { codec: "avc1.640028".into(), width: 1920, height: 1080, description: "AQ==".into() }),
            })
        })
    }
    fn request_keyframe(&self) {
        self.key.store(true, Ordering::Relaxed);
    }
    fn set_bitrate(&self, bps: u32) {
        self.fake.bitrate.store(bps, Ordering::Relaxed);
    }
}

fn local_addr(node: &Node) -> EndpointAddr {
    let addrs = node
        .endpoint
        .bound_sockets()
        .into_iter()
        .filter(|s| s.is_ipv4())
        .map(|s| TransportAddr::Ip(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), s.port())));
    EndpointAddr::from_parts(node.id(), addrs)
}

fn config(tag: &str) -> NodeConfig {
    let dir = std::env::temp_dir().join(format!("p2pcli-screen-{tag}-{:x}", rand::random::<u64>()));
    NodeConfig { data_dir: dir, offline: true, ..Default::default() }
}

async fn pair(screen: ScreenDecision, backend: Option<Arc<Fake>>) -> (Node2, Arc<Delegate>) {
    let delegate = Arc::new(Delegate { screen: Mutex::new(screen), screen_requests: Mutex::new(vec![]), events: Mutex::new(vec![]) });
    let (hc, gc) = (config("host"), config("guest"));
    let dirs = [hc.data_dir.clone(), gc.data_dir.clone()];
    let host = Node::start(hc, delegate.clone()).await.unwrap();
    if let Some(fake) = backend {
        host.host.set_screen_backend(Arc::new(FakeBackend(fake)));
    }
    let guest = Node::start(gc, delegate.clone()).await.unwrap();
    let remote = guest.connect_addr(local_addr(&host), Some(host.host.password())).await.unwrap();
    (Node2 { host, guest, remote, dirs }, delegate)
}

struct Node2 {
    host: Arc<Node>,
    guest: Arc<Node>,
    remote: p2pcli_core::client::RemoteHost,
    dirs: [std::path::PathBuf; 2],
}

impl Node2 {
    async fn shutdown(self) {
        self.guest.shutdown().await;
        self.host.shutdown().await;
        for d in self.dirs {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn view_and_control_over_direct_link() {
    let fake = Arc::new(Fake::default());
    let (n, delegate) = pair(ScreenDecision { allow: true, control: true }, Some(fake.clone())).await;
    assert!(n.remote.supports_screen());

    let (started, mut input, mut output) =
        n.remote.open_screen(ScreenOpen { display: None, quality: Quality::Balanced, control: true }).await.unwrap();
    assert_eq!(started.display, "1");
    assert_eq!(started.displays.len(), 2);
    assert!(started.control);
    assert_eq!(delegate.screen_requests.lock().unwrap().len(), 1);

    // 先来解码配置，再来关键帧，然后是普通帧；每帧都回确认
    let mut frames = 0;
    let mut saw_config = false;
    let mut first_key = None;
    while frames < 40 {
        match timeout(Duration::from_secs(5), output.next()).await.unwrap().unwrap().unwrap() {
            ScreenEvent::Config(c) => {
                assert_eq!(c.width, 1920);
                saw_config = true;
            }
            ScreenEvent::Frame { seq, key, data, .. } => {
                assert!(saw_config, "解码配置要在第一帧之前");
                if first_key.is_none() {
                    first_key = Some(key);
                }
                assert_eq!(data[0], if key { 0x65 } else { 0x41 });
                frames += 1;
                input.send(&ViewerMsg::Ack { seq }).await.unwrap();
            }
            ScreenEvent::Host(_) => {}
        }
    }
    assert_eq!(first_key, Some(true));
    assert!(delegate.events.lock().unwrap().iter().any(|e| matches!(e, HostEvent::ScreenStarted { .. })));
    assert_eq!(n.host.host.screen_viewers().len(), 1);

    // 键鼠：发到当前屏幕
    input.send(&ViewerMsg::Input { ev: InputEvent::Key { code: "KeyA".into(), down: true } }).await.unwrap();
    input.send(&ViewerMsg::Input { ev: InputEvent::Move { x: 0.5, y: 0.25 } }).await.unwrap();
    // 换屏：重新从关键帧开始
    let keys_before = fake.keyframes.load(Ordering::Relaxed);
    input.send(&ViewerMsg::Switch { display: "2".into() }).await.unwrap();
    let mut switched = false;
    let mut key_after_switch = false;
    for _ in 0..200 {
        match timeout(Duration::from_secs(5), output.next()).await.unwrap().unwrap().unwrap() {
            ScreenEvent::Host(p2pcli_core::screen::HostMsg::Switched { display }) => {
                assert_eq!(display, "2");
                switched = true;
            }
            ScreenEvent::Frame { seq, key, .. } => {
                input.send(&ViewerMsg::Ack { seq }).await.unwrap();
                if switched && key {
                    key_after_switch = true;
                    break;
                }
            }
            _ => {}
        }
    }
    assert!(switched && key_after_switch);
    assert!(fake.keyframes.load(Ordering::Relaxed) > keys_before);
    input.send(&ViewerMsg::Input { ev: InputEvent::Button { button: p2pcli_core::screen::MouseButton::Left, down: true, x: 0.1, y: 0.1 } }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    {
        let inputs = fake.inputs.lock().unwrap();
        assert!(inputs.iter().any(|(d, e)| d == "1" && matches!(e, InputEvent::Key { code, down: true } if code == "KeyA")));
        assert!(inputs.iter().any(|(d, e)| d == "1" && matches!(e, InputEvent::Move { .. })));
        assert!(inputs.iter().any(|(d, e)| d == "2" && matches!(e, InputEvent::Button { .. })));
    }

    // 观看方关掉：被连方结束，松开按着的键
    input.close().await;
    drop(output);
    for _ in 0..50 {
        if n.host.host.screen_viewers().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(n.host.host.screen_viewers().is_empty());
    assert!(delegate.events.lock().unwrap().iter().any(|e| matches!(e, HostEvent::ScreenStopped { .. })));
    assert!(matches!(fake.inputs.lock().unwrap().last(), Some((_, InputEvent::ReleaseAll))));
    n.shutdown().await;
}

/// 观看方不回确认：被连方很快就停下，不会一直往外塞
#[tokio::test(flavor = "multi_thread")]
async fn stops_sending_without_acks() {
    let fake = Arc::new(Fake::default());
    let (n, _) = pair(ScreenDecision { allow: true, control: false }, Some(fake.clone())).await;
    let (started, _input, mut output) =
        n.remote.open_screen(ScreenOpen { display: None, quality: Quality::Balanced, control: true }).await.unwrap();
    // 对方不给操作
    assert!(!started.control);
    assert!(started.control_note.is_some());
    let mut frames = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(1500);
    while let Ok(ev) = tokio::time::timeout_at(deadline, output.next()).await {
        if let ScreenEvent::Frame { .. } = ev.unwrap().unwrap() {
            frames += 1;
        }
    }
    // 1.5 秒按 60 帧算有 90 帧；不确认的话，排队上限之后就不再编码新帧
    assert!(frames < 30, "不确认也一直在发：{frames} 帧");
    assert!(fake.produced.load(Ordering::Relaxed) < 35);
    n.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn denied_or_unsupported() {
    // 被连方拒绝
    let fake = Arc::new(Fake::default());
    let (n, _) = pair(ScreenDecision { allow: false, control: false }, Some(fake.clone())).await;
    let err = n.remote.open_screen(ScreenOpen { display: None, quality: Quality::Smooth, control: false }).await.err().unwrap();
    assert!(format!("{err:#}").contains("denied"), "{err:#}");
    assert_eq!(fake.produced.load(Ordering::Relaxed), 0);
    n.shutdown().await;

    // 被连方没有抓屏实现：不报能力，观看方不开
    let (n, _) = pair(ScreenDecision { allow: true, control: true }, None).await;
    assert!(!n.remote.supports_screen());
    assert!(n.remote.open_screen(ScreenOpen { display: None, quality: Quality::Smooth, control: false }).await.is_err());
    n.shutdown().await;
}

/// 被连方能单独读指针：画面里不画指针，指针的样子只发一次，位置有变化就发，观看方用本机指针画、跟手
#[tokio::test(flavor = "multi_thread")]
async fn cursor_is_sent_separately() {
    let fake = Arc::new(Fake { cursor: true, ..Default::default() });
    let (n, _) = pair(ScreenDecision { allow: true, control: true }, Some(fake.clone())).await;
    let (_, mut input, mut output) =
        n.remote.open_screen(ScreenOpen { display: None, quality: Quality::Balanced, control: true }).await.unwrap();
    let (mut shapes, mut moves) = (0, 0);
    let deadline = tokio::time::Instant::now() + Duration::from_millis(800);
    while let Ok(ev) = tokio::time::timeout_at(deadline, output.next()).await {
        match ev.unwrap().unwrap() {
            ScreenEvent::Frame { seq, .. } => input.send(&ViewerMsg::Ack { seq }).await.unwrap(),
            ScreenEvent::Host(p2pcli_core::screen::HostMsg::CursorShape { shape }) => {
                assert_eq!(shape.id, 7);
                shapes += 1;
            }
            ScreenEvent::Host(p2pcli_core::screen::HostMsg::Cursor { visible, .. }) => {
                assert!(visible);
                moves += 1;
            }
            _ => {}
        }
    }
    assert_eq!(*fake.show_cursor.lock().unwrap(), Some(false), "能单独读指针时不该把指针画进画面");
    assert_eq!(shapes, 1, "样子没变只发一次");
    assert!(moves >= 10, "指针在挪，位置要一直报：只收到 {moves} 次");
    n.shutdown().await;

    // 读不了指针的被连方：指针照旧画进画面
    let fake = Arc::new(Fake::default());
    let (n, _) = pair(ScreenDecision { allow: true, control: true }, Some(fake.clone())).await;
    let _s = n.remote.open_screen(ScreenOpen { display: None, quality: Quality::Balanced, control: true }).await.unwrap();
    assert_eq!(*fake.show_cursor.lock().unwrap(), Some(true));
    n.shutdown().await;
}
