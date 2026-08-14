//! Windows actions for the Rust AdvancedGestures state machine.
//!
//! The state machine deliberately knows nothing about HWNDs.  This controller
//! is the small, fail-closed Win32 boundary that turns its events into window
//! movement.  It never substitutes the foreground window when the pointer is
//! not over a manageable window.

use std::{
    ffi::c_void,
    mem,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Instant,
};

use better_touch_advanced_gestures::{
    config::{
        app_compatibility::normalize_process_name, AdvancedConfig, AppCompatibilityMode,
        GridModifier, SwipeDownMode,
    },
    geometry::{zone_rect, Rect as ZoneRect, SnapZone},
    gesture::{DesktopDirection, GestureEvent, MonitorDirection, SwipeDirection},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS},
    Graphics::Gdi::{
        BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, FrameRect, GetMonitorInfoW,
        GetSysColor, InvalidateRect, MonitorFromWindow, COLOR_HIGHLIGHT, MONITORINFO,
        MONITOR_DEFAULTTONEAREST, PAINTSTRUCT,
    },
    System::{
        LibraryLoader::GetModuleHandleW,
        Threading::{
            OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
            PROCESS_QUERY_LIMITED_INFORMATION,
        },
    },
    UI::{
        HiDpi::GetDpiForWindow,
        Input::KeyboardAndMouse::{
            keybd_event, GetAsyncKeyState, KEYEVENTF_KEYUP, VK_CONTROL, VK_LEFT, VK_LWIN, VK_MENU,
            VK_RIGHT, VK_SHIFT,
        },
        WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, EnumWindows, GetAncestor,
            GetClassNameW, GetCursorPos, GetWindowLongPtrW, GetWindowPlacement, GetWindowRect,
            GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
            IsZoomed, PostMessageW, RegisterClassExW, SetForegroundWindow,
            SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, ShowWindow,
            WindowFromPoint, GA_ROOT, GWLP_USERDATA, GWL_STYLE, HTTRANSPARENT, HWND_TOPMOST,
            LWA_ALPHA, SWP_NOACTIVATE, SWP_NOZORDER, SWP_SHOWWINDOW, SW_MAXIMIZE, SW_MINIMIZE,
            SW_RESTORE, WINDOWPLACEMENT, WM_CLOSE, WM_NCHITTEST, WM_PAINT, WNDCLASSEXW, WS_CAPTION,
            WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP,
        },
    },
};

use crate::logging::RingLogger;
use crate::swoosh_hud::{
    chooser_was_reversed, restore_direction_after_dwell, should_show_snap_hud,
};

use super::{cursor_hud::CursorHud, virtual_desktop};

const MIN_WINDOW_WIDTH: i32 = 260;
const MIN_WINDOW_HEIGHT: i32 = 180;
const OVERLAY_CLASS: &str = "ThreeFingerDragRust.AdvancedPreview";

struct OverlayState {
    color: u32,
    alpha: u8,
    border: u32,
}

impl Default for OverlayState {
    fn default() -> Self {
        Self {
            color: colorref_from_hex("#0A84FF"),
            alpha: 86,
            border: colorref_from_hex("#0A84FF"),
        }
    }
}

struct PreviewOverlay {
    hwnd: HWND,
    state: Box<OverlayState>,
}

#[derive(Debug, Clone, Copy, Default)]
struct WindowRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl WindowRect {
    fn width(self) -> i32 {
        self.right - self.left
    }
    fn height(self) -> i32 {
        self.bottom - self.top
    }
    fn center_x(self) -> f64 {
        (self.left + self.right) as f64 / 2.0
    }
    fn center_y(self) -> f64 {
        (self.top + self.bottom) as f64 / 2.0
    }
}

pub struct AdvancedWindowController {
    config: AdvancedConfig,
    target: HWND,
    original_rect: WindowRect,
    have_original_rect: bool,
    original_was_maximized: bool,
    max_restored: bool,
    last_dx: f64,
    last_dy: f64,
    live_moved: bool,
    free_rect: WindowRect,
    free_active: bool,
    axis_rect: WindowRect,
    axis_active: bool,
    axis_horizontal: bool,
    axis_changed: bool,
    live_preview_zone: Option<SnapZone>,
    gesture_started: Option<Instant>,
    app_switch_active: bool,
    app_windows: Vec<HWND>,
    app_start: usize,
    app_selected: usize,
    app_target_rect: WindowRect,
    app_target_was_maximized: bool,
    down_latched: bool,
    down_engaged: bool,
    down_pick_close: bool,
    down_peak_y: f64,
    down_max_y: f64,
    prior_direction: SwipeDirection,
    prior_direction_since: Option<Instant>,
    restore_direction: SwipeDirection,
    pending_rebaseline: Option<SwipeDirection>,
    mouse_tracking: bool,
    mouse_start: POINT,
    mouse_started: Option<Instant>,
    mouse_hold_engaged: bool,
    mouse_hold_monitor: bool,
    mouse_hold_direction: Option<MonitorDirection>,
    mouse_hold_steps: i32,
    monitor_mode: bool,
    logger: RingLogger,
    animation_generation: Arc<AtomicU64>,
    overlay: Option<PreviewOverlay>,
    hud: Option<CursorHud>,
}

impl AdvancedWindowController {
    pub fn new(logger: RingLogger) -> Self {
        Self {
            config: AdvancedConfig::default(),
            target: std::ptr::null_mut(),
            original_rect: WindowRect::default(),
            have_original_rect: false,
            original_was_maximized: false,
            max_restored: false,
            last_dx: 0.0,
            last_dy: 0.0,
            live_moved: false,
            free_rect: WindowRect::default(),
            free_active: false,
            axis_rect: WindowRect::default(),
            axis_active: false,
            axis_horizontal: false,
            axis_changed: false,
            live_preview_zone: None,
            gesture_started: None,
            app_switch_active: false,
            app_windows: Vec::new(),
            app_start: 0,
            app_selected: 0,
            app_target_rect: WindowRect::default(),
            app_target_was_maximized: false,
            down_latched: false,
            down_engaged: false,
            down_pick_close: false,
            down_peak_y: 0.0,
            down_max_y: 0.0,
            prior_direction: SwipeDirection::None,
            prior_direction_since: None,
            restore_direction: SwipeDirection::None,
            pending_rebaseline: None,
            mouse_tracking: false,
            mouse_start: POINT { x: 0, y: 0 },
            mouse_started: None,
            mouse_hold_engaged: false,
            mouse_hold_monitor: false,
            mouse_hold_direction: None,
            mouse_hold_steps: 0,
            monitor_mode: false,
            logger,
            animation_generation: Arc::new(AtomicU64::new(0)),
            overlay: create_preview_overlay(),
            hud: CursorHud::new(),
        }
    }

    pub fn apply_config(&mut self, config: AdvancedConfig) {
        let config = config.normalized();
        if self.config == config {
            return;
        }
        if self.config.animate_snaps != config.animate_snaps {
            self.animation_generation.fetch_add(1, Ordering::Relaxed);
        }
        if self.config.enabled
            && self.config.gestures_enabled
            && (!config.enabled || !config.gestures_enabled)
        {
            self.cancel();
        }
        self.config = config;
        self.update_overlay_appearance();
        if let Some(hud) = self.hud.as_mut() {
            hud.apply_config(&self.config);
        }
    }

    fn update_overlay_appearance(&mut self) {
        let Some(overlay) = self.overlay.as_mut() else {
            return;
        };
        let color = if self.config.overlay_use_accent {
            // GetSysColor returns the current Windows accent/highlight COLORREF,
            // which is the same live fallback used by the source HUD.
            unsafe { GetSysColor(COLOR_HIGHLIGHT) }
        } else {
            colorref_from_hex(&self.config.overlay_color)
        };
        overlay.state.color = color;
        overlay.state.border = color;
        overlay.state.alpha = match self.config.hud_background {
            better_touch_advanced_gestures::config::HudTheme::Light => 72,
            better_touch_advanced_gestures::config::HudTheme::System
            | better_touch_advanced_gestures::config::HudTheme::Dark => 86,
        };
        unsafe {
            SetLayeredWindowAttributes(overlay.hwnd, 0, overlay.state.alpha, LWA_ALPHA);
            InvalidateRect(overlay.hwnd, std::ptr::null(), 1);
        }
    }

    fn hide_overlay(&self) {
        if let Some(overlay) = &self.overlay {
            unsafe {
                ShowWindow(
                    overlay.hwnd,
                    windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE,
                )
            };
        }
    }

    fn show_overlay(&self, rect: WindowRect) {
        let Some(overlay) = &self.overlay else {
            return;
        };
        let width = rect.width().max(1);
        let height = rect.height().max(1);
        unsafe {
            SetWindowPos(
                overlay.hwnd,
                HWND_TOPMOST,
                rect.left,
                rect.top,
                width,
                height,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            ShowWindow(
                overlay.hwnd,
                windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNOACTIVATE,
            );
        }
    }

    pub fn set_monitor_move_mode(&mut self, monitor: bool) {
        self.monitor_mode = monitor && self.config.monitor_move_enabled;
    }

    pub fn mouse_middle_down(&mut self, point: POINT) -> bool {
        if !self.config.enabled
            || !self.config.gestures_enabled
            || !self.config.mouse_middle_button_hud_enabled
        {
            return false;
        }
        self.target = self.target_under_cursor();
        if self.target.is_null() || !get_rect(self.target, &mut self.original_rect) {
            self.clear_target();
            return false;
        }
        self.original_was_maximized = unsafe { IsZoomed(self.target) } != 0;
        self.have_original_rect = true;
        self.mouse_start = point;
        self.mouse_started = Some(Instant::now());
        self.mouse_hold_engaged = false;
        self.mouse_hold_monitor = false;
        self.mouse_hold_direction = None;
        self.mouse_hold_steps = 0;
        self.last_dx = 0.0;
        self.last_dy = 0.0;
        self.mouse_tracking = true;
        if let Some(hud) = self.hud.as_mut() {
            hud.show_snap_at(SnapZone::None, Some(point));
        }
        true
    }

    pub fn mouse_middle_move(&mut self, point: POINT) {
        if !self.mouse_tracking || self.target.is_null() {
            return;
        }
        let dx = (point.x - self.mouse_start.x) as f64 / 120.0;
        let dy = (point.y - self.mouse_start.y) as f64 / 120.0;
        self.last_dx = dx;
        self.last_dy = dy;
        if !self.mouse_hold_engaged
            && self
                .mouse_started
                .map(|started| {
                    started.elapsed().as_secs_f64() >= self.config.desktop_hold_delay_seconds
                })
                .unwrap_or(false)
        {
            self.mouse_hold_engaged = true;
            self.mouse_hold_monitor = self.monitor_mode;
            self.mouse_hold_direction = None;
            self.mouse_hold_steps = 0;
            if self.mouse_hold_monitor {
                self.show_monitor_hud(None);
            } else {
                self.begin_hold_mode();
                if !self.app_switch_active {
                    self.show_desktop_hud(0);
                }
            }
            self.hide_overlay();
            return;
        }
        if self.mouse_hold_engaged {
            if self.mouse_hold_monitor {
                let ax = dx.abs();
                let ay = dy.abs();
                self.mouse_hold_direction = if ax < 14.0 / 120.0 && ay < 14.0 / 120.0 {
                    None
                } else if ax >= ay {
                    Some(if dx >= 0.0 {
                        MonitorDirection::Right
                    } else {
                        MonitorDirection::Left
                    })
                } else {
                    Some(if dy >= 0.0 {
                        MonitorDirection::Down
                    } else {
                        MonitorDirection::Up
                    })
                };
                self.show_monitor_hud(self.mouse_hold_direction);
            } else {
                self.mouse_hold_steps = round_away(dx / 1.0);
                if self.app_switch_active {
                    let selected = if self.mouse_hold_steps >= 0 {
                        self.app_start
                            .saturating_add(self.mouse_hold_steps as usize)
                    } else {
                        self.app_start
                            .saturating_sub(self.mouse_hold_steps.unsigned_abs() as usize)
                    };
                    self.app_selected = selected.min(self.app_windows.len().saturating_sub(1));
                    self.show_app_hud();
                } else {
                    self.show_desktop_hud(self.mouse_hold_steps);
                }
            }
            self.hide_overlay();
            return;
        }
        let distance = (dx * dx + dy * dy).sqrt();
        if distance < 14.0 / 120.0 {
            self.hide_overlay();
            if let Some(hud) = self.hud.as_mut() {
                hud.show_snap_at(SnapZone::None, Some(self.mouse_start));
            }
            return;
        }
        let direction = better_touch_advanced_gestures::gesture::GestureEngine::classify(dx, dy);
        let zone = self.map_zone(direction);
        if let Some(hud) = self.hud.as_mut() {
            hud.show_snap_at(zone, Some(self.mouse_start));
        }
        if zone == SnapZone::None || zone == SnapZone::Minimize {
            self.hide_overlay();
            return;
        }
        if let Some(work) = monitor_work_area(self.target) {
            let rect = if zone == SnapZone::Maximize {
                WindowRect {
                    left: work.left,
                    top: work.top,
                    right: work.right,
                    bottom: work.bottom,
                }
            } else {
                rect_from_zone(zone_rect(
                    ZoneRect::new(work.left, work.top, work.right, work.bottom),
                    zone,
                    self.config.grid_spacing,
                ))
            };
            self.show_overlay(rect);
        }
    }

    pub fn mouse_middle_up(&mut self, point: POINT) {
        if !self.mouse_tracking {
            return;
        }
        self.mouse_middle_move(point);
        self.mouse_tracking = false;
        self.mouse_started = None;
        if self.mouse_hold_engaged {
            if self.mouse_hold_monitor {
                if let Some(direction) = self.mouse_hold_direction {
                    self.move_to_monitor(direction);
                } else {
                    self.clear_target();
                }
            } else if self.app_switch_active {
                self.commit_app_switch(self.mouse_hold_steps);
            } else if self.mouse_hold_steps != 0 {
                self.commit_virtual_desktop_steps(self.mouse_hold_steps);
                self.clear_target();
            } else {
                self.clear_target();
            }
            self.mouse_hold_engaged = false;
            self.mouse_hold_monitor = false;
            self.mouse_hold_direction = None;
            self.mouse_hold_steps = 0;
            self.hide_overlay();
            self.hide_hud();
            return;
        }
        let direction = better_touch_advanced_gestures::gesture::GestureEngine::classify(
            self.last_dx,
            self.last_dy,
        );
        self.complete(direction);
    }

    pub fn handle(&mut self, event: &GestureEvent) {
        if !self.config.enabled || !self.config.gestures_enabled {
            return;
        }
        match *event {
            GestureEvent::Began { .. } => self.begin_gesture(),
            GestureEvent::Raw { dx, dy } => {
                self.last_dx = dx;
                self.last_dy = dy;
                self.down_max_y = self.down_max_y.max(dy);
            }
            GestureEvent::Completed(direction) => self.complete(direction),
            GestureEvent::Updated {
                direction,
                progress,
            } => self.preview(direction, progress),
            GestureEvent::Cancelled => self.cancel(),
            GestureEvent::FreeMoveBegan => self.begin_free_move(),
            GestureEvent::FreeMoveDelta { dx, dy, scale } => self.free_move(dx, dy, scale),
            GestureEvent::FreeMoveEnded { was_tap, cancelled } => {
                self.end_free_move(was_tap, cancelled)
            }
            GestureEvent::PinchOut => self.maximize(),
            GestureEvent::PinchIn => self.restore(),
            GestureEvent::AxisResizeBegan { horizontal } => self.begin_axis_resize(horizontal),
            GestureEvent::AxisResizeDelta { factor, horizontal } => {
                self.axis_resize(factor, horizontal)
            }
            GestureEvent::AxisResizeEnded { cancelled } => self.end_axis_resize(cancelled),
            GestureEvent::DesktopMove(direction) => {
                if !self.app_switch_active {
                    self.move_virtual_desktop(direction)
                }
            }
            GestureEvent::DesktopHoldCommit(steps) => {
                if self.app_switch_active {
                    self.commit_app_switch(steps);
                } else {
                    self.commit_virtual_desktop_steps(steps)
                }
            }
            GestureEvent::MonitorMove(direction) => self.move_to_monitor(direction),
            GestureEvent::HoldEngaged => {
                self.begin_hold_mode();
                if !self.app_switch_active {
                    self.show_desktop_hud(0);
                }
            }
            GestureEvent::HoldUpdated { aim_steps, .. } => {
                if self.app_switch_active && !self.app_windows.is_empty() {
                    let offset = aim_steps;
                    self.app_selected = if offset >= 0 {
                        self.app_start.saturating_add(offset as usize)
                    } else {
                        self.app_start
                            .saturating_sub(offset.unsigned_abs() as usize)
                    }
                    .min(self.app_windows.len().saturating_sub(1));
                    self.show_app_hud();
                } else {
                    self.show_desktop_hud(aim_steps);
                }
            }
            GestureEvent::MonitorMoveUpdated { direction, .. } => self.show_monitor_hud(direction),
            GestureEvent::PinchUpdated { outward, .. } => {
                if let Some(hud) = self.hud.as_mut() {
                    hud.show_snap(if outward {
                        SnapZone::Maximize
                    } else {
                        SnapZone::Center
                    });
                }
            }
        }
    }

    pub fn take_rebaseline_request(&mut self) -> Option<SwipeDirection> {
        self.pending_rebaseline.take()
    }

    pub fn cancel(&mut self) {
        self.animation_generation.fetch_add(1, Ordering::Relaxed);
        if self.live_moved && !self.target.is_null() && self.have_original_rect {
            if self.original_was_maximized() {
                unsafe { ShowWindow(self.target, SW_MAXIMIZE) };
            } else {
                unsafe { ShowWindow(self.target, SW_RESTORE) };
                position(self.target, self.original_rect);
            }
        }
        self.clear_target();
        self.free_active = false;
        self.axis_active = false;
        self.live_preview_zone = None;
        self.mouse_tracking = false;
        self.mouse_started = None;
        self.mouse_hold_engaged = false;
        self.mouse_hold_monitor = false;
        self.mouse_hold_direction = None;
        self.mouse_hold_steps = 0;
        self.down_latched = false;
        self.down_engaged = false;
        self.down_pick_close = false;
        self.down_peak_y = 0.0;
        self.down_max_y = 0.0;
        self.prior_direction = SwipeDirection::None;
        self.prior_direction_since = None;
        self.restore_direction = SwipeDirection::None;
        self.pending_rebaseline = None;
        self.hide_overlay();
        self.hide_hud();
    }

    fn begin_gesture(&mut self) {
        self.last_dx = 0.0;
        self.last_dy = 0.0;
        self.live_moved = false;
        self.live_preview_zone = None;
        self.have_original_rect = false;
        self.original_was_maximized = false;
        self.max_restored = false;
        self.down_latched = false;
        self.down_engaged = false;
        self.down_pick_close = false;
        self.down_peak_y = 0.0;
        self.down_max_y = 0.0;
        self.prior_direction = SwipeDirection::None;
        self.prior_direction_since = None;
        self.restore_direction = SwipeDirection::None;
        self.pending_rebaseline = None;
        self.gesture_started = Some(Instant::now());
        self.hide_hud();
        self.target = self.target_under_cursor();
        if !self.target.is_null() {
            self.have_original_rect = get_rect(self.target, &mut self.original_rect);
            self.original_was_maximized = unsafe { IsZoomed(self.target) } != 0;
        }
    }

    fn begin_hold_mode(&mut self) {
        if !self.config.app_switch_on_hold || self.target.is_null() {
            return;
        }
        self.app_windows = enumerate_switchable_windows();
        if self.app_windows.is_empty() {
            return;
        }
        if !self.app_windows.contains(&self.target) {
            self.app_windows.insert(0, self.target);
        }
        self.app_start = self
            .app_windows
            .iter()
            .position(|window| *window == self.target)
            .unwrap_or(0);
        self.app_selected = self.app_start;
        self.app_target_rect = get_window_rect(self.target).unwrap_or_default();
        self.app_target_was_maximized = unsafe { IsZoomed(self.target) } != 0;
        self.app_switch_active = true;
        self.show_app_hud();
    }

    fn commit_app_switch(&mut self, steps: i32) {
        if !self.app_switch_active || self.app_windows.is_empty() {
            return;
        }
        let selected = if steps >= 0 {
            self.app_start.saturating_add(steps as usize)
        } else {
            self.app_start.saturating_sub(steps.unsigned_abs() as usize)
        }
        .min(self.app_windows.len().saturating_sub(1));
        let hwnd = self.app_windows[selected];
        if hwnd != self.target && manageable(hwnd) {
            unsafe { ShowWindow(hwnd, SW_RESTORE) };
            if self.app_target_was_maximized {
                unsafe { ShowWindow(hwnd, SW_MAXIMIZE) };
            } else {
                position(hwnd, self.app_target_rect);
            }
            unsafe { SetForegroundWindow(hwnd) };
        }
        self.app_switch_active = false;
        self.app_windows.clear();
        self.hide_overlay();
        self.hide_hud();
        self.clear_target();
    }

    fn complete(&mut self, direction: SwipeDirection) {
        if self.target.is_null() {
            return;
        }
        if self.down_latched {
            if self.down_engaged {
                let close = self.down_pick_close
                    || matches!(self.config.swipe_down_action, SwipeDownMode::Close);
                if close {
                    unsafe { PostMessageW(self.target, WM_CLOSE, 0, 0) };
                } else {
                    self.apply(SnapZone::Minimize);
                }
            }
            self.clear_target();
            self.hide_overlay();
            self.hide_hud();
            return;
        }
        let mut zone = self.map_zone(direction);
        if zone == SnapZone::Minimize {
            if self.last_dy < self.config.swipe_down_threshold {
                zone = SnapZone::None;
            } else {
                match self.config.swipe_down_action {
                    SwipeDownMode::Close => {
                        unsafe { PostMessageW(self.target, WM_CLOSE, 0, 0) };
                        self.clear_target();
                        self.hide_overlay();
                        self.hide_hud();
                        return;
                    }
                    SwipeDownMode::Choose if self.last_dx > 0.045 => {
                        unsafe { PostMessageW(self.target, WM_CLOSE, 0, 0) };
                        self.clear_target();
                        self.hide_overlay();
                        self.hide_hud();
                        return;
                    }
                    SwipeDownMode::Minimize | SwipeDownMode::Choose => {}
                }
            }
        }
        if zone == SnapZone::None {
            self.restore_live_original();
        } else {
            let already_live =
                self.config.live_preview && self.live_moved && self.live_preview_zone == Some(zone);
            if !already_live {
                self.apply(zone);
            }
        }
        self.clear_target();
        self.live_preview_zone = None;
        self.hide_overlay();
        self.hide_hud();
    }

    /// Swoosh's live-preview mode moves the real window as the two fingers aim
    /// at a zone. The original rectangle is kept so Esc/rest cancellation can
    /// put the window back exactly where it started.
    fn preview(&mut self, direction: SwipeDirection, progress: f64) {
        if self.target.is_null() {
            return;
        }
        self.down_max_y = self.down_max_y.max(self.last_dy);

        // Once the down-action chooser owns a gesture it must keep receiving
        // diagonal/neutral frames too; leaning right is how Close is picked.
        if self.down_latched && self.handle_down_action(direction) {
            return;
        }

        let elapsed_ms = self
            .gesture_started
            .map(|started| started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64)
            .unwrap_or_default();

        if direction == SwipeDirection::None || progress <= 0.0 {
            self.restore_live_original();
            self.live_preview_zone = None;
            self.hide_overlay();
            if should_show_snap_hud(true, elapsed_ms, SnapZone::None) {
                if let Some(hud) = self.hud.as_mut() {
                    hud.show_snap(SnapZone::None);
                }
            }
            return;
        }

        let zone = self.map_zone(direction);

        // Swoosh deliberately ignores a shallow downward drift in every down
        // mode. It shows the neutral chip, never a minimize preview.
        if self.config.minimize_enabled
            && !self.down_latched
            && zone == SnapZone::Minimize
            && self.down_max_y < self.config.swipe_down_threshold
        {
            self.hide_overlay();
            if let Some(hud) = self.hud.as_mut() {
                hud.show_snap(SnapZone::None);
            }
            return;
        }

        if self.handle_down_action(direction) {
            return;
        }

        // A disabled gesture has no target. Preserve the previous visual just
        // like Swoosh so diagonal classification changes do not make it flash.
        if zone == SnapZone::None {
            return;
        }

        if zone != SnapZone::Minimize && direction != self.prior_direction {
            self.prior_direction = direction;
            self.prior_direction_since = Some(Instant::now());
        }

        let restore_instead_of_max = zone == SnapZone::Maximize && self.original_was_maximized;
        let show_restore =
            zone == SnapZone::Maximize && (self.original_was_maximized || self.max_restored);
        let restore_target = show_restore.then(|| restore_rect(self.target)).flatten();

        if should_show_snap_hud(true, elapsed_ms, zone) {
            if let Some(hud) = self.hud.as_mut() {
                if let (Some(rect), Some(work)) = (restore_target, monitor_work_area(self.target)) {
                    hud.show_fraction(rect_fraction(rect, work));
                } else {
                    hud.show_snap(zone);
                }
            }
        }

        if zone == self.live_preview_zone.unwrap_or(SnapZone::None) || !manageable(self.target) {
            return;
        }

        if zone == SnapZone::Minimize {
            self.restore_live_original();
            self.live_preview_zone = Some(zone);
            if !self.config.live_preview {
                if let Some(work) = monitor_work_area(self.target) {
                    self.show_overlay(minimize_hint(work));
                }
            } else {
                self.hide_overlay();
            }
            return;
        }

        if !self.config.live_preview {
            let target = if restore_instead_of_max {
                restore_target
            } else if zone == SnapZone::Maximize {
                monitor_work_area(self.target)
            } else {
                monitor_work_area(self.target).map(|work| {
                    rect_from_zone(zone_rect(
                        ZoneRect::new(work.left, work.top, work.right, work.bottom),
                        zone,
                        self.config.grid_spacing,
                    ))
                })
            };
            if let Some(rect) = target {
                self.show_overlay(rect);
            }
            self.live_preview_zone = Some(zone);
            return;
        }
        if zone == SnapZone::Maximize {
            if restore_instead_of_max {
                unsafe { ShowWindow(self.target, SW_RESTORE) };
                if let Some(rect) = get_window_rect(self.target) {
                    self.original_rect = rect;
                    self.have_original_rect = true;
                }
                self.original_was_maximized = false;
                self.max_restored = true;
            } else {
                unsafe { ShowWindow(self.target, SW_MAXIMIZE) };
            }
        } else if let Some(work) = monitor_work_area(self.target) {
            unsafe { ShowWindow(self.target, SW_RESTORE) };
            let target = rect_from_zone(zone_rect(
                ZoneRect::new(work.left, work.top, work.right, work.bottom),
                zone,
                self.config.grid_spacing,
            ));
            position(self.target, target);
        }
        self.live_moved = true;
        self.live_preview_zone = Some(zone);
    }

    /// Reproduce Swoosh's deliberate down-swipe chooser without allowing a
    /// shallow diagonal gesture to close or minimize a window.  The chooser is
    /// intentionally stateful: once engaged, leaning right selects Close and
    /// leaning left selects Minimize until the user retreats upward or lifts.
    fn handle_down_action(&mut self, direction: SwipeDirection) -> bool {
        let down_mode = self.config.minimize_enabled
            && !matches!(self.config.swipe_down_action, SwipeDownMode::Minimize);
        if !down_mode {
            return false;
        }

        if self.down_latched {
            self.down_peak_y = self.down_peak_y.max(self.last_dy);
            if chooser_was_reversed(self.last_dy, self.down_peak_y) {
                self.down_latched = false;
                self.down_engaged = false;
                self.down_pick_close = false;
                self.down_peak_y = 0.0;
                self.pending_rebaseline = Some(self.restore_direction);
                self.restore_live_original();
                self.hide_overlay();
                self.hide_hud();
                return true;
            }

            if direction == SwipeDirection::None {
                self.down_engaged = false;
                self.hide_overlay();
                self.hide_hud();
                return true;
            }

            self.down_engaged = true;
            if matches!(self.config.swipe_down_action, SwipeDownMode::Choose) {
                self.down_pick_close = self.last_dx > 0.045;
            }
            self.hide_overlay();
            if let Some(hud) = self.hud.as_mut() {
                hud.show_down_chooser(
                    matches!(self.config.swipe_down_action, SwipeDownMode::Choose),
                    self.down_pick_close,
                    None,
                );
            }
            return true;
        }

        if self.map_zone(direction) != SnapZone::Minimize {
            return false;
        }

        let dwell_ms = self
            .prior_direction_since
            .map(|started| started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64)
            .unwrap_or_default();
        self.restore_direction = restore_direction_after_dwell(self.prior_direction, dwell_ms);
        self.down_latched = true;
        self.down_engaged = true;
        self.down_peak_y = self.last_dy;
        self.down_pick_close = matches!(self.config.swipe_down_action, SwipeDownMode::Close)
            || (matches!(self.config.swipe_down_action, SwipeDownMode::Choose)
                && self.last_dx > 0.045);
        self.restore_live_original();
        self.hide_overlay();
        if let Some(hud) = self.hud.as_mut() {
            hud.show_down_chooser(
                matches!(self.config.swipe_down_action, SwipeDownMode::Choose),
                self.down_pick_close,
                None,
            );
        }
        true
    }

    fn restore_live_original(&mut self) {
        if !self.live_moved || self.target.is_null() || !self.have_original_rect {
            return;
        }
        if self.original_was_maximized {
            unsafe { ShowWindow(self.target, SW_MAXIMIZE) };
        } else {
            unsafe { ShowWindow(self.target, SW_RESTORE) };
            position(self.target, self.original_rect);
        }
        self.live_moved = false;
    }

    fn map_zone(&self, direction: SwipeDirection) -> SnapZone {
        match direction {
            SwipeDirection::Left if self.config.halves_enabled => SnapZone::LeftHalf,
            SwipeDirection::Right if self.config.halves_enabled => SnapZone::RightHalf,
            SwipeDirection::Up if self.config.maximize_enabled => SnapZone::Maximize,
            SwipeDirection::Down if self.config.minimize_enabled => SnapZone::Minimize,
            SwipeDirection::UpLeft if self.config.quarters_enabled => SnapZone::TopLeft,
            SwipeDirection::UpRight if self.config.quarters_enabled => SnapZone::TopRight,
            SwipeDirection::DownLeft if self.config.quarters_enabled => SnapZone::BottomLeft,
            SwipeDirection::DownRight if self.config.quarters_enabled => SnapZone::BottomRight,
            _ => SnapZone::None,
        }
    }

    fn apply(&mut self, zone: SnapZone) {
        if !manageable(self.target) {
            return;
        }
        unsafe { SetForegroundWindow(self.target) };
        if zone == SnapZone::Minimize {
            unsafe { ShowWindow(self.target, SW_MINIMIZE) };
            return;
        }
        if zone == SnapZone::Maximize {
            unsafe {
                ShowWindow(
                    self.target,
                    if self.original_was_maximized() {
                        SW_RESTORE
                    } else {
                        SW_MAXIMIZE
                    },
                )
            };
            return;
        }
        unsafe { ShowWindow(self.target, SW_RESTORE) };
        let Some(work) = monitor_work_area(self.target) else {
            self.logger.record("无法读取目标窗口的显示器工作区。 ");
            return;
        };
        let mut target = zone_rect(
            ZoneRect::new(work.left, work.top, work.right, work.bottom),
            zone,
            self.config.grid_spacing,
        );
        let Some(current) = get_window_rect(self.target) else {
            return;
        };
        // Keep the movement within the work area even when a DPI-scaled window
        // reports a stale outer rectangle.
        target.left = target.left.clamp(work.left, work.right - 1);
        target.top = target.top.clamp(work.top, work.bottom - 1);
        target.right = target.right.clamp(target.left + 1, work.right);
        target.bottom = target.bottom.clamp(target.top + 1, work.bottom);
        let target_rect = rect_from_zone(target);
        if self.config.animate_snaps {
            let generation = self.animation_generation.fetch_add(1, Ordering::Relaxed) + 1;
            animate_position(
                self.target,
                current,
                target_rect,
                self.config.snap_animation_seconds,
                Arc::clone(&self.animation_generation),
                generation,
            );
        } else {
            position(self.target, target_rect);
        }
        if self.config.move_cursor {
            move_cursor_with_window(current, target_rect);
        }
        self.live_moved = false;
    }

    fn begin_free_move(&mut self) {
        self.hide_hud();
        self.hide_overlay();
        self.target = self.target_under_cursor();
        self.free_active = !self.target.is_null() && get_rect(self.target, &mut self.free_rect);
        if self.free_active && unsafe { IsZoomed(self.target) } != 0 {
            unsafe { ShowWindow(self.target, SW_RESTORE) };
        }
    }

    fn free_move(&mut self, dx: f64, dy: f64, scale: f64) {
        if !self.free_active || self.target.is_null() {
            return;
        }
        let Some(work) = monitor_work_area(self.target) else {
            return;
        };
        let move_x = (dx * work.width() as f64).round() as i32;
        let move_y = (dy * work.height() as f64).round() as i32;
        let width = ((self.free_rect.width() as f64) * scale).round() as i32;
        let height = ((self.free_rect.height() as f64) * scale).round() as i32;
        let width = width.clamp(MIN_WINDOW_WIDTH.min(work.width()), work.width());
        let height = height.clamp(MIN_WINDOW_HEIGHT.min(work.height()), work.height());
        self.free_rect = clamp_rect(
            WindowRect {
                left: self.free_rect.left + move_x - (width - self.free_rect.width()) / 2,
                top: self.free_rect.top + move_y - (height - self.free_rect.height()) / 2,
                right: self.free_rect.left + move_x - (width - self.free_rect.width()) / 2 + width,
                bottom: self.free_rect.top + move_y - (height - self.free_rect.height()) / 2
                    + height,
            },
            work,
        );
        position(self.target, self.free_rect);
    }

    fn end_free_move(&mut self, was_tap: bool, cancelled: bool) {
        if !cancelled && was_tap && self.config.center_enabled {
            self.center_target();
        }
        self.free_active = false;
        self.hide_hud();
        self.clear_target();
    }

    fn begin_axis_resize(&mut self, horizontal: bool) {
        self.hide_hud();
        self.hide_overlay();
        self.axis_horizontal = horizontal;
        self.axis_changed = false;
        self.axis_active = !self.target.is_null() && get_rect(self.target, &mut self.axis_rect);
        if self.axis_active && unsafe { IsZoomed(self.target) } != 0 {
            unsafe { ShowWindow(self.target, SW_RESTORE) };
        }
    }

    fn axis_resize(&mut self, factor: f64, horizontal: bool) {
        if !self.axis_active || self.target.is_null() || horizontal != self.axis_horizontal {
            return;
        }
        let Some(work) = monitor_work_area(self.target) else {
            return;
        };
        let mut width = self.axis_rect.width();
        let mut height = self.axis_rect.height();
        if horizontal {
            width = ((width as f64) * factor).round() as i32;
        } else {
            height = ((height as f64) * factor).round() as i32;
        }
        width = width.clamp(MIN_WINDOW_WIDTH.min(work.width()), work.width());
        height = height.clamp(MIN_WINDOW_HEIGHT.min(work.height()), work.height());
        self.axis_rect = clamp_rect(
            WindowRect {
                left: self.axis_rect.left - (width - self.axis_rect.width()) / 2,
                top: self.axis_rect.top - (height - self.axis_rect.height()) / 2,
                right: self.axis_rect.left - (width - self.axis_rect.width()) / 2 + width,
                bottom: self.axis_rect.top - (height - self.axis_rect.height()) / 2 + height,
            },
            work,
        );
        self.axis_changed = true;
        position(self.target, self.axis_rect);
    }

    fn end_axis_resize(&mut self, cancelled: bool) {
        if cancelled && self.axis_changed && self.have_original_rect {
            position(self.target, self.original_rect);
        }
        self.axis_active = false;
        self.axis_changed = false;
        self.hide_hud();
        self.clear_target();
    }

    fn maximize(&mut self) {
        if !self.target.is_null() && self.config.maximize_enabled {
            unsafe { ShowWindow(self.target, SW_MAXIMIZE) };
            self.hide_hud();
            self.clear_target();
        }
    }

    fn restore(&mut self) {
        if !self.target.is_null() {
            unsafe { ShowWindow(self.target, SW_RESTORE) };
            self.hide_hud();
            self.clear_target();
        }
    }

    fn center_target(&mut self) {
        let Some(rect) = get_window_rect(self.target) else {
            return;
        };
        let Some(work) = monitor_work_area(self.target) else {
            return;
        };
        let left = work.left + (work.width() - rect.width()) / 2;
        let top = work.top + (work.height() - rect.height()) / 2;
        position(
            self.target,
            WindowRect {
                left,
                top,
                right: left + rect.width(),
                bottom: top + rect.height(),
            },
        );
    }

    fn move_to_monitor(&mut self, direction: MonitorDirection) {
        if !self.monitor_mode || self.target.is_null() {
            return;
        }
        // Virtual desktop APIs are deliberately not called from the raw-input
        // callback.  Monitor movement is limited to the adjacent work area and
        // preserves the window's relative position.
        let Some(current) = get_window_rect(self.target) else {
            return;
        };
        let Some(source) = monitor_work_area(self.target) else {
            return;
        };
        let Some(target) = adjacent_work_area(source, direction) else {
            return;
        };
        let x = (current.left - source.left) as f64 / source.width().max(1) as f64;
        let y = (current.top - source.top) as f64 / source.height().max(1) as f64;
        let width = current.width().min(target.width());
        let height = current.height().min(target.height());
        let rect = clamp_rect(
            WindowRect {
                left: target.left + (x * target.width() as f64).round() as i32,
                top: target.top + (y * target.height() as f64).round() as i32,
                right: target.left + (x * target.width() as f64).round() as i32 + width,
                bottom: target.top + (y * target.height() as f64).round() as i32 + height,
            },
            target,
        );
        position(self.target, rect);
        unsafe { SetForegroundWindow(self.target) };
        self.hide_hud();
        self.clear_target();
    }

    fn move_virtual_desktop(&mut self, direction: DesktopDirection) {
        self.hide_hud();
        if self.config.create_desktop_on_overflow
            && matches!(direction, DesktopDirection::Right)
            && self.at_rightmost_desktop()
            && self.create_virtual_desktop()
        {
            return;
        }
        self.move_virtual_desktop_steps(match direction {
            DesktopDirection::Left => -1,
            DesktopDirection::Right => 1,
        });
    }

    fn commit_virtual_desktop_steps(&mut self, steps: i32) {
        self.hide_hud();
        if steps > 0
            && self.config.create_desktop_on_overflow
            && self.would_overflow_right(steps)
            && self.create_virtual_desktop()
        {
            return;
        }
        self.move_virtual_desktop_steps(steps);
    }

    fn at_rightmost_desktop(&self) -> bool {
        virtual_desktop::layout()
            .map(|(count, index)| count > 0 && index >= count - 1)
            .unwrap_or(false)
    }

    fn would_overflow_right(&self, steps: i32) -> bool {
        virtual_desktop::layout()
            .map(|(count, index)| count > 0 && index.saturating_add(steps) >= count)
            .unwrap_or(false)
    }

    fn create_virtual_desktop(&self) -> bool {
        if self.target.is_null() {
            return false;
        }
        let follow = self
            .hud
            .as_ref()
            .map(CursorHud::handle)
            .unwrap_or(std::ptr::null_mut());
        let created = virtual_desktop::move_to_new_desktop(self.target, follow);
        if created {
            unsafe { SetForegroundWindow(self.target) };
            self.logger.record("Created a virtual desktop on overflow.");
        } else {
            self.logger
                .record("Unable to create a virtual desktop on overflow.");
        }
        created
    }

    fn move_virtual_desktop_steps(&self, steps: i32) {
        if !self.target.is_null() {
            let direction = if steps < 0 { 3 } else { 4 };
            if virtual_desktop::move_by_steps(
                self.target,
                direction,
                steps.unsigned_abs().min(8) as i32,
            ) {
                unsafe { SetForegroundWindow(self.target) };
                return;
            }
            self.logger
                .record("Virtual desktop COM move failed; using Windows shortcut fallback.");
        }
        let key = if steps < 0 { VK_LEFT } else { VK_RIGHT };
        for _ in 0..steps.unsigned_abs().min(8) {
            unsafe {
                keybd_event(VK_LWIN as u8, 0, 0, 0);
                keybd_event(VK_CONTROL as u8, 0, 0, 0);
                keybd_event(key as u8, 0, 0, 0);
                keybd_event(key as u8, 0, KEYEVENTF_KEYUP, 0);
                keybd_event(VK_CONTROL as u8, 0, KEYEVENTF_KEYUP, 0);
                keybd_event(VK_LWIN as u8, 0, KEYEVENTF_KEYUP, 0);
            }
        }
    }

    fn original_was_maximized(&self) -> bool {
        self.original_was_maximized
    }

    fn hide_hud(&mut self) {
        if let Some(hud) = self.hud.as_mut() {
            hud.hide();
        }
    }

    fn show_app_hud(&mut self) {
        if self.app_windows.is_empty() {
            return;
        }
        let visible = 5_usize.min(self.app_windows.len());
        let start = self
            .app_selected
            .saturating_sub(visible / 2)
            .min(self.app_windows.len().saturating_sub(visible));
        let titles = self.app_windows[start..start + visible]
            .iter()
            .map(|window| window_title(*window))
            .collect::<Vec<_>>();
        if let Some(hud) = self.hud.as_mut() {
            hud.show_app_strip(titles, self.app_selected.saturating_sub(start));
        }
    }

    fn show_desktop_hud(&mut self, steps: i32) {
        let Some((count, current)) = virtual_desktop::layout() else {
            return;
        };
        if count <= 0 || current < 0 {
            return;
        }
        let count = count as usize;
        let current = current as usize;
        let aimed = current as i64 + steps as i64;
        let target = (0..count as i64).contains(&aimed).then_some(aimed as usize);
        let new_tile = (self.config.create_desktop_on_overflow && aimed >= count as i64)
            .then_some(count.min(9));
        let display_count = if new_tile.is_some() {
            (count + 1).min(10)
        } else {
            count.min(10)
        };
        if let Some(hud) = self.hud.as_mut() {
            hud.show_desktop_strip(display_count, current.min(9), target, new_tile);
        }
    }

    fn show_monitor_hud(&mut self, target: Option<MonitorDirection>) {
        if self.target.is_null() {
            return;
        }
        let Some(source) = monitor_work_area(self.target) else {
            return;
        };
        let up = adjacent_work_area(source, MonitorDirection::Up).is_some();
        let down = adjacent_work_area(source, MonitorDirection::Down).is_some();
        let left = adjacent_work_area(source, MonitorDirection::Left).is_some();
        let right = adjacent_work_area(source, MonitorDirection::Right).is_some();
        if let Some(hud) = self.hud.as_mut() {
            hud.show_monitor_map(up, down, left, right, target);
        }
    }

    fn clear_target(&mut self) {
        self.target = std::ptr::null_mut();
        self.have_original_rect = false;
        self.original_was_maximized = false;
        self.max_restored = false;
        self.live_moved = false;
        self.app_switch_active = false;
        self.app_windows.clear();
        self.down_latched = false;
        self.down_engaged = false;
        self.down_pick_close = false;
        self.down_peak_y = 0.0;
        self.down_max_y = 0.0;
        self.gesture_started = None;
        self.mouse_started = None;
        self.mouse_hold_engaged = false;
        self.mouse_hold_monitor = false;
        self.mouse_hold_direction = None;
        self.mouse_hold_steps = 0;
    }

    /// Resolve the same target Swoosh arms: a visible captioned root window with
    /// the pointer inside its DPI-aware titlebar band. Client-area gestures are
    /// intentionally ignored so browser tabs and document content keep their
    /// native touchpad behavior.
    fn target_under_cursor(&self) -> HWND {
        let mut point = POINT { x: 0, y: 0 };
        if unsafe { GetCursorPos(&mut point) } == 0 {
            return std::ptr::null_mut();
        }
        let child = unsafe { WindowFromPoint(point) };
        if child.is_null() {
            return std::ptr::null_mut();
        }
        let hwnd = unsafe { GetAncestor(child, GA_ROOT) };
        if !manageable(hwnd) || self.compatibility_blocks(hwnd) {
            return std::ptr::null_mut();
        }
        let mut rect = RECT::default();
        if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
            return std::ptr::null_mut();
        }
        let top = extended_frame_top(hwnd).unwrap_or(rect.top);
        let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
        let titlebar = (44_i64 * dpi as i64 / 96) as i32;
        if point.x < rect.left
            || point.x > rect.right
            || point.y < top
            || point.y > top.saturating_add(titlebar)
        {
            return std::ptr::null_mut();
        }
        hwnd
    }

    fn compatibility_blocks(&self, hwnd: HWND) -> bool {
        if self.config.app_compatibility_process_names.is_empty() {
            return false;
        }
        let process = process_name(hwnd);
        if process.is_empty()
            || !self
                .config
                .app_compatibility_process_names
                .iter()
                .any(|name| name.eq_ignore_ascii_case(&process))
        {
            return false;
        }
        match self.config.app_compatibility_mode {
            AppCompatibilityMode::Exclude => true,
            AppCompatibilityMode::RequireModifier => {
                !modifier_is_down(self.config.app_compatibility_modifier)
            }
        }
    }
}

impl Drop for PreviewOverlay {
    fn drop(&mut self) {
        if !self.hwnd.is_null() {
            unsafe { DestroyWindow(self.hwnd) };
        }
    }
}

fn create_preview_overlay() -> Option<PreviewOverlay> {
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
    if instance.is_null() {
        return None;
    }
    let class_name = wide(OVERLAY_CLASS);
    let class = WNDCLASSEXW {
        cbSize: mem::size_of::<WNDCLASSEXW>() as u32,
        style: 0,
        lpfnWndProc: Some(overlay_proc),
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
        // RegisterClassExW returns zero when another worker already registered
        // the same class; that is harmless for this process-local overlay.
        let _ = RegisterClassExW(&class);
    }
    let mut state = Box::new(OverlayState::default());
    let state_ptr = state.as_mut() as *mut OverlayState;
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
            state_ptr.cast::<c_void>(),
        )
    };
    if hwnd.is_null() {
        return None;
    }
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize);
        SetLayeredWindowAttributes(hwnd, 0, state.alpha, LWA_ALPHA);
        ShowWindow(hwnd, windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE);
    }
    Some(PreviewOverlay { hwnd, state })
}

unsafe extern "system" fn overlay_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_NCHITTEST => HTTRANSPARENT as LRESULT,
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let hdc = unsafe { BeginPaint(hwnd, &mut paint) };
            let mut rect = RECT::default();
            unsafe { GetWindowRect(hwnd, &mut rect) };
            // WM_PAINT coordinates are client-local.
            let width = rect.right.saturating_sub(rect.left).max(1);
            let height = rect.bottom.saturating_sub(rect.top).max(1);
            rect.left = 0;
            rect.top = 0;
            rect.right = width;
            rect.bottom = height;
            let state = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut OverlayState };
            if !state.is_null() {
                let fill = unsafe { CreateSolidBrush((*state).color) };
                let border = unsafe { CreateSolidBrush((*state).border) };
                unsafe {
                    FillRect(hdc, &rect, fill);
                    FrameRect(hdc, &rect, border);
                    DeleteObject(fill.cast());
                    DeleteObject(border.cast());
                }
            }
            unsafe { EndPaint(hwnd, &paint) };
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn colorref_from_hex(value: &str) -> u32 {
    let trimmed = value.trim().trim_start_matches('#');
    if trimmed.len() != 6 || !trimmed.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return 0x00FF840A;
    }
    let red = u32::from_str_radix(&trimmed[0..2], 16).unwrap_or(10);
    let green = u32::from_str_radix(&trimmed[2..4], 16).unwrap_or(132);
    let blue = u32::from_str_radix(&trimmed[4..6], 16).unwrap_or(255);
    red | (green << 8) | (blue << 16)
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn manageable(hwnd: HWND) -> bool {
    if hwnd.is_null() {
        return false;
    }
    unsafe {
        if IsWindowVisible(hwnd) == 0
            || (GetWindowLongPtrW(hwnd, GWL_STYLE) as u32 & WS_CAPTION) == 0
        {
            return false;
        }
        let mut class = [0_u16; 128];
        let length = GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32);
        if length <= 0 {
            return true;
        }
        let class = String::from_utf16_lossy(&class[..length as usize]);
        !matches!(
            class.as_str(),
            "Progman"
                | "WorkerW"
                | "Shell_TrayWnd"
                | "Windows.UI.Core.CoreWindow"
                | "XamlExplorerHostIslandWindow"
        )
    }
}

fn enumerate_switchable_windows() -> Vec<HWND> {
    let mut windows = Vec::new();
    unsafe {
        EnumWindows(
            Some(enum_switchable_window),
            &mut windows as *mut Vec<HWND> as LPARAM,
        );
    }
    windows
}

fn window_title(hwnd: HWND) -> String {
    let length = unsafe { GetWindowTextLengthW(hwnd) };
    if length <= 0 {
        return "窗口".to_owned();
    }
    let mut title = vec![0_u16; length as usize + 1];
    let copied = unsafe { GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32) };
    if copied <= 0 {
        "窗口".to_owned()
    } else {
        String::from_utf16_lossy(&title[..copied as usize])
    }
}

unsafe extern "system" fn enum_switchable_window(hwnd: HWND, lparam: LPARAM) -> i32 {
    if hwnd.is_null() || unsafe { IsWindowVisible(hwnd) } == 0 || !manageable(hwnd) {
        return 1;
    }
    let length = unsafe { GetWindowTextLengthW(hwnd) };
    if length <= 0 {
        return 1;
    }
    let mut title = vec![0_u16; length as usize + 1];
    if unsafe { GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32) } <= 0 {
        return 1;
    }
    unsafe { (&mut *(lparam as *mut Vec<HWND>)).push(hwnd) };
    1
}

fn extended_frame_top(hwnd: HWND) -> Option<i32> {
    let mut frame = RECT::default();
    let result = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS as u32,
            (&mut frame as *mut RECT).cast(),
            mem::size_of::<RECT>() as u32,
        )
    };
    if result >= 0 {
        Some(frame.top)
    } else {
        None
    }
}

fn process_name(hwnd: HWND) -> String {
    let mut pid = 0_u32;
    if unsafe { GetWindowThreadProcessId(hwnd, &mut pid) } == 0 || pid == 0 {
        return String::new();
    }
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return String::new();
    }
    let mut buffer = [0_u16; 1024];
    let mut length = buffer.len() as u32;
    let ok = unsafe {
        QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, buffer.as_mut_ptr(), &mut length)
    } != 0;
    unsafe { CloseHandle(handle) };
    if !ok {
        return String::new();
    }
    String::from_utf16_lossy(&buffer[..length as usize])
        .rsplit(['\\', '/'])
        .next()
        .map(normalize_process_name)
        .unwrap_or_default()
}

fn modifier_is_down(modifier: GridModifier) -> bool {
    let key = match modifier {
        GridModifier::Shift => VK_SHIFT,
        GridModifier::Ctrl => VK_CONTROL,
        GridModifier::Alt => VK_MENU,
    };
    unsafe { GetAsyncKeyState(key as i32) < 0 }
}

fn get_rect(hwnd: HWND, output: &mut WindowRect) -> bool {
    let Some(rect) = get_window_rect(hwnd) else {
        return false;
    };
    *output = rect;
    true
}

fn get_window_rect(hwnd: HWND) -> Option<WindowRect> {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
        return None;
    }
    Some(WindowRect {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    })
}

fn position(hwnd: HWND, rect: WindowRect) {
    unsafe {
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            rect.left,
            rect.top,
            rect.width().max(1),
            rect.height().max(1),
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

fn animate_position(
    hwnd: HWND,
    start: WindowRect,
    target: WindowRect,
    seconds: f64,
    generation: Arc<AtomicU64>,
    expected_generation: u64,
) {
    let hwnd = hwnd as isize;
    let duration = seconds.clamp(0.05, 0.4);
    std::thread::spawn(move || {
        let started = std::time::Instant::now();
        loop {
            if generation.load(Ordering::Relaxed) != expected_generation {
                break;
            }
            let progress = (started.elapsed().as_secs_f64() / duration).clamp(0.0, 1.0);
            let eased = 1.0 - (1.0 - progress).powi(3);
            let rect = WindowRect {
                left: lerp(start.left, target.left, eased),
                top: lerp(start.top, target.top, eased),
                right: lerp(start.right, target.right, eased),
                bottom: lerp(start.bottom, target.bottom, eased),
            };
            position(hwnd as HWND, rect);
            if progress >= 1.0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(8));
        }
    });
}

fn lerp(start: i32, target: i32, progress: f64) -> i32 {
    (start as f64 + (target - start) as f64 * progress).round() as i32
}

fn round_away(value: f64) -> i32 {
    if value >= 0.0 {
        (value + 0.5).floor() as i32
    } else {
        (value - 0.5).ceil() as i32
    }
}

fn monitor_work_area(hwnd: HWND) -> Option<WindowRect> {
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_null() {
        return None;
    }
    let mut info = MONITORINFO {
        cbSize: mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
        return None;
    }
    Some(WindowRect {
        left: info.rcWork.left,
        top: info.rcWork.top,
        right: info.rcWork.right,
        bottom: info.rcWork.bottom,
    })
}

fn restore_rect(hwnd: HWND) -> Option<WindowRect> {
    let mut placement = WINDOWPLACEMENT {
        length: mem::size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    if unsafe { GetWindowPlacement(hwnd, &mut placement) } == 0 {
        return None;
    }
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let mut offset_x = 0;
    let mut offset_y = 0;
    if !monitor.is_null() {
        let mut info = MONITORINFO {
            cbSize: mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if unsafe { GetMonitorInfoW(monitor, &mut info) } != 0 {
            offset_x = info.rcWork.left - info.rcMonitor.left;
            offset_y = info.rcWork.top - info.rcMonitor.top;
        }
    }
    Some(WindowRect {
        left: placement.rcNormalPosition.left + offset_x,
        top: placement.rcNormalPosition.top + offset_y,
        right: placement.rcNormalPosition.right + offset_x,
        bottom: placement.rcNormalPosition.bottom + offset_y,
    })
}

fn rect_fraction(rect: WindowRect, work: WindowRect) -> (f64, f64, f64, f64) {
    let width = work.width().max(1) as f64;
    let height = work.height().max(1) as f64;
    (
        ((rect.left - work.left) as f64 / width).clamp(0.0, 1.0),
        ((rect.top - work.top) as f64 / height).clamp(0.0, 1.0),
        ((rect.right - work.left) as f64 / width).clamp(0.0, 1.0),
        ((rect.bottom - work.top) as f64 / height).clamp(0.0, 1.0),
    )
}

fn minimize_hint(work: WindowRect) -> WindowRect {
    let width = (work.width() / 4).max(1);
    let height = 48.min(work.height().max(1));
    let left = work.left + (work.width() - width) / 2;
    let top = (work.bottom - height - 8).max(work.top);
    WindowRect {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}

fn rect_from_zone(rect: ZoneRect) -> WindowRect {
    WindowRect {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    }
}

fn clamp_rect(mut rect: WindowRect, work: WindowRect) -> WindowRect {
    let width = rect.width().min(work.width());
    let height = rect.height().min(work.height());
    rect.right = rect.left + width;
    rect.bottom = rect.top + height;
    if rect.left < work.left {
        rect.left = work.left;
        rect.right = rect.left + width;
    }
    if rect.top < work.top {
        rect.top = work.top;
        rect.bottom = rect.top + height;
    }
    if rect.right > work.right {
        rect.right = work.right;
        rect.left = rect.right - width;
    }
    if rect.bottom > work.bottom {
        rect.bottom = work.bottom;
        rect.top = rect.bottom - height;
    }
    rect
}

fn move_cursor_with_window(before: WindowRect, after: WindowRect) {
    let mut point = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut point) } == 0 {
        return;
    }
    let dx = point.x - before.left;
    let dy = point.y - before.top;
    let x = (after.left + dx).clamp(after.left, after.right.saturating_sub(1));
    let y = (after.top + dy).clamp(after.top, after.bottom.saturating_sub(1));
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::SetCursorPos(x, y);
    }
}

fn adjacent_work_area(source: WindowRect, direction: MonitorDirection) -> Option<WindowRect> {
    // The raw-input worker does not own a monitor cache.  For the common
    // two-monitor case, query the neighbouring monitor through the cursor
    // position by nudging a point outside the source work area.
    let mut point = POINT {
        x: source.center_x() as i32,
        y: source.center_y() as i32,
    };
    match direction {
        MonitorDirection::Left => point.x = source.left.saturating_sub(8),
        MonitorDirection::Right => point.x = source.right.saturating_add(8),
        MonitorDirection::Up => point.y = source.top.saturating_sub(8),
        MonitorDirection::Down => point.y = source.bottom.saturating_add(8),
    }
    let hwnd = unsafe { WindowFromPoint(point) };
    if hwnd.is_null() {
        return None;
    }
    monitor_work_area(unsafe { GetAncestor(hwnd, GA_ROOT) })
}
