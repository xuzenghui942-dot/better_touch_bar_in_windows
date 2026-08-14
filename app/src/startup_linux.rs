//! User-level Linux login startup integration.

use std::{
    fs::{self, OpenOptions},
    io,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use three_finger_drag_core::settings::{AppSettings, PendingStartupAction};

const AUTOSTART_FILE: &str = "three-finger-drag-linux.desktop";

pub fn startup_command(executable: &Path) -> String {
    let escaped = executable
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('`', "\\`")
        .replace('$', "\\$")
        .replace('%', "%%");
    format!("\"{escaped}\" --autostart")
}

pub fn is_administrator() -> bool {
    // SAFETY: geteuid has no preconditions and does not dereference pointers.
    unsafe { libc::geteuid() == 0 }
}

pub fn current_executable() -> io::Result<PathBuf> {
    std::env::current_exe()
}

pub fn request_elevated_restart() -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Linux 版不以 root 身份运行；设备访问由最小 udev ACL 提供。",
    ))
}

pub fn enable_unelevated(executable: &Path) -> io::Result<()> {
    if !executable.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "登录启动程序路径必须是绝对路径。",
        ));
    }
    write_autostart_entry(&autostart_path()?, executable)
}

pub fn disable_unelevated() -> io::Result<()> {
    let path = autostart_path()?;
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub fn enable_advanced_startup(executable: &Path, elevated: bool) -> io::Result<()> {
    if elevated {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Linux 登录启动不支持 root 模式。",
        ));
    }
    enable_unelevated(executable)
}

pub fn disable_advanced_startup() -> io::Result<()> {
    disable_unelevated()
}

pub fn refresh_advanced_startup(launch_at_login: bool, elevated: bool) -> io::Result<()> {
    if launch_at_login {
        enable_advanced_startup(&current_executable()?, elevated)
    } else {
        disable_advanced_startup()
    }
}

pub fn is_unelevated_enabled() -> bool {
    autostart_path().is_ok_and(|path| path.is_file())
}

pub fn enable_elevated(_executable: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Linux 版不支持也不需要以 root 身份运行。",
    ))
}

pub fn disable_elevated() -> io::Result<()> {
    Ok(())
}

pub fn is_elevated_enabled() -> bool {
    false
}

pub fn is_enabled(_run_elevated: bool) -> bool {
    is_unelevated_enabled()
}

pub fn apply_pending_action(settings: &mut AppSettings) -> io::Result<()> {
    settings.pending_startup_action = PendingStartupAction::None;
    settings.run_elevated = false;
    Ok(())
}

pub fn refresh_current_startup(settings: &AppSettings) -> io::Result<()> {
    if settings.run_at_startup {
        enable_unelevated(&current_executable()?)
    } else {
        disable_unelevated()
    }
}

pub(crate) fn autostart_path() -> io::Result<PathBuf> {
    dirs::config_dir()
        .map(|directory| directory.join("autostart").join(AUTOSTART_FILE))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "无法确定配置目录。"))
}

fn write_autostart_entry(path: &Path, executable: &Path) -> io::Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "无效的自启动路径。"))?;
    fs::create_dir_all(directory)?;
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
    let temporary = directory.join(format!(
        ".{AUTOSTART_FILE}.{}.{}.tmp",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let contents = desktop_entry(executable);
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub fn desktop_entry(executable: &Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=Three Finger Drag Linux\nComment=Start the touchpad gesture service after GNOME login\nExec={}\nTerminal=false\nNoDisplay=true\nOnlyShowIn=GNOME;\nStartupNotify=false\nX-GNOME-Autostart-enabled=true\n",
        startup_command(executable)
    )
}
