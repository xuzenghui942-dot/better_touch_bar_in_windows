use crate::geometry::SnapZone;

pub type TaskbarTargetId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskbarGestureDirection {
    Up,
    Left,
    Right,
    Down,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskbarGestureAction {
    Activate {
        target: TaskbarTargetId,
    },
    ActivateAndSnap {
        target: TaskbarTargetId,
        zone: SnapZone,
    },
}

/// Maps only a resolved taskbar icon to an action. Absence is intentional: the
/// foreground window must never be substituted for the icon beneath the cursor.
pub fn action_for_taskbar_gesture(
    icon_under_cursor: Option<TaskbarTargetId>,
    direction: TaskbarGestureDirection,
) -> Option<TaskbarGestureAction> {
    let target = icon_under_cursor?;
    match direction {
        TaskbarGestureDirection::Up => Some(TaskbarGestureAction::Activate { target }),
        TaskbarGestureDirection::Left => Some(TaskbarGestureAction::ActivateAndSnap {
            target,
            zone: SnapZone::LeftHalf,
        }),
        TaskbarGestureDirection::Right => Some(TaskbarGestureAction::ActivateAndSnap {
            target,
            zone: SnapZone::RightHalf,
        }),
        TaskbarGestureDirection::Down | TaskbarGestureDirection::Other => None,
    }
}
