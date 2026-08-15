use std::{path::Path, process::Command};

fn main() {
    let manifest = std::env::var_os("CARGO_MANIFEST_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let repository = manifest.parent().unwrap_or(Path::new("."));
    let revision = git_output(repository, &["rev-parse", "--short=12", "HEAD"])
        .filter(|value| value.chars().all(|character| character.is_ascii_hexdigit()))
        .unwrap_or_else(|| "unknown".into());
    let dirty =
        git_output(repository, &["status", "--porcelain"]).is_some_and(|value| !value.is_empty());
    let revision = if dirty {
        format!("{revision}+dirty")
    } else {
        revision
    };
    println!("cargo:rustc-env=THREE_FINGER_DRAG_BUILD_REVISION={revision}");
    let git_head = repository.join(".git/HEAD");
    println!("cargo:rerun-if-changed={}", git_head.display());
    if let Ok(head) = std::fs::read_to_string(&git_head) {
        if let Some(reference) = head.trim().strip_prefix("ref: ") {
            println!(
                "cargo:rerun-if-changed={}",
                repository.join(".git").join(reference).display()
            );
        }
    }
    tauri_build::build()
}

fn git_output(repository: &Path, arguments: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(repository)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
