//! 看画面期间让屏幕亮着：开始时把熄了的屏幕叫醒，看的过程中不让系统因为没人动而熄屏。
//! 画面停了（抓屏对象丢掉）就撤销，系统照原来的节能设置走。
//!
//! 叫醒的是显示器，不是解锁：对方电脑锁着屏时，看到的是锁屏界面（Windows 的锁屏是安全桌面，抓不到也操作不了）。

use std::sync::Arc;

use anyhow::Result;
use p2pcli_core::host::BoxFuture;
use p2pcli_core::screen::{EncodedFrame, ScreenCapture};

/// 抓屏对象外面包一层：活着时屏幕不熄
pub(crate) struct KeepAwake {
    inner: Arc<dyn ScreenCapture>,
    _guard: Guard,
}

impl KeepAwake {
    /// 先叫醒屏幕再开始抓
    pub(crate) fn start(start: impl FnOnce() -> Result<Arc<dyn ScreenCapture>>) -> Result<Arc<dyn ScreenCapture>> {
        let guard = Guard::new();
        let inner = start()?;
        Ok(Arc::new(KeepAwake { inner, _guard: guard }))
    }
}

impl ScreenCapture for KeepAwake {
    fn next_frame(&self) -> BoxFuture<Result<EncodedFrame>> {
        self.inner.next_frame()
    }
    fn request_keyframe(&self) {
        self.inner.request_keyframe()
    }
    fn set_bitrate(&self, bps: u32) {
        self.inner.set_bitrate(bps)
    }
    fn color(&self) -> p2pcli_core::screen::ColorFormat {
        self.inner.color()
    }
    fn stats(&self) -> Option<p2pcli_core::screen::CaptureStats> {
        self.inner.stats()
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use objc2_core_foundation::CFString;
    use tracing::{debug, warn};

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOPMAssertionCreateWithName(kind: &CFString, level: u32, name: &CFString, id: *mut u32) -> i32;
        fn IOPMAssertionDeclareUserActivity(name: &CFString, user_type: u32, id: *mut u32) -> i32;
        fn IOPMAssertionRelease(id: u32) -> i32;
    }
    const LEVEL_ON: u32 = 255;
    /// kIOPMUserActiveLocal：本机有人在用
    const USER_ACTIVE_LOCAL: u32 = 0;

    pub(crate) struct Guard {
        ids: Vec<u32>,
    }

    impl Guard {
        pub(crate) fn new() -> Self {
            let name = CFString::from_static_str("P2PCLI 远程画面");
            let mut ids = Vec::new();
            unsafe {
                // 当作本机有人动了一下：熄了的屏幕会亮起来
                let mut id = 0;
                match IOPMAssertionDeclareUserActivity(&name, USER_ACTIVE_LOCAL, &mut id) {
                    0 => ids.push(id),
                    code => warn!(code, "叫醒屏幕失败"),
                }
                // 看的过程中不因为没人动而熄屏
                let mut id = 0;
                match IOPMAssertionCreateWithName(&CFString::from_static_str("PreventUserIdleDisplaySleep"), LEVEL_ON, &name, &mut id) {
                    0 => ids.push(id),
                    code => warn!(code, "设置看画面期间不熄屏失败"),
                }
            }
            debug!("远程画面：屏幕保持亮着");
            Guard { ids }
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            for id in self.ids.drain(..) {
                unsafe { IOPMAssertionRelease(id) };
            }
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::sync::mpsc::{Sender, channel};

    use tracing::warn;
    use windows::Win32::System::Power::{ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, SetThreadExecutionState};
    use windows::Win32::UI::Input::KeyboardAndMouse::{INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_MOVE, MOUSEINPUT, SendInput};

    /// “不熄屏”是按线程记的，线程结束就撤销：单独起一个线程拿着，丢掉 Guard 时线程退出
    pub(crate) struct Guard {
        _stop: Sender<()>,
    }

    impl Guard {
        pub(crate) fn new() -> Self {
            let (stop, rx) = channel::<()>();
            let spawned = std::thread::Builder::new().name("screen-awake".into()).spawn(move || {
                unsafe {
                    if SetThreadExecutionState(ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED).0 == 0 {
                        warn!("设置看画面期间不熄屏失败");
                    }
                    // 熄了的屏幕要有输入才亮：发一个原地不动的鼠标移动，指针不会跑
                    let input = INPUT {
                        r#type: INPUT_MOUSE,
                        Anonymous: INPUT_0 { mi: MOUSEINPUT { dx: 0, dy: 0, mouseData: 0, dwFlags: MOUSEEVENTF_MOVE, time: 0, dwExtraInfo: 0 } },
                    };
                    SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
                }
                // Guard 丢掉时发送端跟着丢，这里就返回
                let _ = rx.recv();
                unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
            });
            if let Err(e) = spawned {
                warn!("起不了保持亮屏的线程：{e}");
            }
            Guard { _stop: stop }
        }
    }
}

use imp::Guard;
