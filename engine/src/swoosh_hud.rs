//! Source-derived, platform-neutral geometry and visibility rules for Swoosh's
//! cursor-following HUD. The Win32 renderer consumes this module so its trigger
//! contract can be tested without creating native windows.

use better_touch_advanced_gestures::{
    config::HudSize, geometry::SnapZone, gesture::SwipeDirection,
};

/// Swoosh waits briefly before painting the neutral chip so a five-finger
/// landing does not flash a two-finger HUD first. Directed swipes bypass it.
pub const HUD_ARM_GUARD_MS: u64 = 110;
pub const PRIOR_DIRECTION_DWELL_MS: u64 = 150;
pub const DOWN_REVERSE_BAND: f64 = 0.05;

pub struct HudGeometry;

impl HudGeometry {
    pub const MARGIN: f64 = 3.0;
    pub const STROKE: f64 = 2.5;
    pub const CORNER: f64 = 9.0;
    pub const CHIP_HEIGHT: f64 = 60.0;
    pub const SINGLE_CHIP_WIDTH: f64 = 94.0;
    pub const BASE_HEIGHT_PX: f64 = 46.0;
    pub const DESKTOP_WIDTH: f64 = 84.0;
    pub const DESKTOP_GAP: f64 = 11.0;
    pub const MAP_CELL_WIDTH: f64 = 80.0;
    pub const MAP_CELL_HEIGHT: f64 = 54.0;
    pub const MAP_GAP: f64 = 9.0;
    pub const MAP_BASE_HEIGHT_PX: f64 = 122.0;
    pub const APP_TILE_WIDTH: f64 = 66.0;
    pub const APP_TILE_HEIGHT: f64 = 60.0;
    pub const APP_TILE_GAP: f64 = 10.0;
    pub const APP_BASE_HEIGHT_PX: f64 = 96.0;

    pub const fn scale(size: HudSize) -> f64 {
        match size {
            HudSize::Normal => 0.65,
            HudSize::Large => 1.0,
        }
    }
}

pub const fn should_show_snap_hud(armed: bool, elapsed_ms: u64, zone: SnapZone) -> bool {
    armed && (elapsed_ms >= HUD_ARM_GUARD_MS || !matches!(zone, SnapZone::None))
}

pub const fn restore_direction_after_dwell(
    direction: SwipeDirection,
    elapsed_ms: u64,
) -> SwipeDirection {
    if !matches!(direction, SwipeDirection::None) && elapsed_ms >= PRIOR_DIRECTION_DWELL_MS {
        direction
    } else {
        SwipeDirection::None
    }
}

pub fn chooser_was_reversed(current_y: f64, peak_y: f64) -> bool {
    current_y < peak_y - DOWN_REVERSE_BAND
}

/// Fractional fill inside the rounded display chip, copied from Swoosh's
/// `CursorChipOverlay.ZoneFraction` contract.
pub const fn snap_fraction(zone: SnapZone) -> Option<(f64, f64, f64, f64)> {
    match zone {
        SnapZone::LeftHalf => Some((0.0, 0.0, 0.5, 1.0)),
        SnapZone::RightHalf => Some((0.5, 0.0, 1.0, 1.0)),
        SnapZone::TopHalf => Some((0.0, 0.0, 1.0, 0.5)),
        SnapZone::BottomHalf => Some((0.0, 0.5, 1.0, 1.0)),
        SnapZone::TopLeft => Some((0.0, 0.0, 0.5, 0.5)),
        SnapZone::TopRight => Some((0.5, 0.0, 1.0, 0.5)),
        SnapZone::BottomLeft => Some((0.0, 0.5, 0.5, 1.0)),
        SnapZone::BottomRight => Some((0.5, 0.5, 1.0, 1.0)),
        SnapZone::Maximize => Some((0.0, 0.0, 1.0, 1.0)),
        SnapZone::Center => Some((0.2, 0.2, 0.8, 0.8)),
        SnapZone::Minimize => Some((0.32, 0.82, 0.68, 1.0)),
        SnapZone::LeftThird => Some((0.0, 0.0, 1.0 / 3.0, 1.0)),
        SnapZone::CenterThird => Some((1.0 / 3.0, 0.0, 2.0 / 3.0, 1.0)),
        SnapZone::RightThird => Some((2.0 / 3.0, 0.0, 1.0, 1.0)),
        SnapZone::LeftTwoThird => Some((0.0, 0.0, 2.0 / 3.0, 1.0)),
        SnapZone::RightTwoThird => Some((1.0 / 3.0, 0.0, 1.0, 1.0)),
        SnapZone::TopThird => Some((0.0, 0.0, 1.0, 1.0 / 3.0)),
        SnapZone::CenterRowThird => Some((0.0, 1.0 / 3.0, 1.0, 2.0 / 3.0)),
        SnapZone::BottomThird => Some((0.0, 2.0 / 3.0, 1.0, 1.0)),
        SnapZone::TopTwoThird => Some((0.0, 0.0, 1.0, 2.0 / 3.0)),
        SnapZone::BottomTwoThird => Some((0.0, 1.0 / 3.0, 1.0, 1.0)),
        SnapZone::ThirdTopLeft => Some((0.0, 0.0, 1.0 / 3.0, 1.0 / 3.0)),
        SnapZone::ThirdTopRight => Some((2.0 / 3.0, 0.0, 1.0, 1.0 / 3.0)),
        SnapZone::ThirdBottomLeft => Some((0.0, 2.0 / 3.0, 1.0 / 3.0, 1.0)),
        SnapZone::ThirdBottomRight => Some((2.0 / 3.0, 2.0 / 3.0, 1.0, 1.0)),
        SnapZone::None => None,
    }
}
