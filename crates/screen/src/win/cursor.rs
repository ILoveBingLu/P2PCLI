//! 单独读鼠标指针：GetCursorInfo 读位置和当前指针，GetIconInfo 取图案。
//! 观看方拿图案当本机的系统指针，手一动指针就动，不用等画面传过来。

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use p2pcli_core::screen::{CursorInfo, CursorShape};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, GetObjectW, HBITMAP, HDC,
    HGDIOBJ, ReleaseDC,
};
use windows::Win32::UI::WindowsAndMessaging::{CURSOR_SHOWING, CURSORINFO, GetCursorInfo, GetIconInfo, HICON, ICONINFO};

#[derive(Default)]
pub struct CursorReader {
    /// 上次的指针句柄和它的图案：句柄没变就不重画
    shape: Mutex<Option<(isize, Option<Arc<CursorShape>>)>>,
}

impl CursorReader {
    pub fn read(&self, display: &str) -> Option<CursorInfo> {
        let mut ci = CURSORINFO { cbSize: std::mem::size_of::<CURSORINFO>() as u32, ..Default::default() };
        unsafe { GetCursorInfo(&mut ci) }.ok()?;
        let m = super::find_monitor(display).ok()?;
        let (w, h) = ((m.rect.right - m.rect.left).max(1) as f64, (m.rect.bottom - m.rect.top).max(1) as f64);
        let x = (ci.ptScreenPos.x - m.rect.left) as f64 / w;
        let y = (ci.ptScreenPos.y - m.rect.top) as f64 / h;
        let showing = ci.flags.0 & CURSOR_SHOWING.0 != 0 && !ci.hCursor.is_invalid();
        let visible = showing && (0.0..1.0).contains(&x) && (0.0..1.0).contains(&y);
        let shape = if showing {
            let handle = ci.hCursor.0 as isize;
            let mut cache = self.shape.lock().unwrap();
            if cache.as_ref().is_none_or(|(h, _)| *h != handle) {
                *cache = Some((handle, shape_of(HICON(ci.hCursor.0)).map(Arc::new)));
            }
            cache.as_ref().and_then(|(_, s)| s.clone())
        } else {
            None
        };
        Some(CursorInfo { x: x.clamp(0.0, 1.0), y: y.clamp(0.0, 1.0), visible, shape })
    }
}

/// 位图读成从上到下的 32 位 BGRA
fn read_bitmap(dc: HDC, bitmap: HBITMAP) -> Option<(usize, usize, Vec<u8>)> {
    let mut info = BITMAP::default();
    let got = unsafe { GetObjectW(HGDIOBJ(bitmap.0), std::mem::size_of::<BITMAP>() as i32, Some(&mut info as *mut BITMAP as *mut _)) };
    if got == 0 || info.bmWidth <= 0 || info.bmHeight <= 0 {
        return None;
    }
    let (w, h) = (info.bmWidth as usize, info.bmHeight as usize);
    let mut bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            // 负数表示从上到下
            biHeight: -(h as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut pixels = vec![0u8; w * h * 4];
    let lines = unsafe { GetDIBits(dc, bitmap, 0, h as u32, Some(pixels.as_mut_ptr() as *mut _), &mut bmi, DIB_RGB_COLORS) };
    (lines > 0).then_some((w, h, pixels))
}

/// 指针图案画成 RGBA（不预乘透明度）。
/// 彩色指针带透明度就直接用；老式的黑白指针按掩码算：反色的地方（比如文本光标）画成白色再描一圈黑边，
/// 深色、浅色背景上都看得见
fn shape_of(cursor: HICON) -> Option<CursorShape> {
    let mut ii = ICONINFO::default();
    unsafe { GetIconInfo(cursor, &mut ii) }.ok()?;
    let dc = unsafe { GetDC(None) };
    let mask = read_bitmap(dc, ii.hbmMask);
    let color = if ii.hbmColor.is_invalid() { None } else { read_bitmap(dc, ii.hbmColor) };
    unsafe {
        ReleaseDC(None, dc);
        let _ = DeleteObject(HGDIOBJ(ii.hbmMask.0));
        if !ii.hbmColor.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(ii.hbmColor.0));
        }
    }
    let (mw, mh, mask) = mask?;
    let (w, h, mut rgba) = match color {
        Some((w, h, bgra)) => {
            let has_alpha = bgra.chunks_exact(4).any(|p| p[3] != 0);
            let mut out = vec![0u8; w * h * 4];
            for i in 0..w * h {
                let p = &bgra[i * 4..i * 4 + 4];
                let a = if has_alpha {
                    p[3]
                } else if mask.get(i * 4).copied().unwrap_or(0) == 0 {
                    255
                } else {
                    0
                };
                out[i * 4..i * 4 + 4].copy_from_slice(&[p[2], p[1], p[0], a]);
            }
            (w, h, out)
        }
        None => {
            // 黑白指针：上半是 AND 掩码，下半是 XOR 掩码
            let (w, h) = (mw, mh / 2);
            let mut out = vec![0u8; w * h * 4];
            let mut inverted = vec![false; w * h];
            for i in 0..w * h {
                let and = mask[i * 4] != 0;
                let xor = mask[(w * h + i) * 4] != 0;
                let px: [u8; 4] = match (and, xor) {
                    (false, false) => [0, 0, 0, 255],
                    (false, true) => [255, 255, 255, 255],
                    (true, false) => [0, 0, 0, 0],
                    (true, true) => {
                        inverted[i] = true;
                        [255, 255, 255, 255]
                    }
                };
                out[i * 4..i * 4 + 4].copy_from_slice(&px);
            }
            // 反色的地方描黑边
            for y in 0..h {
                for x in 0..w {
                    if out[(y * w + x) * 4 + 3] != 0 {
                        continue;
                    }
                    let near = [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)].iter().any(|(dx, dy)| {
                        let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                        nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h && inverted[ny as usize * w + nx as usize]
                    });
                    if near {
                        out[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&[0, 0, 0, 255]);
                    }
                }
            }
            (w, h, out)
        }
    };
    if w == 0 || h == 0 || w > 256 || h > 256 {
        return None;
    }
    rgba.truncate(w * h * 4);
    // 系统标准指针在 100% 缩放时是 32 像素；更大的是按缩放放大过的
    let scale = (w as f64 / 32.0).max(1.0);
    let (hot_x, hot_y) = (ii.xHotspot as f64, ii.yHotspot as f64);
    let mut hasher = DefaultHasher::new();
    rgba.hash(&mut hasher);
    (w, h, ii.xHotspot, ii.yHotspot).hash(&mut hasher);
    use base64::Engine as _;
    Some(CursorShape {
        id: hasher.finish(),
        width: w as u32,
        height: h as u32,
        hot_x,
        hot_y,
        scale,
        rgba: base64::engine::general_purpose::STANDARD.encode(&rgba),
    })
}
