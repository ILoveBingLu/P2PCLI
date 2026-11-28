//! 分辨率和虚拟屏实测：列出每块屏幕能选的分辨率；加 set 时把主屏换成另一档再换回来；
//! 能建虚拟屏的系统再建一块 1920×1080 HiDPI 的，看它的分辨率列表、截一张图看像素是不是两倍，然后删掉。
//! macOS 上和 P2PCLI 应用一样，主线程跑应用的事件循环（不然系统读不到新屏幕的模式），活在别的线程上做。
//! cargo run -p p2pcli-screen --example display_modes -- [set]
use std::time::Duration;

use p2pcli_core::screen::DisplayMode;

fn show(backend: &dyn p2pcli_core::screen::ScreenBackend) {
    for d in backend.displays().unwrap() {
        let (modes, now) = backend.display_modes(&d.id);
        let list: Vec<String> = modes.iter().map(|m| format!("{}×{}{}", m.width, m.height, if m.hidpi { "H" } else { "" })).collect();
        println!(
            "屏幕 {} {}{}{} 像素 {}×{} 现在 {:?}\n  能选 {} 种：{}",
            d.id,
            d.name,
            if d.primary { "（主屏）" } else { "" },
            if d.is_virtual { "（虚拟）" } else { "" },
            d.width,
            d.height,
            now,
            modes.len(),
            list.join(" ")
        );
    }
}

fn main() {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into())).init();
    #[cfg(windows)]
    let _ = unsafe { windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    #[cfg(target_os = "macos")]
    {
        std::thread::spawn(|| std::process::exit(if run() { 0 } else { 1 }));
        use objc2::runtime::{AnyClass, AnyObject};
        let app: *mut AnyObject = unsafe { objc2::msg_send![AnyClass::get(c"NSApplication").unwrap(), sharedApplication] };
        let _: bool = unsafe { objc2::msg_send![app, setActivationPolicy: 1isize] };
        let _: () = unsafe { objc2::msg_send![app, run] };
    }
    #[cfg(not(target_os = "macos"))]
    std::process::exit(if run() { 0 } else { 1 });
}

fn run() -> bool {
    let set = std::env::args().nth(1).as_deref() == Some("set");
    let backend = p2pcli_screen::backend().expect("这个系统不支持");
    show(backend.as_ref());
    let mut ok = true;

    if set {
        let main = backend.displays().unwrap().into_iter().find(|d| d.primary).unwrap();
        let (modes, now) = backend.display_modes(&main.id);
        let now = now.expect("读不到主屏现在的分辨率");
        // 挑一档同样宽高比、不一样大的
        let other = modes.iter().find(|m| **m != now && m.hidpi == now.hidpi && m.width * now.height == m.height * now.width).copied();
        match other {
            None => println!("主屏没有别的同比例分辨率可换，跳过"),
            Some(m) => {
                let t = std::time::Instant::now();
                let r = backend.set_display_mode(&main.id, &m);
                let after = backend.displays().unwrap().into_iter().find(|d| d.id == main.id).unwrap();
                println!("主屏换成 {m:?}：{r:?}，用了 {} 毫秒，现在像素 {}×{}，读回 {:?}", t.elapsed().as_millis(), after.width, after.height, backend.display_modes(&main.id).1);
                ok &= r.is_ok() && backend.display_modes(&main.id).1 == Some(m);
                std::thread::sleep(Duration::from_secs(1));
                let r = backend.set_display_mode(&main.id, &now);
                println!("换回 {now:?}：{r:?}，读回 {:?}", backend.display_modes(&main.id).1);
                ok &= r.is_ok() && backend.display_modes(&main.id).1 == Some(now);
            }
        }
    }

    if backend.virtual_displays_supported() {
        let want = DisplayMode { width: 1920, height: 1080, hidpi: true };
        let t = std::time::Instant::now();
        match backend.add_virtual_display(&want) {
            Err(e) => {
                println!("建不了虚拟屏：{e:#}");
                ok = false;
            }
            Ok(id) => {
                println!("建了虚拟屏 {id}，用了 {} 毫秒", t.elapsed().as_millis());
                show(backend.as_ref());
                let now = backend.display_modes(&id).1;
                ok &= now == Some(want);
                #[cfg(target_os = "macos")]
                {
                    let px = p2pcli_screen::mac_screen_pixel_size(id.parse().unwrap());
                    println!("抓屏框架看到虚拟屏的像素 {px:?}（HiDPI 应该是 3840×2160）");
                    ok &= px.is_none_or(|p| p == (3840, 2160));
                }
                match backend.screenshot(&id, 4000) {
                    Ok(shot) => {
                        println!("虚拟屏截图 {}×{}（HiDPI 应该是 3840×2160）", shot.width, shot.height);
                        ok &= (shot.width, shot.height) == (3840, 2160);
                    }
                    Err(e) => {
                        println!("截不到虚拟屏：{e:#}");
                        ok = false;
                    }
                }
                let plain = DisplayMode { width: 1920, height: 1080, hidpi: false };
                let r = backend.set_display_mode(&id, &plain);
                println!("虚拟屏换成普通 1920×1080：{r:?}，读回 {:?}", backend.display_modes(&id).1);
                ok &= r.is_ok() && backend.display_modes(&id).1 == Some(plain);
                let hd = DisplayMode { width: 1280, height: 720, hidpi: true };
                let r = backend.set_display_mode(&id, &hd);
                println!("虚拟屏换成 1280×720 HiDPI：{r:?}，读回 {:?}", backend.display_modes(&id).1);
                ok &= r.is_ok() && backend.display_modes(&id).1 == Some(hd);
                let r = backend.remove_virtual_display(&id);
                println!("删掉虚拟屏：{r:?}");
                ok &= r.is_ok();
                std::thread::sleep(Duration::from_secs(2));
                let gone = !backend.displays().unwrap().iter().any(|d| d.id == id);
                println!("两秒后{}", if gone { "已经不在屏幕列表里" } else { "还在屏幕列表里" });
                ok &= gone;
            }
        }
    }
    println!("结果：{}", if ok { "全部通过" } else { "有问题" });
    ok
}
