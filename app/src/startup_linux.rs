//! User-level Linux login startup integration.

use std::{
    fs::{self, OpenOptions},
    io,
    io::{BufRead, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
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
    if is_ephemeral_build_path(executable) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "拒绝让登录启动项直接指向 target 构建目录；请先安装版本化用户运行时。",
        ));
    }
    write_autostart_entry(&autostart_path()?, executable)
}

pub fn prepare_login_executable(executable: &Path, revision: &str) -> io::Result<PathBuf> {
    let home = dirs::home_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "无法确定用户主目录。"))?;
    prepare_login_executable_in(executable, revision, &home.join(".local/libexec"))
}

fn prepare_login_executable_in(
    executable: &Path,
    revision: &str,
    libexec_root: &Path,
) -> io::Result<PathBuf> {
    if !executable.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "登录启动程序路径必须是绝对路径。",
        ));
    }
    if !is_ephemeral_build_path(executable) {
        return Ok(executable.to_path_buf());
    }
    if revision.is_empty()
        || revision.len() > 64
        || !revision.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '+' | '-')
        })
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "构建修订标识不适合版本化运行目录。",
        ));
    }
    let source = fs::symlink_metadata(executable)?;
    if !source.file_type().is_file() || source.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "候选运行程序必须是普通文件。",
        ));
    }

    let directory = libexec_root.join("three-finger-drag-linux").join(revision);
    fs::create_dir_all(&directory)?;
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    let destination = directory.join("three-finger-drag-linux");
    if destination.exists() {
        if files_equal(executable, &destination)? {
            return Ok(destination);
        }
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "该修订的用户运行时已存在但内容不同，拒绝覆盖。",
        ));
    }

    let result: io::Result<()> = (|| {
        let mut source = fs::File::open(executable)?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(&destination)?;
        io::copy(&mut source, &mut output)?;
        output.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&destination);
    }
    result?;
    Ok(destination)
}

fn is_ephemeral_build_path(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component, Component::Normal(value) if value == "target"))
}

fn files_equal(left: &Path, right: &Path) -> io::Result<bool> {
    if fs::metadata(left)?.len() != fs::metadata(right)?.len() {
        return Ok(false);
    }
    let mut left = io::BufReader::new(fs::File::open(left)?);
    let mut right = io::BufReader::new(fs::File::open(right)?);
    loop {
        let left_buffer = left.fill_buf()?;
        let right_buffer = right.fill_buf()?;
        if left_buffer != right_buffer {
            return Ok(false);
        }
        if left_buffer.is_empty() {
            return Ok(true);
        }
        let consumed = left_buffer.len();
        left.consume(consumed);
        right.consume(consumed);
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_build_is_staged_in_a_versioned_user_libexec_directory() {
        let temporary = tempfile::tempdir().unwrap();
        let candidate = temporary
            .path()
            .join("workspace/target/release/three-finger-drag-linux");
        fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        fs::write(&candidate, b"candidate-binary").unwrap();
        let libexec = temporary.path().join("libexec");

        let staged = prepare_login_executable_in(&candidate, "abc123+dirty", &libexec).unwrap();

        assert_eq!(
            staged,
            libexec.join("three-finger-drag-linux/abc123+dirty/three-finger-drag-linux")
        );
        assert_eq!(fs::read(staged).unwrap(), b"candidate-binary");
    }

    #[test]
    fn installed_runtime_path_is_used_without_copying() {
        let temporary = tempfile::tempdir().unwrap();
        let installed = temporary.path().join("usr/bin/three-finger-drag-linux");
        fs::create_dir_all(installed.parent().unwrap()).unwrap();
        fs::write(&installed, b"installed-binary").unwrap();

        let selected =
            prepare_login_executable_in(&installed, "abc123", &temporary.path().join("libexec"))
                .unwrap();

        assert_eq!(selected, installed);
        assert!(!temporary.path().join("libexec").exists());
    }
}
