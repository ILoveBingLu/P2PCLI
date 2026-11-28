//! 屏幕分辨率：列出显卡给这块屏幕的分辨率，远程换成其中一种。
//! 改的是这次开机的设置（不写进注册表，重启后回到原来的）。Windows 没有 macOS 那种 HiDPI 档，缩放另外设。

use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use p2pcli_core::screen::DisplayMode;
use tracing::info;
use windows::Win32::Graphics::Gdi::{
    CDS_TYPE, ChangeDisplaySettingsExW, DEVMODEW, DISP_CHANGE_SUCCESSFUL, DM_DISPLAYFREQUENCY, DM_PELSHEIGHT, DM_PELSWIDTH,
    ENUM_CURRENT_SETTINGS, ENUM_DISPLAY_SETTINGS_MODE, EnumDisplaySettingsW,
};
use windows::core::PCWSTR;

/// 列表里不要太小的
const MIN_WIDTH: u32 = 1024;

fn wide(id: &str) -> Vec<u16> {
    id.encode_utf16().chain(std::iter::once(0)).collect()
}

fn read(name: &[u16], index: ENUM_DISPLAY_SETTINGS_MODE) -> Option<DEVMODEW> {
    let mut mode = DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
    unsafe { EnumDisplaySettingsW(PCWSTR(name.as_ptr()), index, &mut mode) }.as_bool().then_some(mode)
}

/// 显卡给的所有 32 位色的分辨率
fn all(name: &[u16]) -> Vec<DEVMODEW> {
    (0..).map_while(|i| read(name, ENUM_DISPLAY_SETTINGS_MODE(i))).filter(|m| m.dmBitsPerPel == 32 && m.dmPelsWidth > 0).collect()
}

fn describe(m: &DEVMODEW) -> DisplayMode {
    DisplayMode { width: m.dmPelsWidth, height: m.dmPelsHeight, hidpi: false }
}

pub fn current(id: &str) -> Option<DisplayMode> {
    read(&wide(id), ENUM_CURRENT_SETTINGS).map(|m| describe(&m))
}

/// 能选的分辨率，从大到小
pub fn list(id: &str) -> (Vec<DisplayMode>, Option<DisplayMode>) {
    let name = wide(id);
    let now = current(id);
    let mut modes: Vec<DisplayMode> = all(&name).iter().map(describe).filter(|m| m.width >= MIN_WIDTH).collect();
    if let Some(now) = now {
        modes.push(now);
    }
    modes.sort_by_key(|m| (std::cmp::Reverse(m.width), std::cmp::Reverse(m.height)));
    modes.dedup();
    (modes, now)
}

/// 换成这个分辨率：同样大小优先和现在一样的刷新率，没有就挑最高的。等系统换好才返回
pub fn set(id: &str, want: &DisplayMode) -> Result<()> {
    let name = wide(id);
    let now = read(&name, ENUM_CURRENT_SETTINGS).ok_or_else(|| anyhow!("读不到这块屏幕现在的分辨率"))?;
    if describe(&now) == (DisplayMode { hidpi: false, ..*want }) {
        return Ok(());
    }
    let candidates: Vec<DEVMODEW> = all(&name).into_iter().filter(|m| m.dmPelsWidth == want.width && m.dmPelsHeight == want.height).collect();
    let mut mode = candidates
        .iter()
        .find(|m| m.dmDisplayFrequency == now.dmDisplayFrequency)
        .or_else(|| candidates.iter().max_by_key(|m| m.dmDisplayFrequency))
        .copied()
        .ok_or_else(|| anyhow!("这块屏幕没有 {}×{} 这个分辨率", want.width, want.height))?;
    mode.dmFields = DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYFREQUENCY;
    let result = unsafe { ChangeDisplaySettingsExW(PCWSTR(name.as_ptr()), Some(&mode), None, CDS_TYPE(0), None) };
    if result != DISP_CHANGE_SUCCESSFUL {
        bail!("系统没换成 {}×{}（{}）", want.width, want.height, result.0);
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    while current(id).is_none_or(|m| (m.width, m.height) != (want.width, want.height)) {
        if Instant::now() > deadline {
            bail!("系统没换成 {}×{}", want.width, want.height);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // 显示器重新同步要一会儿，抓屏太早开会拿到旧尺寸
    std::thread::sleep(Duration::from_millis(300));
    info!(id, width = want.width, height = want.height, "换了分辨率");
    Ok(())
}
