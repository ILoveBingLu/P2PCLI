//! 虚拟屏：本机看不见、远程画面里和真屏幕一样用的屏幕（远程抓它、键鼠往它上面打）。两种来源：
//! - 无头模式：Mac 一块显示器都没有（Mac mini 没接屏幕、合盖的 MacBook 没接外屏）时自己建一块；
//! - 远程看画面的人在工具栏里建：多一块屏幕用，或者外接显示器没有 HiDPI 时要更细的画面。
//!
//! 用的是 CoreGraphics 里的 CGVirtualDisplay（系统没公开，但 macOS 11 起一直在，DeskPad、BetterDisplay
//! 这些工具都靠它），不用装驱动、不用签名。没公开就可能在以后的系统里变：类找不到、建不起来时报错。
//!
//! 虚拟屏开着 HiDPI：给系统的分辨率里有两倍像素的档（比如 3840×2160），系统就多出“1920×1080（HiDPI）”，
//! 界面和字一样大、像素是两倍。
//!
//! 只在需要时建：看画面时它在，画面都关了 30 秒没人再看就收掉，不在系统里常驻看不见的屏幕。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::msg_send;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject};
use objc2_core_foundation::CGSize;
use objc2_core_graphics::{CGDirectDisplayID, CGGetActiveDisplayList};
use objc2_foundation::{NSArray, NSString};
use p2pcli_core::screen::DisplayMode;
use tracing::{info, warn};

/// 虚拟屏能选的大小（按界面算），每种都有普通档和 HiDPI 档（像素两倍，超过 MAX_PIXELS 的没有）
const SIZES: &[(u32, u32)] = &[(2560, 1440), (1920, 1200), (1920, 1080), (1680, 1050), (1600, 900), (1440, 900), (1366, 768), (1280, 800), (1280, 720)];
const MAX_PIXELS: (u32, u32) = (3840, 2400);
/// 列表里最宽到多少（两倍像素的档系统也当普通档列出来，4K 原生字太小，不列）
pub const MAX_WIDTH: u32 = 2560;
/// 默认 1080p、不开 HiDPI（码率省，要细可以换）
pub const DEFAULT_MODE: DisplayMode = DisplayMode { width: 1920, height: 1080, hidpi: false };
const REFRESH: f64 = 60.0;
/// 远程最多建几块
const MAX_ADDED: usize = 4;
/// 没人看以后多久收掉
const IDLE: Duration = Duration::from_secs(30);
/// 厂商号：认出是自己建的
pub const VENDOR: u32 = 0x5032;

/// 一块虚拟屏。丢掉就从系统里消失
pub struct VirtualDisplay {
    _display: Retained<AnyObject>,
    _queue: DispatchRetained<DispatchQueue>,
    pub id: CGDirectDisplayID,
}

// 只在建和丢的时候碰，CoreGraphics 允许跨线程
unsafe impl Send for VirtualDisplay {}
unsafe impl Sync for VirtualDisplay {}

fn class(name: &std::ffi::CStr) -> Result<&'static AnyClass> {
    AnyClass::get(name).ok_or_else(|| anyhow!("这个 macOS 不支持虚拟屏（找不到 {}）", name.to_string_lossy()))
}

fn active_displays() -> Vec<CGDirectDisplayID> {
    let mut ids = [0u32; 16];
    let mut count = 0u32;
    let err = unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count) };
    if err.0 != 0 {
        return Vec::new();
    }
    ids[..count as usize].to_vec()
}

impl VirtualDisplay {
    /// 建一块虚拟屏，换成 mode 这个分辨率
    pub fn create(name: &str, mode: &DisplayMode) -> Result<Self> {
        let main = objc2_core_graphics::CGMainDisplayID();
        // 建的过程里系统会自动释放一些对象，有的引用着这块显示器：不在池子里清掉，丢掉它以后它也不会消失
        let d = objc2::rc::autoreleasepool(|_| Self::create_inner(name))?;
        // 有真屏幕时，主屏不能被它抢走（菜单栏、程序坞会跑到看不见的屏幕上）
        if main != d.id && objc2_core_graphics::CGMainDisplayID() == d.id && active_displays().contains(&main) {
            match super::modes::keep_main(main, d.id) {
                Ok(()) => info!(main, "虚拟屏被当成了主屏，摆回原来的主屏"),
                Err(e) => warn!("虚拟屏被当成了主屏，摆不回来：{e:#}"),
            }
        }
        // 刚挂上时系统还在重新排屏幕，分辨率列表要等一会儿才读得到
        let deadline = Instant::now() + Duration::from_secs(3);
        while super::modes::list(d.id, MAX_WIDTH).0.len() < 2 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        if let Err(e) = super::modes::set(d.id, mode) {
            warn!("虚拟屏换不成 {}×{}：{e:#}", mode.width, mode.height);
        }
        Ok(d)
    }

    fn create_inner(name: &str) -> Result<Self> {
        let queue = DispatchQueue::new("cn.p2pcli.virtual-display", None);
        let display: Retained<AnyObject> = unsafe {
            let desc: Option<Retained<AnyObject>> = msg_send![class(c"CGVirtualDisplayDescriptor")?, new];
            let desc = desc.ok_or_else(|| anyhow!("建不了虚拟屏的描述"))?;
            let _: () = msg_send![&desc, setDispatchQueue: &*queue];
            let _: () = msg_send![&desc, setName: &*NSString::from_str(&format!("P2PCLI {name}"))];
            let _: () = msg_send![&desc, setMaxPixelsWide: MAX_PIXELS.0];
            let _: () = msg_send![&desc, setMaxPixelsHigh: MAX_PIXELS.1];
            // 24 英寸左右的物理尺寸，系统按它算 DPI
            let _: () = msg_send![&desc, setSizeInMillimeters: CGSize::new(530.0, 300.0)];
            let _: () = msg_send![&desc, setVendorID: VENDOR];
            let _: () = msg_send![&desc, setProductID: 0x0001u32];
            // 序列号每块不一样：上一块还没被系统摘干净（屏幕睡着时会晚一点）就再建，用同一个号会被拒
            let serial = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.subsec_nanos() | 1);
            let _: () = msg_send![&desc, setSerialNum: serial];
            let alloc: Allocated<AnyObject> = msg_send![class(c"CGVirtualDisplay")?, alloc];
            let display: Option<Retained<AnyObject>> = msg_send![alloc, initWithDescriptor: &*desc];
            let display = display.ok_or_else(|| anyhow!("系统没建出虚拟屏"))?;

            // 每种大小给一档原生像素、一档两倍像素（开了 HiDPI，系统把两倍的当 HiDPI 档）
            let mut modes = Vec::new();
            for &(w, h) in SIZES {
                for k in [1, 2] {
                    let (pw, ph) = (w * k, h * k);
                    if pw > MAX_PIXELS.0 || ph > MAX_PIXELS.1 {
                        continue;
                    }
                    let mode_alloc: Allocated<AnyObject> = msg_send![class(c"CGVirtualDisplayMode")?, alloc];
                    let mode: Option<Retained<AnyObject>> = msg_send![mode_alloc, initWithWidth: pw, height: ph, refreshRate: REFRESH];
                    modes.push(mode.ok_or_else(|| anyhow!("建不了虚拟屏的分辨率"))?);
                }
            }
            let settings: Option<Retained<AnyObject>> = msg_send![class(c"CGVirtualDisplaySettings")?, new];
            let settings = settings.ok_or_else(|| anyhow!("建不了虚拟屏的设置"))?;
            let _: () = msg_send![&settings, setHiDPI: 1u32];
            let modes = NSArray::from_retained_slice(&modes);
            let _: () = msg_send![&settings, setModes: &*modes];
            let ok: bool = msg_send![&display, applySettings: &*settings];
            if !ok {
                bail!("虚拟屏的分辨率设置不上");
            }
            display
        };
        let id: u32 = unsafe { msg_send![&display, displayID] };
        if id == 0 {
            bail!("虚拟屏没有编号");
        }
        // 系统挂上它要一点时间：等它出现在屏幕列表里、读得出分辨率（刚挂上时读到的是 0×0）
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let size = super::capture::pixel_size(id);
            if active_displays().contains(&id) && size.0 > 0 && size.1 > 0 {
                break;
            }
            if Instant::now() > deadline {
                let mode = objc2_core_graphics::CGDisplayCopyDisplayMode(id);
                bail!(
                    "虚拟屏建好了，但系统没把它当成屏幕：在列表里 {}，模式 {:?}，像素 {}×{}，范围 {:?}",
                    active_displays().contains(&id),
                    mode.as_deref().map(|m| (objc2_core_graphics::CGDisplayMode::pixel_width(Some(m)), objc2_core_graphics::CGDisplayMode::pixel_height(Some(m)))),
                    objc2_core_graphics::CGDisplayPixelsWide(id),
                    objc2_core_graphics::CGDisplayPixelsHigh(id),
                    objc2_core_graphics::CGDisplayBounds(id),
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        info!(id, name, "建了一块虚拟屏");
        Ok(VirtualDisplay { _display: display, _queue: queue, id })
    }
}

impl Drop for VirtualDisplay {
    fn drop(&mut self) {
        info!(id = self.id, "收掉虚拟屏");
    }
}

/// 等虚拟屏从屏幕列表里消失，最多等 timeout；返回等了多久（没等到是空）
#[cfg(test)]
fn wait_gone(id: CGDirectDisplayID, timeout: Duration) -> Option<Duration> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if !active_displays().contains(&id) {
            return Some(start.elapsed());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    None
}

/// 一块虚拟屏和它的来历
struct Entry {
    display: Arc<VirtualDisplay>,
    /// 无头模式自己建的（不是远程建的）
    auto: bool,
    name: String,
}

/// 本机的虚拟屏。画面开着时，抓屏那边各拿一份；只剩这里这一份、过了 IDLE 就收掉
#[derive(Default)]
pub struct Virtuals {
    slots: Arc<Mutex<Vec<Entry>>>,
    reaper: Arc<Mutex<bool>>,
}

impl Virtuals {
    /// 测试、排查用：P2PCLI_VIRTUAL_DISPLAY=1 时有真屏幕也建一块
    fn forced() -> bool {
        std::env::var_os("P2PCLI_VIRTUAL_DISPLAY").is_some()
    }

    /// 无头模式：本机没有真屏幕（或者强制）、也还没有虚拟屏时建一块
    pub fn ensure_headless(&self, real_displays: usize) {
        let mut slots = self.slots.lock().unwrap();
        if !slots.is_empty() || (real_displays > 0 && !Self::forced()) {
            return;
        }
        let name = "虚拟显示器（没接屏幕时用）".to_string();
        match VirtualDisplay::create(&name, &DEFAULT_MODE) {
            Ok(d) => {
                slots.push(Entry { display: Arc::new(d), auto: true, name });
                drop(slots);
                self.spawn_reaper();
            }
            Err(e) => warn!("没有显示器，虚拟显示器也建不起来：{e:#}"),
        }
    }

    /// 远程建一块，返回编号
    pub fn add(&self, mode: &DisplayMode) -> Result<CGDirectDisplayID> {
        let mut slots = self.slots.lock().unwrap();
        if slots.iter().filter(|e| !e.auto).count() >= MAX_ADDED {
            bail!("最多建 {MAX_ADDED} 块虚拟屏");
        }
        let n = (1..).find(|n| !slots.iter().any(|e| e.name == format!("虚拟屏 {n}"))).unwrap_or(1);
        let name = format!("虚拟屏 {n}");
        let d = VirtualDisplay::create(&name, mode)?;
        let id = d.id;
        slots.push(Entry { display: Arc::new(d), auto: false, name });
        drop(slots);
        self.spawn_reaper();
        Ok(id)
    }

    /// 删掉一块（还有人在抓它时，等他们停了才真正消失）
    pub fn remove(&self, id: CGDirectDisplayID) -> Result<()> {
        let mut slots = self.slots.lock().unwrap();
        let before = slots.len();
        slots.retain(|e| e.display.id != id);
        if slots.len() == before {
            bail!("这不是 P2PCLI 建的虚拟屏");
        }
        Ok(())
    }

    /// 是不是虚拟屏；是就给一份，抓屏期间拿着，它就不会被收掉
    pub fn hold(&self, id: CGDirectDisplayID) -> Option<Arc<VirtualDisplay>> {
        self.slots.lock().unwrap().iter().find(|e| e.display.id == id).map(|e| e.display.clone())
    }

    pub fn is_virtual(&self, id: CGDirectDisplayID) -> bool {
        self.slots.lock().unwrap().iter().any(|e| e.display.id == id)
    }

    /// 虚拟屏的名字（不是虚拟屏是空）
    pub fn name(&self, id: CGDirectDisplayID) -> Option<String> {
        self.slots.lock().unwrap().iter().find(|e| e.display.id == id).map(|e| e.name.clone())
    }

    /// 每 5 秒看一眼：只剩这里这一份（没人在看）超过 IDLE 的收掉；一块都不剩就停
    fn spawn_reaper(&self) {
        let mut running = self.reaper.lock().unwrap();
        if *running {
            return;
        }
        *running = true;
        let (slots, reaper) = (self.slots.clone(), self.reaper.clone());
        let _ = std::thread::Builder::new().name("p2pcli-virtual-display".into()).spawn(move || {
            let mut idle: std::collections::HashMap<CGDirectDisplayID, Instant> = Default::default();
            loop {
                std::thread::sleep(Duration::from_secs(5));
                let mut guard = slots.lock().unwrap();
                guard.retain(|e| {
                    if Arc::strong_count(&e.display) > 1 {
                        idle.remove(&e.display.id);
                        return true;
                    }
                    idle.entry(e.display.id).or_insert_with(Instant::now).elapsed() < IDLE
                });
                idle.retain(|id, _| guard.iter().any(|e| e.display.id == *id));
                if guard.is_empty() {
                    *reaper.lock().unwrap() = false;
                    return;
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 建一块虚拟屏：出现在屏幕列表里、分辨率对、有 1920×1080 HiDPI 档并且换得过去、能截到图；
    /// 丢掉以后从列表里消失。会在本机多出一块屏幕几秒钟，默认不跑：
    /// cargo test -p p2pcli-screen --lib -- --ignored virtual_display --nocapture
    #[test]
    #[ignore]
    fn virtual_display_appears_and_goes() {
        let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();
        let d = VirtualDisplay::create("测试", &DEFAULT_MODE).expect("建不了虚拟屏");
        let id = d.id;
        assert!(active_displays().contains(&id));
        let (w, h) = super::super::capture::pixel_size(id);
        println!("虚拟屏 {id}：{w}×{h}");
        assert_eq!((w, h), (DEFAULT_MODE.width, DEFAULT_MODE.height));
        let (modes, now) = super::super::modes::list(id, MAX_WIDTH);
        println!("能选的分辨率：{modes:?}，现在 {now:?}");
        let hidpi = DisplayMode { width: 1920, height: 1080, hidpi: true };
        assert!(modes.contains(&hidpi), "没有 1920×1080 HiDPI");
        super::super::modes::set(id, &hidpi).expect("换不成 HiDPI");
        assert_eq!(super::super::capture::pixel_size(id), (3840, 2160));
        let shot = super::super::shot::capture(&id.to_string(), 640).expect("截不到虚拟屏");
        println!("截图 {}×{}，{} 字节", shot.width, shot.height, shot.png.len());
        drop(d);
        let gone = wait_gone(id, Duration::from_secs(10));
        println!("丢掉以后 {gone:?} 从屏幕列表里消失");
        assert!(gone.is_some(), "丢掉 10 秒了还在屏幕列表里");
    }
}
