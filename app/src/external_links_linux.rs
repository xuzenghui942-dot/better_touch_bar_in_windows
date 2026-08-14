use std::{io, process::Command};

const PROJECT_URL: &str = "https://github.com/xuzenghui942-dot/better_touch_bar_in_windows";
const ISSUE_URL: &str =
    "https://github.com/xuzenghui942-dot/better_touch_bar_in_windows/issues/new";

pub fn is_allowed_external_link(value: &str) -> bool {
    matches!(value, PROJECT_URL | ISSUE_URL)
}

pub fn open_allowed_external_link(value: &str) -> io::Result<()> {
    if !is_allowed_external_link(value) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "不允许打开此链接。",
        ));
    }
    spawn("xdg-open", &[value])
}

pub fn open_touchpad_settings() -> io::Result<()> {
    spawn("gnome-control-center", &["mouse"])
}

fn spawn(program: &str, arguments: &[&str]) -> io::Result<()> {
    Command::new(program).args(arguments).spawn().map(|_| ())
}
