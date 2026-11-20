//! SendInput 注入键盘鼠标。键盘按扫描码发，对面按它自己的键盘布局出字。
//! 以管理员身份运行的窗口（UIPI）、UAC 弹窗和锁屏界面收不到普通程序注入的事件，这是系统的限制。

use std::collections::HashSet;

use anyhow::Result;
use p2pcli_core::screen::{InputEvent, MouseButton};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
    KEYEVENTF_SCANCODE, MOUSE_EVENT_FLAGS, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT, SendInput, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

/// 滚轮一格是 120；浏览器里一格大约 100 像素
const WHEEL_PER_PIXEL: f64 = 120.0 / 100.0;
const XBUTTON1: u32 = 1;
const XBUTTON2: u32 = 2;

#[derive(Default)]
pub struct Injector {
    /// 按着的键：(扫描码, 扩展键)
    keys: HashSet<(u16, bool)>,
    buttons: [bool; 5],
    /// 不满一格的滚动先攒着
    wheel: (f64, f64),
    last: (i32, i32),
}

fn send(inputs: &[INPUT]) {
    unsafe {
        SendInput(inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

fn mouse(dx: i32, dy: i32, data: i32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: data as u32, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    }
}

fn key(scan: u16, extended: bool, down: bool) -> INPUT {
    let mut flags: KEYBD_EVENT_FLAGS = KEYEVENTF_SCANCODE;
    if extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if !down {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0), wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    }
}

fn button_flags(b: MouseButton, down: bool) -> (MOUSE_EVENT_FLAGS, i32) {
    match (b, down) {
        (MouseButton::Left, true) => (MOUSEEVENTF_LEFTDOWN, 0),
        (MouseButton::Left, false) => (MOUSEEVENTF_LEFTUP, 0),
        (MouseButton::Right, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
        (MouseButton::Right, false) => (MOUSEEVENTF_RIGHTUP, 0),
        (MouseButton::Middle, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
        (MouseButton::Middle, false) => (MOUSEEVENTF_MIDDLEUP, 0),
        (MouseButton::Back, d) => (if d { MOUSEEVENTF_XDOWN } else { MOUSEEVENTF_XUP }, XBUTTON1 as i32),
        (MouseButton::Forward, d) => (if d { MOUSEEVENTF_XDOWN } else { MOUSEEVENTF_XUP }, XBUTTON2 as i32),
    }
}

impl Injector {
    /// 屏幕上的 0~1 → SendInput 的绝对坐标（整个虚拟桌面映射到 0~65535）
    fn absolute(&mut self, display: &str, x: f64, y: f64) -> Result<(i32, i32)> {
        let m = super::find_monitor(display)?;
        let w = (m.rect.right - m.rect.left).max(1);
        let h = (m.rect.bottom - m.rect.top).max(1);
        let px = m.rect.left + (x.clamp(0.0, 1.0) * (w - 1) as f64).round() as i32;
        let py = m.rect.top + (y.clamp(0.0, 1.0) * (h - 1) as f64).round() as i32;
        let (vx, vy, vw, vh) = unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN).max(2),
                GetSystemMetrics(SM_CYVIRTUALSCREEN).max(2),
            )
        };
        let ax = (((px - vx) as i64 * 65535) / (vw - 1) as i64) as i32;
        let ay = (((py - vy) as i64 * 65535) / (vh - 1) as i64) as i32;
        self.last = (ax, ay);
        Ok((ax, ay))
    }

    pub fn inject(&mut self, display: &str, ev: &InputEvent) -> Result<()> {
        let abs = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
        match *ev {
            InputEvent::Move { x, y } => {
                let (ax, ay) = self.absolute(display, x, y)?;
                send(&[mouse(ax, ay, 0, abs)]);
            }
            InputEvent::Button { button, down, x, y } => {
                let (ax, ay) = self.absolute(display, x, y)?;
                let (flags, data) = button_flags(button, down);
                self.buttons[button as usize] = down;
                // 先移过去再按，同一批发出去
                send(&[mouse(ax, ay, 0, abs), mouse(ax, ay, data, flags | abs)]);
            }
            InputEvent::Wheel { dx, dy, .. } => {
                // 浏览器 dy 正数往下滚；Windows 滚轮正数往上。横向两边都是正数往右
                self.wheel.0 += -dy * WHEEL_PER_PIXEL;
                self.wheel.1 += dx * WHEEL_PER_PIXEL;
                let v = self.wheel.0.trunc() as i32;
                let h = self.wheel.1.trunc() as i32;
                self.wheel.0 -= v as f64;
                self.wheel.1 -= h as f64;
                let mut inputs = Vec::new();
                if v != 0 {
                    inputs.push(mouse(0, 0, v, MOUSEEVENTF_WHEEL));
                }
                if h != 0 {
                    inputs.push(mouse(0, 0, h, MOUSEEVENTF_HWHEEL));
                }
                if !inputs.is_empty() {
                    send(&inputs);
                }
            }
            InputEvent::Key { ref code, down } => {
                if let Some((scan, extended)) = crate::keymap::windows(code) {
                    if down {
                        self.keys.insert((scan, extended));
                    } else {
                        self.keys.remove(&(scan, extended));
                    }
                    send(&[key(scan, extended, down)]);
                }
            }
            InputEvent::ReleaseAll => {
                let mut inputs: Vec<INPUT> = self.keys.drain().map(|(s, e)| key(s, e, false)).collect();
                let all = [MouseButton::Left, MouseButton::Right, MouseButton::Middle, MouseButton::Back, MouseButton::Forward];
                for (i, held) in self.buttons.into_iter().enumerate() {
                    if held {
                        let (flags, data) = button_flags(all[i], false);
                        inputs.push(mouse(self.last.0, self.last.1, data, flags | abs));
                    }
                }
                self.buttons = [false; 5];
                if !inputs.is_empty() {
                    send(&inputs);
                }
            }
        }
        Ok(())
    }
}
