const HTML: &str = include_str!("../../ui/index.html");
const SCRIPT: &str = include_str!("../../ui/app.js");
const STYLE: &str = include_str!("../../ui/styles.css");

#[test]
fn linux_ui_keeps_every_basic_drag_control() {
    for required in [
        "touchpad-status",
        "touchpad-devices",
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
        "record-logs",
        "save-logs",
    ] {
        assert!(
            HTML.contains(&format!("id=\"{required}\"")),
            "missing basic drag control: {required}"
        );
    }
}

#[test]
fn linux_ui_exposes_gnome_gesture_guard_state_without_enabling_it() {
    for required in [
        "Ubuntu · GNOME · Wayland",
        "gesture-guard-status",
        "refresh-integration-status",
        "保留四指，只拦截三指",
        "Ubuntu GNOME 默认会用相同的桌面切换逻辑处理三指和四指滑动",
        "应用不会自动启用 GNOME Shell 扩展",
        "get_platform_integration",
        "gesture_guard_active",
        "gesture_guard_enabled",
        "gesture_guard_installed",
        "拦截扩展已启用，但未确认当前 Shell 活动",
    ] {
        assert!(
            format!("{HTML}\n{SCRIPT}").contains(required),
            "missing Linux integration contract: {required}"
        );
    }
    let success_guard = SCRIPT
        .find("integration.gesture_guard_active")
        .expect("guard success is not gated by active state");
    let success_message = SCRIPT
        .find("GNOME 三指系统手势已拦截")
        .expect("missing active guard success message");
    assert!(success_guard < success_message);
}

#[test]
fn advanced_ui_is_runtime_gated_and_keeps_the_core_snap_surface() {
    for required in [
        "高级窗口手势",
        "GNOME/Wayland broker 能力握手",
        "fail-closed",
        "broker 未就绪时此开关不可启用",
        "panel-advanced-home",
        "panel-advanced-snapping",
        "panel-advanced-apps",
        "panel-advanced-appearance",
        "advanced-enabled",
        "advanced-halves",
        "advanced-app-processes",
        "advanced-overlay-color",
        "advanced_window_gestures_available",
        "save_advanced_settings",
        "restore_advanced_defaults",
        "scheduleAdvancedSave",
    ] {
        assert!(
            format!("{HTML}\n{SCRIPT}").contains(required),
            "advanced runtime/UI contract is missing: {required}"
        );
    }
    assert!(HTML.contains("data-advanced-control"));
}

#[test]
fn advanced_config_fields_are_preserved_and_linux_startup_is_forced_off() {
    for field in [
        "enabled",
        "gesturesEnabled",
        "animateSnaps",
        "snapAnimationSeconds",
        "maximizeEnabled",
        "halvesEnabled",
        "quartersEnabled",
        "minimizeEnabled",
        "fourFingerSwipeDownMinimizeAllEnabled",
        "swipeDownAction",
        "swipeDownThreshold",
        "gridModifierEnabled",
        "gridModifier",
        "sensitivity",
        "gridSpacing",
        "cancelTimeoutSeconds",
        "livePreview",
        "moveCursor",
        "mouseMiddleButtonHudEnabled",
        "resizeHorizontalEnabled",
        "resizeVerticalEnabled",
        "fiveFingerEnabled",
        "centerEnabled",
        "appSwitchOnHold",
        "monitorMoveEnabled",
        "monitorMoveModifier",
        "previewDesktopDestination",
        "createDesktopOnOverflow",
        "desktopHoldDelaySeconds",
        "phantomRejection",
        "onboardingCompleted",
        "enableOnAppStart",
        "taskbarIconGesturesEnabled",
        "appCompatibilityProcessNames",
        "appCompatibilityMode",
        "appCompatibilityModifier",
        "overlayUseAccent",
        "hudBackground",
        "hudSize",
        "launchAtLogin",
        "overlayColor",
        "hudFadeOutSeconds",
    ] {
        assert!(
            SCRIPT.contains(field),
            "advanced field is not preserved: {field}"
        );
    }
    assert!(SCRIPT.contains("input.enableOnAppStart = false"));
    assert!(SCRIPT.contains("input.launchAtLogin = false"));
    assert!(SCRIPT.contains("input.gesturesEnabled = original.gesturesEnabled !== false"));
    assert!(!SCRIPT.contains("input.gesturesEnabled = Boolean(input.enabled)"));
    assert!(!HTML.contains("data-advanced-control=\"launchAtLogin\""));
}

#[test]
fn advanced_runtime_listener_and_saves_are_race_safe() {
    let listen = SCRIPT
        .find("await listen(\"touchpad-event\"")
        .expect("advanced event listener is missing");
    let initial_snapshot = SCRIPT
        .find("await refreshSnapshot();")
        .expect("initial snapshot is missing");
    assert!(
        listen < initial_snapshot,
        "listener must be installed before snapshot"
    );
    for required in [
        "pendingAdvancedRuntime",
        "advancedDirty",
        "advancedSaveInFlight",
        "advancedSaveQueued",
        "advancedEditRevision",
        "function renderAdvancedStatus",
        "if (advancedDirty || advancedSaveInFlight) renderAdvancedStatus",
        "queueMicrotask(requestAdvancedSave)",
    ] {
        assert!(SCRIPT.contains(required), "missing race guard: {required}");
    }
    assert!(SCRIPT.contains("return invoke(\"get_snapshot\")"));
}

#[test]
fn broker_capabilities_disable_and_sanitize_unsupported_linux_controls() {
    for capability in [
        "snapHalves",
        "snapQuarters",
        "snapThirds",
        "maximize",
        "minimize",
        "minimizeAll",
        "close",
        "workspace",
        "dynamicWorkspace",
        "monitorMove",
    ] {
        assert!(
            SCRIPT.contains(capability),
            "capability is not consumed: {capability}"
        );
    }
    for required in [
        "function applyAdvancedCapabilities",
        "function setAdvancedControlAvailability",
        "Linux 尚未实现",
        "advanced-taskbar-icon-gestures",
        "advanced-app-switch",
    ] {
        assert!(format!("{HTML}\n{SCRIPT}").contains(required));
    }
    assert!(SCRIPT.contains("input.fiveFingerEnabled = false"));
    assert!(SCRIPT.contains("input.centerEnabled = false"));
}

#[test]
fn linux_application_identifiers_preserve_explicit_exe_suffixes() {
    assert!(SCRIPT.contains("identifier.replace(/\\.desktop$/i, \"\")"));
    assert!(!SCRIPT.contains("replace(/\\.exe$/i, \"\")"));
}

#[test]
fn removed_optional_controls_stay_hidden_and_only_four_finger_down_is_added() {
    for required in [
        "advanced-four-finger-minimize-all",
        "fourFingerSwipeDownMinimizeAllEnabled",
        "四指向下全部最小化",
        "其他四指方向不变",
    ] {
        assert!(
            HTML.contains(required),
            "missing gesture arbitration warning: {required}"
        );
    }
    assert!(!HTML.contains("advanced-five-finger"));
    assert!(!HTML.contains("advanced-center"));
    for removed in [
        "advanced-live-preview",
        "advanced-move-cursor",
        "advanced-mouse-hud",
        "advanced-resize-h",
        "advanced-resize-v",
        "实时窗口预览",
        "鼠标跟随窗口",
        "二指调整大小",
    ] {
        assert!(
            !HTML.contains(removed),
            "removed control remains: {removed}"
        );
    }
    for forced_off in [
        "input.livePreview = false",
        "input.moveCursor = false",
        "input.mouseMiddleButtonHudEnabled = false",
        "input.resizeHorizontalEnabled = false",
        "input.resizeVerticalEnabled = false",
    ] {
        assert!(
            SCRIPT.contains(forced_off),
            "removed behavior is not fail-closed: {forced_off}"
        );
    }
}

#[test]
fn linux_ui_has_user_level_autostart_but_no_elevation_controls() {
    let combined = format!("{HTML}\n{SCRIPT}");
    for forbidden in [
        "id=\"run-elevated\"",
        "set_run_elevated",
        "snapshot.is_administrator",
        "UAC",
        "以管理员身份运行",
        "Windows 精确式触摸板",
        "Windows Raw Input",
        "随 Windows 启动",
    ] {
        assert!(
            !combined.contains(forbidden),
            "Windows-only or unsafe control remains: {forbidden}"
        );
    }
    assert!(HTML.contains("id=\"run-at-startup\""));
    assert!(SCRIPT.contains("set_run_at_startup"));
    assert!(HTML.contains("~/.config/autostart/three-finger-drag-linux.desktop"));
    assert!(HTML.contains("默认关闭"));
    assert!(HTML.contains("请勿以 root 身份运行"));
}

#[test]
fn linux_ui_only_links_to_the_current_project() {
    assert!(HTML.contains("https://github.com/xuzenghui942-dot/better_touch_bar_in_windows"));
    assert!(!HTML.contains("ClementGre"));
    assert!(!HTML.contains("bwya77"));
    assert!(!HTML.contains("paypal"));
}

#[test]
fn ui_invokes_every_exposed_linux_action() {
    for command in [
        "get_snapshot",
        "get_platform_integration",
        "save_gesture_settings",
        "save_advanced_settings",
        "restore_advanced_defaults",
        "list_installed_apps",
        "list_running_apps",
        "set_record_logs",
        "set_run_at_startup",
        "save_logs",
        "advanced_diagnostics",
        "open_touchpad_settings",
        "open_external",
        "close_settings",
        "quit_app",
    ] {
        assert!(
            SCRIPT.contains(&format!("\"{command}\"")),
            "UI does not invoke Linux backend command: {command}"
        );
    }
}

#[test]
fn every_javascript_id_reference_exists_in_the_document() {
    let marker = "byId(\"";
    let mut remainder = SCRIPT;
    while let Some(start) = remainder.find(marker) {
        let after = &remainder[start + marker.len()..];
        let Some(end) = after.find('"') else {
            panic!("unterminated byId reference");
        };
        let id = &after[..end];
        assert!(
            HTML.contains(&format!("id=\"{id}\"")),
            "JavaScript references a missing element: {id}"
        );
        remainder = &after[end + 1..];
    }
}

#[test]
fn linux_fonts_and_responsive_layout_are_used() {
    assert!(STYLE.contains("Ubuntu"));
    assert!(STYLE.contains("Cantarell"));
    assert!(STYLE.contains("prefers-color-scheme: dark"));
    assert!(STYLE.contains("@media (max-width: 820px)"));
    assert!(!STYLE.contains("Segoe UI"));
    assert!(!STYLE.contains("Consolas"));
}

#[test]
fn ubuntu_2604_advanced_ui_explains_exclusive_two_finger_proxy() {
    assert!(HTML.contains("Ubuntu 26.04 双指窗口操作使用独占输入代理"));
    assert!(HTML.contains("回放为普通双指滚动"));
    assert!(HTML.contains("手指移动不会合成左键拖拽"));
    assert!(HTML.contains("松手后直接提交当前方向对应的窗口动作"));
    assert!(!HTML.contains("原生标题栏拖动（Linux 固定启用）"));
    assert!(SCRIPT.contains("安全关闭"));
    assert!(SCRIPT.contains("双指输入代理或 GNOME broker 尚未完成握手"));
}

#[test]
fn bundled_gnome_broker_exposes_two_finger_actions_and_one_passive_four_finger_action() {
    const PROTOCOL: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../gnome-extension/three-finger-drag@local/protocol.js"
    ));
    const BROKER: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../gnome-extension/three-finger-drag@local/broker.js"
    ));
    const EXTENSION: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../gnome-extension/three-finger-drag@local/extension.js"
    ));
    assert!(PROTOCOL.contains("twoFinger: true"));
    assert!(PROTOCOL.contains("fiveFinger: false"));
    for capability in [
        "snapHalves: true",
        "monitorMove: true",
        "freeMove: false",
        "axisResize: true",
        "pinch: true",
        "hud: true",
        "animation: true",
        "livePreview: true",
        "moveCursor: true",
        "appSwitch: true",
        "minimizeAll: true",
    ] {
        assert!(
            PROTOCOL.contains(capability),
            "two-finger capability mismatch: {capability}"
        );
    }
    assert!(BROKER.contains("class CapabilityBroker"));
    assert!(BROKER.contains("new WindowBackend"));
    assert!(BROKER.contains("new GestureHud"));
    assert!(BROKER.contains("observeFourFingerSwipe"));
    assert!(BROKER.contains("minimizeAllOnActiveWorkspace"));
    assert!(EXTENSION.contains("get_gesture_motion_delta_unaccelerated"));
    assert!(EXTENSION.contains("Clutter.EVENT_PROPAGATE"));
    assert!(!EXTENSION.contains("arbitrateFingerScroll(event)"));
    assert!(!EXTENSION.contains("arbitrateTouchpadGesture(event)"));
    assert!(!EXTENSION.contains("GestureHud"));
    assert!(!EXTENSION.contains("WindowBackend"));
}
