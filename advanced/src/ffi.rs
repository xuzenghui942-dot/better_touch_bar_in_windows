use std::ffi::{CStr, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::slice;

use crate::config::AdvancedConfig;
use crate::gesture::{
    Contact, DesktopDirection, GestureEngine, GestureEvent, MonitorDirection, SwipeDirection,
    TouchFrame,
};

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AdvancedContact {
    pub id: i32,
    pub x: f64,
    pub y: f64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct AdvancedEvent {
    pub kind: i32,
    pub direction: i32,
    pub value1: f64,
    pub value2: f64,
    pub integer: i32,
}

pub type AdvancedEventCallback = unsafe extern "C" fn(*mut c_void, AdvancedEvent);

struct NativeEngine {
    config: AdvancedConfig,
    engine: GestureEngine,
    callback: Option<AdvancedEventCallback>,
    callback_context: *mut c_void,
}

impl NativeEngine {
    fn new(
        config: AdvancedConfig,
        callback: Option<AdvancedEventCallback>,
        callback_context: *mut c_void,
    ) -> Self {
        let mut value = Self {
            config: config.normalized(),
            engine: GestureEngine::default(),
            callback,
            callback_context,
        };
        value.apply_config();
        value
    }

    fn apply_config(&mut self) {
        self.engine.enabled = self.config.enabled;
        self.engine.idle_cancel_ms = (self.config.cancel_timeout_seconds * 1000.0).round() as i64;
        self.engine.hold_delay_ms =
            (self.config.desktop_hold_delay_seconds * 1000.0).round() as i64;
        self.engine.desktop_move_on_release =
            self.config.app_switch_on_hold || self.config.preview_desktop_destination;
        self.engine.five_finger_enabled = self.config.five_finger_enabled;
        self.engine.axis_resize_h = self.config.resize_horizontal_enabled;
        self.engine.axis_resize_v = self.config.resize_vertical_enabled;
        if !self.config.enabled {
            let _ = self.engine.reset();
        }
    }

    fn emit(&self, event: GestureEvent) {
        let Some(callback) = self.callback else {
            return;
        };
        let event = encode_event(event);
        // SAFETY: callback and context are supplied together by the managed owner,
        // which keeps the delegate rooted until advanced_destroy returns.
        unsafe { callback(self.callback_context, event) };
    }
}

/// Event kind values consumed by AdvancedGestureNative.cs.
mod event_kind {
    pub const BEGAN: i32 = 1;
    pub const RAW: i32 = 2;
    pub const UPDATED: i32 = 3;
    pub const COMPLETED: i32 = 4;
    pub const CANCELLED: i32 = 5;
    pub const HOLD_ENGAGED: i32 = 6;
    pub const HOLD_UPDATED: i32 = 7;
    pub const DESKTOP_MOVE: i32 = 8;
    pub const DESKTOP_HOLD_COMMIT: i32 = 9;
    pub const MONITOR_UPDATED: i32 = 10;
    pub const MONITOR_MOVE: i32 = 11;
    pub const FREE_MOVE_BEGAN: i32 = 12;
    pub const FREE_MOVE_DELTA: i32 = 13;
    pub const FREE_MOVE_ENDED: i32 = 14;
    pub const PINCH_UPDATED: i32 = 15;
    pub const PINCH_OUT: i32 = 16;
    pub const PINCH_IN: i32 = 17;
    pub const AXIS_RESIZE_BEGAN: i32 = 18;
    pub const AXIS_RESIZE_DELTA: i32 = 19;
    pub const AXIS_RESIZE_ENDED: i32 = 20;
}

fn encode_event(event: GestureEvent) -> AdvancedEvent {
    use event_kind::*;
    match event {
        GestureEvent::Began { contacts } => AdvancedEvent {
            kind: BEGAN,
            integer: contacts as i32,
            ..Default::default()
        },
        GestureEvent::Raw { dx, dy } => AdvancedEvent {
            kind: RAW,
            value1: dx,
            value2: dy,
            ..Default::default()
        },
        GestureEvent::Updated {
            direction,
            progress,
        } => AdvancedEvent {
            kind: UPDATED,
            direction: swipe_code(direction),
            value1: progress,
            ..Default::default()
        },
        GestureEvent::Completed(direction) => AdvancedEvent {
            kind: COMPLETED,
            direction: swipe_code(direction),
            ..Default::default()
        },
        GestureEvent::Cancelled => AdvancedEvent {
            kind: CANCELLED,
            ..Default::default()
        },
        GestureEvent::HoldEngaged => AdvancedEvent {
            kind: HOLD_ENGAGED,
            ..Default::default()
        },
        GestureEvent::HoldUpdated {
            direction,
            progress,
            aim_steps,
        } => AdvancedEvent {
            kind: HOLD_UPDATED,
            direction: desktop_code(direction),
            value1: progress,
            integer: aim_steps,
            ..Default::default()
        },
        GestureEvent::DesktopMove(direction) => AdvancedEvent {
            kind: DESKTOP_MOVE,
            direction: desktop_code(Some(direction)),
            ..Default::default()
        },
        GestureEvent::DesktopHoldCommit(steps) => AdvancedEvent {
            kind: DESKTOP_HOLD_COMMIT,
            integer: steps,
            ..Default::default()
        },
        GestureEvent::MonitorMoveUpdated {
            direction,
            progress,
        } => AdvancedEvent {
            kind: MONITOR_UPDATED,
            direction: monitor_code(direction),
            value1: progress,
            ..Default::default()
        },
        GestureEvent::MonitorMove(direction) => AdvancedEvent {
            kind: MONITOR_MOVE,
            direction: monitor_code(Some(direction)),
            ..Default::default()
        },
        GestureEvent::FreeMoveBegan => AdvancedEvent {
            kind: FREE_MOVE_BEGAN,
            ..Default::default()
        },
        GestureEvent::FreeMoveDelta { dx, dy, scale } => AdvancedEvent {
            kind: FREE_MOVE_DELTA,
            value1: dx,
            value2: dy,
            integer: (scale * 1_000_000.0).round() as i32,
            ..Default::default()
        },
        GestureEvent::FreeMoveEnded { was_tap, cancelled } => AdvancedEvent {
            kind: FREE_MOVE_ENDED,
            direction: i32::from(cancelled),
            integer: i32::from(was_tap),
            ..Default::default()
        },
        GestureEvent::PinchUpdated { outward, progress } => AdvancedEvent {
            kind: PINCH_UPDATED,
            direction: i32::from(outward),
            value1: progress,
            ..Default::default()
        },
        GestureEvent::PinchOut => AdvancedEvent {
            kind: PINCH_OUT,
            ..Default::default()
        },
        GestureEvent::PinchIn => AdvancedEvent {
            kind: PINCH_IN,
            ..Default::default()
        },
        GestureEvent::AxisResizeBegan { horizontal } => AdvancedEvent {
            kind: AXIS_RESIZE_BEGAN,
            direction: i32::from(horizontal),
            ..Default::default()
        },
        GestureEvent::AxisResizeDelta { factor, horizontal } => AdvancedEvent {
            kind: AXIS_RESIZE_DELTA,
            direction: i32::from(horizontal),
            value1: factor,
            ..Default::default()
        },
        GestureEvent::AxisResizeEnded { cancelled } => AdvancedEvent {
            kind: AXIS_RESIZE_ENDED,
            direction: i32::from(cancelled),
            ..Default::default()
        },
    }
}

fn swipe_code(direction: SwipeDirection) -> i32 {
    match direction {
        SwipeDirection::None => 0,
        SwipeDirection::Left => 1,
        SwipeDirection::Right => 2,
        SwipeDirection::Up => 3,
        SwipeDirection::Down => 4,
        SwipeDirection::UpLeft => 5,
        SwipeDirection::UpRight => 6,
        SwipeDirection::DownLeft => 7,
        SwipeDirection::DownRight => 8,
    }
}
fn desktop_code(direction: Option<DesktopDirection>) -> i32 {
    match direction {
        None => 0,
        Some(DesktopDirection::Left) => 1,
        Some(DesktopDirection::Right) => 2,
    }
}
fn monitor_code(direction: Option<MonitorDirection>) -> i32 {
    match direction {
        None => 0,
        Some(MonitorDirection::Left) => 1,
        Some(MonitorDirection::Right) => 2,
        Some(MonitorDirection::Up) => 3,
        Some(MonitorDirection::Down) => 4,
    }
}

fn parse_config(json: *const c_char) -> Option<AdvancedConfig> {
    if json.is_null() {
        return Some(AdvancedConfig::default());
    }
    // SAFETY: callers pass a NUL-terminated UTF-8 buffer for the duration of the call.
    let text = unsafe { CStr::from_ptr(json) }.to_str().ok()?;
    serde_json::from_str::<AdvancedConfig>(text)
        .ok()
        .map(AdvancedConfig::normalized)
}

#[unsafe(no_mangle)]
pub extern "C" fn advanced_create(
    config_json: *const c_char,
    callback: Option<AdvancedEventCallback>,
    callback_context: *mut c_void,
) -> *mut c_void {
    catch_unwind(|| {
        let config = parse_config(config_json)?;
        Some(
            Box::into_raw(Box::new(NativeEngine::new(
                config,
                callback,
                callback_context,
            )))
            .cast(),
        )
    })
    .ok()
    .flatten()
    .unwrap_or(std::ptr::null_mut())
}

#[unsafe(no_mangle)]
pub extern "C" fn advanced_update_config(handle: *mut c_void, config_json: *const c_char) -> bool {
    catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() {
            return false;
        }
        let Some(config) = parse_config(config_json) else {
            return false;
        };
        // SAFETY: handle is returned by advanced_create and owned until destroy.
        let native = unsafe { &mut *handle.cast::<NativeEngine>() };
        native.config = config;
        native.apply_config();
        true
    }))
    .unwrap_or(false)
}

#[unsafe(no_mangle)]
pub extern "C" fn advanced_set_modifier_modes(handle: *mut c_void, thirds: bool, monitor: bool) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() {
            return;
        }
        // SAFETY: handle is returned by advanced_create and owned until destroy.
        let native = unsafe { &mut *handle.cast::<NativeEngine>() };
        native.engine.thirds_mode = thirds && native.config.grid_modifier_enabled;
        native.engine.monitor_move_mode = monitor && native.config.monitor_move_enabled;
    }));
}

#[unsafe(no_mangle)]
/// Process one normalized touch frame.
///
/// # Safety
/// When `contact_count` is non-zero, `contacts` must point to a readable array
/// containing at least that many `AdvancedContact` values for the duration of
/// this call. `handle` must be a live handle returned by `advanced_create`.
pub unsafe extern "C" fn advanced_process_frame(
    handle: *mut c_void,
    contacts: *const AdvancedContact,
    contact_count: usize,
    timestamp_ms: i64,
) -> bool {
    catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() || (contacts.is_null() && contact_count != 0) {
            return false;
        }
        // SAFETY: managed caller pins an array with contact_count entries for this call.
        let input = if contact_count == 0 {
            &[]
        } else {
            unsafe { slice::from_raw_parts(contacts, contact_count) }
        };
        let frame = TouchFrame {
            timestamp_ms,
            contacts: input
                .iter()
                .map(|c| Contact {
                    id: c.id,
                    x: c.x,
                    y: c.y,
                })
                .collect(),
        };
        // SAFETY: handle is returned by advanced_create and calls are serialized by host.
        let native = unsafe { &mut *handle.cast::<NativeEngine>() };
        for event in native.engine.process(&frame) {
            native.emit(event);
        }
        true
    }))
    .unwrap_or(false)
}

#[unsafe(no_mangle)]
pub extern "C" fn advanced_cancel(handle: *mut c_void) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() {
            return;
        }
        // SAFETY: handle is returned by advanced_create and owned until destroy.
        let native = unsafe { &mut *handle.cast::<NativeEngine>() };
        for event in native.engine.cancel() {
            native.emit(event);
        }
    }));
}

#[unsafe(no_mangle)]
pub extern "C" fn advanced_destroy(handle: *mut c_void) {
    if handle.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: ownership of the unique Box is returned exactly once by the host.
        unsafe { drop(Box::from_raw(handle.cast::<NativeEngine>())) };
    }));
}
