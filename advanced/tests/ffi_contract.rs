use std::ffi::{CString, c_void};

use better_touch_advanced_gestures::ffi::{
    AdvancedContact, AdvancedEvent, advanced_create, advanced_destroy, advanced_process_frame,
};

unsafe extern "C" fn collect(context: *mut c_void, event: AdvancedEvent) {
    // SAFETY: the test keeps this Vec alive until advanced_destroy returns.
    unsafe { &mut *context.cast::<Vec<AdvancedEvent>>() }.push(event);
}

#[test]
fn exported_ffi_emits_a_completed_swipe_through_the_callback() {
    let config = CString::new(r#"{"enabled":true}"#).unwrap();
    let mut events = Vec::<AdvancedEvent>::new();
    let handle = advanced_create(
        config.as_ptr(),
        Some(collect),
        (&mut events as *mut Vec<AdvancedEvent>).cast(),
    );
    assert!(!handle.is_null());

    let start = [
        AdvancedContact {
            id: 1,
            x: 0.40,
            y: 0.50,
        },
        AdvancedContact {
            id: 2,
            x: 0.50,
            y: 0.50,
        },
    ];
    let moved = [
        AdvancedContact {
            id: 1,
            x: 0.58,
            y: 0.50,
        },
        AdvancedContact {
            id: 2,
            x: 0.68,
            y: 0.50,
        },
    ];
    // SAFETY: each pointer is valid for the supplied element count and the
    // engine handle remains owned by this test until advanced_destroy.
    unsafe {
        assert!(advanced_process_frame(
            handle,
            start.as_ptr(),
            start.len(),
            0
        ));
        assert!(advanced_process_frame(
            handle,
            moved.as_ptr(),
            moved.len(),
            80
        ));
        assert!(advanced_process_frame(handle, std::ptr::null(), 0, 100));
    }

    assert!(
        events
            .iter()
            .any(|event| event.kind == 4 && event.direction == 2)
    );
    advanced_destroy(handle);
}
