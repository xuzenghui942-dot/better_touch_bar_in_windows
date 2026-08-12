use serde::{Deserialize, Serialize};

/// How Swoosh treats the process names listed in the compatibility page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AppCompatibilityMode {
    Exclude,
    RequireModifier,
}

/// Backdrop theme used by the HUD/preview overlays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HudTheme {
    Dark,
    Light,
    System,
}

/// HUD scale used by Swoosh's snap chip, desktop strip and monitor map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HudSize {
    Normal,
    Large,
}

/// Rust equivalent of Swoosh.Settings.AppCompatibility.
pub mod app_compatibility {
    use std::path::Path;

    /// Normalize a process/path value exactly as the reference application does.
    pub fn normalize_process_name(value: &str) -> String {
        let trimmed = value.trim().trim_matches('"').trim();
        let name = Path::new(trimmed)
            .file_name()
            .and_then(|part| part.to_str())
            .unwrap_or(trimmed)
            .trim();
        if name.is_empty() {
            return String::new();
        }
        let with_extension = if name.contains('.') {
            name.to_owned()
        } else {
            format!("{name}.exe")
        };
        with_extension.to_ascii_lowercase()
    }

    /// Parse the separators accepted by Swoosh's additional-apps editor.
    pub fn parse_process_list(value: &str) -> Vec<String> {
        let values: Vec<String> = value
            .split(['\r', '\n', ',', ';'])
            .map(normalize_process_name)
            .filter(|name| !name.is_empty())
            .collect();
        let mut unique = Vec::with_capacity(values.len());
        for value in values {
            if !unique
                .iter()
                .any(|existing: &String| existing.eq_ignore_ascii_case(&value))
            {
                unique.push(value);
            }
        }
        unique
    }

    pub fn format_process_list<I, S>(process_names: I) -> String
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut values: Vec<String> = process_names
            .into_iter()
            .map(|name| normalize_process_name(name.as_ref()))
            .filter(|name| !name.is_empty())
            .collect();
        values.sort_by_key(|name| name.to_ascii_lowercase());
        values.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
        values.join("\r\n")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GridModifier {
    Shift,
    Ctrl,
    Alt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SwipeDownMode {
    Minimize,
    Close,
    Choose,
}

/// Persisted settings owned exclusively by the opt-in advanced gesture module.
///
/// This deliberately does not contain any three-finger-drag setting. Keeping the
/// serialized model separate makes reset, migration and native failures unable to
/// alter the basic gesture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AdvancedConfig {
    /// Extra Swoosh implementation switch. It is intentionally false by default
    /// so the basic three-finger drag path remains the only active gesture path.
    pub enabled: bool,
    /// Swoosh's own master gesture switch. Kept separate from `enabled` so a
    /// user can disable all advanced gestures without uninstalling the module.
    pub gestures_enabled: bool,
    pub animate_snaps: bool,
    pub snap_animation_seconds: f64,
    pub maximize_enabled: bool,
    pub halves_enabled: bool,
    pub quarters_enabled: bool,
    pub minimize_enabled: bool,
    pub swipe_down_action: SwipeDownMode,
    pub swipe_down_threshold: f64,
    pub grid_modifier_enabled: bool,
    pub grid_modifier: GridModifier,
    pub sensitivity: f64,
    pub grid_spacing: i32,
    pub cancel_timeout_seconds: f64,
    pub live_preview: bool,
    pub move_cursor: bool,
    pub mouse_middle_button_hud_enabled: bool,
    pub resize_horizontal_enabled: bool,
    pub resize_vertical_enabled: bool,
    pub five_finger_enabled: bool,
    pub center_enabled: bool,
    pub app_switch_on_hold: bool,
    pub monitor_move_enabled: bool,
    pub monitor_move_modifier: GridModifier,
    pub preview_desktop_destination: bool,
    pub create_desktop_on_overflow: bool,
    pub desktop_hold_delay_seconds: f64,
    pub phantom_rejection: bool,
    pub onboarding_completed: bool,
    /// Independent opt-in applied when this app starts; it never changes app startup.
    pub enable_on_app_start: bool,
    pub taskbar_icon_gestures_enabled: bool,
    pub app_compatibility_process_names: Vec<String>,
    pub app_compatibility_mode: AppCompatibilityMode,
    pub app_compatibility_modifier: GridModifier,
    pub overlay_use_accent: bool,
    pub hud_background: HudTheme,
    pub hud_size: HudSize,
    pub launch_at_login: bool,
    pub overlay_color: String,
    pub hud_fade_out_seconds: f64,
}

impl Default for AdvancedConfig {
    fn default() -> Self {
        Self {
            // Product requirement: the imported, larger capability is opt-in.
            enabled: false,
            gestures_enabled: true,
            animate_snaps: true,
            snap_animation_seconds: 0.22,
            maximize_enabled: true,
            halves_enabled: true,
            quarters_enabled: true,
            minimize_enabled: true,
            swipe_down_action: SwipeDownMode::Minimize,
            swipe_down_threshold: 0.15,
            grid_modifier_enabled: true,
            grid_modifier: GridModifier::Shift,
            sensitivity: 0.10,
            grid_spacing: 0,
            cancel_timeout_seconds: 0.9,
            live_preview: false,
            move_cursor: false,
            mouse_middle_button_hud_enabled: false,
            resize_horizontal_enabled: false,
            resize_vertical_enabled: false,
            five_finger_enabled: true,
            center_enabled: true,
            app_switch_on_hold: false,
            monitor_move_enabled: true,
            monitor_move_modifier: GridModifier::Alt,
            preview_desktop_destination: true,
            create_desktop_on_overflow: false,
            desktop_hold_delay_seconds: 0.3,
            phantom_rejection: true,
            onboarding_completed: false,
            enable_on_app_start: false,
            taskbar_icon_gestures_enabled: true,
            app_compatibility_process_names: Vec::new(),
            app_compatibility_mode: AppCompatibilityMode::Exclude,
            app_compatibility_modifier: GridModifier::Ctrl,
            overlay_use_accent: true,
            hud_background: HudTheme::Dark,
            hud_size: HudSize::Normal,
            launch_at_login: false,
            overlay_color: "#0A84FF".to_owned(),
            hud_fade_out_seconds: 0.36,
        }
    }
}

impl AdvancedConfig {
    /// Constrains user-edited or older JSON values to the ranges exposed in UI.
    pub fn normalized(mut self) -> Self {
        if !self.snap_animation_seconds.is_finite() {
            self.snap_animation_seconds = 0.22;
        }
        if !self.swipe_down_threshold.is_finite() {
            self.swipe_down_threshold = 0.15;
        }
        if !self.sensitivity.is_finite() {
            self.sensitivity = 0.10;
        }
        if !self.cancel_timeout_seconds.is_finite() {
            self.cancel_timeout_seconds = 0.9;
        }
        if !self.desktop_hold_delay_seconds.is_finite() {
            self.desktop_hold_delay_seconds = 0.3;
        }
        if !self.hud_fade_out_seconds.is_finite() {
            self.hud_fade_out_seconds = 0.36;
        }
        self.snap_animation_seconds = self.snap_animation_seconds.clamp(0.05, 0.4);
        self.swipe_down_threshold = self.swipe_down_threshold.clamp(0.02, 0.30);
        self.sensitivity = self.sensitivity.clamp(0.0, 1.0);
        self.grid_spacing = self.grid_spacing.clamp(0, 10);
        self.cancel_timeout_seconds = self.cancel_timeout_seconds.clamp(0.0, 3.0);
        self.desktop_hold_delay_seconds = self.desktop_hold_delay_seconds.clamp(0.1, 1.0);
        self.hud_fade_out_seconds = self.hud_fade_out_seconds.clamp(0.1, 1.5);
        if !is_hex_color(&self.overlay_color) {
            self.overlay_color = "#0A84FF".to_owned();
        } else {
            self.overlay_color = self.overlay_color.to_ascii_uppercase();
        }
        self.app_compatibility_process_names =
            app_compatibility::parse_process_list(&self.app_compatibility_process_names.join("\n"));
        self
    }
}

fn is_hex_color(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 7 && bytes[0] == b'#' && bytes[1..].iter().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::{AdvancedConfig, AppCompatibilityMode, HudSize, HudTheme, app_compatibility};

    #[test]
    fn app_compatibility_matches_swoosh_normalization() {
        assert_eq!(
            app_compatibility::parse_process_list(
                " Firefox ; brave.exe\r\n\"C:\\Apps\\Vivaldi.exe\"\nfirefox.exe "
            ),
            ["firefox.exe", "brave.exe", "vivaldi.exe"]
        );
    }

    #[test]
    fn appearance_and_apps_are_persisted_with_source_defaults() {
        let settings = AdvancedConfig::default();
        assert_eq!(
            settings.app_compatibility_mode,
            AppCompatibilityMode::Exclude
        );
        assert!(settings.app_compatibility_process_names.is_empty());
        assert!(settings.overlay_use_accent);
        assert_eq!(settings.hud_background, HudTheme::Dark);
        assert_eq!(settings.hud_size, HudSize::Normal);
        assert_eq!(settings.overlay_color, "#0A84FF");
        assert_eq!(settings.hud_fade_out_seconds, 0.36);
        assert!(!settings.enabled);
        assert!(settings.gestures_enabled);
    }

    #[test]
    fn invalid_appearance_values_are_repaired() {
        let settings = AdvancedConfig {
            overlay_color: "not-a-color".into(),
            hud_fade_out_seconds: f64::NAN,
            app_compatibility_process_names: vec!["Chrome".into(), "chrome.exe".into()],
            ..AdvancedConfig::default()
        }
        .normalized();
        assert_eq!(settings.overlay_color, "#0A84FF");
        assert_eq!(settings.hud_fade_out_seconds, 0.36);
        assert_eq!(settings.app_compatibility_process_names, ["chrome.exe"]);
    }

    #[test]
    fn source_enum_names_are_lower_camel_case() {
        let value = serde_json::to_value(AdvancedConfig {
            app_compatibility_mode: AppCompatibilityMode::RequireModifier,
            hud_background: HudTheme::System,
            hud_size: HudSize::Large,
            ..AdvancedConfig::default()
        })
        .unwrap();
        assert_eq!(value["appCompatibilityMode"], "requireModifier");
        assert_eq!(value["hudBackground"], "system");
        assert_eq!(value["hudSize"], "large");
    }
}
