use std::{
    collections::{BTreeMap, BTreeSet},
    io, thread,
    time::{Duration, Instant},
};

use evdev::{
    raw_stream::RawDevice, uinput::VirtualDevice, AbsoluteAxisCode, EventType, InputEvent, KeyCode,
    SynchronizationCode, UinputAbsSetup,
};

const PROXY_NAME_PREFIX: &str = "Three Finger Drag proxied touchpad";
// Real GXTP5100 captures put the sequential 2 -> 3 landing phase below 20 ms.
// Leave enough room for the asynchronous GNOME BeginRejected round trip while
// keeping an established native two-finger scroll outside the drag lane.
const REJECTED_CANDIDATE_LANDING_GRACE: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProxyMode {
    Native,
    TwoFingerCandidate,
    TwoFingerRejectedGrace,
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
    rejected_candidate_deadline: Option<Instant>,
    minimum_slot: i32,
    maximum_slot: i32,
    supported_tool_keys: BTreeSet<u16>,
    physical_buttons: BTreeSet<u16>,
    virtual_buttons: BTreeSet<u16>,
    virtual_contacts: BTreeMap<i32, i32>,
    virtual_current_slot: i32,
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
            rejected_candidate_deadline: None,
            minimum_slot,
            maximum_slot,
            supported_tool_keys,
            physical_buttons: BTreeSet::new(),
            virtual_buttons: BTreeSet::new(),
            virtual_contacts: BTreeMap::new(),
            virtual_current_slot: minimum_slot,
        })
    }

    pub fn is_virtual_name(name: Option<&str>) -> bool {
        name.is_some_and(|value| value.starts_with(PROXY_NAME_PREFIX))
    }

    pub fn handle_frame(
        &mut self,
        frame: &[InputEvent],
        contacts: &[ProxyContact],
        current_slot: Option<i32>,
        global_axes: &[(u16, i32)],
    ) -> io::Result<()> {
        self.observe_physical_buttons(frame);
        if self.mode == ProxyMode::TwoFingerRejectedGrace {
            let grace_is_open = self
                .rejected_candidate_deadline
                .is_some_and(|deadline| Instant::now() <= deadline);
            return self.resolve_rejected_candidate(
                contacts,
                current_slot,
                global_axes,
                grace_is_open,
            );
        }
        let (next, action) = transition(self.mode, contacts.len());
        match action {
            FrameAction::Emit => self.emit_frame(frame, contacts.len())?,
            FrameAction::Suppress => {}
            FrameAction::Release => self.release_native_contacts(current_slot)?,
            FrameAction::Resume => self.resume_native(contacts, current_slot, global_axes)?,
        }
        self.mode = next;
        if self.mode != ProxyMode::TwoFingerRejectedGrace {
            self.rejected_candidate_deadline = None;
        }
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
    /// titlebar. Reconstruct the currently active contacts and pass the stream
    /// through natively. A short rejection-local grace lets a third finger
    /// complete a sequential landing without changing accepted window input.
    pub fn reject_candidate(
        &mut self,
        contacts: &[ProxyContact],
        current_slot: Option<i32>,
        global_axes: &[(u16, i32)],
    ) -> io::Result<()> {
        let (next, action, arm_landing_grace) = transition_on_rejection(self.mode, contacts.len());
        if next == self.mode && action == FrameAction::Suppress && !arm_landing_grace {
            return Ok(());
        }
        match action {
            FrameAction::Suppress => {}
            FrameAction::Resume => self.resume_native(contacts, current_slot, global_axes)?,
            FrameAction::Emit | FrameAction::Release => {
                unreachable!("broker rejection cannot emit an incomplete native frame")
            }
        }
        self.mode = next;
        self.rejected_candidate_deadline =
            arm_landing_grace.then(|| Instant::now() + REJECTED_CANDIDATE_LANDING_GRACE);
        Ok(())
    }

    /// Resume a rejected two-finger stream once the sequential landing window
    /// has elapsed, even if the stationary touchpad produced no new frame.
    pub fn flush_rejected_candidate_if_due(
        &mut self,
        contacts: &[ProxyContact],
        current_slot: Option<i32>,
        global_axes: &[(u16, i32)],
    ) -> io::Result<()> {
        if self.mode != ProxyMode::TwoFingerRejectedGrace
            || self
                .rejected_candidate_deadline
                .is_some_and(|deadline| Instant::now() <= deadline)
        {
            return Ok(());
        }
        self.resolve_rejected_candidate(contacts, current_slot, global_axes, false)
    }

    pub fn release_all(&mut self) -> io::Result<()> {
        self.release_native_contacts(None)?;
        self.mode = ProxyMode::Native;
        self.rejected_candidate_deadline = None;
        Ok(())
    }

    fn resume_native(
        &mut self,
        contacts: &[ProxyContact],
        current_slot: Option<i32>,
        global_axes: &[(u16, i32)],
    ) -> io::Result<()> {
        let mut events = Vec::with_capacity(
            contacts
                .iter()
                .map(|contact| contact.axes.len() + 2)
                .sum::<usize>()
                + global_axes.len()
                + 8,
        );
        events.extend(
            global_axes
                .iter()
                .map(|(code, value)| abs(AbsoluteAxisCode(*code), *value)),
        );
        for contact in contacts {
            events.extend(resumed_contact_events(contact));
        }
        events.extend(self.tool_state_events(contacts.len(), true));
        events.extend(
            self.physical_buttons
                .iter()
                .map(|code| InputEvent::new(EventType::KEY.0, *code, 1)),
        );
        if let Some(event) =
            slot_alignment_event(current_slot, self.minimum_slot, self.maximum_slot)
        {
            events.push(event);
        }
        self.output.emit(&events)?;
        self.virtual_buttons = self.physical_buttons.clone();
        self.virtual_contacts = contacts
            .iter()
            .map(|contact| (contact.slot, contact.tracking_id))
            .collect();
        self.virtual_current_slot = current_slot
            .filter(|slot| (self.minimum_slot..=self.maximum_slot).contains(slot))
            .or_else(|| contacts.last().map(|contact| contact.slot))
            .unwrap_or(self.minimum_slot);
        self.mode = ProxyMode::NativeUntilLift;
        self.rejected_candidate_deadline = None;
        Ok(())
    }

    fn resolve_rejected_candidate(
        &mut self,
        contacts: &[ProxyContact],
        current_slot: Option<i32>,
        global_axes: &[(u16, i32)],
        grace_is_open: bool,
    ) -> io::Result<()> {
        let (next, action) = transition_rejected_candidate(contacts.len(), grace_is_open);
        match action {
            FrameAction::Suppress => {}
            FrameAction::Resume => self.resume_native(contacts, current_slot, global_axes)?,
            FrameAction::Emit | FrameAction::Release => {
                unreachable!("rejected candidate contacts have not entered the native stream")
            }
        }
        self.mode = next;
        if self.mode != ProxyMode::TwoFingerRejectedGrace {
            self.rejected_candidate_deadline = None;
        }
        Ok(())
    }

    fn release_native_contacts(&mut self, current_slot: Option<i32>) -> io::Result<()> {
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
        if let Some(event) =
            slot_alignment_event(current_slot, self.minimum_slot, self.maximum_slot)
        {
            events.push(event);
        }
        self.output.emit(&events)?;
        self.virtual_buttons.clear();
        self.virtual_contacts.clear();
        self.virtual_current_slot = current_slot
            .filter(|slot| (self.minimum_slot..=self.maximum_slot).contains(slot))
            .unwrap_or(self.maximum_slot);
        Ok(())
    }

    fn emit_frame(&mut self, frame: &[InputEvent], contacts: usize) -> io::Result<()> {
        let mut events = sanitized_native_frame_events(
            frame,
            &mut self.virtual_contacts,
            &mut self.virtual_current_slot,
            self.minimum_slot,
            self.maximum_slot,
        );
        // Some touchpad firmware reports mutually inconsistent BTN_TOOL_* keys.
        // libinput treats those as an invalid fake-finger state.  Slot tracking
        // is authoritative here, so replace only the contact/tool keys while
        // preserving physical buttons and every non-contact event.
        events.extend(self.tool_state_events(contacts, contacts > 0));
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
            1 => (ProxyMode::NativeUntilLift, FrameAction::Resume),
            3 => (ProxyMode::ThreeFingerOwned, FrameAction::Suppress),
            4.. => (ProxyMode::NativeUntilLift, FrameAction::Resume),
            _ => (ProxyMode::TwoFingerCandidate, FrameAction::Suppress),
        },
        ProxyMode::TwoFingerRejectedGrace => {
            unreachable!("rejected candidates require a deadline-aware transition")
        }
        ProxyMode::TwoFingerOwned => match contacts {
            0 => (ProxyMode::Native, FrameAction::Suppress),
            1 => (ProxyMode::NativeUntilLift, FrameAction::Resume),
            4.. => (ProxyMode::NativeUntilLift, FrameAction::Resume),
            _ => (ProxyMode::TwoFingerOwned, FrameAction::Suppress),
        },
        ProxyMode::ThreeFingerOwned => match contacts {
            0 => (ProxyMode::Native, FrameAction::Suppress),
            1 => (ProxyMode::NativeUntilLift, FrameAction::Resume),
            4.. => (ProxyMode::NativeUntilLift, FrameAction::Resume),
            _ => (ProxyMode::ThreeFingerOwned, FrameAction::Suppress),
        },
        ProxyMode::NativeUntilLift if contacts == 0 => (ProxyMode::Native, FrameAction::Emit),
        ProxyMode::NativeUntilLift => (ProxyMode::NativeUntilLift, FrameAction::Emit),
    }
}

fn transition_rejected_candidate(
    contacts: usize,
    upgrade_during_landing: bool,
) -> (ProxyMode, FrameAction) {
    match contacts {
        0 => (ProxyMode::Native, FrameAction::Suppress),
        2 if upgrade_during_landing => (ProxyMode::TwoFingerRejectedGrace, FrameAction::Suppress),
        3 if upgrade_during_landing => (ProxyMode::ThreeFingerOwned, FrameAction::Suppress),
        _ => (ProxyMode::NativeUntilLift, FrameAction::Resume),
    }
}

fn transition_on_rejection(mode: ProxyMode, contacts: usize) -> (ProxyMode, FrameAction, bool) {
    match (mode, contacts) {
        (ProxyMode::TwoFingerCandidate, 2) => (
            ProxyMode::TwoFingerRejectedGrace,
            FrameAction::Suppress,
            true,
        ),
        (ProxyMode::TwoFingerCandidate | ProxyMode::TwoFingerOwned, 0) => {
            (ProxyMode::Native, FrameAction::Suppress, false)
        }
        (ProxyMode::TwoFingerCandidate | ProxyMode::TwoFingerOwned, _) => {
            (ProxyMode::NativeUntilLift, FrameAction::Resume, false)
        }
        _ => (mode, FrameAction::Suppress, false),
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
    !matches!(
        mode,
        ProxyMode::TwoFingerRejectedGrace | ProxyMode::TwoFingerOwned | ProxyMode::NativeUntilLift
    )
}

impl Drop for TouchpadProxy {
    fn drop(&mut self) {
        let _ = self.release_all();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyContact {
    pub slot: i32,
    pub tracking_id: i32,
    pub axes: Vec<(u16, i32)>,
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

fn native_frame_events(frame: &[InputEvent]) -> Vec<InputEvent> {
    frame
        .iter()
        .copied()
        .filter(|event| {
            !is_contact_key(event.code())
                && (event.event_type() != EventType::SYNCHRONIZATION
                    || event.code() != SynchronizationCode::SYN_REPORT.0)
        })
        .collect()
}

fn sanitized_native_frame_events(
    frame: &[InputEvent],
    active_slots: &mut BTreeMap<i32, i32>,
    current_slot: &mut i32,
    minimum_slot: i32,
    maximum_slot: i32,
) -> Vec<InputEvent> {
    let mut events = Vec::with_capacity(frame.len() + 6);
    for event in native_frame_events(frame) {
        if event.event_type() != EventType::ABSOLUTE {
            events.push(event);
            continue;
        }

        match AbsoluteAxisCode(event.code()) {
            AbsoluteAxisCode::ABS_MT_SLOT => {
                if (minimum_slot..=maximum_slot).contains(&event.value()) {
                    *current_slot = event.value();
                    events.push(event);
                }
            }
            AbsoluteAxisCode::ABS_MT_TRACKING_ID if event.value() < 0 => {
                active_slots.remove(current_slot);
                events.push(event);
            }
            AbsoluteAxisCode::ABS_MT_TRACKING_ID => {
                let target_slot = *current_slot;
                let tracking_id = event.value();
                let mut slots_to_release = active_slots
                    .iter()
                    .filter_map(|(slot, active_id)| {
                        (*slot == target_slot || *active_id == tracking_id).then_some(*slot)
                    })
                    .collect::<Vec<_>>();
                slots_to_release.sort_unstable();
                slots_to_release.dedup();

                for slot in slots_to_release {
                    if *current_slot != slot {
                        events.push(abs(AbsoluteAxisCode::ABS_MT_SLOT, slot));
                        *current_slot = slot;
                    }
                    events.push(abs(AbsoluteAxisCode::ABS_MT_TRACKING_ID, -1));
                    active_slots.remove(&slot);
                }
                if *current_slot != target_slot {
                    events.push(abs(AbsoluteAxisCode::ABS_MT_SLOT, target_slot));
                    *current_slot = target_slot;
                }
                events.push(event);
                active_slots.insert(target_slot, tracking_id);
            }
            _ => events.push(event),
        }
    }
    if frame_touching_state(frame) == Some(false) {
        events.extend(release_unreported_virtual_contacts(
            active_slots,
            current_slot,
        ));
    }
    events
}

fn release_unreported_virtual_contacts(
    active_slots: &mut BTreeMap<i32, i32>,
    current_slot: &mut i32,
) -> Vec<InputEvent> {
    if active_slots.is_empty() {
        return Vec::new();
    }

    let restore_slot = *current_slot;
    let stale_slots = active_slots.keys().copied().collect::<Vec<_>>();
    let mut events = Vec::with_capacity(stale_slots.len() * 2 + 1);
    for slot in stale_slots {
        if *current_slot != slot {
            events.push(abs(AbsoluteAxisCode::ABS_MT_SLOT, slot));
            *current_slot = slot;
        }
        events.push(abs(AbsoluteAxisCode::ABS_MT_TRACKING_ID, -1));
    }
    active_slots.clear();
    if *current_slot != restore_slot {
        events.push(abs(AbsoluteAxisCode::ABS_MT_SLOT, restore_slot));
        *current_slot = restore_slot;
    }
    events
}

pub(super) fn frame_touching_state(frame: &[InputEvent]) -> Option<bool> {
    frame
        .iter()
        .rev()
        .find(|event| event.event_type() == EventType::KEY && event.code() == KeyCode::BTN_TOUCH.0)
        .map(|event| event.value() != 0)
}

fn slot_alignment_event(
    current_slot: Option<i32>,
    minimum_slot: i32,
    maximum_slot: i32,
) -> Option<InputEvent> {
    current_slot
        .filter(|slot| (minimum_slot..=maximum_slot).contains(slot))
        .map(|slot| abs(AbsoluteAxisCode::ABS_MT_SLOT, slot))
}

fn resumed_contact_events(contact: &ProxyContact) -> Vec<InputEvent> {
    let mut events = Vec::with_capacity(contact.axes.len() + 2);
    events.push(abs(AbsoluteAxisCode::ABS_MT_SLOT, contact.slot));
    events.push(abs(
        AbsoluteAxisCode::ABS_MT_TRACKING_ID,
        contact.tracking_id,
    ));
    events.extend(
        contact
            .axes
            .iter()
            .filter(|(code, _)| is_multitouch_contact_axis(*code))
            .map(|(code, value)| abs(AbsoluteAxisCode(*code), *value)),
    );
    events
}

fn is_multitouch_contact_axis(code: u16) -> bool {
    (AbsoluteAxisCode::ABS_MT_TOUCH_MAJOR.0..=AbsoluteAxisCode::ABS_MT_TOOL_Y.0).contains(&code)
        && code != AbsoluteAxisCode::ABS_MT_TRACKING_ID.0
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
            ProxyMode::TwoFingerRejectedGrace,
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
    fn rejected_non_titlebar_candidate_can_upgrade_to_three_during_landing_grace() {
        assert_eq!(
            transition_rejected_candidate(2, true),
            (ProxyMode::TwoFingerRejectedGrace, FrameAction::Suppress)
        );
        assert_eq!(
            transition_rejected_candidate(3, true),
            (ProxyMode::ThreeFingerOwned, FrameAction::Suppress)
        );
    }

    #[test]
    fn broker_rejection_does_not_expose_a_two_finger_tap_before_grace() {
        assert_eq!(
            transition_on_rejection(ProxyMode::TwoFingerCandidate, 2),
            (
                ProxyMode::TwoFingerRejectedGrace,
                FrameAction::Suppress,
                true
            )
        );
        assert_eq!(
            transition_on_rejection(ProxyMode::TwoFingerOwned, 2),
            (ProxyMode::NativeUntilLift, FrameAction::Resume, false)
        );
    }

    #[test]
    fn rejected_candidate_stays_native_after_landing_grace_expires() {
        assert_eq!(
            transition_rejected_candidate(2, false),
            (ProxyMode::NativeUntilLift, FrameAction::Resume)
        );
        assert_eq!(
            transition_rejected_candidate(3, false),
            (ProxyMode::NativeUntilLift, FrameAction::Resume)
        );
    }

    #[test]
    fn rejected_candidate_preserves_native_lift_and_four_finger_frames() {
        for contacts in [1, 4] {
            assert_eq!(
                transition_rejected_candidate(contacts, true),
                (ProxyMode::NativeUntilLift, FrameAction::Resume),
                "{contacts} contacts"
            );
        }
        assert_eq!(
            transition_rejected_candidate(0, true),
            (ProxyMode::Native, FrameAction::Suppress)
        );
    }

    #[test]
    fn one_finger_remainder_is_reconstructed_after_every_owned_stream() {
        for mode in [
            ProxyMode::TwoFingerCandidate,
            ProxyMode::TwoFingerOwned,
            ProxyMode::ThreeFingerOwned,
        ] {
            assert_eq!(
                transition(mode, 1),
                (ProxyMode::NativeUntilLift, FrameAction::Resume),
                "{mode:?}"
            );
        }
    }

    #[test]
    fn native_forwarding_replaces_firmware_contact_keys() {
        let frame = [
            key(KeyCode::BTN_TOUCH, 1),
            key(KeyCode::BTN_TOOL_FINGER, 1),
            key(KeyCode::BTN_TOOL_TRIPLETAP, 1),
            key(KeyCode::BTN_LEFT, 1),
            abs(AbsoluteAxisCode::ABS_MT_POSITION_X, 123),
            InputEvent::new(
                EventType::SYNCHRONIZATION.0,
                SynchronizationCode::SYN_REPORT.0,
                0,
            ),
        ];

        let forwarded = native_frame_events(&frame);

        assert!(forwarded.iter().all(|event| !is_contact_key(event.code())));
        assert!(forwarded.iter().all(|event| {
            event.event_type() != EventType::SYNCHRONIZATION
                || event.code() != SynchronizationCode::SYN_REPORT.0
        }));
        assert!(forwarded.iter().any(|event| {
            event.event_type() == EventType::KEY
                && event.code() == KeyCode::BTN_LEFT.0
                && event.value() == 1
        }));
        assert!(forwarded.iter().any(|event| {
            event.event_type() == EventType::ABSOLUTE
                && event.code() == AbsoluteAxisCode::ABS_MT_POSITION_X.0
                && event.value() == 123
        }));
    }

    #[test]
    fn native_forwarding_releases_a_slot_before_a_repeated_tracking_id() {
        let mut active_slots = BTreeMap::from([(0, 10)]);
        let mut current_slot = 0;
        let frame = [
            abs(AbsoluteAxisCode::ABS_MT_TRACKING_ID, 11),
            abs(AbsoluteAxisCode::ABS_MT_POSITION_X, 300),
            abs(AbsoluteAxisCode::ABS_MT_POSITION_Y, 400),
        ];

        let forwarded =
            sanitized_native_frame_events(&frame, &mut active_slots, &mut current_slot, 0, 4);

        let tracking_ids = forwarded
            .iter()
            .filter(|event| event.code() == AbsoluteAxisCode::ABS_MT_TRACKING_ID.0)
            .map(InputEvent::value)
            .collect::<Vec<_>>();
        assert_eq!(tracking_ids, vec![-1, 11]);
        assert_eq!(active_slots, BTreeMap::from([(0, 11)]));
    }

    #[test]
    fn native_forwarding_moves_a_duplicate_tracking_id_between_slots_safely() {
        let mut active_slots = BTreeMap::from([(0, 10)]);
        let mut current_slot = 0;
        let frame = [
            abs(AbsoluteAxisCode::ABS_MT_SLOT, 1),
            abs(AbsoluteAxisCode::ABS_MT_TRACKING_ID, 10),
            abs(AbsoluteAxisCode::ABS_MT_POSITION_X, 300),
            abs(AbsoluteAxisCode::ABS_MT_POSITION_Y, 400),
        ];

        let forwarded =
            sanitized_native_frame_events(&frame, &mut active_slots, &mut current_slot, 0, 4);

        let slot_and_tracking = forwarded
            .iter()
            .filter(|event| {
                matches!(
                    AbsoluteAxisCode(event.code()),
                    AbsoluteAxisCode::ABS_MT_SLOT | AbsoluteAxisCode::ABS_MT_TRACKING_ID
                )
            })
            .map(|event| (event.code(), event.value()))
            .collect::<Vec<_>>();
        assert_eq!(
            slot_and_tracking,
            vec![
                (AbsoluteAxisCode::ABS_MT_SLOT.0, 1),
                (AbsoluteAxisCode::ABS_MT_SLOT.0, 0),
                (AbsoluteAxisCode::ABS_MT_TRACKING_ID.0, -1),
                (AbsoluteAxisCode::ABS_MT_SLOT.0, 1),
                (AbsoluteAxisCode::ABS_MT_TRACKING_ID.0, 10),
            ]
        );
        assert_eq!(active_slots, BTreeMap::from([(1, 10)]));
        assert_eq!(current_slot, 1);
    }

    #[test]
    fn explicit_full_lift_releases_virtual_slots_missing_from_the_physical_frame() {
        let frame = [
            key(KeyCode::BTN_TOUCH, 0),
            InputEvent::new(
                EventType::SYNCHRONIZATION.0,
                SynchronizationCode::SYN_REPORT.0,
                0,
            ),
        ];
        let mut active_slots = BTreeMap::from([(0, 10), (2, 12)]);
        let mut current_slot = 1;

        let events =
            sanitized_native_frame_events(&frame, &mut active_slots, &mut current_slot, 0, 4);

        let slot_and_tracking = events
            .iter()
            .filter(|event| {
                event.event_type() == EventType::ABSOLUTE
                    && matches!(
                        AbsoluteAxisCode(event.code()),
                        AbsoluteAxisCode::ABS_MT_SLOT | AbsoluteAxisCode::ABS_MT_TRACKING_ID
                    )
            })
            .map(|event| (event.code(), event.value()))
            .collect::<Vec<_>>();
        assert_eq!(
            slot_and_tracking,
            vec![
                (AbsoluteAxisCode::ABS_MT_SLOT.0, 0),
                (AbsoluteAxisCode::ABS_MT_TRACKING_ID.0, -1),
                (AbsoluteAxisCode::ABS_MT_SLOT.0, 2),
                (AbsoluteAxisCode::ABS_MT_TRACKING_ID.0, -1),
                (AbsoluteAxisCode::ABS_MT_SLOT.0, 1),
            ]
        );
        assert!(active_slots.is_empty());
        assert_eq!(current_slot, 1);
    }

    #[test]
    fn touching_frame_never_triggers_full_lift_slot_repair() {
        let frame = [key(KeyCode::BTN_TOUCH, 1)];
        let mut active_slots = BTreeMap::from([(0, 10)]);
        let mut current_slot = 0;

        let events =
            sanitized_native_frame_events(&frame, &mut active_slots, &mut current_slot, 0, 4);

        assert!(events.iter().all(|event| {
            event.code() != AbsoluteAxisCode::ABS_MT_TRACKING_ID.0 || event.value() >= 0
        }));
        assert_eq!(active_slots, BTreeMap::from([(0, 10)]));
        assert_eq!(current_slot, 0);
    }

    #[test]
    fn missing_or_superseded_touch_key_never_triggers_full_lift_slot_repair() {
        for frame in [
            Vec::new(),
            vec![key(KeyCode::BTN_TOOL_FINGER, 0)],
            vec![key(KeyCode::BTN_TOUCH, 0), key(KeyCode::BTN_TOUCH, 1)],
        ] {
            let mut active_slots = BTreeMap::from([(0, 10)]);
            let mut current_slot = 0;

            let events =
                sanitized_native_frame_events(&frame, &mut active_slots, &mut current_slot, 0, 4);

            assert!(events.iter().all(|event| {
                event.code() != AbsoluteAxisCode::ABS_MT_TRACKING_ID.0 || event.value() >= 0
            }));
            assert_eq!(active_slots, BTreeMap::from([(0, 10)]));
            assert_eq!(current_slot, 0);
        }
    }

    #[test]
    fn generated_frames_restore_the_physical_current_slot() {
        let event = slot_alignment_event(Some(3), 0, 5).expect("valid slot");
        assert_eq!(event.event_type(), EventType::ABSOLUTE);
        assert_eq!(event.code(), AbsoluteAxisCode::ABS_MT_SLOT.0);
        assert_eq!(event.value(), 3);
        assert!(slot_alignment_event(None, 0, 5).is_none());
        assert!(slot_alignment_event(Some(6), 0, 5).is_none());
    }

    #[test]
    fn resumed_contact_replays_every_tracked_multitouch_axis() {
        let contact = ProxyContact {
            slot: 2,
            tracking_id: 10,
            axes: vec![
                (AbsoluteAxisCode::ABS_MT_POSITION_X.0, 100),
                (AbsoluteAxisCode::ABS_MT_POSITION_Y.0, 200),
                (AbsoluteAxisCode::ABS_MT_TOOL_TYPE.0, 1),
                (AbsoluteAxisCode::ABS_MT_PRESSURE.0, 400),
            ],
        };

        let events = resumed_contact_events(&contact);

        assert_eq!(events[0].code(), AbsoluteAxisCode::ABS_MT_SLOT.0);
        assert_eq!(events[0].value(), 2);
        assert_eq!(events[1].code(), AbsoluteAxisCode::ABS_MT_TRACKING_ID.0);
        assert_eq!(events[1].value(), 10);
        assert!(events.iter().any(|event| {
            event.code() == AbsoluteAxisCode::ABS_MT_TOOL_TYPE.0 && event.value() == 1
        }));
        assert!(events.iter().any(|event| {
            event.code() == AbsoluteAxisCode::ABS_MT_PRESSURE.0 && event.value() == 400
        }));
    }

    #[test]
    fn two_finger_owned_and_native_replay_lanes_block_the_drag_engine() {
        assert!(allows_three_finger_drag(ProxyMode::Native));
        assert!(allows_three_finger_drag(ProxyMode::TwoFingerCandidate));
        assert!(allows_three_finger_drag(ProxyMode::ThreeFingerOwned));
        assert!(!allows_three_finger_drag(ProxyMode::TwoFingerRejectedGrace));
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
