//! Small Win32 application catalog used by the Apps compatibility page.
//! It intentionally returns process names (the persisted contract) rather than
//! HWNDs, so the settings UI never stores unstable window handles.

use std::{
    ffi::OsStr,
    fs,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
};

use serde::Serialize;
use three_finger_drag_core::advanced_gestures::config::app_compatibility::normalize_process_name;
use windows::{
    core::{Interface, PCWSTR},
    Win32::{
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER,
            COINIT_APARTMENTTHREADED, STGM_READ,
        },
        UI::Shell::{IShellLinkW, ShellLink},
    },
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HWND, LPARAM},
    System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    },
    UI::WindowsAndMessaging::{
        EnumWindows, GetWindowLongPtrW, GetWindowTextLengthW, GetWindowTextW,
        GetWindowThreadProcessId, IsWindowVisible, GWL_STYLE, WS_CAPTION,
    },
};

#[derive(Debug, Clone, Serialize)]
pub struct RunningApp {
    pub process_name: String,
    pub title: String,
}

pub fn list_running_apps() -> Vec<RunningApp> {
    let mut apps: Vec<RunningApp> = Vec::new();
    unsafe {
        EnumWindows(
            Some(enum_window),
            &mut apps as *mut Vec<RunningApp> as LPARAM,
        );
    }
    apps.sort_by(|left, right| {
        left.process_name
            .cmp(&right.process_name)
            .then_with(|| left.title.cmp(&right.title))
    });
    apps.dedup_by(|left, right| left.process_name.eq_ignore_ascii_case(&right.process_name));
    apps
}

/// Swoosh's Apps page builds its Installed list from both Start Menu trees and
/// resolves each `.lnk` through `IShellLinkW`; keeping that same source avoids
/// inventing process names from shortcut display names.
pub fn list_installed_apps() -> Vec<RunningApp> {
    let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
    let mut shortcuts = Vec::new();
    for root in start_menu_roots() {
        collect_shortcuts(&root, &mut shortcuts);
    }
    let mut apps = shortcuts
        .into_iter()
        .filter_map(|shortcut| {
            let target = resolve_shortcut(&shortcut)?;
            if !target.is_file()
                || !target
                    .extension()
                    .and_then(OsStr::to_str)
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
            {
                return None;
            }
            let process_name = normalize_process_name(&target.to_string_lossy());
            if process_name.is_empty() {
                return None;
            }
            let title = shortcut
                .file_stem()
                .and_then(OsStr::to_str)
                .unwrap_or(&process_name)
                .to_owned();
            Some(RunningApp {
                process_name,
                title,
            })
        })
        .collect::<Vec<_>>();
    if initialized {
        unsafe { CoUninitialize() };
    }
    apps.sort_by(|left, right| {
        left.title
            .to_ascii_lowercase()
            .cmp(&right.title.to_ascii_lowercase())
            .then_with(|| left.process_name.cmp(&right.process_name))
    });
    apps.dedup_by(|left, right| {
        left.process_name.eq_ignore_ascii_case(&right.process_name)
            && left.title.eq_ignore_ascii_case(&right.title)
    });
    apps
}

fn start_menu_roots() -> Vec<PathBuf> {
    let mut roots = Vec::with_capacity(2);
    if let Some(program_data) = std::env::var_os("ProgramData") {
        roots.push(PathBuf::from(program_data).join("Microsoft/Windows/Start Menu/Programs"));
    }
    if let Some(app_data) = std::env::var_os("APPDATA") {
        roots.push(PathBuf::from(app_data).join("Microsoft/Windows/Start Menu/Programs"));
    }
    roots
}

fn collect_shortcuts(root: &Path, output: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_shortcuts(&path, output);
        } else if path
            .extension()
            .and_then(OsStr::to_str)
            .is_some_and(|extension| extension.eq_ignore_ascii_case("lnk"))
        {
            output.push(path);
        }
    }
}

fn resolve_shortcut(shortcut: &Path) -> Option<PathBuf> {
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let persist: IPersistFile = link.cast().ok()?;
        let wide = shortcut
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        persist.Load(PCWSTR(wide.as_ptr()), STGM_READ).ok()?;
        let mut target = [0_u16; 1024];
        link.GetPath(&mut target, std::ptr::null_mut(), 0).ok()?;
        let length = target
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(target.len());
        (length > 0).then(|| PathBuf::from(String::from_utf16_lossy(&target[..length])))
    }
}

unsafe extern "system" fn enum_window(hwnd: HWND, lparam: LPARAM) -> i32 {
    if hwnd.is_null() || unsafe { IsWindowVisible(hwnd) } == 0 {
        return 1;
    }
    if (unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32 & WS_CAPTION) == 0 {
        return 1;
    }
    let length = unsafe { GetWindowTextLengthW(hwnd) };
    if length <= 0 {
        return 1;
    }
    let mut title = vec![0_u16; length as usize + 1];
    let copied = unsafe { GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32) };
    if copied <= 0 {
        return 1;
    }
    let process_name = process_name_for(hwnd);
    if process_name.is_empty() {
        return 1;
    }
    let apps = unsafe { &mut *(lparam as *mut Vec<RunningApp>) };
    apps.push(RunningApp {
        process_name,
        title: String::from_utf16_lossy(&title[..copied as usize]),
    });
    1
}

fn process_name_for(hwnd: HWND) -> String {
    let mut pid = 0_u32;
    if unsafe { GetWindowThreadProcessId(hwnd, &mut pid) } == 0 || pid == 0 {
        return String::new();
    }
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return String::new();
    }
    let mut buffer = [0_u16; 1024];
    let mut length = buffer.len() as u32;
    let ok = unsafe {
        QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, buffer.as_mut_ptr(), &mut length)
    } != 0;
    unsafe { CloseHandle(handle) };
    if !ok {
        return String::new();
    }
    normalize_process_name(&String::from_utf16_lossy(&buffer[..length as usize]))
}
