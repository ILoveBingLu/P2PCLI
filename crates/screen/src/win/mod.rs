//! Windows：Windows.Graphics.Capture 抓成 D3D11 纹理 → GPU 视频处理器缩放、转 NV12 →
//! 显卡的硬件 H.264 编码器（NVENC、Quick Sync、AMF，经 Media Foundation）；SendInput 注入键鼠。

use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow, bail};
use p2pcli_core::screen::{CaptureOptions, DisplayInfo, InputEvent, ScreenBackend, ScreenCapture};
use windows::Graphics::Capture::GraphicsCaptureSession;
use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplayMonitors, EnumDisplaySettingsW, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW,
};
use windows::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;
use windows::core::{BOOL, PCWSTR};

mod capture;
mod encode;
mod input;

pub fn backend() -> Option<Arc<dyn ScreenBackend>> {
    Some(Arc::new(WinBackend { input: Mutex::new(input::Injector::default()) }))
}

pub struct WinBackend {
    input: Mutex<input::Injector>,
}

/// 一块屏幕：系统里的设备名（\\.\DISPLAY1）当编号，加上它在虚拟桌面里的位置（物理像素）
#[derive(Debug, Clone)]
pub(crate) struct Monitor {
    pub handle: HMONITOR,
    pub id: String,
    pub rect: RECT,
    pub primary: bool,
}

unsafe impl Send for Monitor {}

pub(crate) fn monitors() -> Vec<Monitor> {
    unsafe extern "system" fn each(handle: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        let list = unsafe { &mut *(data.0 as *mut Vec<Monitor>) };
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if unsafe { GetMonitorInfoW(handle, &mut info as *mut MONITORINFOEXW as *mut _) }.as_bool() {
            let name = String::from_utf16_lossy(&info.szDevice);
            let id = name.trim_end_matches('\0').to_string();
            list.push(Monitor { handle, id, rect: info.monitorInfo.rcMonitor, primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0 });
        }
        true.into()
    }
    let mut list: Vec<Monitor> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(each), LPARAM(&mut list as *mut Vec<Monitor> as isize));
    }
    list
}

/// 屏幕当前的真实分辨率（像素）。程序没声明感知缩放时，显示器区域是按缩放换算过的，这里不受影响
fn pixel_size(m: &Monitor) -> (u32, u32) {
    let name: Vec<u16> = m.id.encode_utf16().chain(std::iter::once(0)).collect();
    let mut mode = DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
    if unsafe { EnumDisplaySettingsW(PCWSTR(name.as_ptr()), ENUM_CURRENT_SETTINGS, &mut mode) }.as_bool() && mode.dmPelsWidth > 0 {
        return (mode.dmPelsWidth, mode.dmPelsHeight);
    }
    ((m.rect.right - m.rect.left).max(0) as u32, (m.rect.bottom - m.rect.top).max(0) as u32)
}

pub(crate) fn find_monitor(id: &str) -> Result<Monitor> {
    monitors().into_iter().find(|m| m.id == id).ok_or_else(|| anyhow!("找不到这块屏幕，可能已经拔掉了"))
}

impl ScreenBackend for WinBackend {
    fn displays(&self) -> Result<Vec<DisplayInfo>> {
        let mut n = 0;
        Ok(monitors()
            .into_iter()
            .map(|m| {
                n += 1;
                let (width, height) = pixel_size(&m);
                DisplayInfo {
                    name: if m.primary { "主显示器".to_string() } else { format!("显示器 {n}") },
                    width,
                    height,
                    primary: m.primary,
                    id: m.id,
                }
            })
            .collect())
    }

    fn check_capture(&self) -> Result<()> {
        if !GraphicsCaptureSession::IsSupported().unwrap_or(false) {
            bail!("对方的 Windows 版本太旧，不支持远程画面（需要 Windows 10 1903 或更新的版本）");
        }
        Ok(())
    }

    fn check_control(&self) -> Result<()> {
        Ok(())
    }

    fn start(&self, opts: CaptureOptions) -> Result<Arc<dyn ScreenCapture>> {
        Ok(Arc::new(capture::WinCapture::start(opts)?))
    }

    fn inject(&self, display: &str, ev: &InputEvent) -> Result<()> {
        self.input.lock().unwrap().inject(display, ev)
    }
}
