use better_touch_advanced_gestures::config::{
    AdvancedConfig, AppCompatibilityMode, GridModifier, HudSize, HudTheme, SwipeDownMode,
};
use better_touch_advanced_gestures::geometry::{Rect, SnapZone, zone_rect};
use better_touch_advanced_gestures::taskbar::{
    TaskbarGestureAction, TaskbarGestureDirection, action_for_taskbar_gesture,
};

#[test]
fn advanced_module_is_off_by_default_while_swoosh_subdefaults_are_preserved() {
    let settings = AdvancedConfig::default();

    assert!(!settings.enabled, "the advanced module must be opt-in");
    assert!(settings.animate_snaps);
    assert_eq!(settings.snap_animation_seconds, 0.22);
    assert!(settings.maximize_enabled);
    assert!(settings.halves_enabled);
    assert!(settings.quarters_enabled);
    assert!(settings.minimize_enabled);
    assert!(settings.four_finger_swipe_down_minimize_all_enabled);
    assert_eq!(settings.swipe_down_action, SwipeDownMode::Minimize);
    assert_eq!(settings.swipe_down_threshold, 0.15);
    assert_eq!(settings.grid_spacing, 0);
    assert_eq!(settings.cancel_timeout_seconds, 0.9);
    assert!(!settings.live_preview);
    assert!(!settings.move_cursor);
    assert!(!settings.mouse_middle_button_hud_enabled);
    assert!(!settings.resize_horizontal_enabled);
    assert!(!settings.resize_vertical_enabled);
    assert!(settings.five_finger_enabled);
    assert!(settings.center_enabled);
    assert!(!settings.app_switch_on_hold);
    assert!(settings.monitor_move_enabled);
    assert_eq!(settings.monitor_move_modifier, GridModifier::Alt);
    assert!(settings.preview_desktop_destination);
    assert!(!settings.create_desktop_on_overflow);
    assert_eq!(settings.desktop_hold_delay_seconds, 0.3);
    assert!(settings.phantom_rejection);
    assert!(!settings.onboarding_completed);
    assert!(!settings.enable_on_app_start);
    assert!(settings.taskbar_icon_gestures_enabled);
    assert!(settings.gestures_enabled);
    assert!(settings.app_compatibility_process_names.is_empty());
    assert_eq!(
        settings.app_compatibility_mode,
        AppCompatibilityMode::Exclude
    );
    assert_eq!(settings.app_compatibility_modifier, GridModifier::Ctrl);
    assert!(settings.overlay_use_accent);
    assert_eq!(settings.hud_background, HudTheme::Dark);
    assert_eq!(settings.hud_size, HudSize::Normal);
    assert_eq!(settings.overlay_color, "#0A84FF");
    assert_eq!(settings.hud_fade_out_seconds, 0.36);
}

#[test]
fn config_json_round_trip_preserves_home_snapping_and_taskbar_values() {
    let original = AdvancedConfig {
        enabled: true,
        animate_snaps: false,
        snap_animation_seconds: 0.31,
        maximize_enabled: false,
        halves_enabled: false,
        quarters_enabled: false,
        minimize_enabled: false,
        four_finger_swipe_down_minimize_all_enabled: false,
        swipe_down_action: SwipeDownMode::Choose,
        swipe_down_threshold: 0.23,
        grid_spacing: 7,
        cancel_timeout_seconds: 1.7,
        live_preview: true,
        move_cursor: true,
        mouse_middle_button_hud_enabled: true,
        resize_horizontal_enabled: true,
        resize_vertical_enabled: true,
        five_finger_enabled: false,
        center_enabled: false,
        app_switch_on_hold: true,
        monitor_move_enabled: false,
        monitor_move_modifier: GridModifier::Ctrl,
        preview_desktop_destination: false,
        create_desktop_on_overflow: true,
        desktop_hold_delay_seconds: 0.8,
        phantom_rejection: false,
        onboarding_completed: true,
        enable_on_app_start: true,
        taskbar_icon_gestures_enabled: false,
        gestures_enabled: false,
        app_compatibility_process_names: vec!["Firefox".into(), "C:\\Apps\\Brave.exe".into()],
        app_compatibility_mode: AppCompatibilityMode::RequireModifier,
        app_compatibility_modifier: GridModifier::Alt,
        overlay_use_accent: false,
        hud_background: HudTheme::Light,
        hud_size: HudSize::Large,
        launch_at_login: true,
        overlay_color: "#FF2D55".into(),
        hud_fade_out_seconds: 1.2,
    };

    let json = serde_json::to_string(&original).unwrap();
    let copy: AdvancedConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(copy, original);
    assert!(json.contains("\"taskbarIconGesturesEnabled\""));
    assert!(json.contains("\"fourFingerSwipeDownMinimizeAllEnabled\""));
    assert!(json.contains("\"overlayColor\""));
    assert!(json.contains("\"appCompatibilityProcessNames\""));
}

#[test]
fn serialized_config_has_no_three_column_feature_contract() {
    let json = serde_json::to_value(AdvancedConfig::default()).unwrap();
    let settings = json.as_object().unwrap();

    for removed in ["gridModifierEnabled", "gridModifier", "sensitivity"] {
        assert!(
            !settings.contains_key(removed),
            "removed three-column setting remains in the public config: {removed}"
        );
    }
}

#[test]
fn rust_json_schema_covers_the_integrated_home_and_snapping_contract() {
    let value = serde_json::to_value(AdvancedConfig::default()).unwrap();
    let mut actual = value
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    actual.sort();
    let mut expected = vec![
        "animateSnaps",
        "appCompatibilityMode",
        "appCompatibilityModifier",
        "appCompatibilityProcessNames",
        "appSwitchOnHold",
        "cancelTimeoutSeconds",
        "centerEnabled",
        "createDesktopOnOverflow",
        "desktopHoldDelaySeconds",
        "enableOnAppStart",
        "enabled",
        "gesturesEnabled",
        "fiveFingerEnabled",
        "fourFingerSwipeDownMinimizeAllEnabled",
        "gridSpacing",
        "hudBackground",
        "hudFadeOutSeconds",
        "hudSize",
        "halvesEnabled",
        "livePreview",
        "maximizeEnabled",
        "minimizeEnabled",
        "monitorMoveEnabled",
        "monitorMoveModifier",
        "mouseMiddleButtonHudEnabled",
        "moveCursor",
        "onboardingCompleted",
        "launchAtLogin",
        "overlayColor",
        "overlayUseAccent",
        "phantomRejection",
        "previewDesktopDestination",
        "quartersEnabled",
        "resizeHorizontalEnabled",
        "resizeVerticalEnabled",
        "snapAnimationSeconds",
        "swipeDownAction",
        "swipeDownThreshold",
        "taskbarIconGesturesEnabled",
    ];
    expected.sort();
    assert_eq!(actual, expected);
}

#[test]
fn normalization_replaces_non_finite_values_before_json_crosses_the_ffi() {
    let settings = AdvancedConfig {
        snap_animation_seconds: f64::NAN,
        desktop_hold_delay_seconds: f64::NEG_INFINITY,
        ..AdvancedConfig::default()
    };
    let normalized = settings.normalized();

    assert_eq!(normalized.snap_animation_seconds, 0.22);
    assert_eq!(normalized.desktop_hold_delay_seconds, 0.3);
    assert!(serde_json::to_string(&normalized).is_ok());
}

#[test]
fn halves_and_quarters_tile_odd_work_areas_without_gaps() {
    let work = Rect::new(10, 20, 1376, 789);
    let left = zone_rect(work, SnapZone::LeftHalf, 0);
    let right = zone_rect(work, SnapZone::RightHalf, 0);
    assert_eq!(left.right, right.left);
    assert_eq!(right.right, work.right);

    let tl = zone_rect(work, SnapZone::TopLeft, 0);
    let tr = zone_rect(work, SnapZone::TopRight, 0);
    let bl = zone_rect(work, SnapZone::BottomLeft, 0);
    let br = zone_rect(work, SnapZone::BottomRight, 0);
    assert_eq!(tl.right, tr.left);
    assert_eq!(tl.bottom, bl.top);
    assert_eq!(br.right, work.right);
    assert_eq!(br.bottom, work.bottom);
    assert_eq!(tl.area() + tr.area() + bl.area() + br.area(), work.area());
}

#[test]
fn grid_spacing_is_inset_once_at_outer_edges_and_between_tiles() {
    let work = Rect::new(0, 0, 1200, 800);
    let left = zone_rect(work, SnapZone::LeftHalf, 8);
    let right = zone_rect(work, SnapZone::RightHalf, 8);

    assert_eq!(left, Rect::new(8, 8, 592, 792));
    assert_eq!(right, Rect::new(608, 8, 1192, 792));
}

#[test]
fn taskbar_gesture_mapping_never_falls_back_to_the_foreground_window() {
    let target_under_cursor = 0xBEEFu64;

    assert_eq!(
        action_for_taskbar_gesture(Some(target_under_cursor), TaskbarGestureDirection::Up),
        Some(TaskbarGestureAction::Activate {
            target: target_under_cursor
        })
    );
    assert_eq!(
        action_for_taskbar_gesture(Some(target_under_cursor), TaskbarGestureDirection::Left),
        Some(TaskbarGestureAction::ActivateAndSnap {
            target: target_under_cursor,
            zone: SnapZone::LeftHalf,
        })
    );
    assert_eq!(
        action_for_taskbar_gesture(Some(target_under_cursor), TaskbarGestureDirection::Right),
        Some(TaskbarGestureAction::ActivateAndSnap {
            target: target_under_cursor,
            zone: SnapZone::RightHalf,
        })
    );
    assert_eq!(
        action_for_taskbar_gesture(None, TaskbarGestureDirection::Left),
        None,
        "no icon under the cursor means no action; foreground is not a fallback"
    );
    assert_eq!(
        action_for_taskbar_gesture(Some(target_under_cursor), TaskbarGestureDirection::Down),
        None
    );
}
