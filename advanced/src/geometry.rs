use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SnapZone {
    None,
    LeftHalf,
    RightHalf,
    TopHalf,
    BottomHalf,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Maximize,
    Center,
    Minimize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    pub const fn width(self) -> i32 {
        self.right - self.left
    }
    pub const fn height(self) -> i32 {
        self.bottom - self.top
    }
    pub fn area(self) -> i64 {
        i64::from(self.width()) * i64::from(self.height())
    }
}

fn sized(x: i32, y: i32, width: i32, height: i32) -> Rect {
    Rect::new(x, y, x + width, y + height)
}

/// Computes the same work-area subdivisions as Swoosh, including assigning
/// integer-division remainders to the trailing zone.
pub fn zone_rect(work: Rect, zone: SnapZone, grid_spacing: i32) -> Rect {
    let x = work.left;
    let y = work.top;
    let width = work.width();
    let height = work.height();
    let half_width = width / 2;
    let half_height = height / 2;

    let target = match zone {
        SnapZone::LeftHalf => sized(x, y, half_width, height),
        SnapZone::RightHalf => sized(x + half_width, y, width - half_width, height),
        SnapZone::TopHalf => sized(x, y, width, half_height),
        SnapZone::BottomHalf => sized(x, y + half_height, width, height - half_height),
        SnapZone::TopLeft => sized(x, y, half_width, half_height),
        SnapZone::TopRight => sized(x + half_width, y, width - half_width, half_height),
        SnapZone::BottomLeft => sized(x, y + half_height, half_width, height - half_height),
        SnapZone::BottomRight => sized(
            x + half_width,
            y + half_height,
            width - half_width,
            height - half_height,
        ),
        SnapZone::Center => sized(x + width / 6, y + height / 6, width * 2 / 3, height * 2 / 3),
        SnapZone::Maximize | SnapZone::Minimize | SnapZone::None => work,
    };

    let spacing = grid_spacing.clamp(0, 10);
    if spacing > 0 && target.width() > 4 * spacing && target.height() > 4 * spacing {
        Rect::new(
            target.left + spacing,
            target.top + spacing,
            target.right - spacing,
            target.bottom - spacing,
        )
    } else {
        target
    }
}
