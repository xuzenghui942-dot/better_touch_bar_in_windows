#![cfg(target_os = "linux")]

const INPUT_BACKEND: &str = include_str!("../src/linux/input_service.rs");
const MOUSE_BACKEND: &str = include_str!("../src/linux/mouse_output.rs");
const TOUCHPAD_PROXY: &str = include_str!("../src/linux/touchpad_proxy.rs");
const ADVANCED_TRANSPORT: &str = include_str!("../src/linux/advanced_transport.rs");
const EXTENSION: &str = include_str!("../../gnome-extension/three-finger-drag@local/extension.js");
const WINDOW_BACKEND: &str =
    include_str!("../../gnome-extension/three-finger-drag@local/windowBackend.js");

#[test]
fn basic_mode_is_read_only_and_advanced_mode_uses_a_fail_open_touchpad_proxy() {
    assert!(!INPUT_BACKEND.contains(".grab()"));
    assert!(INPUT_BACKEND.contains("OpenOptions::new()"));
    assert!(INPUT_BACKEND.contains(".read(true)"));
    assert!(!INPUT_BACKEND.contains(".write(true)"));
    assert!(INPUT_BACKEND.contains("ABS_MT_SLOT"));
    assert!(INPUT_BACKEND.contains("SYN_DROPPED"));
    assert!(MOUSE_BACKEND.contains("/dev/uinput"));
    assert!(TOUCHPAD_PROXY.contains("physical.grab()?"));
    assert!(TOUCHPAD_PROXY.contains("physical.ungrab()"));
    assert!(TOUCHPAD_PROXY.contains("Three Finger Drag proxied touchpad"));
    assert!(TOUCHPAD_PROXY.contains("TwoFingerCandidate"));
    assert!(TOUCHPAD_PROXY.contains("TwoFingerOwned"));
    assert!(TOUCHPAD_PROXY.contains("NativeUntilLift"));
    assert!(TOUCHPAD_PROXY.contains("commit_two_finger_candidate"));
    assert!(INPUT_BACKEND.contains("proxy.allows_three_finger_drag()"));
    assert!(INPUT_BACKEND.contains("two_finger_intent_is_committed"));
    assert!(INPUT_BACKEND.contains("fail_touchpad_proxy"));
    assert!(INPUT_BACKEND.contains("disable_touchpad_proxy"));
}

#[test]
fn dropped_events_reset_the_existing_proxy_without_recreating_the_input_device() {
    let recovery = INPUT_BACKEND
        .split("fn recover_from_dropped_events(")
        .nth(1)
        .expect("dropped-event recovery function must exist")
        .split("fn process_frame(")
        .next()
        .expect("recovery function must end before frame processing");

    assert!(recovery.contains("proxy.release_all()"));
    assert!(recovery.contains("get_key_state()"));
    assert!(!recovery.contains("disable_touchpad_proxy"));
}

#[test]
fn gnome_guard_blocks_exactly_three_fingers_and_preserves_four_finger_ownership() {
    assert!(EXTENSION.contains("const fingers = event.get_touchpad_gesture_finger_count()"));
    assert!(EXTENSION.contains("const exactThreeStop = this._exactThreeGuard.decide("));
    assert!(EXTENSION.contains("'swipe', phaseName, fingers"));
    assert!(EXTENSION.contains("if (exactThreeStop)"));
    assert!(!EXTENSION.contains("arbitrateTouchpadSwipe(event)"));
    assert!(!EXTENSION.contains(">= 3"));
    assert!(EXTENSION.contains("Clutter.EVENT_PROPAGATE"));
    assert!(EXTENSION.contains("get_gesture_motion_delta_unaccelerated"));
    assert!(EXTENSION.contains("observeFourFingerSwipe"));
}

#[test]
fn linux_core_releases_on_four_fingers_and_never_drags_that_frame() {
    assert!(INPUT_BACKEND.contains("if contacts.len() >= 4"));
    assert!(INPUT_BACKEND.contains("state.drag.force_release(&snapshot)"));
    assert!(INPUT_BACKEND.contains("state.drag = DragEngine::default()"));
    assert!(INPUT_BACKEND.contains("mouse.release_source(state.source_id(), logger)"));
}

#[test]
fn runtime_status_tracks_uinput_availability_without_reporting_unrelated_denials() {
    assert!(INPUT_BACKEND.contains("receiver_installed: output_available"));
    assert!(INPUT_BACKEND.contains("publish_status(&events, &devices, mouse.is_available())"));
    assert!(!INPUT_BACKEND.contains("可读触摸板仍在运行"));
}

#[test]
fn advanced_transport_is_versioned_bounded_and_never_runs_dbus_on_evdev_thread() {
    assert!(ADVANCED_TRANSPORT.contains("pub const PROTOCOL_VERSION: u32 = 1"));
    assert!(ADVANCED_TRANSPORT.contains("const QUEUE_CAPACITY"));
    assert!(ADVANCED_TRANSPORT.contains("ThreeFingerDrag GNOME D-Bus"));
    assert!(ADVANCED_TRANSPORT.contains("method_timeout"));
    assert!(ADVANCED_TRANSPORT.contains("GetCapabilities"));
    assert!(ADVANCED_TRANSPORT.contains("Configure"));
    assert!(ADVANCED_TRANSPORT.contains("Begin"));
    assert!(ADVANCED_TRANSPORT.contains("Update"));
    assert!(ADVANCED_TRANSPORT.contains("rebaseline_feedback"));
    assert!(ADVANCED_TRANSPORT.contains("Commit"));
    assert!(ADVANCED_TRANSPORT.contains("Cancel"));
    assert!(!INPUT_BACKEND.contains("zbus::blocking"));
}

#[test]
fn advanced_runtime_keeps_three_and_four_fingers_out_of_window_gestures() {
    assert!(INPUT_BACKEND.contains("if contacts == 3"));
    assert!(INPUT_BACKEND.contains("contacts == 4"));
    assert!(INPUT_BACKEND.contains("AdvancedContactDisposition::CancelAndSuppress"));
    assert!(INPUT_BACKEND.contains("state.advanced.cancel()"));
    assert!(INPUT_BACKEND.contains("config.five_finger_enabled = false"));
}

#[test]
fn two_finger_windows_parity_never_turns_a_swipe_into_a_pointer_drag() {
    for forbidden in [
        "AdvancedPointerDrag",
        "advanced_pointer",
        "advanced-titlebar-drag",
        "start_advanced_pointer_drag",
        "finish_advanced_pointer_drag",
    ] {
        assert!(
            !INPUT_BACKEND.contains(forbidden),
            "synthetic two-finger pointer drag remains: {forbidden}"
        );
    }
    assert!(WINDOW_BACKEND.contains("cursorAfterWindowMove"));
    assert!(WINDOW_BACKEND.contains("warp_pointer(point.x, point.y)"));
    assert!(WINDOW_BACKEND.contains("this._settings.moveCursor"));
    assert!(WINDOW_BACKEND.contains("this._settings.livePreview"));
    assert!(WINDOW_BACKEND.contains("this._settings.appSwitchOnHold"));
    assert!(WINDOW_BACKEND.contains("takeRebaselineRequest"));
    assert!(INPUT_BACKEND.contains("state.advanced.rebaseline_seed(direction)"));
}

#[test]
#[ignore = "requires a real, permission-configured touchpad and /dev/uinput"]
fn real_linux_input_service_starts_and_stops_without_injecting_motion() {
    use std::sync::{mpsc, Arc, RwLock};

    use three_finger_drag_core::{linux::InputService, logging::RingLogger, settings::AppSettings};

    // Gestures are disabled before startup. This opens a real evdev touchpad
    // and /dev/uinput, then immediately exercises the safe shutdown path
    // without ever requesting pointer movement or a button press.
    let settings = Arc::new(RwLock::new(AppSettings {
        three_finger_drag: false,
        ..AppSettings::default()
    }));
    let (events, _receiver) = mpsc::channel();
    let mut service = InputService::start(settings, RingLogger::default(), events)
        .expect("real Linux touchpad and /dev/uinput must be usable by this login user");
    service.stop();
}
