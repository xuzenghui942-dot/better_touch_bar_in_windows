use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use three_finger_drag_core::{
    advanced::AdvancedRuntimeStatus,
    advanced_gestures::config::AdvancedConfig,
    settings::{AppSettings, DeviceDragSettings, DragButton, PendingStartupAction},
    win32::TouchpadRuntimeStatus,
};

use crate::{app_catalog::RunningApp, app_state::AppState, external_links, startup};

#[derive(Debug, Deserialize)]
pub struct GestureSettingsInput {
    pub three_finger_drag: bool,
    pub drag_button: DragButton,
    pub allow_release_and_restart: bool,
    pub release_delay_ms: u32,
    pub devices: BTreeMap<String, DeviceDragSettings>,
    pub cursor_averaging: u32,
    pub max_finger_move_distance: u32,
    pub start_threshold: u32,
    pub stop_threshold: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct StartupPresentation {
    pub enabled: bool,
    pub elevated_mode: bool,
    pub title: String,
    pub severity: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AppSnapshot {
    pub settings: AppSettings,
    pub advanced: AdvancedConfig,
    pub advanced_runtime: AdvancedRuntimeStatus,
    pub touchpad: TouchpadRuntimeStatus,
    pub startup: StartupPresentation,
    pub is_administrator: bool,
    pub version: &'static str,
}

#[tauri::command]
pub fn get_snapshot(state: State<'_, AppState>) -> AppSnapshot {
    snapshot(&state)
}

#[tauri::command]
pub fn save_gesture_settings(
    input: GestureSettingsInput,
    state: State<'_, AppState>,
) -> Result<AppSnapshot, String> {
    state
        .update_settings(|settings| {
            settings.three_finger_drag = input.three_finger_drag;
            settings.drag_button = input.drag_button;
            settings.allow_release_and_restart = input.allow_release_and_restart;
            settings.release_delay_ms = input.release_delay_ms;
            settings.devices = input.devices;
            settings.cursor_averaging = input.cursor_averaging;
            settings.max_finger_move_distance = input.max_finger_move_distance;
            settings.start_threshold = input.start_threshold;
            settings.stop_threshold = input.stop_threshold;
        })
        .map_err(|error| error.to_string())?;
    Ok(snapshot(&state))
}

#[tauri::command]
pub fn save_advanced_settings(
    input: AdvancedConfig,
    state: State<'_, AppState>,
) -> Result<AppSnapshot, String> {
    let elevated = state.settings().run_elevated;
    let previous_launch_at_login = state.advanced_settings().launch_at_login;
    let launch_at_login = input.launch_at_login;
    state
        .update_advanced_settings(|settings| *settings = input.normalized())
        .map_err(|error| error.to_string())?;
    if previous_launch_at_login != launch_at_login {
        startup::refresh_advanced_startup(launch_at_login, elevated)
            .map_err(|error| error.to_string())?;
    }
    Ok(snapshot(&state))
}

#[tauri::command]
pub fn restore_advanced_defaults(state: State<'_, AppState>) -> Result<AppSnapshot, String> {
    state
        .update_advanced_settings(|settings| *settings = AdvancedConfig::default())
        .map_err(|error| error.to_string())?;
    startup::disable_advanced_startup().map_err(|error| error.to_string())?;
    Ok(snapshot(&state))
}

#[tauri::command]
pub fn list_running_apps() -> Vec<RunningApp> {
    crate::app_catalog::list_running_apps()
}

#[tauri::command]
pub fn list_installed_apps() -> Vec<RunningApp> {
    crate::app_catalog::list_installed_apps()
}

#[tauri::command]
pub fn advanced_diagnostics(state: State<'_, AppState>) -> String {
    let settings = state.advanced_settings();
    let runtime = state.advanced_status();
    let touchpad = state.status();
    let mut report = String::from("Rust Swoosh advanced diagnostics\r\n");
    report.push_str(&format!("Version: {}\r\n", env!("CARGO_PKG_VERSION")));
    report.push_str(&format!("OS: {}\r\n", std::env::consts::OS));
    report.push_str(&format!("Architecture: {}\r\n", std::env::consts::ARCH));
    report.push_str(&format!("Advanced config: {:?}\r\n", settings));
    report.push_str(&format!("Advanced runtime: {:?}\r\n", runtime));
    report.push_str(&format!("Touchpad status: {:?}\r\n", touchpad));
    report.push_str("\r\nRecent log entries:\r\n");
    report.push_str(&state.logger().snapshot().join("\r\n"));
    report
}

#[tauri::command]
pub fn set_record_logs(enabled: bool, state: State<'_, AppState>) -> Result<AppSnapshot, String> {
    state
        .update_settings(|settings| settings.record_logs = enabled)
        .map_err(|error| error.to_string())?;
    Ok(snapshot(&state))
}

#[tauri::command]
pub fn set_run_at_startup(
    enabled: bool,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<AppSnapshot, String> {
    let before = state.settings();
    let executable = startup::current_executable().map_err(|error| error.to_string())?;

    if before.run_elevated {
        if startup::is_administrator() {
            if enabled {
                startup::enable_elevated(&executable)
            } else {
                startup::disable_elevated()
            }
            .map_err(|error| error.to_string())?;
            state
                .update_settings(|settings| {
                    settings.run_at_startup = enabled;
                    settings.pending_startup_action = PendingStartupAction::None;
                })
                .map_err(|error| error.to_string())?;
        } else {
            state
                .update_settings(|settings| {
                    settings.run_at_startup = enabled;
                    settings.pending_startup_action = if enabled {
                        PendingStartupAction::EnableElevatedStartup
                    } else {
                        PendingStartupAction::DisableElevatedStartup
                    };
                })
                .map_err(|error| error.to_string())?;
            if let Err(error) = restart_elevated(&app) {
                restore_settings(&state, before)?;
                return Err(error);
            }
        }
    } else {
        if enabled {
            startup::enable_unelevated(&executable)
        } else {
            startup::disable_unelevated()
        }
        .map_err(|error| error.to_string())?;
        state
            .update_settings(|settings| settings.run_at_startup = enabled)
            .map_err(|error| error.to_string())?;
    }
    Ok(snapshot(&state))
}

#[tauri::command]
pub fn set_run_elevated(
    enabled: bool,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<AppSnapshot, String> {
    let before = state.settings();
    if before.run_elevated == enabled {
        return Ok(snapshot(&state));
    }

    let startup_enabled = startup::is_enabled(before.run_elevated) || before.run_at_startup;
    let executable = startup::current_executable().map_err(|error| error.to_string())?;

    if enabled {
        state
            .update_settings(|settings| {
                settings.run_elevated = true;
                settings.run_at_startup = startup_enabled;
                settings.pending_startup_action = if startup_enabled {
                    PendingStartupAction::EnableElevatedRunWithStartup
                } else {
                    PendingStartupAction::None
                };
            })
            .map_err(|error| error.to_string())?;

        if startup::is_administrator() {
            if startup_enabled {
                startup::enable_elevated(&executable).map_err(|error| error.to_string())?;
            }
            state
                .update_settings(|settings| {
                    settings.pending_startup_action = PendingStartupAction::None
                })
                .map_err(|error| error.to_string())?;
        } else if let Err(error) = restart_elevated(&app) {
            restore_settings(&state, before)?;
            return Err(error);
        }
    } else if startup::is_administrator() {
        if startup_enabled {
            startup::disable_elevated().map_err(|error| error.to_string())?;
            startup::enable_unelevated(&executable).map_err(|error| error.to_string())?;
        }
        state
            .update_settings(|settings| {
                settings.run_elevated = false;
                settings.run_at_startup = startup_enabled;
                settings.pending_startup_action = PendingStartupAction::None;
            })
            .map_err(|error| error.to_string())?;
    } else if startup_enabled && startup::is_elevated_enabled() {
        state
            .update_settings(|settings| {
                settings.pending_startup_action =
                    PendingStartupAction::DisableElevatedRunWithStartup;
            })
            .map_err(|error| error.to_string())?;
        if let Err(error) = restart_elevated(&app) {
            restore_settings(&state, before)?;
            return Err(error);
        }
    } else {
        state
            .update_settings(|settings| {
                settings.run_elevated = false;
                settings.pending_startup_action = PendingStartupAction::None;
            })
            .map_err(|error| error.to_string())?;
    }

    Ok(snapshot(&state))
}

#[tauri::command]
pub fn save_logs(state: State<'_, AppState>) -> Result<String, String> {
    let directory =
        dirs::download_dir().ok_or_else(|| "无法确定“下载”文件夹的位置。".to_owned())?;
    let path = directory.join("Logs_ThreeFingerDragRust.txt");
    state
        .logger()
        .export(&path)
        .map_err(|error| error.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

#[tauri::command]
pub fn open_touchpad_settings() -> Result<(), String> {
    external_links::open_touchpad_settings().map_err(|error| error.to_string())
}

#[tauri::command]
pub fn open_external(url: String) -> Result<(), String> {
    external_links::open_allowed_external_link(&url).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn close_settings(app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "设置窗口不存在。".to_owned())?;
    window.hide().map_err(|error| error.to_string())
}

#[tauri::command]
pub fn quit_app(app: AppHandle, state: State<'_, AppState>) {
    state.stop_input_service();
    app.exit(0);
}

pub fn snapshot(state: &AppState) -> AppSnapshot {
    let settings = state.settings();
    AppSnapshot {
        startup: startup_presentation(&settings),
        settings,
        advanced: state.advanced_settings(),
        advanced_runtime: state.advanced_status(),
        touchpad: state.status(),
        is_administrator: startup::is_administrator(),
        version: env!("CARGO_PKG_VERSION"),
    }
}

fn startup_presentation(settings: &AppSettings) -> StartupPresentation {
    let enabled = startup::is_enabled(settings.run_elevated);
    let administrator = startup::is_administrator();
    let title = if settings.run_elevated {
        match (enabled, administrator) {
            (true, true) => {
                "已设置为开机启动，并已配置跳过 UAC 提示。".to_owned()
            }
            (true, false) => "已设置为开机启动，并已配置跳过 UAC 提示。关闭此选项时，应用需要以管理员权限重新启动。".to_owned(),
            (false, true) => {
                "当前未设置为开机启动。可在此启用，并配置为跳过 UAC 提示。".to_owned()
            }
            (false, false) => "当前未设置为开机启动。启用时应用需要以管理员权限重新启动。".to_owned(),
        }
    } else if enabled {
        "已设置为开机启动。".to_owned()
    } else {
        "当前未设置为开机启动。".to_owned()
    };
    let severity = if enabled {
        "success"
    } else if settings.run_elevated && !administrator {
        "warning"
    } else {
        "info"
    };
    StartupPresentation {
        enabled,
        elevated_mode: settings.run_elevated,
        title,
        severity: severity.to_owned(),
    }
}

fn restart_elevated(app: &AppHandle) -> Result<(), String> {
    startup::request_elevated_restart().map_err(|error| error.to_string())?;
    app.exit(0);
    Ok(())
}

fn restore_settings(state: &AppState, original: AppSettings) -> Result<(), String> {
    state
        .update_settings(|settings| *settings = original)
        .map(|_| ())
        .map_err(|error| error.to_string())
}
