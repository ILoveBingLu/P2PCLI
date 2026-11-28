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

#[cfg(any(target_os = "macos", windows))]
mod awake;
#[cfg(any(target_os = "macos", windows))]
mod privacy;
pub mod keymap;
#[cfg(target_os = "macos")]
mod mac;
/// 验证用：直接拿到硬件编码器
#[cfg(target_os = "macos")]
#[doc(hidden)]
pub use mac::encode as mac_encode;
#[cfg(windows)]
mod win;
/// 验证用：本机的真显示器是不是都被增强隐私屏弄黑了
#[cfg(target_os = "macos")]
#[doc(hidden)]
pub fn mac_screens_black() -> bool {
    mac::blackout::all_black()
}

/// 隐私屏背景的小样（PNG 的 data URL），设置页里给人挑
#[cfg(any(target_os = "macos", windows))]
pub fn privacy_thumbnail(background: &p2pcli_core::screen::PrivacyBackground, width: u32, height: u32) -> anyhow::Result<String> {
    use base64::Engine;
    let png = privacy::thumbnail(background, width, height)?;
    Ok(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png)))
}

/// 别的系统没有隐私屏
#[cfg(not(any(target_os = "macos", windows)))]
pub fn privacy_thumbnail(_background: &p2pcli_core::screen::PrivacyBackground, _width: u32, _height: u32) -> anyhow::Result<String> {
    anyhow::bail!("这个系统不支持隐私屏")
}

/// 看一张图片能不能当隐私屏背景（读得出来、是认得的格式）
pub fn check_privacy_image(path: &std::path::Path) -> anyhow::Result<()> {
    let (w, h) = image::ImageReader::open(path)?.with_guessed_format()?.into_dimensions().map_err(|e| anyhow::anyhow!("这不是认得的图片（支持 PNG、JPG、BMP、WebP、GIF）：{e}"))?;
    if w == 0 || h == 0 || (w as u64) * (h as u64) > 100_000_000 {
        anyhow::bail!("图片尺寸不对：{w}×{h}");
    }
    Ok(())
}

/// 本机剪贴板（远程控制时和对方互通）；这个系统还不支持就是空
pub fn clipboard() -> Option<Arc<dyn p2pcli_core::screen_clip::ClipPlatform>> {
    #[cfg(target_os = "macos")]
    return Some(Arc::new(mac::clip::MacClip));
    #[cfg(windows)]
    return Some(Arc::new(win::clip::WinClip));
    #[allow(unreachable_code)]
    None
}

/// 本机的远程画面实现；这个系统还不支持就是空
pub fn backend() -> Option<Arc<dyn ScreenBackend>> {
    #[cfg(target_os = "macos")]
    return Some(Arc::new(mac::MacBackend::new()));
    #[cfg(windows)]
    return win::backend();
    #[allow(unreachable_code)]
    None
}

/// 合成屏幕：不抓真屏幕，按帧率定时出会动的画面（假装是一块 2560×1600 的主屏幕），
/// 之后走和真抓屏一样的缩放、4:4:4、编码、流控和计数，给端到端的网络测试用。现在只有 Windows 有
pub fn synthetic_backend() -> Option<Arc<dyn ScreenBackend>> {
    #[cfg(windows)]
    return win::synthetic_backend();
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
