use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    fs::OpenOptions,
    io,
    os::{fd::OwnedFd, unix::fs::OpenOptionsExt},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Sender},
        Arc, RwLock,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use better_touch_advanced_gestures::gesture::{GestureEvent, SwipeDirection};
use evdev::{
    raw_stream::RawDevice, AbsoluteAxisCode, EventType, InputEvent, PropType, SynchronizationCode,
};
use serde::{Deserialize, Serialize};

use crate::{
    advanced::{AdvancedCapabilities, AdvancedRuntime, AdvancedRuntimeStatus, AxisRanges},
    gesture::{Contact, DragEngine, TimerDirective},
    logging::RingLogger,
    settings::AppSettings,
};

use super::{
    advanced_transport::{
        AdvancedConfigEnvelope, AdvancedTransport, GestureEventDto, SubmitError,
        TransportAvailability, TransportNotice,
    },
    mouse_output::MouseOutput,
    stable_device_id,
    touchpad_proxy::{ProxyContact, TouchpadProxy},
};

const POLL_INTERVAL: Duration = Duration::from_millis(4);
const PREVIEW_INTERVAL: Duration = Duration::from_millis(50);
const ADVANCED_FRAME_INTERVAL: Duration = Duration::from_millis(16);
const RESCAN_INTERVAL: Duration = Duration::from_secs(2);
const OUTPUT_RETRY_INTERVAL: Duration = Duration::from_secs(2);
const NORMALIZED_AXIS_MAX: i32 = 32_767;
const ADVANCED_UNAVAILABLE: &str =
    "GNOME 扩展尚未完成进阶手势能力握手；原生两指和四指手势保持不变。";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceDescriptor {
    pub id: String,
    pub vendor_id: String,
    pub product_id: String,
}

impl DeviceDescriptor {
    pub fn label(&self) -> String {
        format!("{}({}:{})", self.id, self.product_id, self.vendor_id)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TouchpadRuntimeStatus {
    pub initialized: bool,
    pub touchpad_exists: bool,
    pub receiver_installed: bool,
    pub devices: Vec<DeviceDescriptor>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContactPreview {
    pub device: DeviceDescriptor,
    pub contacts: Vec<Contact>,
    pub event_interval_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", content = "payload", rename_all = "snake_case")]
pub enum BackendEvent {
    Status(TouchpadRuntimeStatus),
    Contacts(ContactPreview),
    Advanced(AdvancedRuntimeStatus),
    Error(String),
}

pub struct InputService {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl InputService {
    pub fn start(
        settings: Arc<RwLock<AppSettings>>,
        logger: RingLogger,
        events: Sender<BackendEvent>,
    ) -> io::Result<Self> {
        Self::start_with_advanced(
            settings,
            Arc::new(RwLock::new(
                better_touch_advanced_gestures::config::AdvancedConfig::default(),
            )),
            logger,
            events,
        )
    }

    pub fn start_with_advanced(
        settings: Arc<RwLock<AppSettings>>,
        advanced_settings: Arc<RwLock<better_touch_advanced_gestures::config::AdvancedConfig>>,
        logger: RingLogger,
        events: Sender<BackendEvent>,
    ) -> io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("ThreeFingerDrag evdev input".into())
            .spawn(move || {
                run_input_loop(
                    settings,
                    advanced_settings,
                    logger,
                    events,
                    worker_stop,
                    ready_sender,
                )
            })?;

        match ready_receiver.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Self {
                stop,
                thread: Some(worker),
            }),
            Ok(Err(error)) => {
                stop.store(true, Ordering::Release);
                let _ = worker.join();
                Err(error)
            }
            Err(error) => {
                stop.store(true, Ordering::Release);
                let _ = worker.join();
                Err(io::Error::new(io::ErrorKind::TimedOut, error))
            }
        }
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.thread.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for InputService {
    fn drop(&mut self) {
        self.stop();
    }
}

struct TouchpadDevice {
    path: PathBuf,
    device: RawDevice,
    descriptor: DeviceDescriptor,
    source_key: String,
    ranges: AxisRanges,
    contacts: MtSlotTracker,
    drag: DragEngine,
    advanced: AdvancedRuntime,
    advanced_session: Option<AdvancedSession>,
    advanced_gate: AdvancedContactGate,
    touchpad_proxy: Option<TouchpadProxy>,
    pending_input_frame: Vec<InputEvent>,
    last_advanced_frame: Option<Instant>,
    last_advanced_contact_count: usize,
    release_deadline: Option<Instant>,
    last_preview: Instant,
    last_frame: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdvancedContactDisposition {
    Process,
    ProcessAfterReset,
    CancelAndSuppress,
    IgnoreUntilLift,
}

#[derive(Debug, Default)]
struct AdvancedContactGate {
    suppressed_until_lift: bool,
    rearm_five_after_four: bool,
}

impl AdvancedContactGate {
    fn observe(
        &mut self,
        contacts: usize,
        five_finger_allowed: bool,
    ) -> AdvancedContactDisposition {
        if contacts == 0 {
            self.suppressed_until_lift = false;
            self.rearm_five_after_four = false;
            return AdvancedContactDisposition::Process;
        }
        if contacts == 3 {
            self.suppress();
            return AdvancedContactDisposition::CancelAndSuppress;
        }
        if contacts == 4 {
            // Raw contacts normally arrive 1/2/3/4/5 before Clutter begins a
            // five-finger gesture. Permit a five-finger candidate only when
            // the broker explicitly supports that arbitration. The broker is
            // authoritative: if it already observed a compositor four-finger
            // BEGIN/UPDATE it rejects Begin and this stream stays suppressed.
            self.suppressed_until_lift = true;
            self.rearm_five_after_four = five_finger_allowed;
            return AdvancedContactDisposition::CancelAndSuppress;
        }
        if contacts >= 5 && !five_finger_allowed {
            self.suppress();
            return AdvancedContactDisposition::CancelAndSuppress;
        }
        if contacts >= 5 && self.rearm_five_after_four {
            self.suppressed_until_lift = false;
            self.rearm_five_after_four = false;
            return AdvancedContactDisposition::ProcessAfterReset;
        }
        if self.suppressed_until_lift {
            AdvancedContactDisposition::IgnoreUntilLift
        } else {
            AdvancedContactDisposition::Process
        }
    }

    fn suppress(&mut self) {
        self.suppressed_until_lift = true;
        self.rearm_five_after_four = false;
    }

    fn suppress_if_contacts(&mut self, contacts_present: bool) {
        if contacts_present {
            self.suppress();
        } else {
            self.suppressed_until_lift = false;
            self.rearm_five_after_four = false;
        }
    }
}

fn cancel_advanced_runtime_state(
    advanced: &mut AdvancedRuntime,
    gate: &mut AdvancedContactGate,
    contacts_present: bool,
    ranges: AxisRanges,
    timestamp_ms: u64,
    replacement_config: Option<better_touch_advanced_gestures::config::AdvancedConfig>,
) {
    let _ = advanced.cancel();
    if let Some(config) = replacement_config {
        advanced.set_config(config);
    }
    gate.suppress_if_contacts(contacts_present);
    if !contacts_present && advanced.config().enabled && advanced.config().gestures_enabled {
        // GestureEngine::cancel intentionally latches suppression until an empty
        // frame. A broker notice can arrive after the physical lift frame, so
        // settle that latch here instead of swallowing the next gesture.
        let _ = advanced.process(&[], ranges, timestamp_ms as i64);
    }
}

fn cancel_device_advanced_runtime(
    state: &mut TouchpadDevice,
    replacement_config: Option<better_touch_advanced_gestures::config::AdvancedConfig>,
) {
    let contacts_present = !state.contacts.contacts().is_empty();
    cancel_advanced_runtime_state(
        &mut state.advanced,
        &mut state.advanced_gate,
        contacts_present,
        state.ranges,
        monotonic_millis(),
        replacement_config,
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdvancedSessionPhase {
    AwaitingBegin,
    Accepted,
    AwaitingTerminal,
}

#[derive(Debug)]
struct AdvancedSession {
    id: String,
    next_sequence: u64,
    phase: AdvancedSessionPhase,
}

impl AdvancedSession {
    fn new(id: String) -> Self {
        Self {
            id,
            next_sequence: 1,
            phase: AdvancedSessionPhase::AwaitingBegin,
        }
    }

    fn take_sequence(&mut self) -> u64 {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        sequence
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct BrokerModifiers {
    thirds: bool,
    monitor: bool,
    escape: bool,
}

struct AdvancedBrokerState {
    transport: AdvancedTransport,
    availability: TransportAvailability,
    modifiers: BrokerModifiers,
    session_counter: u64,
    active_session: Option<String>,
    last_published: Option<AdvancedRuntimeStatus>,
    last_status_publish: Instant,
    configured_json: Option<String>,
    configuring_json: Option<String>,
    pending_reconfigure: bool,
}

impl TouchpadDevice {
    fn source_id(&self) -> &str {
        &self.source_key
    }
}

#[derive(Default)]
struct DeviceScan {
    readable: Vec<TouchpadDevice>,
    event_paths: BTreeSet<PathBuf>,
    denied_paths: BTreeSet<PathBuf>,
}

fn run_input_loop(
    settings: Arc<RwLock<AppSettings>>,
    advanced_settings: Arc<RwLock<better_touch_advanced_gestures::config::AdvancedConfig>>,
    logger: RingLogger,
    events: Sender<BackendEvent>,
    stop: Arc<AtomicBool>,
    ready: mpsc::SyncSender<io::Result<()>>,
) {
    let initial_advanced_config = advanced_settings_snapshot(&advanced_settings);
    let mut scan = match discover_touchpads(&initial_advanced_config) {
        Ok(scan) if !scan.readable.is_empty() => scan,
        Ok(_) => {
            let error = io::Error::new(
                io::ErrorKind::NotFound,
                "没有检测到支持多点触控的 Linux 触摸板。",
            );
            let _ = ready.send(Err(error));
            return;
        }
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };

    let mut devices = std::mem::take(&mut scan.readable);
    let mut mouse = match MouseOutput::create() {
        Ok(mouse) => mouse,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let transport = match AdvancedTransport::start(logger.clone()) {
        Ok(transport) => transport,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let mut advanced_broker = AdvancedBrokerState {
        transport,
        availability: TransportAvailability::Connecting,
        modifiers: BrokerModifiers::default(),
        session_counter: 0,
        active_session: None,
        last_published: None,
        last_status_publish: Instant::now() - PREVIEW_INTERVAL,
        configured_json: None,
        configuring_json: None,
        pending_reconfigure: false,
    };

    publish_status(&events, &devices, mouse.is_available());
    publish_advanced_status(&events, &devices, &mut advanced_broker);
    let _ = ready.send(Ok(()));

    let mut last_rescan = Instant::now();
    let mut last_output_retry = Instant::now();
    while !stop.load(Ordering::Acquire) {
        drain_transport_notices(&mut devices, &mut advanced_broker, &events, &logger);
        sync_broker_configuration(
            &advanced_settings,
            &mut devices,
            &mut advanced_broker,
            &events,
            &logger,
        );
        sync_touchpad_proxies(
            &advanced_settings,
            &mut devices,
            &mut advanced_broker,
            &events,
            &logger,
        );
        let mut lost = BTreeSet::new();
        for state in &mut devices {
            match process_available_events(
                state,
                &settings,
                &advanced_settings,
                &mut advanced_broker,
                &mut mouse,
                &logger,
                &events,
            ) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) if is_disconnected_error(&error) => {
                    logger.record(format!(
                        "Touchpad {} disconnected while reading: {error}",
                        state.path.display()
                    ));
                    lost.insert(state.path.clone());
                }
                Err(error) => {
                    logger.record(format!("Unable to read {}: {error}", state.path.display()));
                }
            }
            release_if_due(state, &settings, &mut mouse, &logger);
        }
        publish_advanced_status(&events, &devices, &mut advanced_broker);

        if !lost.is_empty() {
            remove_devices(
                &mut devices,
                &lost,
                &settings,
                &mut advanced_broker,
                &mut mouse,
                &logger,
            );
            publish_status(&events, &devices, mouse.is_available());
            publish_advanced_status(&events, &devices, &mut advanced_broker);
        }

        if let Some(message) = mouse.take_failure() {
            for state in &mut devices {
                cancel_advanced_session(state, &mut advanced_broker, "virtual_mouse_failed");
                cancel_device_advanced_runtime(state, None);
            }
            let _ = events.send(BackendEvent::Error(message));
            publish_status(&events, &devices, mouse.is_available());
            last_output_retry = Instant::now();
        }
        if !mouse.is_available() && last_output_retry.elapsed() >= OUTPUT_RETRY_INTERVAL {
            match mouse.reconnect() {
                Ok(true) => {
                    // DragEngine may still consider a gesture active, but the
                    // recreated uinput device owns no buttons. Reset every
                    // engine so the next stable gesture starts a clean drag.
                    for state in &mut devices {
                        state.drag = DragEngine::default();
                        state.contacts.clear_for_new_contacts();
                        state.release_deadline = None;
                        state.last_frame = None;
                    }
                    publish_status(&events, &devices, mouse.is_available());
                }
                Ok(false) => {}
                Err(error) => {
                    let _ = events.send(BackendEvent::Error(error.to_string()));
                }
            }
            last_output_retry = Instant::now();
        }

        if last_rescan.elapsed() >= RESCAN_INTERVAL {
            let advanced_config = advanced_settings_snapshot(&advanced_settings);
            match discover_touchpads(&advanced_config) {
                Ok(scan) => reconcile_devices(
                    &mut devices,
                    scan,
                    &settings,
                    &mut advanced_broker,
                    &mut mouse,
                    &logger,
                    &events,
                ),
                Err(error) => {
                    let _ = events.send(BackendEvent::Error(error.to_string()));
                }
            }
            last_rescan = Instant::now();
        }
        thread::sleep(POLL_INTERVAL);
    }

    let snapshot = settings_snapshot(&settings);
    for state in &mut devices {
        let actions = state.drag.force_release(&snapshot);
        mouse.apply_all(state.source_id(), &actions, &logger);
        mouse.release_source(state.source_id(), &logger);
        cancel_advanced_session(state, &mut advanced_broker, "input_service_stopped");
        disable_touchpad_proxy(state, &logger);
    }
    advanced_broker.transport.stop();
    mouse.release_all(&logger);
}

fn discover_touchpads(
    advanced_config: &better_touch_advanced_gestures::config::AdvancedConfig,
) -> io::Result<DeviceScan> {
    let mut paths = fs::read_dir("/dev/input")?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("event"))
        })
        .collect::<Vec<_>>();
    paths.sort();

    let mut scan = DeviceScan::default();
    for path in paths {
        scan.event_paths.insert(path.clone());
        let device = match open_input_read_only(&path) {
            Ok(device) => device,
            Err(error) => {
                if error.kind() == io::ErrorKind::PermissionDenied {
                    scan.denied_paths.insert(path);
                }
                continue;
            }
        };
        if !is_touchpad(&device) {
            continue;
        }
        scan.readable
            .push(make_device(path, device, advanced_config.clone())?);
    }

    if scan.readable.is_empty() && !scan.denied_paths.is_empty() {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "无法读取 /dev/input/event*（{} 个节点被拒绝）。请授予当前登录用户触摸板事件节点的读取权限后重试。",
                scan.denied_paths.len()
            ),
        ))
    } else {
        Ok(scan)
    }
}

fn open_input_read_only(path: &PathBuf) -> io::Result<RawDevice> {
    // Physical input is observation-only. `RawDevice::open` tries O_RDWR
    // first, which is more authority than the gesture backend needs.
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)?;
    RawDevice::from_fd(OwnedFd::from(file))
}

fn reconcile_devices(
    devices: &mut Vec<TouchpadDevice>,
    mut scan: DeviceScan,
    settings: &Arc<RwLock<AppSettings>>,
    advanced_broker: &mut AdvancedBrokerState,
    mouse: &mut MouseOutput,
    logger: &RingLogger,
    events: &Sender<BackendEvent>,
) {
    let removed = devices
        .iter()
        .filter(|device| !scan.event_paths.contains(&device.path))
        .map(|device| device.path.clone())
        .collect::<BTreeSet<_>>();
    let known = devices
        .iter()
        .map(|device| device.path.clone())
        .collect::<BTreeSet<_>>();
    let added = scan
        .readable
        .drain(..)
        .filter(|device| !known.contains(&device.path))
        .collect::<Vec<_>>();
    let changed = !removed.is_empty() || !added.is_empty();

    if !removed.is_empty() {
        remove_devices(devices, &removed, settings, advanced_broker, mouse, logger);
    }
    devices.extend(added);
    devices.sort_by(|left, right| left.path.cmp(&right.path));

    // A least-privilege ACL normally grants only the touchpad node. Other
    // event devices (keyboards, power buttons, etc.) remaining unreadable is
    // expected and must not turn the UI into a repeating error state.
    if changed {
        publish_status(events, devices, mouse.is_available());
    }
}

fn remove_devices(
    devices: &mut Vec<TouchpadDevice>,
    removed: &BTreeSet<PathBuf>,
    settings: &Arc<RwLock<AppSettings>>,
    advanced_broker: &mut AdvancedBrokerState,
    mouse: &mut MouseOutput,
    logger: &RingLogger,
) {
    let snapshot = settings_snapshot(settings);
    for state in devices
        .iter_mut()
        .filter(|state| removed.contains(&state.path))
    {
        let actions = state.drag.force_release(&snapshot);
        mouse.apply_all(state.source_id(), &actions, logger);
        // Also release recorded ownership if settings changed to `None` or the
        // DragEngine did not emit an action for another reason.
        mouse.release_source(state.source_id(), logger);
        cancel_advanced_session(state, advanced_broker, "touchpad_disconnected");
        state.release_deadline = None;
    }
    devices.retain(|state| !removed.contains(&state.path));
}

fn is_disconnected_error(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(code) if matches!(code, libc::ENODEV | libc::ENXIO | libc::EIO | libc::EBADF))
}

fn is_touchpad(device: &RawDevice) -> bool {
    if TouchpadProxy::is_virtual_name(device.name()) {
        return false;
    }
    let has_axes = device.supported_absolute_axes().is_some_and(|axes| {
        axes.contains(AbsoluteAxisCode::ABS_MT_SLOT)
            && axes.contains(AbsoluteAxisCode::ABS_MT_TRACKING_ID)
            && axes.contains(AbsoluteAxisCode::ABS_MT_POSITION_X)
            && axes.contains(AbsoluteAxisCode::ABS_MT_POSITION_Y)
    });
    has_axes
        && device.properties().contains(PropType::POINTER)
        && !device.properties().contains(PropType::DIRECT)
}

fn make_device(
    path: PathBuf,
    device: RawDevice,
    advanced_config: better_touch_advanced_gestures::config::AdvancedConfig,
) -> io::Result<TouchpadDevice> {
    let input_id = device.input_id();
    let identity = format!(
        "{}|{}|{:04x}|{:04x}",
        device.name().unwrap_or_default(),
        device.physical_path().unwrap_or_default(),
        input_id.vendor(),
        input_id.product(),
    );
    let descriptor = DeviceDescriptor {
        id: stable_device_id(&identity),
        vendor_id: format!("{:04x}", input_id.vendor()),
        product_id: format!("{:04x}", input_id.product()),
    };
    let mut ranges = AxisRanges::default();
    let mut minimum_slot = 0;
    let mut maximum_slot = -1;
    for (axis, info) in device.get_absinfo()? {
        match axis {
            AbsoluteAxisCode::ABS_MT_SLOT => {
                minimum_slot = info.minimum();
                maximum_slot = info.maximum();
            }
            AbsoluteAxisCode::ABS_MT_POSITION_X => {
                ranges.min_x = info.minimum();
                ranges.max_x = info.maximum();
            }
            AbsoluteAxisCode::ABS_MT_POSITION_Y => {
                ranges.min_y = info.minimum();
                ranges.max_y = info.maximum();
            }
            _ => {}
        }
    }

    let source_key = path.to_string_lossy().into_owned();
    Ok(TouchpadDevice {
        source_key,
        path,
        device,
        descriptor,
        ranges,
        contacts: MtSlotTracker::new(minimum_slot, maximum_slot),
        drag: DragEngine::default(),
        advanced: AdvancedRuntime::new(advanced_config),
        advanced_session: None,
        advanced_gate: AdvancedContactGate::default(),
        touchpad_proxy: None,
        pending_input_frame: Vec::new(),
        last_advanced_frame: None,
        last_advanced_contact_count: 0,
        release_deadline: None,
        last_preview: Instant::now(),
        last_frame: None,
    })
}

fn process_available_events(
    state: &mut TouchpadDevice,
    settings: &Arc<RwLock<AppSettings>>,
    advanced_settings: &Arc<RwLock<better_touch_advanced_gestures::config::AdvancedConfig>>,
    advanced_broker: &mut AdvancedBrokerState,
    mouse: &mut MouseOutput,
    logger: &RingLogger,
    events: &Sender<BackendEvent>,
) -> io::Result<()> {
    let batch = state.device.fetch_events()?.collect::<Vec<_>>();
    for event in batch {
        if event.event_type() == EventType::SYNCHRONIZATION
            && event.code() == SynchronizationCode::SYN_DROPPED.0
        {
            state.pending_input_frame.clear();
            recover_from_dropped_events(state, settings, advanced_broker, mouse, logger, events);
        } else if state.contacts.is_resynchronizing() {
            if event.event_type() == EventType::SYNCHRONIZATION
                && event.code() == SynchronizationCode::SYN_REPORT.0
            {
                state.contacts.finish_resync();
            }
        } else if event.event_type() == EventType::ABSOLUTE {
            state.pending_input_frame.push(event);
            state.contacts.accept_absolute(event.code(), event.value());
        } else if event.event_type() == EventType::SYNCHRONIZATION
            && event.code() == SynchronizationCode::SYN_REPORT.0
        {
            state.pending_input_frame.push(event);
            if let Some(proxy) = state.touchpad_proxy.as_mut() {
                let proxy_contacts = state.contacts.proxy_contacts();
                if let Err(error) = proxy.handle_frame(&state.pending_input_frame, &proxy_contacts)
                {
                    state.pending_input_frame.clear();
                    fail_touchpad_proxy(state, advanced_broker, logger, events, error);
                    continue;
                }
            }
            state.pending_input_frame.clear();
            process_frame(
                state,
                settings,
                advanced_settings,
                advanced_broker,
                mouse,
                logger,
                events,
            );
        } else {
            state.pending_input_frame.push(event);
        }
    }
    Ok(())
}

fn recover_from_dropped_events(
    state: &mut TouchpadDevice,
    settings: &Arc<RwLock<AppSettings>>,
    advanced_broker: &mut AdvancedBrokerState,
    mouse: &mut MouseOutput,
    logger: &RingLogger,
    events: &Sender<BackendEvent>,
) {
    let snapshot = settings_snapshot(settings);
    let actions = state.drag.force_release(&snapshot);
    mouse.apply_all(state.source_id(), &actions, logger);
    mouse.release_source(state.source_id(), logger);
    state.drag = DragEngine::default();
    cancel_advanced_session(state, advanced_broker, "syn_dropped");
    let _ = state.advanced.cancel();
    state.advanced_gate.suppress();
    state.contacts.begin_resync();
    state.release_deadline = None;
    state.last_frame = None;
    disable_touchpad_proxy(state, logger);
    let message = format!(
        "触摸板 {} 的 evdev 事件发生丢失；已安全释放拖动，请抬起手指后重新触摸。",
        state.path.display()
    );
    logger.record(&message);
    let _ = events.send(BackendEvent::Error(message));
}

fn process_frame(
    state: &mut TouchpadDevice,
    settings: &Arc<RwLock<AppSettings>>,
    advanced_settings: &Arc<RwLock<better_touch_advanced_gestures::config::AdvancedConfig>>,
    advanced_broker: &mut AdvancedBrokerState,
    mouse: &mut MouseOutput,
    logger: &RingLogger,
    events: &Sender<BackendEvent>,
) {
    let now = Instant::now();
    let timestamp_ms = monotonic_millis();
    let contacts = state
        .contacts
        .contacts()
        .into_iter()
        .map(|contact| Contact {
            id: contact.id,
            x: normalize_axis(contact.x, state.ranges.min_x, state.ranges.max_x),
            y: normalize_axis(contact.y, state.ranges.min_y, state.ranges.max_y),
        })
        .collect::<Vec<_>>();
    let snapshot = settings_snapshot(settings);

    let three_finger_drag_allowed = state
        .touchpad_proxy
        .as_ref()
        .map(|proxy| proxy.allows_three_finger_drag())
        .unwrap_or(true);

    if !three_finger_drag_allowed {
        // This physical stream already belongs to a recognized two-finger
        // gesture or to native replay. Keep the basic drag state completely
        // outside that lane, including if a third contact appears briefly.
        let actions = state.drag.force_release(&snapshot);
        mouse.apply_all(state.source_id(), &actions, logger);
        mouse.release_source(state.source_id(), logger);
        state.drag = DragEngine::default();
        state
            .drag
            .observe_inactive_frame(contacts.clone(), timestamp_ms);
        state.release_deadline = None;
    } else if contacts.len() >= 4 {
        // GNOME owns all four-finger gestures. If a fourth contact arrives
        // during a drag, release immediately and do not inject motion from
        // this frame. Resetting prevents a four-to-three transition from
        // inheriting the old drag state.
        let actions = state.drag.force_release(&snapshot);
        mouse.apply_all(state.source_id(), &actions, logger);
        mouse.release_source(state.source_id(), logger);
        state.drag = DragEngine::default();
        state
            .drag
            .observe_inactive_frame(contacts.clone(), timestamp_ms);
        state.release_deadline = None;
    } else if snapshot.three_finger_drag {
        let result = state.drag.process_frame(
            &state.descriptor.id,
            contacts.clone(),
            timestamp_ms,
            &snapshot,
        );
        mouse.apply_all(state.source_id(), &result.mouse, logger);
        if !state.drag.is_dragging() {
            state.release_deadline = None;
            mouse.release_source(state.source_id(), logger);
        }
        match result.timer {
            TimerDirective::Arm(milliseconds) => {
                state.release_deadline = Some(now + Duration::from_millis(milliseconds.into()));
            }
            TimerDirective::Keep => {}
        }
    } else {
        if state.drag.is_dragging() {
            let actions = state.drag.force_release(&snapshot);
            mouse.apply_all(state.source_id(), &actions, logger);
            // `force_release` cannot know which button was pressed if settings
            // changed while dragging; MouseOutput owns that authoritative state.
            mouse.release_source(state.source_id(), logger);
        }
        state.release_deadline = None;
        state
            .drag
            .observe_inactive_frame(contacts.clone(), timestamp_ms);
    }

    process_advanced_frame(
        state,
        advanced_settings,
        advanced_broker,
        timestamp_ms,
        logger,
    );

    if state.last_preview.elapsed() >= PREVIEW_INTERVAL {
        let interval = state
            .last_frame
            .map(|previous| now.duration_since(previous).as_millis() as u64)
            .unwrap_or_default();
        let _ = events.send(BackendEvent::Contacts(ContactPreview {
            device: state.descriptor.clone(),
            contacts,
            event_interval_ms: interval,
        }));
        state.last_preview = now;
    }
    state.last_frame = Some(now);
}

fn process_advanced_frame(
    state: &mut TouchpadDevice,
    advanced_settings: &Arc<RwLock<better_touch_advanced_gestures::config::AdvancedConfig>>,
    broker: &mut AdvancedBrokerState,
    timestamp_ms: u64,
    logger: &RingLogger,
) {
    let raw_contacts = state.contacts.contacts();
    let contacts = raw_contacts
        .iter()
        .map(|contact| (contact.id, contact.x, contact.y))
        .collect::<Vec<_>>();

    let contact_count_changed = state.last_advanced_contact_count != contacts.len();
    state.last_advanced_contact_count = contacts.len();
    let mut config = advanced_settings_snapshot(advanced_settings);
    // Removed optional interactions stay fail-closed at the input boundary as
    // well as in the settings UI. This prevents a stale profile from reviving
    // preview movement, pointer warping or axis-resize event generation.
    config.live_preview = false;
    config.move_cursor = false;
    config.resize_horizontal_enabled = false;
    config.resize_vertical_enabled = false;
    // Ubuntu/Linux deliberately exposes only two-finger directional gestures.
    // Ignore stale five-finger preferences even if a third-party broker lies
    // about that capability.
    config.five_finger_enabled = false;
    // Four fingers are a hard GNOME boundary on Ubuntu. Five-finger free move
    // would necessarily cross that boundary while fingers lift, so it is only
    // enabled when a future broker explicitly advertises five-finger
    // arbitration. The current safe contract still cancels all 4+ frames.
    let five_finger_capable = false;
    if config != *state.advanced.config() {
        cancel_advanced_session(state, broker, "advanced_config_changed");
        cancel_device_advanced_runtime(state, Some(config.clone()));
    }

    let five_finger_allowed = five_finger_capable && config.five_finger_enabled;
    match state
        .advanced_gate
        .observe(contacts.len(), five_finger_allowed)
    {
        AdvancedContactDisposition::Process => {}
        AdvancedContactDisposition::ProcessAfterReset => {
            // `cancel` latches the recognizer until an empty frame. Rebaseline
            // before presenting the capability-approved five-finger candidate.
            let _ = state
                .advanced
                .process(&[], state.ranges, timestamp_ms as i64);
        }
        AdvancedContactDisposition::CancelAndSuppress => {
            let _ = state.advanced.cancel();
            let reason = match contacts.len() {
                3 => "three_finger_drag_owned",
                4 => "four_finger_gnome_owned",
                _ => "unsupported_contact_count",
            };
            cancel_advanced_session(state, broker, reason);
            return;
        }
        AdvancedContactDisposition::IgnoreUntilLift => return,
    }

    if !config.enabled || !config.gestures_enabled {
        cancel_advanced_session(state, broker, "advanced_disabled");
        cancel_device_advanced_runtime(state, None);
        return;
    }

    if !matches!(broker.availability, TransportAvailability::Available(_)) {
        // Fail closed: do not even advance the recognizer while the shell has
        // not promised to arbitrate native two-finger input.
        cancel_device_advanced_runtime(state, None);
        return;
    }

    if broker.modifiers.escape {
        cancel_device_advanced_runtime(state, None);
        cancel_advanced_session(state, broker, "escape_pressed");
        return;
    }
    if !contacts.is_empty() {
        // Modifier state matters only while a physical gesture is in flight.
        // Coalescing requests in the transport avoids an idle 60 Hz D-Bus
        // loop in GNOME Shell while still refreshing Shift/Alt/Escape during
        // every active gesture.
        broker.transport.request_modifier_poll();
    }
    if contacts.len() == 2
        && !contact_count_changed
        && state
            .last_advanced_frame
            .is_some_and(|last| last.elapsed() < ADVANCED_FRAME_INTERVAL)
    {
        return;
    }
    state.last_advanced_frame = Some(Instant::now());
    state
        .advanced
        .set_modifier_modes(broker.modifiers.thirds, broker.modifiers.monitor);

    let generated = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        state
            .advanced
            .process(&contacts, state.ranges, timestamp_ms as i64)
    }));
    let gesture_events = match generated {
        Ok(gesture_events) => gesture_events,
        Err(_) => {
            state
                .advanced
                .disable_after_error("Linux 进阶手势状态机异常，已安全停用。");
            cancel_advanced_session(state, broker, "gesture_runtime_panicked");
            state
                .advanced_gate
                .suppress_if_contacts(!contacts.is_empty());
            logger.record("Linux advanced gesture runtime panicked; disabled safely.");
            return;
        }
    };

    for gesture_event in gesture_events {
        if state
            .advanced_session
            .as_ref()
            .is_some_and(|session| session.phase == AdvancedSessionPhase::AwaitingTerminal)
        {
            continue;
        }
        if two_finger_intent_is_committed(&gesture_event) {
            if let Some(proxy) = state.touchpad_proxy.as_mut() {
                proxy.commit_two_finger_candidate();
            }
        }
        let terminal = {
            let dto = GestureEventDto::from_event(&gesture_event);
            dto.is_commit() || dto.is_cancel()
        };
        if let Err(error) = submit_advanced_event(state, broker, &config, &gesture_event) {
            if matches!(error, SubmitEventError::Busy) {
                cancel_device_advanced_runtime(state, None);
                logger.record(
                    "Another touchpad owns the advanced gesture session; candidate suppressed.",
                );
                break;
            }
            if matches!(error, SubmitEventError::Transport(SubmitError::Closed))
                && state
                    .advanced_session
                    .as_ref()
                    .is_some_and(|session| session.phase == AdvancedSessionPhase::AwaitingBegin)
            {
                cancel_device_advanced_runtime(state, None);
                abandon_advanced_session(state, broker);
                logger.record("GNOME gesture candidate was not accepted; native gesture kept.");
                break;
            }
            let queue_pressure =
                matches!(&error, SubmitEventError::Transport(SubmitError::QueueFull));
            let detail = match error {
                SubmitEventError::Serialize(error) => {
                    format!("进阶手势 JSON 编码失败：{error}")
                }
                SubmitEventError::Transport(SubmitError::QueueFull) => {
                    "GNOME 扩展传输队列已满；已取消当前进阶手势。".into()
                }
                SubmitEventError::Transport(SubmitError::Closed) => {
                    "GNOME 扩展传输线程已关闭；已取消当前进阶手势。".into()
                }
                SubmitEventError::MissingSession => {
                    "GNOME 扩展尚未接受 Begin；已丢弃后续进阶事件。".into()
                }
                SubmitEventError::Busy => unreachable!("handled above"),
            };
            cancel_device_advanced_runtime(state, None);
            cancel_advanced_session(state, broker, "local_transport_failure");
            if !queue_pressure {
                broker.availability = TransportAvailability::Unavailable(detail.clone());
            }
            logger.record(&detail);
            break;
        } else if terminal {
            // Some terminal actions (notably pinch) commit while fingers are
            // still down. Reset the recognizer immediately and keep it gated
            // until lift; otherwise the later lift-generated `Cancelled`
            // could be mistaken for a second session after the Commit ACK.
            cancel_device_advanced_runtime(state, None);
        }
    }
}

/// Returns true only after the existing advanced recognizer has produced a
/// semantic two-finger action. `Began`, raw movement, and neutral updates stay
/// candidates, preserving the current latency and three-finger landing path.
fn two_finger_intent_is_committed(event: &GestureEvent) -> bool {
    match event {
        GestureEvent::Updated { direction, .. } => *direction != SwipeDirection::None,
        GestureEvent::Completed(direction) => *direction != SwipeDirection::None,
        GestureEvent::HoldEngaged
        | GestureEvent::HoldUpdated { .. }
        | GestureEvent::DesktopMove(_)
        | GestureEvent::DesktopHoldCommit(_)
        | GestureEvent::MonitorMove(_)
        | GestureEvent::PinchUpdated { .. }
        | GestureEvent::PinchOut
        | GestureEvent::PinchIn
        | GestureEvent::AxisResizeBegan { .. }
        | GestureEvent::AxisResizeDelta { .. }
        | GestureEvent::AxisResizeEnded { .. } => true,
        GestureEvent::MonitorMoveUpdated { direction, .. } => direction.is_some(),
        GestureEvent::Began { .. }
        | GestureEvent::Raw { .. }
        | GestureEvent::Cancelled
        | GestureEvent::FreeMoveBegan
        | GestureEvent::FreeMoveDelta { .. }
        | GestureEvent::FreeMoveEnded { .. } => false,
    }
}

fn abandon_advanced_session(state: &mut TouchpadDevice, broker: &mut AdvancedBrokerState) {
    let Some(session) = state.advanced_session.take() else {
        return;
    };
    if broker.active_session.as_deref() == Some(session.id.as_str()) {
        broker.active_session = None;
    }
}

#[derive(Debug)]
enum SubmitEventError {
    Serialize(serde_json::Error),
    Transport(SubmitError),
    MissingSession,
    Busy,
}

fn submit_advanced_event(
    state: &mut TouchpadDevice,
    broker: &mut AdvancedBrokerState,
    config: &better_touch_advanced_gestures::config::AdvancedConfig,
    event: &better_touch_advanced_gestures::gesture::GestureEvent,
) -> Result<(), SubmitEventError> {
    let dto = GestureEventDto::from_event(event);
    if dto.is_begin() {
        if broker.active_session.is_some() {
            return Err(SubmitEventError::Busy);
        }
        if state.advanced_session.is_some() {
            cancel_advanced_session(state, broker, "duplicate_begin");
        }
        broker.session_counter = broker.session_counter.saturating_add(1);
        let session_id = format!(
            "{}-{}-{}",
            state.descriptor.id,
            monotonic_millis(),
            broker.session_counter
        );
        let mut session = AdvancedSession::new(session_id.clone());
        let sequence = session.take_sequence();
        let config_json = AdvancedConfigEnvelope::new(config.clone())
            .to_json()
            .map_err(SubmitEventError::Serialize)?;
        let event_json = dto.to_json().map_err(SubmitEventError::Serialize)?;
        broker
            .transport
            .begin(session_id.clone(), sequence, config_json, event_json)
            .map_err(SubmitEventError::Transport)?;
        broker.active_session = Some(session_id);
        state.advanced_session = Some(session);
        return Ok(());
    }

    let Some(session) = state.advanced_session.as_mut() else {
        return Err(SubmitEventError::MissingSession);
    };
    let session_id = session.id.clone();
    let sequence = session.take_sequence();
    if dto.is_cancel() {
        session.phase = AdvancedSessionPhase::AwaitingTerminal;
        broker
            .transport
            .cancel(session_id, sequence, "gesture_cancelled")
            .map_err(SubmitEventError::Transport)
    } else {
        let event_json = dto.to_json().map_err(SubmitEventError::Serialize)?;
        if dto.is_commit() {
            session.phase = AdvancedSessionPhase::AwaitingTerminal;
            broker
                .transport
                .commit(session_id, sequence, event_json)
                .map_err(SubmitEventError::Transport)
        } else {
            broker
                .transport
                .update(session_id, sequence, event_json)
                .map_err(SubmitEventError::Transport)
        }
    }
}

fn cancel_advanced_session(
    state: &mut TouchpadDevice,
    broker: &mut AdvancedBrokerState,
    reason: &str,
) {
    let Some(mut session) = state.advanced_session.take() else {
        return;
    };
    let sequence = session.take_sequence();
    if broker.active_session.as_deref() == Some(session.id.as_str()) {
        broker.active_session = None;
    }
    if broker
        .transport
        .cancel(session.id.clone(), sequence, reason)
        .is_err()
    {
        broker.availability = TransportAvailability::Unavailable(
            "无法可靠发送进阶手势 Cancel；已停用代理直到重新握手。".into(),
        );
    }
}

fn drain_transport_notices(
    devices: &mut [TouchpadDevice],
    broker: &mut AdvancedBrokerState,
    events: &Sender<BackendEvent>,
    logger: &RingLogger,
) {
    while let Some(notice) = broker.transport.try_notice() {
        match notice {
            TransportNotice::Availability(availability) => {
                if matches!(
                    availability,
                    TransportAvailability::Connecting | TransportAvailability::Unavailable(_)
                ) {
                    for state in devices.iter_mut() {
                        cancel_device_advanced_runtime(state, None);
                        state.advanced_session = None;
                    }
                    broker.active_session = None;
                }
                if matches!(availability, TransportAvailability::Configuring(_)) {
                    broker.configured_json = None;
                    broker.configuring_json = None;
                    broker.pending_reconfigure = false;
                }
                broker.availability = availability;
            }
            TransportNotice::Configured { config_json } => {
                broker.configured_json = Some(config_json);
                broker.configuring_json = None;
                if let TransportAvailability::Configuring(capabilities) = &broker.availability {
                    broker.availability = TransportAvailability::Available(capabilities.clone());
                }
            }
            TransportNotice::Modifiers {
                thirds,
                monitor,
                escape,
            } => {
                broker.modifiers = BrokerModifiers {
                    thirds,
                    monitor,
                    escape,
                };
            }
            TransportNotice::Rebaseline {
                session_id,
                direction,
            } => {
                if let Some(state) = devices.iter_mut().find(|state| {
                    state
                        .advanced_session
                        .as_ref()
                        .is_some_and(|session| session.id == session_id)
                }) {
                    state.advanced.rebaseline_seed(direction);
                }
            }
            TransportNotice::BeginAccepted { session_id } => {
                if let Some(session) = find_session_mut(devices, &session_id) {
                    if session.phase == AdvancedSessionPhase::AwaitingBegin {
                        session.phase = AdvancedSessionPhase::Accepted;
                    }
                }
            }
            TransportNotice::BeginRejected { session_id, detail } => {
                logger.record(format!("GNOME gesture arbitration declined: {detail}"));
                if broker.active_session.as_deref() == Some(session_id.as_str()) {
                    broker.active_session = None;
                }
                for state in devices.iter_mut() {
                    if state
                        .advanced_session
                        .as_ref()
                        .is_some_and(|session| session.id == session_id)
                    {
                        cancel_device_advanced_runtime(state, None);
                        let proxy_error = if let Some(proxy) = state.touchpad_proxy.as_mut() {
                            let contacts = state.contacts.proxy_contacts();
                            proxy.reject_candidate(&contacts).err()
                        } else {
                            None
                        };
                        if let Some(error) = proxy_error {
                            fail_touchpad_proxy(state, broker, logger, events, error);
                        }
                        state.advanced_session = None;
                    }
                }
            }
            TransportNotice::GestureFinished { session_id } => {
                clear_session(devices, &session_id);
                if broker.active_session.as_deref() == Some(session_id.as_str()) {
                    broker.active_session = None;
                }
            }
            TransportNotice::GestureFailed { session_id, detail } => {
                logger.record(&detail);
                // Gesture rejection is session-local. The transport publishes
                // a separate Availability(Unavailable) notice for I/O or
                // handshake failures that require taking the broker offline.
                for state in devices.iter_mut() {
                    if state
                        .advanced_session
                        .as_ref()
                        .is_some_and(|session| session.id == session_id)
                    {
                        cancel_device_advanced_runtime(state, None);
                        state.advanced_session = None;
                    }
                }
                if broker.active_session.as_deref() == Some(session_id.as_str()) {
                    broker.active_session = None;
                }
            }
        }
    }
    publish_advanced_status(events, devices, broker);
}

fn find_session_mut<'a>(
    devices: &'a mut [TouchpadDevice],
    session_id: &str,
) -> Option<&'a mut AdvancedSession> {
    devices.iter_mut().find_map(|state| {
        state
            .advanced_session
            .as_mut()
            .filter(|session| session.id == session_id)
    })
}

fn clear_session(devices: &mut [TouchpadDevice], session_id: &str) {
    for state in devices {
        if state
            .advanced_session
            .as_ref()
            .is_some_and(|session| session.id == session_id)
        {
            state.advanced_session = None;
        }
    }
}

fn aggregate_advanced_status(
    devices: &[TouchpadDevice],
    broker: &AdvancedBrokerState,
) -> AdvancedRuntimeStatus {
    let mut status = AdvancedRuntimeStatus::default();
    for state in devices {
        let device = state.advanced.status();
        status.processed_frames = status
            .processed_frames
            .saturating_add(device.processed_frames);
        status.emitted_events = status.emitted_events.saturating_add(device.emitted_events);
        status.completed_swooshes = status
            .completed_swooshes
            .saturating_add(device.completed_swooshes);
        status.last_event = device.last_event.or(status.last_event);
    }
    status.available = matches!(broker.availability, TransportAvailability::Available(_));
    status.enabled =
        status.available && devices.iter().any(|state| state.advanced.status().enabled);
    status.last_error = match &broker.availability {
        TransportAvailability::Available(_) => None,
        TransportAvailability::Connecting | TransportAvailability::Configuring(_) => {
            Some(ADVANCED_UNAVAILABLE.into())
        }
        TransportAvailability::Unavailable(detail) => Some(detail.clone()),
    };
    let capabilities = match &broker.availability {
        TransportAvailability::Available(value) | TransportAvailability::Configuring(value) => {
            Some(value)
        }
        TransportAvailability::Connecting | TransportAvailability::Unavailable(_) => None,
    };
    if let Some(capabilities) = capabilities {
        let actions = &capabilities.actions;
        status.capabilities = AdvancedCapabilities {
            two_finger: capabilities.contact_arbitration.two_finger,
            five_finger: capabilities.contact_arbitration.five_finger,
            snap_halves: actions.snap_halves,
            snap_quarters: actions.snap_quarters,
            snap_thirds: actions.snap_thirds,
            maximize: actions.maximize,
            minimize: actions.minimize,
            minimize_all: actions.minimize_all,
            close: actions.close,
            workspace: actions.workspace,
            dynamic_workspace: actions.dynamic_workspace,
            monitor_move: actions.monitor_move,
            free_move: actions.free_move,
            free_resize: actions.free_resize,
            axis_resize: actions.axis_resize,
            pinch: actions.pinch,
            hud: actions.hud,
            animation: actions.animation,
            live_preview: actions.live_preview,
            move_cursor: actions.move_cursor,
            app_switch: actions.app_switch,
        };
    }
    status
}

fn sync_broker_configuration(
    settings: &Arc<RwLock<better_touch_advanced_gestures::config::AdvancedConfig>>,
    devices: &mut [TouchpadDevice],
    broker: &mut AdvancedBrokerState,
    events: &Sender<BackendEvent>,
    logger: &RingLogger,
) {
    let can_configure = matches!(
        broker.availability,
        TransportAvailability::Configuring(_) | TransportAvailability::Available(_)
    );
    if !can_configure {
        return;
    }
    let config = advanced_settings_snapshot(settings);
    let json = match AdvancedConfigEnvelope::new(config.clone()).to_json() {
        Ok(json) => json,
        Err(error) => {
            let detail = format!("进阶配置 JSON 编码失败：{error}");
            broker.availability = TransportAvailability::Unavailable(detail.clone());
            let _ = events.send(BackendEvent::Error(detail));
            return;
        }
    };
    if !broker.pending_reconfigure
        && (broker.configured_json.as_deref() == Some(json.as_str())
            || broker.configuring_json.as_deref() == Some(json.as_str()))
    {
        return;
    }
    if broker.active_session.is_some() {
        broker.pending_reconfigure = true;
        return;
    }
    if broker.configured_json.is_some() {
        for state in devices {
            cancel_advanced_session(state, broker, "advanced_config_changed");
            cancel_device_advanced_runtime(state, Some(config.clone()));
        }
    }
    if let TransportAvailability::Available(capabilities) = &broker.availability {
        broker.availability = TransportAvailability::Configuring(capabilities.clone());
    }
    match broker.transport.configure(json.clone()) {
        Ok(()) => {
            broker.configuring_json = Some(json);
            broker.pending_reconfigure = false;
        }
        Err(error) => {
            let detail = format!("无法提交 GNOME 扩展配置：{error:?}");
            logger.record(&detail);
            broker.availability = TransportAvailability::Unavailable(detail);
        }
    }
}

fn sync_touchpad_proxies(
    settings: &Arc<RwLock<better_touch_advanced_gestures::config::AdvancedConfig>>,
    devices: &mut [TouchpadDevice],
    broker: &mut AdvancedBrokerState,
    events: &Sender<BackendEvent>,
    logger: &RingLogger,
) {
    let config = advanced_settings_snapshot(settings);
    let broker_supports_two = match &broker.availability {
        TransportAvailability::Available(capabilities) => {
            capabilities.contact_arbitration.two_finger
        }
        TransportAvailability::Connecting
        | TransportAvailability::Configuring(_)
        | TransportAvailability::Unavailable(_) => false,
    };
    let requested = config.enabled && config.gestures_enabled && broker_supports_two;
    let mut failure = None;

    for state in devices.iter_mut() {
        if !requested {
            // Keep ownership through the terminal physical frame. Dropping a
            // grab while contacts are down would expose an update-only stream
            // to libinput with no matching contact begin.
            if state.contacts.contacts().is_empty() {
                disable_touchpad_proxy(state, logger);
            }
            continue;
        }
        if state.touchpad_proxy.is_some() || !state.contacts.contacts().is_empty() {
            continue;
        }
        match TouchpadProxy::create_and_grab(&mut state.device) {
            Ok(proxy) => {
                logger.record(format!(
                    "Advanced two-finger arbitration owns {} through a cloned uinput touchpad.",
                    state.path.display()
                ));
                state.touchpad_proxy = Some(proxy);
            }
            Err(error) => {
                failure = Some(format!(
                    "无法为 {} 建立双指独占输入代理：{error}。已保持原生触摸板输入。",
                    state.path.display()
                ));
                break;
            }
        }
    }

    if let Some(detail) = failure {
        for state in devices.iter_mut() {
            disable_touchpad_proxy(state, logger);
            cancel_device_advanced_runtime(state, None);
            state.advanced_session = None;
        }
        broker.active_session = None;
        broker.availability = TransportAvailability::Unavailable(detail.clone());
        logger.record(&detail);
        let _ = events.send(BackendEvent::Error(detail));
    }
}

fn disable_touchpad_proxy(state: &mut TouchpadDevice, logger: &RingLogger) {
    let Some(mut proxy) = state.touchpad_proxy.take() else {
        return;
    };
    if let Err(error) = proxy.release_all() {
        logger.record(format!(
            "Unable to release cloned touchpad state for {}: {error}",
            state.path.display()
        ));
    }
    if let Err(error) = state.device.ungrab() {
        logger.record(format!(
            "Unable to release exclusive touchpad ownership for {}: {error}",
            state.path.display()
        ));
    }
    drop(proxy);
}

fn fail_touchpad_proxy(
    state: &mut TouchpadDevice,
    broker: &mut AdvancedBrokerState,
    logger: &RingLogger,
    events: &Sender<BackendEvent>,
    error: io::Error,
) {
    let detail = format!("双指输入代理写入失败：{error}。已解除物理触摸板独占并停用高级手势。");
    disable_touchpad_proxy(state, logger);
    cancel_advanced_session(state, broker, "touchpad_proxy_failed");
    cancel_device_advanced_runtime(state, None);
    broker.availability = TransportAvailability::Unavailable(detail.clone());
    logger.record(&detail);
    let _ = events.send(BackendEvent::Error(detail));
}

fn publish_advanced_status(
    events: &Sender<BackendEvent>,
    devices: &[TouchpadDevice],
    broker: &mut AdvancedBrokerState,
) {
    let status = aggregate_advanced_status(devices, broker);
    let changed = broker
        .last_published
        .as_ref()
        .is_none_or(|previous| !advanced_status_eq(previous, &status));
    let critical_changed = broker.last_published.as_ref().is_none_or(|previous| {
        previous.enabled != status.enabled
            || previous.available != status.available
            || previous.last_error != status.last_error
    });
    if changed && (critical_changed || broker.last_status_publish.elapsed() >= PREVIEW_INTERVAL) {
        let _ = events.send(BackendEvent::Advanced(status.clone()));
        broker.last_published = Some(status);
        broker.last_status_publish = Instant::now();
    }
}

fn advanced_status_eq(left: &AdvancedRuntimeStatus, right: &AdvancedRuntimeStatus) -> bool {
    left.enabled == right.enabled
        && left.available == right.available
        && left.processed_frames == right.processed_frames
        && left.emitted_events == right.emitted_events
        && left.last_event == right.last_event
        && left.last_error == right.last_error
        && left.completed_swooshes == right.completed_swooshes
        && left.capabilities == right.capabilities
}

fn release_if_due(
    state: &mut TouchpadDevice,
    settings: &Arc<RwLock<AppSettings>>,
    mouse: &mut MouseOutput,
    logger: &RingLogger,
) {
    if !state
        .release_deadline
        .is_some_and(|deadline| Instant::now() >= deadline)
    {
        return;
    }
    state.release_deadline = None;
    let snapshot = settings_snapshot(settings);
    let actions = state.drag.release_timeout(&snapshot);
    mouse.apply_all(state.source_id(), &actions, logger);
    if !state.drag.is_dragging() {
        mouse.release_source(state.source_id(), logger);
    }
}

fn publish_status(
    events: &Sender<BackendEvent>,
    devices: &[TouchpadDevice],
    output_available: bool,
) {
    let _ = events.send(BackendEvent::Status(TouchpadRuntimeStatus {
        initialized: true,
        touchpad_exists: !devices.is_empty(),
        receiver_installed: output_available,
        devices: devices
            .iter()
            .map(|device| device.descriptor.clone())
            .collect(),
    }));
}

fn settings_snapshot(settings: &Arc<RwLock<AppSettings>>) -> AppSettings {
    settings
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

fn advanced_settings_snapshot(
    settings: &Arc<RwLock<better_touch_advanced_gestures::config::AdvancedConfig>>,
) -> better_touch_advanced_gestures::config::AdvancedConfig {
    settings
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

fn normalize_axis(value: i32, minimum: i32, maximum: i32) -> i32 {
    if maximum <= minimum {
        return value;
    }
    let numerator = i64::from(value.saturating_sub(minimum)) * i64::from(NORMALIZED_AXIS_MAX);
    (numerator / i64::from(maximum - minimum)).clamp(0, i64::from(NORMALIZED_AXIS_MAX)) as i32
}

fn monotonic_millis() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

#[derive(Debug, Clone, Copy)]
struct SlotContact {
    id: i32,
    x: Option<i32>,
    y: Option<i32>,
}

#[derive(Debug, Default)]
struct MtSlotTracker {
    current_slot: Option<i32>,
    minimum_slot: i32,
    maximum_slot: i32,
    slots: BTreeMap<i32, SlotContact>,
    resynchronizing: bool,
}

impl MtSlotTracker {
    fn new(minimum_slot: i32, maximum_slot: i32) -> Self {
        Self {
            current_slot: Some(minimum_slot),
            minimum_slot,
            maximum_slot,
            slots: BTreeMap::new(),
            resynchronizing: false,
        }
    }

    fn valid_slot(&self, slot: i32) -> bool {
        self.maximum_slot < self.minimum_slot
            || (self.minimum_slot..=self.maximum_slot).contains(&slot)
    }

    fn accept_absolute(&mut self, code: u16, value: i32) {
        if self.resynchronizing {
            return;
        }
        match AbsoluteAxisCode(code) {
            AbsoluteAxisCode::ABS_MT_SLOT if self.valid_slot(value) => {
                self.current_slot = Some(value)
            }
            AbsoluteAxisCode::ABS_MT_SLOT => self.current_slot = None,
            AbsoluteAxisCode::ABS_MT_TRACKING_ID if value < 0 => {
                if let Some(slot) = self.current_slot {
                    self.slots.remove(&slot);
                }
            }
            AbsoluteAxisCode::ABS_MT_TRACKING_ID => {
                // Reusing a slot with a new tracking id begins a new contact;
                // stale coordinates from the previous finger must not leak.
                if let Some(slot) = self.current_slot {
                    self.slots.retain(|existing_slot, contact| {
                        *existing_slot == slot || contact.id != value
                    });
                    self.slots.insert(
                        slot,
                        SlotContact {
                            id: value,
                            x: None,
                            y: None,
                        },
                    );
                }
            }
            AbsoluteAxisCode::ABS_MT_POSITION_X => {
                if let Some(slot) = self.current_slot {
                    if let Some(contact) = self.slots.get_mut(&slot) {
                        contact.x = Some(value);
                    }
                }
            }
            AbsoluteAxisCode::ABS_MT_POSITION_Y => {
                if let Some(slot) = self.current_slot {
                    if let Some(contact) = self.slots.get_mut(&slot) {
                        contact.y = Some(value);
                    }
                }
            }
            _ => {}
        }
    }

    fn contacts(&self) -> Vec<CompleteSlotContact> {
        self.slots
            .iter()
            .filter_map(|(slot, contact)| {
                Some(CompleteSlotContact {
                    slot: *slot,
                    id: contact.id,
                    x: contact.x?,
                    y: contact.y?,
                })
            })
            .collect()
    }

    fn proxy_contacts(&self) -> Vec<ProxyContact> {
        self.contacts()
            .into_iter()
            .map(|contact| ProxyContact {
                slot: contact.slot,
                tracking_id: contact.id,
                x: contact.x,
                y: contact.y,
            })
            .collect()
    }

    fn begin_resync(&mut self) {
        self.slots.clear();
        self.current_slot = None;
        self.resynchronizing = true;
    }

    fn clear_for_new_contacts(&mut self) {
        self.slots.clear();
        self.current_slot = None;
        self.resynchronizing = false;
    }

    fn finish_resync(&mut self) {
        self.resynchronizing = false;
    }

    fn is_resynchronizing(&self) -> bool {
        self.resynchronizing
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CompleteSlotContact {
    slot: i32,
    id: i32,
    x: i32,
    y: i32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::advanced_transport::{
        BrokerActionCapabilities, BrokerCapabilities, GestureArbitrationCapabilities,
    };
    use better_touch_advanced_gestures::{
        config::AdvancedConfig,
        gesture::{DesktopDirection, GestureEvent, MonitorDirection, SwipeDirection},
    };

    fn test_broker_capabilities() -> BrokerCapabilities {
        BrokerCapabilities {
            protocol_version: 1,
            generation: "test-generation".into(),
            advanced_events: true,
            modifiers: true,
            rebaseline_feedback: true,
            contact_arbitration: GestureArbitrationCapabilities {
                two_finger: true,
                five_finger: true,
            },
            actions: BrokerActionCapabilities::default(),
        }
    }

    fn enabled_advanced_config() -> AdvancedConfig {
        AdvancedConfig {
            enabled: true,
            gestures_enabled: true,
            five_finger_enabled: false,
            ..AdvancedConfig::default()
        }
    }

    fn two_contacts() -> [(i32, i32, i32); 2] {
        [(1, 10_000, 16_000), (2, 22_000, 16_000)]
    }

    fn assert_begins_two_finger(runtime: &mut AdvancedRuntime, timestamp_ms: i64) {
        assert!(runtime
            .process(&two_contacts(), AxisRanges::default(), timestamp_ms)
            .iter()
            .any(|event| matches!(event, GestureEvent::Began { contacts: 2 })));
    }

    #[test]
    fn two_finger_lane_commits_only_after_existing_recognizer_has_intent() {
        let candidates = [
            GestureEvent::Began { contacts: 2 },
            GestureEvent::Raw { dx: 0.01, dy: 0.0 },
            GestureEvent::Updated {
                direction: SwipeDirection::None,
                progress: 0.0,
            },
            GestureEvent::Completed(SwipeDirection::None),
            GestureEvent::Cancelled,
            GestureEvent::MonitorMoveUpdated {
                direction: None,
                progress: 0.0,
            },
            GestureEvent::FreeMoveBegan,
            GestureEvent::FreeMoveDelta {
                dx: 0.1,
                dy: 0.1,
                scale: 1.0,
            },
            GestureEvent::FreeMoveEnded {
                was_tap: false,
                cancelled: true,
            },
        ];
        for event in candidates {
            assert!(!two_finger_intent_is_committed(&event), "{event:?}");
        }

        let committed = [
            GestureEvent::Updated {
                direction: SwipeDirection::Right,
                progress: 0.5,
            },
            GestureEvent::Completed(SwipeDirection::Right),
            GestureEvent::HoldEngaged,
            GestureEvent::HoldUpdated {
                direction: Some(DesktopDirection::Right),
                progress: 0.5,
                aim_steps: 1,
            },
            GestureEvent::DesktopMove(DesktopDirection::Left),
            GestureEvent::DesktopHoldCommit(1),
            GestureEvent::MonitorMoveUpdated {
                direction: Some(MonitorDirection::Right),
                progress: 0.5,
            },
            GestureEvent::MonitorMove(MonitorDirection::Left),
            GestureEvent::PinchUpdated {
                outward: true,
                progress: 0.2,
            },
            GestureEvent::PinchOut,
            GestureEvent::PinchIn,
            GestureEvent::AxisResizeBegan { horizontal: true },
            GestureEvent::AxisResizeDelta {
                factor: 1.1,
                horizontal: true,
            },
            GestureEvent::AxisResizeEnded { cancelled: false },
        ];
        for event in committed {
            assert!(two_finger_intent_is_committed(&event), "{event:?}");
        }
    }

    #[test]
    fn idle_availability_or_failure_transition_does_not_swallow_next_gesture() {
        let mut runtime = AdvancedRuntime::new(enabled_advanced_config());
        let mut gate = AdvancedContactGate::default();
        assert_begins_two_finger(&mut runtime, 1);
        gate.suppress();

        // The broker notice arrives after the physical device already reports
        // zero contacts, but before another evdev empty frame is guaranteed.
        cancel_advanced_runtime_state(
            &mut runtime,
            &mut gate,
            false,
            AxisRanges::default(),
            2,
            None,
        );

        assert_eq!(gate.observe(2, false), AdvancedContactDisposition::Process);
        assert_begins_two_finger(&mut runtime, 3);
    }

    #[test]
    fn idle_configuration_transition_applies_config_and_rearms_first_gesture() {
        let mut runtime = AdvancedRuntime::new(AdvancedConfig::default());
        let mut gate = AdvancedContactGate::default();
        gate.suppress();
        let mut replacement = enabled_advanced_config();
        replacement.sensitivity = 0.42;

        cancel_advanced_runtime_state(
            &mut runtime,
            &mut gate,
            false,
            AxisRanges::default(),
            1,
            Some(replacement.clone()),
        );

        assert_eq!(runtime.config(), &replacement.normalized());
        assert_eq!(gate.observe(2, false), AdvancedContactDisposition::Process);
        assert_begins_two_finger(&mut runtime, 2);
    }

    #[test]
    fn active_broker_transition_remains_suppressed_until_lift() {
        let mut runtime = AdvancedRuntime::new(enabled_advanced_config());
        let mut gate = AdvancedContactGate::default();
        assert_begins_two_finger(&mut runtime, 1);

        cancel_advanced_runtime_state(
            &mut runtime,
            &mut gate,
            true,
            AxisRanges::default(),
            2,
            None,
        );

        assert_eq!(
            gate.observe(2, false),
            AdvancedContactDisposition::IgnoreUntilLift
        );
        assert_eq!(gate.observe(0, false), AdvancedContactDisposition::Process);
        assert!(runtime.process(&[], AxisRanges::default(), 3).is_empty());
        assert_eq!(gate.observe(2, false), AdvancedContactDisposition::Process);
        assert_begins_two_finger(&mut runtime, 4);
    }

    #[test]
    fn gesture_failure_keeps_broker_available_until_explicit_transport_unavailable() {
        let (notice_sender, notice_receiver) = mpsc::channel();
        let transport = AdvancedTransport::from_notice_receiver_for_test(notice_receiver);
        let capabilities = test_broker_capabilities();
        let mut broker = AdvancedBrokerState {
            transport,
            availability: TransportAvailability::Available(capabilities.clone()),
            modifiers: BrokerModifiers::default(),
            session_counter: 1,
            active_session: Some("gesture-1".into()),
            last_published: None,
            last_status_publish: Instant::now() - PREVIEW_INTERVAL,
            configured_json: Some("{}".into()),
            configuring_json: None,
            pending_reconfigure: false,
        };
        let (event_sender, _events) = mpsc::channel();
        let logger = RingLogger::default();
        logger.set_enabled(true);

        notice_sender
            .send(TransportNotice::GestureFailed {
                session_id: "gesture-1".into(),
                detail: "gesture rejected".into(),
            })
            .unwrap();
        drain_transport_notices(&mut [], &mut broker, &event_sender, &logger);

        assert_eq!(
            broker.availability,
            TransportAvailability::Available(capabilities)
        );
        assert!(broker.active_session.is_none());
        assert!(logger
            .snapshot()
            .iter()
            .any(|entry| entry.contains("gesture rejected")));

        notice_sender
            .send(TransportNotice::Availability(
                TransportAvailability::Unavailable("transport failed".into()),
            ))
            .unwrap();
        drain_transport_notices(&mut [], &mut broker, &event_sender, &logger);

        assert_eq!(
            broker.availability,
            TransportAvailability::Unavailable("transport failed".into())
        );
    }

    #[test]
    fn multitouch_slots_track_contacts_and_lifts() {
        let mut tracker = MtSlotTracker::new(0, 4);
        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_SLOT.0, 0);
        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_TRACKING_ID.0, 10);
        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_POSITION_X.0, 100);
        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_POSITION_Y.0, 200);
        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_SLOT.0, 1);
        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_TRACKING_ID.0, 11);
        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_POSITION_X.0, 300);
        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_POSITION_Y.0, 400);

        assert_eq!(
            tracker.contacts(),
            vec![
                CompleteSlotContact {
                    slot: 0,
                    id: 10,
                    x: 100,
                    y: 200
                },
                CompleteSlotContact {
                    slot: 1,
                    id: 11,
                    x: 300,
                    y: 400
                }
            ]
        );

        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_TRACKING_ID.0, -1);
        assert_eq!(tracker.contacts().len(), 1);
    }

    #[test]
    fn reused_slot_does_not_publish_stale_coordinates() {
        let mut tracker = MtSlotTracker::new(0, 1);
        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_SLOT.0, 0);
        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_TRACKING_ID.0, 10);
        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_POSITION_X.0, 100);
        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_POSITION_Y.0, 200);
        assert_eq!(tracker.contacts().len(), 1);

        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_TRACKING_ID.0, 11);
        assert!(tracker.contacts().is_empty());
        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_POSITION_X.0, 300);
        assert!(tracker.contacts().is_empty());
        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_POSITION_Y.0, 400);
        assert_eq!(
            tracker.contacts(),
            vec![CompleteSlotContact {
                slot: 0,
                id: 11,
                x: 300,
                y: 400,
            }]
        );
    }

    #[test]
    fn invalid_slot_and_duplicate_tracking_id_are_rejected() {
        let mut tracker = MtSlotTracker::new(0, 1);
        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_SLOT.0, 0);
        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_TRACKING_ID.0, 10);
        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_POSITION_X.0, 100);
        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_POSITION_Y.0, 200);

        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_SLOT.0, 9);
        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_TRACKING_ID.0, 99);
        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_POSITION_X.0, 900);
        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_POSITION_Y.0, 900);
        assert_eq!(tracker.contacts().len(), 1);

        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_SLOT.0, 1);
        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_TRACKING_ID.0, 10);
        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_POSITION_X.0, 300);
        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_POSITION_Y.0, 400);
        assert_eq!(
            tracker.contacts(),
            vec![CompleteSlotContact {
                slot: 1,
                id: 10,
                x: 300,
                y: 400,
            }]
        );
    }

    #[test]
    fn syn_dropped_clears_slots_and_ignores_the_remainder_of_that_report() {
        let mut tracker = MtSlotTracker::new(0, 4);
        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_SLOT.0, 0);
        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_TRACKING_ID.0, 10);
        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_POSITION_X.0, 100);
        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_POSITION_Y.0, 200);
        assert_eq!(tracker.contacts().len(), 1);

        tracker.begin_resync();
        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_SLOT.0, 0);
        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_TRACKING_ID.0, 20);
        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_POSITION_X.0, 300);
        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_POSITION_Y.0, 400);
        assert!(tracker.contacts().is_empty());

        tracker.finish_resync();
        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_SLOT.0, 0);
        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_TRACKING_ID.0, 20);
        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_POSITION_X.0, 300);
        tracker.accept_absolute(AbsoluteAxisCode::ABS_MT_POSITION_Y.0, 400);
        assert_eq!(tracker.contacts().len(), 1);
    }

    #[test]
    fn linux_coordinates_are_normalized_to_the_windows_engine_range() {
        assert_eq!(normalize_axis(100, 100, 1_100), 0);
        assert_eq!(normalize_axis(600, 100, 1_100), 16_383);
        assert_eq!(normalize_axis(1_100, 100, 1_100), NORMALIZED_AXIS_MAX);
    }

    #[test]
    fn advanced_contact_gate_never_restarts_after_three_or_four_until_lift() {
        let mut gate = AdvancedContactGate::default();
        assert_eq!(gate.observe(2, false), AdvancedContactDisposition::Process);
        assert_eq!(
            gate.observe(3, false),
            AdvancedContactDisposition::CancelAndSuppress
        );
        assert_eq!(
            gate.observe(2, false),
            AdvancedContactDisposition::IgnoreUntilLift
        );
        assert_eq!(gate.observe(0, false), AdvancedContactDisposition::Process);
        assert_eq!(gate.observe(2, false), AdvancedContactDisposition::Process);

        assert_eq!(
            gate.observe(4, false),
            AdvancedContactDisposition::CancelAndSuppress
        );
        assert_eq!(
            gate.observe(2, false),
            AdvancedContactDisposition::IgnoreUntilLift
        );
        assert_eq!(gate.observe(0, false), AdvancedContactDisposition::Process);
    }

    #[test]
    fn five_finger_candidate_requires_explicit_arbitration_capability() {
        let mut gate = AdvancedContactGate::default();
        assert_eq!(
            gate.observe(4, true),
            AdvancedContactDisposition::CancelAndSuppress
        );
        assert_eq!(
            gate.observe(5, true),
            AdvancedContactDisposition::ProcessAfterReset
        );
        assert_eq!(gate.observe(0, true), AdvancedContactDisposition::Process);
        assert_eq!(gate.observe(5, true), AdvancedContactDisposition::Process);

        let mut unsupported = AdvancedContactGate::default();
        assert_eq!(
            unsupported.observe(4, false),
            AdvancedContactDisposition::CancelAndSuppress
        );
        assert_eq!(
            unsupported.observe(5, false),
            AdvancedContactDisposition::CancelAndSuppress
        );
    }
}
