//! macOS：ScreenCaptureKit → VideoToolbox 硬件编码；CGEvent 注入键鼠

use std::sync::{Arc, Mutex};

use anyhow::{Result, bail};
use objc2_core_foundation::{CFBoolean, CFDictionary, CFString};
use objc2_core_graphics::{CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess};
use p2pcli_core::screen::{CaptureOptions, DisplayInfo, InputEvent, ScreenBackend, ScreenCapture};

mod capture;
pub(crate) mod clip;
mod shot;
mod cursor;
pub mod encode;
mod flow;
mod input;
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
}

impl MacBackend {
    pub fn new() -> Self {
        MacBackend { input: Mutex::new(input::Injector::new()), cursor: cursor::CursorReader::default(), terminal: Mutex::new(None) }
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
        capture::displays()
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
        crate::awake::KeepAwake::start(|| Ok(Arc::new(capture::MacCapture::start(opts)?)))
    }

    fn inject(&self, display: &str, ev: &InputEvent) -> Result<()> {
        self.input.lock().unwrap().inject(display, ev)
    }

    fn input_v2(&self) -> bool {
        true
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
}

#[cfg(test)]
mod tests {
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
