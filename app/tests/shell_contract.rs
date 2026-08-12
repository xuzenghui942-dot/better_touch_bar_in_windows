#![cfg(windows)]

use std::path::Path;

use three_finger_drag_rust::{
    external_links::is_allowed_external_link,
    startup::{startup_command, ELEVATED_TASK_NAME, UNELEVATED_RUN_VALUE},
    tray_icon::create_tray_rgba,
};

#[test]
fn startup_targets_are_namespaced_away_from_the_original_application() {
    assert_eq!(ELEVATED_TASK_NAME, r"\ThreeFingerDragRust\Run on Startup");
    assert_eq!(UNELEVATED_RUN_VALUE, "ThreeFingerDragRust");
    assert!(!ELEVATED_TASK_NAME.contains("ThreeFingerDragOnWindows"));
}

#[test]
fn startup_command_quotes_paths_and_marks_autostart_launches() {
    assert_eq!(
        startup_command(Path::new(r"C:\Program Files\ThreeFingerDragRust\app.exe")),
        r#""C:\Program Files\ThreeFingerDragRust\app.exe" --autostart"#
    );
}

#[test]
fn external_link_allowlist_contains_only_the_existing_projects_links() {
    assert!(is_allowed_external_link(
        "https://github.com/ClementGre/ThreeFingerDragOnWindows"
    ));
    assert!(is_allowed_external_link(
        "https://github.com/ClementGre/ThreeFingerDragOnWindows/issues/new"
    ));
    assert!(is_allowed_external_link(
        "https://www.paypal.com/paypalme/themsou"
    ));
    assert!(!is_allowed_external_link("https://example.com"));
    assert!(!is_allowed_external_link(
        "file:///C:/Windows/System32/cmd.exe"
    ));
}

#[test]
fn generated_tray_icon_has_the_expected_rgba_shape() {
    let pixels = create_tray_rgba(32);
    assert_eq!(pixels.len(), 32 * 32 * 4);
    assert!(pixels.chunks_exact(4).any(|pixel| pixel[3] == 255));
    assert!(pixels.chunks_exact(4).any(|pixel| pixel[3] == 0));
}
