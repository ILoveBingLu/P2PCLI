//! 把隐私屏的内置主题拼成一张总览图，眼看一下好不好看：
//! cargo run -p p2pcli-screen --example privacy_themes -- 输出.png
use p2pcli_core::screen::{PRIVACY_PRESETS, PrivacyBackground};

fn main() -> anyhow::Result<()> {
    let out = std::env::args().nth(1).unwrap_or_else(|| "privacy-themes.png".into());
    let (w, h, cols) = (480u32, 270u32, 5u32);
    let rows = (PRIVACY_PRESETS as u32).div_ceil(cols);
    let mut sheet = image::RgbaImage::new(w * cols, h * rows);
    for index in 0..PRIVACY_PRESETS {
        let url = p2pcli_screen::privacy_thumbnail(&PrivacyBackground::Preset { index }, w, h)?;
        use base64::Engine;
        let png = base64::engine::general_purpose::STANDARD.decode(url.split_once(',').unwrap().1)?;
        let img = image::load_from_memory(&png)?.to_rgba8();
        image::imageops::overlay(&mut sheet, &img, ((index as u32 % cols) * w) as i64, ((index as u32 / cols) * h) as i64);
    }
    sheet.save(&out)?;
    println!("写好了 {out}");
    Ok(())
}
