use std::{
    collections::BTreeMap,
    fs,
    io::{self, Write},
    path::Path,
};

use serde::{Deserialize, Serialize};

pub const CURRENT_SETTINGS_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DragButton {
    None,
    #[default]
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingStartupAction {
    #[default]
    None,
    EnableElevatedRunWithStartup,
    DisableElevatedRunWithStartup,
    EnableElevatedStartup,
    DisableElevatedStartup,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceDragSettings {
    pub cursor_move: bool,
    pub cursor_speed: f32,
    pub cursor_acceleration: f32,
}

impl Default for DeviceDragSettings {
    fn default() -> Self {
        Self {
            cursor_move: true,
            cursor_speed: 30.0,
            cursor_acceleration: 10.0,
        }
    }
}

impl DeviceDragSettings {
    pub fn normalize(&mut self) {
        if !self.cursor_speed.is_finite() {
            self.cursor_speed = 30.0;
        }
        if !self.cursor_acceleration.is_finite() {
            self.cursor_acceleration = 10.0;
        }
        self.cursor_speed = self.cursor_speed.clamp(0.0, 100_000.0);
        self.cursor_acceleration = self.cursor_acceleration.clamp(0.0, 1_000.0);
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub version: u32,
    pub three_finger_drag: bool,
    pub drag_button: DragButton,
    pub allow_release_and_restart: bool,
    pub release_delay_ms: u32,
    pub devices: BTreeMap<String, DeviceDragSettings>,
    pub cursor_averaging: u32,
    pub max_finger_move_distance: u32,
    pub start_threshold: u32,
    pub stop_threshold: u32,
    pub run_at_startup: bool,
    pub run_elevated: bool,
    pub record_logs: bool,
    pub pending_startup_action: PendingStartupAction,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            version: CURRENT_SETTINGS_VERSION,
            three_finger_drag: true,
            drag_button: DragButton::Left,
            allow_release_and_restart: true,
            release_delay_ms: 500,
            devices: BTreeMap::new(),
            cursor_averaging: 1,
            max_finger_move_distance: 0,
            start_threshold: 100,
            stop_threshold: 10,
            run_at_startup: false,
            run_elevated: true,
            record_logs: false,
            pending_startup_action: PendingStartupAction::None,
        }
    }
}

impl AppSettings {
    pub fn normalize(&mut self) {
        self.version = CURRENT_SETTINGS_VERSION;
        self.release_delay_ms = self.release_delay_ms.min(2_000);
        self.cursor_averaging = self.cursor_averaging.clamp(1, 10);
        self.max_finger_move_distance = self.max_finger_move_distance.min(10_000);
        self.start_threshold = self.start_threshold.min(1_000);
        self.stop_threshold = self.stop_threshold.min(1_000);
        if self.stop_threshold > self.start_threshold {
            self.start_threshold = self.stop_threshold;
        }
        self.devices
            .values_mut()
            .for_each(DeviceDragSettings::normalize);
    }

    pub fn load(path: &Path) -> io::Result<Self> {
        let serialized = fs::read_to_string(path)?;
        let mut settings: Self = serde_json::from_str(&serialized)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        settings.normalize();
        Ok(settings)
    }

    pub fn load_or_default(path: &Path) -> (Self, bool) {
        match Self::load(path) {
            Ok(settings) => (settings, false),
            Err(_) => (Self::default(), true),
        }
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let serialized = serde_json::to_vec_pretty(self)
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
}
