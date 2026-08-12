use std::io;

use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};

const PROJECT_URL: &str = "https://github.com/ClementGre/ThreeFingerDragOnWindows";
const ISSUE_URL: &str = "https://github.com/ClementGre/ThreeFingerDragOnWindows/issues/new";
const DONATION_URL: &str = "https://www.paypal.com/paypalme/themsou";
const SWOOSH_URL: &str = "https://github.com/bwya77/swoosh";
const SWOOSH_ISSUE_URL: &str = "https://github.com/bwya77/swoosh/issues/new";
const SWOOSH_GESTURE_URL: &str = "https://github.com/bwya77/swoosh#gestures";

pub fn is_allowed_external_link(value: &str) -> bool {
    matches!(
        value,
        PROJECT_URL | ISSUE_URL | DONATION_URL | SWOOSH_URL | SWOOSH_ISSUE_URL | SWOOSH_GESTURE_URL
    )
}

pub fn open_allowed_external_link(value: &str) -> io::Result<()> {
    if !is_allowed_external_link(value) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "不允许打开此链接。",
        ));
    }
    shell_open(value)
}

pub fn open_touchpad_settings() -> io::Result<()> {
    shell_open("ms-settings:devices-touchpad")
}

fn shell_open(target: &str) -> io::Result<()> {
    let operation = wide("open");
    let target = wide(target);
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;
    if result > 32 {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "ShellExecuteW failed with code {result}."
        )))
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
