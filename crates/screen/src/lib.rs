//! 远程画面的平台实现。全走系统的硬件接口：
//!
//! - macOS：ScreenCaptureKit 抓屏，画面在 GPU 里缩放、转成 NV12，留在 IOSurface 里直接交给
//!   VideoToolbox 的硬件 H.264 编码器，中间不经过 CPU；CGEvent 注入键鼠。
//! - Windows：Windows.Graphics.Capture 抓成 D3D11 纹理，GPU 上转 NV12，交给显卡的硬件编码器
//!   （NVENC、Quick Sync、AMF，经 Media Foundation）；SendInput 注入键鼠。
//!
//! 编码器都是系统自带的，不用另外带编解码库，也没有许可证和专利费的问题。

use std::sync::Arc;

use p2pcli_core::screen::ScreenBackend;

pub mod keymap;
#[cfg(target_os = "macos")]
mod mac;
/// 验证用：直接拿到硬件编码器
#[cfg(target_os = "macos")]
#[doc(hidden)]
pub use mac::encode as mac_encode;
#[cfg(windows)]
mod win;

/// 本机的远程画面实现；这个系统还不支持就是空
pub fn backend() -> Option<Arc<dyn ScreenBackend>> {
    #[cfg(target_os = "macos")]
    return Some(Arc::new(mac::MacBackend::new()));
    #[cfg(windows)]
    return win::backend();
    #[allow(unreachable_code)]
    None
}

/// 系统权限：能不能录屏、能不能控制键鼠。只有 macOS 要单独授权，别的系统总是能
#[derive(Debug, Clone, Copy)]
pub struct Permissions {
    pub capture: bool,
    pub control: bool,
}

pub fn permissions() -> Permissions {
    #[cfg(target_os = "macos")]
    return mac::permissions();
    #[allow(unreachable_code)]
    Permissions { capture: true, control: true }
}

/// 弹系统的授权提示（macOS）。capture 为真问录屏，否则问辅助功能
pub fn request_permission(capture: bool) {
    #[cfg(target_os = "macos")]
    mac::request_permission(capture);
    #[cfg(not(target_os = "macos"))]
    let _ = capture;
}
