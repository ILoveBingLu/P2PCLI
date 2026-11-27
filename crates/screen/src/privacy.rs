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

/// 一团色光：中心（0~1）、半径（以屏幕高为 1）、横向拉长多少倍、颜色、浓度
struct Glow {
    x: f32,
    y: f32,
    r: f32,
    stretch: f32,
    color: [u8; 3],
    a: f32,
}

/// 一条光带（极光、银河）：沿一条起伏的曲线，下边沿亮、往上慢慢淡，带竖着的光丝
struct Band {
    /// 曲线的高度、起伏、起伏的快慢、相位
    y: f32,
    amp: f32,
    freq: f32,
    phase: f32,
    /// 往上淡出的距离、下边沿的软硬
    height: f32,
    edge: f32,
    /// 竖光丝的明显程度（0 是没有）
    rays: f32,
    color: [u8; 3],
    a: f32,
}

struct Theme {
    /// 底色：上面一个、下面一个，竖着渐变
    top: [u8; 3],
    bottom: [u8; 3],
    glows: &'static [Glow],
    bands: &'static [Band],
    /// 每百万像素撒多少颗星
    stars: u32,
}

const fn g(x: f32, y: f32, r: f32, stretch: f32, color: [u8; 3], a: f32) -> Glow {
    Glow { x, y, r, stretch, color, a }
}

#[allow(clippy::too_many_arguments)]
const fn band(y: f32, amp: f32, freq: f32, phase: f32, height: f32, edge: f32, rays: f32, color: [u8; 3], a: f32) -> Band {
    Band { y, amp, freq, phase, height, edge, rays, color, a }
}

/// 内置主题，顺序和设置页一致（老版本存的 0~2 还是深海、暮色、森林）
const THEMES: [Theme; PRIVACY_PRESETS as usize] = [
    // 深海：深蓝底，左上一片青光，右下一片靛蓝
    Theme {
        top: [5, 20, 34],
        bottom: [3, 10, 22],
        glows: &[g(0.1, 0.05, 0.8, 1.6, [24, 104, 136], 0.7), g(0.95, 1.0, 0.85, 1.5, [30, 58, 124], 0.6), g(0.55, 0.6, 0.6, 2.2, [16, 96, 120], 0.22)],
        bands: &[],
        stars: 0,
    },
    // 暮色：紫底，右下晚霞的玫红和橘
    Theme {
        top: [30, 14, 60],
        bottom: [40, 16, 58],
        glows: &[g(0.05, 0.05, 0.8, 1.5, [76, 34, 132], 0.65), g(0.95, 1.05, 0.85, 1.6, [204, 78, 104], 0.7), g(0.8, 0.75, 0.5, 1.8, [236, 132, 96], 0.3)],
        bands: &[],
        stars: 0,
    },
    // 森林：墨绿底，左下一片青绿，右上一点嫩绿
    Theme {
        top: [8, 34, 32],
        bottom: [4, 22, 20],
        glows: &[g(0.1, 0.95, 0.85, 1.6, [40, 114, 82], 0.7), g(0.9, 0.05, 0.75, 1.5, [92, 150, 100], 0.45), g(0.5, 0.5, 0.6, 2.4, [20, 72, 62], 0.35)],
        bands: &[],
        stars: 0,
    },
    // 极光：夜空里三条起伏的光带，绿的最亮，带竖光丝
    Theme {
        top: [2, 5, 16],
        bottom: [6, 16, 32],
        glows: &[g(0.5, 1.1, 0.7, 2.0, [10, 40, 54], 0.6)],
        bands: &[
            band(0.58, 0.07, 0.9, 0.3, 0.3, 0.025, 0.35, [56, 232, 160], 0.75),
            band(0.47, 0.06, 1.4, 1.7, 0.22, 0.02, 0.4, [64, 150, 240], 0.42),
            band(0.4, 0.05, 1.9, 3.1, 0.16, 0.02, 0.45, [176, 96, 232], 0.32),
        ],
        stars: 240,
    },
    // 星空：深夜蓝，一道斜跨的淡银河，满天星星
    Theme {
        top: [2, 4, 14],
        bottom: [12, 16, 42],
        glows: &[g(0.5, 1.15, 0.8, 2.0, [64, 50, 130], 0.5)],
        bands: &[band(0.55, 0.25, 0.45, 2.4, 0.22, 0.12, 0.0, [150, 140, 200], 0.16)],
        stars: 950,
    },
    // 日落：上面紫，地平线一片橘红
    Theme {
        top: [36, 20, 68],
        bottom: [112, 40, 64],
        glows: &[g(0.5, 1.1, 0.75, 2.2, [255, 148, 80], 0.8), g(0.12, 0.7, 0.6, 1.6, [230, 82, 110], 0.45), g(0.88, 0.15, 0.6, 1.6, [86, 50, 142], 0.5)],
        bands: &[],
        stars: 0,
    },
    // 晨雾：灰蓝，柔和的亮雾
    Theme {
        top: [74, 90, 112],
        bottom: [52, 64, 86],
        glows: &[g(0.2, 0.1, 0.8, 1.8, [146, 164, 188], 0.55), g(0.85, 0.95, 0.8, 1.6, [92, 106, 140], 0.55)],
        bands: &[],
        stars: 0,
    },
    // 石墨：近黑，上方一点冷光
    Theme {
        top: [22, 22, 25],
        bottom: [10, 10, 12],
        glows: &[g(0.5, 0.15, 0.8, 2.2, [64, 66, 74], 0.55)],
        bands: &[],
        stars: 0,
    },
    // 樱花：粉紫
    Theme {
        top: [62, 28, 60],
        bottom: [40, 20, 52],
        glows: &[g(0.15, 0.2, 0.75, 1.6, [232, 136, 170], 0.5), g(0.85, 0.85, 0.75, 1.6, [148, 106, 204], 0.5), g(0.55, 0.0, 0.5, 2.0, [250, 190, 204], 0.22)],
        bands: &[],
        stars: 0,
    },
    // 海湾：青绿和海蓝
    Theme {
        top: [3, 38, 66],
        bottom: [2, 22, 46],
        glows: &[g(0.05, 0.95, 0.75, 1.6, [0, 156, 164], 0.55), g(0.95, 0.05, 0.75, 1.6, [40, 90, 200], 0.55), g(0.5, 0.6, 0.5, 2.4, [16, 110, 150], 0.22)],
        bands: &[],
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

/// 按 w×h 画第 index 个内置主题。text 为真时在中间压一圈柔和的暗晕，白字更清楚
fn render_theme(index: u8, w: u32, h: u32, text: bool) -> RgbaImage {
    let index = (index as usize).min(THEMES.len() - 1);
    let theme = &THEMES[index];
    let (w, h) = (w.max(2), h.max(2));
    let aspect = w as f32 / h as f32;
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ index as u64);
    let tau = std::f32::consts::TAU;
    let mut img = RgbaImage::from_fn(w, h, |x, y| {
        let (u, v) = (x as f32 / (w - 1) as f32, y as f32 / (h - 1) as f32);
        let mut c: [f32; 3] = std::array::from_fn(|i| theme.top[i] as f32 + (theme.bottom[i] as f32 - theme.top[i] as f32) * v);
        for glow in theme.glows {
            let (dx, dy) = ((u - glow.x) * aspect / glow.stretch, v - glow.y);
            let k = glow.a * (-1.6 * (dx * dx + dy * dy) / (glow.r * glow.r)).exp();
            for (ch, target) in c.iter_mut().zip(glow.color) {
                *ch += (target as f32 - *ch) * k;
            }
        }
        for b in theme.bands {
            let curve = b.y + b.amp * (u * b.freq * tau + b.phase).sin() + b.amp * 0.35 * (u * b.freq * 2.7 * tau + b.phase * 1.9).sin();
            let d = v - curve;
            let mut k = if d > 0.0 { (-(d / b.edge).powi(2)).exp() } else { (d / b.height).exp() };
            if b.rays > 0.0 {
                let ray = 0.5 + 0.5 * (u * 140.0 + (u * 11.0).sin() * 6.0).sin() * (u * 37.0 + 1.3).sin();
                k *= 1.0 - b.rays + b.rays * ray;
            }
            // 两头淡出
            k *= b.a * (u * std::f32::consts::PI).sin().powf(0.6);
            // 滤色叠加：只会更亮，像光
            for (ch, light) in c.iter_mut().zip(b.color) {
                *ch = 255.0 - (255.0 - *ch) * (1.0 - k * light as f32 / 255.0);
            }
        }
        // 暗角：四周压暗一点，目光落在中间
        let (cx, cy) = (u * 2.0 - 1.0, v * 2.0 - 1.0);
        let mut shade = 1.0 - 0.28 * (cx * cx * 0.6 + cy * cy).min(1.6);
        // 字后面一圈暗晕
        if text {
            let (tx, ty) = ((u - 0.5) * aspect / 1.6, (v - 0.5) / 0.5);
            shade *= 1.0 - 0.3 * (-(tx * tx + ty * ty) * 2.0).exp();
        }
        let px = [0, 1, 2].map(|i| (c[i] * shade).clamp(0.0, 255.0) as u8);
        image::Rgba([px[0], px[1], px[2], 255])
    });
    // 一点颗粒：放大以后不出一圈圈的色带
    for p in img.pixels_mut() {
        let n = (rng.next() % 7) as i16 - 3;
        for c in &mut p.0[..3] {
            *c = (*c as i16 + n).clamp(0, 255) as u8;
        }
    }
    // 星星：大多是一个点，少数亮的带一圈光；越往下越少（地平线那边亮）
    let count = (theme.stars as u64 * w as u64 * h as u64 / 1_000_000) as u32;
    for _ in 0..count {
        let (x, y) = ((rng.unit() * (w - 1) as f32) as i32, (rng.unit().powf(1.3) * (h - 1) as f32 * 0.9) as i32);
        let bright = 100.0 + rng.unit().powi(3) * 155.0;
        let big = rng.unit() > 0.965;
        let tint = if rng.unit() > 0.7 { [0.82, 0.9, 1.0] } else { [1.0, 1.0, 1.0] };
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
                add(x + dx, y + dy, 0.4);
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
            Ok(Some(render_theme(*index, ((width as f32 * scale) as u32).max(2), ((height as f32 * scale) as u32).max(2), has_text)))
        }
        PrivacyBackground::Image { path } => {
            let mut img = load_cover(path, width, height)?;
            if has_text {
                // 整体压暗一点，字的位置再多压一些
                let (w, h) = img.dimensions();
                let aspect = w as f32 / h as f32;
                for (x, y, p) in img.enumerate_pixels_mut() {
                    let (tx, ty) = ((x as f32 / w as f32 - 0.5) * aspect / 1.6, (y as f32 / h as f32 - 0.5) / 0.5);
                    let shade = 0.72 * (1.0 - 0.3 * (-(tx * tx + ty * ty) * 2.0).exp());
                    for c in &mut p.0[..3] {
                        *c = (*c as f32 * shade) as u8;
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
        PrivacyBackground::Preset { index } => render_theme(*index, w, h, true),
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
            let img = render_theme(index, 160, 90, false);
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
        assert_eq!(render_theme(4, 200, 100, true), render_theme(4, 200, 100, true));
        // 越界的编号用最后一种
        assert_eq!(render_theme(99, 50, 50, false), render_theme(PRIVACY_PRESETS - 1, 50, 50, false));
        // 有字时中间压暗
        let (plain, dim) = (render_theme(6, 160, 90, false), render_theme(6, 160, 90, true));
        assert!(dim.get_pixel(80, 45).0[0] + 20 < plain.get_pixel(80, 45).0[0]);
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
