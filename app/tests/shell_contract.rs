#![cfg(target_os = "linux")]

use std::path::Path;

use three_finger_drag_core::settings::{AppSettings, PendingStartupAction};
use three_finger_drag_rust::{
    external_links::is_allowed_external_link, startup, tray_icon::create_tray_rgba,
};

const TAURI_CONFIG: &str = include_str!("../tauri.conf.json");
const BUILD_SCRIPT: &str = include_str!("../build.rs");
const APP_ENTRY: &str = include_str!("../src/lib.rs");
const COMMANDS: &str = include_str!("../src/commands.rs");
const APP_STATE: &str = include_str!("../src/app_state.rs");
const LINUX_APP_CATALOG: &str = include_str!("../src/app_catalog_linux.rs");
const SESSION_VERIFIER: &str = include_str!("../../scripts/verify-linux-session.sh");
const STARTUP_LINUX: &str = include_str!("../src/startup_linux.rs");

#[test]
fn linux_bundle_targets_deb_and_not_windows_installers() {
    assert!(TAURI_CONFIG.contains("\"productName\": \"Three Finger Drag Linux\""));
    assert!(TAURI_CONFIG.contains("io.github.xuzenghui942.threefingerdrag"));
    assert!(TAURI_CONFIG.contains("三指拖动（Linux）"));
    assert!(TAURI_CONFIG.contains("\"deb\""));
    assert!(!TAURI_CONFIG.contains("\"msi\""));
    assert!(!TAURI_CONFIG.contains("webviewInstallMode"));
    assert!(!BUILD_SCRIPT.contains("ICO"));
    assert!(!BUILD_SCRIPT.contains("Windows application icon"));
}

#[test]
fn linux_shell_never_requests_elevation() {
    // SAFETY: geteuid has no preconditions and only reads process credentials.
    let running_as_root = unsafe { libc::geteuid() == 0 };
    assert_eq!(startup::is_administrator(), running_as_root);
    assert!(startup::request_elevated_restart().is_err());
    assert!(startup::enable_elevated(Path::new("/tmp/three-finger-drag-linux")).is_err());
}

#[test]
fn linux_entry_rejects_root_before_initializing_the_application() {
    let guard = APP_ENTRY
        .find("if startup::is_administrator()")
        .expect("missing EUID 0 guard");
    let state_load = APP_ENTRY
        .find("AppState::load()")
        .expect("missing state initialization");
    assert!(
        guard < state_load,
        "root guard must run before app initialization"
    );
    assert!(APP_ENTRY.contains("拒绝以 root 身份运行三指拖动"));
    assert!(APP_ENTRY[guard..state_load].contains("return;"));
}

#[test]
fn user_level_autostart_entry_is_explicit_atomic_and_reversible() {
    let executable = Path::new("/opt/three finger drag/three-finger-drag-linux");
    assert_eq!(
        startup::startup_command(executable),
        "\"/opt/three finger drag/three-finger-drag-linux\" --autostart"
    );
    let entry = startup::desktop_entry(executable);
    assert!(entry.contains("Type=Application"));
    assert!(entry.contains("OnlyShowIn=GNOME;"));
    assert!(entry.contains("Terminal=false"));
    assert!(entry.contains("NoDisplay=true"));
    assert!(entry.contains(&format!("Exec={}", startup::startup_command(executable))));
    assert!(!entry.contains("systemctl"));
    assert!(!entry.contains("sudo"));
    for required in [
        "create_new(true)",
        "mode(0o600)",
        "fs::rename(&temporary, path)",
        "fs::remove_file(path)",
    ] {
        assert!(STARTUP_LINUX.contains(required));
    }
    assert!(COMMANDS.contains("startup::enable_unelevated(&executable)"));
    assert!(COMMANDS.contains("startup::disable_unelevated()"));
    assert!(COMMANDS.contains("autostart_configuration_available: true"));
}

#[test]
fn advanced_linux_enablement_is_gated_by_the_live_broker_status() {
    assert!(COMMANDS.contains("update_advanced_settings_checked"));
    assert!(COMMANDS.contains("prepare_linux_advanced_settings(input, runtime)"));
    assert!(COMMANDS.contains("if input.enabled && !runtime.available"));
    assert!(COMMANDS.contains("advanced_window_gestures_available: advanced_runtime_available"));
    assert!(COMMANDS.contains("input.enable_on_app_start = false"));
    assert!(COMMANDS.contains("input.launch_at_login = false"));
    assert!(!APP_STATE.contains("advanced_settings.enabled = false"));
}

#[test]
fn gnome_guard_distinguishes_enabled_preference_from_active_shell_state() {
    for required in [
        "gesture_guard_active",
        "query_extension_state(\"--enabled\")",
        "query_extension_state(\"--active\")",
        "if active",
        "当前 Shell 中加载并活动",
        "启用偏好已打开",
    ] {
        assert!(
            COMMANDS.contains(required),
            "missing GNOME guard runtime-state contract: {required}"
        );
    }
}

#[test]
fn advanced_settings_are_persisted_before_being_published_to_the_input_worker() {
    let persist = APP_STATE
        .find("persist_advanced_settings(&candidate")
        .expect("advanced candidate is not persisted");
    let publish = APP_STATE[persist..]
        .find("advanced_settings\n            .write()")
        .map(|offset| persist + offset)
        .expect("advanced candidate is not published");
    assert!(persist < publish);
    for required in [
        "advanced_settings_transaction",
        "create_new(true)",
        "TEMPORARY_COUNTER",
        "fs::rename(&temporary, path)",
    ] {
        assert!(
            APP_STATE.contains(required),
            "missing atomic save guard: {required}"
        );
    }
    assert!(!APP_STATE[persist..publish].contains("advanced_settings\n            .write()"));
}

#[test]
fn removed_linux_gestures_are_cleared_from_fresh_and_saved_profiles() {
    for setting in [
        "advanced_settings.live_preview = false",
        "advanced_settings.move_cursor = false",
        "advanced_settings.mouse_middle_button_hud_enabled = false",
        "advanced_settings.resize_horizontal_enabled = false",
        "advanced_settings.resize_vertical_enabled = false",
        "advanced_settings.five_finger_enabled = false",
        "advanced_settings.center_enabled = false",
    ] {
        assert!(
            APP_STATE.contains(setting),
            "missing safe Linux default: {setting}"
        );
    }
}

#[test]
fn linux_app_catalog_uses_native_app_ids_instead_of_exe_names() {
    for required in [
        "StartupWMClass",
        "X-GNOME-WMClass",
        "X-Flatpak",
        "file_stem",
    ] {
        assert!(LINUX_APP_CATALOG.contains(required));
    }
    assert!(!LINUX_APP_CATALOG.contains("format!(\"{name}.exe\")"));
    assert!(!LINUX_APP_CATALOG.contains("strip_suffix(\".exe\")"));
}

#[test]
fn session_verifier_rejects_a_login_older_than_desktop_changes() {
    assert!(SESSION_VERIFIER.contains("org.freedesktop.login1.Session Timestamp"));
    assert!(SESSION_VERIFIER.contains("session_started_usec / 1000000"));
    assert!(SESSION_VERIFIER.contains("session_started_epoch\" -le \"$changed_epoch"));
    assert!(SESSION_VERIFIER.contains("stat -c '%Y %Z' \"$rule_path\""));
    assert!(SESSION_VERIFIER.contains("GNOME 扩展文件"));
    assert!(!SESSION_VERIFIER.contains("date -d \"$session_started_text\""));
    assert!(SESSION_VERIFIER.contains("legacy_backup_root"));
    assert!(SESSION_VERIFIER.contains("旧扩展备份目录仍位于 GNOME 扫描路径中"));
    assert!(SESSION_VERIFIER.contains("gnome-extensions list --active"));
    assert!(SESSION_VERIFIER.contains("扩展启用偏好已打开，但当前 Shell 未确认活动"));
}

#[test]
fn pending_elevation_state_is_safely_cleared_on_linux() {
    let mut settings = AppSettings {
        run_elevated: true,
        pending_startup_action: PendingStartupAction::EnableElevatedRunWithStartup,
        ..AppSettings::default()
    };
    startup::apply_pending_action(&mut settings).unwrap();
    assert!(!settings.run_elevated);
    assert_eq!(settings.pending_startup_action, PendingStartupAction::None);
}

#[test]
fn external_link_allowlist_contains_only_the_current_project() {
    assert!(is_allowed_external_link(
        "https://github.com/xuzenghui942-dot/better_touch_bar_in_windows"
    ));
    assert!(is_allowed_external_link(
        "https://github.com/xuzenghui942-dot/better_touch_bar_in_windows/issues/new"
    ));
    assert!(!is_allowed_external_link(
        "https://github.com/bwya77/swoosh"
    ));
    assert!(!is_allowed_external_link(
        "https://www.paypal.com/paypalme/themsou"
    ));
    assert!(!is_allowed_external_link("file:///etc/passwd"));
}

#[test]
fn generated_tray_icon_has_the_expected_rgba_shape() {
    let pixels = create_tray_rgba(32);
    assert_eq!(pixels.len(), 32 * 32 * 4);
    assert!(pixels.chunks_exact(4).any(|pixel| pixel[3] == 255));
    assert!(pixels.chunks_exact(4).any(|pixel| pixel[3] == 0));
}
