//! 单独读鼠标指针：位置用 CoreGraphics，样子用 NSCursor.currentSystemCursor（整个系统当前的指针，
//! 不只是本程序的）。观看方拿样子当本机的系统指针，手一动指针就动，不用等画面传过来。

use std::collections::hash_map::DefaultHasher;
use std::ffi::c_void;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use objc2::rc::autoreleasepool;
use objc2_app_kit::NSCursor;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGDirectDisplayID, CGDisplayBounds, CGEvent, CGImage, CGImageAlphaInfo,
    kCGColorSpaceSRGB,
};
use objc2_foundation::NSRect;
use p2pcli_core::screen::{CursorInfo, CursorShape};

/// 指针样子最多隔这么久读一次：要碰 AppKit、画一张小图，比读位置贵
const SHAPE_EVERY: Duration = Duration::from_millis(50);

#[derive(Default)]
pub struct CursorReader {
    shape: Mutex<Option<(Instant, Option<Arc<CursorShape>>)>>,
}

impl CursorReader {
    pub fn read(&self, display: &str) -> Option<CursorInfo> {
        let id: CGDirectDisplayID = display.parse().ok()?;
        let at = CGEvent::location(CGEvent::new(None).as_deref());
        let b = CGDisplayBounds(id);
        if b.size.width <= 0.0 || b.size.height <= 0.0 {
            return None;
        }
        let x = (at.x - b.origin.x) / b.size.width;
        let y = (at.y - b.origin.y) / b.size.height;
        let visible = (0.0..1.0).contains(&x) && (0.0..1.0).contains(&y);
        let shape = {
            let mut cache = self.shape.lock().unwrap();
            if cache.as_ref().is_none_or(|(t, _)| t.elapsed() >= SHAPE_EVERY) {
                let fresh = current_shape();
                // 新读到的和上次一样就留着原来那份（编号不变，不重发）
                let old = cache.as_ref().and_then(|(_, s)| s.clone());
                let keep = match (old, fresh) {
                    (Some(old), Some(new)) if old.id == new.id => Some(old),
                    (_, fresh) => fresh.map(Arc::new),
                };
                *cache = Some((Instant::now(), keep));
            }
            cache.as_ref().and_then(|(_, s)| s.clone())
        };
        Some(CursorInfo { x: x.clamp(0.0, 1.0), y: y.clamp(0.0, 1.0), visible, shape })
    }
}

/// 现在系统的指针画成 RGBA。读不到（系统不给）就是空，那时观看方用普通箭头
#[allow(deprecated)]
fn current_shape() -> Option<CursorShape> {
    autoreleasepool(|_| {
        let cursor = NSCursor::currentSystemCursor()?;
        let image = cursor.image();
        let size = image.size();
        let hot = cursor.hotSpot();
        if size.width <= 0.0 || size.height <= 0.0 {
            return None;
        }
        let mut rect = NSRect::new(CGPoint::new(0.0, 0.0), size);
        let cg = unsafe { image.CGImageForProposedRect_context_hints(&mut rect, None, None) }?;
        let (w, h) = (CGImage::width(Some(&cg)), CGImage::height(Some(&cg)));
        if w == 0 || h == 0 || w > 256 || h > 256 {
            return None;
        }
        let scale = w as f64 / size.width;
        let mut pixels = vec![0u8; w * h * 4];
        let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }));
        let ctx = unsafe {
            CGBitmapContextCreate(pixels.as_mut_ptr() as *mut c_void, w, h, 8, w * 4, space.as_deref(), CGImageAlphaInfo::PremultipliedLast.0)
        }?;
        CGContext::draw_image(Some(&ctx), CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(w as f64, h as f64)), Some(&cg));
        drop(ctx);
        // 画出来是预乘过透明度的，浏览器的 ImageData 要不预乘的
        for px in pixels.as_chunks_mut::<4>().0 {
            let a = px[3] as u32;
            if a > 0 && a < 255 {
                for c in &mut px[..3] {
                    *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
        let (hot_x, hot_y) = (hot.x * scale, hot.y * scale);
        let mut hasher = DefaultHasher::new();
        pixels.hash(&mut hasher);
        (w, h, hot_x.to_bits(), hot_y.to_bits()).hash(&mut hasher);
        use base64::Engine as _;
        Some(CursorShape {
            id: hasher.finish(),
            width: w as u32,
            height: h as u32,
            hot_x,
            hot_y,
            scale,
            rgba: base64::engine::general_purpose::STANDARD.encode(&pixels),
        })
    })
}
