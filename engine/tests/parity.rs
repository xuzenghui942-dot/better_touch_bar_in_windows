use std::collections::BTreeMap;

use approx::assert_abs_diff_eq;
use three_finger_drag_core::{
    contacts::{AssembledContacts, ContactAssembler},
    gesture::{
        same_contact_ids, Contact, DragEngine, FrameResult, MouseAction, TimerDirective,
        RELEASE_GAP_MS,
    },
    settings::{AppSettings, DeviceDragSettings, DragButton},
    speed::{apply_cursor_response, apply_threshold_speed},
};

fn contact(id: i32, x: i32, y: i32) -> Contact {
    Contact { id, x, y }
}

fn three_at(x: i32) -> Vec<Contact> {
    vec![contact(1, x, 100), contact(2, x, 200), contact(3, x, 300)]
}

fn device_settings(speed: f32, acceleration: f32) -> DeviceDragSettings {
    DeviceDragSettings {
        cursor_move: true,
        cursor_speed: speed,
        cursor_acceleration: acceleration,
    }
}

fn test_settings() -> AppSettings {
    let mut settings = AppSettings {
        start_threshold: 0,
        stop_threshold: 0,
        ..AppSettings::default()
    };
    settings
        .devices
        .insert("device-a".into(), device_settings(60.0, 0.0));
    settings
}

fn begin_drag(engine: &mut DragEngine, settings: &AppSettings) {
    engine.process_frame("device-a", three_at(100), 0, settings);
    engine.process_frame("device-a", three_at(101), RELEASE_GAP_MS, settings);
    let start = engine.process_frame("device-a", three_at(102), RELEASE_GAP_MS + 1, settings);
    assert_eq!(start.mouse, vec![MouseAction::ButtonDown(DragButton::Left)]);
}

#[test]
fn fresh_defaults_match_the_current_csharp_application() {
    let settings = AppSettings::default();
    let device = DeviceDragSettings::default();

    assert_eq!(settings.version, 1);
    assert!(settings.three_finger_drag);
    assert_eq!(settings.drag_button, DragButton::Left);
    assert!(settings.allow_release_and_restart);
    assert_eq!(settings.release_delay_ms, 500);
    assert!(settings.devices.is_empty());
    assert_eq!(settings.cursor_averaging, 1);
    assert_eq!(settings.max_finger_move_distance, 0);
    assert_eq!(settings.start_threshold, 100);
    assert_eq!(settings.stop_threshold, 10);
    assert!(!settings.run_at_startup);
    assert!(settings.run_elevated);
    assert!(!settings.record_logs);

    assert!(device.cursor_move);
    assert_abs_diff_eq!(device.cursor_speed, 30.0);
    assert_abs_diff_eq!(device.cursor_acceleration, 10.0);
}

#[test]
fn settings_normalization_preserves_the_start_stop_invariant() {
    let mut settings = AppSettings {
        start_threshold: 10,
        stop_threshold: 40,
        cursor_averaging: 0,
        release_delay_ms: 9_999,
        ..AppSettings::default()
    };

    settings.normalize();

    assert_eq!(settings.start_threshold, 40);
    assert_eq!(settings.stop_threshold, 40);
    assert_eq!(settings.cursor_averaging, 1);
    assert_eq!(settings.release_delay_ms, 2_000);
}

#[test]
fn settings_round_trip_keeps_per_device_values() {
    let settings = AppSettings {
        devices: BTreeMap::from([(
            "touchpad-hash".into(),
            DeviceDragSettings {
                cursor_move: false,
                cursor_speed: 52.0,
                cursor_acceleration: 0.0,
            },
        )]),
        ..AppSettings::default()
    };

    let encoded = serde_json::to_string_pretty(&settings).unwrap();
    let decoded: AppSettings = serde_json::from_str(&encoded).unwrap();

    assert_eq!(decoded, settings);
}

#[test]
fn contact_identity_is_order_independent_but_requires_equal_lengths() {
    let first = vec![contact(1, 10, 10), contact(2, 20, 20), contact(3, 30, 30)];
    let reordered = vec![contact(3, 31, 30), contact(1, 11, 10), contact(2, 21, 20)];

    assert!(same_contact_ids(&first, &reordered));
    assert!(!same_contact_ids(&first, &reordered[..2]));
}

#[test]
fn complete_contact_report_is_forwarded_and_clears_partial_state() {
    let mut assembler = ContactAssembler::default();
    assert_eq!(
        assembler.accept(vec![contact(1, 10, 10)], 2),
        AssembledContacts::Pending
    );

    let complete = vec![contact(1, 11, 10), contact(2, 21, 20)];
    assert_eq!(
        assembler.accept(complete.clone(), 2),
        AssembledContacts::Complete(complete)
    );
    assert_eq!(assembler.pending_len(), 0);
}

#[test]
fn split_reports_are_reassembled_and_duplicate_ids_are_removed() {
    let mut assembler = ContactAssembler::default();
    assert_eq!(
        assembler.accept(vec![contact(1, 10, 10), contact(2, 20, 20)], 3),
        AssembledContacts::Pending
    );

    assert_eq!(
        assembler.accept(vec![contact(2, 22, 20), contact(3, 30, 30)], 0),
        AssembledContacts::Complete(vec![
            contact(1, 10, 10),
            contact(2, 20, 20),
            contact(3, 30, 30),
        ])
    );
}

#[test]
fn empty_reports_are_ignored_like_the_current_application() {
    let mut assembler = ContactAssembler::default();
    assert_eq!(assembler.accept(Vec::new(), 0), AssembledContacts::Ignored);
}

#[test]
fn a_stale_partial_report_is_padded_before_the_next_incomplete_report() {
    let mut assembler = ContactAssembler::default();
    assert_eq!(
        assembler.accept(vec![contact(5, 10, 10), contact(7, 20, 20)], 4),
        AssembledContacts::Pending
    );

    let result = assembler.accept(vec![contact(1, 100, 100)], 3);
    assert_eq!(
        result,
        AssembledContacts::CompleteThenPending {
            complete: vec![
                contact(5, 10, 10),
                contact(7, 20, 20),
                contact(8, 20, 20),
                contact(9, 20, 20),
            ],
            pending: vec![contact(1, 100, 100)],
            target: 3,
        }
    );
}

#[test]
fn threshold_speed_uses_the_original_divisor() {
    let device = device_settings(30.0, 0.0);
    assert_abs_diff_eq!(apply_threshold_speed(120.0, &device), 60.0);
}

#[test]
fn cursor_response_uses_the_original_speed_and_acceleration_curve() {
    let no_acceleration = device_settings(60.0, 0.0);
    let delta = apply_cursor_response((12.0, -8.0), 10, &no_acceleration);
    assert_abs_diff_eq!(delta.0, 6.0, epsilon = 0.0001);
    assert_abs_diff_eq!(delta.1, -4.0, epsilon = 0.0001);

    let accelerated = device_settings(30.0, 10.0);
    let accelerated_delta = apply_cursor_response((12.0, 0.0), 10, &accelerated);
    assert_abs_diff_eq!(accelerated_delta.0, 2.272_704, epsilon = 0.000_1);
}

#[test]
fn high_but_allowed_acceleration_uses_the_original_double_precision_curve() {
    let high_acceleration = device_settings(120.0, 200.0);
    let delta = apply_cursor_response((120.0, 0.0), 1, &high_acceleration);

    assert!(delta.0.is_finite());
    assert_abs_diff_eq!(delta.0, 180.0, epsilon = 0.001);
    assert_abs_diff_eq!(delta.1, 0.0, epsilon = 0.001);
}

#[test]
fn three_stable_contacts_start_drag_only_after_quarantine() {
    let settings = test_settings();
    let mut engine = DragEngine::default();

    assert_eq!(
        engine.process_frame("device-a", three_at(100), 0, &settings),
        FrameResult::default()
    );
    assert_eq!(
        engine.process_frame("device-a", three_at(101), RELEASE_GAP_MS, &settings),
        FrameResult::default()
    );

    let result = engine.process_frame("device-a", three_at(102), RELEASE_GAP_MS + 1, &settings);
    assert_eq!(
        result.mouse,
        vec![MouseAction::ButtonDown(DragButton::Left)]
    );
    assert_eq!(result.timer, TimerDirective::Keep);
    assert!(engine.is_dragging());
}

#[test]
fn active_drag_moves_by_the_longest_contact_delta_and_arms_release_timer() {
    let settings = test_settings();
    let mut engine = DragEngine::default();
    begin_drag(&mut engine, &settings);

    let contacts = vec![
        contact(1, 112, 100),
        contact(2, 107, 200),
        contact(3, 105, 300),
    ];
    let result = engine.process_frame("device-a", contacts, RELEASE_GAP_MS + 11, &settings);

    assert_eq!(result.mouse, vec![MouseAction::Move { dx: 5.0, dy: 0.0 }]);
    assert_eq!(result.timer, TimerDirective::Arm(500));
}

#[test]
fn disabling_restart_uses_the_forty_millisecond_release_floor() {
    let mut settings = test_settings();
    settings.allow_release_and_restart = false;
    settings.release_delay_ms = 2_000;
    let mut engine = DragEngine::default();
    begin_drag(&mut engine, &settings);

    let result = engine.process_frame("device-a", three_at(112), RELEASE_GAP_MS + 11, &settings);

    assert_eq!(result.timer, TimerDirective::Arm(RELEASE_GAP_MS as u32));
}

#[test]
fn release_timeout_emits_the_matching_button_up_once() {
    let settings = test_settings();
    let mut engine = DragEngine::default();
    begin_drag(&mut engine, &settings);

    assert_eq!(
        engine.release_timeout(&settings),
        vec![MouseAction::ButtonUp(DragButton::Left)]
    );
    assert!(engine.release_timeout(&settings).is_empty());
    assert!(!engine.is_dragging());
}

#[test]
fn absent_device_cursor_config_starts_click_but_does_not_move_or_arm_timer() {
    let mut settings = test_settings();
    settings.devices.clear();
    let mut engine = DragEngine::default();
    engine.process_frame("device-a", three_at(100), 0, &settings);
    engine.process_frame("device-a", three_at(101), RELEASE_GAP_MS, &settings);
    let start = engine.process_frame("device-a", three_at(103), RELEASE_GAP_MS + 1, &settings);
    assert_eq!(start.mouse, vec![MouseAction::ButtonDown(DragButton::Left)]);

    let movement = engine.process_frame("device-a", three_at(113), RELEASE_GAP_MS + 11, &settings);
    assert_eq!(movement, FrameResult::default());
}

#[test]
fn cursor_averaging_emits_only_after_the_configured_number_of_frames() {
    let mut settings = test_settings();
    settings.cursor_averaging = 2;
    let mut engine = DragEngine::default();
    begin_drag(&mut engine, &settings);

    let first = engine.process_frame("device-a", three_at(106), RELEASE_GAP_MS + 11, &settings);
    assert!(first.mouse.is_empty());
    assert_eq!(first.timer, TimerDirective::Arm(500));

    let second = engine.process_frame("device-a", three_at(110), RELEASE_GAP_MS + 21, &settings);
    assert_eq!(second.mouse, vec![MouseAction::Move { dx: 4.0, dy: 0.0 }]);
}

#[test]
fn excessive_single_frame_motion_is_discarded_but_timer_is_refreshed() {
    let mut settings = test_settings();
    settings.max_finger_move_distance = 5;
    let mut engine = DragEngine::default();
    begin_drag(&mut engine, &settings);

    let result = engine.process_frame("device-a", three_at(112), RELEASE_GAP_MS + 11, &settings);

    assert!(result.mouse.is_empty());
    assert_eq!(result.timer, TimerDirective::Arm(500));
}

#[test]
fn disabled_frames_keep_the_same_input_baseline_as_the_original_handler() {
    let settings = test_settings();
    let mut engine = DragEngine::default();
    begin_drag(&mut engine, &settings);

    engine.observe_inactive_frame(three_at(200), 200);
    let resumed = engine.process_frame("device-a", three_at(202), 210, &settings);

    assert_eq!(resumed.mouse, vec![MouseAction::Move { dx: 1.0, dy: 0.0 }]);
    assert_eq!(resumed.timer, TimerDirective::Arm(500));
}
