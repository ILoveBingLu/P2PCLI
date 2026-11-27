//! 远程控制时 Mac 和 Windows 之间的快捷键转换：按“修饰键的用途”换，不按一个个快捷键查表。
//!
//! 两边的差别其实就三类，每类一条规则，所有程序的快捷键都跟着对：
//!
//! 1. **命令键**：Mac 的 ⌘ 和 Windows 的 Ctrl 是同一个角色（复制、保存、新标签……）。
//!    Mac 的 ⌃ 在 Windows 上还是 Ctrl（终端里 ⌃C、⌃R 照用），Windows 的 Win 键在 Mac 上当 ⌃。
//! 2. **文字里移动光标**：Mac 按词是 ⌥、到行首行尾是 ⌘←/⌘→、到开头结尾是 ⌘↑/⌘↓；
//!    Windows 按词是 Ctrl、行首行尾是 Home/End、开头结尾是 Ctrl+Home/End。方向键、退格、删除键按这个换。
//! 3. **终端**（只在 Mac 被连方上看得出来）：终端里 ⌃ 才是控制键，Windows 用户按 Ctrl+C 是想中断，
//!    所以 Mac 前台是终端时 Ctrl 发 ⌃，Ctrl+Shift+字母发 ⌘+字母（和 Windows Terminal 的复制粘贴一个习惯）。
//!
//! 再加两条两边都有的：Mac 的 ⌘Q（退出）↔ Windows 的 Alt+F4；Mac 上 ⌃+点击是右键。
//! 单按一下 ⌘ 再松开（中间没按别的）在 Windows 上是按一下 Win 键（开始菜单）。
//!
//! 难点在修饰键要“晚一点按”：Mac 的 ⌘ 按下时还不知道接下来是 C（要 Ctrl）还是 ←（要 Home，不能带 Ctrl）。
//! 所以 ⌘、⌥（Mac 到 Windows）和 Ctrl（Windows 到 Mac）先记着不发，等按下第一个普通键、或者点鼠标时，
//! 按这个组合算出对方要按着哪些修饰键，把对方实际按着的修饰键补齐或者松开，再发这个键。
//! Shift 和别的一对一的修饰键（Mac 的 ⌃、Windows 的 Alt、Win）照常马上发，游戏里按住 Ctrl 蹲下这类不受影响。
//!
//! 转换在被连方做：被连方知道自己是什么系统、前台是不是终端。键还是按物理位置（KeyboardEvent.code）走，
//! 换出来的也是对方键盘上的物理键，对方按它自己的布局出字。

use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use crate::screen::{InputEvent, MouseButton};

/// 键盘是哪种习惯
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyboardKind {
    /// ⌘ 是命令键
    Mac,
    /// Ctrl 是命令键（Windows、Linux）
    Pc,
}

impl KeyboardKind {
    /// 本机（被连方）是哪种
    pub fn local() -> Self {
        if cfg!(target_os = "macos") { KeyboardKind::Mac } else { KeyboardKind::Pc }
    }
}

/// 观看方按着的修饰键（按它自己键盘上的叫法）
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Held {
    shift: bool,
    ctrl: bool,
    alt: bool,
    /// Mac 的 ⌘、PC 的 Win
    meta: bool,
}

/// 对方（被连方）键盘上的修饰键，发出去的就是这几个键名
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum HostMod {
    Shift,
    Ctrl,
    Alt,
    Meta,
}

impl HostMod {
    fn code(self) -> &'static str {
        match self {
            HostMod::Shift => "ShiftLeft",
            HostMod::Ctrl => "ControlLeft",
            HostMod::Alt => "AltLeft",
            HostMod::Meta => "MetaLeft",
        }
    }
}

type Mods = BTreeSet<HostMod>;

fn modifier_of(code: &str) -> Option<fn(&mut Held) -> &mut bool> {
    Some(match code {
        "ShiftLeft" | "ShiftRight" => |h| &mut h.shift,
        "ControlLeft" | "ControlRight" => |h| &mut h.ctrl,
        "AltLeft" | "AltRight" => |h| &mut h.alt,
        "MetaLeft" | "MetaRight" => |h| &mut h.meta,
        _ => return None,
    })
}

/// 移动光标、删字的键：按“文字里移动”的规则换
fn is_nav(code: &str) -> bool {
    matches!(code, "ArrowLeft" | "ArrowRight" | "ArrowUp" | "ArrowDown" | "Backspace" | "Delete" | "Home" | "End")
}

/// 一个普通键换成什么：对方按的键，和按的时候对方要按着哪些修饰键
struct Chord {
    code: String,
    mods: Mods,
}

pub struct KeyTranslator {
    from: KeyboardKind,
    to: KeyboardKind,
    enabled: bool,
    held: Held,
    /// 对方现在实际按着的修饰键（我们发过去的）
    down: Mods,
    /// 观看方按下的键 → 发给对方的键：抬起时抬同一个
    keys: HashMap<String, String>,
    /// 按下的鼠标键 → 发给对方的鼠标键（⌃+左键会变成右键）
    buttons: HashMap<MouseButtonKey, MouseButton>,
    /// 只按了一个修饰键、还没按别的：松开时可能要当成“单按一下”
    tap: Option<String>,
}

type MouseButtonKey = u8;

fn button_key(b: MouseButton) -> MouseButtonKey {
    b as u8
}

impl KeyTranslator {
    /// from：观看方的键盘；to：被连方。两边一样时什么都不换
    pub fn new(from: KeyboardKind, to: KeyboardKind, enabled: bool) -> Self {
        KeyTranslator {
            from,
            to,
            enabled,
            held: Held::default(),
            down: Mods::new(),
            keys: HashMap::new(),
            buttons: HashMap::new(),
            tap: None,
        }
    }

    fn active(&self) -> bool {
        self.enabled && self.from != self.to
    }

    /// 开关转换。切换前先把对方按着的全松开，免得按着的键对不上
    pub fn set_enabled(&mut self, on: bool) -> Vec<InputEvent> {
        if on == self.enabled {
            return Vec::new();
        }
        self.enabled = on;
        self.reset();
        vec![InputEvent::ReleaseAll]
    }

    fn reset(&mut self) {
        self.held = Held::default();
        self.down.clear();
        self.keys.clear();
        self.buttons.clear();
        self.tap = None;
    }

    /// 一个修饰键按下就马上发的那几个（一对一、不会因为后面按什么而变）
    fn immediate(&self) -> Mods {
        let h = self.held;
        let mut m = Mods::new();
        if h.shift {
            m.insert(HostMod::Shift);
        }
        match (self.from, self.to) {
            // Mac 的 ⌃ 在 Windows 上就是 Ctrl
            (KeyboardKind::Mac, KeyboardKind::Pc) => {
                if h.ctrl {
                    m.insert(HostMod::Ctrl);
                }
            }
            // Windows 的 Alt → ⌥，Win → ⌃
            (KeyboardKind::Pc, KeyboardKind::Mac) => {
                if h.alt {
                    m.insert(HostMod::Alt);
                }
                if h.meta {
                    m.insert(HostMod::Ctrl);
                }
            }
            _ => {}
        }
        m
    }

    /// 普通键（字母、数字、F 键……）和鼠标按着这些修饰键时，对方要按着哪些
    fn general(&self, terminal: bool) -> Mods {
        let h = self.held;
        let mut m = self.immediate();
        match (self.from, self.to) {
            (KeyboardKind::Mac, KeyboardKind::Pc) => {
                if h.meta {
                    m.insert(HostMod::Ctrl);
                }
                if h.alt {
                    m.insert(HostMod::Alt);
                }
            }
            (KeyboardKind::Pc, KeyboardKind::Mac) if h.ctrl => {
                if !terminal {
                    m.insert(HostMod::Meta);
                } else if h.shift {
                    // 终端里 Ctrl+Shift+字母是复制、粘贴这类：发 ⌘+字母，不带 Shift
                    m.remove(&HostMod::Shift);
                    m.insert(HostMod::Meta);
                } else {
                    m.insert(HostMod::Ctrl);
                }
            }
            _ => {}
        }
        m
    }

    fn chord(&self, code: &str, terminal: bool) -> Chord {
        let h = self.held;
        let plain = |code: &str, mods: Mods| Chord { code: code.to_string(), mods };
        match (self.from, self.to) {
            (KeyboardKind::Mac, KeyboardKind::Pc) => {
                if is_nav(code) && h.meta {
                    // ⌘←/→ 行首行尾，⌘↑/↓ 开头结尾，⌘⌫/⌦ 按词删（Windows 没有删到行首的键）
                    let mut mods = self.immediate();
                    let (target, ctrl) = match code {
                        "ArrowLeft" => ("Home", false),
                        "ArrowRight" => ("End", false),
                        "ArrowUp" => ("Home", true),
                        "ArrowDown" => ("End", true),
                        other => (other, true),
                    };
                    if ctrl || h.ctrl {
                        mods.insert(HostMod::Ctrl);
                    }
                    if h.alt {
                        mods.insert(HostMod::Alt);
                    }
                    return plain(target, mods);
                }
                if is_nav(code) && h.alt {
                    // ⌥ 按词移动、按词删 → Ctrl
                    let mut mods = self.immediate();
                    mods.insert(HostMod::Ctrl);
                    return plain(code, mods);
                }
                // ⌘Q 退出 → Alt+F4（只按着 ⌘ 时）
                if code == "KeyQ" && h.meta && !h.ctrl && !h.alt && !h.shift {
                    return plain("F4", Mods::from([HostMod::Alt]));
                }
                plain(code, self.general(terminal))
            }
            (KeyboardKind::Pc, KeyboardKind::Mac) => {
                if is_nav(code) && !terminal {
                    let mut mods = self.immediate();
                    // Alt 在这些键上（浏览器的前进后退）照常带着
                    match code {
                        // Home/End：行首行尾；Ctrl+Home/End：开头结尾
                        "Home" | "End" => {
                            mods.insert(HostMod::Meta);
                            let target = match (code, h.ctrl) {
                                ("Home", false) => "ArrowLeft",
                                ("End", false) => "ArrowRight",
                                ("Home", true) => "ArrowUp",
                                _ => "ArrowDown",
                            };
                            return plain(target, mods);
                        }
                        // Ctrl+方向、退格、删除：按词 → ⌥
                        _ if h.ctrl => {
                            mods.insert(HostMod::Alt);
                            return plain(code, mods);
                        }
                        _ => return plain(code, mods),
                    }
                }
                // Alt+F4 关窗口 → ⌘W
                if code == "F4" && h.alt && !h.ctrl && !h.meta && !h.shift {
                    return plain("KeyW", Mods::from([HostMod::Meta]));
                }
                plain(code, self.general(terminal))
            }
            _ => plain(code, self.general(terminal)),
        }
    }

    /// 让对方实际按着的修饰键变成 want：多的松开、少的按下
    fn sync(&mut self, want: &Mods, out: &mut Vec<InputEvent>) {
        let release: Vec<HostMod> = self.down.difference(want).copied().collect();
        let press: Vec<HostMod> = want.difference(&self.down).copied().collect();
        for m in release {
            out.push(InputEvent::Key { code: m.code().into(), down: false });
            self.down.remove(&m);
        }
        for m in press {
            out.push(InputEvent::Key { code: m.code().into(), down: true });
            self.down.insert(m);
        }
    }

    /// 修饰键变了以后：还“说得通”的留着（还按着的修饰键能推出来的），一对一的马上补上
    fn settle(&mut self, out: &mut Vec<InputEvent>) {
        let general = self.general(false);
        let general_terminal = self.general(true);
        let mut want: Mods = self.down.iter().filter(|m| general.contains(m) || general_terminal.contains(m)).copied().collect();
        want.extend(self.immediate());
        self.sync(&want, out);
    }

    /// 观看方发来的一个事件换成发给对方的。terminal：问一下对方前台是不是终端（只在要用时才问）
    pub fn feed(&mut self, ev: InputEvent, terminal: impl FnOnce() -> bool) -> Vec<InputEvent> {
        if !self.active() {
            return vec![ev];
        }
        let mut out = Vec::new();
        match ev {
            InputEvent::Key { code, down } => {
                if let Some(slot) = modifier_of(&code) {
                    let was = *slot(&mut self.held);
                    *slot(&mut self.held) = down;
                    if down {
                        // 连按（自动重复）时不算新的单按
                        if !was {
                            self.tap = Some(code.clone());
                        }
                    } else if self.tap.as_deref() == Some(code.as_str()) {
                        self.tap = None;
                        // Mac 上单按一下 ⌘ → Windows 上单按一下 Win（开始菜单）
                        if self.from == KeyboardKind::Mac && self.to == KeyboardKind::Pc && code.starts_with("Meta") {
                            self.settle(&mut out);
                            out.push(InputEvent::Key { code: "MetaLeft".into(), down: true });
                            out.push(InputEvent::Key { code: "MetaLeft".into(), down: false });
                            return out;
                        }
                    }
                    self.settle(&mut out);
                    return out;
                }
                if down {
                    self.tap = None;
                    // 只有换到 Mac、按着 Ctrl 或者按的是移动光标的键时，结果才和终端有关，这时才问
                    let terminal = self.from == KeyboardKind::Pc
                        && self.to == KeyboardKind::Mac
                        && (self.held.ctrl || is_nav(&code))
                        && terminal();
                    let chord = self.chord(&code, terminal);
                    self.sync(&chord.mods, &mut out);
                    // 换了目标键还按着原来的（比如按着 ← 时松开了 ⌘）：先抬起旧的
                    if let Some(old) = self.keys.insert(code, chord.code.clone())
                        && old != chord.code
                    {
                        out.push(InputEvent::Key { code: old, down: false });
                    }
                    out.push(InputEvent::Key { code: chord.code, down: true });
                } else {
                    let target = self.keys.remove(&code).unwrap_or(code);
                    out.push(InputEvent::Key { code: target, down: false });
                    // 为这个键临时加的修饰键（Home → ⌘←、⌘Q → Alt+F4）松开
                    self.settle(&mut out);
                }
            }
            InputEvent::Button { button, down, x, y } => {
                let button = self.mouse(button, down, &mut out);
                out.push(InputEvent::Button { button, down, x, y });
            }
            InputEvent::ButtonHere { button, down } => {
                let button = self.mouse(button, down, &mut out);
                out.push(InputEvent::ButtonHere { button, down });
            }
            InputEvent::Wheel { .. } | InputEvent::WheelHere { .. } => {
                self.tap = None;
                let want = self.general(false);
                self.sync(&want, &mut out);
                out.push(ev);
            }
            InputEvent::ReleaseAll => {
                self.reset();
                out.push(InputEvent::ReleaseAll);
            }
            other => out.push(other),
        }
        out
    }

    /// 点鼠标时按着修饰键：⌘ 点击 → Ctrl 点击；Mac 的 ⌃+左键是右键
    fn mouse(&mut self, button: MouseButton, down: bool, out: &mut Vec<InputEvent>) -> MouseButton {
        if !down {
            return self.buttons.remove(&button_key(button)).unwrap_or(button);
        }
        self.tap = None;
        let mut want = self.general(false);
        let mut target = button;
        if self.from == KeyboardKind::Mac && self.to == KeyboardKind::Pc && button == MouseButton::Left && self.held.ctrl {
            target = MouseButton::Right;
            want.remove(&HostMod::Ctrl);
        }
        self.sync(&want, out);
        self.buttons.insert(button_key(button), target);
        target
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use KeyboardKind::{Mac, Pc};

    fn key(code: &str, down: bool) -> InputEvent {
        InputEvent::Key { code: code.into(), down }
    }

    /// 按顺序喂一串键（"+X" 按下、"-X" 抬起），收集发给对方的
    fn run(t: &mut KeyTranslator, keys: &[&str], terminal: bool) -> Vec<String> {
        let mut out = Vec::new();
        for k in keys {
            let (down, code) = (k.starts_with('+'), &k[1..]);
            for ev in t.feed(key(code, down), || terminal) {
                match ev {
                    InputEvent::Key { code, down } => out.push(format!("{}{code}", if down { '+' } else { '-' })),
                    other => out.push(format!("{other:?}")),
                }
            }
        }
        out
    }

    #[test]
    fn mac_command_becomes_ctrl() {
        let mut t = KeyTranslator::new(Mac, Pc, true);
        // ⌘ 先不发，按 C 时才按 Ctrl；⌘ 松开时 Ctrl 跟着松开
        assert_eq!(run(&mut t, &["+MetaLeft", "+KeyC", "-KeyC", "-MetaLeft"], false), ["+ControlLeft", "+KeyC", "-KeyC", "-ControlLeft"]);
        // ⌘⇧T：Shift 照常马上发
        assert_eq!(
            run(&mut t, &["+MetaLeft", "+ShiftLeft", "+KeyT", "-KeyT", "-ShiftLeft", "-MetaLeft"], false),
            ["+ShiftLeft", "+ControlLeft", "+KeyT", "-KeyT", "-ShiftLeft", "-ControlLeft"]
        );
    }

    #[test]
    fn mac_control_stays_ctrl() {
        // 终端里 ⌃C：马上发 Ctrl，不是 Win
        let mut t = KeyTranslator::new(Mac, Pc, true);
        assert_eq!(run(&mut t, &["+ControlLeft", "+KeyC", "-KeyC", "-ControlLeft"], false), ["+ControlLeft", "+KeyC", "-KeyC", "-ControlLeft"]);
    }

    #[test]
    fn mac_text_navigation() {
        let mut t = KeyTranslator::new(Mac, Pc, true);
        assert_eq!(run(&mut t, &["+MetaLeft", "+ArrowLeft", "-ArrowLeft", "-MetaLeft"], false), ["+Home", "-Home"]);
        assert_eq!(run(&mut t, &["+MetaLeft", "+ArrowDown", "-ArrowDown", "-MetaLeft"], false), ["+ControlLeft", "+End", "-End", "-ControlLeft"]);
        // ⌘⇧→ 选到行尾
        assert_eq!(
            run(&mut t, &["+ShiftLeft", "+MetaLeft", "+ArrowRight", "-ArrowRight", "-MetaLeft", "-ShiftLeft"], false),
            ["+ShiftLeft", "+End", "-End", "-ShiftLeft"]
        );
        // ⌥← 按词 → Ctrl+←，不带 Alt
        assert_eq!(run(&mut t, &["+AltLeft", "+ArrowLeft", "-ArrowLeft", "-AltLeft"], false), ["+ControlLeft", "+ArrowLeft", "-ArrowLeft", "-ControlLeft"]);
        // ⌘C 接着 ⌘←：Home 之前先松开 Ctrl
        assert_eq!(
            run(&mut t, &["+MetaLeft", "+KeyC", "-KeyC", "+ArrowLeft", "-ArrowLeft", "-MetaLeft"], false),
            ["+ControlLeft", "+KeyC", "-KeyC", "-ControlLeft", "+Home", "-Home"]
        );
    }

    #[test]
    fn mac_quit_and_start_menu() {
        let mut t = KeyTranslator::new(Mac, Pc, true);
        assert_eq!(run(&mut t, &["+MetaLeft", "+KeyQ", "-KeyQ", "-MetaLeft"], false), ["+AltLeft", "+F4", "-F4", "-AltLeft"]);
        // 单按一下 ⌘ → Win
        assert_eq!(run(&mut t, &["+MetaLeft", "-MetaLeft"], false), ["+MetaLeft", "-MetaLeft"]);
        // 自动重复的按下不算新的单按，按了别的键也不算
        assert_eq!(run(&mut t, &["+MetaLeft", "+MetaLeft", "+KeyA", "-KeyA", "-MetaLeft"], false), ["+ControlLeft", "+KeyA", "-KeyA", "-ControlLeft"]);
    }

    #[test]
    fn mac_control_click_is_right_click() {
        let mut t = KeyTranslator::new(Mac, Pc, true);
        t.feed(key("ControlLeft", true), || false);
        let out = t.feed(InputEvent::Button { button: MouseButton::Left, down: true, x: 0.5, y: 0.5 }, || false);
        assert_eq!(out, [key("ControlLeft", false), InputEvent::Button { button: MouseButton::Right, down: true, x: 0.5, y: 0.5 }]);
        let out = t.feed(InputEvent::Button { button: MouseButton::Left, down: false, x: 0.5, y: 0.5 }, || false);
        assert_eq!(out, [InputEvent::Button { button: MouseButton::Right, down: false, x: 0.5, y: 0.5 }]);
        // ⌘ 点击 → Ctrl 点击（多选、新标签打开）
        let mut t = KeyTranslator::new(Mac, Pc, true);
        t.feed(key("MetaLeft", true), || false);
        let out = t.feed(InputEvent::ButtonHere { button: MouseButton::Left, down: true }, || false);
        assert_eq!(out, [key("ControlLeft", true), InputEvent::ButtonHere { button: MouseButton::Left, down: true }]);
    }

    #[test]
    fn pc_to_mac() {
        let mut t = KeyTranslator::new(Pc, Mac, true);
        assert_eq!(run(&mut t, &["+ControlLeft", "+KeyC", "-KeyC", "-ControlLeft"], false), ["+MetaLeft", "+KeyC", "-KeyC", "-MetaLeft"]);
        assert_eq!(run(&mut t, &["+Home", "-Home"], false), ["+MetaLeft", "+ArrowLeft", "-ArrowLeft", "-MetaLeft"]);
        assert_eq!(run(&mut t, &["+ControlLeft", "+End", "-End", "-ControlLeft"], false), ["+MetaLeft", "+ArrowDown", "-ArrowDown", "-MetaLeft"]);
        assert_eq!(run(&mut t, &["+ControlLeft", "+Backspace", "-Backspace", "-ControlLeft"], false), ["+AltLeft", "+Backspace", "-Backspace", "-AltLeft"]);
        // Win → ⌃，马上发
        assert_eq!(run(&mut t, &["+MetaLeft", "+KeyA", "-KeyA", "-MetaLeft"], false), ["+ControlLeft", "+KeyA", "-KeyA", "-ControlLeft"]);
        // Alt 一按就发；Alt+F4 时先松开、换成 ⌘W，松开 W 以后 Alt 还按着就补回去
        assert_eq!(
            run(&mut t, &["+AltLeft", "+F4", "-F4", "-AltLeft"], false),
            ["+AltLeft", "-AltLeft", "+MetaLeft", "+KeyW", "-KeyW", "-MetaLeft", "+AltLeft", "-AltLeft"]
        );
    }

    #[test]
    fn pc_to_mac_terminal() {
        let mut t = KeyTranslator::new(Pc, Mac, true);
        // 终端里 Ctrl+C 是中断：⌃C
        assert_eq!(run(&mut t, &["+ControlLeft", "+KeyC", "-KeyC", "-ControlLeft"], true), ["+ControlLeft", "+KeyC", "-KeyC", "-ControlLeft"]);
        // Ctrl+Shift+V 是粘贴：⌘V，不带 Shift
        assert_eq!(
            run(&mut t, &["+ControlLeft", "+ShiftLeft", "+KeyV", "-KeyV", "-ShiftLeft", "-ControlLeft"], true),
            ["+ShiftLeft", "-ShiftLeft", "+MetaLeft", "+KeyV", "-KeyV", "+ShiftLeft", "-ShiftLeft", "-MetaLeft"]
        );
        // 终端里 Home/End 不换成 ⌘←/⌘→（终端里那是切标签）
        assert_eq!(run(&mut t, &["+Home", "-Home"], true), ["+Home", "-Home"]);
        // 不按 Ctrl 时不问前台是不是终端
        let mut asked = false;
        t.feed(key("KeyA", true), || {
            asked = true;
            true
        });
        assert!(!asked);
    }

    #[test]
    fn same_system_or_disabled_passes_through() {
        let mut t = KeyTranslator::new(Mac, Mac, true);
        assert_eq!(run(&mut t, &["+MetaLeft", "+KeyC", "-KeyC", "-MetaLeft"], false), ["+MetaLeft", "+KeyC", "-KeyC", "-MetaLeft"]);
        let mut t = KeyTranslator::new(Mac, Pc, false);
        assert_eq!(run(&mut t, &["+MetaLeft", "+KeyC", "-KeyC", "-MetaLeft"], false), ["+MetaLeft", "+KeyC", "-KeyC", "-MetaLeft"]);
        // 中途打开：先全松开
        assert_eq!(t.set_enabled(true), [InputEvent::ReleaseAll]);
        assert!(t.set_enabled(true).is_empty());
    }
}
