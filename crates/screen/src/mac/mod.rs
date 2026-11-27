//! macOS：ScreenCaptureKit → VideoToolbox 硬件编码；CGEvent 注入键鼠

use std::sync::{Arc, Mutex};

use anyhow::{Result, bail};
use objc2_core_foundation::{CFBoolean, CFDictionary, CFString};
use objc2_core_graphics::{CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess};
use p2pcli_core::screen::{CaptureOptions, DisplayInfo, InputEvent, ScreenBackend, ScreenCapture};

mod audio;
mod capture;
pub(crate) mod clip;
mod privacy;
mod shot;
mod virtual_display;
mod cursor;
pub mod encode;
mod flow;
mod input;
/// Metal 把彩色画面转成 NV12：只给测试造画面用
#[cfg(test)]
mod pack;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: Option<&CFDictionary>) -> bool;
    static kAXTrustedCheckOptionPrompt: &'static CFString;
}

pub fn permissions() -> crate::Permissions {
    crate::Permissions { capture: CGPreflightScreenCaptureAccess(), control: unsafe { AXIsProcessTrusted() } }
}

pub fn request_permission(capture: bool) {
    if capture {
        CGRequestScreenCaptureAccess();
    } else {
        prompt_accessibility();
    }
}

/// 弹“辅助功能”授权提示（已经给过就什么都不弹）；返回现在有没有权限
fn prompt_accessibility() -> bool {
    unsafe {
        let key: &CFString = kAXTrustedCheckOptionPrompt;
        let options = CFDictionary::<CFString, CFBoolean>::from_slices(&[key], &[CFBoolean::new(true)]);
        AXIsProcessTrustedWithOptions(Some(options.as_opaque()))
    }
}

pub struct MacBackend {
    input: Mutex<input::Injector>,
    cursor: cursor::CursorReader,
    /// 上次看的前台程序是不是终端、什么时候看的：一串快捷键里只问一次系统
    terminal: Mutex<Option<(std::time::Instant, bool)>>,
    /// 无头模式：一块显示器都没有时建的虚拟显示器
    headless: virtual_display::Headless,
}

impl MacBackend {
    pub fn new() -> Self {
        MacBackend {
            input: Mutex::new(input::Injector::new()),
            cursor: cursor::CursorReader::default(),
            terminal: Mutex::new(None),
            headless: virtual_display::Headless::default(),
        }
    }
}

/// 常见终端程序的包名。前台是它们时，Windows 那边的 Ctrl 按 ⌃ 发（Ctrl+C 是中断）
const TERMINALS: &[&str] = &[
    "com.apple.Terminal",
    "com.googlecode.iterm2",
    "net.kovidgoyal.kitty",
    "com.github.wez.wezterm",
    "org.alacritty",
    "io.alacritty",
    "com.mitchellh.ghostty",
    "dev.warp.Warp-Stable",
    "dev.warp.Warp",
    "co.zeit.hyper",
    "org.tabby",
    "com.raphaelamorim.rio",
];

fn frontmost_bundle() -> Option<String> {
    let app = objc2_app_kit::NSWorkspace::sharedWorkspace().frontmostApplication()?;
    Some(app.bundleIdentifier()?.to_string())
}

impl ScreenBackend for MacBackend {
    fn displays(&self) -> Result<Vec<DisplayInfo>> {
        let mut list = capture::displays()?;
        let real = list.iter().filter(|d| !d.id.parse().is_ok_and(|id| self.headless.is_virtual(id))).count();
        // 无头模式：一块屏幕都没有时自己建一块虚拟的，远程画面照样能看、能操作
        if let Some(id) = self.headless.ensure(real) {
            let id = id.to_string();
            if !list.iter().any(|d| d.id == id) {
                list = capture::displays()?;
            }
            for d in list.iter_mut().filter(|d| d.id == id) {
                d.name = "虚拟显示器（没接屏幕时用）".into();
                d.primary |= real == 0;
            }
        }
        Ok(list)
    }

    fn check_capture(&self) -> Result<()> {
        if CGPreflightScreenCaptureAccess() {
            return Ok(());
        }
        // 在被连方的电脑上弹系统的授权提示，那边的人点了以后要重开 P2PCLI 才生效
        CGRequestScreenCaptureAccess();
        bail!(
            "permission: 对方电脑还没允许 P2PCLI 录制屏幕。请在对方电脑上打开 系统设置 → 隐私与安全性 → 录屏与系统录音，打开 P2PCLI，然后重新打开 P2PCLI"
        )
    }

    fn check_control(&self) -> Result<()> {
        if prompt_accessibility() {
            return Ok(());
        }
        bail!("对方电脑还没允许 P2PCLI 控制键盘鼠标，现在只能看。要操作的话，请在对方电脑上打开 系统设置 → 隐私与安全性 → 辅助功能，打开 P2PCLI")
    }

    fn start(&self, opts: CaptureOptions) -> Result<Arc<dyn ScreenCapture>> {
        // 抓的是虚拟显示器：抓屏期间拿着它，画面关了它才可能被收掉
        let hold = opts.display.parse().ok().and_then(|id| self.headless.hold(id)).map(|d| Box::new(d) as Box<dyn std::any::Any + Send + Sync>);
        crate::awake::KeepAwake::start_holding(|| Ok(Arc::new(capture::MacCapture::start(opts)?)), hold)
    }

    fn inject(&self, display: &str, ev: &InputEvent) -> Result<()> {
        self.input.lock().unwrap().inject(display, ev)
    }

    fn input_v2(&self) -> bool {
        true
    }

    fn audio_supported(&self) -> bool {
        true
    }

    fn start_audio(&self, sink: p2pcli_core::screen_audio::AudioSink) -> Result<Box<dyn std::any::Any + Send>> {
        Ok(Box::new(audio::MacAudio::start(sink)?))
    }

    fn frontmost_terminal(&self) -> bool {
        let mut cache = self.terminal.lock().unwrap();
        if let Some((at, yes)) = *cache
            && at.elapsed() < std::time::Duration::from_millis(300)
        {
            return yes;
        }
        let yes = frontmost_bundle().is_some_and(|id| TERMINALS.contains(&id.as_str()));
        *cache = Some((std::time::Instant::now(), yes));
        yes
    }

    fn cursor(&self, display: &str) -> Option<p2pcli_core::screen::CursorInfo> {
        self.cursor.read(display)
    }

    fn clipboard(&self) -> Option<Arc<dyn p2pcli_core::screen_clip::ClipPlatform>> {
        crate::clipboard()
    }

    fn screenshot(&self, display: &str, max_width: u32) -> Result<p2pcli_core::screen::Screenshot> {
        shot::capture(display, max_width)
    }

    fn privacy_supported(&self) -> bool {
        true
    }

    fn start_privacy(&self, style: &p2pcli_core::screen::PrivacyStyle, block_input: bool) -> Result<Box<dyn std::any::Any + Send>> {
        Ok(Box::new(privacy::MacPrivacy::start(style, block_input)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use p2pcli_core::screen::{CaptureOptions, Quality, VideoCodec};

    /// 无头模式走一遍：强制建虚拟显示器，屏幕列表里有它；抓它能拿到 1920×1080 的关键帧；
    /// 抓屏停了 30 多秒以后它被收掉。会在本机多出一块屏幕约 40 秒，默认不跑：
    /// cargo test -p p2pcli-screen --lib -- --ignored headless_flow --nocapture
    #[test]
    #[ignore]
    fn headless_flow() {
        let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();
        unsafe { std::env::set_var("P2PCLI_VIRTUAL_DISPLAY", "1") };
        let backend = MacBackend::new();
        let list = backend.displays().unwrap();
        let v = list.iter().find(|d| d.name.starts_with("虚拟显示器")).expect("屏幕列表里没有虚拟显示器").clone();
        println!("屏幕列表：{:?}", list.iter().map(|d| format!("{} {} {}×{}", d.id, d.name, d.width, d.height)).collect::<Vec<_>>());
        let q = Quality::Balanced;
        let cap = backend
            .start(CaptureOptions {
                display: v.id.clone(),
                max_long_edge: q.max_long_edge(VideoCodec::H265),
                fps: 60,
                bitrate: q.start_bitrate(),
                show_cursor: true,
                codec: VideoCodec::H265,
                full_range: false,
                wide_gamut: false,
                yuv444: false,
            })
            .unwrap();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let frame = rt.block_on(async { tokio::time::timeout(std::time::Duration::from_secs(5), cap.next_frame()).await }).expect("等不到虚拟显示器的画面").unwrap();
        let c = frame.config.expect("第一帧要带解码配置");
        println!("虚拟显示器的画面：{} {}×{}", c.codec, c.width, c.height);
        assert_eq!((c.width, c.height), (1920, 1080));
        // 键鼠能打到它上面：挪到它中间，读回指针位置
        let id: u32 = v.id.parse().unwrap();
        backend.inject(&v.id, &InputEvent::Move { x: 0.5, y: 0.5 }).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let b = objc2_core_graphics::CGDisplayBounds(id);
        let at = objc2_core_graphics::CGEvent::location(objc2_core_graphics::CGEvent::new(None).as_deref());
        println!("指针 {:?}，虚拟显示器范围 {:?}", (at.x, at.y), (b.origin.x, b.origin.y, b.size.width, b.size.height));
        assert!(at.x >= b.origin.x && at.x < b.origin.x + b.size.width && at.y >= b.origin.y && at.y < b.origin.y + b.size.height, "指针没挪到虚拟显示器上");
        drop(cap);
        // 没人看 30 秒以后收掉（每 5 秒查一次）
        std::thread::sleep(std::time::Duration::from_secs(42));
        assert!(!backend.headless.is_virtual(id), "没人看了虚拟显示器还在");
        unsafe { std::env::remove_var("P2PCLI_VIRTUAL_DISPLAY") };
        let after = capture::displays().unwrap();
        assert!(!after.iter().any(|d| d.id == v.id), "虚拟显示器收掉了还在屏幕列表里");
    }

    /// 锁屏时能不能隔着远程打字：往锁屏界面打一个 a，截图存成 /tmp/p2pcli-lock-typed.png（看密码框里有没有圆点），
    /// 再按两下退格删掉。不按回车，不会去解锁。默认不跑
    #[test]
    #[ignore]
    fn lock_screen_typing_probe() {
        let backend = MacBackend::new();
        let main = objc2_core_graphics::CGMainDisplayID().to_string();
        for (code, down) in [("KeyA", true), ("KeyA", false)] {
            backend.inject(&main, &InputEvent::Key { code: code.into(), down }).unwrap();
        }
        std::thread::sleep(std::time::Duration::from_millis(700));
        let shot = shot::capture(&main, 640).unwrap();
        std::fs::write("/tmp/p2pcli-lock-typed.png", &shot.png).unwrap();
        println!("打了一个 a，截图 {}×{}", shot.width, shot.height);
        for _ in 0..2 {
            for down in [true, false] {
                backend.inject(&main, &InputEvent::Key { code: "Backspace".into(), down }).unwrap();
            }
        }
    }

    /// 锁屏时系统抓屏拿到的是什么：打印锁没锁，截主屏存成 /tmp/p2pcli-lock-probe.png。默认不跑
    #[test]
    #[ignore]
    fn lock_screen_probe() {
        let main = objc2_core_graphics::CGMainDisplayID();
        let shot = shot::capture(&main.to_string(), 640).unwrap();
        std::fs::write("/tmp/p2pcli-lock-probe.png", &shot.png).unwrap();
        println!("截到 {}×{}，存在 /tmp/p2pcli-lock-probe.png", shot.width, shot.height);
    }
    /// 能读出前台程序的包名（快捷键转换靠它判断是不是终端）。要有前台程序，默认不跑：
    /// cargo test -p p2pcli-screen --lib -- --ignored frontmost
    #[test]
    #[ignore]
    fn frontmost_app_is_readable() {
        let id = super::frontmost_bundle();
        println!("前台程序：{id:?}");
        assert!(id.is_some());
    }
}
