//! 隐私屏两个平台共用的部分：按样式画背景（纯黑、内置渐变、自定义图片铺满），时钟的字，
//! 远程注入的键鼠打的标记（拦本机键鼠时认这个放行）。
//!
//! 遮挡窗口、拦键鼠各平台自己做：
//! - Windows：每块屏幕一个置顶窗口，SetWindowDisplayAffinity 让抓屏看不到它；低级键鼠钩子拦本机输入
//! - macOS：每块屏幕一个最高层窗口，抓屏时把它排除掉；事件拦截（event tap）拦本机输入

use std::path::Path;

use anyhow::{Context, Result};
use image::RgbaImage;
use p2pcli_core::screen::{PrivacyBackground, PrivacyStyle};

/// 远程注入的键鼠带的标记（Windows 是 SendInput 的 dwExtraInfo，macOS 是事件源的 userData）。
/// 隐私屏拦本机键鼠时，带这个标记的放行
pub(crate) const INPUT_TAG: u32 = 0x5032_4349;

/// 内置渐变：左上角到右下角
const PRESETS: [([u8; 3], [u8; 3]); p2pcli_core::screen::PRIVACY_PRESETS as usize] = [
    // 深海
    ([15, 32, 39], [44, 83, 100]),
    // 暮色
    ([40, 22, 78], [176, 78, 96]),
    // 森林
    ([12, 52, 50], [86, 150, 104]),
];

/// 背景图，宽高是屏幕的像素。纯黑返回空（平台自己涂黑，不用传一大张图）。
/// 有字的时候把图片压暗一点，白字看得清
pub(crate) fn background(style: &PrivacyStyle, width: u32, height: u32) -> Result<Option<RgbaImage>> {
    let (width, height) = (width.max(1), height.max(1));
    let has_text = style.clock || !style.message.trim().is_empty();
    match &style.background {
        PrivacyBackground::Black => Ok(None),
        PrivacyBackground::Preset { index } => {
            let (a, b) = PRESETS[(*index as usize).min(PRESETS.len() - 1)];
            // 渐变很平滑，按四分之一画再交给系统放大，省时间
            let (w, h) = ((width / 4).max(2), (height / 4).max(2));
            let diag = (w + h - 2).max(1) as f32;
            Ok(Some(RgbaImage::from_fn(w, h, |x, y| {
                let t = (x + y) as f32 / diag;
                let c = |i: usize| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t).round() as u8;
                image::Rgba([c(0), c(1), c(2), 255])
            })))
        }
        PrivacyBackground::Image { path } => {
            let mut img = load_cover(path, width, height)?;
            if has_text {
                for p in img.pixels_mut() {
                    for c in &mut p.0[..3] {
                        *c = (*c as u16 * 3 / 5) as u8;
                    }
                }
            }
            Ok(Some(img))
        }
    }
}

/// 读图片，等比放大或缩小到盖满 width×height，多出来的从中间裁掉。太大的屏幕按 3840 宽画，系统再放大
fn load_cover(path: &Path, width: u32, height: u32) -> Result<RgbaImage> {
    // 存的时候不带扩展名，格式按内容认
    let img = image::ImageReader::open(path)
        .and_then(|r| r.with_guessed_format())
        .map_err(anyhow::Error::from)
        .and_then(|r| Ok(r.decode()?))
        .with_context(|| format!("读不了隐私屏图片 {}", path.display()))?;
    let scale = (3840.0 / width.max(height) as f64).min(1.0);
    let (w, h) = (((width as f64 * scale) as u32).max(1), ((height as f64 * scale) as u32).max(1));
    Ok(img.resize_to_fill(w, h, image::imageops::FilterType::Triangle).to_rgba8())
}

/// 时钟上显示的字：本地时间的时和分
pub(crate) fn clock_text() -> String {
    chrono::Local::now().format("%H:%M").to_string()
}

/// 时钟下面一行小字：日期和星期
pub(crate) fn date_text() -> String {
    use chrono::Datelike;
    let now = chrono::Local::now();
    let week = ["一", "二", "三", "四", "五", "六", "日"][now.weekday().num_days_from_monday() as usize];
    format!("{}月{}日 星期{week}", now.month(), now.day())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backgrounds() {
        let style = |background| PrivacyStyle { background, clock: false, message: String::new() };
        assert!(background(&style(PrivacyBackground::Black), 1920, 1080).unwrap().is_none());
        let g = background(&style(PrivacyBackground::Preset { index: 1 }), 1920, 1080).unwrap().unwrap();
        assert_eq!(g.dimensions(), (480, 270));
        // 左上角是起始色、右下角是结束色
        assert_eq!(g.get_pixel(0, 0).0, [40, 22, 78, 255]);
        assert_eq!(g.get_pixel(479, 269).0, [176, 78, 96, 255]);
        // 越界的编号用最后一种
        assert!(background(&style(PrivacyBackground::Preset { index: 99 }), 100, 100).is_ok());
        // 图片铺满：4×2 的图盖到 300×300 上
        let dir = std::env::temp_dir().join(format!("p2pcli-privacy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bg.png");
        RgbaImage::from_fn(4, 2, |x, _| if x < 2 { image::Rgba([255, 0, 0, 255]) } else { image::Rgba([0, 0, 255, 255]) }).save(&path).unwrap();
        let img = background(&style(PrivacyBackground::Image { path: path.clone() }), 300, 300).unwrap().unwrap();
        assert_eq!(img.dimensions(), (300, 300));
        assert!(img.get_pixel(10, 150).0[0] > 200, "左边是红的");
        assert!(img.get_pixel(290, 150).0[2] > 200, "右边是蓝的");
        // 有字时压暗
        let dim = background(&PrivacyStyle { clock: true, ..style(PrivacyBackground::Image { path: path.clone() }) }, 300, 300).unwrap().unwrap();
        assert!(dim.get_pixel(10, 150).0[0] < 200);
        // 读不了的图片报错
        assert!(background(&style(PrivacyBackground::Image { path: dir.join("没有.png") }), 10, 10).is_err());
        let _ = std::fs::remove_dir_all(dir);
        assert_eq!(clock_text().len(), 5);
        assert!(date_text().contains("星期"));
    }
}
