//! 屏幕分辨率：列出系统给这块屏幕的分辨率（含 HiDPI 档），远程换成其中一种。
//! 改的是这次登录的设置（注销、重启后回到原来的），不写进系统偏好。

use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use objc2_core_foundation::{CFArray, CFBoolean, CFDictionary, CFRetained, CFString};
use objc2_core_graphics::{
    CGBeginDisplayConfiguration, CGCancelDisplayConfiguration, CGCompleteDisplayConfiguration, CGConfigureDisplayWithDisplayMode,
    CGConfigureOption, CGDirectDisplayID, CGDisplayConfigRef, CGDisplayCopyAllDisplayModes, CGDisplayCopyDisplayMode, CGDisplayMode,
    kCGDisplayShowDuplicateLowResolutionModes,
};
use p2pcli_core::screen::DisplayMode;
use tracing::info;

/// 列表里不要太小的（菜单里一长串没用的）
const MIN_WIDTH: u32 = 1024;

fn describe(mode: &CGDisplayMode) -> DisplayMode {
    let (w, pw) = (CGDisplayMode::width(Some(mode)), CGDisplayMode::pixel_width(Some(mode)));
    DisplayMode { width: w as u32, height: CGDisplayMode::height(Some(mode)) as u32, hidpi: pw > w }
}

/// 系统给这块屏幕的所有分辨率（连 HiDPI 的重复档一起）
fn all(id: CGDirectDisplayID) -> Vec<CFRetained<CGDisplayMode>> {
    let key: &CFString = unsafe { kCGDisplayShowDuplicateLowResolutionModes };
    let options = CFDictionary::<CFString, CFBoolean>::from_slices(&[key], &[CFBoolean::new(true)]);
    let Some(array) = (unsafe { CGDisplayCopyAllDisplayModes(id, Some(options.as_opaque())) }) else { return Vec::new() };
    let array: CFRetained<CFArray<CGDisplayMode>> = unsafe { CFRetained::cast_unchecked(array) };
    array.iter().filter(|m| CGDisplayMode::is_usable_for_desktop_gui(Some(m))).collect()
}

/// 现在用的分辨率
pub fn current(id: CGDirectDisplayID) -> Option<DisplayMode> {
    CGDisplayCopyDisplayMode(id).map(|m| describe(&m))
}

/// 能选的分辨率：先普通的、再 HiDPI 的，各自从大到小；max_width 以上的不要（虚拟屏的 4K 原生档字太小）
pub fn list(id: CGDirectDisplayID, max_width: u32) -> (Vec<DisplayMode>, Option<DisplayMode>) {
    let now = current(id);
    let mut modes: Vec<DisplayMode> = all(id).iter().map(|m| describe(m)).filter(|m| m.width >= MIN_WIDTH && m.width <= max_width).collect();
    if let Some(now) = now {
        modes.push(now);
    }
    modes.sort_by_key(|m| (m.hidpi, std::cmp::Reverse(m.width), std::cmp::Reverse(m.height)));
    modes.dedup();
    (modes, now)
}

/// 换成这个分辨率：同样大小有几种刷新率时，优先和现在一样的，没有就挑最高的。等系统认了新尺寸才返回
pub fn set(id: CGDirectDisplayID, want: &DisplayMode) -> Result<()> {
    if current(id).as_ref() == Some(want) {
        return Ok(());
    }
    let now_hz = CGDisplayCopyDisplayMode(id).map_or(0.0, |m| CGDisplayMode::refresh_rate(Some(&m)));
    let candidates: Vec<_> = all(id).into_iter().filter(|m| describe(m) == *want).collect();
    let mode = candidates
        .iter()
        .find(|m| (CGDisplayMode::refresh_rate(Some(m)) - now_hz).abs() < 0.5)
        .or_else(|| candidates.iter().max_by(|a, b| CGDisplayMode::refresh_rate(Some(a)).total_cmp(&CGDisplayMode::refresh_rate(Some(b)))))
        .ok_or_else(|| anyhow!("这块屏幕没有 {}×{}{} 这个分辨率", want.width, want.height, if want.hidpi { "（HiDPI）" } else { "" }))?;
    let pixels = (CGDisplayMode::pixel_width(Some(mode)) as u32, CGDisplayMode::pixel_height(Some(mode)) as u32);
    let mut config: CGDisplayConfigRef = std::ptr::null_mut();
    let err = unsafe { CGBeginDisplayConfiguration(&mut config) };
    if err.0 != 0 {
        bail!("改不了分辨率（{}）", err.0);
    }
    let err = unsafe { CGConfigureDisplayWithDisplayMode(config, id, Some(mode), None) };
    if err.0 != 0 {
        unsafe { CGCancelDisplayConfiguration(config) };
        bail!("改不了分辨率（{}）", err.0);
    }
    let err = unsafe { CGCompleteDisplayConfiguration(config, CGConfigureOption::ForSession) };
    if err.0 != 0 {
        bail!("改不了分辨率（{}）", err.0);
    }
    // 等系统换好：抓屏按新尺寸开
    let deadline = Instant::now() + Duration::from_secs(3);
    while super::capture::pixel_size(id) != pixels {
        if Instant::now() > deadline {
            bail!("系统没换成 {}×{}", want.width, want.height);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    info!(id, width = want.width, height = want.height, hidpi = want.hidpi, "换了分辨率");
    Ok(())
}
