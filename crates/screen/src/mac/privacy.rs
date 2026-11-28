//! macOS 隐私屏：每块屏幕盖一个最高层的窗口（黑底、渐变或图片，加上时钟和一行字），
//! 抓屏时把这些窗口排除掉，远程画面里看到的是底下的桌面；再用事件拦截（event tap）拦住本机的键盘鼠标，
//! 只放行远程注入的（事件源带着 [`INPUT_TAG`]）。
//!
//! 窗口只能在主线程上建：P2PCLI 的界面程序主线程一直在跑；命令行、测试里要自己在主线程上跑 run loop，
//! 不然等 3 秒报错。丢掉时不等主线程，排好撤窗口的活就返回。

use std::ffi::c_void;
use std::ptr::{NonNull, null_mut};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::{AllocAnyThread, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBackingStoreType, NSBitmapImageRep, NSDeviceRGBColorSpace, NSColor, NSFont, NSImage, NSImageScaling, NSImageView, NSScreen, NSTextAlignment,
    NSShadow, NSTextField, NSWindow, NSWindowAnimationBehavior, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_core_foundation::{CFMachPort, CFRetained, CFRunLoop, CGFloat, CGPoint, CGRect, CGSize, kCFRunLoopCommonModes, kCFRunLoopDefaultMode};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventMask, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventTapProxy, CGEventType,
    CGShieldingWindowLevel, CGWarpMouseCursorPosition,
};
use objc2_foundation::NSString;
use p2pcli_core::screen::PrivacyStyle;
use tracing::{debug, info, warn};

use crate::privacy::{INPUT_TAG, background, clock_text, date_text};

/// 主线程多久没接活就算不在跑
const MAIN_TIMEOUT: Duration = Duration::from_secs(3);

/// 只在主线程上碰的东西，跨线程传一下
struct MainOnly<T>(T);
unsafe impl<T> Send for MainOnly<T> {}

/// 一块屏幕上的遮挡窗口和时钟的两行字
struct Cover {
    window: Retained<NSWindow>,
    clock: Option<(Retained<NSTextField>, Retained<NSTextField>)>,
}

/// 盖着的隐私屏。丢掉就撤掉
pub struct MacPrivacy {
    covers: Arc<Mutex<Option<MainOnly<Vec<Cover>>>>>,
    window_ids: Vec<u32>,
    stop: Arc<AtomicBool>,
    block: Option<InputBlock>,
    blackout: Option<super::blackout::Blackout>,
}

/// 在主线程上跑 f，等它跑完拿结果；主线程不在跑时等 [`MAIN_TIMEOUT`] 报错（那时 f 也不会再跑）
fn on_main<T: Send + 'static>(f: impl FnOnce(MainThreadMarker) -> T + Send + 'static) -> Result<T> {
    if let Some(mtm) = MainThreadMarker::new() {
        return Ok(f(mtm));
    }
    let (tx, rx) = mpsc::channel();
    let gave_up = Arc::new(AtomicBool::new(false));
    let flag = gave_up.clone();
    DispatchQueue::main().exec_async(move || {
        if flag.load(Ordering::SeqCst) {
            return;
        }
        let _ = tx.send(f(MainThreadMarker::new().expect("主线程队列")));
    });
    rx.recv_timeout(MAIN_TIMEOUT).map_err(|_| {
        gave_up.store(true, Ordering::SeqCst);
        anyhow!("这个程序的主线程没在跑界面，开不了隐私屏")
    })
}

/// 白字标签，居中
fn label(text: &str, size: CGFloat, weight: CGFloat, mono: bool, wrap_width: Option<CGFloat>, mtm: MainThreadMarker) -> Retained<NSTextField> {
    let s = NSString::from_str(text);
    let l = match wrap_width {
        Some(_) => NSTextField::wrappingLabelWithString(&s, mtm),
        None => NSTextField::labelWithString(&s, mtm),
    };
    l.setTextColor(Some(&NSColor::whiteColor()));
    let font = if mono { NSFont::monospacedDigitSystemFontOfSize_weight(size, weight) } else { NSFont::systemFontOfSize_weight(size, weight) };
    l.setFont(Some(&font));
    l.setAlignment(NSTextAlignment::Center);
    // 柔和的阴影：浅色背景、图片上白字也看得清
    let shadow = NSShadow::new();
    shadow.setShadowColor(Some(&NSColor::colorWithWhite_alpha(0.0, 0.55)));
    shadow.setShadowBlurRadius((size * 0.18).max(3.0));
    shadow.setShadowOffset(CGSize::new(0.0, -(size * 0.03).max(1.0)));
    l.setShadow(Some(&shadow));
    if let Some(w) = wrap_width {
        l.setPreferredMaxLayoutWidth(w);
    }
    l
}

/// 背景图的像素直接填进 NSBitmapImageRep 交给 NSImage（不经过编码、解码）
fn ns_image(img: &image::RgbaImage) -> Option<Retained<NSImage>> {
    let (w, h) = img.dimensions();
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            null_mut(),
            w as isize,
            h as isize,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            (w * 4) as isize,
            32,
        )
    }?;
    let dst = rep.bitmapData();
    if dst.is_null() {
        return None;
    }
    let src = img.as_raw();
    unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), dst, src.len()) };
    let image = NSImage::initWithSize(NSImage::alloc(), CGSize::new(w as CGFloat, h as CGFloat));
    image.addRepresentation(&rep);
    Some(image)
}

/// 给一块屏幕建遮挡窗口（先透明，排除进抓屏以后再显示出来）
fn build_cover(screen: &NSScreen, style: &PrivacyStyle, mtm: MainThreadMarker) -> Cover {
    let frame = screen.frame();
    let scale = screen.backingScaleFactor();
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            frame,
            NSWindowStyleMask::Borderless,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { window.setReleasedWhenClosed(false) };
    // 比菜单栏、程序坞、全屏程序都高
    window.setLevel(CGShieldingWindowLevel() as isize);
    window.setOpaque(true);
    window.setBackgroundColor(Some(&NSColor::blackColor()));
    window.setHasShadow(false);
    // 点不到它：远程注入的点击落在底下的程序上
    window.setIgnoresMouseEvents(true);
    window.setAnimationBehavior(NSWindowAnimationBehavior::None);
    window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    window.setFrame_display(frame, false);
    window.setAlphaValue(0.0);

    let Some(content) = window.contentView() else { return Cover { window, clock: None } };
    let bounds = content.bounds();
    let (w, h) = (bounds.size.width, bounds.size.height);
    let px = ((w * scale) as u32, (h * scale) as u32);
    match background(style, px.0, px.1) {
        Ok(Some(img)) => {
            if let Some(image) = ns_image(&img).or_else(|| {
                warn!("隐私屏背景交不给系统，用黑底");
                None
            }) {
                let view = NSImageView::imageViewWithImage(&image, mtm);
                view.setImageScaling(NSImageScaling::ScaleAxesIndependently);
                view.setFrame(bounds);
                view.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable);
                content.addSubview(&view);
            }
        }
        Ok(None) => {}
        Err(e) => warn!("隐私屏背景画不出来，用黑底：{e:#}"),
    }

    // 字：有时钟时钟在中间偏上，日期在下面，那行字再往下；没时钟时那行字在正中间
    let place = |l: &NSTextField, center_y: CGFloat| {
        let size = l.fittingSize();
        let size = CGSize::new(size.width.min(w * 0.9), size.height);
        l.setFrame(CGRect::new(CGPoint::new((w - size.width) / 2.0, center_y - size.height / 2.0), size));
        content.addSubview(l);
    };
    let message = style.message.trim();
    let msg_size = (h * 0.026).clamp(15.0, 40.0);
    let mut clock = None;
    if style.clock {
        let big = label(&clock_text(), (h * 0.16).max(48.0), -0.6, true, None, mtm);
        let date = label(&date_text(), (h * 0.028).max(15.0), 0.0, false, None, mtm);
        place(&big, h * 0.56);
        let below = h * 0.56 - big.frame().size.height / 2.0;
        place(&date, below - date.frame().size.height.max(msg_size) * 0.6);
        if !message.is_empty() {
            let m = label(message, msg_size, 0.0, false, Some(w * 0.8), mtm);
            place(&m, below - h * 0.12);
        }
        clock = Some((big, date));
    } else if !message.is_empty() {
        let m = label(message, msg_size, 0.0, false, Some(w * 0.8), mtm);
        place(&m, h * 0.5);
    }
    window.orderFrontRegardless();
    Cover { window, clock }
}

impl MacPrivacy {
    pub fn start(style: &PrivacyStyle, block_input: bool) -> Result<Self> {
        // 先拦键鼠：拦不住（没给辅助功能权限）就不盖，免得本机的人以为被锁住了
        let block = if block_input { Some(InputBlock::start()?) } else { None };
        let blackout = style.blackout;
        let style = style.clone();
        let covers = on_main(move |mtm| {
            let screens = NSScreen::screens(mtm);
            MainOnly(screens.iter().map(|s| build_cover(&s, &style, mtm)).collect::<Vec<_>>())
        })?;
        let window_ids: Vec<u32> = covers.0.iter().map(|c| c.window.windowNumber() as u32).collect();
        if window_ids.is_empty() {
            bail!("没有屏幕可以盖");
        }
        // 窗口已经在屏幕上（还是透明的）：抓屏排除掉它们以后再显示出来，远程画面一下都不会黑
        super::capture::set_excluded(&window_ids, &[]);
        let covers = Arc::new(Mutex::new(Some(covers)));
        let shown = covers.clone();
        on_main(move |_| {
            if let Some(c) = shown.lock().unwrap().as_ref() {
                for cover in &c.0 {
                    cover.window.setAlphaValue(1.0);
                }
            }
        })?;
        let stop = Arc::new(AtomicBool::new(false));
        spawn_clock(covers.clone(), stop.clone());
        // 增强：窗口盖好以后屏幕再整个变黑（伽马表被系统改回去的一瞬间，看到的还是隐私屏）
        let blackout = match blackout {
            true => match super::blackout::Blackout::start() {
                Ok(b) => Some(b),
                Err(e) => {
                    warn!("增强隐私屏开不了，只盖窗口：{e:#}");
                    None
                }
            },
            false => None,
        };
        info!(screens = window_ids.len(), block_input, blackout = blackout.is_some(), "隐私屏盖上了");
        Ok(MacPrivacy { covers, window_ids, stop, block, blackout })
    }
}

/// 每秒更新一次时钟（没有时钟的样式也跑着，什么都不改）
fn spawn_clock(covers: Arc<Mutex<Option<MainOnly<Vec<Cover>>>>>, stop: Arc<AtomicBool>) {
    let _ = std::thread::Builder::new().name("p2pcli-privacy-clock".into()).spawn(move || {
        while !stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(500));
            let covers = covers.clone();
            let (clock, date) = (NSString::from_str(&clock_text()), NSString::from_str(&date_text()));
            let texts = MainOnly((clock, date));
            DispatchQueue::main().exec_async(move || {
                let texts = texts;
                if let Some(c) = covers.lock().unwrap().as_ref() {
                    for (big, small) in c.0.iter().filter_map(|c| c.clock.as_ref()) {
                        big.setStringValue(&texts.0.0);
                        small.setStringValue(&texts.0.1);
                    }
                }
            });
        }
    });
}

impl Drop for MacPrivacy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // 先放开键鼠、屏幕亮回来，再撤窗口
        self.block.take();
        self.blackout.take();
        let covers = self.covers.clone();
        DispatchQueue::main().exec_async(move || {
            if let Some(c) = covers.lock().unwrap().take() {
                for cover in c.0 {
                    cover.window.orderOut(None);
                    cover.window.close();
                }
            }
        });
        // 窗口没了以后，抓屏的过滤里也去掉它们（要等系统，放到别的线程）
        let ids = std::mem::take(&mut self.window_ids);
        let _ = std::thread::Builder::new().name("p2pcli-privacy-off".into()).spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            super::capture::set_excluded(&[], &ids);
        });
        info!("隐私屏撤掉了");
    }
}

// ---------------------------------------------------------------- 拦本机键鼠

/// 拦住本机键鼠的事件拦截，在自己的线程上跑。丢掉就停
struct InputBlock {
    run_loop: MainOnly<CFRetained<CFRunLoop>>,
    stop: Arc<AtomicBool>,
}

/// 拦这些：键盘（按下、松开、修饰键）、鼠标按键、移动、拖动、滚轮
fn event_mask() -> CGEventMask {
    [
        CGEventType::LeftMouseDown,
        CGEventType::LeftMouseUp,
        CGEventType::RightMouseDown,
        CGEventType::RightMouseUp,
        CGEventType::MouseMoved,
        CGEventType::LeftMouseDragged,
        CGEventType::RightMouseDragged,
        CGEventType::KeyDown,
        CGEventType::KeyUp,
        CGEventType::FlagsChanged,
        CGEventType::ScrollWheel,
        CGEventType::OtherMouseDown,
        CGEventType::OtherMouseUp,
        CGEventType::OtherMouseDragged,
    ]
    .iter()
    .fold(0, |m, t| m | (1u64 << t.0))
}

/// 指针该在的地方：远程最后一次把它挪到哪（盖上时是当时的位置）。
/// 系统在事件到拦截之前就已经挪了指针，光丢掉本机的移动事件拦不住指针，要再把它挪回来
static PIN: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];

fn set_pin(p: CGPoint) {
    PIN[0].store(p.x.to_bits(), Ordering::SeqCst);
    PIN[1].store(p.y.to_bits(), Ordering::SeqCst);
}

fn is_move(kind: CGEventType) -> bool {
    [CGEventType::MouseMoved, CGEventType::LeftMouseDragged, CGEventType::RightMouseDragged, CGEventType::OtherMouseDragged].contains(&kind)
}

/// 系统嫌回调慢、或者别的原因把拦截关了：马上重新打开
unsafe extern "C-unwind" fn tap_callback(_proxy: CGEventTapProxy, kind: CGEventType, event: NonNull<CGEvent>, user: *mut c_void) -> *mut CGEvent {
    if kind == CGEventType::TapDisabledByTimeout || kind == CGEventType::TapDisabledByUserInput {
        let port = unsafe { &*(user as *const AtomicPtr<CFMachPort>) }.load(Ordering::SeqCst);
        if let Some(port) = unsafe { port.as_ref() } {
            CGEvent::tap_enable(port, true);
        }
        return event.as_ptr();
    }
    let ev = unsafe { event.as_ref() };
    if CGEvent::integer_value_field(Some(ev), CGEventField::EventSourceUserData) == INPUT_TAG as i64 {
        if is_move(kind) {
            set_pin(CGEvent::location(Some(ev)));
        }
        return event.as_ptr();
    }
    if is_move(kind) {
        let p = CGPoint::new(f64::from_bits(PIN[0].load(Ordering::SeqCst)), f64::from_bits(PIN[1].load(Ordering::SeqCst)));
        let _ = CGWarpMouseCursorPosition(p);
    }
    null_mut()
}

impl InputBlock {
    fn start() -> Result<Self> {
        let (tx, rx) = mpsc::channel::<Result<MainOnly<CFRetained<CFRunLoop>>>>();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        std::thread::Builder::new().name("p2pcli-privacy-input".into()).spawn(move || {
            // 回调里要用到拦截自己（重新打开），先占个位置；线程结束才释放
            let slot: &'static AtomicPtr<CFMachPort> = Box::leak(Box::new(AtomicPtr::new(null_mut())));
            let user = slot as *const AtomicPtr<CFMachPort> as *mut c_void;
            // 先试最底层（键鼠刚进系统的地方，连指针移动都拦得住），不行再拦会话这一层
            let mut tap = None;
            for location in [CGEventTapLocation::HIDEventTap, CGEventTapLocation::SessionEventTap] {
                tap = unsafe {
                    CGEvent::tap_create(location, CGEventTapPlacement::HeadInsertEventTap, CGEventTapOptions::Default, event_mask(), Some(tap_callback), user)
                };
                if tap.is_some() {
                    debug!(location = location.0, "隐私屏拦键鼠");
                    break;
                }
            }
            set_pin(CGEvent::location(CGEvent::new(None).as_deref()));
            let Some(tap) = tap else {
                let _ = tx.send(Err(anyhow!("拦不住本机的键盘鼠标：P2PCLI 还没有辅助功能权限")));
                return;
            };
            slot.store(CFRetained::as_ptr(&tap).as_ptr(), Ordering::SeqCst);
            let Some(source) = CFMachPort::new_run_loop_source(None, Some(&tap), 0) else {
                let _ = tx.send(Err(anyhow!("拦不住本机的键盘鼠标")));
                return;
            };
            let Some(run_loop) = CFRunLoop::current() else {
                let _ = tx.send(Err(anyhow!("拦不住本机的键盘鼠标")));
                return;
            };
            run_loop.add_source(Some(&source), unsafe { kCFRunLoopCommonModes });
            CGEvent::tap_enable(&tap, true);
            let _ = tx.send(Ok(MainOnly(run_loop)));
            // 一段段地跑：丢掉的时候可能赶在 run loop 跑起来之前，光叫它停会漏掉
            while !stopped.load(Ordering::SeqCst) {
                CFRunLoop::run_in_mode(unsafe { kCFRunLoopDefaultMode }, 0.5, false);
            }
            CGEvent::tap_enable(&tap, false);
            tap.invalidate();
            slot.store(null_mut(), Ordering::SeqCst);
            debug!("隐私屏不再拦键鼠");
        })?;
        let run_loop = rx.recv_timeout(MAIN_TIMEOUT).map_err(|_| anyhow!("拦本机键鼠的线程没响应"))??;
        Ok(InputBlock { run_loop, stop })
    }
}

impl Drop for InputBlock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.run_loop.0.stop();
    }
}
