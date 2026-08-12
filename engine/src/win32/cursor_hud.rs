//! Native Rust renderer for Swoosh's cursor-following gesture HUD.
//!
//! This tiny click-through window appears only while a valid window gesture is
//! armed.

use std::{ffi::c_void, mem, time::Instant};

use better_touch_advanced_gestures::{
    config::{AdvancedConfig, HudSize, HudTheme},
    geometry::SnapZone,
    gesture::MonitorDirection,
};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Gdi::{
        BeginPaint, CreatePen, CreateRoundRectRgn, CreateSolidBrush, DeleteObject, DrawTextW,
        EndPaint, FillRect, GetStockObject, InvalidateRect, RoundRect, SelectClipRgn, SelectObject,
        SetBkMode, SetTextColor, UpdateWindow, DT_CENTER, DT_END_ELLIPSIS, DT_SINGLELINE,
        DT_VCENTER, HDC, HOLLOW_BRUSH, PAINTSTRUCT, PS_SOLID, TRANSPARENT,
    },
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        HiDpi::GetDpiForWindow,
        WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetCursorPos,
            GetWindowLongPtrW, KillTimer, RegisterClassExW, SetLayeredWindowAttributes, SetTimer,
            SetWindowLongPtrW, SetWindowPos, ShowWindow, GWLP_USERDATA, HTTRANSPARENT,
            HWND_TOPMOST, LWA_ALPHA, LWA_COLORKEY, SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_HIDE,
            SW_SHOWNOACTIVATE, WM_ERASEBKGND, WM_NCHITTEST, WM_PAINT, WM_TIMER, WNDCLASSEXW,
            WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP,
        },
    },
};

use crate::swoosh_hud::{snap_fraction, HudGeometry};

const HUD_CLASS: &str = "ThreeFingerDragRust.SwooshCursorChip";
const TRANSPARENT_KEY: u32 = 0x0003_0201;
const FADE_TIMER: usize = 0x5348_01;
const MAX_DESKTOP_TILES: usize = 10;
const MAX_APP_TILES: usize = 5;

#[derive(Clone)]
enum HudContent {
    Snap(SnapZone),
    Fraction((f64, f64, f64, f64)),
    Desktop {
        count: usize,
        current: usize,
        target: Option<usize>,
        new_tile: Option<usize>,
    },
    Monitor {
        up: bool,
        down: bool,
        left: bool,
        right: bool,
        target: Option<MonitorDirection>,
    },
    Apps {
        titles: Vec<String>,
        selected: usize,
    },
    Chooser {
        choose: bool,
        close_picked: bool,
    },
}

struct HudState {
    content: HudContent,
    accent: u32,
    background: u32,
    edge: u32,
    text: u32,
    animate: bool,
    fade_ms: f64,
    fade_started: Option<Instant>,
    hud_size: HudSize,
    design_w: f64,
    design_h: f64,
    base_height: f64,
}

impl Default for HudState {
    fn default() -> Self {
        Self {
            content: HudContent::Snap(SnapZone::None),
            accent: rgb(10, 132, 255),
            background: rgb(18, 20, 26),
            edge: rgb(245, 245, 245),
            text: rgb(255, 255, 255),
            animate: true,
            fade_ms: 360.0,
            fade_started: None,
            hud_size: HudSize::Normal,
            design_w: 100.0,
            design_h: 66.0,
            base_height: HudGeometry::BASE_HEIGHT_PX,
        }
    }
}

pub struct CursorHud {
    hwnd: HWND,
    state: Box<HudState>,
}

impl CursorHud {
    pub fn new() -> Option<Self> {
        let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
        if instance.is_null() {
            return None;
        }
        let class_name = wide(HUD_CLASS);
        let class = WNDCLASSEXW {
            cbSize: mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(hud_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: std::ptr::null_mut(),
            hCursor: std::ptr::null_mut(),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class_name.as_ptr(),
            hIconSm: std::ptr::null_mut(),
        };
        unsafe {
            let _ = RegisterClassExW(&class);
        }
        let mut state = Box::<HudState>::default();
        let state_pointer = state.as_mut() as *mut HudState;
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class_name.as_ptr(),
                std::ptr::null(),
                WS_POPUP,
                -10_000,
                -10_000,
                1,
                1,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                state_pointer.cast::<c_void>(),
            )
        };
        if hwnd.is_null() {
            return None;
        }
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_pointer as isize);
            SetLayeredWindowAttributes(hwnd, TRANSPARENT_KEY, 255, LWA_COLORKEY | LWA_ALPHA);
            ShowWindow(hwnd, SW_HIDE);
        }
        Some(Self { hwnd, state })
    }

    pub fn handle(&self) -> HWND {
        self.hwnd
    }

    pub fn apply_config(&mut self, config: &AdvancedConfig) {
        self.state.animate = config.animate_snaps;
        self.state.fade_ms = config.hud_fade_out_seconds.clamp(0.1, 1.5) * 1000.0;
        self.state.hud_size = config.hud_size;
        let light = matches!(config.hud_background, HudTheme::Light)
            || (matches!(config.hud_background, HudTheme::System) && system_uses_light_theme());
        if light {
            self.state.background = rgb(244, 246, 249);
            self.state.edge = rgb(150, 156, 166);
            self.state.text = rgb(40, 44, 52);
        } else {
            self.state.background = rgb(18, 20, 26);
            self.state.edge = rgb(245, 245, 245);
            self.state.text = rgb(255, 255, 255);
        }
        self.state.accent = resolve_accent(config);
        unsafe { InvalidateRect(self.hwnd, std::ptr::null(), 0) };
    }

    pub fn show_snap(&mut self, zone: SnapZone) {
        self.show_snap_at(zone, None);
    }

    pub fn show_snap_at(&mut self, zone: SnapZone, cursor: Option<POINT>) {
        self.state.content = HudContent::Snap(zone);
        self.state.design_w = 100.0;
        self.state.design_h = 66.0;
        self.state.base_height = HudGeometry::BASE_HEIGHT_PX;
        self.show(cursor, 0.5);
    }

    pub fn show_fraction(&mut self, fraction: (f64, f64, f64, f64)) {
        self.state.content = HudContent::Fraction(fraction);
        self.state.design_w = 100.0;
        self.state.design_h = 66.0;
        self.state.base_height = HudGeometry::BASE_HEIGHT_PX;
        self.show(None, 0.5);
    }

    pub fn show_desktop_strip(
        &mut self,
        count: usize,
        current: usize,
        target: Option<usize>,
        new_tile: Option<usize>,
    ) {
        let count = count.clamp(1, MAX_DESKTOP_TILES);
        let current = current.min(count - 1);
        let chips_w = count as f64 * HudGeometry::DESKTOP_WIDTH
            + count.saturating_sub(1) as f64 * HudGeometry::DESKTOP_GAP;
        self.state.content = HudContent::Desktop {
            count,
            current,
            target: target.map(|value| value.min(count - 1)),
            new_tile: new_tile.filter(|value| *value < count),
        };
        self.state.design_w = chips_w + HudGeometry::MARGIN * 2.0;
        self.state.design_h = 66.0;
        self.state.base_height = HudGeometry::BASE_HEIGHT_PX;
        let anchor = (HudGeometry::MARGIN
            + current as f64 * (HudGeometry::DESKTOP_WIDTH + HudGeometry::DESKTOP_GAP)
            + HudGeometry::DESKTOP_WIDTH / 2.0)
            / self.state.design_w;
        self.show(None, anchor);
    }

    pub fn show_monitor_map(
        &mut self,
        up: bool,
        down: bool,
        left: bool,
        right: bool,
        target: Option<MonitorDirection>,
    ) {
        self.state.content = HudContent::Monitor {
            up,
            down,
            left,
            right,
            target,
        };
        self.state.design_w = HudGeometry::MAP_CELL_WIDTH * 3.0 + HudGeometry::MAP_GAP * 2.0;
        self.state.design_h = HudGeometry::MAP_CELL_HEIGHT * 3.0 + HudGeometry::MAP_GAP * 2.0;
        self.state.base_height = HudGeometry::MAP_BASE_HEIGHT_PX;
        self.show(None, 0.5);
    }

    pub fn show_app_strip(&mut self, titles: Vec<String>, selected: usize) {
        if titles.is_empty() {
            return;
        }
        let titles = titles.into_iter().take(MAX_APP_TILES).collect::<Vec<_>>();
        let count = titles.len();
        let tiles_w = count as f64 * HudGeometry::APP_TILE_WIDTH
            + count.saturating_sub(1) as f64 * HudGeometry::APP_TILE_GAP;
        self.state.content = HudContent::Apps {
            titles,
            selected: selected.min(count - 1),
        };
        self.state.design_w =
            tiles_w.max(HudGeometry::SINGLE_CHIP_WIDTH) + HudGeometry::MARGIN * 2.0;
        self.state.design_h = 99.0;
        self.state.base_height = HudGeometry::APP_BASE_HEIGHT_PX;
        self.show(None, 0.5);
    }

    pub fn show_down_chooser(&mut self, choose: bool, close_picked: bool, cursor: Option<POINT>) {
        let count = if choose { 2.0 } else { 1.0 };
        let circles_w = count * 56.0 + (count - 1.0) * 22.0;
        self.state.content = HudContent::Chooser {
            choose,
            close_picked,
        };
        self.state.design_w = HudGeometry::SINGLE_CHIP_WIDTH.max(circles_w) + 6.0;
        self.state.design_h = 140.0;
        self.state.base_height = HudGeometry::BASE_HEIGHT_PX * 140.0 / 66.0;
        self.show(cursor, 0.5);
    }

    pub fn hide(&mut self) {
        if !self.state.animate {
            unsafe { ShowWindow(self.hwnd, SW_HIDE) };
            self.state.fade_started = None;
            return;
        }
        self.state.fade_started = Some(Instant::now());
        unsafe {
            SetTimer(self.hwnd, FADE_TIMER, 16, None);
        }
    }

    fn show(&mut self, cursor: Option<POINT>, anchor: f64) {
        let mut point = cursor.unwrap_or(POINT { x: 0, y: 0 });
        if cursor.is_none() && unsafe { GetCursorPos(&mut point) } == 0 {
            return;
        }
        self.state.fade_started = None;
        unsafe { KillTimer(self.hwnd, FADE_TIMER) };

        let dpi = unsafe { GetDpiForWindow(self.hwnd) }.max(96) as f64;
        let scale = dpi / 96.0 * HudGeometry::scale(self.state.hud_size);
        let height = (self.state.base_height * scale).round().max(1.0) as i32;
        let width = (height as f64 * self.state.design_w / self.state.design_h)
            .round()
            .max(1.0) as i32;
        let x = point.x - (anchor.clamp(0.0, 1.0) * width as f64).round() as i32;
        let y = point.y + (14.0 * dpi / 96.0).round() as i32;
        unsafe {
            SetLayeredWindowAttributes(self.hwnd, TRANSPARENT_KEY, 255, LWA_COLORKEY | LWA_ALPHA);
            SetWindowPos(
                self.hwnd,
                HWND_TOPMOST,
                x,
                y,
                width,
                height,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            InvalidateRect(self.hwnd, std::ptr::null(), 0);
            UpdateWindow(self.hwnd);
        }
    }
}

impl Drop for CursorHud {
    fn drop(&mut self) {
        if !self.hwnd.is_null() {
            unsafe {
                KillTimer(self.hwnd, FADE_TIMER);
                DestroyWindow(self.hwnd);
            }
        }
    }
}

unsafe extern "system" fn hud_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut HudState;
    match message {
        WM_NCHITTEST => HTTRANSPARENT as LRESULT,
        WM_ERASEBKGND => 1,
        WM_TIMER if wparam == FADE_TIMER => {
            if !pointer.is_null() {
                let state = &mut *pointer;
                if let Some(started) = state.fade_started {
                    let progress = (started.elapsed().as_secs_f64() * 1000.0
                        / state.fade_ms.max(1.0))
                    .clamp(0.0, 1.0);
                    let alpha = ((1.0 - progress) * 255.0).round() as u8;
                    SetLayeredWindowAttributes(
                        hwnd,
                        TRANSPARENT_KEY,
                        alpha,
                        LWA_COLORKEY | LWA_ALPHA,
                    );
                    if progress >= 1.0 {
                        KillTimer(hwnd, FADE_TIMER);
                        ShowWindow(hwnd, SW_HIDE);
                        state.fade_started = None;
                    }
                }
            }
            0
        }
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut paint);
            if !pointer.is_null() && !hdc.is_null() {
                paint_hud(hwnd, hdc, &*pointer);
            }
            EndPaint(hwnd, &paint);
            0
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

unsafe fn paint_hud(hwnd: HWND, hdc: HDC, state: &HudState) {
    let mut client = RECT::default();
    if GetClientRect(hwnd, &mut client) == 0 {
        return;
    }
    fill(hdc, client, TRANSPARENT_KEY);
    let sx = (client.right - client.left) as f64 / state.design_w.max(1.0);
    let sy = (client.bottom - client.top) as f64 / state.design_h.max(1.0);
    match &state.content {
        HudContent::Snap(zone) => {
            draw_chip(
                hdc,
                design_rect(3.0, 3.0, 94.0, 60.0, sx, sy),
                snap_fraction(*zone),
                false,
                state,
            );
        }
        HudContent::Fraction(fraction) => {
            draw_chip(
                hdc,
                design_rect(3.0, 3.0, 94.0, 60.0, sx, sy),
                Some(*fraction),
                false,
                state,
            );
        }
        HudContent::Desktop {
            count,
            current,
            target,
            new_tile,
        } => {
            let selected = target.unwrap_or(*current);
            for index in 0..*count {
                let rect = design_rect(3.0 + index as f64 * 95.0, 3.0, 84.0, 60.0, sx, sy);
                let faint = target.is_some() && index == *current && selected != *current;
                draw_chip(
                    hdc,
                    rect,
                    if index == selected || faint {
                        Some((0.0, 0.0, 1.0, 1.0))
                    } else {
                        None
                    },
                    faint,
                    state,
                );
                if *new_tile == Some(index) {
                    draw_centered_text(hdc, rect, "+", state.text, 0);
                }
            }
        }
        HudContent::Monitor {
            up,
            down,
            left,
            right,
            target,
        } => {
            let cells = [
                (1, 0, *up, Some(MonitorDirection::Up)),
                (0, 1, *left, Some(MonitorDirection::Left)),
                (1, 1, true, None),
                (2, 1, *right, Some(MonitorDirection::Right)),
                (1, 2, *down, Some(MonitorDirection::Down)),
            ];
            for (col, row, exists, direction) in cells {
                let rect = design_rect(col as f64 * 89.0, row as f64 * 63.0, 80.0, 54.0, sx, sy);
                let aimed = direction.is_some() && *target == direction && exists;
                draw_chip(
                    hdc,
                    rect,
                    if aimed || direction.is_none() {
                        Some((0.0, 0.0, 1.0, 1.0))
                    } else {
                        None
                    },
                    direction.is_none() && target.is_some(),
                    state,
                );
                if !exists {
                    draw_centered_text(hdc, rect, "·", mix(state.background, state.text, 0.26), 0);
                }
            }
        }
        HudContent::Apps { titles, selected } => {
            let tiles_w = titles.len() as f64 * 66.0 + titles.len().saturating_sub(1) as f64 * 10.0;
            let left = (state.design_w - tiles_w) / 2.0;
            for (index, title) in titles.iter().enumerate() {
                let rect = design_rect(left + index as f64 * 76.0, 3.0, 66.0, 60.0, sx, sy);
                draw_chip(
                    hdc,
                    rect,
                    if index == *selected {
                        Some((0.0, 0.0, 1.0, 1.0))
                    } else {
                        None
                    },
                    index == *selected,
                    state,
                );
                let initial = title
                    .chars()
                    .next()
                    .unwrap_or('?')
                    .to_uppercase()
                    .to_string();
                draw_centered_text(hdc, rect, &initial, state.text, 0);
            }
            if let Some(title) = titles.get(*selected) {
                draw_centered_text(
                    hdc,
                    design_rect(3.0, 72.0, state.design_w - 6.0, 24.0, sx, sy),
                    title,
                    state.text,
                    DT_END_ELLIPSIS,
                );
            }
        }
        HudContent::Chooser {
            choose,
            close_picked,
        } => {
            let chip_x = (state.design_w - 94.0) / 2.0;
            draw_chip(
                hdc,
                design_rect(chip_x, 3.0, 94.0, 60.0, sx, sy),
                None,
                true,
                state,
            );
            let count: usize = if *choose { 2 } else { 1 };
            let circles_w = count as f64 * 56.0 + count.saturating_sub(1) as f64 * 22.0;
            let left = (state.design_w - circles_w) / 2.0;
            for index in 0..count {
                let close = !*choose || index == 1;
                let active = if *choose {
                    (close && *close_picked) || (!close && !*close_picked)
                } else {
                    true
                };
                let rect = design_rect(left + index as f64 * 78.0, 81.0, 56.0, 56.0, sx, sy);
                draw_circle(
                    hdc,
                    rect,
                    if close {
                        if active {
                            rgb(229, 72, 74)
                        } else {
                            rgb(106, 50, 54)
                        }
                    } else if active {
                        state.accent
                    } else {
                        state.background
                    },
                    state.edge,
                );
                draw_centered_text(hdc, rect, if close { "×" } else { "−" }, state.text, 0);
            }
        }
    }
}

unsafe fn draw_chip(
    hdc: HDC,
    rect: RECT,
    fraction: Option<(f64, f64, f64, f64)>,
    faint: bool,
    state: &HudState,
) {
    let radius = ((rect.bottom - rect.top) as f64 * 9.0 / 60.0)
        .round()
        .max(2.0) as i32;
    round_rect(hdc, rect, state.background, state.edge, radius);
    let stroke = ((rect.bottom - rect.top) as f64 * 2.5 / 60.0)
        .round()
        .max(1.0) as i32;
    let inner = RECT {
        left: rect.left + stroke,
        top: rect.top + stroke,
        right: rect.right - stroke,
        bottom: rect.bottom - stroke,
    };
    if let Some((x0, y0, x1, y1)) = fraction {
        let width = (inner.right - inner.left) as f64;
        let height = (inner.bottom - inner.top) as f64;
        let fill_rect = RECT {
            left: inner.left + (x0 * width).round() as i32,
            top: inner.top + (y0 * height).round() as i32,
            right: inner.left + (x1 * width).round() as i32,
            bottom: inner.top + (y1 * height).round() as i32,
        };
        let clip = CreateRoundRectRgn(
            inner.left,
            inner.top,
            inner.right + 1,
            inner.bottom + 1,
            radius,
            radius,
        );
        if !clip.is_null() {
            SelectClipRgn(hdc, clip);
        }
        fill(
            hdc,
            fill_rect,
            if faint {
                mix(state.background, state.accent, 0.28)
            } else {
                state.accent
            },
        );
        SelectClipRgn(hdc, std::ptr::null_mut());
        if !clip.is_null() {
            DeleteObject(clip);
        }
        outline_round_rect(hdc, rect, state.edge, radius);
    }
}

unsafe fn round_rect(hdc: HDC, rect: RECT, background: u32, edge: u32, radius: i32) {
    let brush = CreateSolidBrush(background);
    let pen = CreatePen(PS_SOLID, 1, edge);
    let old_brush = SelectObject(hdc, brush);
    let old_pen = SelectObject(hdc, pen);
    RoundRect(
        hdc,
        rect.left,
        rect.top,
        rect.right,
        rect.bottom,
        radius * 2,
        radius * 2,
    );
    SelectObject(hdc, old_brush);
    SelectObject(hdc, old_pen);
    DeleteObject(brush);
    DeleteObject(pen);
}

unsafe fn outline_round_rect(hdc: HDC, rect: RECT, edge: u32, radius: i32) {
    let pen = CreatePen(PS_SOLID, 1, edge);
    let old_pen = SelectObject(hdc, pen);
    let hollow = GetStockObject(HOLLOW_BRUSH);
    let old_brush = SelectObject(hdc, hollow);
    RoundRect(
        hdc,
        rect.left,
        rect.top,
        rect.right,
        rect.bottom,
        radius * 2,
        radius * 2,
    );
    SelectObject(hdc, old_brush);
    SelectObject(hdc, old_pen);
    DeleteObject(pen);
}

unsafe fn draw_circle(hdc: HDC, rect: RECT, background: u32, edge: u32) {
    let radius = ((rect.right - rect.left).min(rect.bottom - rect.top) / 2).max(2);
    round_rect(hdc, rect, background, edge, radius);
}

unsafe fn draw_centered_text(hdc: HDC, mut rect: RECT, value: &str, color: u32, extra: u32) {
    let text = wide_without_null(value);
    SetBkMode(hdc, TRANSPARENT as i32);
    SetTextColor(hdc, color);
    DrawTextW(
        hdc,
        text.as_ptr(),
        text.len() as i32,
        &mut rect,
        DT_CENTER | DT_VCENTER | DT_SINGLELINE | extra,
    );
}

unsafe fn fill(hdc: HDC, rect: RECT, color: u32) {
    let brush = CreateSolidBrush(color);
    FillRect(hdc, &rect, brush);
    DeleteObject(brush);
}

fn design_rect(x: f64, y: f64, width: f64, height: f64, sx: f64, sy: f64) -> RECT {
    RECT {
        left: (x * sx).round() as i32,
        top: (y * sy).round() as i32,
        right: ((x + width) * sx).round() as i32,
        bottom: ((y + height) * sy).round() as i32,
    }
}

fn resolve_accent(config: &AdvancedConfig) -> u32 {
    if config.overlay_use_accent {
        read_windows_accent().unwrap_or_else(|| rgb(10, 132, 255))
    } else {
        parse_hex(&config.overlay_color).unwrap_or_else(|| rgb(10, 132, 255))
    }
}

fn read_windows_accent() -> Option<u32> {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};
    let current = RegKey::predef(HKEY_CURRENT_USER);
    let key = current
        .open_subkey(r"Software\Microsoft\Windows\DWM")
        .ok()?;
    let value = key.get_value::<u32, _>("AccentColor").ok()?;
    Some(rgb(
        (value & 0xff) as u8,
        ((value >> 8) & 0xff) as u8,
        ((value >> 16) & 0xff) as u8,
    ))
}

fn system_uses_light_theme() -> bool {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};
    let current = RegKey::predef(HKEY_CURRENT_USER);
    current
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
        .and_then(|key| key.get_value::<u32, _>("AppsUseLightTheme"))
        .map(|value| value != 0)
        .unwrap_or(false)
}

fn parse_hex(value: &str) -> Option<u32> {
    let raw = value.strip_prefix('#')?;
    if raw.len() != 6 {
        return None;
    }
    Some(rgb(
        u8::from_str_radix(&raw[0..2], 16).ok()?,
        u8::from_str_radix(&raw[2..4], 16).ok()?,
        u8::from_str_radix(&raw[4..6], 16).ok()?,
    ))
}

const fn rgb(red: u8, green: u8, blue: u8) -> u32 {
    red as u32 | ((green as u32) << 8) | ((blue as u32) << 16)
}

fn mix(background: u32, foreground: u32, amount: f64) -> u32 {
    let channel = |shift: u32| {
        let bg = ((background >> shift) & 0xff) as f64;
        let fg = ((foreground >> shift) & 0xff) as f64;
        (bg + (fg - bg) * amount.clamp(0.0, 1.0)).round() as u8
    };
    rgb(channel(0), channel(8), channel(16))
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn wide_without_null(value: &str) -> Vec<u16> {
    value.encode_utf16().collect()
}
