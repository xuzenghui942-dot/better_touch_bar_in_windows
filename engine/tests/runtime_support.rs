use std::fs;

use three_finger_drag_core::{
    logging::RingLogger, mouse::PointerAccumulator, settings::AppSettings,
};

#[test]
fn pointer_accumulator_preserves_fractional_relative_motion() {
    let mut accumulator = PointerAccumulator::default();

    assert_eq!(accumulator.consume(0.4, -0.4), (0, 0));
    assert_eq!(accumulator.consume(0.7, -0.7), (1, -1));
    assert_eq!(accumulator.consume(0.9, -0.9), (1, -1));
}

#[test]
fn logger_records_only_when_enabled_and_keeps_the_newest_entries() {
    let logger = RingLogger::with_capacity(3);
    logger.record_at("ignored", "10:00:00.000");
    assert!(logger.snapshot().is_empty());

    logger.set_enabled(true);
    logger.record_at("one", "10:00:00.001");
    logger.record_at("two", "10:00:00.002");
    logger.record_at("three", "10:00:00.003");
    logger.record_at("four", "10:00:00.004");

    assert_eq!(
        logger.snapshot(),
        vec![
            "[10:00:00.002] two",
            "[10:00:00.003] three",
            "[10:00:00.004] four",
        ]
    );
}

#[test]
fn settings_use_fresh_defaults_for_missing_or_corrupt_files() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("preferences.json");

    let (missing, missing_was_fresh) = AppSettings::load_or_default(&path);
    assert!(missing_was_fresh);
    assert_eq!(missing, AppSettings::default());

    fs::write(&path, "{not-json").unwrap();
    let (corrupt, corrupt_was_fresh) = AppSettings::load_or_default(&path);
    assert!(corrupt_was_fresh);
    assert_eq!(corrupt, AppSettings::default());
}

#[test]
fn settings_save_and_load_from_the_new_projects_own_path() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary
        .path()
        .join("ThreeFingerDragRust/preferences.json");
    let expected = AppSettings {
        release_delay_ms: 321,
        ..AppSettings::default()
    };

    expected.save(&path).unwrap();
    let (actual, was_fresh) = AppSettings::load_or_default(&path);

    assert!(!was_fresh);
    assert_eq!(actual, expected);
    assert!(!path.with_extension("json.tmp").exists());
}
