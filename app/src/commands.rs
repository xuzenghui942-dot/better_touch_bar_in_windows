use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
#[cfg(windows)]
use three_finger_drag_core::settings::PendingStartupAction;
use three_finger_drag_core::{
    advanced::AdvancedRuntimeStatus,
    advanced_gestures::config::{AdvancedConfig, SwipeDownMode},
    platform::TouchpadRuntimeStatus,
    settings::{AppSettings, DeviceDragSettings, DragButton},
};

use crate::startup;
use crate::{app_catalog::RunningApp, app_state::AppState, external_links};

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
    pub integration: PlatformIntegration,
    pub version: &'static str,
    pub platform: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlatformIntegration {
    pub session_type: String,
    pub desktop: String,
    pub is_gnome_wayland: bool,
    pub gesture_guard_installed: bool,
    pub gesture_guard_enabled: bool,
    pub gesture_guard_active: bool,
    pub gesture_guard_detail: String,
    pub advanced_window_gestures_available: bool,
    pub autostart_configuration_available: bool,
    pub elevated_mode_available: bool,
}

#[tauri::command]
pub fn get_snapshot(state: State<'_, AppState>) -> AppSnapshot {
    snapshot(&state)
}

#[tauri::command]
pub fn get_platform_integration(state: State<'_, AppState>) -> PlatformIntegration {
    platform_integration(&state)
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
    #[cfg(target_os = "linux")]
    {
        state.update_advanced_settings_checked(move |settings, runtime| {
            *settings = prepare_linux_advanced_settings(input, runtime)?;
            Ok(())
        })?;
        Ok(snapshot(&state))
    }
    #[cfg(windows)]
    {
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
}

#[tauri::command]
pub fn restore_advanced_defaults(state: State<'_, AppState>) -> Result<AppSnapshot, String> {
    #[cfg(target_os = "linux")]
    state
        .update_advanced_settings_checked(|settings, runtime| {
            let defaults = AdvancedConfig {
                enabled: false,
                resize_horizontal_enabled: false,
                resize_vertical_enabled: false,
                five_finger_enabled: false,
                center_enabled: false,
                ..AdvancedConfig::default()
            };
            *settings = prepare_linux_advanced_settings(defaults, runtime)?;
            Ok(())
        })
        .map_err(|error| error.to_string())?;
    #[cfg(windows)]
    state
        .update_advanced_settings(|settings| *settings = AdvancedConfig::default())
        .map_err(|error| error.to_string())?;
    #[cfg(windows)]
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
    let integration = platform_integration(&state);
    let mut report = String::from("Three Finger Drag Linux diagnostics\n");
    report.push_str(&format!("Version: {}\n", env!("CARGO_PKG_VERSION")));
    report.push_str(&format!("OS: {}\n", std::env::consts::OS));
    report.push_str(&format!("Architecture: {}\n", std::env::consts::ARCH));
    report.push_str(&format!("Session: {}\n", integration.session_type));
    report.push_str(&format!("Desktop: {}\n", integration.desktop));
    report.push_str(&format!(
        "Three-finger gesture guard: {}\n",
        integration.gesture_guard_detail
    ));
    report.push_str(&format!(
        "Advanced config (runtime-gated on Linux): {:?}\n",
        settings
    ));
    report.push_str(&format!("Advanced runtime: {:?}\n", runtime));
    report.push_str(&format!("Touchpad status: {:?}\n", touchpad));
    report.push_str("\nRecent log entries:\n");
    report.push_str(&state.logger().snapshot().join("\n"));
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
    #[cfg(target_os = "linux")]
    {
        let _ = app;
        let before = state.settings();
        if before.run_at_startup == enabled && startup::is_unelevated_enabled() == enabled {
            return Ok(snapshot(&state));
        }
        let executable = startup::current_executable().map_err(|error| error.to_string())?;
        if enabled {
            startup::enable_unelevated(&executable)
        } else {
            startup::disable_unelevated()
        }
        .map_err(|error| error.to_string())?;

        if let Err(error) = state.update_settings(|settings| settings.run_at_startup = enabled) {
            let rollback = if before.run_at_startup {
                startup::enable_unelevated(&executable)
            } else {
                startup::disable_unelevated()
            };
            return Err(match rollback {
                Ok(()) => error.to_string(),
                Err(rollback_error) => {
                    format!("保存自启动设置失败：{error}；回滚启动项也失败：{rollback_error}")
                }
            });
        }
        Ok(snapshot(&state))
    }
    #[cfg(windows)]
    {
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
}

#[tauri::command]
pub fn set_run_elevated(
    enabled: bool,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<AppSnapshot, String> {
    #[cfg(target_os = "linux")]
    {
        let _ = (enabled, app, state);
        Err(
            "Linux 版不支持也不应以 root 身份运行；输入设备访问应通过最小 udev 权限规则提供。"
                .to_owned(),
        )
    }
    #[cfg(windows)]
    {
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
}

#[tauri::command]
pub fn save_logs(state: State<'_, AppState>) -> Result<String, String> {
    let directory =
        dirs::download_dir().ok_or_else(|| "无法确定“下载”文件夹的位置。".to_owned())?;
    let path = directory.join("three-finger-drag-linux.log");
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
        integration: platform_integration(state),
        version: env!("CARGO_PKG_VERSION"),
        platform: std::env::consts::OS,
    }
}

fn startup_presentation(settings: &AppSettings) -> StartupPresentation {
    #[cfg(target_os = "linux")]
    {
        let _ = settings;
        StartupPresentation {
            enabled: false,
            elevated_mode: false,
            title: "登录自启动暂未开放：需先完成手动运行和重新登录稳定性验证。".to_owned(),
            severity: "info".to_owned(),
        }
    }
    #[cfg(windows)]
    {
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
}

#[cfg(windows)]
fn restart_elevated(app: &AppHandle) -> Result<(), String> {
    startup::request_elevated_restart().map_err(|error| error.to_string())?;
    app.exit(0);
    Ok(())
}

#[cfg(windows)]
fn restore_settings(state: &AppState, original: AppSettings) -> Result<(), String> {
    state
        .update_settings(|settings| *settings = original)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn platform_integration(state: &AppState) -> PlatformIntegration {
    #[cfg(target_os = "linux")]
    {
        linux_platform_integration(state.advanced_status().available)
    }
    #[cfg(windows)]
    {
        PlatformIntegration {
            session_type: "desktop".to_owned(),
            desktop: "Windows".to_owned(),
            is_gnome_wayland: false,
            gesture_guard_installed: false,
            gesture_guard_enabled: false,
            gesture_guard_active: false,
            gesture_guard_detail: "不适用。".to_owned(),
            advanced_window_gestures_available: true,
            autostart_configuration_available: true,
            elevated_mode_available: true,
        }
    }
}

#[cfg(target_os = "linux")]
fn linux_platform_integration(advanced_runtime_available: bool) -> PlatformIntegration {
    use std::{path::PathBuf, process::Command};

    const EXTENSION_UUID: &str = "three-finger-drag@local";

    let session_type = std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".to_owned());
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| "unknown".to_owned());
    let is_gnome_wayland = session_type.eq_ignore_ascii_case("wayland")
        && desktop.to_ascii_lowercase().contains("gnome");
    let mut extension_roots = vec![PathBuf::from("/usr/share/gnome-shell/extensions")];
    if let Some(data_directory) = dirs::data_local_dir() {
        extension_roots.push(data_directory.join("gnome-shell/extensions"));
    }
    let gesture_guard_installed = extension_roots
        .iter()
        .any(|root| root.join(EXTENSION_UUID).join("metadata.json").is_file());

    let query_extension_state = |filter: &str| {
        let query = Command::new("gnome-extensions")
            .args(["list", filter])
            .output();
        match query {
            Ok(output) if output.status.success() => {
                let matched = String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .any(|line| line.trim() == EXTENSION_UUID);
                (matched, None)
            }
            Ok(output) => {
                let error = String::from_utf8_lossy(&output.stderr).trim().to_owned();
                (false, (!error.is_empty()).then_some(error))
            }
            Err(error) => (false, Some(error.to_string())),
        }
    };
    let (gesture_guard_enabled, enabled_query_detail) = query_extension_state("--enabled");
    let (gesture_guard_active, active_query_detail) = query_extension_state("--active");
    let gesture_guard_detail = linux_gesture_guard_detail(
        gesture_guard_installed,
        gesture_guard_enabled,
        gesture_guard_active,
        enabled_query_detail.as_deref(),
        active_query_detail.as_deref(),
    );

    PlatformIntegration {
        session_type,
        desktop,
        is_gnome_wayland,
        gesture_guard_installed,
        gesture_guard_enabled,
        gesture_guard_active,
        gesture_guard_detail,
        // This is intentionally the live broker handshake result.  Merely
        // running GNOME Wayland or installing an extension is not capability.
        advanced_window_gestures_available: advanced_runtime_available,
        autostart_configuration_available: true,
        elevated_mode_available: false,
    }
}

#[cfg(target_os = "linux")]
fn linux_gesture_guard_detail(
    installed: bool,
    enabled: bool,
    active: bool,
    enabled_query_error: Option<&str>,
    active_query_error: Option<&str>,
) -> String {
    if active {
        return "GNOME 三指滑动拦截扩展已在当前 Shell 中加载并活动，四指滑动保持由 GNOME 处理。"
            .to_owned();
    }
    if enabled {
        return match active_query_error {
            Some(detail) => format!(
                "扩展的启用偏好已打开，但无法确认当前 GNOME Shell 是否已加载并活动：{detail}。在确认前，三指滑动仍可能触发 GNOME 系统手势。"
            ),
            None => "扩展的启用偏好已打开，但尚未确认当前 GNOME Shell 已加载并活动；在确认前，三指滑动仍可能触发 GNOME 系统手势。"
                .to_owned(),
        };
    }
    if installed {
        return match enabled_query_error {
            Some(detail) => format!("扩展已安装，但暂时无法确认是否已启用：{detail}"),
            None => "扩展已安装但未启用；三指滑动仍可能触发 GNOME 系统手势。".to_owned(),
        };
    }
    "未检测到配套的 GNOME 三指滑动拦截扩展。".to_owned()
}

#[cfg(target_os = "linux")]
fn prepare_linux_advanced_settings(
    input: AdvancedConfig,
    runtime: &AdvancedRuntimeStatus,
) -> Result<AdvancedConfig, String> {
    let mut input = input.normalized();
    // Neither advanced preferences nor a successful broker handshake grants
    // permission to create persistent desktop startup integration.
    input.enable_on_app_start = false;
    input.launch_at_login = false;
    if input.enabled && !runtime.available {
        return Err(runtime.last_error.clone().unwrap_or_else(|| {
            "无法启用高级窗口手势：GNOME/Wayland broker 尚未完成能力握手。设置未更改；四指手势继续由 GNOME 处理。".to_owned()
        }));
    }

    // These source-model options have no Linux implementation.  Keeping them
    // visibly configurable would make a successful save look like successful
    // behavior, so Linux persists their fail-closed values instead.
    input.mouse_middle_button_hud_enabled = false;
    input.phantom_rejection = false;
    input.taskbar_icon_gestures_enabled = false;
    input.overlay_use_accent = false;
    input.five_finger_enabled = false;
    input.center_enabled = false;
    input.live_preview = false;
    input.move_cursor = false;
    input.resize_horizontal_enabled = false;
    input.resize_vertical_enabled = false;

    if runtime.available {
        let capabilities = &runtime.capabilities;
        input.maximize_enabled &= capabilities.maximize;
        input.halves_enabled &= capabilities.snap_halves;
        input.quarters_enabled &= capabilities.snap_quarters;
        input.minimize_enabled &= capabilities.minimize;
        input.four_finger_swipe_down_minimize_all_enabled &= capabilities.minimize_all;
        input.monitor_move_enabled &= capabilities.monitor_move;
        input.preview_desktop_destination &= capabilities.workspace;
        input.create_desktop_on_overflow &= capabilities.dynamic_workspace;
        input.animate_snaps &= capabilities.animation;
        input.app_switch_on_hold &= capabilities.app_switch;
        if !capabilities.close
            && matches!(
                input.swipe_down_action,
                SwipeDownMode::Close | SwipeDownMode::Choose
            )
        {
            input.swipe_down_action = SwipeDownMode::Minimize;
        }
    }
    Ok(input)
}

#[cfg(all(test, target_os = "linux"))]
mod linux_advanced_tests {
    use super::*;

    #[test]
    fn gesture_guard_detail_only_claims_runtime_protection_when_active() {
        let active = linux_gesture_guard_detail(true, true, true, None, None);
        assert!(active.contains("当前 Shell 中加载并活动"));
        assert!(!active.contains("仍可能触发"));

        let enabled_only = linux_gesture_guard_detail(true, true, false, None, None);
        assert!(enabled_only.contains("启用偏好已打开"));
        assert!(enabled_only.contains("仍可能触发 GNOME 系统手势"));
        assert!(!enabled_only.contains("当前 Shell 中加载并活动"));
    }

    #[test]
    fn gesture_guard_detail_preserves_active_query_errors_for_enabled_only_state() {
        let detail =
            linux_gesture_guard_detail(true, true, false, None, Some("session bus unavailable"));
        assert!(detail.contains("无法确认当前 GNOME Shell"));
        assert!(detail.contains("session bus unavailable"));
        assert!(detail.contains("仍可能触发 GNOME 系统手势"));
    }

    #[test]
    fn unavailable_broker_rejects_enable_without_sanitizing_it_into_success() {
        let input = AdvancedConfig {
            enabled: true,
            ..AdvancedConfig::default()
        };
        let runtime = AdvancedRuntimeStatus {
            available: false,
            last_error: Some("broker unavailable".to_owned()),
            ..AdvancedRuntimeStatus::default()
        };
        assert_eq!(
            prepare_linux_advanced_settings(input, &runtime).unwrap_err(),
            "broker unavailable"
        );
    }

    #[test]
    fn offline_preferences_persist_but_linux_startup_flags_stay_off() {
        let input = AdvancedConfig {
            enabled: false,
            grid_spacing: 7,
            enable_on_app_start: true,
            launch_at_login: true,
            ..AdvancedConfig::default()
        };
        let runtime = AdvancedRuntimeStatus {
            available: false,
            ..AdvancedRuntimeStatus::default()
        };
        let prepared = prepare_linux_advanced_settings(input, &runtime).unwrap();
        assert_eq!(prepared.grid_spacing, 7);
        assert!(!prepared.enable_on_app_start);
        assert!(!prepared.launch_at_login);
    }

    #[test]
    fn broker_handshake_allows_explicit_enable() {
        let input = AdvancedConfig {
            enabled: true,
            ..AdvancedConfig::default()
        };
        let runtime = AdvancedRuntimeStatus {
            available: true,
            ..AdvancedRuntimeStatus::default()
        };
        assert!(
            prepare_linux_advanced_settings(input, &runtime)
                .unwrap()
                .enabled
        );
    }

    #[test]
    fn broker_capabilities_sanitize_unsupported_actions_and_linux_placeholders() {
        let input = AdvancedConfig {
            enabled: true,
            gestures_enabled: false,
            swipe_down_action: SwipeDownMode::Close,
            create_desktop_on_overflow: true,
            five_finger_enabled: true,
            center_enabled: true,
            resize_horizontal_enabled: true,
            resize_vertical_enabled: true,
            animate_snaps: true,
            live_preview: true,
            move_cursor: true,
            mouse_middle_button_hud_enabled: true,
            app_switch_on_hold: true,
            phantom_rejection: true,
            taskbar_icon_gestures_enabled: true,
            overlay_use_accent: true,
            ..AdvancedConfig::default()
        };
        let runtime = AdvancedRuntimeStatus {
            available: true,
            capabilities: three_finger_drag_core::advanced::AdvancedCapabilities {
                two_finger: true,
                axis_resize: true,
                snap_halves: true,
                minimize: true,
                minimize_all: true,
                hud: true,
                animation: true,
                live_preview: true,
                move_cursor: true,
                app_switch: true,
                ..Default::default()
            },
            ..AdvancedRuntimeStatus::default()
        };
        let prepared = prepare_linux_advanced_settings(input, &runtime).unwrap();
        assert!(prepared.enabled);
        assert!(
            !prepared.gestures_enabled,
            "independent engine switch was folded"
        );
        assert!(prepared.halves_enabled);
        assert!(!prepared.resize_horizontal_enabled);
        assert!(!prepared.resize_vertical_enabled);
        assert!(!prepared.maximize_enabled);
        assert!(!prepared.quarters_enabled);
        assert!(prepared.four_finger_swipe_down_minimize_all_enabled);
        assert!(!prepared.five_finger_enabled);
        assert!(!prepared.center_enabled);
        assert!(!prepared.create_desktop_on_overflow);
        assert_eq!(prepared.swipe_down_action, SwipeDownMode::Minimize);
        assert!(prepared.animate_snaps);
        assert!(!prepared.move_cursor);
        assert!(!prepared.mouse_middle_button_hud_enabled);
        assert!(!prepared.live_preview);
        assert!(prepared.app_switch_on_hold);
        assert!(!prepared.phantom_rejection);
        assert!(!prepared.taskbar_icon_gestures_enabled);
        assert!(!prepared.overlay_use_accent);
    }

    #[test]
    fn linux_port_always_removes_five_finger_runtime_preferences() {
        let input = AdvancedConfig {
            enabled: true,
            five_finger_enabled: true,
            center_enabled: true,
            ..AdvancedConfig::default()
        };
        let runtime = AdvancedRuntimeStatus {
            available: true,
            capabilities: three_finger_drag_core::advanced::AdvancedCapabilities {
                five_finger: true,
                free_move: true,
                free_resize: false,
                pinch: false,
                ..Default::default()
            },
            ..AdvancedRuntimeStatus::default()
        };

        let prepared = prepare_linux_advanced_settings(input, &runtime).unwrap();
        assert!(!prepared.five_finger_enabled);
        assert!(!prepared.center_enabled);
    }
}
