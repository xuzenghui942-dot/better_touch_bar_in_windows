#![cfg(windows)]

use three_finger_drag_core::win32::stable_device_id;

#[test]
fn device_identifier_matches_the_original_utf8_md5_format() {
    assert_eq!(stable_device_id("abc"), "900150983cd24fb0d6963f7d28e17f72");
    assert_eq!(stable_device_id(""), "default");
}
