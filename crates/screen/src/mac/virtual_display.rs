//! 无头模式：Mac 一块显示器都没有（Mac mini 没接屏幕、合盖的 MacBook 没接外屏）时，自己建一块虚拟显示器，
//! 远程画面抓它、键鼠往它上面打，和真屏幕一样用。
//!
//! 用的是 CoreGraphics 里的 CGVirtualDisplay（系统没公开，但 macOS 11 起一直在，DeskPad、BetterDisplay
//! 这些工具都靠它），不用装驱动、不用签名。没公开就可能在以后的系统里变：类找不到、建不起来时报错，
//! 观看方看到的还是“没有可用的显示器”。
//!
//! 只在需要时建：看画面时它在，画面都关了 30 秒没人再看就收掉，不在系统里常驻一块看不见的屏幕。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject};
use objc2::msg_send;
use objc2_core_foundation::CGSize;
use objc2_core_graphics::{CGDirectDisplayID, CGGetActiveDisplayList};
use objc2_foundation::{NSArray, NSString};
use tracing::{info, warn};

/// 虚拟显示器的分辨率：1080p、60 Hz、不开 HiDPI（看小字够用，码率也省）
const WIDTH: u32 = 1920;
const HEIGHT: u32 = 1080;
const REFRESH: f64 = 60.0;
/// 没人看以后多久收掉
const IDLE: Duration = Duration::from_secs(30);

/// 一块虚拟显示器。丢掉就从系统里消失
pub struct VirtualDisplay {
    _display: Retained<AnyObject>,
    _queue: DispatchRetained<DispatchQueue>,
    pub id: CGDirectDisplayID,
}

// 只在建和丢的时候碰，CoreGraphics 允许跨线程
unsafe impl Send for VirtualDisplay {}
unsafe impl Sync for VirtualDisplay {}

fn class(name: &std::ffi::CStr) -> Result<&'static AnyClass> {
    AnyClass::get(name).ok_or_else(|| anyhow!("这个 macOS 不支持虚拟显示器（找不到 {}）", name.to_string_lossy()))
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
    pub fn create() -> Result<Self> {
        // 建的过程里系统会自动释放一些对象，有的引用着这块显示器：不在池子里清掉，丢掉它以后它也不会消失
        objc2::rc::autoreleasepool(|_| Self::create_inner())
    }

    fn create_inner() -> Result<Self> {
        let queue = DispatchQueue::new("cn.p2pcli.virtual-display", None);
        let display: Retained<AnyObject> = unsafe {
            let desc: Option<Retained<AnyObject>> = msg_send![class(c"CGVirtualDisplayDescriptor")?, new];
            let desc = desc.ok_or_else(|| anyhow!("建不了虚拟显示器的描述"))?;
            let _: () = msg_send![&desc, setDispatchQueue: &*queue];
            let _: () = msg_send![&desc, setName: &*NSString::from_str("P2PCLI 虚拟显示器")];
            let _: () = msg_send![&desc, setMaxPixelsWide: WIDTH];
            let _: () = msg_send![&desc, setMaxPixelsHigh: HEIGHT];
            // 24 英寸左右的物理尺寸，系统按它算 DPI
            let _: () = msg_send![&desc, setSizeInMillimeters: CGSize::new(530.0, 300.0)];
            let _: () = msg_send![&desc, setVendorID: 0x5032u32];
            let _: () = msg_send![&desc, setProductID: 0x0001u32];
            // 序列号每块不一样：上一块还没被系统摘干净（屏幕睡着时会晚一点）就再建，用同一个号会被拒
            let serial = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.subsec_nanos() | 1);
            let _: () = msg_send![&desc, setSerialNum: serial];
            let alloc: Allocated<AnyObject> = msg_send![class(c"CGVirtualDisplay")?, alloc];
            let display: Option<Retained<AnyObject>> = msg_send![alloc, initWithDescriptor: &*desc];
            let display = display.ok_or_else(|| anyhow!("系统没建出虚拟显示器"))?;

            let mode_alloc: Allocated<AnyObject> = msg_send![class(c"CGVirtualDisplayMode")?, alloc];
            let mode: Option<Retained<AnyObject>> = msg_send![mode_alloc, initWithWidth: WIDTH, height: HEIGHT, refreshRate: REFRESH];
            let mode = mode.ok_or_else(|| anyhow!("建不了虚拟显示器的分辨率"))?;
            let settings: Option<Retained<AnyObject>> = msg_send![class(c"CGVirtualDisplaySettings")?, new];
            let settings = settings.ok_or_else(|| anyhow!("建不了虚拟显示器的设置"))?;
            let _: () = msg_send![&settings, setHiDPI: 0u32];
            let modes = NSArray::from_retained_slice(&[mode]);
            let _: () = msg_send![&settings, setModes: &*modes];
            let ok: bool = msg_send![&display, applySettings: &*settings];
            if !ok {
                bail!("虚拟显示器的分辨率设置不上");
            }
            display
        };
        let id: u32 = unsafe { msg_send![&display, displayID] };
        if id == 0 {
            bail!("虚拟显示器没有编号");
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
                    "虚拟显示器建好了，但系统没把它当成屏幕：在列表里 {}，模式 {:?}，像素 {}×{}，范围 {:?}",
                    active_displays().contains(&id),
                    mode.as_deref().map(|m| (objc2_core_graphics::CGDisplayMode::pixel_width(Some(m)), objc2_core_graphics::CGDisplayMode::pixel_height(Some(m)))),
                    objc2_core_graphics::CGDisplayPixelsWide(id),
                    objc2_core_graphics::CGDisplayPixelsHigh(id),
                    objc2_core_graphics::CGDisplayBounds(id),
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        info!(id, width = WIDTH, height = HEIGHT, "没有显示器，建了一块虚拟显示器");
        Ok(VirtualDisplay { _display: display, _queue: queue, id })
    }
}

impl Drop for VirtualDisplay {
    fn drop(&mut self) {
        info!(id = self.id, "收掉虚拟显示器");
    }
}

/// 等虚拟显示器从屏幕列表里消失，最多等 timeout；返回等了多久（没等到是空）
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

/// 本机的虚拟显示器（没有就是空）。画面开着时，抓屏那边各拿一份；只剩这里这一份、过了 IDLE 就收掉
#[derive(Default)]
pub struct Headless {
    slot: Arc<Mutex<Option<Arc<VirtualDisplay>>>>,
}

impl Headless {
    /// 测试、排查用：P2PCLI_VIRTUAL_DISPLAY=1 时有真屏幕也建一块
    fn forced() -> bool {
        std::env::var_os("P2PCLI_VIRTUAL_DISPLAY").is_some()
    }

    /// 本机没有屏幕（或者强制）时，确保有一块虚拟显示器，返回它的编号
    pub fn ensure(&self, real_displays: usize) -> Option<CGDirectDisplayID> {
        let mut slot = self.slot.lock().unwrap();
        if let Some(d) = slot.as_ref() {
            return Some(d.id);
        }
        if real_displays > 0 && !Self::forced() {
            return None;
        }
        match VirtualDisplay::create() {
            Ok(d) => {
                let id = d.id;
                *slot = Some(Arc::new(d));
                self.spawn_reaper();
                Some(id)
            }
            Err(e) => {
                warn!("没有显示器，虚拟显示器也建不起来：{e:#}");
                None
            }
        }
    }

    /// 是不是虚拟显示器；是就给一份，抓屏期间拿着，它就不会被收掉
    pub fn hold(&self, id: CGDirectDisplayID) -> Option<Arc<VirtualDisplay>> {
        self.slot.lock().unwrap().as_ref().filter(|d| d.id == id).cloned()
    }

    pub fn is_virtual(&self, id: CGDirectDisplayID) -> bool {
        self.slot.lock().unwrap().as_ref().is_some_and(|d| d.id == id)
    }

    /// 每 5 秒看一眼：只剩这里这一份（没人在看）超过 IDLE 就收掉
    fn spawn_reaper(&self) {
        let slot = self.slot.clone();
        let _ = std::thread::Builder::new().name("p2pcli-virtual-display".into()).spawn(move || {
            let mut idle_since: Option<Instant> = None;
            loop {
                std::thread::sleep(Duration::from_secs(5));
                let mut guard = slot.lock().unwrap();
                let Some(d) = guard.as_ref() else { return };
                if Arc::strong_count(d) > 1 {
                    idle_since = None;
                    continue;
                }
                let since = *idle_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= IDLE {
                    *guard = None;
                    return;
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 建一块虚拟显示器：出现在屏幕列表里、分辨率对、能截到图；丢掉以后从列表里消失。
    /// 会在本机多出一块屏幕几秒钟，默认不跑：cargo test -p p2pcli-screen --lib -- --ignored virtual_display --nocapture
    #[test]
    #[ignore]
    fn virtual_display_appears_and_goes() {
        let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();
        let d = VirtualDisplay::create().expect("建不了虚拟显示器");
        let id = d.id;
        assert!(active_displays().contains(&id));
        let (w, h) = super::super::capture::pixel_size(id);
        println!("虚拟显示器 {id}：{w}×{h}");
        assert_eq!((w, h), (WIDTH, HEIGHT));
        let shot = super::super::shot::capture(&id.to_string(), 640).expect("截不到虚拟显示器");
        println!("截图 {}×{}，{} 字节", shot.width, shot.height, shot.png.len());
        drop(d);
        let gone = wait_gone(id, Duration::from_secs(10));
        println!("丢掉以后 {gone:?} 从屏幕列表里消失");
        assert!(gone.is_some(), "丢掉 10 秒了还在屏幕列表里");
    }
}
