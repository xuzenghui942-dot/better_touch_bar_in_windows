//! Cross-platform Rust core for the three-finger drag application.

/// The imported AdvancedGestures state machine is a Rust workspace dependency,
/// not a DLL loaded by the old C# application.
pub use better_touch_advanced_gestures as advanced_gestures;

pub mod advanced;
pub mod contacts;
pub mod gesture;
pub mod logging;
pub mod mouse;
pub mod settings;
pub mod speed;
pub mod swoosh_hud;

#[cfg(windows)]
pub mod win32;

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "linux")]
pub use linux as platform;
#[cfg(windows)]
pub use win32 as platform;
