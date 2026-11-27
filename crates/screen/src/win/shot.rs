//! Windows 单帧截屏：GDI 把整块屏幕按比例缩到要的宽度（HALFTONE 缩放，字不糊），编成 PNG。
//! 只截一张，用不着 Windows.Graphics.Capture 那一套；指针不画进去。

use anyhow::{Result, anyhow, bail};
use p2pcli_core::screen::Screenshot;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CAPTUREBLT, CreateCompatibleBitmap, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC,
    GetDIBits, HALFTONE, HGDIOBJ, ReleaseDC, SRCCOPY, SelectObject, SetBrushOrgEx, SetStretchBltMode, StretchBlt,
};

/// 按宽度上限等比缩小
pub(crate) fn fit(w: u32, h: u32, max_width: u32) -> (u32, u32) {
    if w <= max_width || w == 0 {
        return (w.max(1), h.max(1));
    }
    let h2 = (h as u64 * max_width as u64 / w as u64).max(1) as u32;
    (max_width, h2)
}

/// 截屏幕上 rect 这块（虚拟桌面坐标），缩到宽度不超过 max_width
pub(crate) fn capture(rect: RECT, max_width: u32) -> Result<Screenshot> {
    let (sw, sh) = ((rect.right - rect.left).max(0) as u32, (rect.bottom - rect.top).max(0) as u32);
    if sw == 0 || sh == 0 {
        bail!("屏幕大小不对");
    }
    let (w, h) = fit(sw, sh, max_width);
    let mut bgra = vec![0u8; (w * h * 4) as usize];
    unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            bail!("拿不到屏幕");
        }
        let mem = CreateCompatibleDC(Some(screen));
        let bitmap = CreateCompatibleBitmap(screen, w as i32, h as i32);
        let old = SelectObject(mem, HGDIOBJ(bitmap.0));
        SetStretchBltMode(mem, HALFTONE);
        let _ = SetBrushOrgEx(mem, 0, 0, None);
        let copied = StretchBlt(mem, 0, 0, w as i32, h as i32, Some(screen), rect.left, rect.top, sw as i32, sh as i32, SRCCOPY | CAPTUREBLT);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                // 负的高度：自上而下
                biHeight: -(h as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        SelectObject(mem, old);
        let lines = GetDIBits(mem, bitmap, 0, h, Some(bgra.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        if !copied.as_bool() || lines != h as i32 {
            bail!("截屏失败");
        }
    }
    // BGRA → RGBA，GDI 的 alpha 不可靠，一律不透明
    for px in bgra.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
        px[3] = 255;
    }
    Ok(Screenshot { png: encode_png(&bgra, w, h)?, width: w, height: h })
}

pub(crate) fn encode_png(rgba: &[u8], w: u32, h: u32) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        // 屏幕截图里大片纯色，快速压缩就够小
        enc.set_compression(png::Compression::Fast);
        let mut writer = enc.write_header().map_err(|e| anyhow!("编 PNG 失败：{e}"))?;
        writer.write_image_data(rgba).map_err(|e| anyhow!("编 PNG 失败：{e}"))?;
    }
    Ok(out)
}

/// 合成画面的“截图”：渐变加格子，给不碰真屏幕的测试用
pub(crate) fn synthetic(width: u32, height: u32, max_width: u32) -> Result<Screenshot> {
    let (w, h) = fit(width, height, max_width);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let grid = (x / 32 + y / 32) % 2 == 0;
            rgba[i] = (x * 255 / w.max(1)) as u8;
            rgba[i + 1] = (y * 255 / h.max(1)) as u8;
            rgba[i + 2] = if grid { 200 } else { 60 };
            rgba[i + 3] = 255;
        }
    }
    Ok(Screenshot { png: encode_png(&rgba, w, h)?, width: w, height: h })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_width() {
        assert_eq!(fit(2560, 1600, 1440), (1440, 900));
        assert_eq!(fit(1280, 800, 1440), (1280, 800));
    }

    #[test]
    fn synthetic_png_decodes() {
        let shot = synthetic(2560, 1600, 1024).unwrap();
        assert_eq!((shot.width, shot.height), (1024, 640));
        let dec = png::Decoder::new(std::io::Cursor::new(shot.png)).read_info().unwrap();
        assert_eq!((dec.info().width, dec.info().height), (1024, 640));
    }
}
