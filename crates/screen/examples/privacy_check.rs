//! 隐私屏实测：盖上以后远程截图里看不到它（看到的是底下的桌面）、本机的键鼠被拦住、远程注入的照常能用，
//! 撤掉以后本机键鼠恢复。截图存到给的目录里，眼看一下：
//! cargo run -p p2pcli-screen --example privacy_check -- 输出目录 [背景:black|0|1|2|图片路径] [秒数]
//!
//! macOS 上遮挡窗口要在主线程建，这里主线程跑 run loop，检查放在别的线程里做。
use std::path::PathBuf;
use std::time::Duration;

use p2pcli_core::screen::{InputEvent, PrivacyBackground, PrivacyStyle};

fn main() {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into())).init();
    let mut args = std::env::args().skip(1);
    let out = PathBuf::from(args.next().unwrap_or_else(|| "privacy-check".into()));
    let background = match args.next().as_deref() {
        None | Some("black") => PrivacyBackground::Black,
        Some(n) if n.len() == 1 => PrivacyBackground::Preset { index: n.parse().unwrap_or(0) },
        Some(path) => PrivacyBackground::Image { path: path.into() },
    };
    let hold: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(3);
    std::fs::create_dir_all(&out).unwrap();
    let style = PrivacyStyle { background, clock: true, message: p2pcli_core::screen::privacy_message("测试用的电脑") };
    let worker = std::thread::spawn(move || {
        let code = match check(&out, &style, hold) {
            Ok(()) => 0,
            Err(e) => {
                println!("失败：{e:#}");
                1
            }
        };
        std::process::exit(code);
    });
    #[cfg(target_os = "macos")]
    loop {
        objc2_core_foundation::CFRunLoop::run_in_mode(unsafe { objc2_core_foundation::kCFRunLoopDefaultMode }, 0.2, false);
        if worker.is_finished() {
            break;
        }
    }
    let _ = worker.join();
}

/// 两张截图差多少：每个像素每个通道差的平均（0~255）
fn diff(a: &[u8], b: &[u8]) -> f64 {
    let (a, b) = (image::load_from_memory(a).unwrap().to_rgb8(), image::load_from_memory(b).unwrap().to_rgb8());
    if a.dimensions() != b.dimensions() {
        return 255.0;
    }
    let sum: u64 = a.as_raw().iter().zip(b.as_raw()).map(|(x, y)| x.abs_diff(*y) as u64).sum();
    sum as f64 / a.as_raw().len() as f64
}

fn check(out: &std::path::Path, style: &PrivacyStyle, hold: u64) -> anyhow::Result<()> {
    let backend = p2pcli_screen::backend().expect("这个系统不支持");
    println!("能开隐私屏：{}", backend.privacy_supported());
    let displays = backend.displays()?;
    let main = displays.iter().find(|d| d.primary).unwrap_or(&displays[0]).clone();
    println!("主屏 {} {}×{}", main.id, main.width, main.height);

    let before = backend.screenshot(&main.id, 1280)?;
    std::fs::write(out.join("1-盖上前.png"), &before.png)?;
    let p0 = untagged::cursor();

    let t = std::time::Instant::now();
    let guard = backend.start_privacy(style, true)?;
    println!("盖上用了 {} 毫秒", t.elapsed().as_millis());
    std::thread::sleep(Duration::from_millis(800));

    // 远程看到的：应该和盖上前差不多（底下的桌面）
    let during = backend.screenshot(&main.id, 1280)?;
    std::fs::write(out.join("2-盖着时远程看到的.png"), &during.png)?;
    let d = diff(&before.png, &during.png);
    println!("远程截图和盖上前差 {d:.1}（小于 20 算看到的是桌面）");
    // 本机屏幕上实际显示的：应该是隐私屏
    if let Some(local) = untagged::local_screenshot(&out.join("3-盖着时本机屏幕.png")) {
        println!("本机屏幕截图：{local}");
    }

    // 本机键鼠：没带标记的移动被拦住，远程注入的照常
    let target = (p0.0 + 137.0, p0.1 + 91.0);
    untagged::move_to(target);
    std::thread::sleep(Duration::from_millis(300));
    let p1 = untagged::cursor();
    let blocked = (p1.0 - target.0).abs() > 2.0 || (p1.1 - target.1).abs() > 2.0;
    println!("本机鼠标挪到 {target:?}：指针在 {p1:?}，{}", if blocked { "被拦住了" } else { "没拦住" });
    backend.inject(&main.id, &InputEvent::Move { x: 0.25, y: 0.25 })?;
    std::thread::sleep(Duration::from_millis(300));
    let p2 = untagged::cursor();
    let moved = (p2.0 - p1.0).abs() > 2.0 || (p2.1 - p1.1).abs() > 2.0;
    println!("远程注入挪到屏幕四分之一处：指针在 {p2:?}，{}", if moved { "挪过去了" } else { "没动" });

    std::thread::sleep(Duration::from_secs(hold));
    drop(guard);
    std::thread::sleep(Duration::from_millis(1000));
    let after = backend.screenshot(&main.id, 1280)?;
    std::fs::write(out.join("4-撤掉后.png"), &after.png)?;
    if let Some(local) = untagged::local_screenshot(&out.join("5-撤掉后本机屏幕.png")) {
        println!("撤掉后本机屏幕截图：{local}");
    }
    let target = (p2.0 + 53.0, p2.1 + 41.0);
    untagged::move_to(target);
    std::thread::sleep(Duration::from_millis(300));
    let p3 = untagged::cursor();
    let free = (p3.0 - target.0).abs() <= 2.0 && (p3.1 - target.1).abs() <= 2.0;
    println!("撤掉后本机鼠标挪到 {target:?}：指针在 {p3:?}，{}", if free { "能动了" } else { "还被拦着" });

    let ok = d < 20.0 && blocked && moved && free;
    println!("结果：{}", if ok { "全部通过" } else { "有问题" });
    if !ok {
        anyhow::bail!("隐私屏有一项不对");
    }
    Ok(())
}

/// 模拟本机的人动鼠标：不带远程注入的标记
#[cfg(target_os = "macos")]
mod untagged {
    use objc2_core_graphics::{CGEvent, CGEventTapLocation, CGEventType, CGMouseButton};
    use objc2_core_foundation::CGPoint;

    pub fn cursor() -> (f64, f64) {
        let p = CGEvent::location(CGEvent::new(None).as_deref());
        (p.x, p.y)
    }

    pub fn move_to(p: (f64, f64)) {
        let ev = CGEvent::new_mouse_event(None, CGEventType::MouseMoved, CGPoint::new(p.0, p.1), CGMouseButton::Left);
        CGEvent::post(CGEventTapLocation::HIDEventTap, ev.as_deref());
    }

    /// 系统自带的截屏（不排除隐私屏窗口）：本机的人实际看到的
    pub fn local_screenshot(path: &std::path::Path) -> Option<String> {
        let ok = std::process::Command::new("screencapture").arg("-x").arg("-m").arg(path).status().ok()?.success();
        Some(if ok { path.display().to_string() } else { "截不了".into() })
    }
}

#[cfg(windows)]
mod untagged {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::Input::KeyboardAndMouse::{INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_MOVE, MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT, SendInput};
    use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN};

    pub fn cursor() -> (f64, f64) {
        let mut p = POINT::default();
        let _ = unsafe { GetCursorPos(&mut p) };
        (p.x as f64, p.y as f64)
    }

    pub fn move_to(p: (f64, f64)) {
        let (x, y, w, h) = unsafe {
            (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN), GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN))
        };
        let dx = (((p.0 - x as f64) * 65535.0) / (w - 1).max(1) as f64).round() as i32;
        let dy = (((p.1 - y as f64) * 65535.0) / (h - 1).max(1) as f64).round() as i32;
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: 0, dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK, time: 0, dwExtraInfo: 0 } },
        };
        unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
    }

    /// Windows 上系统截屏也会排除隐私屏窗口，看不到本机实际显示的
    pub fn local_screenshot(_path: &std::path::Path) -> Option<String> {
        None
    }
}
