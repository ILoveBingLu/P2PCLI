//! Win32 caption controls rendered above WebView2.
//!
//! A per-pixel-alpha child HWND owns all three caption buttons. The maximize
//! region returns `HTMAXBUTTON`, preserving the Windows 11 Snap Layout flyout,
//! while rendering and input never enter the DOM.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
    time::Instant,
};

use tauri::{Runtime, Theme, WebviewWindow};
use windows_sys::Win32::{
    Foundation::{GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM},
    Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, DrawTextW,
        GetTextFaceW, SelectObject, SetBkColor, SetBkMode, SetTextColor, AC_SRC_ALPHA, AC_SRC_OVER,
        ANTIALIASED_QUALITY, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION,
        CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DIB_RGB_COLORS, DT_CENTER,
        DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, FW_NORMAL, OPAQUE, OUT_DEFAULT_PRECIS,
    },
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        HiDpi::GetDpiForWindow,
        Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT},
        Shell::{DefSubclassProc, GetWindowSubclass, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect,
            GetMenuState, GetParent, GetSystemMenu, GetWindowLongW, GetWindowRect, IsZoomed,
            KillTimer, PostMessageW, RegisterClassExW, SetTimer, SetWindowPos, ShowWindow,
            UpdateLayeredWindow, CS_HREDRAW, CS_VREDRAW, GWL_STYLE, HTCLOSE, HTMAXBUTTON,
            HTMINBUTTON, HWND_TOP, MF_DISABLED, MF_GRAYED, SC_CLOSE, SC_MAXIMIZE, SC_MINIMIZE,
            SC_RESTORE, SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_HIDE, ULW_ALPHA,
            WM_DPICHANGED, WM_NCDESTROY, WM_NCHITTEST, WM_NCLBUTTONDOWN, WM_NCLBUTTONUP,
            WM_NCMOUSELEAVE, WM_NCMOUSEMOVE, WM_SETTINGCHANGE, WM_SIZE, WM_STYLECHANGED,
            WM_SYSCOMMAND, WM_THEMECHANGED, WM_TIMER, WM_WINDOWPOSCHANGED, WNDCLASSEXW, WS_CAPTION,
            WS_CHILD, WS_CLIPSIBLINGS, WS_EX_LAYERED, WS_EX_TRANSPARENT, WS_MAXIMIZEBOX,
            WS_MINIMIZEBOX, WS_VISIBLE,
        },
    },
};

const BUTTON_WIDTH: u32 = 46;
const BUTTON_COUNT: u32 = 3;
const SUBCLASS_ID: usize = 0x7462_6f63;
const ANIMATION_TIMER_ID: usize = 0x7462_6f61;
const ANIMATION_INTERVAL_MS: u32 = 16;
const BACKGROUND_TRANSITION_MS: f32 = 120.0;
const COLOR_TRANSITION_MS: f32 = 80.0;

/// `TauriWindowControls\0` as UTF-16.
const CLASS: &[u16] = &[
    0x54, 0x61, 0x75, 0x72, 0x69, 0x57, 0x69, 0x6e, 0x64, 0x6f, 0x77, 0x43, 0x6f, 0x6e, 0x74, 0x72,
    0x6f, 0x6c, 0x73, 0x00,
];

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
enum CaptionButton {
    Minimize,
    Maximize,
    Close,
}

impl CaptionButton {
    fn index(self) -> usize {
        self as usize
    }
}

const BUTTONS: [CaptionButton; 3] = [
    CaptionButton::Minimize,
    CaptionButton::Maximize,
    CaptionButton::Close,
];

#[derive(Clone, Copy)]
struct CaptionState {
    visual: HWND,
    input: HWND,
    height: u32,
    dark: bool,
    hovering: Option<CaptionButton>,
    pressing: Option<CaptionButton>,
    background_progress: [f32; 3],
    color_progress: [f32; 3],
    last_animation_tick: Instant,
}

static GLYPH_MASKS: OnceLock<Mutex<GlyphMaskCache>> = OnceLock::new();

type GlyphMask = Arc<[u8]>;
type GlyphMaskCache = HashMap<GlyphMaskKey, Option<GlyphMask>>;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GlyphMaskKey {
    width: i32,
    height: i32,
    dpi: u32,
    glyph: char,
}

#[derive(Clone, Copy)]
struct Geometry {
    dpi: u32,
    x: i32,
    width: i32,
    height: i32,
}

struct Canvas<'a> {
    pixels: &'a mut [u8],
    width: i32,
    height: i32,
}

impl Canvas<'_> {
    fn fill(&mut self, x: i32, y: i32, width: i32, height: i32, color: [u8; 4]) {
        for py in y.max(0)..(y + height).min(self.height) {
            for px in x.max(0)..(x + width).min(self.width) {
                self.blend(px, py, color, 1.0);
            }
        }
    }

    fn glyph(&mut self, x: i32, mask: &[u8], color: [u8; 4]) {
        let glyph_width = self.width / BUTTON_COUNT as i32;
        for py in 0..self.height {
            for px in 0..glyph_width {
                let coverage = mask[(py * glyph_width + px) as usize] as f32 / 255.0;
                if coverage > 0.0 {
                    self.blend(x + px, py, color, coverage);
                }
            }
        }
    }

    fn blend(&mut self, x: i32, y: i32, color: [u8; 4], coverage: f32) {
        let index = ((y * self.width + x) * 4) as usize;
        let alpha = (color[3] as f32 * coverage).round() as u16;
        let inverse = 255 - alpha;
        for (channel, value) in color[..3].iter().copied().enumerate() {
            let source = value as u16 * alpha / 255;
            let destination = self.pixels[index + 2 - channel] as u16 * inverse / 255;
            self.pixels[index + 2 - channel] = (source + destination).min(255) as u8;
        }
        let destination_alpha = self.pixels[index + 3] as u16 * inverse / 255;
        self.pixels[index + 3] = (alpha + destination_alpha).min(255) as u8;
    }
}

unsafe fn caption_state(parent: HWND) -> Option<*mut CaptionState> {
    let mut data = 0;
    if GetWindowSubclass(parent, Some(parent_subclass_proc), SUBCLASS_ID, &mut data) == 0
        || data == 0
    {
        None
    } else {
        Some(data as *mut CaptionState)
    }
}

pub(crate) fn install<R: Runtime>(
    window: &WebviewWindow<R>,
    titlebar_height: u32,
) -> tauri::Result<()> {
    let hwnd = window.hwnd()?.0 as isize;
    let height = titlebar_height.max(1);
    let dark = matches!(window.theme()?, Theme::Dark);

    window.run_on_main_thread(move || unsafe {
        install_hwnd(hwnd as HWND, height, dark);
    })?;
    Ok(())
}

pub(crate) fn set_theme<R: Runtime>(window: &WebviewWindow<R>, theme: Theme) -> tauri::Result<()> {
    let hwnd = window.hwnd()?.0 as isize;
    window.run_on_main_thread(move || unsafe {
        if let Some(state) = caption_state(hwnd as HWND) {
            (*state).dark = matches!(theme, Theme::Dark);
            repaint(hwnd as HWND);
        }
    })?;
    Ok(())
}

unsafe fn install_hwnd(hwnd: HWND, height: u32, dark: bool) {
    if caption_state(hwnd).is_some() {
        return;
    }
    register_class();

    let input = CreateWindowExW(
        0,
        CLASS.as_ptr(),
        CLASS.as_ptr(),
        WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS,
        0,
        0,
        0,
        0,
        hwnd,
        std::ptr::null_mut(),
        module_instance(),
        std::ptr::null(),
    );
    if input.is_null() {
        eprintln!(
            "window-controls: input CreateWindowExW failed: {}",
            GetLastError()
        );
        return;
    }

    let visual = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TRANSPARENT,
        CLASS.as_ptr(),
        CLASS.as_ptr(),
        WS_CHILD | WS_CLIPSIBLINGS,
        0,
        0,
        0,
        0,
        hwnd,
        std::ptr::null_mut(),
        module_instance(),
        std::ptr::null(),
    );
    if visual.is_null() {
        eprintln!(
            "window-controls: visual CreateWindowExW failed: {}",
            GetLastError()
        );
        DestroyWindow(input);
        return;
    }

    let state = Box::new(CaptionState {
        visual,
        input,
        height,
        dark,
        hovering: None,
        pressing: None,
        background_progress: [0.0; 3],
        color_progress: [0.0; 3],
        last_animation_tick: Instant::now(),
    });
    let state = Box::into_raw(state);
    if SetWindowSubclass(
        hwnd,
        Some(parent_subclass_proc),
        SUBCLASS_ID,
        state as usize,
    ) == 0
    {
        drop(Box::from_raw(state));
        DestroyWindow(input);
        DestroyWindow(visual);
        eprintln!("window-controls: SetWindowSubclass failed");
        return;
    }
    update_position(hwnd);
    repaint(hwnd);
}

unsafe fn register_class() {
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(controls_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: module_instance(),
        hIcon: std::ptr::null_mut(),
        hCursor: std::ptr::null_mut(),
        hbrBackground: std::ptr::null_mut(),
        lpszMenuName: std::ptr::null(),
        lpszClassName: CLASS.as_ptr(),
        hIconSm: std::ptr::null_mut(),
    };
    if RegisterClassExW(&class) == 0 {
        let error = GetLastError();
        // ERROR_CLASS_ALREADY_EXISTS is harmless on repeated installations.
        if error != 1410 {
            eprintln!("window-controls: RegisterClassExW failed: {error}");
        }
    }
}

unsafe fn module_instance() -> HINSTANCE {
    GetModuleHandleW(std::ptr::null()) as HINSTANCE
}

unsafe fn update_position(hwnd: HWND) {
    let Some(state) = caption_state(hwnd).map(|state| *state) else {
        return;
    };

    if !has_caption_style(hwnd) {
        ShowWindow(state.input, SW_HIDE);
        ShowWindow(state.visual, SW_HIDE);
        return;
    }

    let Some(geometry) = geometry(hwnd, state.height) else {
        return;
    };

    for child in [state.input, state.visual] {
        SetWindowPos(
            child,
            HWND_TOP,
            geometry.x,
            0,
            geometry.width,
            geometry.height,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
}

unsafe fn repaint(parent: HWND) {
    let Some(state) = caption_state(parent).map(|state| *state) else {
        return;
    };
    if !has_caption_style(parent) {
        ShowWindow(state.input, SW_HIDE);
        ShowWindow(state.visual, SW_HIDE);
        return;
    }
    let Some(geometry) = geometry(parent, state.height) else {
        return;
    };
    let mut pixels = vec![0u8; (geometry.width * geometry.height * 4) as usize];
    render(&mut pixels, geometry, parent, &state);
    update_layered_bitmap(
        state.visual,
        POINT {
            x: geometry.x,
            y: 0,
        },
        geometry.width,
        geometry.height,
        &pixels,
    );
}

unsafe fn geometry(parent: HWND, logical_height: u32) -> Option<Geometry> {
    let mut client = std::mem::zeroed();
    if GetClientRect(parent, &mut client) == 0 {
        return None;
    }
    let dpi = GetDpiForWindow(parent);
    let width = scale(BUTTON_WIDTH, dpi).max(1) * BUTTON_COUNT as i32;
    Some(Geometry {
        dpi,
        x: client.right - width,
        width,
        height: scale(logical_height, dpi).max(1),
    })
}

unsafe fn update_layered_bitmap(
    hwnd: HWND,
    destination: POINT,
    width: i32,
    height: i32,
    pixels: &[u8],
) {
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    let bitmap = CreateDIBSection(
        std::ptr::null_mut(),
        &info,
        DIB_RGB_COLORS,
        &mut bits,
        std::ptr::null_mut(),
        0,
    );
    if bitmap.is_null() || bits.is_null() {
        return;
    }
    std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast(), pixels.len());

    let dc = CreateCompatibleDC(std::ptr::null_mut());
    if dc.is_null() {
        DeleteObject(bitmap);
        return;
    }
    let old = SelectObject(dc, bitmap);
    let size = SIZE {
        cx: width,
        cy: height,
    };
    let source = POINT { x: 0, y: 0 };
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    if UpdateLayeredWindow(
        hwnd,
        std::ptr::null_mut(),
        &destination,
        &size,
        dc,
        &source,
        0,
        &blend,
        ULW_ALPHA,
    ) == 0
    {
        eprintln!(
            "window-controls: UpdateLayeredWindow failed: {}",
            GetLastError()
        );
    }
    SelectObject(dc, old);
    DeleteDC(dc);
    DeleteObject(bitmap);
}

fn render(pixels: &mut [u8], geometry: Geometry, parent: HWND, state: &CaptionState) {
    let mut canvas = Canvas {
        pixels,
        width: geometry.width,
        height: geometry.height,
    };
    canvas.fill(0, 0, geometry.width, geometry.height, [0, 0, 0, 1]);

    let button_width = geometry.width / BUTTON_COUNT as i32;
    let maximized = unsafe { IsZoomed(parent) != 0 };

    for (index, button) in BUTTONS.into_iter().enumerate() {
        let x = index as i32 * button_width;
        let enabled = unsafe { button_enabled(parent, button) };
        let hovered = state.hovering == Some(button) && enabled;
        let pressed = state.pressing == Some(button) && hovered;
        let background_progress = state.background_progress[button.index()];
        let color_progress = state.color_progress[button.index()];

        if button == CaptionButton::Close && (background_progress > 0.0 || pressed) {
            let alpha = if pressed {
                204
            } else {
                (255.0 * background_progress).round() as u8
            };
            canvas.fill(x, 0, button_width, geometry.height, [196, 43, 28, alpha]);
        } else if background_progress > 0.0 || pressed {
            let value = if state.dark { 255 } else { 0 };
            let alpha = if pressed {
                10
            } else {
                (15.0 * background_progress).round() as u8
            };
            canvas.fill(
                x,
                0,
                button_width,
                geometry.height,
                [value, value, value, alpha],
            );
        }

        let value = if state.dark {
            255
        } else if button == CaptionButton::Close {
            (255.0 * color_progress).round() as u8
        } else {
            0
        };
        // Enabled controls keep their contrast when the window loses focus.
        let alpha = if enabled { 255 } else { 92 };
        let color = [value, value, value, alpha];
        let glyph = match button {
            CaptionButton::Minimize => '\u{e921}',
            CaptionButton::Maximize if maximized => '\u{e923}',
            CaptionButton::Maximize => '\u{e922}',
            CaptionButton::Close => '\u{e8bb}',
        };
        if let Some(mask) =
            unsafe { glyph_mask(button_width, geometry.height, geometry.dpi, glyph) }
        {
            canvas.glyph(x, &mask, color);
        }
    }
}

unsafe fn glyph_mask(width: i32, height: i32, dpi: u32, glyph: char) -> Option<GlyphMask> {
    let key = GlyphMaskKey {
        width,
        height,
        dpi,
        glyph,
    };
    let mut masks = GLYPH_MASKS
        .get_or_init(Default::default)
        .lock()
        .expect("native glyph cache poisoned");
    masks
        .entry(key)
        .or_insert_with(|| rasterize_glyph_mask(width, height, dpi, glyph).map(Arc::from))
        .clone()
}

unsafe fn rasterize_glyph_mask(width: i32, height: i32, dpi: u32, glyph: char) -> Option<Vec<u8>> {
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    let bitmap = CreateDIBSection(
        std::ptr::null_mut(),
        &info,
        DIB_RGB_COLORS,
        &mut bits,
        std::ptr::null_mut(),
        0,
    );
    if bitmap.is_null() || bits.is_null() {
        return None;
    }
    let dc = CreateCompatibleDC(std::ptr::null_mut());
    if dc.is_null() {
        DeleteObject(bitmap);
        return None;
    }

    let old_bitmap = SelectObject(dc, bitmap);
    let font = create_caption_font(dc, dpi);
    if font.is_null() {
        SelectObject(dc, old_bitmap);
        DeleteDC(dc);
        DeleteObject(bitmap);
        return None;
    }
    let old_font = SelectObject(dc, font);
    SetBkMode(dc, OPAQUE as i32);
    SetBkColor(dc, 0);
    SetTextColor(dc, 0x00ff_ffff);

    let glyph_code = glyph as u32 as u16;
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: width,
        bottom: height,
    };
    let drawn = DrawTextW(
        dc,
        &glyph_code,
        1,
        &mut rect,
        DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    ) != 0;

    let result = if drawn {
        let mask = std::slice::from_raw_parts(bits.cast::<u8>(), (width * height * 4) as usize);
        // GDI has already antialiased the glyph. Squaring this coverage again
        // makes thin strokes look disabled, especially at fractional DPI.
        Some(
            mask.chunks_exact(4)
                .map(|pixel| pixel[0].max(pixel[1]).max(pixel[2]))
                .collect(),
        )
    } else {
        None
    };

    SelectObject(dc, old_font);
    DeleteObject(font);
    SelectObject(dc, old_bitmap);
    DeleteDC(dc);
    DeleteObject(bitmap);
    result
}

unsafe fn create_caption_font(
    dc: windows_sys::Win32::Graphics::Gdi::HDC,
    dpi: u32,
) -> windows_sys::Win32::Graphics::Gdi::HFONT {
    for face in ["Segoe Fluent Icons", "Segoe MDL2 Assets"] {
        let wide = face
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let font = CreateFontW(
            -scale(10, dpi),
            0,
            0,
            0,
            FW_NORMAL as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            ANTIALIASED_QUALITY as u32,
            DEFAULT_PITCH as u32,
            wide.as_ptr(),
        );
        if font.is_null() {
            continue;
        }

        let old = SelectObject(dc, font);
        let mut actual = [0u16; 64];
        let length = GetTextFaceW(dc, actual.len() as i32, actual.as_mut_ptr());
        SelectObject(dc, old);
        let actual = if length > 0 {
            String::from_utf16_lossy(&actual[..length as usize])
                .trim_end_matches('\0')
                .to_string()
        } else {
            String::new()
        };
        if actual.eq_ignore_ascii_case(face) {
            return font;
        }
        DeleteObject(font);
    }
    std::ptr::null_mut()
}

fn scale(value: u32, dpi: u32) -> i32 {
    ((value as u64 * dpi.max(96) as u64 + 48) / 96) as i32
}

fn approach(value: &mut f32, target: f32, step: f32) -> bool {
    let delta = target - *value;
    if delta.abs() <= step {
        *value = target;
        false
    } else {
        *value += step.copysign(delta);
        true
    }
}

unsafe fn start_animation(input: HWND) {
    if let Some(parent) = controls_parent(input) {
        (*caption_state(parent).unwrap()).last_animation_tick = Instant::now();
    }
    SetTimer(input, ANIMATION_TIMER_ID, ANIMATION_INTERVAL_MS, None);
}

unsafe fn tick_animation(input: HWND, parent: HWND) {
    let Some(state) = caption_state(parent) else {
        KillTimer(input, ANIMATION_TIMER_ID);
        return;
    };
    let hovered = (*state)
        .hovering
        .filter(|button| button_enabled(parent, *button));
    let now = Instant::now();
    let mut animating = false;
    let state = &mut *state;
    let elapsed_ms = now
        .saturating_duration_since(state.last_animation_tick)
        .as_secs_f32()
        * 1000.0;
    state.last_animation_tick = now;
    let background_step = elapsed_ms / BACKGROUND_TRANSITION_MS;
    let color_step = elapsed_ms / COLOR_TRANSITION_MS;
    for button in BUTTONS {
        let target = if hovered == Some(button) { 1.0 } else { 0.0 };
        animating |= approach(
            &mut state.background_progress[button.index()],
            target,
            background_step,
        );
        animating |= approach(
            &mut state.color_progress[button.index()],
            target,
            color_step,
        );
    }

    repaint(parent);
    if !animating {
        KillTimer(input, ANIMATION_TIMER_ID);
    }
}

unsafe fn has_caption_style(hwnd: HWND) -> bool {
    let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
    style & WS_CAPTION == WS_CAPTION
}

unsafe fn controls_parent(input: HWND) -> Option<HWND> {
    let parent = GetParent(input);
    (!parent.is_null() && caption_state(parent).is_some()).then_some(parent)
}

unsafe fn button_at(input: HWND, lparam: LPARAM) -> Option<CaptionButton> {
    let mut rect = std::mem::zeroed();
    if GetWindowRect(input, &mut rect) == 0 {
        return None;
    }
    let x = (lparam as u32 & 0xffff) as u16 as i16 as i32 - rect.left;
    let y = ((lparam as u32 >> 16) & 0xffff) as u16 as i16 as i32 - rect.top;
    if x < 0 || x >= rect.right - rect.left || y < 0 || y >= rect.bottom - rect.top {
        return None;
    }
    match x * 3 / (rect.right - rect.left) {
        0 => Some(CaptionButton::Minimize),
        1 => Some(CaptionButton::Maximize),
        _ => Some(CaptionButton::Close),
    }
}

unsafe fn button_enabled(parent: HWND, button: CaptionButton) -> bool {
    let style = GetWindowLongW(parent, GWL_STYLE) as u32;
    match button {
        CaptionButton::Minimize => style & WS_MINIMIZEBOX != 0,
        CaptionButton::Maximize => style & WS_MAXIMIZEBOX != 0,
        CaptionButton::Close => {
            let menu = GetSystemMenu(parent, 0);
            if menu.is_null() {
                return true;
            }
            let state = GetMenuState(menu, SC_CLOSE, 0);
            state == u32::MAX || state & (MF_DISABLED | MF_GRAYED) == 0
        }
    }
}

unsafe fn invoke(parent: HWND, button: CaptionButton) {
    let command = match button {
        CaptionButton::Minimize => SC_MINIMIZE,
        CaptionButton::Maximize if IsZoomed(parent) != 0 => SC_RESTORE,
        CaptionButton::Maximize => SC_MAXIMIZE,
        CaptionButton::Close => SC_CLOSE,
    };
    PostMessageW(parent, WM_SYSCOMMAND, command as WPARAM, 0);
}

unsafe extern "system" fn parent_subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    state: usize,
) -> LRESULT {
    if msg == WM_NCDESTROY {
        RemoveWindowSubclass(hwnd, Some(parent_subclass_proc), SUBCLASS_ID);
        if state != 0 {
            drop(Box::from_raw(state as *mut CaptionState));
        }
        return DefSubclassProc(hwnd, msg, wparam, lparam);
    }
    let result = DefSubclassProc(hwnd, msg, wparam, lparam);
    match msg {
        WM_SIZE | WM_DPICHANGED | WM_STYLECHANGED | WM_WINDOWPOSCHANGED => {
            update_position(hwnd);
            repaint(hwnd);
            if let Some(state) = caption_state(hwnd) {
                start_animation((*state).input);
            }
        }
        WM_SETTINGCHANGE | WM_THEMECHANGED => repaint(hwnd),
        _ => {}
    }
    result
}

unsafe extern "system" fn controls_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_TIMER if wparam == ANIMATION_TIMER_ID => {
            if let Some(parent) = controls_parent(hwnd) {
                tick_animation(hwnd, parent);
            } else {
                KillTimer(hwnd, ANIMATION_TIMER_ID);
            }
            return 0;
        }
        WM_NCHITTEST => {
            let Some(parent) = controls_parent(hwnd) else {
                return 0;
            };
            return match button_at(hwnd, lparam) {
                Some(CaptionButton::Minimize)
                    if button_enabled(parent, CaptionButton::Minimize) =>
                {
                    HTMINBUTTON as LRESULT
                }
                Some(CaptionButton::Maximize)
                    if button_enabled(parent, CaptionButton::Maximize) =>
                {
                    HTMAXBUTTON as LRESULT
                }
                Some(CaptionButton::Close) => HTCLOSE as LRESULT,
                _ => 0,
            };
        }
        WM_NCMOUSEMOVE => {
            if let Some(parent) = controls_parent(hwnd) {
                let button =
                    button_at(hwnd, lparam).filter(|button| button_enabled(parent, *button));
                let state = caption_state(parent).unwrap();
                let changed = (*state).hovering != button;
                (*state).hovering = button;
                if changed {
                    start_animation(hwnd);
                    repaint(parent);
                }
                let mut track = TRACKMOUSEEVENT {
                    cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE | TME_NONCLIENT,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                TrackMouseEvent(&mut track);
            }
            return 0;
        }
        WM_NCMOUSELEAVE => {
            if let Some(parent) = controls_parent(hwnd) {
                let state = caption_state(parent).unwrap();
                (*state).hovering = None;
                (*state).pressing = None;
                start_animation(hwnd);
                repaint(parent);
            }
            return 0;
        }
        WM_NCLBUTTONDOWN => {
            if let Some(parent) = controls_parent(hwnd) {
                let button =
                    button_at(hwnd, lparam).filter(|button| button_enabled(parent, *button));
                (*caption_state(parent).unwrap()).pressing = button;
                repaint(parent);
            }
            return 0;
        }
        WM_NCLBUTTONUP => {
            if let Some(parent) = controls_parent(hwnd) {
                let released = button_at(hwnd, lparam);
                let state = caption_state(parent).unwrap();
                let pressed = (*state).pressing.take();
                repaint(parent);
                if let Some(pressed) = pressed.filter(|pressed| Some(*pressed) == released) {
                    invoke(parent, pressed);
                }
            }
            return 0;
        }
        _ => {}
    }

    DefWindowProcW(hwnd, msg, wparam, lparam)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpi_scaling_keeps_each_button_integral() {
        for (dpi, expected) in [(96, 46), (120, 58), (144, 69), (168, 81), (192, 92)] {
            assert_eq!(scale(BUTTON_WIDTH, dpi), expected);
        }
    }
}
