const HTML: &str = include_str!("../../ui/index.html");
const SCRIPT: &str = include_str!("../../ui/app.js");

#[test]
fn ui_contains_every_existing_settings_area() {
    for required in [
        "touchpad-status",
        "contact-preview",
        "three-finger-enabled",
        "drag-button",
        "allow-release",
        "release-delay",
        "device-settings",
        "start-threshold",
        "stop-threshold",
        "max-move-distance",
        "cursor-averaging",
        "run-at-startup",
        "run-elevated",
        "record-logs",
        "save-logs",
        "panel-advanced-home",
        "panel-advanced-snapping",
        "advanced-enabled",
        "advanced-halves",
        "advanced-five-finger",
        "advanced-status",
    ] {
        assert!(
            HTML.contains(required),
            "missing original settings control: {required}"
        );
    }
}

#[test]
fn ui_has_no_out_of_scope_gesture_or_edge_controls() {
    let combined = format!("{HTML}\n{SCRIPT}").to_ascii_lowercase();
    for forbidden in [
        "四指",
        "four-finger",
        "边缘滑动",
        "edge gesture",
        "音量",
        "volume",
        "亮度",
        "brightness",
        "平滑滚动",
        "smooth scroll",
    ] {
        assert!(
            !combined.contains(forbidden),
            "out-of-scope capability appeared in the UI: {forbidden}"
        );
    }
}

#[test]
fn ui_invokes_all_mutating_backend_commands() {
    for command in [
        "save_gesture_settings",
        "set_record_logs",
        "set_run_at_startup",
        "set_run_elevated",
        "save_logs",
        "open_touchpad_settings",
        "open_external",
        "close_settings",
        "save_advanced_settings",
        "restore_advanced_defaults",
    ] {
        assert!(
            SCRIPT.contains(command),
            "UI does not invoke backend command: {command}"
        );
    }
}

#[test]
fn advanced_pages_mirror_the_four_swoosh_settings_surfaces_in_chinese() {
    for required in [
        "swoosh-settings-shell",
        "swoosh-sidebar",
        "swoosh-hero",
        "panel-advanced-home",
        "panel-advanced-snapping",
        "panel-advanced-apps",
        "panel-advanced-appearance",
        "手势与动画",
        "启动与故障排查",
        "吸附行为",
        "双指调整大小",
        "虚拟桌面与显示器",
        "列表中的应用如何处理",
        "高亮颜色",
        "手势 HUD",
    ] {
        assert!(
            HTML.contains(required),
            "missing Swoosh surface: {required}"
        );
    }
}

#[test]
fn swoosh_snapping_page_uses_source_style_gesture_tiles_and_sliders() {
    for required in [
        "swoosh-gesture-grid",
        "data-gesture-toggle=\"maximizeEnabled\"",
        "data-gesture-toggle=\"halvesEnabled\"",
        "data-gesture-toggle=\"quartersEnabled\"",
        "data-gesture-toggle=\"minimizeEnabled\"",
        "data-gesture-toggle=\"centerEnabled\"",
        "data-gesture-toggle=\"gridModifierEnabled\"",
        "type=\"range\" data-advanced-control=\"swipeDownThreshold\"",
        "type=\"range\" data-advanced-control=\"sensitivity\"",
        "type=\"range\" data-advanced-control=\"gridSpacing\"",
        "type=\"range\" data-advanced-control=\"cancelTimeoutSeconds\"",
        "type=\"range\" data-advanced-control=\"desktopHoldDelaySeconds\"",
    ] {
        assert!(
            HTML.contains(required),
            "missing source-style control: {required}"
        );
    }
}

#[test]
fn swoosh_apps_and_appearance_keep_all_source_interactions() {
    for required in [
        "advanced-search-apps",
        "advanced-list-installed-apps",
        "advanced-list-running-apps",
        "advanced-selected-apps",
        "advanced-additional-apps",
        "data-overlay-color=\"#0A84FF\"",
        "data-overlay-color=\"#5AC8FA\"",
        "data-overlay-color=\"#34C759\"",
        "data-overlay-color=\"#AF52DE\"",
        "data-overlay-color=\"#FF2D55\"",
        "data-overlay-color=\"#FF9500\"",
        "data-overlay-color=\"#FFCC00\"",
        "data-overlay-color=\"#8E8E93\"",
        "type=\"range\" data-advanced-control=\"hudFadeOutSeconds\"",
    ] {
        assert!(
            HTML.contains(required),
            "missing Apps/Appearance interaction: {required}"
        );
    }
}

#[test]
fn touchpad_demo_overlay_has_been_removed() {
    assert!(!HTML.contains("advanced-demo-overlay"));
    assert!(!HTML.contains("触控板演示覆盖层"));
    assert!(!HTML.contains("仅用于录屏展示手指位置"));
}

#[test]
fn swoosh_home_actions_keep_the_source_tutorial_and_prefilled_report_flow() {
    for required in [
        "advanced-tutorial",
        "advanced-tutorial-back",
        "advanced-tutorial-next",
        "advanced-tutorial-skip",
        "将光标移到窗口标题栏",
        "最小化或关闭",
        "切换虚拟桌面或显示器",
    ] {
        assert!(
            HTML.contains(required),
            "missing tutorial surface: {required}"
        );
    }
    for required in [
        "openAdvancedTutorial",
        "renderAdvancedTutorialStep",
        "## 发生了什么？",
        "## 诊断信息",
        "advanced_diagnostics",
        "encodeURIComponent",
    ] {
        assert!(
            SCRIPT.contains(required),
            "missing source home action: {required}"
        );
    }
}
