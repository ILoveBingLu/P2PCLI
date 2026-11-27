//! 远程画面整条链路：同一进程里起两台设备，走真的 iroh 连接（本机回环，算直连）。
//! 抓屏和编码换成假的后端，只验协议：同意、解码配置、关键帧、确认节奏、码率、键鼠事件、停止。
#![cfg(feature = "host")]

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use iroh::{EndpointAddr, TransportAddr};
use p2pcli_core::host::{BoxFuture, Decision, HostDelegate, HostEvent, IncomingRequest, ScreenDecision, ScreenRequest};
use p2pcli_core::node::{Node, NodeConfig};
use p2pcli_core::screen::{
    CaptureOptions, HostMsg, DisplayInfo, EncodedFrame, InputEvent, Quality, ScreenBackend, ScreenCapture, ScreenEvent, ScreenOpen,
    VideoConfig, ViewerMsg,
};
use p2pcli_core::screen_clip::{ClipContent, ClipPlatform, ClipSource, ViewerClip};
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
    /// 被要求用哪种编码格式
    codec: Mutex<Option<p2pcli_core::screen::VideoCodec>>,
    /// 被要求的颜色：全范围、广色域、4:4:4
    color: Mutex<Option<(bool, bool, bool)>>,
    cursor_reads: AtomicUsize,
    produced: AtomicUsize,
    keyframes: AtomicUsize,
    bitrate: AtomicU32,
    inputs: Mutex<Vec<(String, InputEvent)>>,
    /// 本机剪贴板
    clip: Option<Arc<FakeClip>>,
    /// 注入键鼠、设剪贴板的先后
    order: Arc<Mutex<Vec<String>>>,
    /// 认得后加的键鼠事件（送文字、锁定鼠标）
    input_v2: bool,
}

/// 内存里的剪贴板
#[derive(Default)]
struct FakeClip {
    count: AtomicU64,
    ours: AtomicU64,
    content: Mutex<ClipContent>,
    written: Mutex<Option<Arc<dyn ClipSource>>>,
    order: Arc<Mutex<Vec<String>>>,
}

impl FakeClip {
    fn copy(&self, content: ClipContent) {
        *self.content.lock().unwrap() = content;
        self.count.fetch_add(1, Ordering::SeqCst);
    }
    fn written(&self) -> Option<Arc<dyn ClipSource>> {
        self.written.lock().unwrap().clone()
    }
}

impl ClipPlatform for FakeClip {
    fn change_count(&self) -> u64 {
        self.count.load(Ordering::SeqCst)
    }
    fn is_ours(&self) -> bool {
        self.ours.load(Ordering::SeqCst) == self.change_count()
    }
    fn read(&self) -> Result<ClipContent> {
        Ok(self.content.lock().unwrap().clone())
    }
    fn read_format(&self, key: &str) -> Result<Vec<u8>> {
        self.content.lock().unwrap().formats.iter().find(|(k, _)| k == key).and_then(|(_, d)| d.clone()).or_else(|| (key == "mac:com.example.lazy").then(|| b"lazy".to_vec())).ok_or_else(|| anyhow::anyhow!("没有 {key}"))
    }
    fn write(&self, source: Arc<dyn ClipSource>) -> Result<()> {
        // 慢一点：后面的按键也得等它放好
        std::thread::sleep(Duration::from_millis(30));
        *self.written.lock().unwrap() = Some(source);
        *self.content.lock().unwrap() = ClipContent::default();
        let n = self.count.fetch_add(1, Ordering::SeqCst) + 1;
        self.ours.store(n, Ordering::SeqCst);
        self.order.lock().unwrap().push("clip".into());
        Ok(())
    }
}

struct FakeBackend(Arc<Fake>);
struct FakeCapture {
    fake: Arc<Fake>,
    key: Arc<std::sync::atomic::AtomicBool>,
}

impl ScreenBackend for FakeBackend {
    fn displays(&self) -> Result<Vec<DisplayInfo>> {
        Ok(vec![
            DisplayInfo { id: "1".into(), name: "主屏".into(), width: 1920, height: 1080, primary: true, refresh_hz: 0 },
            DisplayInfo { id: "2".into(), name: "副屏".into(), width: 1280, height: 1024, primary: false, refresh_hz: 0 },
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
        *self.0.codec.lock().unwrap() = Some(opts.codec);
        *self.0.color.lock().unwrap() = Some((opts.full_range, opts.wide_gamut, opts.yuv444));
        Ok(Arc::new(FakeCapture { fake: self.0.clone(), key: Arc::new(true.into()) }))
    }
    fn inject(&self, display: &str, ev: &InputEvent) -> Result<()> {
        self.0.inputs.lock().unwrap().push((display.to_string(), ev.clone()));
        if let InputEvent::Key { code, .. } = ev {
            self.0.order.lock().unwrap().push(format!("key:{code}"));
        }
        Ok(())
    }
    fn clipboard(&self) -> Option<Arc<dyn ClipPlatform>> {
        self.0.clip.clone().map(|c| c as Arc<dyn ClipPlatform>)
    }
    fn input_v2(&self) -> bool {
        self.0.input_v2
    }
    fn screenshot(&self, display: &str, max_width: u32) -> Result<p2pcli_core::screen::Screenshot> {
        // 假的“PNG”：带上屏幕编号和宽度，测试里核对传对了
        let png: Vec<u8> = format!("PNG:{display}:{max_width}:").bytes().chain((0..2_500_000u32).map(|i| (i % 251) as u8)).collect();
        Ok(p2pcli_core::screen::Screenshot { png, width: max_width, height: max_width * 10 / 16 })
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
                config: key.then(|| VideoConfig { codec: "avc1.640028".into(), width: 1920, height: 1080, description: "AQ==".into(), full_range: false, primaries: Default::default(), chroma444: false }),
            })
        })
    }
    fn request_keyframe(&self) {
        self.key.store(true, Ordering::Relaxed);
    }
    fn set_bitrate(&self, bps: u32) {
        self.fake.bitrate.store(bps, Ordering::Relaxed);
    }
    fn encoder(&self) -> Option<p2pcli_core::screen::EncoderInfo> {
        Some(p2pcli_core::screen::EncoderInfo { name: "假编码器".into(), hardware: true })
    }
    fn color(&self) -> p2pcli_core::screen::ColorFormat {
        // 像 Mac：要什么给什么
        let (full_range, wide, yuv444) = self.fake.color.lock().unwrap().unwrap_or_default();
        let primaries = if wide { p2pcli_core::screen::Primaries::P3 } else { Default::default() };
        p2pcli_core::screen::ColorFormat { full_range, primaries, chroma444: yuv444 }
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
    (Node2 { host, guest, remote: Arc::new(remote), dirs }, delegate)
}

struct Node2 {
    host: Arc<Node>,
    guest: Arc<Node>,
    remote: Arc<p2pcli_core::client::RemoteHost>,
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
        n.remote.open_screen(ScreenOpen { display: None, quality: Quality::Balanced, control: true, codecs: vec![], ..Default::default() }).await.unwrap();
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
        n.remote.open_screen(ScreenOpen { display: None, quality: Quality::Balanced, control: true, codecs: vec![], ..Default::default() }).await.unwrap();
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
    let err = n.remote.open_screen(ScreenOpen { display: None, quality: Quality::Smooth, control: false, codecs: vec![], ..Default::default() }).await.err().unwrap();
    assert!(format!("{err:#}").contains("denied"), "{err:#}");
    assert_eq!(fake.produced.load(Ordering::Relaxed), 0);
    n.shutdown().await;

    // 被连方没有抓屏实现：不报能力，观看方不开
    let (n, _) = pair(ScreenDecision { allow: true, control: true }, None).await;
    assert!(!n.remote.supports_screen());
    assert!(n.remote.open_screen(ScreenOpen { display: None, quality: Quality::Smooth, control: false, codecs: vec![], ..Default::default() }).await.is_err());
    n.shutdown().await;
}

/// 被连方能单独读指针：画面里不画指针，指针的样子只发一次，位置有变化就发，观看方用本机指针画、跟手
#[tokio::test(flavor = "multi_thread")]
async fn cursor_is_sent_separately() {
    let fake = Arc::new(Fake { cursor: true, ..Default::default() });
    let (n, _) = pair(ScreenDecision { allow: true, control: true }, Some(fake.clone())).await;
    let (_, mut input, mut output) =
        n.remote.open_screen(ScreenOpen { display: None, quality: Quality::Balanced, control: true, codecs: vec![], ..Default::default() }).await.unwrap();
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
    let _s = n.remote.open_screen(ScreenOpen { display: None, quality: Quality::Balanced, control: true, codecs: vec![], ..Default::default() }).await.unwrap();
    assert_eq!(*fake.show_cursor.lock().unwrap(), Some(true));
    n.shutdown().await;
}

/// 送文字、锁定鼠标这几种键鼠事件：被连方认得、而且给操作时才告诉观看方；送的文字太长截掉。
/// 每秒的统计里带着对方在用的编码器
#[tokio::test(flavor = "multi_thread")]
async fn extra_input_only_when_supported() {
    use p2pcli_core::screen::{HostMsg, MouseButton};
    let fake = Arc::new(Fake { input_v2: true, ..Default::default() });
    let (n, _) = pair(ScreenDecision { allow: true, control: true }, Some(fake.clone())).await;
    let (started, mut input, mut output) =
        n.remote.open_screen(ScreenOpen { quality: Quality::Balanced, control: true, ..Default::default() }).await.unwrap();
    assert!(started.input_v2);
    let long: String = "字".repeat(p2pcli_core::screen::MAX_TEXT_INPUT + 50);
    for ev in [
        InputEvent::Text { text: long },
        InputEvent::RelativeMove { dx: 3.0, dy: -2.0 },
        InputEvent::ButtonHere { button: MouseButton::Left, down: true },
        InputEvent::ButtonHere { button: MouseButton::Left, down: false },
        InputEvent::WheelHere { dx: 0.0, dy: 120.0 },
    ] {
        input.send(&ViewerMsg::Input { ev }).await.unwrap();
    }
    // 收画面、回确认，等统计（每秒一次）
    let mut encoder = None;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(2500);
    while let Ok(ev) = tokio::time::timeout_at(deadline, output.next()).await {
        match ev.unwrap().unwrap() {
            ScreenEvent::Frame { seq, .. } => input.send(&ViewerMsg::Ack { seq }).await.unwrap(),
            ScreenEvent::Host(HostMsg::Stats { encoder: Some(e), .. }) => {
                encoder = Some(e);
                break;
            }
            _ => {}
        }
    }
    assert_eq!(encoder.map(|e| (e.name, e.hardware)), Some(("假编码器".to_string(), true)));
    let got: Vec<InputEvent> = fake.inputs.lock().unwrap().iter().map(|(_, ev)| ev.clone()).collect();
    let InputEvent::Text { text } = &got[0] else { panic!("第一个应该是文字：{got:?}") };
    assert_eq!(text.chars().count(), p2pcli_core::screen::MAX_TEXT_INPUT, "太长的文字要截掉");
    assert_eq!(got[1], InputEvent::RelativeMove { dx: 3.0, dy: -2.0 });
    assert_eq!(got[4], InputEvent::WheelHere { dx: 0.0, dy: 120.0 });
    n.shutdown().await;

    // 只给看：不开
    let fake = Arc::new(Fake { input_v2: true, ..Default::default() });
    let (n, _) = pair(ScreenDecision { allow: true, control: false }, Some(fake.clone())).await;
    let (started, _input, _output) = n.remote.open_screen(ScreenOpen { quality: Quality::Balanced, control: true, ..Default::default() }).await.unwrap();
    assert!(!started.input_v2);
    n.shutdown().await;

    // 被连方不认得（老版本）：不开
    let fake = Arc::new(Fake::default());
    let (n, _) = pair(ScreenDecision { allow: true, control: true }, Some(fake.clone())).await;
    let (started, _input, _output) = n.remote.open_screen(ScreenOpen { quality: Quality::Balanced, control: true, ..Default::default() }).await.unwrap();
    assert!(!started.input_v2);
    n.shutdown().await;
}

/// 快捷键转换在被连方做：观看方报了自己的键盘习惯（这里故意和被连方相反），⌘C / Ctrl+C 换成对方的复制；
/// 关掉以后原样发；老版本的观看方不报，就不转换
#[tokio::test(flavor = "multi_thread")]
async fn shortcuts_are_translated_on_host() {
    use p2pcli_core::screen_keys::KeyboardKind;
    let host = KeyboardKind::local();
    let viewer = if host == KeyboardKind::Mac { KeyboardKind::Pc } else { KeyboardKind::Mac };
    // 观看方的“命令键”和对方的
    let (theirs, ours) = if viewer == KeyboardKind::Mac { ("MetaLeft", "ControlLeft") } else { ("ControlLeft", "MetaLeft") };
    let key = |code: &str, down: bool| InputEvent::Key { code: code.into(), down };
    let copy = [key(theirs, true), key("KeyC", true), key("KeyC", false), key(theirs, false)];
    let injected = |fake: &Fake| fake.inputs.lock().unwrap().iter().map(|(_, ev)| ev.clone()).collect::<Vec<_>>();

    let fake = Arc::new(Fake::default());
    let (n, _) = pair(ScreenDecision { allow: true, control: true }, Some(fake.clone())).await;
    let (started, mut input, _output) = n
        .remote
        .open_screen(ScreenOpen { control: true, keyboard: Some(viewer), translate_keys: true, ..Default::default() })
        .await
        .unwrap();
    assert!(started.translate_keys);
    for ev in copy.clone() {
        input.send(&ViewerMsg::Input { ev }).await.unwrap();
    }
    let want = vec![key(ours, true), key("KeyC", true), key("KeyC", false), key(ours, false)];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while injected(&fake) != want && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(injected(&fake), want, "命令键要换成对方的");
    // 关掉：先全松开，之后原样发
    fake.inputs.lock().unwrap().clear();
    input.send(&ViewerMsg::TranslateKeys { on: false }).await.unwrap();
    for ev in copy.clone() {
        input.send(&ViewerMsg::Input { ev }).await.unwrap();
    }
    let mut raw = vec![InputEvent::ReleaseAll];
    raw.extend(copy.clone());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while injected(&fake) != raw && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(injected(&fake), raw, "关掉以后原样发");
    n.shutdown().await;

    // 老版本的观看方不报键盘习惯：不转换
    let fake = Arc::new(Fake::default());
    let (n, _) = pair(ScreenDecision { allow: true, control: true }, Some(fake.clone())).await;
    let (started, mut input, _output) = n.remote.open_screen(ScreenOpen { control: true, ..Default::default() }).await.unwrap();
    assert!(!started.translate_keys);
    for ev in copy.clone() {
        input.send(&ViewerMsg::Input { ev }).await.unwrap();
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while injected(&fake).len() < copy.len() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(injected(&fake), copy.to_vec());
    n.shutdown().await;
}

/// 编码格式协商：观看方说能解 H.265 就请编码器用 H.265，没说（老版本）就是 H.264
#[tokio::test(flavor = "multi_thread")]
async fn codec_is_negotiated() {
    use p2pcli_core::screen::VideoCodec;
    for (codecs, want) in [(vec![], VideoCodec::H264), (vec![VideoCodec::H264], VideoCodec::H264), (vec![VideoCodec::H265, VideoCodec::H264], VideoCodec::H265)] {
        let fake = Arc::new(Fake::default());
        let (n, _) = pair(ScreenDecision { allow: true, control: false }, Some(fake.clone())).await;
        let _s = n.remote.open_screen(ScreenOpen { display: None, quality: Quality::Balanced, control: false, codecs, ..Default::default() }).await.unwrap();
        assert_eq!(*fake.codec.lock().unwrap(), Some(want));
        n.shutdown().await;
    }
}

/// 颜色协商：观看方认得颜色标记才用全范围、广色域，解码配置里带上实际的格式；老观看方照旧是视频范围、sRGB
#[tokio::test(flavor = "multi_thread")]
async fn color_is_negotiated() {
    use p2pcli_core::screen::{Primaries, ScreenEvent};
    for (color_tags, wide_gamut, want) in [
        (false, false, (false, Primaries::Bt709)),
        // 老观看方不会说自己认得标记，就算屏幕是广色域也不用
        (false, true, (false, Primaries::Bt709)),
        (true, false, (true, Primaries::Bt709)),
        (true, true, (true, Primaries::P3)),
    ] {
        let fake = Arc::new(Fake::default());
        let (n, _) = pair(ScreenDecision { allow: true, control: false }, Some(fake.clone())).await;
        let (_, _input, mut output) = n
            .remote
            .open_screen(ScreenOpen { quality: Quality::Balanced, color_tags, wide_gamut, ..Default::default() })
            .await
            .unwrap();
        let config = loop {
            match tokio::time::timeout(Duration::from_secs(5), output.next()).await.unwrap().unwrap() {
                Some(ScreenEvent::Config(c)) => break c,
                Some(_) => continue,
                None => panic!("画面没给解码配置就结束了"),
            }
        };
        assert_eq!((config.full_range, config.primaries), want, "color_tags={color_tags} wide_gamut={wide_gamut}");
        n.shutdown().await;
    }
}

/// 4:4:4 协商：观看方能硬件解 H.265 4:4:4、用的是 H.265、又不是流畅档，才请被连方用真正的 4:4:4；
/// 别的情况（包括老观看方）一律 4:2:0
#[tokio::test(flavor = "multi_thread")]
async fn yuv444_is_negotiated() {
    use p2pcli_core::screen::{ScreenEvent, VideoCodec};
    let h265 = vec![VideoCodec::H265, VideoCodec::H264];
    for (color_tags, chroma444, codecs, quality, want) in [
        (true, true, h265.clone(), Quality::Balanced, true),
        (true, true, h265.clone(), Quality::Sharp, true),
        (true, true, h265.clone(), Quality::Smooth, false),
        (true, true, vec![VideoCodec::H264], Quality::Sharp, false),
        (true, false, h265.clone(), Quality::Sharp, false),
        (false, true, h265.clone(), Quality::Sharp, false),
    ] {
        let fake = Arc::new(Fake::default());
        let (n, _) = pair(ScreenDecision { allow: true, control: false }, Some(fake.clone())).await;
        let (_, _input, mut output) = n
            .remote
            .open_screen(ScreenOpen { quality, color_tags, chroma444, codecs: codecs.clone(), ..Default::default() })
            .await
            .unwrap();
        let config = loop {
            match tokio::time::timeout(Duration::from_secs(5), output.next()).await.unwrap().unwrap() {
                Some(ScreenEvent::Config(c)) => break c,
                Some(_) => continue,
                None => panic!("画面没给解码配置就结束了"),
            }
        };
        assert_eq!(config.chroma444, want, "color_tags={color_tags} chroma444={chroma444} codecs={codecs:?} quality={quality:?}");
        n.shutdown().await;
    }
}

/// 剪贴板互通：清单先到、粘贴键排在后面；大内容和文件粘贴时才传（两个方向）；对方放进来的不发回去
#[tokio::test(flavor = "multi_thread")]
async fn clipboard_is_shared_while_controlling() {
    let order = Arc::new(Mutex::new(vec![]));
    let host_clip = Arc::new(FakeClip { order: order.clone(), ..Default::default() });
    host_clip.copy(ClipContent { formats: vec![("text".into(), Some(b"host old".to_vec()))], files: vec![] });
    let fake = Arc::new(Fake { clip: Some(host_clip.clone()), order: order.clone(), ..Default::default() });
    let (n, _) = pair(ScreenDecision { allow: true, control: true }, Some(fake.clone())).await;
    let (started, mut input, mut output) = n.remote.open_screen(ScreenOpen { control: true, clipboard: true, ..Default::default() }).await.unwrap();
    assert!(started.clipboard);
    assert_ne!(started.clip_token, 0);

    // 观看方剪贴板：一段文字、一张大图、一个文件夹
    let dir = std::env::temp_dir().join(format!("p2pcli-clip-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("folder/sub")).unwrap();
    let big: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(dir.join("folder/a.txt"), b"hello").unwrap();
    std::fs::write(dir.join("folder/sub/b.bin"), &big).unwrap();
    std::fs::create_dir_all(dir.join("folder/empty")).unwrap();
    let viewer_clip = Arc::new(FakeClip::default());
    viewer_clip.copy(ClipContent {
        formats: vec![("text".into(), Some("本机复制的".as_bytes().to_vec())), ("png".into(), Some(big.clone())), ("mac:com.example.lazy".into(), None)],
        files: vec![dir.join("folder")],
    });
    let vclip = ViewerClip::new(viewer_clip.clone(), n.remote.clone(), started.clip_token);

    // 画面一直收着、确认着；被连方要内容就送过去，发来的清单交给测试
    let (offers_tx, mut offers) = tokio::sync::mpsc::unbounded_channel();
    let (ack_tx, mut ack_rx) = tokio::sync::mpsc::unbounded_channel::<u64>();
    let pump = {
        let vclip = vclip.clone();
        tokio::spawn(async move {
            while let Ok(Some(ev)) = output.next().await {
                match ev {
                    ScreenEvent::Frame { seq, .. } => { let _ = ack_tx.send(seq); }
                    ScreenEvent::Host(HostMsg::ClipOffer { offer }) => { let _ = offers_tx.send(offer); }
                    ScreenEvent::Host(HostMsg::ClipWant { req, offer, what }) => {
                        let vclip = vclip.clone();
                        tokio::spawn(async move { vclip.put(req, offer, what).await.unwrap() });
                    }
                    _ => {}
                }
            }
        })
    };
    let sender = tokio::spawn({
        let vclip = vclip.clone();
        async move {
        let mut first = true;
        loop {
            tokio::select! {
                Some(seq) = ack_rx.recv() => { input.send(&ViewerMsg::Ack { seq }).await.unwrap(); }
                _ = tokio::time::sleep(Duration::from_millis(10)), if first => {
                    first = false;
                    let s = vclip_poll(&vclip).await.expect("一开始就发本机剪贴板");
                    assert_eq!(s.format("text").unwrap().data.as_deref(), Some("本机复制的".as_bytes()));
                    assert!(s.format("png").unwrap().data.is_none(), "大图不该带在清单里");
                    assert_eq!(s.files.len(), 1);
                    input.send(&ViewerMsg::ClipOffer { offer: s }).await.unwrap();
                    input.send(&ViewerMsg::Input { ev: InputEvent::Key { code: "KeyV".into(), down: true } }).await.unwrap();
                }
                else => break,
            }
        }
    }});
    async fn vclip_poll(v: &Arc<ViewerClip>) -> Option<p2pcli_core::screen_clip::ClipOffer> {
        let sync = v.sync.clone();
        tokio::task::spawn_blocking(move || sync.poll()).await.unwrap().unwrap()
    }

    // 清单放好了才按粘贴键
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while order.lock().unwrap().len() < 2 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(*order.lock().unwrap(), vec!["clip".to_string(), "key:KeyV".to_string()]);

    // 被连方这边粘贴：小的直接有，大图和文件现传
    let source = host_clip.written().unwrap();
    let (text, png, files) = tokio::task::spawn_blocking(move || {
        (source.format("text").unwrap(), source.format("png").unwrap(), source.download().unwrap())
    })
    .await
    .unwrap();
    assert_eq!(text, "本机复制的".as_bytes());
    assert_eq!(png, big);
    // 只有名字的格式：粘贴时才读
    let source = host_clip.written().unwrap();
    let lazy = tokio::task::spawn_blocking(move || source.format("mac:com.example.lazy").unwrap()).await.unwrap();
    assert_eq!(lazy, b"lazy");
    assert_eq!(files.len(), 1);
    assert!(files[0].ends_with("folder"));
    assert_eq!(std::fs::read(files[0].join("a.txt")).unwrap(), b"hello");
    assert_eq!(std::fs::read(files[0].join("sub/b.bin")).unwrap(), big);
    assert!(files[0].join("empty").is_dir());

    // 被连方原有的、刚放进来的观看方的，都不发回去
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(offers.try_recv().is_err(), "不该发回去");

    // 被连方在画面里复制了：清单发给观看方，大的粘贴时现要
    let rtf: Vec<u8> = (0..700_000u32).map(|i| (i % 7) as u8).collect();
    host_clip.copy(ClipContent { formats: vec![("text".into(), Some(b"from host".to_vec())), ("rtf".into(), Some(rtf.clone()))], files: vec![] });
    let offer = timeout(Duration::from_secs(3), offers.recv()).await.unwrap().unwrap();
    assert_eq!(offer.format("text").unwrap().data.as_deref(), Some(&b"from host"[..]));
    let (sync, fetch) = (vclip.sync.clone(), vclip.fetcher());
    tokio::task::spawn_blocking(move || sync.apply(offer, fetch)).await.unwrap().unwrap();
    let source = viewer_clip.written().unwrap();
    let got = tokio::task::spawn_blocking(move || source.format("rtf").unwrap()).await.unwrap();
    assert_eq!(got, rtf);
    // 观看方刚放进来的不再发回去
    assert!(vclip_poll(&vclip).await.is_none());

    sender.abort();
    pump.abort();
    n.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);

    // 只给看：不互通
    let fake = Arc::new(Fake { clip: Some(Arc::new(FakeClip::default())), ..Default::default() });
    let (n, _) = pair(ScreenDecision { allow: true, control: false }, Some(fake.clone())).await;
    let (started, _input, _output) = n.remote.open_screen(ScreenOpen { control: true, clipboard: true, ..Default::default() }).await.unwrap();
    assert!(!started.clipboard);
    n.shutdown().await;

    // 观看方没要（老版本）：不互通
    let fake = Arc::new(Fake { clip: Some(Arc::new(FakeClip::default())), ..Default::default() });
    let (n, _) = pair(ScreenDecision { allow: true, control: true }, Some(fake.clone())).await;
    let (started, _input, _output) = n.remote.open_screen(ScreenOpen { control: true, ..Default::default() }).await.unwrap();
    assert!(!started.clipboard);
    n.shutdown().await;
}

/// 单帧截屏：要对方同意；同意了传回整张图（比一帧上限大，分块传）；不同意、没有画面实现都说清楚
#[tokio::test(flavor = "multi_thread")]
async fn screenshot_needs_approval_and_arrives_whole() {
    let fake = Arc::new(Fake::default());
    let (n, delegate) = pair(ScreenDecision { allow: true, control: true }, Some(fake.clone())).await;
    assert!(n.remote.supports_screenshot());
    let (shot, display, relay) = n.remote.screenshot(Some("2".into()), 800).await.unwrap();
    assert_eq!(display, "2");
    assert!(!relay, "本机回环是直连");
    assert_eq!((shot.width, shot.height), (800, 500));
    assert!(shot.png.starts_with(b"PNG:2:800:"));
    assert_eq!(shot.png.len(), "PNG:2:800:".len() + 2_500_000);
    // 不写屏幕就是主屏；宽度按上限收住
    let (shot, display, _) = n.remote.screenshot(None, 10_000).await.unwrap();
    assert_eq!(display, "1");
    assert_eq!(shot.width, 3840);
    let req = delegate.screen_requests.lock().unwrap().last().cloned().unwrap();
    assert!(!req.control, "截屏只是看");
    assert!(delegate.events.lock().unwrap().iter().any(|e| matches!(e, HostEvent::ScreenshotTaken { .. })));
    n.shutdown().await;

    // 对方不同意
    let fake = Arc::new(Fake::default());
    let (n, _) = pair(ScreenDecision { allow: false, control: false }, Some(fake)).await;
    let err = n.remote.screenshot(None, 800).await.err().unwrap();
    assert!(format!("{err:#}").contains("denied"), "{err:#}");
    n.shutdown().await;

    // 对方没有远程画面实现：不报能力，直接说
    let (n, _) = pair(ScreenDecision { allow: true, control: true }, None).await;
    assert!(!n.remote.supports_screenshot());
    assert!(n.remote.screenshot(None, 800).await.is_err());
    n.shutdown().await;
}
