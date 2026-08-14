use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct RunningApp {
    pub process_name: String,
    pub title: String,
}

pub fn list_running_apps() -> Vec<RunningApp> {
    let mut names = BTreeSet::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .bytes()
            .all(|byte| byte.is_ascii_digit())
        {
            continue;
        }
        if let Ok(name) = fs::read_to_string(entry.path().join("comm")) {
            let name = normalize_linux_app_identifier(name.trim());
            if !name.is_empty() {
                names.insert(name);
            }
        }
    }
    names
        .into_iter()
        .map(|process_name| RunningApp {
            title: process_name.clone(),
            process_name,
        })
        .collect()
}

pub fn list_installed_apps() -> Vec<RunningApp> {
    let mut files = Vec::new();
    for root in application_roots() {
        collect_desktop_files(&root, &mut files);
    }
    let mut apps = files
        .into_iter()
        .filter_map(|path| parse_desktop_file(&path))
        .collect::<Vec<_>>();
    apps.sort_by_key(|app| app.title.to_lowercase());
    apps.dedup_by(|left, right| left.process_name == right.process_name);
    apps
}

fn application_roots() -> Vec<PathBuf> {
    let mut roots = vec![PathBuf::from("/usr/share/applications")];
    if let Some(data) = dirs::data_local_dir() {
        roots.push(data.join("applications"));
    }
    roots
}

fn collect_desktop_files(root: &Path, output: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_desktop_files(&path, output);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "desktop")
        {
            output.push(path);
        }
    }
}

fn parse_desktop_file(path: &Path) -> Option<RunningApp> {
    let content = fs::read_to_string(path).ok()?;
    if content.lines().any(|line| line.trim() == "NoDisplay=true") {
        return None;
    }
    let title = value(&content, "Name")?;
    // GNOME/Wayland brokers commonly identify a window by app ID or
    // StartupWMClass, not by a Windows-style executable name.  Prefer the
    // desktop entry's explicit identifiers, then its stable desktop-file ID.
    let candidate = value(&content, "StartupWMClass")
        .or_else(|| value(&content, "X-GNOME-WMClass"))
        .or_else(|| value(&content, "X-Flatpak"))
        .or_else(|| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .map(str::to_owned)
        })?;
    let process_name = normalize_linux_app_identifier(&candidate);
    (!process_name.is_empty()).then_some(RunningApp {
        process_name,
        title,
    })
}

fn value(content: &str, key: &str) -> Option<String> {
    content.lines().find_map(|line| {
        let (candidate, value) = line.split_once('=')?;
        (candidate == key).then(|| value.trim().to_owned())
    })
}

fn normalize_linux_app_identifier(value: &str) -> String {
    let trimmed = value.trim().trim_matches(['\'', '"']);
    let basename = trimmed
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(trimmed)
        .trim();
    let without_desktop = basename.strip_suffix(".desktop").unwrap_or(basename);
    without_desktop.to_owned()
}

#[cfg(test)]
mod tests {
    use super::normalize_linux_app_identifier;

    #[test]
    fn linux_identifiers_are_native_and_never_invent_or_remove_an_exe_suffix() {
        assert_eq!(
            normalize_linux_app_identifier("/usr/bin/nautilus"),
            "nautilus"
        );
        assert_eq!(
            normalize_linux_app_identifier("org.gnome.Nautilus.desktop"),
            "org.gnome.Nautilus"
        );
        assert_eq!(normalize_linux_app_identifier("legacy.EXE"), "legacy.EXE");
    }
}
