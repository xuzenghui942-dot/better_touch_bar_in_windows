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
const MAX_BUFFERED_FRAMES: usize = 64;
const PROBE_DEADLINE: Duration = Duration::from_millis(16);
// Real GXTP5100 captures put the sequential 2 -> 3 landing phase below 20 ms.
// Standalone one-finger frames remain native. Keep only the continuation from
// the exact-two transition private for one bounded chord window, then either
// replay it continuously or release the visible prefix when ownership commits.
const CHORD_DEADLINE: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProxyMode {
    Native,
    PreflightPending,
    TwoFingerCandidate,
    TwoFingerOwned,
    NativeChordUntilLift,
    NativeUntilLift,
    NativeDragUntilLift,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameAction {
    Emit,
    Suppress,
    Buffer,
    Replay,
    ReplayAndEmit,
    Resume,
}

#[derive(Debug, Clone)]
struct BufferedFrame {
    events: Vec<InputEvent>,
    contacts: usize,
}

#[derive(Debug, Default)]
struct NativeFrameBuffer {
    frames: Vec<BufferedFrame>,
}

impl NativeFrameBuffer {
    fn push(&mut self, events: Vec<InputEvent>, contacts: usize) -> Result<(), BufferedFrame> {
        let frame = BufferedFrame { events, contacts };
        if self.frames.len() >= MAX_BUFFERED_FRAMES {
            return Err(frame);
        }
        self.frames.push(frame);
        Ok(())
    }

    fn take(&mut self) -> Vec<BufferedFrame> {
        std::mem::take(&mut self.frames)
    }

    fn clear(&mut self) {
        self.frames.clear();
    }

    fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct ProxyFrameOutcome {
    pub request_probe: bool,
    pub native_fallback: bool,
    pub stream_ended: bool,
}

/// Exclusive physical-touchpad owner used only while advanced two-finger
/// gestures are enabled.  Every input family not owned by this application is
/// copied to a virtual touchpad, so libinput keeps normal pointer, scrolling,
/// four-finger and hardware-button behavior.
pub struct TouchpadProxy {
    output: VirtualDevice,
    mode: ProxyMode,
    buffered_frames: NativeFrameBuffer,
    probe_deadline: Option<Instant>,
    chord_deadline: Option<Instant>,
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
            buffered_frames: NativeFrameBuffer::default(),
            probe_deadline: None,
            chord_deadline: None,
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
    ) -> io::Result<ProxyFrameOutcome> {
        self.handle_frame_at(frame, contacts, current_slot, global_axes, Instant::now())
    }

    fn handle_frame_at(
        &mut self,
        frame: &[InputEvent],
        contacts: &[ProxyContact],
        current_slot: Option<i32>,
        global_axes: &[(u16, i32)],
        now: Instant,
    ) -> io::Result<ProxyFrameOutcome> {
        self.observe_physical_buttons(frame);
        let previous = self.mode;
        let chord_is_open = self.chord_deadline.is_some_and(|deadline| now <= deadline);
        let (next, action) = if self.mode == ProxyMode::NativeChordUntilLift {
            transition_native_chord(contacts.len(), chord_is_open)
        } else {
            transition(self.mode, contacts.len())
        };
        let mut outcome = ProxyFrameOutcome::default();
        match action {
            FrameAction::Emit => self.emit_frame(frame, contacts.len())?,
            FrameAction::Suppress => {}
            FrameAction::Buffer => {
                if let Err(overflow) = self.buffered_frames.push(frame.to_vec(), contacts.len()) {
                    self.replay_buffered_frames()?;
                    self.emit_frame(&overflow.events, overflow.contacts)?;
                    self.enter_native_replay_mode(contacts.len(), now);
                    outcome.native_fallback = true;
                    outcome.stream_ended = contacts.is_empty();
                    return Ok(outcome);
                }
            }
            FrameAction::Replay => {
                self.replay_buffered_frames()?;
                outcome.native_fallback = !contacts.is_empty();
            }
            FrameAction::ReplayAndEmit => {
                self.replay_buffered_frames()?;
                self.emit_frame(frame, contacts.len())?;
                outcome.native_fallback = !contacts.is_empty();
            }
            FrameAction::Resume => self.resume_native(contacts, current_slot, global_axes)?,
        }
        self.mode = next;
        if previous == ProxyMode::Native && self.mode == ProxyMode::PreflightPending {
            self.probe_deadline = Some(now + PROBE_DEADLINE);
            self.chord_deadline = Some(now + CHORD_DEADLINE);
            outcome.request_probe = true;
        }
        if self.mode == ProxyMode::Native {
            self.clear_preflight();
            outcome.stream_ended = previous != ProxyMode::Native;
        } else if matches!(
            self.mode,
            ProxyMode::NativeUntilLift | ProxyMode::NativeDragUntilLift
        ) {
            self.clear_preflight();
        }
        Ok(outcome)
    }

    /// Lock a recognized two-finger gesture to its current physical contact
    /// stream. The recognizer calls this only after crossing its existing
    /// intent threshold, so a normal sequential three-finger landing can still
    /// replay its complete prefix to the native drag lane without delay.
    pub fn commit_two_finger_candidate(&mut self) -> io::Result<()> {
        let (next, release_native) = transition_on_two_finger_commit(self.mode);
        if release_native {
            // One-finger input is passed through immediately. Once the second
            // finger has produced a real advanced intent, terminate only that
            // already-visible native prefix before taking exclusive ownership.
            self.release_native_contacts(None)?;
            self.buffered_frames.clear();
            self.clear_preflight_deadlines();
        }
        self.mode = next;
        Ok(())
    }

    /// The cloned touchpad owns native/replayed streams independently from the
    /// basic drag engine. A replayed two-finger scroll or a committed advanced
    /// gesture must never be stolen by three-finger drag before every contact
    /// lifts.
    pub fn allows_three_finger_drag(&self) -> bool {
        allows_three_finger_drag(self.mode)
    }

    pub fn resolve_probe(
        &mut self,
        accepted: bool,
        contacts: usize,
    ) -> io::Result<ProxyFrameOutcome> {
        let (next, action) = transition_on_probe(self.mode, accepted, contacts);
        let mut outcome = ProxyFrameOutcome::default();
        match action {
            FrameAction::Buffer => {}
            FrameAction::Suppress => {}
            FrameAction::Replay => {
                self.replay_buffered_frames()?;
                outcome.native_fallback = contacts > 0;
            }
            FrameAction::Emit | FrameAction::ReplayAndEmit | FrameAction::Resume => {
                unreachable!("probe resolution cannot synthesize a partial native frame")
            }
        }
        self.mode = next;
        self.probe_deadline = None;
        if self.mode == ProxyMode::NativeChordUntilLift {
            self.buffered_frames.clear();
            self.probe_deadline = None;
        } else if matches!(
            self.mode,
            ProxyMode::Native | ProxyMode::NativeUntilLift | ProxyMode::NativeDragUntilLift
        ) {
            self.clear_preflight();
        }
        outcome.stream_ended = self.mode == ProxyMode::Native;
        Ok(outcome)
    }

    pub fn flush_preflight_if_due(
        &mut self,
        contacts: usize,
        begin_accepted: bool,
    ) -> io::Result<ProxyFrameOutcome> {
        self.flush_preflight_if_due_at(contacts, begin_accepted, Instant::now())
    }

    fn flush_preflight_if_due_at(
        &mut self,
        contacts: usize,
        begin_accepted: bool,
        now: Instant,
    ) -> io::Result<ProxyFrameOutcome> {
        let probe_timed_out = self.mode == ProxyMode::PreflightPending
            && self.probe_deadline.is_some_and(|deadline| now >= deadline);
        if self.mode == ProxyMode::NativeChordUntilLift
            && self.chord_deadline.is_some_and(|deadline| now >= deadline)
        {
            self.mode = native_fallback_mode(contacts);
            self.clear_preflight();
            return Ok(ProxyFrameOutcome {
                stream_ended: contacts == 0,
                ..ProxyFrameOutcome::default()
            });
        }
        let chord_timed_out = matches!(self.mode, ProxyMode::TwoFingerCandidate)
            && self.chord_deadline.is_some_and(|deadline| now >= deadline);
        match preflight_deadline_action(
            self.mode,
            contacts,
            begin_accepted,
            probe_timed_out,
            chord_timed_out,
        ) {
            PreflightDeadlineAction::None => return Ok(ProxyFrameOutcome::default()),
            PreflightDeadlineAction::Commit => {
                self.commit_two_finger_candidate()?;
                return Ok(ProxyFrameOutcome::default());
            }
            PreflightDeadlineAction::Fallback => {}
        }

        self.replay_buffered_frames()?;
        self.enter_native_replay_mode(contacts, now);
        Ok(ProxyFrameOutcome {
            native_fallback: contacts > 0,
            stream_ended: contacts == 0,
            ..ProxyFrameOutcome::default()
        })
    }

    pub fn release_all(&mut self) -> io::Result<()> {
        self.release_native_contacts(None)?;
        self.mode = ProxyMode::Native;
        self.clear_preflight();
        Ok(())
    }

    pub fn has_accepted_preflight(&self) -> bool {
        self.mode == ProxyMode::TwoFingerCandidate
    }

    pub fn advanced_two_finger_lane(&self) -> bool {
        matches!(
            self.mode,
            ProxyMode::TwoFingerCandidate | ProxyMode::TwoFingerOwned
        )
    }

    pub fn fallback_to_native(&mut self, contacts: usize) -> io::Result<ProxyFrameOutcome> {
        if self.buffered_frames.is_empty() {
            return Ok(ProxyFrameOutcome::default());
        }
        self.replay_buffered_frames()?;
        self.enter_native_replay_mode(contacts, Instant::now());
        Ok(ProxyFrameOutcome {
            native_fallback: contacts > 0,
            stream_ended: contacts == 0,
            ..ProxyFrameOutcome::default()
        })
    }

    fn replay_buffered_frames(&mut self) -> io::Result<()> {
        let frames = self.buffered_frames.take();
        for frame in frames {
            self.emit_frame(&frame.events, frame.contacts)?;
        }
        Ok(())
    }

    fn native_replay_mode(&self, contacts: usize, now: Instant) -> ProxyMode {
        if self.chord_deadline.is_some_and(|deadline| now <= deadline) {
            native_chord_mode(contacts)
        } else {
            native_fallback_mode(contacts)
        }
    }

    fn enter_native_replay_mode(&mut self, contacts: usize, now: Instant) {
        self.mode = self.native_replay_mode(contacts, now);
        self.buffered_frames.clear();
        self.probe_deadline = None;
        if self.mode != ProxyMode::NativeChordUntilLift {
            self.chord_deadline = None;
        }
    }

    fn clear_preflight_deadlines(&mut self) {
        self.probe_deadline = None;
        self.chord_deadline = None;
    }

    fn clear_preflight(&mut self) {
        self.buffered_frames.clear();
        self.clear_preflight_deadlines();
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
        self.clear_preflight();
        Ok(())
    }

    fn release_native_contacts(&mut self, current_slot: Option<i32>) -> io::Result<()> {
        let mut events = Vec::new();
        // Type-B multitouch slots are finite. Releasing every advertised slot
        // is safe during shutdown and owned-stream recovery, including after
        // firmware omitted an individual slot release.
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
            0 | 1 => (ProxyMode::Native, FrameAction::Emit),
            2 => (ProxyMode::PreflightPending, FrameAction::Buffer),
            3 => (ProxyMode::NativeDragUntilLift, FrameAction::Emit),
            _ => (ProxyMode::NativeUntilLift, FrameAction::Emit),
        },
        ProxyMode::PreflightPending => match contacts {
            0 => (ProxyMode::Native, FrameAction::ReplayAndEmit),
            1 => (ProxyMode::NativeUntilLift, FrameAction::ReplayAndEmit),
            2 => (ProxyMode::PreflightPending, FrameAction::Buffer),
            3 => (ProxyMode::NativeDragUntilLift, FrameAction::ReplayAndEmit),
            _ => (ProxyMode::NativeUntilLift, FrameAction::ReplayAndEmit),
        },
        ProxyMode::TwoFingerCandidate => match contacts {
            0 => (ProxyMode::Native, FrameAction::ReplayAndEmit),
            1 => (ProxyMode::NativeUntilLift, FrameAction::ReplayAndEmit),
            2 => (ProxyMode::TwoFingerCandidate, FrameAction::Buffer),
            3 => (ProxyMode::NativeDragUntilLift, FrameAction::ReplayAndEmit),
            _ => (ProxyMode::NativeUntilLift, FrameAction::ReplayAndEmit),
        },
        ProxyMode::TwoFingerOwned => match contacts {
            0 => (ProxyMode::Native, FrameAction::Suppress),
            1 => (ProxyMode::NativeUntilLift, FrameAction::Resume),
            4.. => (ProxyMode::NativeUntilLift, FrameAction::Resume),
            _ => (ProxyMode::TwoFingerOwned, FrameAction::Suppress),
        },
        ProxyMode::NativeChordUntilLift => {
            unreachable!("native chord transitions require their landing deadline")
        }
        ProxyMode::NativeUntilLift if contacts == 0 => (ProxyMode::Native, FrameAction::Emit),
        ProxyMode::NativeUntilLift => (ProxyMode::NativeUntilLift, FrameAction::Emit),
        ProxyMode::NativeDragUntilLift if contacts == 0 => (ProxyMode::Native, FrameAction::Emit),
        ProxyMode::NativeDragUntilLift if contacts >= 4 => {
            (ProxyMode::NativeUntilLift, FrameAction::Emit)
        }
        ProxyMode::NativeDragUntilLift => (ProxyMode::NativeDragUntilLift, FrameAction::Emit),
    }
}

fn transition_native_chord(contacts: usize, chord_is_open: bool) -> (ProxyMode, FrameAction) {
    match contacts {
        0 => (ProxyMode::Native, FrameAction::Emit),
        3 if chord_is_open => (ProxyMode::NativeDragUntilLift, FrameAction::Emit),
        1 | 2 if chord_is_open => (ProxyMode::NativeChordUntilLift, FrameAction::Emit),
        _ => (ProxyMode::NativeUntilLift, FrameAction::Emit),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreflightDeadlineAction {
    None,
    Commit,
    Fallback,
}

fn preflight_deadline_action(
    mode: ProxyMode,
    contacts: usize,
    begin_accepted: bool,
    probe_timed_out: bool,
    chord_timed_out: bool,
) -> PreflightDeadlineAction {
    if probe_timed_out && mode == ProxyMode::PreflightPending {
        return PreflightDeadlineAction::Fallback;
    }
    if !chord_timed_out || !matches!(mode, ProxyMode::TwoFingerCandidate) {
        return PreflightDeadlineAction::None;
    }
    if mode == ProxyMode::TwoFingerCandidate && contacts == 2 && begin_accepted {
        PreflightDeadlineAction::Commit
    } else {
        PreflightDeadlineAction::Fallback
    }
}

fn transition_on_probe(
    mode: ProxyMode,
    accepted: bool,
    contacts: usize,
) -> (ProxyMode, FrameAction) {
    if mode != ProxyMode::PreflightPending {
        return (mode, FrameAction::Suppress);
    }
    if accepted {
        match contacts {
            0 => (ProxyMode::Native, FrameAction::Replay),
            1 => (ProxyMode::NativeUntilLift, FrameAction::Replay),
            2 => (ProxyMode::TwoFingerCandidate, FrameAction::Buffer),
            3 => (ProxyMode::NativeDragUntilLift, FrameAction::Replay),
            _ => (ProxyMode::NativeUntilLift, FrameAction::Replay),
        }
    } else {
        (native_chord_mode(contacts), FrameAction::Replay)
    }
}

fn native_chord_mode(contacts: usize) -> ProxyMode {
    match contacts {
        0 => ProxyMode::Native,
        1 | 2 => ProxyMode::NativeChordUntilLift,
        3 => ProxyMode::NativeDragUntilLift,
        _ => ProxyMode::NativeUntilLift,
    }
}

fn native_fallback_mode(contacts: usize) -> ProxyMode {
    match contacts {
        0 => ProxyMode::Native,
        3 => ProxyMode::NativeDragUntilLift,
        _ => ProxyMode::NativeUntilLift,
    }
}

fn transition_on_two_finger_commit(mode: ProxyMode) -> (ProxyMode, bool) {
    if mode == ProxyMode::TwoFingerCandidate {
        (ProxyMode::TwoFingerOwned, true)
    } else {
        (mode, false)
    }
}

fn allows_three_finger_drag(mode: ProxyMode) -> bool {
    !matches!(
        mode,
        ProxyMode::TwoFingerOwned | ProxyMode::NativeChordUntilLift | ProxyMode::NativeUntilLift
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
    fn exact_one_finger_stream_is_immediate_native_passthrough() {
        assert_eq!(
            transition(ProxyMode::Native, 1),
            (ProxyMode::Native, FrameAction::Emit),
            "a standalone one-finger frame must never enter titlebar preflight",
        );
        assert_eq!(
            transition(ProxyMode::Native, 0),
            (ProxyMode::Native, FrameAction::Emit),
            "the matching lift/click release must remain native too",
        );
    }

    #[test]
    fn first_contact_is_native_and_preflight_starts_only_on_exactly_two() {
        assert_eq!(
            transition(ProxyMode::Native, 1),
            (ProxyMode::Native, FrameAction::Emit)
        );
        assert_eq!(
            transition(ProxyMode::Native, 2),
            (ProxyMode::PreflightPending, FrameAction::Buffer)
        );
        assert_eq!(
            transition_on_probe(ProxyMode::PreflightPending, true, 2),
            (ProxyMode::TwoFingerCandidate, FrameAction::Buffer)
        );
        assert_eq!(
            transition_on_probe(ProxyMode::PreflightPending, false, 2),
            (ProxyMode::NativeChordUntilLift, FrameAction::Replay)
        );
    }

    #[test]
    fn a_candidate_boundary_frame_is_emitted_after_its_buffered_prefix() {
        assert_eq!(
            transition(ProxyMode::PreflightPending, 0),
            (ProxyMode::Native, FrameAction::ReplayAndEmit),
            "the physical lift that ends preflight must not be dropped",
        );
        assert_eq!(
            transition(ProxyMode::PreflightPending, 3),
            (ProxyMode::NativeDragUntilLift, FrameAction::ReplayAndEmit),
            "the third-contact frame must follow the replayed two-finger prefix",
        );
    }

    #[test]
    fn a_two_to_one_transition_immediately_returns_to_native_passthrough() {
        assert_eq!(
            transition(ProxyMode::PreflightPending, 1),
            (ProxyMode::NativeUntilLift, FrameAction::ReplayAndEmit),
            "one remaining finger must not stay inside advanced preflight",
        );
        assert_eq!(
            transition(ProxyMode::TwoFingerCandidate, 1),
            (ProxyMode::NativeUntilLift, FrameAction::ReplayAndEmit),
            "one remaining finger must leave an accepted candidate immediately",
        );
    }

    #[test]
    fn rejected_native_probe_keeps_only_the_bounded_three_finger_landing_window() {
        assert_eq!(
            transition_on_probe(ProxyMode::PreflightPending, false, 1),
            (ProxyMode::NativeChordUntilLift, FrameAction::Replay)
        );
        assert_eq!(
            transition_native_chord(3, true),
            (ProxyMode::NativeDragUntilLift, FrameAction::Emit)
        );
        assert_eq!(
            transition_native_chord(2, false),
            (ProxyMode::NativeUntilLift, FrameAction::Emit)
        );
        assert!(!allows_three_finger_drag(ProxyMode::NativeChordUntilLift));
    }

    #[test]
    fn third_and_fourth_contacts_replay_the_complete_native_prefix() {
        assert_eq!(
            transition(ProxyMode::PreflightPending, 3),
            (ProxyMode::NativeDragUntilLift, FrameAction::ReplayAndEmit)
        );
        assert_eq!(
            transition(ProxyMode::PreflightPending, 4),
            (ProxyMode::NativeUntilLift, FrameAction::ReplayAndEmit)
        );
    }

    #[test]
    fn preflight_buffer_is_bounded_and_drains_exactly_once() {
        let mut buffer = NativeFrameBuffer::default();
        for index in 0..MAX_BUFFERED_FRAMES {
            buffer
                .push(
                    vec![abs(AbsoluteAxisCode::ABS_MT_POSITION_X, index as i32)],
                    2,
                )
                .unwrap();
        }
        let overflow = buffer
            .push(vec![abs(AbsoluteAxisCode::ABS_MT_POSITION_X, 999)], 2)
            .expect_err("the sixty-fifth frame must fail open");
        assert_eq!(overflow.events[0].value(), 999);
        assert_eq!(buffer.take().len(), MAX_BUFFERED_FRAMES);
        assert!(buffer.take().is_empty(), "buffered frames replayed twice");
    }

    #[test]
    fn buffered_one_to_two_prefix_contains_no_synthetic_lift_or_left_button() {
        let mut buffer = NativeFrameBuffer::default();
        buffer
            .push(
                vec![
                    abs(AbsoluteAxisCode::ABS_MT_SLOT, 0),
                    abs(AbsoluteAxisCode::ABS_MT_TRACKING_ID, 10),
                    key(KeyCode::BTN_TOUCH, 1),
                ],
                1,
            )
            .unwrap();
        buffer
            .push(
                vec![
                    abs(AbsoluteAxisCode::ABS_MT_SLOT, 1),
                    abs(AbsoluteAxisCode::ABS_MT_TRACKING_ID, 11),
                    key(KeyCode::BTN_TOUCH, 1),
                ],
                2,
            )
            .unwrap();

        let events = buffer
            .take()
            .into_iter()
            .flat_map(|frame| frame.events)
            .collect::<Vec<_>>();
        assert!(events.iter().all(|event| {
            !(event.event_type() == EventType::KEY
                && event.code() == KeyCode::BTN_TOUCH.0
                && event.value() == 0)
        }));
        assert!(events
            .iter()
            .all(|event| event.code() != KeyCode::BTN_LEFT.0));
    }

    #[test]
    fn probe_and_chord_deadlines_fail_open_except_for_an_accepted_candidate() {
        assert_eq!(
            preflight_deadline_action(ProxyMode::PreflightPending, 2, false, true, false),
            PreflightDeadlineAction::Fallback
        );
        assert_eq!(
            preflight_deadline_action(ProxyMode::TwoFingerCandidate, 2, false, false, true),
            PreflightDeadlineAction::Fallback
        );
        assert_eq!(
            preflight_deadline_action(ProxyMode::TwoFingerCandidate, 2, true, false, true),
            PreflightDeadlineAction::Commit
        );
        assert_eq!(
            preflight_deadline_action(ProxyMode::Native, 0, false, false, false),
            PreflightDeadlineAction::None
        );
        assert_eq!(
            preflight_deadline_action(ProxyMode::TwoFingerCandidate, 1, true, false, true),
            PreflightDeadlineAction::Fallback
        );
    }

    #[test]
    fn every_contact_transition_has_an_explicit_owner() {
        let cases = [
            (ProxyMode::Native, 0, ProxyMode::Native, FrameAction::Emit),
            (ProxyMode::Native, 1, ProxyMode::Native, FrameAction::Emit),
            (
                ProxyMode::Native,
                3,
                ProxyMode::NativeDragUntilLift,
                FrameAction::Emit,
            ),
            (
                ProxyMode::PreflightPending,
                0,
                ProxyMode::Native,
                FrameAction::ReplayAndEmit,
            ),
            (
                ProxyMode::PreflightPending,
                1,
                ProxyMode::NativeUntilLift,
                FrameAction::ReplayAndEmit,
            ),
            (
                ProxyMode::PreflightPending,
                3,
                ProxyMode::NativeDragUntilLift,
                FrameAction::ReplayAndEmit,
            ),
            (
                ProxyMode::TwoFingerCandidate,
                0,
                ProxyMode::Native,
                FrameAction::ReplayAndEmit,
            ),
            (
                ProxyMode::TwoFingerCandidate,
                1,
                ProxyMode::NativeUntilLift,
                FrameAction::ReplayAndEmit,
            ),
            (
                ProxyMode::TwoFingerCandidate,
                2,
                ProxyMode::TwoFingerCandidate,
                FrameAction::Buffer,
            ),
            (
                ProxyMode::TwoFingerCandidate,
                3,
                ProxyMode::NativeDragUntilLift,
                FrameAction::ReplayAndEmit,
            ),
            (
                ProxyMode::TwoFingerOwned,
                0,
                ProxyMode::Native,
                FrameAction::Suppress,
            ),
            (
                ProxyMode::TwoFingerOwned,
                1,
                ProxyMode::NativeUntilLift,
                FrameAction::Resume,
            ),
            (
                ProxyMode::TwoFingerOwned,
                2,
                ProxyMode::TwoFingerOwned,
                FrameAction::Suppress,
            ),
            (
                ProxyMode::TwoFingerOwned,
                4,
                ProxyMode::NativeUntilLift,
                FrameAction::Resume,
            ),
            (
                ProxyMode::NativeUntilLift,
                0,
                ProxyMode::Native,
                FrameAction::Emit,
            ),
            (
                ProxyMode::NativeUntilLift,
                2,
                ProxyMode::NativeUntilLift,
                FrameAction::Emit,
            ),
            (
                ProxyMode::NativeDragUntilLift,
                0,
                ProxyMode::Native,
                FrameAction::Emit,
            ),
            (
                ProxyMode::NativeDragUntilLift,
                3,
                ProxyMode::NativeDragUntilLift,
                FrameAction::Emit,
            ),
        ];
        for (mode, contacts, expected_mode, expected_action) in cases {
            assert_eq!(transition(mode, contacts), (expected_mode, expected_action));
        }
    }

    #[test]
    fn probe_resolution_covers_accept_reject_and_stale_results() {
        assert_eq!(
            transition_on_probe(ProxyMode::Native, true, 2),
            (ProxyMode::Native, FrameAction::Suppress)
        );
        for (contacts, expected) in [
            (0, ProxyMode::Native),
            (1, ProxyMode::NativeUntilLift),
            (2, ProxyMode::TwoFingerCandidate),
            (3, ProxyMode::NativeDragUntilLift),
        ] {
            let action = if contacts == 2 {
                FrameAction::Buffer
            } else {
                FrameAction::Replay
            };
            assert_eq!(
                transition_on_probe(ProxyMode::PreflightPending, true, contacts),
                (expected, action)
            );
        }
        for (contacts, expected) in [
            (0, ProxyMode::Native),
            (1, ProxyMode::NativeChordUntilLift),
            (2, ProxyMode::NativeChordUntilLift),
            (3, ProxyMode::NativeDragUntilLift),
            (4, ProxyMode::NativeUntilLift),
        ] {
            assert_eq!(
                transition_on_probe(ProxyMode::PreflightPending, false, contacts),
                (expected, FrameAction::Replay)
            );
        }
        assert_eq!(native_fallback_mode(1), ProxyMode::NativeUntilLift);
        assert_eq!(native_fallback_mode(2), ProxyMode::NativeUntilLift);
        assert_eq!(native_fallback_mode(3), ProxyMode::NativeDragUntilLift);
        assert_eq!(native_fallback_mode(4), ProxyMode::NativeUntilLift);
    }

    #[test]
    fn clearing_a_buffer_removes_every_pending_frame() {
        let mut buffer = NativeFrameBuffer::default();
        buffer.push(Vec::new(), 1).unwrap();
        assert!(!buffer.is_empty());
        buffer.clear();
        assert!(buffer.is_empty());
    }

    #[test]
    fn committed_two_finger_stream_never_transfers_to_three_before_lift() {
        assert_eq!(
            transition_on_two_finger_commit(ProxyMode::TwoFingerCandidate),
            (ProxyMode::TwoFingerOwned, true),
            "committing must release the one-finger state already visible to libinput",
        );
        for unchanged in [
            ProxyMode::Native,
            ProxyMode::PreflightPending,
            ProxyMode::TwoFingerOwned,
            ProxyMode::NativeChordUntilLift,
            ProxyMode::NativeUntilLift,
            ProxyMode::NativeDragUntilLift,
        ] {
            assert_eq!(
                transition_on_two_finger_commit(unchanged),
                (unchanged, false)
            );
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
    fn rejected_probe_replays_once_then_closes_its_native_chord_window() {
        assert_eq!(
            transition_on_probe(ProxyMode::PreflightPending, false, 2),
            (ProxyMode::NativeChordUntilLift, FrameAction::Replay)
        );
        assert_eq!(
            transition_native_chord(2, false),
            (ProxyMode::NativeUntilLift, FrameAction::Emit)
        );
        assert_eq!(
            transition(ProxyMode::NativeUntilLift, 0),
            (ProxyMode::Native, FrameAction::Emit)
        );
    }

    #[test]
    fn accepted_probe_requires_exactly_two_contacts() {
        assert_eq!(
            transition_on_probe(ProxyMode::PreflightPending, true, 1),
            (ProxyMode::NativeUntilLift, FrameAction::Replay)
        );
        assert_eq!(
            transition_on_probe(ProxyMode::PreflightPending, true, 2),
            (ProxyMode::TwoFingerCandidate, FrameAction::Buffer)
        );
    }

    #[test]
    fn candidate_lift_replays_a_complete_native_stream() {
        assert_eq!(
            transition(ProxyMode::TwoFingerCandidate, 0),
            (ProxyMode::Native, FrameAction::ReplayAndEmit)
        );
    }

    #[test]
    fn one_finger_remainder_replays_a_candidate_but_reconstructs_owned_input() {
        assert_eq!(
            transition(ProxyMode::TwoFingerCandidate, 1),
            (ProxyMode::NativeUntilLift, FrameAction::ReplayAndEmit)
        );
        assert_eq!(
            transition(ProxyMode::TwoFingerOwned, 1),
            (ProxyMode::NativeUntilLift, FrameAction::Resume)
        );
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
        assert!(allows_three_finger_drag(ProxyMode::PreflightPending));
        assert!(allows_three_finger_drag(ProxyMode::TwoFingerCandidate));
        assert!(allows_three_finger_drag(ProxyMode::NativeDragUntilLift));
        assert!(!allows_three_finger_drag(ProxyMode::TwoFingerOwned));
        assert!(!allows_three_finger_drag(ProxyMode::NativeUntilLift));
    }

    #[test]
    fn four_fingers_resume_native_and_stay_native_until_lift() {
        assert_eq!(
            transition(ProxyMode::TwoFingerCandidate, 4),
            (ProxyMode::NativeUntilLift, FrameAction::ReplayAndEmit)
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
        assert_eq!(
            transition(ProxyMode::NativeDragUntilLift, 4),
            (ProxyMode::NativeUntilLift, FrameAction::Emit)
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
