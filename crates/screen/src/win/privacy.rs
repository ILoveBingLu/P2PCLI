//! Windows 隐私屏：每块屏幕盖一个置顶窗口（黑底、渐变或图片，加上时钟和一行字），
//! SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE) 让所有抓屏（WGC、BitBlt）都看不到它，
//! 远程画面里看到的是底下的桌面；窗口点不到，远程注入的点击落在底下的程序上。
//! 低级键鼠钩子拦住本机的键盘鼠标，只放行远程注入的（dwExtraInfo 是 [`INPUT_TAG`]）。
//! Ctrl+Alt+Del、Win+L 钩子拦不住，本机的人照样能锁屏。
//!
//! 窗口和钩子都在一个自己的线程上，带消息循环；丢掉时给它发 WM_QUIT，不等。
//! WDA_EXCLUDEFROMCAPTURE 要 Windows 10 2004 以上，老系统设不上就报错、不盖。

use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use p2pcli_core::screen::PrivacyStyle;
use tracing::{debug, info, warn};
use windows::Win32::Foundation::{COLORREF, ERROR_CLASS_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLACK_BRUSH, BeginPaint, BitBlt, CLEARTYPE_QUALITY, CreateCompatibleBitmap, CreateCompatibleDC,
    CreateFontW, DEFAULT_CHARSET, DIB_RGB_COLORS, DT_CALCRECT, DT_CENTER, DT_NOPREFIX, DT_SINGLELINE, DT_WORDBREAK, DeleteDC, DeleteObject,
    DrawTextW, EndPaint, EnumDisplayMonitors, FillRect, GetMonitorInfoW, GetStockObject, HALFTONE, HBRUSH, HDC, HFONT, HMONITOR,
    InvalidateRect, MONITORINFO, OUT_DEFAULT_PRECIS, PAINTSTRUCT, SRCCOPY, SelectObject, SetBkMode, SetStretchBltMode, SetTextColor,
    StretchDIBits, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA, GetClientRect,
    GetMessageW, GetWindowLongPtrW, HC_ACTION, HHOOK, HWND_TOPMOST, KBDLLHOOKSTRUCT, LWA_ALPHA, MA_NOACTIVATE, MSG, MSLLHOOKSTRUCT,
    PostThreadMessageW, RegisterClassW, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetLayeredWindowAttributes,
    SetTimer, SetWindowDisplayAffinity, SetWindowLongPtrW, SetWindowPos, SetWindowsHookExW, ShowWindow, TranslateMessage,
    UnhookWindowsHookEx, WDA_EXCLUDEFROMCAPTURE, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_ERASEBKGND, WM_MOUSEACTIVATE, WM_NCCREATE,
    WM_NCDESTROY, WM_PAINT, WM_QUIT, WM_TIMER, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{BOOL, PCWSTR, w};

use crate::privacy::{INPUT_TAG, background, clock_text, date_text};

const CLASS: PCWSTR = w!("P2PCLIPrivacyCover");
/// 每秒重画时钟、把窗口再提到最上面
const TIMER: usize = 1;

/// 盖着的隐私屏。丢掉就撤掉
pub struct WinPrivacy {
    thread: u32,
}

/// 一块屏幕上的窗口要画的东西
struct Cover {
    /// 背景：BGRA，从上到下；空是纯黑
    bg: Option<(Vec<u8>, i32, i32)>,
    clock: bool,
    message: Vec<u16>,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

impl WinPrivacy {
    pub fn start(style: &PrivacyStyle, block_input: bool) -> Result<Self> {
        let (tx, rx) = mpsc::channel::<Result<u32>>();
        let style = style.clone();
        std::thread::Builder::new().name("p2pcli-privacy".into()).spawn(move || run(style, block_input, tx))?;
        let thread = rx.recv_timeout(Duration::from_secs(10)).map_err(|_| anyhow!("隐私屏的线程没响应"))??;
        info!(block_input, "隐私屏盖上了");
        Ok(WinPrivacy { thread })
    }
}

impl Drop for WinPrivacy {
    fn drop(&mut self) {
        let _ = unsafe { PostThreadMessageW(self.thread, WM_QUIT, WPARAM(0), LPARAM(0)) };
        info!("隐私屏撤掉了");
    }
}

unsafe extern "system" fn collect_monitor(monitor: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
    let list = unsafe { &mut *(data.0 as *mut Vec<RECT>) };
    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        list.push(info.rcMonitor);
    }
    true.into()
}

/// 隐私屏线程：建窗口、装钩子，跑消息循环到收到 WM_QUIT，再全部撤掉
fn run(style: PrivacyStyle, block_input: bool, ready: mpsc::Sender<Result<u32>>) {
    // 这个线程按物理像素算：程序没声明高 DPI 时，系统会把窗口按缩放比例放大，盖歪、字也糊
    unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let mut windows: Vec<HWND> = Vec::new();
    let mut hooks: Vec<HHOOK> = Vec::new();
    let setup = (|| -> Result<()> {
        let instance = unsafe { GetModuleHandleW(None) }?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance.into(),
            lpszClassName: CLASS,
            hbrBackground: HBRUSH(unsafe { GetStockObject(BLACK_BRUSH) }.0),
            ..Default::default()
        };
        if unsafe { RegisterClassW(&class) } == 0 && unsafe { GetLastError() } != ERROR_CLASS_ALREADY_EXISTS {
            return Err(anyhow!("隐私屏窗口建不起来：{}", windows::core::Error::from_thread()));
        }
        let mut monitors: Vec<RECT> = Vec::new();
        let _ = unsafe { EnumDisplayMonitors(None, None, Some(collect_monitor), LPARAM(&mut monitors as *mut _ as isize)) };
        if monitors.is_empty() {
            return Err(anyhow!("没有屏幕可以盖"));
        }
        for r in &monitors {
            let (w, h) = (r.right - r.left, r.bottom - r.top);
            let bg = match background(&style, w as u32, h as u32) {
                Ok(Some(img)) => {
                    let (bw, bh) = (img.width() as i32, img.height() as i32);
                    let mut bgra = img.into_raw();
                    for p in bgra.as_chunks_mut::<4>().0 {
                        p.swap(0, 2);
                    }
                    Some((bgra, bw, bh))
                }
                Ok(None) => None,
                Err(e) => {
                    warn!("隐私屏背景画不出来，用黑底：{e:#}");
                    None
                }
            };
            let cover = Box::new(Cover { bg, clock: style.clock, message: wide(style.message.trim()) });
            let hwnd = unsafe {
                CreateWindowExW(
                    WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT,
                    CLASS,
                    w!("P2PCLI 隐私屏"),
                    WS_POPUP,
                    r.left,
                    r.top,
                    w,
                    h,
                    None,
                    None,
                    Some(instance.into()),
                    Some(Box::into_raw(cover) as *const _),
                )
            }?;
            windows.push(hwnd);
            // 分层窗口 + 透明：鼠标点击穿过去，落在底下的程序上
            unsafe { SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA) }?;
            // 先排除出抓屏再显示：远程画面一下都不会黑。
            // 排查用：P2PCLI_PRIVACY_VISIBLE=1 时不排除，截图里能看到隐私屏画成什么样
            if std::env::var_os("P2PCLI_PRIVACY_VISIBLE").is_none() {
                unsafe { SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) }
                    .map_err(|e| anyhow!("这台 Windows 太老，抓屏排除不了隐私屏（要 Windows 10 2004 或更新）：{e}"))?;
            }
            let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
            unsafe { SetTimer(Some(hwnd), TIMER, 1000, None) };
        }
        if block_input {
            hooks.push(unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), Some(instance.into()), 0) }.map_err(|e| anyhow!("拦不住本机的键盘：{e}"))?);
            hooks.push(unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), Some(instance.into()), 0) }.map_err(|e| anyhow!("拦不住本机的鼠标：{e}"))?);
        }
        Ok(())
    })();
    let ok = setup.is_ok();
    let _ = ready.send(setup.map(|_| unsafe { GetCurrentThreadId() }));
    if ok {
        let mut msg = MSG::default();
        while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
    // 先放开键鼠，再撤窗口
    for hook in hooks {
        let _ = unsafe { UnhookWindowsHookEx(hook) };
    }
    for hwnd in windows {
        let _ = unsafe { DestroyWindow(hwnd) };
    }
    debug!("隐私屏线程结束");
}

/// 没带远程注入标记的键盘事件（本机的人按的）吃掉
unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) }.dwExtraInfo != INPUT_TAG as usize {
        return LRESULT(1);
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// 没带远程注入标记的鼠标事件（本机的人动的）吃掉
unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) }.dwExtraInfo != INPUT_TAG as usize {
        return LRESULT(1);
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn font(height: i32, weight: i32) -> HFONT {
    unsafe {
        CreateFontW(
            -height,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            Default::default(),
            CLEARTYPE_QUALITY,
            0,
            w!("Microsoft YaHei UI"),
        )
    }
}

/// 在以 center_y 为中心的地方写一段居中的字，返回占了多高
fn text(dc: HDC, s: &[u16], height: i32, weight: i32, width: i32, center_y: i32, wrap: bool) -> i32 {
    if s.is_empty() {
        return 0;
    }
    let f = font(height, weight);
    let old = unsafe { SelectObject(dc, f.into()) };
    let flags = DT_CENTER | DT_NOPREFIX | if wrap { DT_WORDBREAK } else { DT_SINGLELINE };
    let margin = width / 10;
    let mut buf = s.to_vec();
    let mut r = RECT { left: margin, top: 0, right: width - margin, bottom: 0 };
    unsafe { DrawTextW(dc, &mut buf, &mut r, flags | DT_CALCRECT) };
    let h = r.bottom - r.top;
    // 字后面不画硬阴影（GDI 画不出柔的，硬的显得廉价）：背景在字的位置已经压了一圈暗晕
    let mut r = RECT { left: margin, top: center_y - h / 2, right: width - margin, bottom: center_y + h / 2 + 1 };
    unsafe { DrawTextW(dc, &mut buf, &mut r, flags) };
    unsafe {
        SelectObject(dc, old);
        let _ = DeleteObject(f.into());
    }
    h
}

fn paint(hwnd: HWND, cover: &Cover) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
    let mut rc = RECT::default();
    let _ = unsafe { GetClientRect(hwnd, &mut rc) };
    let (w, h) = (rc.right, rc.bottom);
    unsafe {
        // 先画在内存里再一次贴上去，时钟每秒重画不闪
        let mem = CreateCompatibleDC(Some(hdc));
        let bmp = CreateCompatibleBitmap(hdc, w, h);
        let old = SelectObject(mem, bmp.into());
        FillRect(mem, &rc, HBRUSH(GetStockObject(BLACK_BRUSH).0));
        if let Some((bits, bw, bh)) = &cover.bg {
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: *bw,
                    biHeight: -*bh,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            SetStretchBltMode(mem, HALFTONE);
            StretchDIBits(mem, 0, 0, w, h, 0, 0, *bw, *bh, Some(bits.as_ptr() as *const _), &info, DIB_RGB_COLORS, SRCCOPY);
        }
        SetBkMode(mem, TRANSPARENT);
        SetTextColor(mem, COLORREF(0x00FF_FFFF));
        let msg_size = ((h as f32 * 0.026) as i32).clamp(16, 44);
        if cover.clock {
            let big = (h as f32 * 0.16) as i32;
            let center = (h as f32 * 0.44) as i32;
            let bh = text(mem, &wide(&clock_text()), big.max(48), 300, w, center, false);
            let below = center + bh / 2;
            let dh = text(mem, &wide(&date_text()), ((h as f32 * 0.028) as i32).max(16), 400, w, below + msg_size, false);
            text(mem, &cover.message, msg_size, 400, w, below + dh + (h as f32 * 0.08) as i32, true);
        } else {
            text(mem, &cover.message, msg_size, 400, w, h / 2, true);
        }
        let _ = BitBlt(hdc, 0, 0, w, h, Some(mem), 0, 0, SRCCOPY);
        SelectObject(mem, old);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem);
        let _ = EndPaint(hwnd, &ps);
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_NCCREATE => {
                let create = &*(lparam.0 as *const CREATESTRUCTW);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
            }
            WM_PAINT => {
                let cover = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Cover;
                if let Some(cover) = cover.as_ref() {
                    paint(hwnd, cover);
                    return LRESULT(0);
                }
            }
            WM_ERASEBKGND => return LRESULT(1),
            WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
            WM_TIMER => {
                let cover = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Cover;
                if cover.as_ref().is_some_and(|c| c.clock) {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                // 别的置顶窗口可能盖上来，每秒提一次
                let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                return LRESULT(0);
            }
            WM_NCDESTROY => {
                let cover = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Cover;
                if !cover.is_null() {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    drop(Box::from_raw(cover));
                }
            }
            _ => {}
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}
