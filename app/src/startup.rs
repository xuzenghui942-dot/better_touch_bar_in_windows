use std::{
    ffi::OsStr,
    io,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use three_finger_drag_core::settings::{AppSettings, PendingStartupAction};
use windows_sys::Win32::UI::{
    Shell::{IsUserAnAdmin, ShellExecuteW},
    WindowsAndMessaging::SW_SHOWNORMAL,
};
use winreg::{
    enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE},
    RegKey,
};

pub const ELEVATED_TASK_NAME: &str = r"\ThreeFingerDragRust\Run on Startup";
pub const UNELEVATED_RUN_VALUE: &str = "ThreeFingerDragRust";
pub const ADVANCED_ELEVATED_TASK_NAME: &str = r"\ThreeFingerDragRust\Advanced on Startup";
pub const ADVANCED_RUN_VALUE: &str = "ThreeFingerDragRustAdvanced";

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn startup_command(executable: &Path) -> String {
    format!("\"{}\" --autostart", executable.display())
}

fn advanced_startup_command(executable: &Path) -> String {
    // The same executable owns both modules; AppState turns the persisted
    // launchAtLogin flag back on before the raw-input worker starts.
    startup_command(executable)
}

pub fn is_administrator() -> bool {
    unsafe { IsUserAnAdmin() != 0 }
}

pub fn current_executable() -> io::Result<PathBuf> {
    std::env::current_exe()
}

pub fn request_elevated_restart() -> io::Result<()> {
    let executable = current_executable()?;
    let operation = wide("runas");
    let executable = wide(&executable.to_string_lossy());
    let parameters = wide("--elevated-restart");
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            executable.as_ptr(),
            parameters.as_ptr(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;
    if result > 32 {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "管理员启动请求失败或已取消（代码 {result}）。"
        )))
    }
}

pub fn enable_unelevated(executable: &Path) -> io::Result<()> {
    let current_user = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = current_user.create_subkey(RUN_KEY)?;
    key.set_value(UNELEVATED_RUN_VALUE, &startup_command(executable))
}

pub fn disable_unelevated() -> io::Result<()> {
    let current_user = RegKey::predef(HKEY_CURRENT_USER);
    match current_user.open_subkey_with_flags(RUN_KEY, KEY_WRITE) {
        Ok(key) => match key.delete_value(UNELEVATED_RUN_VALUE) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub fn enable_advanced_startup(executable: &Path, elevated: bool) -> io::Result<()> {
    if elevated && is_administrator() {
        disable_advanced_unelevated()?;
        let command = advanced_startup_command(executable);
        let output = run_schtasks([
            OsStr::new("/Create"),
            OsStr::new("/TN"),
            OsStr::new(ADVANCED_ELEVATED_TASK_NAME),
            OsStr::new("/TR"),
            OsStr::new(&command),
            OsStr::new("/SC"),
            OsStr::new("ONLOGON"),
            OsStr::new("/RL"),
            OsStr::new("HIGHEST"),
            OsStr::new("/F"),
        ])?;
        require_success(output, "创建进阶管理员开机启动任务")
    } else {
        let current_user = RegKey::predef(HKEY_CURRENT_USER);
        let (key, _) = current_user.create_subkey(RUN_KEY)?;
        key.set_value(ADVANCED_RUN_VALUE, &advanced_startup_command(executable))
    }
}

pub fn disable_advanced_startup() -> io::Result<()> {
    disable_advanced_unelevated()?;
    let output = run_schtasks([
        OsStr::new("/Delete"),
        OsStr::new("/TN"),
        OsStr::new(ADVANCED_ELEVATED_TASK_NAME),
        OsStr::new("/F"),
    ])?;
    if output.status.success() || !is_advanced_elevated_enabled() {
        Ok(())
    } else {
        require_success(output, "删除进阶管理员开机启动任务")
    }
}

fn disable_advanced_unelevated() -> io::Result<()> {
    let current_user = RegKey::predef(HKEY_CURRENT_USER);
    match current_user.open_subkey_with_flags(RUN_KEY, KEY_WRITE) {
        Ok(key) => match key.delete_value(ADVANCED_RUN_VALUE) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn is_advanced_elevated_enabled() -> bool {
    run_schtasks([
        OsStr::new("/Query"),
        OsStr::new("/TN"),
        OsStr::new(ADVANCED_ELEVATED_TASK_NAME),
    ])
    .map(|output| output.status.success())
    .unwrap_or(false)
}

pub fn refresh_advanced_startup(launch_at_login: bool, elevated: bool) -> io::Result<()> {
    let executable = current_executable()?;
    if launch_at_login {
        enable_advanced_startup(&executable, elevated)
    } else {
        disable_advanced_startup()
    }
}

pub fn is_unelevated_enabled() -> bool {
    let current_user = RegKey::predef(HKEY_CURRENT_USER);
    current_user
        .open_subkey_with_flags(RUN_KEY, KEY_READ)
        .and_then(|key| key.get_value::<String, _>(UNELEVATED_RUN_VALUE))
        .is_ok()
}

pub fn enable_elevated(executable: &Path) -> io::Result<()> {
    disable_unelevated()?;
    let task_command = startup_command(executable);
    let output = run_schtasks([
        OsStr::new("/Create"),
        OsStr::new("/TN"),
        OsStr::new(ELEVATED_TASK_NAME),
        OsStr::new("/TR"),
        OsStr::new(&task_command),
        OsStr::new("/SC"),
        OsStr::new("ONLOGON"),
        OsStr::new("/RL"),
        OsStr::new("HIGHEST"),
        OsStr::new("/F"),
    ])?;
    require_success(output, "创建管理员开机启动任务")
}

pub fn disable_elevated() -> io::Result<()> {
    let output = run_schtasks([
        OsStr::new("/Delete"),
        OsStr::new("/TN"),
        OsStr::new(ELEVATED_TASK_NAME),
        OsStr::new("/F"),
    ])?;
    if output.status.success() || !is_elevated_enabled() {
        Ok(())
    } else {
        require_success(output, "删除管理员开机启动任务")
    }
}

pub fn is_elevated_enabled() -> bool {
    run_schtasks([
        OsStr::new("/Query"),
        OsStr::new("/TN"),
        OsStr::new(ELEVATED_TASK_NAME),
    ])
    .map(|output| output.status.success())
    .unwrap_or(false)
}

pub fn is_enabled(run_elevated: bool) -> bool {
    if run_elevated {
        is_elevated_enabled()
    } else {
        is_unelevated_enabled()
    }
}

pub fn apply_pending_action(settings: &mut AppSettings) -> io::Result<()> {
    if !is_administrator() {
        return Ok(());
    }
    let executable = current_executable()?;
    match settings.pending_startup_action {
        PendingStartupAction::None => {}
        PendingStartupAction::EnableElevatedRunWithStartup => {
            enable_elevated(&executable)?;
            settings.run_elevated = true;
            settings.run_at_startup = true;
        }
        PendingStartupAction::DisableElevatedRunWithStartup => {
            disable_elevated()?;
            enable_unelevated(&executable)?;
            settings.run_elevated = false;
            settings.run_at_startup = true;
        }
        PendingStartupAction::EnableElevatedStartup => {
            enable_elevated(&executable)?;
            settings.run_at_startup = true;
        }
        PendingStartupAction::DisableElevatedStartup => {
            disable_elevated()?;
            settings.run_at_startup = false;
        }
    }
    settings.pending_startup_action = PendingStartupAction::None;
    Ok(())
}

pub fn refresh_current_startup(settings: &AppSettings) -> io::Result<()> {
    if !settings.run_at_startup {
        return Ok(());
    }
    let executable = current_executable()?;
    if settings.run_elevated && is_administrator() {
        enable_elevated(&executable)
    } else if !settings.run_elevated {
        if is_administrator() && is_elevated_enabled() {
            disable_elevated()?;
        }
        enable_unelevated(&executable)
    } else {
        Ok(())
    }
}

fn run_schtasks<I, S>(arguments: I) -> io::Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new("schtasks.exe")
        .creation_flags(CREATE_NO_WINDOW)
        .args(arguments)
        .output()
}

fn require_success(output: Output, operation: &str) -> io::Result<()> {
    if output.status.success() {
        return Ok(());
    }
    let details = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    Err(io::Error::other(if details.is_empty() {
        format!("{operation}失败。")
    } else {
        format!("{operation}失败：{details}")
    }))
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
