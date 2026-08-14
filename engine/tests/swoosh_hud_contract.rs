use three_finger_drag_core::{
    advanced_gestures::{config::HudSize, geometry::SnapZone, gesture::SwipeDirection},
    swoosh_hud::{
        chooser_was_reversed, restore_direction_after_dwell, should_show_snap_hud, snap_fraction,
        HudGeometry, DOWN_REVERSE_BAND, HUD_ARM_GUARD_MS, PRIOR_DIRECTION_DWELL_MS,
    },
};

const ACTIONS: &str = include_str!("../src/win32/advanced_actions.rs");

#[test]
fn source_hud_geometry_matches_cursor_chip_overlay() {
    assert_eq!(HudGeometry::MARGIN, 3.0);
    assert_eq!(HudGeometry::STROKE, 2.5);
    assert_eq!(HudGeometry::CORNER, 9.0);
    assert_eq!(HudGeometry::CHIP_HEIGHT, 60.0);
    assert_eq!(HudGeometry::SINGLE_CHIP_WIDTH, 94.0);
    assert_eq!(HudGeometry::BASE_HEIGHT_PX, 46.0);
    assert_eq!(HudGeometry::scale(HudSize::Normal), 0.65);
    assert_eq!(HudGeometry::scale(HudSize::Large), 1.0);
}

#[test]
fn snap_chip_uses_the_same_fractional_targets_as_swoosh() {
    assert_eq!(
        snap_fraction(SnapZone::LeftHalf),
        Some((0.0, 0.0, 0.5, 1.0))
    );
    assert_eq!(
        snap_fraction(SnapZone::TopRight),
        Some((0.5, 0.0, 1.0, 0.5))
    );
    assert_eq!(snap_fraction(SnapZone::Center), Some((0.2, 0.2, 0.8, 0.8)));
    assert_eq!(
        snap_fraction(SnapZone::Minimize),
        Some((0.32, 0.82, 0.68, 1.0))
    );
    assert_eq!(snap_fraction(SnapZone::None), None);
}

#[test]
fn at_rest_chip_waits_for_the_source_arm_guard_but_real_swipes_do_not() {
    assert_eq!(HUD_ARM_GUARD_MS, 110);
    assert!(!should_show_snap_hud(false, 500, SnapZone::LeftHalf));
    assert!(!should_show_snap_hud(true, 109, SnapZone::None));
    assert!(should_show_snap_hud(true, 110, SnapZone::None));
    assert!(should_show_snap_hud(true, 0, SnapZone::LeftHalf));
}

#[test]
fn touchpad_demo_window_is_not_part_of_the_rust_runtime() {
    assert!(!ACTIONS.contains("AdvancedDemo"));
    assert!(!ACTIONS.contains("demo_contacts"));
}

#[test]
fn source_down_chooser_cancel_guardrails_are_preserved() {
    assert_eq!(PRIOR_DIRECTION_DWELL_MS, 150);
    assert_eq!(DOWN_REVERSE_BAND, 0.05);
    assert_eq!(
        restore_direction_after_dwell(SwipeDirection::Left, 149),
        SwipeDirection::None
    );
    assert_eq!(
        restore_direction_after_dwell(SwipeDirection::Left, 150),
        SwipeDirection::Left
    );
    assert!(!chooser_was_reversed(0.36, 0.40));
    assert!(chooser_was_reversed(0.349, 0.40));
}
