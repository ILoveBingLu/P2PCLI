//! macOS 单帧截屏：ScreenCaptureKit 的 SCScreenshotManager 直接按要的大小截一张（系统负责缩放），
//! 再用 NSBitmapImageRep 编成 PNG。要录屏权限，和远程画面一样。

use std::ptr::NonNull;
use std::time::Duration;

use anyhow::{Result, anyhow};
use objc2::AllocAnyThread;
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep};
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::{CGDirectDisplayID, CGImage, kCGColorSpaceSRGB};
use objc2_foundation::{NSDictionary, NSError};
use objc2_screen_capture_kit::{SCScreenshotManager, SCStreamConfiguration};
use p2pcli_core::screen::Screenshot;

use super::capture::{SendRetained, pixel_size, wait};

/// 按宽度上限等比缩小
fn fit(w: u32, h: u32, max_width: u32) -> (u32, u32) {
    if w <= max_width || w == 0 {
        return (w.max(1), h.max(1));
    }
    (max_width, ((h as u64 * max_width as u64) / w as u64).max(1) as u32)
}

pub(crate) fn capture(display: &str, max_width: u32) -> Result<Screenshot> {
    let id: CGDirectDisplayID = display.parse().map_err(|_| anyhow!("屏幕编号不对"))?;
    let filter = super::capture::content_filter(id)?;
    let (pw, ph) = pixel_size(id);
    let (w, h) = fit(pw, ph, max_width);
    let config = unsafe { SCStreamConfiguration::new() };
    unsafe {
        config.setWidth(w as usize);
        config.setHeight(h as usize);
        config.setShowsCursor(true);
        config.setScalesToFit(true);
        config.setColorSpaceName(kCGColorSpaceSRGB);
    }
    let image = wait(Duration::from_secs(10), |tx| {
        let block = block2::RcBlock::new(move |image: *mut CGImage, error: *mut NSError| {
            let result = match NonNull::new(image) {
                Some(ptr) => Ok(SendRetained(unsafe { CFRetained::retain(ptr) })),
                None => Err(unsafe { error.as_ref() }.map(|e| e.localizedDescription().to_string()).unwrap_or_default()),
            };
            let _ = tx.send(result);
        });
        unsafe { SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(&filter, &config, Some(&block)) };
    })?
    .map_err(|e| anyhow!("截屏失败：{e}"))?
    .0;
    let (width, height) = (CGImage::width(Some(&image)) as u32, CGImage::height(Some(&image)) as u32);
    let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &image);
    let png = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }.ok_or_else(|| anyhow!("编 PNG 失败"))?;
    Ok(Screenshot { png: png.to_vec(), width, height })
}
