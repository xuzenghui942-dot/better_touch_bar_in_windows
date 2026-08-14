//! Linux/Wayland input backend.
//!
//! The basic three-finger drag backend observes evdev without grabbing it.  If
//! the user explicitly enables advanced two-finger window gestures, the input
//! service temporarily owns the physical touchpad and mirrors all non-owned
//! input through a cloned uinput touchpad.  That earlier arbitration point is
//! required on Wayland: a Shell extension cannot retract a gesture already
//! delivered to a focused client.

pub mod advanced_transport;
mod input_service;
mod mouse_output;
mod touchpad_proxy;

pub use input_service::{
    BackendEvent, ContactPreview, DeviceDescriptor, InputService, TouchpadRuntimeStatus,
};

pub fn stable_device_id(value: &str) -> String {
    if value.is_empty() {
        "default".to_owned()
    } else {
        format!("{:x}", md5::compute(value.as_bytes()))
    }
}
