use std::{
    io,
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
};

#[cfg(target_os = "linux")]
use std::{
    fs,
    io::Write,
    sync::atomic::{AtomicU64, Ordering},
};

use three_finger_drag_core::{
    advanced::{self, AdvancedRuntimeStatus},
    advanced_gestures::config::AdvancedConfig,
    logging::RingLogger,
    platform::{BackendEvent, InputService, TouchpadRuntimeStatus},
    settings::{AppSettings, DeviceDragSettings},
};

#[derive(Clone)]
pub struct AppState {
    inner: Arc<AppStateInner>,
}

struct AppStateInner {
    settings_path: PathBuf,
    settings: Arc<RwLock<AppSettings>>,
    advanced_settings_path: PathBuf,
    advanced_settings: Arc<RwLock<AdvancedConfig>>,
    // Serializes the complete prepare -> persist -> publish transaction.  The
    // shared settings handle is read live by the input worker, so publishing a
    // candidate before its file has been committed would make a failed save
    // take effect for the current process only.
    advanced_settings_transaction: Mutex<()>,
    stats_path: PathBuf,
    status: RwLock<TouchpadRuntimeStatus>,
    advanced_status: RwLock<AdvancedRuntimeStatus>,
    lifetime_swooshes: RwLock<u64>,
    logger: RingLogger,
    input_service: Mutex<Option<InputService>>,
}

impl AppState {
    pub fn load() -> io::Result<Self> {
        let settings_path = settings_path()?;
        let (mut settings, fresh_configuration) = AppSettings::load_or_default(&settings_path);
        let mut settings_changed = fresh_configuration;
        #[cfg(target_os = "linux")]
        {
            let previous = settings.clone();
            settings.run_at_startup = crate::startup::is_unelevated_enabled();
            settings.run_elevated = false;
            settings.pending_startup_action =
                three_finger_drag_core::settings::PendingStartupAction::None;
            settings_changed |= settings != previous;
        }
        if settings_changed {
            settings.save(&settings_path)?;
        }
        let advanced_settings_path = advanced_settings_path()?;
        let (mut advanced_settings, fresh_advanced_configuration) =
            advanced::load_or_default(&advanced_settings_path);
        let mut advanced_settings_changed = fresh_advanced_configuration;
        #[cfg(target_os = "linux")]
        {
            let previous = advanced_settings.clone();
            // Desktop startup remains deliberately unavailable until it has
            // passed the separate login-session safety process.  Advanced
            // window gestures themselves are gated by the runtime broker
            // handshake, not by mutating the user's persisted master switch.
            advanced_settings.enable_on_app_start = false;
            advanced_settings.launch_at_login = false;
            advanced_settings.live_preview = false;
            advanced_settings.move_cursor = false;
            advanced_settings.mouse_middle_button_hud_enabled = false;
            advanced_settings.resize_horizontal_enabled = false;
            advanced_settings.resize_vertical_enabled = false;
            advanced_settings.app_switch_on_hold = false;
            advanced_settings.phantom_rejection = false;
            advanced_settings.taskbar_icon_gestures_enabled = false;
            advanced_settings.overlay_use_accent = false;
            let linux_defaults = AdvancedConfig::default();
            advanced_settings.hud_background = linux_defaults.hud_background;
            advanced_settings.hud_size = linux_defaults.hud_size;
            advanced_settings.hud_fade_out_seconds = linux_defaults.hud_fade_out_seconds;
            // Linux implements two-finger window transactions plus one
            // passive four-finger-down action. Never revive legacy
            // five-finger preferences loaded from another profile.
            advanced_settings.five_finger_enabled = false;
            advanced_settings.center_enabled = false;
            advanced_settings_changed |= advanced_settings != previous;
        }
        #[cfg(windows)]
        if advanced_settings.enable_on_app_start {
            advanced_settings.enabled = true;
        }
        #[cfg(windows)]
        {
            advanced_settings_changed |= advanced_settings.enable_on_app_start;
        }
        if advanced_settings_changed {
            advanced::save(&advanced_settings, &advanced_settings_path)?;
        }
        let stats_path = stats_path()?;
        let lifetime_swooshes = load_stats(&stats_path);
        let logger = RingLogger::default();
        logger.set_enabled(settings.record_logs);

        Ok(Self {
            inner: Arc::new(AppStateInner {
                settings_path,
                settings: Arc::new(RwLock::new(settings)),
                advanced_settings_path,
                advanced_settings: Arc::new(RwLock::new(advanced_settings.clone())),
                advanced_settings_transaction: Mutex::new(()),
                stats_path,
                status: RwLock::new(TouchpadRuntimeStatus {
                    initialized: false,
                    touchpad_exists: false,
                    receiver_installed: false,
                    devices: Vec::new(),
                }),
                advanced_status: RwLock::new(initial_advanced_status(&advanced_settings)),
                lifetime_swooshes: RwLock::new(lifetime_swooshes),
                logger,
                input_service: Mutex::new(None),
            }),
        })
    }

    pub fn settings_handle(&self) -> Arc<RwLock<AppSettings>> {
        Arc::clone(&self.inner.settings)
    }

    pub fn advanced_settings_handle(&self) -> Arc<RwLock<AdvancedConfig>> {
        Arc::clone(&self.inner.advanced_settings)
    }

    pub fn logger(&self) -> RingLogger {
        self.inner.logger.clone()
    }

    pub fn settings(&self) -> AppSettings {
        self.inner
            .settings
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn status(&self) -> TouchpadRuntimeStatus {
        self.inner
            .status
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn advanced_settings(&self) -> AdvancedConfig {
        self.inner
            .advanced_settings
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn advanced_status(&self) -> AdvancedRuntimeStatus {
        let mut status = self
            .inner
            .advanced_status
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        status.completed_swooshes = *self
            .inner
            .lifetime_swooshes
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        status
    }

    #[cfg(windows)]
    pub fn update_advanced_settings(
        &self,
        update: impl FnOnce(&mut AdvancedConfig),
    ) -> io::Result<AdvancedConfig> {
        self.update_advanced_settings_checked(|settings, _runtime| {
            update(settings);
            Ok(())
        })
        .map_err(io::Error::other)
    }

    /// Atomically validates, persists, and publishes an advanced configuration.
    ///
    /// The broker status read guard is held for the transaction so an enable
    /// request cannot race an already-delivered availability transition.  The
    /// candidate is not exposed through `advanced_settings_handle` until the
    /// durable save succeeds.
    pub fn update_advanced_settings_checked(
        &self,
        update: impl FnOnce(&mut AdvancedConfig, &AdvancedRuntimeStatus) -> Result<(), String>,
    ) -> Result<AdvancedConfig, String> {
        let _transaction = self
            .inner
            .advanced_settings_transaction
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let runtime = self
            .inner
            .advanced_status
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut candidate = self
            .inner
            .advanced_settings
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        update(&mut candidate, &runtime)?;
        candidate = candidate.normalized();
        persist_advanced_settings(&candidate, &self.inner.advanced_settings_path)
            .map_err(|error| error.to_string())?;
        *self
            .inner
            .advanced_settings
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = candidate.clone();
        Ok(candidate)
    }

    pub fn update_settings(
        &self,
        update: impl FnOnce(&mut AppSettings),
    ) -> io::Result<AppSettings> {
        let snapshot = {
            let mut settings = self
                .inner
                .settings
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            update(&mut settings);
            settings.normalize();
            settings.clone()
        };
        self.inner.logger.set_enabled(snapshot.record_logs);
        snapshot.save(&self.inner.settings_path)?;
        Ok(snapshot)
    }

    pub fn save_current_settings(&self) -> io::Result<()> {
        self.settings().save(&self.inner.settings_path)
    }

    pub fn handle_backend_event(&self, event: &BackendEvent) {
        match event {
            BackendEvent::Status(status) => {
                let added_device = {
                    let mut settings = self
                        .inner
                        .settings
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let mut added = false;
                    for device in &status.devices {
                        if !settings.devices.contains_key(&device.id) {
                            settings
                                .devices
                                .insert(device.id.clone(), DeviceDragSettings::default());
                            added = true;
                        }
                    }
                    added
                };
                if added_device {
                    let _ = self.save_current_settings();
                }

                *self
                    .inner
                    .status
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = status.clone();
            }
            BackendEvent::Advanced(status) => {
                let previous = self
                    .inner
                    .advanced_status
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .completed_swooshes;
                if status.completed_swooshes > previous {
                    let mut lifetime = self
                        .inner
                        .lifetime_swooshes
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    *lifetime = lifetime.saturating_add(status.completed_swooshes - previous);
                    let _ = save_stats(&self.inner.stats_path, *lifetime);
                }
                *self
                    .inner
                    .advanced_status
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = status.clone();
            }
            BackendEvent::Contacts(_) | BackendEvent::Error(_) => {}
        }
    }

    pub fn install_input_service(&self, service: InputService) {
        *self
            .inner
            .input_service
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(service);
    }

    pub fn stop_input_service(&self) {
        if let Some(mut service) = self
            .inner
            .input_service
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            service.stop();
        }
    }
}

#[cfg(target_os = "linux")]
fn persist_advanced_settings(config: &AdvancedConfig, path: &PathBuf) -> io::Result<()> {
    static TEMPORARY_COUNTER: AtomicU64 = AtomicU64::new(0);

    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "高级设置路径没有父目录。"))?;
    fs::create_dir_all(parent)?;
    let serialized = serde_json::to_vec_pretty(&config.clone().normalized())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let sequence = TEMPORARY_COUNTER.fetch_add(1, Ordering::Relaxed);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("advanced-window-gestures.json");
    let temporary = parent.join(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        sequence
    ));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&serialized)?;
        file.sync_all()?;
        // Linux rename replaces an existing file atomically; there is never a
        // window where the committed settings path has been removed first.
        fs::rename(&temporary, path)?;
        // The committed file already contains synced data. Directory fsync is
        // best effort because a failure after rename cannot be rolled back.
        let _ = fs::File::open(parent).and_then(|directory| directory.sync_all());
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(windows)]
fn persist_advanced_settings(config: &AdvancedConfig, path: &PathBuf) -> io::Result<()> {
    advanced::save(config, path)
}

fn settings_path() -> io::Result<PathBuf> {
    Ok(application_data_directory()?.join("preferences.json"))
}

fn advanced_settings_path() -> io::Result<PathBuf> {
    Ok(application_data_directory()?.join("advanced-window-gestures.json"))
}

fn stats_path() -> io::Result<PathBuf> {
    Ok(application_data_directory()?.join("gesture-stats.json"))
}

fn application_data_directory() -> io::Result<PathBuf> {
    let base = dirs::data_local_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "无法确定本地应用数据目录。"))?;
    #[cfg(target_os = "linux")]
    {
        Ok(base.join("three-finger-drag-linux"))
    }
    #[cfg(windows)]
    {
        Ok(base.join("ThreeFingerDragRust"))
    }
}

fn initial_advanced_status(settings: &AdvancedConfig) -> AdvancedRuntimeStatus {
    #[cfg(target_os = "linux")]
    {
        AdvancedRuntimeStatus {
            enabled: false,
            available: false,
            last_error: Some(
                if settings.enabled {
                    "高级设置已保存，但仍在等待 GNOME/Wayland broker 完成能力握手；握手前不会执行窗口动作。"
                } else {
                    "正在等待 GNOME/Wayland broker 能力握手；基础三指拖动不受影响。"
                }
                .to_owned(),
            ),
            ..AdvancedRuntimeStatus::default()
        }
    }
    #[cfg(windows)]
    {
        AdvancedRuntimeStatus {
            enabled: settings.enabled && settings.gestures_enabled,
            ..AdvancedRuntimeStatus::default()
        }
    }
}

fn load_stats(path: &PathBuf) -> u64 {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|value| {
            value
                .get("lifetimeSwooshes")
                .and_then(|number| number.as_u64())
        })
        .unwrap_or(0)
}

fn save_stats(path: &PathBuf, count: u64) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    let serialized = serde_json::to_vec_pretty(&serde_json::json!({
        "lifetimeSwooshes": count
    }))
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    std::fs::write(&temporary, serialized)?;
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    std::fs::rename(temporary, path)
}
