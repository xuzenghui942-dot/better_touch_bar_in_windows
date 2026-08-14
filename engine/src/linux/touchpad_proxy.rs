use std::{collections::BTreeSet, io, thread, time::Duration};

use evdev::{
    raw_stream::RawDevice, uinput::VirtualDevice, AbsoluteAxisCode, EventType, InputEvent, KeyCode,
    SynchronizationCode, UinputAbsSetup,
};

const PROXY_NAME_PREFIX: &str = "Three Finger Drag proxied touchpad";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProxyMode {
    Native,
    TwoFingerCandidate,
    TwoFingerOwned,
    ThreeFingerOwned,
    NativeUntilLift,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameAction {
    Emit,
    Suppress,
    Release,
    Resume,
}

/// Exclusive physical-touchpad owner used only while advanced two-finger
/// gestures are enabled.  Every input family not owned by this application is
/// copied to a virtual touchpad, so libinput keeps normal pointer, scrolling,
/// four-finger and hardware-button behavior.
pub struct TouchpadProxy {
    output: VirtualDevice,
    mode: ProxyMode,
    minimum_slot: i32,
    maximum_slot: i32,
    supported_tool_keys: BTreeSet<u16>,
    physical_buttons: BTreeSet<u16>,
    virtual_buttons: BTreeSet<u16>,
}

impl TouchpadProxy {
    pub fn create_and_grab(physical: &mut RawDevice) -> io::Result<Self> {
        let (minimum_slot, maximum_slot) = physical
            .get_absinfo()?
            .find_map(|(axis, info)| {
                (axis == AbsoluteAxisCode::ABS_MT_SLOT).then_some((info.minimum(), info.maximum()))
            })
            .unwrap_or((0, 31));
        let supported_tool_keys = physical
            .supported_keys()
            .map(|keys| keys.iter().map(|code| code.0).collect())
            .unwrap_or_default();
        let mut output = clone_touchpad(physical)?;
        // uinput creation is asynchronous from the compositor's libinput
        // device monitor. Creation only occurs on an empty physical frame, so
        // this short bounded settle cannot strand an in-progress gesture.
        let device_node = output.enumerate_dev_nodes_blocking()?.next().transpose()?;
        if device_node.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "the proxied touchpad uinput node was not created",
            ));
        }
        thread::sleep(Duration::from_millis(120));
        // Create the replacement before excluding the physical device.  If
        // EVIOCGRAB fails the uinput device is dropped and native input was
        // never interrupted.
        physical.grab()?;
        // An empty frame makes the virtual device visible to libinput without
        // inventing any contact or key state.
        if let Err(error) = output.emit(&[]) {
            let _ = physical.ungrab();
            return Err(error);
        }
        Ok(Self {
            output,
            mode: ProxyMode::Native,
            minimum_slot,
            maximum_slot,
            supported_tool_keys,
            physical_buttons: BTreeSet::new(),
            virtual_buttons: BTreeSet::new(),
        })
    }

    pub fn is_virtual_name(name: Option<&str>) -> bool {
        name.is_some_and(|value| value.starts_with(PROXY_NAME_PREFIX))
    }

    pub fn handle_frame(
        &mut self,
        frame: &[InputEvent],
        contacts: &[ProxyContact],
    ) -> io::Result<()> {
        self.observe_physical_buttons(frame);
        let (next, action) = transition(self.mode, contacts.len());
        match action {
            FrameAction::Emit => self.emit_frame(frame)?,
            FrameAction::Suppress => {}
            FrameAction::Release => self.release_native_contacts()?,
            FrameAction::Resume => self.resume_native(contacts)?,
        }
        self.mode = next;
        Ok(())
    }

    /// Lock a recognized two-finger gesture to its current physical contact
    /// stream. The recognizer calls this only after crossing its existing
    /// intent threshold, so a normal sequential three-finger landing can still
    /// move from `TwoFingerCandidate` to `ThreeFingerOwned` without delay.
    pub fn commit_two_finger_candidate(&mut self) {
        self.mode = commit_two_finger_candidate(self.mode);
    }

    /// The cloned touchpad owns native/replayed streams independently from the
    /// basic drag engine. A replayed two-finger scroll or a committed advanced
    /// gesture must never be stolen by three-finger drag before every contact
    /// lifts.
    pub fn allows_three_finger_drag(&self) -> bool {
        allows_three_finger_drag(self.mode)
    }

    /// A broker rejection means the pointer was not over a manageable
    /// titlebar.  Reconstruct the currently active contacts and then pass the
    /// rest of this physical stream through, preserving ordinary scrolling.
    pub fn reject_candidate(&mut self, contacts: &[ProxyContact]) -> io::Result<()> {
        if !matches!(
            self.mode,
            ProxyMode::TwoFingerCandidate | ProxyMode::TwoFingerOwned
        ) {
            return Ok(());
        }
        if contacts.is_empty() {
            self.mode = ProxyMode::Native;
        } else {
            self.resume_native(contacts)?;
        }
        Ok(())
    }

    pub fn release_all(&mut self) -> io::Result<()> {
        self.release_native_contacts()?;
        self.mode = ProxyMode::Native;
        Ok(())
    }

    fn resume_native(&mut self, contacts: &[ProxyContact]) -> io::Result<()> {
        let mut events = Vec::with_capacity(contacts.len() * 4 + 8);
        for contact in contacts {
            events.push(abs(AbsoluteAxisCode::ABS_MT_SLOT, contact.slot));
            events.push(abs(
                AbsoluteAxisCode::ABS_MT_TRACKING_ID,
                contact.tracking_id,
            ));
            events.push(abs(AbsoluteAxisCode::ABS_MT_POSITION_X, contact.x));
            events.push(abs(AbsoluteAxisCode::ABS_MT_POSITION_Y, contact.y));
        }
        events.extend(self.tool_state_events(contacts.len(), true));
        events.extend(
            self.physical_buttons
                .iter()
                .map(|code| InputEvent::new(EventType::KEY.0, *code, 1)),
        );
        self.output.emit(&events)?;
        self.virtual_buttons = self.physical_buttons.clone();
        self.mode = ProxyMode::NativeUntilLift;
        Ok(())
    }

    fn release_native_contacts(&mut self) -> io::Result<()> {
        let mut events = Vec::new();
        // Type-B multitouch slots are finite. Releasing every advertised slot
        // is safe and prevents a partially forwarded one-finger frame from
        // surviving when a second/third finger changes ownership.
        for slot in self.minimum_slot..=self.maximum_slot {
            events.push(abs(AbsoluteAxisCode::ABS_MT_SLOT, slot));
            events.push(abs(AbsoluteAxisCode::ABS_MT_TRACKING_ID, -1));
        }
        events.extend(self.tool_state_events(0, false));
        events.extend(
            self.virtual_buttons
                .iter()
                .map(|code| InputEvent::new(EventType::KEY.0, *code, 0)),
        );
        self.output.emit(&events)?;
        self.virtual_buttons.clear();
        Ok(())
    }

    fn emit_frame(&mut self, frame: &[InputEvent]) -> io::Result<()> {
        let events = frame
            .iter()
            .copied()
            .filter(|event| {
                event.event_type() != EventType::SYNCHRONIZATION
                    || event.code() != SynchronizationCode::SYN_REPORT.0
            })
            .collect::<Vec<_>>();
        self.output.emit(&events)?;
        self.virtual_buttons = self.physical_buttons.clone();
        Ok(())
    }

    fn observe_physical_buttons(&mut self, frame: &[InputEvent]) {
        for event in frame
            .iter()
            .filter(|event| event.event_type() == EventType::KEY && !is_contact_key(event.code()))
        {
            if event.value() == 0 {
                self.physical_buttons.remove(&event.code());
            } else {
                self.physical_buttons.insert(event.code());
            }
        }
    }

    fn tool_state_events(&self, count: usize, touching: bool) -> Vec<InputEvent> {
        let states = [
            (KeyCode::BTN_TOUCH, touching),
            (KeyCode::BTN_TOOL_FINGER, count == 1),
            (KeyCode::BTN_TOOL_DOUBLETAP, count == 2),
            (KeyCode::BTN_TOOL_TRIPLETAP, count == 3),
            (KeyCode::BTN_TOOL_QUADTAP, count == 4),
            (KeyCode::BTN_TOOL_QUINTTAP, count >= 5),
        ];
        states
            .into_iter()
            .filter(|(code, _)| self.supported_tool_keys.contains(&code.0))
            .map(|(code, active)| key(code, i32::from(active)))
            .collect()
    }
}

fn transition(mode: ProxyMode, contacts: usize) -> (ProxyMode, FrameAction) {
    match mode {
        ProxyMode::Native => match contacts {
            2 => (ProxyMode::TwoFingerCandidate, FrameAction::Release),
            3 => (ProxyMode::ThreeFingerOwned, FrameAction::Release),
            _ => (ProxyMode::Native, FrameAction::Emit),
        },
        ProxyMode::TwoFingerCandidate => match contacts {
            0 => (ProxyMode::Native, FrameAction::Suppress),
            3 => (ProxyMode::ThreeFingerOwned, FrameAction::Suppress),
            4.. => (ProxyMode::NativeUntilLift, FrameAction::Resume),
            _ => (ProxyMode::TwoFingerCandidate, FrameAction::Suppress),
        },
        ProxyMode::TwoFingerOwned => match contacts {
            0 => (ProxyMode::Native, FrameAction::Suppress),
            4.. => (ProxyMode::NativeUntilLift, FrameAction::Resume),
            _ => (ProxyMode::TwoFingerOwned, FrameAction::Suppress),
        },
        ProxyMode::ThreeFingerOwned => match contacts {
            0 => (ProxyMode::Native, FrameAction::Suppress),
            4.. => (ProxyMode::NativeUntilLift, FrameAction::Resume),
            _ => (ProxyMode::ThreeFingerOwned, FrameAction::Suppress),
        },
        ProxyMode::NativeUntilLift if contacts == 0 => (ProxyMode::Native, FrameAction::Emit),
        ProxyMode::NativeUntilLift => (ProxyMode::NativeUntilLift, FrameAction::Emit),
    }
}

fn commit_two_finger_candidate(mode: ProxyMode) -> ProxyMode {
    if mode == ProxyMode::TwoFingerCandidate {
        ProxyMode::TwoFingerOwned
    } else {
        mode
    }
}

fn allows_three_finger_drag(mode: ProxyMode) -> bool {
    !matches!(mode, ProxyMode::TwoFingerOwned | ProxyMode::NativeUntilLift)
}

impl Drop for TouchpadProxy {
    fn drop(&mut self) {
        let _ = self.release_all();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProxyContact {
    pub slot: i32,
    pub tracking_id: i32,
    pub x: i32,
    pub y: i32,
}

fn clone_touchpad(physical: &RawDevice) -> io::Result<VirtualDevice> {
    let physical_name = physical.name().unwrap_or("touchpad");
    let name = format!("{PROXY_NAME_PREFIX}: {physical_name}");
    let mut builder = VirtualDevice::builder()?
        .name(&name)
        .input_id(physical.input_id())
        .with_properties(physical.properties())?;

    if let Some(keys) = physical.supported_keys() {
        builder = builder.with_keys(keys)?;
    }
    if let Some(axes) = physical.supported_relative_axes() {
        builder = builder.with_relative_axes(axes)?;
    }
    for (axis, info) in physical.get_absinfo()? {
        builder = builder.with_absolute_axis(&UinputAbsSetup::new(axis, info))?;
    }
    if let Some(switches) = physical.supported_switches() {
        builder = builder.with_switches(switches)?;
    }
    if let Some(misc) = physical.misc_properties() {
        builder = builder.with_msc(misc)?;
    }
    builder.build()
}

fn abs(code: AbsoluteAxisCode, value: i32) -> InputEvent {
    InputEvent::new(EventType::ABSOLUTE.0, code.0, value)
}

fn key(code: KeyCode, value: i32) -> InputEvent {
    InputEvent::new(EventType::KEY.0, code.0, value)
}

fn is_contact_key(code: u16) -> bool {
    matches!(
        KeyCode(code),
        KeyCode::BTN_TOUCH
            | KeyCode::BTN_TOOL_FINGER
            | KeyCode::BTN_TOOL_DOUBLETAP
            | KeyCode::BTN_TOOL_TRIPLETAP
            | KeyCode::BTN_TOOL_QUADTAP
            | KeyCode::BTN_TOOL_QUINTTAP
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_and_three_finger_streams_are_owned_before_native_dispatch() {
        assert_eq!(
            transition(ProxyMode::Native, 2),
            (ProxyMode::TwoFingerCandidate, FrameAction::Release)
        );
        assert_eq!(
            transition(ProxyMode::Native, 3),
            (ProxyMode::ThreeFingerOwned, FrameAction::Release)
        );
        assert_eq!(
            transition(ProxyMode::TwoFingerCandidate, 2),
            (ProxyMode::TwoFingerCandidate, FrameAction::Suppress)
        );
        assert_eq!(
            transition(ProxyMode::TwoFingerCandidate, 3),
            (ProxyMode::ThreeFingerOwned, FrameAction::Suppress)
        );
    }

    #[test]
    fn committed_two_finger_stream_never_transfers_to_three_before_lift() {
        assert_eq!(
            commit_two_finger_candidate(ProxyMode::TwoFingerCandidate),
            ProxyMode::TwoFingerOwned
        );
        for unchanged in [
            ProxyMode::Native,
            ProxyMode::TwoFingerOwned,
            ProxyMode::ThreeFingerOwned,
            ProxyMode::NativeUntilLift,
        ] {
            assert_eq!(commit_two_finger_candidate(unchanged), unchanged);
        }
        assert_eq!(
            transition(ProxyMode::TwoFingerOwned, 3),
            (ProxyMode::TwoFingerOwned, FrameAction::Suppress)
        );
        assert_eq!(
            transition(ProxyMode::TwoFingerOwned, 2),
            (ProxyMode::TwoFingerOwned, FrameAction::Suppress)
        );
        assert_eq!(
            transition(ProxyMode::TwoFingerOwned, 0),
            (ProxyMode::Native, FrameAction::Suppress)
        );
    }

    #[test]
    fn two_finger_owned_and_native_replay_lanes_block_the_drag_engine() {
        assert!(allows_three_finger_drag(ProxyMode::Native));
        assert!(allows_three_finger_drag(ProxyMode::TwoFingerCandidate));
        assert!(allows_three_finger_drag(ProxyMode::ThreeFingerOwned));
        assert!(!allows_three_finger_drag(ProxyMode::TwoFingerOwned));
        assert!(!allows_three_finger_drag(ProxyMode::NativeUntilLift));
    }

    #[test]
    fn four_fingers_resume_native_and_stay_native_until_lift() {
        assert_eq!(
            transition(ProxyMode::TwoFingerCandidate, 4),
            (ProxyMode::NativeUntilLift, FrameAction::Resume)
        );
        assert_eq!(
            transition(ProxyMode::ThreeFingerOwned, 4),
            (ProxyMode::NativeUntilLift, FrameAction::Resume)
        );
        assert_eq!(
            transition(ProxyMode::TwoFingerOwned, 4),
            (ProxyMode::NativeUntilLift, FrameAction::Resume)
        );
        assert_eq!(
            transition(ProxyMode::NativeUntilLift, 2),
            (ProxyMode::NativeUntilLift, FrameAction::Emit)
        );
        assert_eq!(
            transition(ProxyMode::NativeUntilLift, 0),
            (ProxyMode::Native, FrameAction::Emit)
        );
    }

    #[test]
    fn virtual_clone_is_excluded_from_physical_device_discovery() {
        assert!(TouchpadProxy::is_virtual_name(Some(
            "Three Finger Drag proxied touchpad: ELAN"
        )));
        assert!(!TouchpadProxy::is_virtual_name(Some("ELAN Touchpad")));
    }
}
