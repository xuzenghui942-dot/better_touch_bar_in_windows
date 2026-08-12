#![cfg(target_os = "windows")]

use std::{
    sync::{mpsc, Arc, RwLock},
    time::Duration,
};

use three_finger_drag_core::{
    logging::RingLogger,
    settings::AppSettings,
    win32::{BackendEvent, InputService},
};

#[test]
fn raw_input_service_initializes_and_stops_with_gestures_disabled() {
    let mut settings = AppSettings {
        three_finger_drag: false,
        ..AppSettings::default()
    };
    settings.normalize();
    let settings = Arc::new(RwLock::new(settings));
    let (events, receiver) = mpsc::channel();

    let mut service = InputService::start(settings, RingLogger::default(), events)
        .expect("Raw Input service should start");

    let status = receiver
        .recv_timeout(Duration::from_secs(3))
        .expect("service should publish its initialized status");
    match status {
        BackendEvent::Status(status) => {
            assert!(status.initialized);
            assert!(status.receiver_installed);
        }
        other => panic!("unexpected first backend event: {other:?}"),
    }

    service.stop();
}
