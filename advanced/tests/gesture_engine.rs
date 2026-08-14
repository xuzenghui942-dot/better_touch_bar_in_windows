use better_touch_advanced_gestures::gesture::{
    Contact, GestureEngine, GestureEvent, SwipeDirection, TouchFrame,
};

fn frame(timestamp_ms: i64, points: &[(f64, f64)]) -> TouchFrame {
    TouchFrame {
        timestamp_ms,
        contacts: points
            .iter()
            .enumerate()
            .map(|(id, &(x, y))| Contact {
                id: id as i32,
                x,
                y,
            })
            .collect(),
    }
}

fn two(timestamp_ms: i64, cx: f64, cy: f64, gap: f64) -> TouchFrame {
    frame(timestamp_ms, &[(cx - gap / 2.0, cy), (cx + gap / 2.0, cy)])
}

#[test]
fn chooser_cancel_can_reseed_the_previous_snap_direction() {
    let mut engine = GestureEngine::default();
    let _ = engine.process(&two(0, 0.50, 0.50, 0.12));
    let _ = engine.process(&two(40, 0.50, 0.68, 0.12));

    engine.rebaseline_seed(SwipeDirection::Left);
    let events = engine.process(&two(50, 0.50, 0.68, 0.12));

    assert!(events.iter().any(|event| matches!(
        event,
        GestureEvent::Updated {
            direction: SwipeDirection::Left,
            ..
        }
    )));
}

#[test]
fn classify_matches_all_eight_screen_directions() {
    use SwipeDirection::*;
    let cases = [
        ((-1.0, 0.0), Left),
        ((1.0, 0.0), Right),
        ((0.0, -1.0), Up),
        ((0.0, 1.0), Down),
        ((-1.0, -1.0), UpLeft),
        ((1.0, -1.0), UpRight),
        ((-1.0, 1.0), DownLeft),
        ((1.0, 1.0), DownRight),
    ];
    for ((dx, dy), expected) in cases {
        assert_eq!(GestureEngine::classify(dx, dy), expected);
    }
}

#[test]
fn cardinal_swipes_tolerate_natural_off_axis_drift() {
    use SwipeDirection::*;

    // Captured from the failed VS Code right-to-left gesture: the horizontal
    // displacement is more than twice the vertical drift, so this is a half-
    // screen gesture rather than a quarter-screen diagonal.
    assert_eq!(
        GestureEngine::classify(-0.13063388768377926, -0.059152305542617634),
        Left
    );
    assert_eq!(GestureEngine::classify(0.13, 0.059), Right);

    // A deliberate diagonal remains available for quarter-screen snapping.
    assert_eq!(GestureEngine::classify(-0.10, -0.10), UpLeft);
    assert_eq!(GestureEngine::classify(0.10, 0.10), DownRight);
}

#[test]
fn captured_horizontal_swipe_commits_to_half_screen() {
    let mut engine = GestureEngine::default();
    engine.process(&two(0, 0.50, 0.50, 0.20));
    engine.process(&two(
        20,
        0.50 - 0.13063388768377926,
        0.50 - 0.059152305542617634,
        0.20,
    ));

    assert_eq!(
        engine.process(&frame(30, &[])),
        vec![GestureEvent::Completed(SwipeDirection::Left)]
    );
}

#[test]
fn two_finger_swipe_commits_on_lift_and_dead_zone_does_not() {
    let mut engine = GestureEngine::default();
    engine.process(&two(0, 0.50, 0.50, 0.20));
    let moved = engine.process(&two(20, 0.36, 0.50, 0.20));
    assert!(moved.iter().any(|e| matches!(
        e,
        GestureEvent::Updated {
            direction: SwipeDirection::Left,
            ..
        }
    )));
    assert_eq!(
        engine.process(&frame(30, &[])),
        vec![GestureEvent::Completed(SwipeDirection::Left)]
    );

    let mut tiny = GestureEngine::default();
    tiny.process(&two(0, 0.50, 0.50, 0.20));
    tiny.process(&two(20, 0.53, 0.50, 0.20));
    assert_eq!(tiny.process(&frame(30, &[])), vec![GestureEvent::Cancelled]);
}

#[test]
fn three_or_four_contacts_cancel_without_later_commit() {
    let mut engine = GestureEngine::default();
    engine.process(&two(0, 0.50, 0.50, 0.20));
    engine.process(&two(20, 0.36, 0.50, 0.20));
    assert_eq!(
        engine.process(&frame(30, &[(0.2, 0.2), (0.3, 0.2), (0.4, 0.2)])),
        vec![GestureEvent::Cancelled]
    );
    assert!(engine.process(&frame(40, &[])).is_empty());
}

#[test]
fn disabled_engine_never_tracks_or_emits_actions() {
    let mut engine = GestureEngine::default();
    engine.enabled = false;
    assert!(engine.process(&two(0, 0.5, 0.5, 0.2)).is_empty());
    assert!(engine.process(&two(20, 0.3, 0.5, 0.2)).is_empty());
    assert!(engine.process(&frame(30, &[])).is_empty());
}

#[test]
fn still_two_finger_touch_engages_hold_and_commits_aim_on_release() {
    let mut engine = GestureEngine::default();
    engine.desktop_move_on_release = true;
    engine.process(&two(0, 0.5, 0.5, 0.2));
    engine.process(&two(50, 0.5, 0.5, 0.2));
    engine.process(&two(100, 0.5, 0.5, 0.2));
    engine.process(&two(150, 0.5, 0.5, 0.2));
    let dwell = engine.process(&two(200, 0.5, 0.5, 0.2));
    assert!(dwell.contains(&GestureEvent::HoldEngaged));
    let aim = engine.process(&two(220, 0.70, 0.5, 0.2));
    assert!(
        aim.iter()
            .any(|e| matches!(e, GestureEvent::HoldUpdated { aim_steps: 2, .. }))
    );
    assert_eq!(
        engine.process(&frame(240, &[])),
        vec![GestureEvent::DesktopHoldCommit(2)]
    );
}

#[test]
fn two_finger_pinch_fires_only_with_a_still_centroid() {
    let mut pinch = GestureEngine::default();
    pinch.process(&two(0, 0.5, 0.5, 0.12));
    let events = pinch.process(&two(30, 0.5, 0.5, 0.24));
    assert!(events.contains(&GestureEvent::PinchOut));

    let mut translating = GestureEngine::default();
    translating.process(&two(0, 0.5, 0.5, 0.12));
    let events = translating.process(&two(30, 0.65, 0.5, 0.24));
    assert!(!events.contains(&GestureEvent::PinchOut));
}

#[test]
fn five_finger_tap_is_distinguished_from_free_move() {
    let five = &[
        (0.30, 0.4),
        (0.40, 0.4),
        (0.50, 0.4),
        (0.60, 0.4),
        (0.70, 0.4),
    ];
    let mut tap = GestureEngine::default();
    assert_eq!(
        tap.process(&frame(0, five)),
        vec![GestureEvent::FreeMoveBegan]
    );
    assert_eq!(
        tap.process(&frame(150, &[])),
        vec![GestureEvent::FreeMoveEnded {
            was_tap: true,
            cancelled: false,
        }]
    );

    let moved = &[
        (0.40, 0.4),
        (0.50, 0.4),
        (0.60, 0.4),
        (0.70, 0.4),
        (0.80, 0.4),
    ];
    let mut drag = GestureEngine::default();
    drag.process(&frame(0, five));
    drag.process(&frame(40, moved));
    assert_eq!(
        drag.process(&frame(100, &[])),
        vec![GestureEvent::FreeMoveEnded {
            was_tap: false,
            cancelled: false,
        }]
    );
}

#[test]
fn cancelling_free_move_is_not_reported_as_a_completed_drag() {
    let five = &[
        (0.30, 0.4),
        (0.40, 0.4),
        (0.50, 0.4),
        (0.60, 0.4),
        (0.70, 0.4),
    ];
    let mut engine = GestureEngine::default();
    engine.process(&frame(0, five));
    assert_eq!(
        engine.cancel(),
        vec![GestureEvent::FreeMoveEnded {
            was_tap: false,
            cancelled: true,
        }]
    );
}
