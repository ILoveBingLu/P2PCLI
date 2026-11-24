//! 手机看远程画面走一遍：同一个进程里起一台被连的电脑（抓屏、编码换成假的），用手机接口连它、
//! 开画面、收解码配置和帧（自动回确认，对方才会一直发）、发键鼠和文字、换画质、停止。
//! 走真的 iroh 连接，只用本机回环地址，不连外网。

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use p2pcli_core::host::{BoxFuture, Decision, HostDelegate, HostEvent, IncomingRequest, ScreenDecision, ScreenRequest};
use p2pcli_core::node::{Node, NodeConfig};
use p2pcli_core::screen::{CaptureOptions, DisplayInfo, EncodedFrame, InputEvent, ScreenBackend, ScreenCapture, VideoConfig};
use p2pcli_mobile::{
    AppListener, FileRequestView, P2p, ScreenCodec, ScreenCursorShape, ScreenInput, ScreenListener, ScreenMouseButton,
    ScreenQuality, ScreenVideoConfig, TransferView,
};

struct AllowAll;
impl HostDelegate for AllowAll {
    fn approve(&self, _req: IncomingRequest) -> BoxFuture<Decision> {
        Box::pin(async { Decision::Allow { trust: false } })
    }
    fn event(&self, _ev: HostEvent) {}
    fn approve_screen(&self, _req: ScreenRequest) -> BoxFuture<ScreenDecision> {
        Box::pin(async { ScreenDecision { allow: true, control: true } })
    }
}

#[derive(Default)]
struct Fake {
    inputs: Mutex<Vec<InputEvent>>,
    codec: Mutex<Option<p2pcli_core::screen::VideoCodec>>,
    stopped: AtomicBool,
}

struct FakeBackend(Arc<Fake>);
struct FakeCapture {
    fake: Arc<Fake>,
    key: AtomicBool,
}

impl ScreenBackend for FakeBackend {
    fn displays(&self) -> Result<Vec<DisplayInfo>> {
        Ok(vec![DisplayInfo { id: "1".into(), name: "主屏".into(), width: 1920, height: 1080, primary: true }])
    }
    fn check_capture(&self) -> Result<()> {
        Ok(())
    }
    fn check_control(&self) -> Result<()> {
        Ok(())
    }
    fn start(&self, opts: CaptureOptions) -> Result<Arc<dyn ScreenCapture>> {
        *self.0.codec.lock().unwrap() = Some(opts.codec);
        Ok(Arc::new(FakeCapture { fake: self.0.clone(), key: AtomicBool::new(true) }))
    }
    fn inject(&self, _display: &str, ev: &InputEvent) -> Result<()> {
        self.0.inputs.lock().unwrap().push(ev.clone());
        Ok(())
    }
}

impl Drop for FakeCapture {
    fn drop(&mut self) {
        self.fake.stopped.store(true, Ordering::Relaxed);
    }
}

impl ScreenCapture for FakeCapture {
    fn next_frame(&self) -> BoxFuture<Result<EncodedFrame>> {
        let key = self.key.swap(false, Ordering::Relaxed);
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(16)).await;
            Ok(EncodedFrame {
                key,
                ts_us: 0,
                data: vec![if key { 0x65 } else { 0x41 }; 2000],
                // avcC 的头一个字节是 1：base64 “AQ==”
                config: key.then(|| VideoConfig {
                    codec: "avc1.640028".into(),
                    width: 1920,
                    height: 1080,
                    description: "AQ==".into(),
                    full_range: false,
                    primaries: Default::default(),
                    layout: Default::default(),
                }),
            })
        })
    }
    fn request_keyframe(&self) {
        self.key.store(true, Ordering::Relaxed);
    }
    fn set_bitrate(&self, _bps: u32) {}
}

#[derive(Default)]
struct Viewer {
    config: Mutex<Option<ScreenVideoConfig>>,
    frames: AtomicUsize,
    first_key: Mutex<Option<bool>>,
    closed: Mutex<Option<Option<String>>>,
    changed: Condvar,
    lock: Mutex<()>,
}

impl ScreenListener for Viewer {
    fn on_config(&self, config: ScreenVideoConfig) {
        *self.config.lock().unwrap() = Some(config);
    }
    fn on_frame(&self, key: bool, data: Vec<u8>) {
        assert!(self.config.lock().unwrap().is_some(), "解码配置要在第一帧之前");
        assert_eq!(data[0], if key { 0x65 } else { 0x41 });
        self.first_key.lock().unwrap().get_or_insert(key);
        self.frames.fetch_add(1, Ordering::Relaxed);
        self.changed.notify_all();
    }
    fn on_cursor(&self, _x: f64, _y: f64, _visible: bool) {}
    fn on_cursor_shape(&self, _shape: ScreenCursorShape) {}
    fn on_paused(&self, _relay: bool) {}
    fn on_resumed(&self) {}
    fn on_switched(&self, _display: String) {}
    fn on_stats(&self, _bitrate: u32, _fps: u32) {}
    fn on_closed(&self, reason: Option<String>) {
        *self.closed.lock().unwrap() = Some(reason);
        self.changed.notify_all();
    }
}

impl Viewer {
    fn wait(&self, what: &str, ok: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut g = self.lock.lock().unwrap();
        while !ok(self) {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "等不到{what}");
            g = self.changed.wait_timeout(g, left.min(Duration::from_millis(100))).unwrap().0;
        }
    }
}

#[derive(Default)]
struct Events;
impl AppListener for Events {
    fn on_overview_changed(&self) {}
    fn on_disconnected(&self, _conn_id: u64, _reason: String) {}
    fn on_transfers_changed(&self, _transfers: Vec<TransferView>) {}
    fn on_file_request(&self, _request: FileRequestView) {}
    fn on_file_request_cancelled(&self, _id: u64) {}
}

fn wait<T>(fut: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(fut)
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("p2pcli-mobile-screen-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[test]
fn view_and_control_from_phone() {
    let (host_dir, phone_dir) = (temp_dir("host"), temp_dir("phone"));
    let host_rt = tokio::runtime::Runtime::new().unwrap();
    let host = host_rt
        .block_on(Node::start(NodeConfig { data_dir: host_dir.clone(), offline: true, ..Default::default() }, Arc::new(AllowAll)))
        .unwrap();
    let fake = Arc::new(Fake::default());
    host.host.set_screen_backend(Arc::new(FakeBackend(fake.clone())));
    host.host.set_password("abc234");
    let port = host.endpoint.bound_sockets().into_iter().find(|s| s.is_ipv4()).unwrap().port();
    let target = format!("{}@{}", host.id(), SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port));

    let phone = P2p::start_with(NodeConfig { data_dir: phone_dir.clone(), offline: true, ..Default::default() }, Arc::new(Events)).unwrap();
    let conn = wait(phone.connect(target, Some("abc234".into()))).expect("应该连上");
    assert!(phone.supports_screen(conn.conn_id));

    let viewer = Arc::new(Viewer::default());
    let screen = wait(phone.open_screen(conn.conn_id, ScreenQuality::Balanced, true, vec![ScreenCodec::H265, ScreenCodec::H264], viewer.clone()))
        .expect("应该开出画面");
    let started = screen.started();
    assert_eq!(started.display, "1");
    assert!(started.control);
    assert!(started.text_input, "新版本被连方能收文字");
    assert_eq!(*fake.codec.lock().unwrap(), Some(p2pcli_core::screen::VideoCodec::H265), "手机说能解 H.265 就请对方用");

    // 自动回确认：对方一直发下去
    viewer.wait("30 帧画面", |v| v.frames.load(Ordering::Relaxed) >= 30);
    assert_eq!(*viewer.first_key.lock().unwrap(), Some(true));
    let config = viewer.config.lock().unwrap().clone().unwrap();
    assert_eq!((config.codec, config.width, config.description.as_slice()), (ScreenCodec::H264, 1920, &[1u8][..]));

    // 键鼠和文字：一连串移动合并发，最后一个位置不丢；点击、按键、文字都到
    for i in 0..50 {
        screen.send(ScreenInput::Move { x: i as f64 / 100.0, y: 0.5 });
    }
    screen.send(ScreenInput::Button { button: ScreenMouseButton::Left, down: true, x: 0.49, y: 0.5 });
    screen.send(ScreenInput::Button { button: ScreenMouseButton::Left, down: false, x: 0.49, y: 0.5 });
    screen.send(ScreenInput::Key { code: "Enter".into(), down: true });
    screen.send(ScreenInput::Text { text: "你好，P2P 👋".into() });
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let inputs = fake.inputs.lock().unwrap().clone();
        if inputs.iter().any(|e| matches!(e, InputEvent::Text { .. })) {
            assert!(inputs.iter().any(|e| matches!(e, InputEvent::Move { x, .. } if (*x - 0.49).abs() < 1e-9)), "{inputs:?}");
            assert!(inputs.iter().any(|e| matches!(e, InputEvent::Button { down: false, .. })));
            assert!(inputs.iter().any(|e| matches!(e, InputEvent::Key { code, down: true } if code == "Enter")));
            assert!(inputs.iter().any(|e| matches!(e, InputEvent::Text { text } if text == "你好，P2P 👋")));
            // 按顺序到：点击在文字前面
            let click = inputs.iter().position(|e| matches!(e, InputEvent::Button { .. })).unwrap();
            let text = inputs.iter().position(|e| matches!(e, InputEvent::Text { .. })).unwrap();
            assert!(click < text);
            break;
        }
        assert!(Instant::now() < deadline, "文字没到：{inputs:?}");
        std::thread::sleep(Duration::from_millis(50));
    }

    screen.set_quality(ScreenQuality::Smooth);
    screen.request_keyframe();

    // 停：对方停止抓屏，松开按着的键，界面收到正常结束
    screen.stop();
    viewer.wait("画面结束", |v| v.closed.lock().unwrap().is_some());
    assert_eq!(*viewer.closed.lock().unwrap(), Some(None));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !fake.stopped.load(Ordering::Relaxed) {
        assert!(Instant::now() < deadline, "对方没停止抓屏");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(fake.inputs.lock().unwrap().iter().any(|e| matches!(e, InputEvent::ReleaseAll)));

    phone.shutdown();
    drop(screen);
    drop(phone);
    host_rt.block_on(host.shutdown());
    let _ = std::fs::remove_dir_all(host_dir);
    let _ = std::fs::remove_dir_all(phone_dir);
}
