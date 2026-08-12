use std::mem;

use windows_sys::Win32::{
    Foundation::GetLastError,
    UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
        MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
        MOUSEEVENTF_RIGHTUP, MOUSEINPUT, MOUSE_EVENT_FLAGS,
    },
};

use crate::{
    gesture::MouseAction, logging::RingLogger, mouse::PointerAccumulator, settings::DragButton,
};

#[derive(Default)]
pub(super) struct MouseOutput {
    pointer: PointerAccumulator,
}

impl MouseOutput {
    pub fn apply_all(&mut self, actions: &[MouseAction], logger: &RingLogger) {
        for action in actions {
            match *action {
                MouseAction::Move { dx, dy } => {
                    let (integer_x, integer_y) = self.pointer.consume(dx, dy);
                    send(integer_x, integer_y, MOUSEEVENTF_MOVE, logger);
                }
                MouseAction::ButtonDown(button) => {
                    if let Some(flags) = button_flags(button, true) {
                        send(0, 0, flags, logger);
                    }
                }
                MouseAction::ButtonUp(button) => {
                    if let Some(flags) = button_flags(button, false) {
                        send(0, 0, flags, logger);
                    }
                }
            }
        }
    }
}

fn button_flags(button: DragButton, pressed: bool) -> Option<MOUSE_EVENT_FLAGS> {
    match (button, pressed) {
        (DragButton::Left, true) => Some(MOUSEEVENTF_LEFTDOWN),
        (DragButton::Left, false) => Some(MOUSEEVENTF_LEFTUP),
        (DragButton::Right, true) => Some(MOUSEEVENTF_RIGHTDOWN),
        (DragButton::Right, false) => Some(MOUSEEVENTF_RIGHTUP),
        (DragButton::Middle, true) => Some(MOUSEEVENTF_MIDDLEDOWN),
        (DragButton::Middle, false) => Some(MOUSEEVENTF_MIDDLEUP),
        (DragButton::None, _) => None,
    }
}

fn send(delta_x: i32, delta_y: i32, flags: MOUSE_EVENT_FLAGS, logger: &RingLogger) {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: delta_x,
                dy: delta_y,
                mouseData: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };

    let sent = unsafe { SendInput(1, &input, mem::size_of::<INPUT>() as i32) };
    if sent == 0 {
        let error = unsafe { GetLastError() };
        logger.record(format!("SendInput failed with error {error}."));
    }
}
