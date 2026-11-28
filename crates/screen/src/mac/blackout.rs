//! 增强隐私屏：把本机每块真显示器的伽马表（显卡送到屏幕前最后一道颜色映射）设成全零，屏幕上整个是黑的，
//! 指针、通知、锁屏这些盖在隐私屏窗口上面的东西也看不见。抓屏抓的是映射之前的画面，远程照样看得到桌面。
//!
//! 系统在程序退出（包括崩溃）时自动恢复伽马表，不会把屏幕留成黑的。显示器睡醒、插拔、夜览这些会把伽马表改回去，
//! 所以每半秒看一眼，不是全黑就再设一次。虚拟显示器（无头模式建的）没人看得见，不动它。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayGammaTableCapacity, CGDisplayRestoreColorSyncSettings, CGDisplayVendorNumber, CGGetActiveDisplayList,
    CGGetDisplayTransferByTable, CGSetDisplayTransferByTable,
};
use tracing::{info, warn};

/// 无头模式虚拟显示器的厂商号（见 virtual_display.rs）
const VIRTUAL_VENDOR: u32 = 0x5032;
/// 多久看一眼伽马表有没有被改回去
const RECHECK: Duration = Duration::from_millis(500);

/// 黑着的屏幕。丢掉就恢复
pub struct Blackout {
    stop: Arc<AtomicBool>,
}

/// 本机的真显示器
fn real_displays() -> Vec<CGDirectDisplayID> {
    let mut ids = [0u32; 16];
    let mut count = 0u32;
    if unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count) }.0 != 0 {
        return Vec::new();
    }
    ids[..count as usize].iter().copied().filter(|&id| CGDisplayVendorNumber(id) != VIRTUAL_VENDOR).collect()
}

/// 这块屏幕的伽马表是不是全零
fn is_black(id: CGDirectDisplayID) -> bool {
    let cap = CGDisplayGammaTableCapacity(id).clamp(2, 4096);
    let (mut r, mut g, mut b) = (vec![0f32; cap as usize], vec![0f32; cap as usize], vec![0f32; cap as usize]);
    let mut n = 0u32;
    let err = unsafe { CGGetDisplayTransferByTable(id, cap, r.as_mut_ptr(), g.as_mut_ptr(), b.as_mut_ptr(), &mut n) };
    let n = n.min(cap) as usize;
    err.0 == 0 && n > 0 && [&r, &g, &b].iter().all(|t| t[..n].iter().all(|v| *v <= 0.0))
}

/// 把这块屏幕的伽马表设成全零，返回设上了没有
fn make_black(id: CGDirectDisplayID) -> bool {
    let zeros = [0f32; 2];
    let err = unsafe { CGSetDisplayTransferByTable(id, zeros.len() as u32, zeros.as_ptr(), zeros.as_ptr(), zeros.as_ptr()) };
    err.0 == 0
}

/// 现在本机的真显示器是不是都黑着（测试、排查用）
pub fn all_black() -> bool {
    let ids = real_displays();
    !ids.is_empty() && ids.into_iter().all(is_black)
}

impl Blackout {
    pub fn start() -> anyhow::Result<Self> {
        let ids = real_displays();
        if ids.is_empty() {
            // 只有虚拟显示器（无头模式）：本来就没人看得见
            info!("没有真显示器，不用变黑");
        }
        for &id in &ids {
            if !make_black(id) {
                CGDisplayRestoreColorSyncSettings();
                anyhow::bail!("屏幕 {id} 变不了黑");
            }
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let spawned = std::thread::Builder::new().name("p2pcli-privacy-blackout".into()).spawn(move || {
            while !stopped.load(Ordering::SeqCst) {
                std::thread::sleep(RECHECK);
                if stopped.load(Ordering::SeqCst) {
                    break;
                }
                // 新插上的屏幕也一起变黑
                for id in real_displays() {
                    if !is_black(id) && !make_black(id) {
                        warn!(id, "屏幕又亮了，变不回黑");
                    }
                }
            }
            // 停的时候可能刚好又设了一次黑：最后再恢复一次
            CGDisplayRestoreColorSyncSettings();
        });
        if let Err(e) = spawned {
            CGDisplayRestoreColorSyncSettings();
            anyhow::bail!("开不了检查屏幕的线程：{e}");
        }
        info!(screens = ids.len(), "本机屏幕变黑了");
        Ok(Blackout { stop })
    }
}

impl Drop for Blackout {
    fn drop(&mut self) {
        // 不等检查的线程：马上恢复，它停下时会再恢复一次
        self.stop.store(true, Ordering::SeqCst);
        CGDisplayRestoreColorSyncSettings();
        info!("本机屏幕恢复了");
    }
}
