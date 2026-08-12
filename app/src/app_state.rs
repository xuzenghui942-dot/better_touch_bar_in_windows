use std::{
    io,
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
};

use three_finger_drag_core::{
    advanced::{self, AdvancedRuntimeStatus},
    advanced_gestures::config::AdvancedConfig,
    logging::RingLogger,
    settings::{AppSettings, DeviceDragSettings},
    win32::{BackendEvent, InputService, TouchpadRuntimeStatus},
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
        let (settings, fresh_configuration) = AppSettings::load_or_default(&settings_path);
        if fresh_configuration {
            settings.save(&settings_path)?;
        }
        let advanced_settings_path = advanced_settings_path()?;
        let (mut advanced_settings, fresh_advanced_configuration) =
            advanced::load_or_default(&advanced_settings_path);
        if advanced_settings.enable_on_app_start {
            advanced_settings.enabled = true;
        }
        if fresh_advanced_configuration || advanced_settings.enable_on_app_start {
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
                stats_path,
                status: RwLock::new(TouchpadRuntimeStatus {
                    initialized: false,
                    touchpad_exists: false,
                    receiver_installed: false,
                    devices: Vec::new(),
                }),
                advanced_status: RwLock::new(AdvancedRuntimeStatus {
                    enabled: advanced_settings.enabled && advanced_settings.gestures_enabled,
                    ..AdvancedRuntimeStatus::default()
                }),
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

    pub fn update_advanced_settings(
        &self,
        update: impl FnOnce(&mut AdvancedConfig),
    ) -> io::Result<AdvancedConfig> {
        let snapshot = {
            let mut settings = self
                .inner
                .advanced_settings
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            update(&mut settings);
            *settings = settings.clone().normalized();
            settings.clone()
        };
        advanced::save(&snapshot, &self.inner.advanced_settings_path)?;
        Ok(snapshot)
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

fn settings_path() -> io::Result<PathBuf> {
    let base = dirs::data_local_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "无法确定本地应用数据目录。"))?;
    Ok(base.join("ThreeFingerDragRust").join("preferences.json"))
}

fn advanced_settings_path() -> io::Result<PathBuf> {
    let base = dirs::data_local_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "无法确定本地应用数据目录。"))?;
    Ok(base
        .join("ThreeFingerDragRust")
        .join("advanced-gestures.json"))
}

fn stats_path() -> io::Result<PathBuf> {
    let base = dirs::data_local_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "无法确定本地应用数据目录。"))?;
    Ok(base.join("ThreeFingerDragRust").join("swoosh-stats.json"))
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
