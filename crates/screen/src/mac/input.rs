//! CGEvent 注入键盘鼠标。要“辅助功能”权限。

use std::collections::HashSet;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use objc2_core_foundation::{CFRetained, CGPoint};
use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayBounds, CGEvent, CGEventField, CGEventFlags, CGEventSource, CGEventSourceStateID,
    CGEventTapLocation, CGEventType, CGMouseButton, CGScrollEventUnit,
};
use p2pcli_core::screen::{InputEvent, MouseButton};

/// 双击判定：间隔和距离
const MULTI_CLICK: Duration = Duration::from_millis(500);
const MULTI_CLICK_DIST: f64 = 6.0;

pub struct Injector {
    source: Option<CFRetained<CGEventSource>>,
    /// 对面按着的键（mac 键码）
    keys: HashSet<u16>,
    /// 按着的鼠标键
    buttons: [bool; 5],
    flags: CGEventFlags,
    last: CGPoint,
    click: Option<(MouseButton, Instant, CGPoint, i64)>,
}

unsafe impl Send for Injector {}

fn index(b: MouseButton) -> usize {
    match b {
        MouseButton::Left => 0,
        MouseButton::Right => 1,
        MouseButton::Middle => 2,
        MouseButton::Back => 3,
        MouseButton::Forward => 4,
    }
}

fn modifier(vk: u16) -> Option<CGEventFlags> {
    Some(match vk {
        0x38 | 0x3C => CGEventFlags::MaskShift,
        0x3B | 0x3E => CGEventFlags::MaskControl,
        0x3A | 0x3D => CGEventFlags::MaskAlternate,
        0x37 | 0x36 => CGEventFlags::MaskCommand,
        _ => return None,
    })
}

/// 方向键、翻页这些键真实键盘发出来时带 Fn 和小键盘标记，有的程序认这个
fn extra_flags(vk: u16) -> CGEventFlags {
    match vk {
        0x7B..=0x7E => CGEventFlags::MaskSecondaryFn | CGEventFlags::MaskNumericPad,
        0x72..=0x79 => CGEventFlags::MaskSecondaryFn,
        _ => CGEventFlags(0),
    }
}

impl Injector {
    pub fn new() -> Self {
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState);
        // 默认注入一个事件后本机的键鼠会被压住 0.25 秒；对面在操作时本机的人也要能动
        CGEventSource::set_local_events_suppression_interval(source.as_deref(), 0.0);
        // 打上标记：隐私屏拦本机键鼠时认它放行
        CGEventSource::set_user_data(source.as_deref(), crate::privacy::INPUT_TAG as i64);
        Injector { source, keys: HashSet::new(), buttons: [false; 5], flags: CGEventFlags(0), last: CGPoint::new(0.0, 0.0), click: None }
    }

    fn point(display: &str, x: f64, y: f64) -> Result<CGPoint> {
        let id: CGDirectDisplayID = display.parse().context("屏幕编号不对")?;
        let b = CGDisplayBounds(id);
        let x = x.clamp(0.0, 1.0) * (b.size.width - 1.0).max(0.0) + b.origin.x;
        let y = y.clamp(0.0, 1.0) * (b.size.height - 1.0).max(0.0) + b.origin.y;
        Ok(CGPoint::new(x, y))
    }

    fn post(&self, ev: Option<CFRetained<CGEvent>>) {
        if let Some(ev) = ev {
            CGEvent::set_flags(Some(&ev), self.flags);
            CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&ev));
        }
    }

    fn mouse(&self, kind: CGEventType, at: CGPoint, button: CGMouseButton, number: i64, clicks: i64) {
        let ev = CGEvent::new_mouse_event(self.source.as_deref(), kind, at, button);
        if let Some(ev) = &ev {
            if number > 1 {
                CGEvent::set_integer_value_field(Some(ev), CGEventField::MouseEventButtonNumber, number);
            }
            if clicks > 0 {
                CGEvent::set_integer_value_field(Some(ev), CGEventField::MouseEventClickState, clicks);
            }
        }
        self.post(ev);
    }

    fn button_kind(button: MouseButton, down: bool) -> (CGEventType, CGMouseButton, i64) {
        match (button, down) {
            (MouseButton::Left, true) => (CGEventType::LeftMouseDown, CGMouseButton::Left, 0),
            (MouseButton::Left, false) => (CGEventType::LeftMouseUp, CGMouseButton::Left, 0),
            (MouseButton::Right, true) => (CGEventType::RightMouseDown, CGMouseButton::Right, 1),
            (MouseButton::Right, false) => (CGEventType::RightMouseUp, CGMouseButton::Right, 1),
            (b, d) => {
                let n = match b {
                    MouseButton::Middle => 2,
                    MouseButton::Back => 3,
                    _ => 4,
                };
                (if d { CGEventType::OtherMouseDown } else { CGEventType::OtherMouseUp }, CGMouseButton::Center, n)
            }
        }
    }

    fn key(&mut self, vk: u16, down: bool) {
        if let Some(m) = modifier(vk) {
            if down {
                self.flags.0 |= m.0;
            } else {
                self.flags.0 &= !m.0;
            }
        }
        if down {
            self.keys.insert(vk);
        } else {
            self.keys.remove(&vk);
        }
        let ev = CGEvent::new_keyboard_event(self.source.as_deref(), vk, down);
        if let Some(ev) = ev {
            CGEvent::set_flags(Some(&ev), CGEventFlags(self.flags.0 | extra_flags(vk).0));
            CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&ev));
        }
    }

    pub fn inject(&mut self, display: &str, ev: &InputEvent) -> Result<()> {
        match *ev {
            InputEvent::Move { x, y } => {
                let at = Self::point(display, x, y)?;
                self.last = at;
                let (kind, button) = self.move_kind();
                self.mouse(kind, at, button, 0, 0);
            }
            InputEvent::Button { button, down, x, y } => {
                let at = Self::point(display, x, y)?;
                self.last = at;
                let clicks = if down {
                    let n = match self.click {
                        Some((b, t, p, n))
                            if b == button && t.elapsed() < MULTI_CLICK && (p.x - at.x).hypot(p.y - at.y) < MULTI_CLICK_DIST =>
                        {
                            (n % 3) + 1
                        }
                        _ => 1,
                    };
                    self.click = Some((button, Instant::now(), at, n));
                    n
                } else {
                    self.click.map(|c| c.3).unwrap_or(1)
                };
                self.buttons[index(button)] = down;
                let (kind, cg, number) = Self::button_kind(button, down);
                self.mouse(kind, at, cg, number, clicks);
            }
            InputEvent::Wheel { dx, dy, x, y } => {
                // 滚轮落在鼠标下面的窗口上：鼠标不在滚轮的位置（移动事件丢了、还没到）就先移过去
                let at = Self::point(display, x, y)?;
                if (at.x - self.last.x).abs() >= 1.0 || (at.y - self.last.y).abs() >= 1.0 {
                    self.inject(display, &InputEvent::Move { x, y })?;
                }
                // 浏览器 dy 正数是往下滚，CGEvent 正数是往上
                let ev = CGEvent::new_scroll_wheel_event2(
                    self.source.as_deref(),
                    CGScrollEventUnit::Pixel,
                    2,
                    (-dy).round() as i32,
                    (-dx).round() as i32,
                    0,
                );
                self.post(ev);
            }
            InputEvent::Key { ref code, down } => {
                if let Some(vk) = crate::keymap::mac(code) {
                    self.key(vk, down);
                }
            }
            InputEvent::ReleaseAll => self.release_all(),
            InputEvent::Text { ref text } => {
                // 按着的修饰键会把字变成快捷键：先松开
                self.release_all();
                self.text(text);
            }
            InputEvent::RelativeMove { dx, dy } => {
                // 指针在原来的位置挪一点，同时带上移动量：游戏读的是移动量
                let mut at = Self::here();
                at.x += dx;
                at.y += dy;
                if let Ok(id) = display.parse::<CGDirectDisplayID>() {
                    let b = CGDisplayBounds(id);
                    at.x = at.x.clamp(b.origin.x, b.origin.x + (b.size.width - 1.0).max(0.0));
                    at.y = at.y.clamp(b.origin.y, b.origin.y + (b.size.height - 1.0).max(0.0));
                }
                self.last = at;
                let (kind, button) = self.move_kind();
                let ev = CGEvent::new_mouse_event(self.source.as_deref(), kind, at, button);
                if let Some(ev) = &ev {
                    CGEvent::set_integer_value_field(Some(ev), CGEventField::MouseEventDeltaX, dx.round() as i64);
                    CGEvent::set_integer_value_field(Some(ev), CGEventField::MouseEventDeltaY, dy.round() as i64);
                }
                self.post(ev);
            }
            InputEvent::ButtonHere { button, down } => {
                let at = Self::here();
                self.last = at;
                self.buttons[index(button)] = down;
                let (kind, cg, number) = Self::button_kind(button, down);
                self.mouse(kind, at, cg, number, 1);
            }
            InputEvent::WheelHere { dx, dy } => {
                let ev = CGEvent::new_scroll_wheel_event2(
                    self.source.as_deref(),
                    CGScrollEventUnit::Pixel,
                    2,
                    (-dy).round() as i32,
                    (-dx).round() as i32,
                    0,
                );
                self.post(ev);
            }
        }
        Ok(())
    }

    /// 指针现在在哪（全局坐标）
    fn here() -> CGPoint {
        CGEvent::new(None).map(|e| CGEvent::location(Some(&e))).unwrap_or(CGPoint::new(0.0, 0.0))
    }

    /// 移动用哪种事件：按着键时是拖动
    fn move_kind(&self) -> (CGEventType, CGMouseButton) {
        if self.buttons[0] {
            (CGEventType::LeftMouseDragged, CGMouseButton::Left)
        } else if self.buttons[1] {
            (CGEventType::RightMouseDragged, CGMouseButton::Right)
        } else if self.buttons[2..].iter().any(|b| *b) {
            (CGEventType::OtherMouseDragged, CGMouseButton::Center)
        } else {
            (CGEventType::MouseMoved, CGMouseButton::Left)
        }
    }

    /// 直接打字：每个键盘事件带一段 Unicode 文字（一次最多 20 个 UTF-16 码元），不经过键盘布局和输入法。
    /// 回车、Tab 按真键发
    fn text(&mut self, text: &str) {
        let mut pending: Vec<u16> = Vec::new();
        let flush = |this: &Self, pending: &mut Vec<u16>| {
            for chunk in pending.chunks(20) {
                for down in [true, false] {
                    let ev = CGEvent::new_keyboard_event(this.source.as_deref(), 0, down);
                    if let Some(ev) = &ev {
                        unsafe { CGEvent::keyboard_set_unicode_string(Some(ev), chunk.len() as _, chunk.as_ptr()) };
                        CGEvent::set_flags(Some(ev), CGEventFlags(0));
                        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(ev));
                    }
                }
            }
            pending.clear();
        };
        for ch in text.replace("\r\n", "\n").chars() {
            let real = match ch {
                '\n' | '\r' => Some(0x24),
                '\t' => Some(0x30),
                _ => None,
            };
            if let Some(vk) = real {
                flush(self, &mut pending);
                self.key(vk, true);
                self.key(vk, false);
            } else {
                let mut buf = [0u16; 2];
                pending.extend_from_slice(ch.encode_utf16(&mut buf));
                // 别把一对代理码元（表情之类）拆到两个事件里
                if pending.len() >= 18 {
                    flush(self, &mut pending);
                }
            }
        }
        flush(self, &mut pending);
    }

    fn release_all(&mut self) {
        let keys: Vec<u16> = self.keys.iter().copied().collect();
        for vk in keys {
            self.key(vk, false);
        }
        self.flags = CGEventFlags(0);
        for (i, held) in self.buttons.into_iter().enumerate() {
            if held {
                let button = [MouseButton::Left, MouseButton::Right, MouseButton::Middle, MouseButton::Back, MouseButton::Forward][i];
                let (kind, cg, number) = Self::button_kind(button, false);
                self.mouse(kind, self.last, cg, number, 1);
            }
        }
        self.buttons = [false; 5];
    }
}

#[cfg(test)]
mod tests {
    //! 会真的挪本机的鼠标、要辅助功能权限，默认不跑：cargo test -p p2pcli-screen --lib -- --ignored relative_move
    use super::*;

    /// 相对移动真的挪了指针（往右挪再挪回来，贴着屏幕边时有一次挪不动）
    #[test]
    #[ignore]
    fn relative_move_moves_pointer() {
        let mut inj = Injector::new();
        let display = objc2_core_graphics::CGMainDisplayID().to_string();
        let start = Injector::here();
        inj.inject(&display, &InputEvent::RelativeMove { dx: 40.0, dy: 0.0 }).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let mid = Injector::here();
        inj.inject(&display, &InputEvent::RelativeMove { dx: -40.0, dy: 0.0 }).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let end = Injector::here();
        println!("指针 {:?} → {:?} → {:?}", (start.x, start.y), (mid.x, mid.y), (end.x, end.y));
        assert!((mid.x - start.x).abs() >= 1.0 || (end.x - mid.x).abs() >= 1.0, "相对移动没挪动指针");
    }
}
