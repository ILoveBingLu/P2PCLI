//! 排查虚拟屏：建一块，每半秒记一次主屏是谁、它在哪、读不读得到分辨率；换成 HiDPI；删掉以后多久从列表里消失。
//! runloop：主线程跑事件循环，活在别的线程上做；nsapp：再先注册成应用（和 P2PCLI 应用一样）；
//! fresh：建之前不查屏幕列表。
//! cargo run -p p2pcli-screen --example vd_probe -- [runloop|nsapp|fresh]
#[cfg(target_os = "macos")]
fn probe(fresh: bool) {
    use std::time::{Duration, Instant};

    use objc2_core_graphics::{CGDisplayBounds, CGDisplayCopyDisplayMode, CGDisplayVendorNumber, CGMainDisplayID};
    use p2pcli_core::screen::DisplayMode;

    let backend = p2pcli_screen::backend().unwrap();
    let ids = || backend.displays().unwrap().iter().map(|d| format!("{}{}", d.id, if d.primary { "*" } else { "" })).collect::<Vec<_>>().join(",");
    if !fresh {
        println!("开始：主屏 {}，屏幕 {}", CGMainDisplayID(), ids());
    }
    let t = Instant::now();
    let hidpi = std::env::var_os("HIDPI").is_some();
    let id = backend.add_virtual_display(&DisplayMode { width: 1920, height: 1080, hidpi }).unwrap();
    {
        // 系统实际选的模式和给的所有模式（新进程才读得到）
        use objc2_core_graphics::CGDisplayMode;
        let n: u32 = id.parse().unwrap();
        let show = |m: &CGDisplayMode| format!("{}×{}点/{}×{}像素", CGDisplayMode::width(Some(m)), CGDisplayMode::height(Some(m)), CGDisplayMode::pixel_width(Some(m)), CGDisplayMode::pixel_height(Some(m)));
        println!("系统选的：{:?}", CGDisplayCopyDisplayMode(n).map(|m| show(&m)));
        let key: &objc2_core_foundation::CFString = unsafe { objc2_core_graphics::kCGDisplayShowDuplicateLowResolutionModes };
        let opts = objc2_core_foundation::CFDictionary::<objc2_core_foundation::CFString, objc2_core_foundation::CFBoolean>::from_slices(&[key], &[objc2_core_foundation::CFBoolean::new(true)]);
        if let Some(arr) = unsafe { objc2_core_graphics::CGDisplayCopyAllDisplayModes(n, Some(opts.as_opaque())) } {
            let arr: objc2_core_foundation::CFRetained<objc2_core_foundation::CFArray<CGDisplayMode>> = unsafe { objc2_core_foundation::CFRetained::cast_unchecked(arr) };
            println!("系统给的：{}", arr.iter().map(|m| show(&m)).collect::<Vec<_>>().join("，"));
        }
        println!("抓屏框架看到的像素：{:?}", p2pcli_screen::mac_screen_pixel_size(n));
    }
    let n: u32 = id.parse().unwrap();
    println!("建好 {id}，用了 {} 毫秒", t.elapsed().as_millis());
    let online = || {
        let mut ids = [0u32; 16];
        let mut count = 0u32;
        unsafe { objc2_core_graphics::CGGetOnlineDisplayList(16, ids.as_mut_ptr(), &mut count) };
        ids[..count as usize].iter().map(|id| format!("{id}({:#x})", CGDisplayVendorNumber(*id))).collect::<Vec<_>>().join(",")
    };
    for i in 0..6 {
        let b = CGDisplayBounds(n);
        println!(
            "  {:>5} 毫秒：在线 {}，主屏 {}，它在 ({}, {}) {}×{}，系统读得到模式 {}，能选 {} 种",
            i * 1000,
            online(),
            CGMainDisplayID(),
            b.origin.x,
            b.origin.y,
            b.size.width,
            b.size.height,
            CGDisplayCopyDisplayMode(n).is_some(),
            backend.display_modes(&id).0.len()
        );
        std::thread::sleep(Duration::from_millis(1000));
    }
    let r = backend.set_display_mode(&id, &DisplayMode { width: 1920, height: 1080, hidpi: true });
    println!("换成 1920×1080 HiDPI：{r:?}，屏幕 {}", ids());
    backend.remove_virtual_display(&id).unwrap();
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(20) {
        if !backend.displays().unwrap().iter().any(|d| d.id == id) {
            println!("删掉后 {} 毫秒消失", t.elapsed().as_millis());
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if backend.displays().unwrap().iter().any(|d| d.id == id) {
        println!("删掉 20 秒了还在");
    }
    println!("结束：主屏 {}，屏幕 {}", CGMainDisplayID(), ids());
}

fn main() {
    #[cfg(target_os = "macos")]
    {
        tracing_subscriber::fmt().with_env_filter("info").init();
        let arg = std::env::args().nth(1).unwrap_or_default();
        if arg == "nsapp" {
            use objc2::runtime::{AnyClass, AnyObject};
            let class = AnyClass::get(c"NSApplication").unwrap();
            let _app: *mut AnyObject = unsafe { objc2::msg_send![class, sharedApplication] };
        }
        if arg == "runloop" || arg == "nsapp" {
            let worker = std::thread::spawn(|| probe(false));
            while !worker.is_finished() {
                objc2_core_foundation::CFRunLoop::run_in_mode(unsafe { objc2_core_foundation::kCFRunLoopDefaultMode }, 0.1, false);
            }
        } else {
            probe(arg == "fresh");
        }
    }
}
