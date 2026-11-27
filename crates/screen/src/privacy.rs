//! 隐私屏两个平台共用的部分：按样式画背景（纯黑、内置主题、自定义图片铺满），时钟的字，
//! 远程注入的键鼠打的标记（拦本机键鼠时认这个放行）。
//!
//! 内置主题是“几团柔和的色光叠在底色上”，再加暗角和一点颗粒（放大以后不出色带），星空、极光再撒些星星。
//! 设置页里的小样也用这里画，看到的就是盖上以后的样子。
//!
//! 遮挡窗口、拦键鼠各平台自己做：
//! - Windows：每块屏幕一个置顶窗口，SetWindowDisplayAffinity 让抓屏看不到它；低级键鼠钩子拦本机输入
//! - macOS：每块屏幕一个最高层窗口，抓屏时把它排除掉；事件拦截（event tap）拦本机输入

use std::path::Path;

use anyhow::{Context, Result};
use image::RgbaImage;
use p2pcli_core::screen::{PRIVACY_PRESETS, PrivacyBackground, PrivacyStyle};

/// 远程注入的键鼠带的标记（Windows 是 SendInput 的 dwExtraInfo，macOS 是事件源的 userData）。
/// 隐私屏拦本机键鼠时，带这个标记的放行
pub(crate) const INPUT_TAG: u32 = 0x5032_4349;

/// 一团色光：中心（0~1，横向按屏幕比例拉伸）、半径（以屏幕高为 1）、颜色、浓度
struct Glow {
    x: f32,
    y: f32,
    r: f32,
    color: [u8; 3],
    a: f32,
}

struct Theme {
    /// 底色：上面一个、下面一个，竖着渐变
    top: [u8; 3],
    bottom: [u8; 3],
    glows: &'static [Glow],
    /// 每百万像素撒多少颗星
    stars: u32,
}

const fn g(x: f32, y: f32, r: f32, color: [u8; 3], a: f32) -> Glow {
    Glow { x, y, r, color, a }
}

/// 内置主题，顺序和设置页一致（老版本存的 0~2 还是深海、暮色、森林）
const THEMES: [Theme; PRIVACY_PRESETS as usize] = [
    // 深海
    Theme {
        top: [6, 22, 36],
        bottom: [4, 14, 26],
        glows: &[g(0.12, 0.15, 0.75, [22, 96, 128], 0.75), g(0.9, 0.9, 0.8, [28, 62, 120], 0.65), g(0.55, 0.45, 0.45, [18, 128, 150], 0.25)],
        stars: 0,
    },
    // 暮色
    Theme {
        top: [32, 16, 64],
        bottom: [22, 10, 44],
        glows: &[g(0.08, 0.1, 0.7, [84, 36, 140], 0.7), g(0.95, 0.95, 0.85, [206, 82, 104], 0.75), g(0.75, 0.35, 0.45, [236, 128, 92], 0.35)],
        stars: 0,
    },
    // 森林
    Theme {
        top: [8, 38, 34],
        bottom: [4, 24, 22],
        glows: &[g(0.15, 0.85, 0.8, [42, 118, 84], 0.75), g(0.88, 0.12, 0.7, [96, 156, 104], 0.5), g(0.5, 0.5, 0.5, [22, 78, 66], 0.4)],
        stars: 0,
    },
    // 极光
    Theme {
        top: [4, 8, 24],
        bottom: [8, 14, 36],
        glows: &[g(0.28, 0.32, 0.42, [36, 214, 160], 0.55), g(0.55, 0.22, 0.38, [56, 136, 236], 0.45), g(0.82, 0.4, 0.34, [166, 88, 226], 0.42), g(0.5, 1.05, 0.6, [20, 40, 90], 0.6)],
        stars: 260,
    },
    // 星空
    Theme {
        top: [2, 5, 16],
        bottom: [16, 20, 52],
        glows: &[g(0.5, 1.15, 0.9, [70, 56, 140], 0.55), g(0.15, 0.2, 0.5, [20, 34, 80], 0.4)],
        stars: 900,
    },
    // 日落
    Theme {
        top: [44, 22, 70],
        bottom: [120, 46, 70],
        glows: &[g(0.5, 1.05, 0.75, [255, 150, 84], 0.8), g(0.15, 0.65, 0.6, [232, 84, 112], 0.5), g(0.85, 0.15, 0.6, [92, 52, 146], 0.55)],
        stars: 0,
    },
    // 晨雾
    Theme {
        top: [78, 94, 116],
        bottom: [58, 70, 92],
        glows: &[g(0.2, 0.15, 0.75, [146, 166, 190], 0.6), g(0.85, 0.85, 0.75, [96, 112, 146], 0.6), g(0.6, 0.5, 0.5, [124, 140, 160], 0.3)],
        stars: 0,
    },
    // 石墨
    Theme {
        top: [22, 22, 25],
        bottom: [12, 12, 14],
        glows: &[g(0.5, 0.3, 0.75, [62, 64, 72], 0.6), g(0.9, 0.95, 0.6, [36, 38, 46], 0.4)],
        stars: 0,
    },
    // 樱花
    Theme {
        top: [64, 30, 62],
        bottom: [44, 22, 54],
        glows: &[g(0.18, 0.25, 0.65, [232, 136, 170], 0.55), g(0.82, 0.75, 0.65, [150, 108, 204], 0.55), g(0.55, 0.05, 0.4, [250, 190, 200], 0.25)],
        stars: 0,
    },
    // 海湾
    Theme {
        top: [4, 40, 70],
        bottom: [2, 26, 50],
        glows: &[g(0.1, 0.9, 0.65, [0, 160, 168], 0.6), g(0.9, 0.1, 0.65, [40, 92, 204], 0.6), g(0.5, 0.55, 0.4, [20, 120, 160], 0.25)],
        stars: 0,
    },
];

/// 简单的伪随机数，同一个种子每次画出来一样（星星不会每次换位置）
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 16) as u32
    }
    fn unit(&mut self) -> f32 {
        (self.next() & 0xFFFF) as f32 / 65535.0
    }
}

/// 按 w×h 画第 index 个内置主题
fn render_theme(index: u8, w: u32, h: u32) -> RgbaImage {
    let theme = &THEMES[(index as usize).min(THEMES.len() - 1)];
    let (w, h) = (w.max(2), h.max(2));
    let aspect = w as f32 / h as f32;
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ index as u64);
    let mut img = RgbaImage::from_fn(w, h, |x, y| {
        let (u, v) = (x as f32 / (w - 1) as f32, y as f32 / (h - 1) as f32);
        let mut c: [f32; 3] = std::array::from_fn(|i| theme.top[i] as f32 + (theme.bottom[i] as f32 - theme.top[i] as f32) * v);
        for glow in theme.glows {
            let (dx, dy) = ((u - glow.x) * aspect, v - glow.y);
            let d2 = (dx * dx + dy * dy) / (glow.r * glow.r);
            let k = glow.a * (-2.2 * d2).exp();
            for (ch, target) in c.iter_mut().zip(glow.color) {
                *ch += (target as f32 - *ch) * k;
            }
        }
        // 暗角：四周压暗一点，目光落在中间的字上
        let (cx, cy) = (u * 2.0 - 1.0, v * 2.0 - 1.0);
        let vignette = 1.0 - 0.28 * (cx * cx * 0.6 + cy * cy).min(1.6);
        let px = [0, 1, 2].map(|i| (c[i] * vignette).clamp(0.0, 255.0) as u8);
        image::Rgba([px[0], px[1], px[2], 255])
    });
    // 一点颗粒：放大以后不出一圈圈的色带
    for p in img.pixels_mut() {
        let n = (rng.next() % 7) as i16 - 3;
        for c in &mut p.0[..3] {
            *c = (*c as i16 + n).clamp(0, 255) as u8;
        }
    }
    // 星星：大多是一个点，少数亮的带一圈光
    let count = (theme.stars as u64 * w as u64 * h as u64 / 1_000_000) as u32;
    for _ in 0..count {
        let (x, y) = ((rng.unit() * (w - 1) as f32) as i32, (rng.unit() * (h - 1) as f32 * 0.92) as i32);
        let bright = 110.0 + rng.unit().powi(3) * 145.0;
        let big = rng.unit() > 0.96;
        let tint = if rng.unit() > 0.7 { [0.85, 0.9, 1.0] } else { [1.0, 1.0, 1.0] };
        let mut add = |px: i32, py: i32, k: f32| {
            if px >= 0 && py >= 0 && (px as u32) < w && (py as u32) < h {
                let p = img.get_pixel_mut(px as u32, py as u32);
                for (ch, t) in p.0.iter_mut().zip(tint) {
                    *ch = (*ch as f32 + bright * k * t).min(255.0) as u8;
                }
            }
        };
        add(x, y, 1.0);
        if big {
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                add(x + dx, y + dy, 0.45);
            }
        }
    }
    img
}

/// 背景图，宽高是屏幕的像素。纯黑返回空（平台自己涂黑，不用传一大张图）。
/// 有字的时候把图片压暗一点，白字看得清
pub(crate) fn background(style: &PrivacyStyle, width: u32, height: u32) -> Result<Option<RgbaImage>> {
    let (width, height) = (width.max(1), height.max(1));
    let has_text = style.clock || !style.message.trim().is_empty();
    match &style.background {
        PrivacyBackground::Black => Ok(None),
        PrivacyBackground::Preset { index } => {
            // 色光很柔，按一半大小画（长边不超过 1920）再交给系统放大，省时间
            let scale = (0.5f32).min(1920.0 / width.max(height) as f32);
            Ok(Some(render_theme(*index, ((width as f32 * scale) as u32).max(2), ((height as f32 * scale) as u32).max(2))))
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

/// 设置页里的小样：按 w×h 画一张 PNG。纯黑也画（一张黑图）
pub fn thumbnail(background: &PrivacyBackground, w: u32, h: u32) -> Result<Vec<u8>> {
    let img = match background {
        PrivacyBackground::Black => RgbaImage::from_pixel(w.max(1), h.max(1), image::Rgba([0, 0, 0, 255])),
        PrivacyBackground::Preset { index } => render_theme(*index, w, h),
        PrivacyBackground::Image { path } => load_cover(path, w, h)?,
    };
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;
    Ok(png)
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
        // 主题按一半大小画
        let g = background(&style(PrivacyBackground::Preset { index: 1 }), 1920, 1080).unwrap().unwrap();
        assert_eq!(g.dimensions(), (960, 540));
        // 4K 屏长边不超过 1920
        let g = background(&style(PrivacyBackground::Preset { index: 4 }), 3840, 2160).unwrap().unwrap();
        assert_eq!(g.dimensions(), (1920, 1080));
        // 每个主题都画得出来，而且彼此不一样、不是一片黑
        let mut means = Vec::new();
        for index in 0..PRIVACY_PRESETS {
            let img = render_theme(index, 160, 90);
            let sum: u64 = img.pixels().map(|p| p.0[0] as u64 + p.0[1] as u64 + p.0[2] as u64).sum();
            let mean = sum / (160 * 90 * 3);
            assert!(mean > 8, "主题 {index} 太黑了：{mean}");
            means.push((img.get_pixel(20, 20).0, img.get_pixel(140, 70).0));
        }
        for i in 0..means.len() {
            for j in i + 1..means.len() {
                assert_ne!(means[i], means[j], "主题 {i} 和 {j} 一样");
            }
        }
        // 同一个主题每次画得一样（星星不乱跳）
        assert_eq!(render_theme(4, 200, 100), render_theme(4, 200, 100));
        // 越界的编号用最后一种
        assert_eq!(render_theme(99, 50, 50), render_theme(PRIVACY_PRESETS - 1, 50, 50));
        // 小样是 PNG
        let png = thumbnail(&PrivacyBackground::Preset { index: 3 }, 192, 108).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        // 图片铺满：4×2 的图盖到 300×300 上
        let dir = std::env::temp_dir().join(format!("p2pcli-privacy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bg.png");
        RgbaImage::from_fn(4, 2, |x, _| if x < 2 { image::Rgba([255, 0, 0, 255]) } else { image::Rgba([0, 0, 255, 255]) }).save(&path).unwrap();
        // 不带扩展名也认得
        let bare = dir.join("bg");
        std::fs::copy(&path, &bare).unwrap();
        let img = background(&style(PrivacyBackground::Image { path: bare.clone() }), 300, 300).unwrap().unwrap();
        assert_eq!(img.dimensions(), (300, 300));
        assert!(img.get_pixel(10, 150).0[0] > 200, "左边是红的");
        assert!(img.get_pixel(290, 150).0[2] > 200, "右边是蓝的");
        // 有字时压暗
        let dim = background(&PrivacyStyle { clock: true, ..style(PrivacyBackground::Image { path: bare.clone() }) }, 300, 300).unwrap().unwrap();
        assert!(dim.get_pixel(10, 150).0[0] < 200);
        // 读不了的图片报错
        assert!(background(&style(PrivacyBackground::Image { path: dir.join("没有.png") }), 10, 10).is_err());
        let _ = std::fs::remove_dir_all(dir);
        assert_eq!(clock_text().len(), 5);
        assert!(date_text().contains("星期"));
    }
}
