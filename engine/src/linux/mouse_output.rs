use std::{collections::BTreeMap, io};

use evdev::{
    uinput::VirtualDevice, AttributeSet, EventType, InputEvent, KeyCode, RelativeAxisCode,
};

use crate::{
    gesture::{MouseAction, RELEASE_GAP_MS},
    logging::RingLogger,
    mouse::PointerAccumulator,
    settings::DragButton,
};

pub(super) trait EventSink {
    fn emit_events(&mut self, events: &[InputEvent]) -> io::Result<()>;
}

impl EventSink for VirtualDevice {
    fn emit_events(&mut self, events: &[InputEvent]) -> io::Result<()> {
        self.emit(events)
    }
}

/// Owns the synthetic mouse and keeps button ownership separate for every
/// physical touchpad.  A single uinput key must stay down until the last
/// touchpad that requested it has ended its drag.
pub(super) struct MouseOutput<S: EventSink = VirtualDevice> {
    device: Option<S>,
    pointer: PointerAccumulator,
    held_by_source: BTreeMap<String, DragButton>,
    failure: Option<String>,
}

impl MouseOutput<VirtualDevice> {
    pub(super) fn create() -> io::Result<Self> {
        Ok(Self::from_sink(create_virtual_device()?))
    }

    /// Recreate a uinput device after an I/O failure. Dropping the failed
    /// virtual device first makes the kernel release any keys it still held.
    pub(super) fn reconnect(&mut self) -> io::Result<bool> {
        if self.device.is_some() {
            return Ok(false);
        }
        self.device = Some(create_virtual_device()?);
        self.pointer = PointerAccumulator::default();
        self.held_by_source.clear();
        Ok(true)
    }
}

impl<S: EventSink> MouseOutput<S> {
    fn from_sink(device: S) -> Self {
        Self {
            device: Some(device),
            pointer: PointerAccumulator::default(),
            held_by_source: BTreeMap::new(),
            failure: None,
        }
    }

    pub(super) fn is_available(&self) -> bool {
        self.device.is_some()
    }

    pub(super) fn take_failure(&mut self) -> Option<String> {
        self.failure.take()
    }

    pub(super) fn apply_all(&mut self, source: &str, actions: &[MouseAction], logger: &RingLogger) {
        if self.device.is_none() {
            return;
        }

        for action in actions {
            let result = match *action {
                MouseAction::Move { dx, dy } => self.emit_move(dx, dy),
                MouseAction::ButtonDown(button) => self.press(source, button),
                // DragEngine releases using the *current* setting. The setting
                // may have changed since ButtonDown, so release what this
                // source actually owns rather than trusting `button`.
                MouseAction::ButtonUp(_button) => self.release_source_inner(source),
            };
            if let Err(error) = result {
                self.invalidate(error, logger);
                break;
            }
        }
    }

    pub(super) fn release_source(&mut self, source: &str, logger: &RingLogger) {
        if self.device.is_none() {
            self.held_by_source.remove(source);
            return;
        }
        if let Err(error) = self.release_source_inner(source) {
            self.invalidate(error, logger);
        }
    }

    pub(super) fn release_all(&mut self, logger: &RingLogger) {
        let sources = self.held_by_source.keys().cloned().collect::<Vec<_>>();
        for source in sources {
            if let Err(error) = self.release_source_inner(&source) {
                self.invalidate(error, logger);
                break;
            }
        }
    }

    fn emit_move(&mut self, dx: f32, dy: f32) -> io::Result<()> {
        let (dx, dy) = self.pointer.consume(dx, dy);
        let mut events = Vec::with_capacity(2);
        if dx != 0 {
            events.push(InputEvent::new(
                EventType::RELATIVE.0,
                RelativeAxisCode::REL_X.0,
                dx,
            ));
        }
        if dy != 0 {
            events.push(InputEvent::new(
                EventType::RELATIVE.0,
                RelativeAxisCode::REL_Y.0,
                dy,
            ));
        }
        if events.is_empty() {
            Ok(())
        } else {
            self.emit(&events)
        }
    }

    fn press(&mut self, source: &str, button: DragButton) -> io::Result<()> {
        if button == DragButton::None {
            return self.release_source_inner(source);
        }
        if self.held_by_source.get(source).copied() == Some(button) {
            return Ok(());
        }

        // A settings change can switch buttons during a drag. Safely finish
        // the old ownership before acquiring the new one.
        self.release_source_inner(source)?;
        let already_held = self.held_by_source.values().any(|held| *held == button);
        if !already_held {
            self.emit_button(button, true)?;
        }
        self.held_by_source.insert(source.to_owned(), button);
        Ok(())
    }

    fn release_source_inner(&mut self, source: &str) -> io::Result<()> {
        let Some(button) = self.held_by_source.get(source).copied() else {
            return Ok(());
        };
        let held_elsewhere = self
            .held_by_source
            .iter()
            .any(|(owner, held)| owner != source && *held == button);
        if !held_elsewhere {
            self.emit_button(button, false)?;
        }
        self.held_by_source.remove(source);
        Ok(())
    }

    fn emit_button(&mut self, button: DragButton, pressed: bool) -> io::Result<()> {
        let Some(key) = button_key(button) else {
            return Ok(());
        };
        self.emit(&[InputEvent::new(EventType::KEY.0, key.0, i32::from(pressed))])
    }

    fn emit(&mut self, events: &[InputEvent]) -> io::Result<()> {
        self.device
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "uinput is unavailable"))?
            .emit_events(events)
    }

    fn invalidate(&mut self, error: io::Error, logger: &RingLogger) {
        let message = format!("uinput event failed; virtual mouse was reset: {error}");
        logger.record(&message);
        self.failure = Some(message);
        // Closing a uinput device is the fail-safe for a potentially stuck
        // button when a release write itself failed.
        self.device.take();
        self.held_by_source.clear();
        self.pointer = PointerAccumulator::default();
    }
}

impl<S: EventSink> Drop for MouseOutput<S> {
    fn drop(&mut self) {
        // This is best effort. Even if the write fails, dropping the uinput fd
        // immediately afterwards unregisters the device and clears key state.
        let Some(device) = self.device.as_mut() else {
            return;
        };
        let mut released = Vec::new();
        for button in [DragButton::Left, DragButton::Right, DragButton::Middle] {
            if self.held_by_source.values().any(|held| *held == button) {
                if let Some(key) = button_key(button) {
                    released.push(InputEvent::new(EventType::KEY.0, key.0, 0));
                }
            }
        }
        if !released.is_empty() {
            let _ = device.emit_events(&released);
        }
    }
}

fn create_virtual_device() -> io::Result<VirtualDevice> {
    let result = (|| {
        let mut buttons = AttributeSet::<KeyCode>::new();
        buttons.insert(KeyCode::BTN_LEFT);
        buttons.insert(KeyCode::BTN_RIGHT);
        buttons.insert(KeyCode::BTN_MIDDLE);

        let axes = AttributeSet::from_iter([RelativeAxisCode::REL_X, RelativeAxisCode::REL_Y]);
        let device = VirtualDevice::builder()?
            .name("Three Finger Drag virtual mouse")
            .with_keys(&buttons)?
            .with_relative_axes(&axes)?
            .build()?;

        // Let Mutter/libinput finish discovering the new virtual pointer before
        // the first possible drag event is emitted.
        std::thread::sleep(std::time::Duration::from_millis(RELEASE_GAP_MS));
        Ok(device)
    })();
    result.map_err(permission_hint)
}

fn button_key(button: DragButton) -> Option<KeyCode> {
    match button {
        DragButton::Left => Some(KeyCode::BTN_LEFT),
        DragButton::Right => Some(KeyCode::BTN_RIGHT),
        DragButton::Middle => Some(KeyCode::BTN_MIDDLE),
        DragButton::None => None,
    }
}

fn permission_hint(error: io::Error) -> io::Error {
    if error.kind() == io::ErrorKind::PermissionDenied {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "无法打开 /dev/uinput。请授予当前登录用户 uinput 读写权限后重试。",
        )
    } else {
        error
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeSink {
        batches: Vec<Vec<(u16, u16, i32)>>,
        fail_next: bool,
    }

    impl EventSink for FakeSink {
        fn emit_events(&mut self, events: &[InputEvent]) -> io::Result<()> {
            if std::mem::take(&mut self.fail_next) {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "test failure"));
            }
            self.batches.push(
                events
                    .iter()
                    .map(|event| (event.event_type().0, event.code(), event.value()))
                    .collect(),
            );
            Ok(())
        }
    }

    fn action(output: &mut MouseOutput<FakeSink>, source: &str, action: MouseAction) {
        output.apply_all(source, &[action], &RingLogger::default());
    }

    #[test]
    fn shared_button_stays_down_until_the_last_touchpad_releases_it() {
        let mut output = MouseOutput::from_sink(FakeSink::default());
        action(
            &mut output,
            "touchpad-a",
            MouseAction::ButtonDown(DragButton::Left),
        );
        action(
            &mut output,
            "touchpad-b",
            MouseAction::ButtonDown(DragButton::Left),
        );
        action(
            &mut output,
            "touchpad-a",
            MouseAction::ButtonUp(DragButton::Left),
        );
        action(
            &mut output,
            "touchpad-b",
            MouseAction::ButtonUp(DragButton::Left),
        );

        let batches = &output.device.as_ref().unwrap().batches;
        assert_eq!(
            batches,
            &vec![
                vec![(EventType::KEY.0, KeyCode::BTN_LEFT.0, 1)],
                vec![(EventType::KEY.0, KeyCode::BTN_LEFT.0, 0)],
            ]
        );
    }

    #[test]
    fn button_up_releases_the_button_that_source_actually_pressed() {
        let mut output = MouseOutput::from_sink(FakeSink::default());
        action(
            &mut output,
            "touchpad",
            MouseAction::ButtonDown(DragButton::Left),
        );
        action(
            &mut output,
            "touchpad",
            MouseAction::ButtonUp(DragButton::Right),
        );

        let batches = &output.device.as_ref().unwrap().batches;
        assert_eq!(batches[1], vec![(EventType::KEY.0, KeyCode::BTN_LEFT.0, 0)]);
        assert!(output.held_by_source.is_empty());
    }

    #[test]
    fn movement_axes_are_emitted_in_one_uinput_frame() {
        let mut output = MouseOutput::from_sink(FakeSink::default());
        action(
            &mut output,
            "touchpad",
            MouseAction::Move { dx: 2.5, dy: -3.5 },
        );

        assert_eq!(
            output.device.as_ref().unwrap().batches,
            vec![vec![
                (EventType::RELATIVE.0, RelativeAxisCode::REL_X.0, 2),
                (EventType::RELATIVE.0, RelativeAxisCode::REL_Y.0, -3),
            ]]
        );
    }

    #[test]
    fn failed_write_drops_the_virtual_device_and_forgets_ownership() {
        let sink = FakeSink {
            fail_next: true,
            ..FakeSink::default()
        };
        let mut output = MouseOutput::from_sink(sink);
        action(
            &mut output,
            "touchpad",
            MouseAction::ButtonDown(DragButton::Left),
        );

        assert!(!output.is_available());
        assert!(output.held_by_source.is_empty());
        assert!(output.take_failure().is_some());
    }
}
