//! Safe Rust-side integration for the imported AdvancedGestures engine.
//!
//! The advanced state machine is kept as a normal workspace dependency.  This
//! module owns the small amount of glue needed by the raw-input worker: loading
//! its independent JSON file, normalising HID coordinates and applying the
//! settings that the state machine exposes.

use std::{
    fs,
    io::{self, Write},
    path::Path,
};

use better_touch_advanced_gestures::{
    config::AdvancedConfig,
    gesture::{
        Contact as AdvancedContact, GestureEngine, GestureEvent, SwipeDirection, TouchFrame,
    },
};
use serde::{Deserialize, Serialize};

pub use better_touch_advanced_gestures::config::{GridModifier, SwipeDownMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdvancedEventKind {
    Began,
    Raw,
    Updated,
    Completed,
    Cancelled,
    HoldEngaged,
    HoldUpdated,
    DesktopMove,
    DesktopHoldCommit,
    MonitorMoveUpdated,
    MonitorMove,
    FreeMoveBegan,
    FreeMoveDelta,
    FreeMoveEnded,
    PinchUpdated,
    PinchOut,
    PinchIn,
    AxisResizeBegan,
    AxisResizeDelta,
    AxisResizeEnded,
}

impl AdvancedEventKind {
    pub fn of(event: &GestureEvent) -> Self {
        use AdvancedEventKind::*;
        match event {
            GestureEvent::Began { .. } => Began,
            GestureEvent::Raw { .. } => Raw,
            GestureEvent::Updated { .. } => Updated,
            GestureEvent::Completed(_) => Completed,
            GestureEvent::Cancelled => Cancelled,
            GestureEvent::HoldEngaged => HoldEngaged,
            GestureEvent::HoldUpdated { .. } => HoldUpdated,
            GestureEvent::DesktopMove(_) => DesktopMove,
            GestureEvent::DesktopHoldCommit(_) => DesktopHoldCommit,
            GestureEvent::MonitorMoveUpdated { .. } => MonitorMoveUpdated,
            GestureEvent::MonitorMove(_) => MonitorMove,
            GestureEvent::FreeMoveBegan => FreeMoveBegan,
            GestureEvent::FreeMoveDelta { .. } => FreeMoveDelta,
            GestureEvent::FreeMoveEnded { .. } => FreeMoveEnded,
            GestureEvent::PinchUpdated { .. } => PinchUpdated,
            GestureEvent::PinchOut => PinchOut,
            GestureEvent::PinchIn => PinchIn,
            GestureEvent::AxisResizeBegan { .. } => AxisResizeBegan,
            GestureEvent::AxisResizeDelta { .. } => AxisResizeDelta,
            GestureEvent::AxisResizeEnded { .. } => AxisResizeEnded,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AdvancedRuntimeStatus {
    pub enabled: bool,
    pub available: bool,
    pub processed_frames: u64,
    pub emitted_events: u64,
    pub last_event: Option<AdvancedEventKind>,
    pub last_error: Option<String>,
    pub completed_swooshes: u64,
}

impl Default for AdvancedRuntimeStatus {
    fn default() -> Self {
        Self {
            enabled: false,
            available: true,
            processed_frames: 0,
            emitted_events: 0,
            last_event: None,
            last_error: None,
            completed_swooshes: 0,
        }
    }
}

/// Runtime state for one raw-input worker.  It is deliberately independent
/// from [`crate::gesture::DragEngine`], so changing advanced settings or a
/// malformed advanced frame cannot corrupt the basic three-finger path.
#[derive(Debug)]
pub struct AdvancedRuntime {
    config: AdvancedConfig,
    engine: GestureEngine,
    status: AdvancedRuntimeStatus,
}

impl AdvancedRuntime {
    pub fn new(config: AdvancedConfig) -> Self {
        let mut runtime = Self {
            config: config.normalized(),
            engine: GestureEngine::default(),
            status: AdvancedRuntimeStatus::default(),
        };
        runtime.apply_config();
        runtime
    }

    pub fn config(&self) -> &AdvancedConfig {
        &self.config
    }

    pub fn status(&self) -> AdvancedRuntimeStatus {
        self.status.clone()
    }

    pub fn set_config(&mut self, config: AdvancedConfig) {
        self.config = config.normalized();
        self.apply_config();
    }

    pub fn set_modifier_modes(&mut self, thirds: bool, monitor: bool) {
        self.engine.thirds_mode = thirds && self.config.grid_modifier_enabled;
        self.engine.monitor_move_mode = monitor && self.config.monitor_move_enabled;
    }

    pub fn process(
        &mut self,
        contacts: &[(i32, i32, i32)],
        ranges: AxisRanges,
        timestamp_ms: i64,
    ) -> Vec<GestureEvent> {
        if !self.config.enabled || !self.config.gestures_enabled || !self.status.available {
            return Vec::new();
        }

        self.status.processed_frames = self.status.processed_frames.saturating_add(1);
        let frame = TouchFrame {
            timestamp_ms,
            contacts: contacts
                .iter()
                .map(|(id, x, y)| AdvancedContact {
                    id: *id,
                    x: ranges.normalize_x(*x),
                    y: ranges.normalize_y(*y),
                })
                .collect(),
        };
        let events = self.engine.process(&frame);
        self.status.emitted_events = self
            .status
            .emitted_events
            .saturating_add(events.len() as u64);
        self.status.completed_swooshes = self.status.completed_swooshes.saturating_add(
            events
                .iter()
                .filter(|event| self.counts_as_swoosh(event))
                .count() as u64,
        );
        self.status.last_event = events.last().map(AdvancedEventKind::of);
        events
    }

    pub fn cancel(&mut self) -> Vec<GestureEvent> {
        let events = self.engine.cancel();
        self.status.emitted_events = self
            .status
            .emitted_events
            .saturating_add(events.len() as u64);
        self.status.last_event = events.last().map(AdvancedEventKind::of);
        events
    }

    pub fn rebaseline(&mut self) {
        self.engine.rebaseline();
    }

    pub fn rebaseline_seed(&mut self, direction: SwipeDirection) {
        self.engine.rebaseline_seed(direction);
    }

    pub fn disable_after_error(&mut self, error: impl Into<String>) {
        self.status.available = false;
        self.status.enabled = false;
        self.status.last_error = Some(error.into());
        self.config.enabled = false;
        self.engine.enabled = false;
        let _ = self.engine.reset();
    }

    fn apply_config(&mut self) {
        self.status.enabled = self.config.enabled && self.config.gestures_enabled;
        self.status.available = true;
        self.status.last_error = None;
        self.engine.enabled = self.config.enabled && self.config.gestures_enabled;
        self.engine.idle_cancel_ms = (self.config.cancel_timeout_seconds * 1000.0).round() as i64;
        self.engine.hold_delay_ms =
            (self.config.desktop_hold_delay_seconds * 1000.0).round() as i64;
        self.engine.desktop_move_on_release =
            self.config.app_switch_on_hold || self.config.preview_desktop_destination;
        self.engine.five_finger_enabled = self.config.five_finger_enabled;
        self.engine.axis_resize_h = self.config.resize_horizontal_enabled;
        self.engine.axis_resize_v = self.config.resize_vertical_enabled;
        self.engine.thirds_dead_zone = if self.config.grid_modifier_enabled {
            0.03
        } else {
            0.055
        };
        self.engine.commit_distance = 0.12;
        self.engine.dead_zone = 0.055;
        self.engine.pinch_engage_delta = 0.10;
        self.engine.pinch_engage_ratio = 1.45;
        self.engine.pinch_max_centroid_travel = 0.06;
        self.engine.pinch_preview_delta = 0.035;
        if !self.config.enabled {
            let _ = self.engine.reset();
        }
    }

    fn counts_as_swoosh(&self, event: &GestureEvent) -> bool {
        match event {
            GestureEvent::Completed(_)
            | GestureEvent::DesktopMove(_)
            | GestureEvent::MonitorMove(_)
            | GestureEvent::PinchOut
            | GestureEvent::PinchIn => true,
            GestureEvent::DesktopHoldCommit(steps) => *steps != 0,
            GestureEvent::AxisResizeEnded { cancelled } => !cancelled,
            GestureEvent::FreeMoveEnded { was_tap, cancelled } => {
                *was_tap && !cancelled && self.config.center_enabled
            }
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AxisRanges {
    pub min_x: i32,
    pub max_x: i32,
    pub min_y: i32,
    pub max_y: i32,
}

impl Default for AxisRanges {
    fn default() -> Self {
        Self {
            min_x: 0,
            max_x: 32_767,
            min_y: 0,
            max_y: 32_767,
        }
    }
}

impl AxisRanges {
    pub fn normalize_x(self, value: i32) -> f64 {
        normalize(value, self.min_x, self.max_x)
    }

    pub fn normalize_y(self, value: i32) -> f64 {
        normalize(value, self.min_y, self.max_y)
    }
}

fn normalize(value: i32, minimum: i32, maximum: i32) -> f64 {
    if maximum <= minimum {
        return 0.0;
    }
    ((value - minimum) as f64 / (maximum - minimum) as f64).clamp(0.0, 1.0)
}

pub fn load_or_default(path: &Path) -> (AdvancedConfig, bool) {
    match fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<AdvancedConfig>(&text).ok())
    {
        Some(value) => (value.normalized(), false),
        None => (AdvancedConfig::default(), true),
    }
}

pub fn save(config: &AdvancedConfig, path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let serialized = serde_json::to_vec_pretty(&config.clone().normalized())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let temporary_path = path.with_extension("json.tmp");
    let mut temporary = fs::File::create(&temporary_path)?;
    temporary.write_all(&serialized)?;
    temporary.sync_all()?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(temporary_path, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_uses_hid_ranges_and_never_emits_non_finite_values() {
        let ranges = AxisRanges {
            min_x: 100,
            max_x: 1_100,
            min_y: 200,
            max_y: 1_200,
        };
        assert_eq!(ranges.normalize_x(100), 0.0);
        assert_eq!(ranges.normalize_x(600), 0.5);
        assert_eq!(ranges.normalize_y(1_400), 1.0);
    }

    #[test]
    fn advanced_settings_round_trip_is_independent_from_basic_settings() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("advanced-gestures.json");
        let config = AdvancedConfig {
            enabled: true,
            grid_spacing: 7,
            ..AdvancedConfig::default()
        };
        save(&config, &path).unwrap();
        let (loaded, fresh) = load_or_default(&path);
        assert!(!fresh);
        assert_eq!(loaded.enabled, config.enabled);
        assert_eq!(loaded.grid_spacing, config.grid_spacing);
    }
}
