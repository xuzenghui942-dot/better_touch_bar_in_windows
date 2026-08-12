mod advanced_actions;
mod contact_report;
mod cursor_hud;
mod device_catalog;
mod input_service;
mod mouse_output;
mod phantom_filter;
mod virtual_desktop;

pub use device_catalog::DeviceDescriptor;
pub use input_service::{BackendEvent, ContactPreview, InputService, TouchpadRuntimeStatus};

pub fn stable_device_id(device_name: &str) -> String {
    if device_name.is_empty() {
        return "default".to_owned();
    }
    format!("{:x}", md5::compute(device_name.as_bytes()))
}
