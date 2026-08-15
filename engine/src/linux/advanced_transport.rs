//! Fault-contained transport from the evdev worker to the GNOME Shell broker.
//!
//! The physical-input loop must never wait for D-Bus.  It submits immutable
//! JSON envelopes to a bounded queue; a dedicated worker owns the session-bus
//! connection and all method calls.  A gesture is not considered accepted
//! until the broker acknowledges `Begin`, and every terminal call is checked.

use std::{
    collections::{BTreeSet, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use better_touch_advanced_gestures::{
    config::AdvancedConfig,
    gesture::{DesktopDirection, GestureEvent, MonitorDirection, SwipeDirection},
};
use serde::{de::Error as _, Deserialize, Deserializer, Serialize};

use crate::logging::RingLogger;

pub const PROTOCOL_VERSION: u32 = 2;
pub const INPUT_PROXY_GENERATION: &str = "v6-buffered-preflight-1";
pub const BUS_NAME: &str = "io.github.xuzenghui942.ThreeFingerDrag.Gnome";
pub const OBJECT_PATH: &str = "/io/github/xuzenghui942/ThreeFingerDrag/Gnome";
pub const INTERFACE_NAME: &str = "io.github.xuzenghui942.ThreeFingerDrag.Gnome1";

const QUEUE_CAPACITY: usize = 64;
const METHOD_TIMEOUT: Duration = Duration::from_millis(350);
const RECONNECT_INTERVAL: Duration = Duration::from_secs(2);
const MODIFIER_POLL_INTERVAL: Duration = Duration::from_millis(16);

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GestureArbitrationCapabilities {
    #[serde(default)]
    pub two_finger: bool,
    #[serde(default)]
    pub five_finger: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrokerActionCapabilities {
    #[serde(default)]
    pub snap_halves: bool,
    #[serde(default)]
    pub snap_quarters: bool,
    #[serde(default)]
    pub maximize: bool,
    #[serde(default)]
    pub minimize: bool,
    #[serde(default)]
    pub minimize_all: bool,
    #[serde(default)]
    pub close: bool,
    #[serde(default)]
    pub workspace: bool,
    #[serde(default)]
    pub dynamic_workspace: bool,
    #[serde(default)]
    pub monitor_move: bool,
    #[serde(default)]
    pub free_move: bool,
    #[serde(default)]
    pub free_resize: bool,
    #[serde(default)]
    pub axis_resize: bool,
    #[serde(default)]
    pub pinch: bool,
    #[serde(default)]
    pub hud: bool,
    #[serde(default)]
    pub animation: bool,
    #[serde(default)]
    pub live_preview: bool,
    #[serde(default)]
    pub snap_preview: bool,
    #[serde(default)]
    pub adaptive_snap_animation: bool,
    #[serde(default)]
    pub move_cursor: bool,
    #[serde(default)]
    pub app_switch: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrokerActionCapabilitiesWire {
    #[serde(default)]
    snap_halves: bool,
    #[serde(default)]
    snap_quarters: bool,
    #[serde(default)]
    maximize: bool,
    #[serde(default)]
    minimize: bool,
    #[serde(default)]
    minimize_all: bool,
    #[serde(default)]
    close: bool,
    workspace: Option<bool>,
    existing_workspace: Option<bool>,
    #[serde(default)]
    dynamic_workspace: bool,
    monitor_move: Option<bool>,
    existing_monitor: Option<bool>,
    #[serde(default)]
    free_move: bool,
    #[serde(default)]
    free_resize: bool,
    #[serde(default)]
    axis_resize: bool,
    #[serde(default)]
    pinch: bool,
    #[serde(default)]
    hud: bool,
    #[serde(default)]
    animation: bool,
    #[serde(default)]
    live_preview: bool,
    #[serde(default)]
    snap_preview: bool,
    #[serde(default)]
    adaptive_snap_animation: bool,
    #[serde(default)]
    move_cursor: bool,
    #[serde(default)]
    app_switch: bool,
}

impl<'de> Deserialize<'de> for BrokerActionCapabilities {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = BrokerActionCapabilitiesWire::deserialize(deserializer)?;
        let workspace = merge_capability_alias(
            "workspace",
            wire.workspace,
            "existingWorkspace",
            wire.existing_workspace,
        )
        .map_err(D::Error::custom)?;
        let monitor_move = merge_capability_alias(
            "monitorMove",
            wire.monitor_move,
            "existingMonitor",
            wire.existing_monitor,
        )
        .map_err(D::Error::custom)?;
        Ok(Self {
            snap_halves: wire.snap_halves,
            snap_quarters: wire.snap_quarters,
            maximize: wire.maximize,
            minimize: wire.minimize,
            minimize_all: wire.minimize_all,
            close: wire.close,
            workspace,
            dynamic_workspace: wire.dynamic_workspace,
            monitor_move,
            free_move: wire.free_move,
            free_resize: wire.free_resize,
            axis_resize: wire.axis_resize,
            pinch: wire.pinch,
            hud: wire.hud,
            animation: wire.animation,
            live_preview: wire.live_preview,
            snap_preview: wire.snap_preview,
            adaptive_snap_animation: wire.adaptive_snap_animation,
            move_cursor: wire.move_cursor,
            app_switch: wire.app_switch,
        })
    }
}

fn merge_capability_alias(
    canonical_name: &str,
    canonical: Option<bool>,
    legacy_name: &str,
    legacy: Option<bool>,
) -> Result<bool, String> {
    match (canonical, legacy) {
        (Some(left), Some(right)) if left != right => Err(format!(
            "conflicting capability fields `{canonical_name}` and `{legacy_name}`"
        )),
        (Some(value), _) | (_, Some(value)) => Ok(value),
        (None, None) => Ok(false),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrokerCapabilities {
    pub protocol_version: u32,
    pub generation: String,
    #[serde(default)]
    pub extension_version: String,
    #[serde(default)]
    pub input_proxy_generation: String,
    #[serde(default)]
    pub advanced_events: bool,
    #[serde(default)]
    pub modifiers: bool,
    #[serde(default)]
    pub rebaseline_feedback: bool,
    #[serde(default, alias = "nativeGestureArbitration")]
    pub contact_arbitration: GestureArbitrationCapabilities,
    #[serde(default)]
    pub actions: BrokerActionCapabilities,
}

impl BrokerCapabilities {
    pub fn supports_advanced(&self) -> bool {
        self.protocol_version == PROTOCOL_VERSION
            && !self.generation.trim().is_empty()
            && !self.extension_version.trim().is_empty()
            && self.input_proxy_generation == INPUT_PROXY_GENERATION
            && self.advanced_events
            && self.modifiers
            && self.rebaseline_feedback
            && self.contact_arbitration.two_finger
            && self.actions.snap_preview
            && self.actions.adaptive_snap_animation
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdvancedConfigEnvelope {
    pub version: u32,
    pub advanced_enabled: bool,
    pub settings: AdvancedConfig,
}

impl AdvancedConfigEnvelope {
    pub fn new(settings: AdvancedConfig) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            advanced_enabled: settings.enabled && settings.gestures_enabled,
            settings,
        }
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum GestureEventDto {
    Began {
        version: u32,
        contacts: u32,
    },
    Raw {
        version: u32,
        dx: f64,
        dy: f64,
    },
    Updated {
        version: u32,
        direction: SwipeDirectionDto,
        progress: f64,
    },
    Completed {
        version: u32,
        direction: SwipeDirectionDto,
    },
    Cancelled {
        version: u32,
    },
    HoldEngaged {
        version: u32,
    },
    HoldUpdated {
        version: u32,
        direction: Option<DesktopDirectionDto>,
        progress: f64,
        aim_steps: i32,
    },
    DesktopMove {
        version: u32,
        direction: DesktopDirectionDto,
    },
    DesktopHoldCommit {
        version: u32,
        steps: i32,
    },
    MonitorMoveUpdated {
        version: u32,
        direction: Option<MonitorDirectionDto>,
        progress: f64,
    },
    MonitorMove {
        version: u32,
        direction: MonitorDirectionDto,
    },
    FreeMoveBegan {
        version: u32,
    },
    FreeMoveDelta {
        version: u32,
        dx: f64,
        dy: f64,
        scale: f64,
    },
    FreeMoveEnded {
        version: u32,
        was_tap: bool,
        cancelled: bool,
    },
    PinchUpdated {
        version: u32,
        outward: bool,
        progress: f64,
    },
    PinchOut {
        version: u32,
    },
    PinchIn {
        version: u32,
    },
    AxisResizeBegan {
        version: u32,
        horizontal: bool,
    },
    AxisResizeDelta {
        version: u32,
        factor: f64,
        horizontal: bool,
    },
    AxisResizeEnded {
        version: u32,
        cancelled: bool,
    },
}

impl GestureEventDto {
    pub fn from_event(event: &GestureEvent) -> Self {
        let version = PROTOCOL_VERSION;
        match *event {
            GestureEvent::Began { contacts } => Self::Began {
                version,
                contacts: contacts.try_into().unwrap_or(u32::MAX),
            },
            GestureEvent::Raw { dx, dy } => Self::Raw { version, dx, dy },
            GestureEvent::Updated {
                direction,
                progress,
            } => Self::Updated {
                version,
                direction: direction.into(),
                progress,
            },
            GestureEvent::Completed(direction) => Self::Completed {
                version,
                direction: direction.into(),
            },
            GestureEvent::Cancelled => Self::Cancelled { version },
            GestureEvent::HoldEngaged => Self::HoldEngaged { version },
            GestureEvent::HoldUpdated {
                direction,
                progress,
                aim_steps,
            } => Self::HoldUpdated {
                version,
                direction: direction.map(Into::into),
                progress,
                aim_steps,
            },
            GestureEvent::DesktopMove(direction) => Self::DesktopMove {
                version,
                direction: direction.into(),
            },
            GestureEvent::DesktopHoldCommit(steps) => Self::DesktopHoldCommit { version, steps },
            GestureEvent::MonitorMoveUpdated {
                direction,
                progress,
            } => Self::MonitorMoveUpdated {
                version,
                direction: direction.map(Into::into),
                progress,
            },
            GestureEvent::MonitorMove(direction) => Self::MonitorMove {
                version,
                direction: direction.into(),
            },
            GestureEvent::FreeMoveBegan => Self::FreeMoveBegan { version },
            GestureEvent::FreeMoveDelta { dx, dy, scale } => Self::FreeMoveDelta {
                version,
                dx,
                dy,
                scale,
            },
            GestureEvent::FreeMoveEnded { was_tap, cancelled } => Self::FreeMoveEnded {
                version,
                was_tap,
                cancelled,
            },
            GestureEvent::PinchUpdated { outward, progress } => Self::PinchUpdated {
                version,
                outward,
                progress,
            },
            GestureEvent::PinchOut => Self::PinchOut { version },
            GestureEvent::PinchIn => Self::PinchIn { version },
            GestureEvent::AxisResizeBegan { horizontal } => Self::AxisResizeBegan {
                version,
                horizontal,
            },
            GestureEvent::AxisResizeDelta { factor, horizontal } => Self::AxisResizeDelta {
                version,
                factor,
                horizontal,
            },
            GestureEvent::AxisResizeEnded { cancelled } => {
                Self::AxisResizeEnded { version, cancelled }
            }
        }
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    pub fn is_begin(&self) -> bool {
        matches!(self, Self::Began { .. } | Self::FreeMoveBegan { .. })
    }

    pub fn is_cancel(&self) -> bool {
        matches!(
            self,
            Self::Cancelled { .. }
                | Self::FreeMoveEnded {
                    cancelled: true,
                    ..
                }
                | Self::AxisResizeEnded {
                    cancelled: true,
                    ..
                }
        )
    }

    pub fn is_commit(&self) -> bool {
        matches!(
            self,
            Self::Completed { .. }
                | Self::DesktopHoldCommit { .. }
                | Self::MonitorMove { .. }
                | Self::FreeMoveEnded {
                    cancelled: false,
                    ..
                }
                | Self::PinchOut { .. }
                | Self::PinchIn { .. }
                | Self::AxisResizeEnded {
                    cancelled: false,
                    ..
                }
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SwipeDirectionDto {
    None,
    Left,
    Right,
    Up,
    Down,
    UpLeft,
    UpRight,
    DownLeft,
    DownRight,
}

impl From<SwipeDirection> for SwipeDirectionDto {
    fn from(value: SwipeDirection) -> Self {
        match value {
            SwipeDirection::None => Self::None,
            SwipeDirection::Left => Self::Left,
            SwipeDirection::Right => Self::Right,
            SwipeDirection::Up => Self::Up,
            SwipeDirection::Down => Self::Down,
            SwipeDirection::UpLeft => Self::UpLeft,
            SwipeDirection::UpRight => Self::UpRight,
            SwipeDirection::DownLeft => Self::DownLeft,
            SwipeDirection::DownRight => Self::DownRight,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DesktopDirectionDto {
    Left,
    Right,
}

impl From<DesktopDirection> for DesktopDirectionDto {
    fn from(value: DesktopDirection) -> Self {
        match value {
            DesktopDirection::Left => Self::Left,
            DesktopDirection::Right => Self::Right,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MonitorDirectionDto {
    Left,
    Right,
    Up,
    Down,
}

impl From<MonitorDirection> for MonitorDirectionDto {
    fn from(value: MonitorDirection) -> Self {
        match value {
            MonitorDirection::Left => Self::Left,
            MonitorDirection::Right => Self::Right,
            MonitorDirection::Up => Self::Up,
            MonitorDirection::Down => Self::Down,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportAvailability {
    Connecting,
    Configuring(BrokerCapabilities),
    Available(BrokerCapabilities),
    Unavailable(String),
}

impl TransportAvailability {
    pub fn capabilities(&self) -> Option<&BrokerCapabilities> {
        match self {
            Self::Available(capabilities) => Some(capabilities),
            Self::Connecting | Self::Configuring(_) | Self::Unavailable(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportNotice {
    Availability(TransportAvailability),
    BeginAccepted {
        session_id: String,
    },
    Configured {
        config_json: String,
    },
    BeginRejected {
        session_id: String,
        detail: String,
    },
    ProbeResolved {
        request_id: String,
        target_token: Option<String>,
        detail: String,
    },
    GestureFinished {
        session_id: String,
    },
    /// The current gesture failed closed. Transport health is reported only
    /// through `Availability`, so this notice must not take the broker offline.
    GestureFailed {
        session_id: String,
        detail: String,
    },
    Modifiers {
        monitor: bool,
        escape: bool,
    },
    Rebaseline {
        session_id: String,
        direction: SwipeDirection,
    },
}

#[derive(Debug, Clone)]
enum Outbound {
    Configure {
        config_json: String,
    },
    ProbeTarget {
        request_id: String,
    },
    Begin {
        session_id: String,
        sequence: u64,
        target_token: String,
        config_json: String,
        event_json: String,
    },
    Update {
        session_id: String,
        sequence: u64,
        event_json: String,
    },
    Commit {
        session_id: String,
        sequence: u64,
        event_json: String,
    },
    Cancel {
        session_id: String,
        sequence: u64,
        reason: String,
    },
    Shutdown,
}

impl Outbound {
    fn session_id(&self) -> Option<&str> {
        match self {
            Self::Begin { session_id, .. }
            | Self::Update { session_id, .. }
            | Self::Commit { session_id, .. }
            | Self::Cancel { session_id, .. } => Some(session_id),
            Self::Configure { .. } | Self::ProbeTarget { .. } | Self::Shutdown => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitError {
    QueueFull,
    Closed,
}

struct QueueState {
    items: VecDeque<Outbound>,
    live_sessions: BTreeSet<String>,
    closed: bool,
}

struct SharedQueue {
    state: Mutex<QueueState>,
    changed: Condvar,
    modifier_poll_requested: AtomicBool,
    capacity: usize,
}

enum QueuePoll {
    Item(Outbound),
    Timeout,
    Closed,
}

impl SharedQueue {
    fn discard_session(&self, session_id: &str) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.live_sessions.remove(session_id);
        state
            .items
            .retain(|item| item.session_id() != Some(session_id));
    }
}

impl SharedQueue {
    fn new(capacity: usize) -> Self {
        Self {
            state: Mutex::new(QueueState {
                items: VecDeque::with_capacity(capacity),
                live_sessions: BTreeSet::new(),
                closed: false,
            }),
            changed: Condvar::new(),
            modifier_poll_requested: AtomicBool::new(false),
            capacity,
        }
    }

    fn request_modifier_poll(&self) {
        self.modifier_poll_requested.store(true, Ordering::Release);
        self.changed.notify_one();
    }

    fn take_modifier_poll_request(&self) -> bool {
        self.modifier_poll_requested.swap(false, Ordering::AcqRel)
    }

    fn push(&self, item: Outbound) -> Result<(), SubmitError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.closed {
            return Err(SubmitError::Closed);
        }
        let terminal_session = match &item {
            Outbound::Begin { session_id, .. } => {
                state.live_sessions.insert(session_id.clone());
                None
            }
            Outbound::Update { session_id, .. } => {
                if !state.live_sessions.contains(session_id) {
                    return Err(SubmitError::Closed);
                }
                None
            }
            Outbound::Commit { session_id, .. } | Outbound::Cancel { session_id, .. } => {
                if !state.live_sessions.contains(session_id) {
                    return Err(SubmitError::Closed);
                }
                Some(session_id.clone())
            }
            Outbound::Configure { .. } | Outbound::ProbeTarget { .. } | Outbound::Shutdown => None,
        };
        // Keep one slot available for the session's Commit/Cancel. An update
        // overflow is recoverable only if the producer can still enqueue the
        // terminal cancellation that makes the Shell restore its window.
        if matches!(item, Outbound::Update { .. })
            && state.items.len() >= self.capacity.saturating_sub(1)
        {
            return Err(SubmitError::QueueFull);
        }
        if state.items.len() >= self.capacity {
            if let Outbound::Begin { session_id, .. } = &item {
                state.live_sessions.remove(session_id);
            }
            return Err(SubmitError::QueueFull);
        }
        if let Some(session_id) = terminal_session {
            state.live_sessions.remove(&session_id);
        }
        state.items.push_back(item);
        self.changed.notify_one();
        Ok(())
    }

    fn pop_timeout(&self, timeout: Duration) -> QueuePoll {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.items.is_empty() && !state.closed {
            let (next, _) = self
                .changed
                .wait_timeout(state, timeout)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state = next;
        }
        if let Some(item) = state.items.pop_front() {
            QueuePoll::Item(item)
        } else if state.closed {
            QueuePoll::Closed
        } else {
            QueuePoll::Timeout
        }
    }

    fn close(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.closed = true;
        self.changed.notify_all();
    }
}

pub struct AdvancedTransport {
    queue: Arc<SharedQueue>,
    notices: std::sync::mpsc::Receiver<TransportNotice>,
    worker: Option<JoinHandle<()>>,
}

impl AdvancedTransport {
    pub fn start(logger: RingLogger) -> std::io::Result<Self> {
        Self::start_with_capacity(logger, QUEUE_CAPACITY)
    }

    fn start_with_capacity(logger: RingLogger, capacity: usize) -> std::io::Result<Self> {
        let queue = Arc::new(SharedQueue::new(capacity));
        let worker_queue = Arc::clone(&queue);
        let (notice_sender, notices) = std::sync::mpsc::channel();
        let worker = thread::Builder::new()
            .name("ThreeFingerDrag GNOME D-Bus".into())
            .spawn(move || run_worker(worker_queue, notice_sender, logger))?;
        Ok(Self {
            queue,
            notices,
            worker: Some(worker),
        })
    }

    pub fn request_modifier_poll(&self) {
        self.queue.request_modifier_poll();
    }

    pub fn try_notice(&self) -> Option<TransportNotice> {
        self.notices.try_recv().ok()
    }

    #[cfg(test)]
    pub(super) fn from_notice_receiver_for_test(
        notices: std::sync::mpsc::Receiver<TransportNotice>,
    ) -> Self {
        let queue = Arc::new(SharedQueue::new(1));
        queue.close();
        Self {
            queue,
            notices,
            worker: None,
        }
    }

    pub fn begin(
        &self,
        session_id: String,
        sequence: u64,
        target_token: String,
        config_json: String,
        event_json: String,
    ) -> Result<(), SubmitError> {
        self.queue.push(Outbound::Begin {
            session_id,
            sequence,
            target_token,
            config_json,
            event_json,
        })
    }

    pub fn probe_target(&self, request_id: String) -> Result<(), SubmitError> {
        self.queue.push(Outbound::ProbeTarget { request_id })
    }

    pub fn configure(&self, config_json: String) -> Result<(), SubmitError> {
        self.queue.push(Outbound::Configure { config_json })
    }

    pub fn update(
        &self,
        session_id: String,
        sequence: u64,
        event_json: String,
    ) -> Result<(), SubmitError> {
        self.queue.push(Outbound::Update {
            session_id,
            sequence,
            event_json,
        })
    }

    pub fn commit(
        &self,
        session_id: String,
        sequence: u64,
        event_json: String,
    ) -> Result<(), SubmitError> {
        self.queue.push(Outbound::Commit {
            session_id,
            sequence,
            event_json,
        })
    }

    pub fn cancel(
        &self,
        session_id: String,
        sequence: u64,
        reason: impl Into<String>,
    ) -> Result<(), SubmitError> {
        self.queue.push(Outbound::Cancel {
            session_id,
            sequence,
            reason: reason.into(),
        })
    }

    pub fn stop(&mut self) {
        let _ = self.queue.push(Outbound::Shutdown);
        self.queue.close();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for AdvancedTransport {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run_worker(
    queue: Arc<SharedQueue>,
    notices: std::sync::mpsc::Sender<TransportNotice>,
    logger: RingLogger,
) {
    run_worker_with_connector(queue, notices, logger, || {
        DbusBroker::connect()
            .map(|broker| Box::new(broker) as Box<dyn BrokerClient>)
            .map_err(|error| format!("GNOME 扩展 D-Bus 不可用：{error}"))
    });
}

fn run_worker_with_connector<F>(
    queue: Arc<SharedQueue>,
    notices: std::sync::mpsc::Sender<TransportNotice>,
    logger: RingLogger,
    mut connector: F,
) where
    F: FnMut() -> Result<Box<dyn BrokerClient>, String>,
{
    let _ = notices.send(TransportNotice::Availability(
        TransportAvailability::Connecting,
    ));
    let mut broker = None;
    let mut active_sessions = BTreeSet::<String>::new();
    let mut last_modifier_poll = Instant::now() - MODIFIER_POLL_INTERVAL;
    let mut last_connect_attempt = Instant::now() - RECONNECT_INTERVAL;
    let mut configured = false;

    loop {
        if broker.is_none() && last_connect_attempt.elapsed() >= RECONNECT_INTERVAL {
            last_connect_attempt = Instant::now();
            match connect_and_handshake(&mut connector) {
                Ok((connected, capabilities)) => {
                    let _ = notices.send(TransportNotice::Availability(
                        TransportAvailability::Configuring(capabilities),
                    ));
                    broker = Some(connected);
                    configured = false;
                }
                Err(detail) => {
                    let _ = notices.send(TransportNotice::Availability(
                        TransportAvailability::Unavailable(detail),
                    ));
                }
            }
        }

        if broker.is_some()
            && last_modifier_poll.elapsed() >= MODIFIER_POLL_INTERVAL
            && queue.take_modifier_poll_request()
        {
            if let Some(connected) = broker.as_ref() {
                match publish_modifiers(connected.as_ref(), &notices) {
                    Ok(()) => last_modifier_poll = Instant::now(),
                    Err(error) => {
                        let detail = format!("GNOME 扩展修饰键状态不可用：{error}");
                        let _ = notices.send(TransportNotice::Availability(
                            TransportAvailability::Unavailable(detail),
                        ));
                        for session_id in &active_sessions {
                            queue.discard_session(session_id);
                            let _ = notices.send(TransportNotice::GestureFailed {
                                session_id: session_id.clone(),
                                detail: "GNOME 扩展修饰键读取失败，手势已取消。".into(),
                            });
                        }
                        broker = None;
                        configured = false;
                        active_sessions.clear();
                        last_connect_attempt = Instant::now();
                    }
                }
            }
        }

        let message = match queue.pop_timeout(MODIFIER_POLL_INTERVAL) {
            QueuePoll::Item(message) => message,
            QueuePoll::Timeout => continue,
            QueuePoll::Closed => break,
        };
        if matches!(message, Outbound::Shutdown) {
            break;
        }

        if broker.is_none() {
            let detail = "GNOME 扩展尚未完成能力握手。".to_owned();
            fail_and_discard(&queue, &notices, &message, detail);
            continue;
        }

        if !configured && !matches!(message, Outbound::Configure { .. }) {
            let detail = "GNOME 扩展尚未确认 Configure，拒绝发送手势。".to_owned();
            fail_and_discard(&queue, &notices, &message, detail);
            continue;
        }

        let result = broker.as_ref().expect("broker established").send(&message);
        match result {
            Ok(SendOutcome::ProbeAccepted {
                target_token,
                detail,
            }) => {
                if let Outbound::ProbeTarget { request_id } = &message {
                    let _ = notices.send(TransportNotice::ProbeResolved {
                        request_id: request_id.clone(),
                        target_token: Some(target_token),
                        detail,
                    });
                } else {
                    fail_and_discard(
                        &queue,
                        &notices,
                        &message,
                        "GNOME 扩展对非 Probe 请求返回了目标令牌。".into(),
                    );
                }
            }
            Ok(outcome @ (SendOutcome::Accepted | SendOutcome::AcceptedWithRebaseline(_))) => {
                if let SendOutcome::AcceptedWithRebaseline(direction) = outcome {
                    if let Some(session_id) = message.session_id() {
                        let _ = notices.send(TransportNotice::Rebaseline {
                            session_id: session_id.to_owned(),
                            direction,
                        });
                    }
                }
                if let Outbound::Configure { config_json } = &message {
                    configured = true;
                    let _ = notices.send(TransportNotice::Configured {
                        config_json: config_json.clone(),
                    });
                } else if let Outbound::Begin { session_id, .. } = &message {
                    active_sessions.insert(session_id.clone());
                    let _ = notices.send(TransportNotice::BeginAccepted {
                        session_id: session_id.clone(),
                    });
                } else if matches!(message, Outbound::Commit { .. } | Outbound::Cancel { .. }) {
                    if let Some(session_id) = message.session_id() {
                        active_sessions.remove(session_id);
                        let _ = notices.send(TransportNotice::GestureFinished {
                            session_id: session_id.to_owned(),
                        });
                    }
                }
            }
            Ok(SendOutcome::Rejected(detail)) => {
                if let Outbound::ProbeTarget { request_id } = &message {
                    let _ = notices.send(TransportNotice::ProbeResolved {
                        request_id: request_id.clone(),
                        target_token: None,
                        detail,
                    });
                } else if let Outbound::Begin { session_id, .. } = &message {
                    queue.discard_session(session_id);
                    let _ = notices.send(TransportNotice::BeginRejected {
                        session_id: session_id.clone(),
                        detail,
                    });
                } else {
                    reject_and_discard(&queue, &notices, &message, detail, &mut active_sessions);
                    if matches!(message, Outbound::Configure { .. }) {
                        let _ = notices.send(TransportNotice::Availability(
                            TransportAvailability::Unavailable(
                                "GNOME 扩展拒绝 Configure；进阶手势保持关闭。".into(),
                            ),
                        ));
                        broker = None;
                        configured = false;
                        active_sessions.clear();
                        last_connect_attempt = Instant::now();
                    }
                }
            }
            Err(error) => {
                let detail = format!("GNOME 扩展传输失败：{error}");
                logger.record(&detail);
                fail_and_discard(&queue, &notices, &message, detail.clone());
                let _ = notices.send(TransportNotice::Availability(
                    TransportAvailability::Unavailable(detail),
                ));
                broker = None;
                configured = false;
                active_sessions.clear();
                last_connect_attempt = Instant::now();
            }
        }
    }
}

fn connect_and_handshake<F>(
    connector: &mut F,
) -> Result<(Box<dyn BrokerClient>, BrokerCapabilities), String>
where
    F: FnMut() -> Result<Box<dyn BrokerClient>, String>,
{
    let broker = connector()?;
    let capabilities = broker
        .capabilities()
        .map_err(|error| format!("GNOME 扩展能力握手失败：{error}"))?;
    if !capabilities.supports_advanced() {
        return Err(capability_error(&capabilities));
    }
    Ok((broker, capabilities))
}

fn fail_and_discard(
    queue: &SharedQueue,
    notices: &std::sync::mpsc::Sender<TransportNotice>,
    message: &Outbound,
    detail: String,
) {
    if let Some(session_id) = message.session_id() {
        queue.discard_session(session_id);
    }
    fail_message(notices, message, detail);
}

fn reject_and_discard(
    queue: &SharedQueue,
    notices: &std::sync::mpsc::Sender<TransportNotice>,
    message: &Outbound,
    detail: String,
    active_sessions: &mut BTreeSet<String>,
) {
    if let Some(session_id) = message.session_id() {
        active_sessions.remove(session_id);
    }
    fail_and_discard(queue, notices, message, detail);
}

fn publish_modifiers(
    broker: &dyn BrokerClient,
    notices: &std::sync::mpsc::Sender<TransportNotice>,
) -> Result<(), String> {
    let (_reserved, monitor, escape) = broker.modifiers()?;
    let _ = notices.send(TransportNotice::Modifiers { monitor, escape });
    Ok(())
}

fn capability_error(capabilities: &BrokerCapabilities) -> String {
    if capabilities.protocol_version != PROTOCOL_VERSION {
        return format!(
            "GNOME 扩展协议版本不匹配：需要 {PROTOCOL_VERSION}，得到 {}。",
            capabilities.protocol_version
        );
    }
    if capabilities.generation.trim().is_empty() {
        return "GNOME 扩展未提供 generation，拒绝建立不安全会话。".into();
    }
    if capabilities.extension_version.trim().is_empty() {
        return "GNOME 扩展未提供 extensionVersion，拒绝建立不可诊断的会话。".into();
    }
    if capabilities.input_proxy_generation != INPUT_PROXY_GENERATION {
        return format!(
            "GNOME 扩展输入代理构建不匹配：需要 {INPUT_PROXY_GENERATION}，得到 {}。为保护原生触控板，未建立设备独占。",
            capabilities.input_proxy_generation
        );
    }
    if !capabilities.advanced_events {
        return "GNOME 扩展未声明 advancedEvents 能力。".into();
    }
    if !capabilities.modifiers {
        return "GNOME 扩展未声明 modifiers 能力。".into();
    }
    if !capabilities.rebaseline_feedback {
        return "GNOME 扩展未声明 rebaselineFeedback 能力。".into();
    }
    if !capabilities.contact_arbitration.two_finger {
        return "GNOME 扩展未声明 twoFinger 原生手势仲裁能力；为保留两指滚动，进阶手势保持关闭。"
            .into();
    }
    "GNOME 扩展能力不足。".into()
}

fn fail_message(
    notices: &std::sync::mpsc::Sender<TransportNotice>,
    message: &Outbound,
    detail: String,
) {
    if let Outbound::ProbeTarget { request_id } = message {
        let _ = notices.send(TransportNotice::ProbeResolved {
            request_id: request_id.clone(),
            target_token: None,
            detail,
        });
    } else if let Some(session_id) = message.session_id() {
        let _ = notices.send(TransportNotice::GestureFailed {
            session_id: session_id.to_owned(),
            detail,
        });
    }
}

enum SendOutcome {
    Accepted,
    AcceptedWithRebaseline(SwipeDirection),
    ProbeAccepted {
        target_token: String,
        detail: String,
    },
    Rejected(String),
}

trait BrokerClient: Send {
    fn capabilities(&self) -> Result<BrokerCapabilities, String>;
    fn modifiers(&self) -> Result<(bool, bool, bool), String>;
    fn send(&self, message: &Outbound) -> Result<SendOutcome, String>;
}

struct DbusBroker {
    proxy: zbus::blocking::Proxy<'static>,
}

impl DbusBroker {
    fn connect() -> Result<Self, zbus::Error> {
        let connection = zbus::blocking::connection::Builder::session()?
            .method_timeout(METHOD_TIMEOUT)
            .build()?;
        let proxy = zbus::blocking::Proxy::new_owned(
            connection,
            BUS_NAME.to_owned(),
            OBJECT_PATH.to_owned(),
            INTERFACE_NAME.to_owned(),
        )?;
        Ok(Self { proxy })
    }

    fn capabilities(&self) -> Result<BrokerCapabilities, String> {
        let json: String = self
            .proxy
            .call("GetCapabilities", &())
            .map_err(|error| error.to_string())?;
        serde_json::from_str(&json).map_err(|error| format!("能力 JSON 无效：{error}"))
    }

    fn modifiers(&self) -> Result<(bool, bool, bool), zbus::Error> {
        self.proxy.call("GetModifiers", &())
    }

    fn send(&self, message: &Outbound) -> Result<SendOutcome, zbus::Error> {
        match message {
            Outbound::Configure { config_json } => {
                bool_outcome(self.proxy.call("Configure", &(config_json,))?)
            }
            Outbound::ProbeTarget { .. } => {
                let (accepted, target_token, detail): (bool, String, String) =
                    self.proxy.call("ProbeTarget", &())?;
                if accepted && !target_token.trim().is_empty() && target_token.len() <= 128 {
                    Ok(SendOutcome::ProbeAccepted {
                        target_token,
                        detail,
                    })
                } else {
                    Ok(SendOutcome::Rejected(if detail.is_empty() {
                        "GNOME 扩展未找到可管理的标题栏目标。".into()
                    } else {
                        detail
                    }))
                }
            }
            Outbound::Begin {
                session_id,
                sequence,
                target_token,
                config_json,
                event_json,
            } => {
                let (accepted, detail): (bool, String) = self.proxy.call(
                    "Begin",
                    &(session_id, *sequence, target_token, config_json, event_json),
                )?;
                Ok(if accepted {
                    SendOutcome::Accepted
                } else {
                    SendOutcome::Rejected(if detail.is_empty() {
                        "GNOME 扩展拒绝接管该原生手势。".into()
                    } else {
                        detail
                    })
                })
            }
            Outbound::Update {
                session_id,
                sequence,
                event_json,
            } => {
                let (accepted, rebaseline): (bool, String) = self
                    .proxy
                    .call("Update", &(session_id, *sequence, event_json))?;
                update_outcome(accepted, &rebaseline)
            }
            Outbound::Commit {
                session_id,
                sequence,
                event_json,
            } => bool_outcome(
                self.proxy
                    .call("Commit", &(session_id, *sequence, event_json))?,
            ),
            Outbound::Cancel {
                session_id,
                sequence,
                reason,
            } => bool_outcome(
                self.proxy
                    .call("Cancel", &(session_id, *sequence, reason))?,
            ),
            Outbound::Shutdown => Ok(SendOutcome::Accepted),
        }
    }
}

impl BrokerClient for DbusBroker {
    fn capabilities(&self) -> Result<BrokerCapabilities, String> {
        DbusBroker::capabilities(self)
    }

    fn modifiers(&self) -> Result<(bool, bool, bool), String> {
        DbusBroker::modifiers(self).map_err(|error| error.to_string())
    }

    fn send(&self, message: &Outbound) -> Result<SendOutcome, String> {
        DbusBroker::send(self, message).map_err(|error| error.to_string())
    }
}

fn bool_outcome(accepted: bool) -> Result<SendOutcome, zbus::Error> {
    Ok(if accepted {
        SendOutcome::Accepted
    } else {
        SendOutcome::Rejected("GNOME 扩展拒绝或丢失了手势会话。".into())
    })
}

fn update_outcome(accepted: bool, rebaseline: &str) -> Result<SendOutcome, zbus::Error> {
    if !accepted {
        return Ok(SendOutcome::Rejected(
            "GNOME 扩展拒绝或丢失了手势会话。".into(),
        ));
    }
    let direction = match rebaseline {
        "" => return Ok(SendOutcome::Accepted),
        "none" => SwipeDirection::None,
        "left" => SwipeDirection::Left,
        "right" => SwipeDirection::Right,
        "up" => SwipeDirection::Up,
        "down" => SwipeDirection::Down,
        "upLeft" => SwipeDirection::UpLeft,
        "upRight" => SwipeDirection::UpRight,
        "downLeft" => SwipeDirection::DownLeft,
        "downRight" => SwipeDirection::DownRight,
        value => {
            return Err(zbus::Error::Failure(format!(
                "GNOME 扩展返回了无效的 rebaseline 方向 `{value}`"
            )))
        }
    };
    Ok(SendOutcome::AcceptedWithRebaseline(direction))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockBroker {
        capabilities: BrokerCapabilities,
        reject_begin: bool,
        modifier_calls: Option<Arc<std::sync::atomic::AtomicUsize>>,
    }

    struct FailingTerminalBroker {
        capabilities: BrokerCapabilities,
    }

    impl BrokerClient for MockBroker {
        fn capabilities(&self) -> Result<BrokerCapabilities, String> {
            Ok(self.capabilities.clone())
        }

        fn modifiers(&self) -> Result<(bool, bool, bool), String> {
            if let Some(calls) = &self.modifier_calls {
                calls.fetch_add(1, Ordering::Relaxed);
            }
            Ok((false, false, false))
        }

        fn send(&self, message: &Outbound) -> Result<SendOutcome, String> {
            if matches!(message, Outbound::ProbeTarget { .. }) {
                Ok(SendOutcome::ProbeAccepted {
                    target_token: "mock-target-token".into(),
                    detail: "mock target".into(),
                })
            } else if self.reject_begin && matches!(message, Outbound::Begin { .. }) {
                Ok(SendOutcome::Rejected("not a window target".into()))
            } else {
                Ok(SendOutcome::Accepted)
            }
        }
    }

    impl BrokerClient for FailingTerminalBroker {
        fn capabilities(&self) -> Result<BrokerCapabilities, String> {
            Ok(self.capabilities.clone())
        }

        fn modifiers(&self) -> Result<(bool, bool, bool), String> {
            Ok((false, false, false))
        }

        fn send(&self, message: &Outbound) -> Result<SendOutcome, String> {
            if matches!(message, Outbound::ProbeTarget { .. }) {
                Ok(SendOutcome::ProbeAccepted {
                    target_token: "mock-target-token".into(),
                    detail: "mock target".into(),
                })
            } else if matches!(message, Outbound::Commit { .. }) {
                Err("session bus disconnected".into())
            } else {
                Ok(SendOutcome::Accepted)
            }
        }
    }

    fn supported_capabilities() -> BrokerCapabilities {
        BrokerCapabilities {
            protocol_version: PROTOCOL_VERSION,
            generation: "generation-1".into(),
            extension_version: "6".into(),
            input_proxy_generation: INPUT_PROXY_GENERATION.into(),
            advanced_events: true,
            modifiers: true,
            rebaseline_feedback: true,
            contact_arbitration: GestureArbitrationCapabilities {
                two_finger: true,
                five_finger: false,
            },
            actions: BrokerActionCapabilities {
                snap_preview: true,
                adaptive_snap_animation: true,
                ..BrokerActionCapabilities::default()
            },
        }
    }

    fn wait_for_available(
        notices: &std::sync::mpsc::Receiver<TransportNotice>,
    ) -> BrokerCapabilities {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let notice = notices
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("worker should publish availability without an outbound gesture");
            if let TransportNotice::Availability(TransportAvailability::Configuring(value)) = notice
            {
                return value;
            }
        }
    }

    #[test]
    fn capabilities_require_two_finger_arbitration() {
        let mut capabilities = BrokerCapabilities {
            protocol_version: PROTOCOL_VERSION,
            generation: "generation-1".into(),
            extension_version: "6".into(),
            input_proxy_generation: INPUT_PROXY_GENERATION.into(),
            advanced_events: true,
            modifiers: true,
            rebaseline_feedback: true,
            contact_arbitration: GestureArbitrationCapabilities {
                two_finger: false,
                five_finger: true,
            },
            actions: BrokerActionCapabilities {
                snap_preview: true,
                adaptive_snap_animation: true,
                ..BrokerActionCapabilities::default()
            },
        };
        assert!(!capabilities.supports_advanced());
        capabilities.contact_arbitration.two_finger = true;
        assert!(capabilities.supports_advanced());
        capabilities.actions.snap_preview = false;
        assert!(!capabilities.supports_advanced());
        capabilities.actions.snap_preview = true;
        capabilities.actions.adaptive_snap_animation = false;
        assert!(!capabilities.supports_advanced());
        capabilities.actions.adaptive_snap_animation = true;
        capabilities.rebaseline_feedback = false;
        assert!(!capabilities.supports_advanced());
    }

    #[test]
    fn stale_input_proxy_generation_fails_the_handshake() {
        let mut capabilities = supported_capabilities();
        capabilities.input_proxy_generation = "stale-input-proxy".into();

        assert!(!capabilities.supports_advanced());
        assert!(capability_error(&capabilities).contains(INPUT_PROXY_GENERATION));
    }

    #[test]
    fn capability_errors_explain_every_fail_closed_handshake_gate() {
        let mut capability = supported_capabilities();
        capability.protocol_version = 1;
        assert!(capability_error(&capability).contains("协议版本不匹配"));

        capability = supported_capabilities();
        capability.generation.clear();
        assert!(capability_error(&capability).contains("generation"));

        capability = supported_capabilities();
        capability.extension_version.clear();
        assert!(capability_error(&capability).contains("extensionVersion"));

        capability = supported_capabilities();
        capability.input_proxy_generation = "old".into();
        assert!(capability_error(&capability).contains(INPUT_PROXY_GENERATION));

        capability = supported_capabilities();
        capability.advanced_events = false;
        assert!(capability_error(&capability).contains("advancedEvents"));

        capability = supported_capabilities();
        capability.modifiers = false;
        assert!(capability_error(&capability).contains("modifiers"));

        capability = supported_capabilities();
        capability.rebaseline_feedback = false;
        assert!(capability_error(&capability).contains("rebaselineFeedback"));

        capability = supported_capabilities();
        capability.contact_arbitration.two_finger = false;
        assert!(capability_error(&capability).contains("twoFinger"));

        capability = supported_capabilities();
        capability.actions.snap_preview = false;
        assert_eq!(capability_error(&capability), "GNOME 扩展能力不足。");
    }

    #[test]
    fn probe_requests_are_bounded_and_correlated_before_begin() {
        let queue = SharedQueue::new(4);
        queue
            .push(Outbound::ProbeTarget {
                request_id: "probe-1".into(),
            })
            .unwrap();
        assert!(matches!(
            queue.pop_timeout(Duration::ZERO),
            QueuePoll::Item(Outbound::ProbeTarget { request_id }) if request_id == "probe-1"
        ));

        queue
            .push(Outbound::Begin {
                session_id: "gesture-1".into(),
                sequence: 1,
                target_token: "target-token-1".into(),
                config_json: "{}".into(),
                event_json: "{}".into(),
            })
            .unwrap();
        assert!(matches!(
            queue.pop_timeout(Duration::ZERO),
            QueuePoll::Item(Outbound::Begin { target_token, .. })
                if target_token == "target-token-1"
        ));
    }

    #[test]
    fn worker_handshakes_while_outbound_queue_is_empty() {
        let queue = Arc::new(SharedQueue::new(4));
        let worker_queue = Arc::clone(&queue);
        let (sender, notices) = std::sync::mpsc::channel();
        let capabilities = supported_capabilities();
        let worker = thread::spawn(move || {
            run_worker_with_connector(worker_queue, sender, RingLogger::default(), move || {
                Ok(Box::new(MockBroker {
                    capabilities: capabilities.clone(),
                    reject_begin: false,
                    modifier_calls: None,
                }))
            });
        });

        assert!(wait_for_available(&notices).supports_advanced());
        queue.close();
        worker.join().unwrap();
    }

    #[test]
    fn worker_correlates_a_probe_token_before_begin() {
        let queue = Arc::new(SharedQueue::new(8));
        let worker_queue = Arc::clone(&queue);
        let (sender, notices) = std::sync::mpsc::channel();
        let capabilities = supported_capabilities();
        let worker = thread::spawn(move || {
            run_worker_with_connector(worker_queue, sender, RingLogger::default(), move || {
                Ok(Box::new(MockBroker {
                    capabilities: capabilities.clone(),
                    reject_begin: false,
                    modifier_calls: None,
                }))
            });
        });
        wait_for_available(&notices);
        queue
            .push(Outbound::Configure {
                config_json: r#"{"version":2,"advancedEnabled":true,"settings":{}}"#.into(),
            })
            .unwrap();
        loop {
            if matches!(
                notices.recv_timeout(Duration::from_secs(1)),
                Ok(TransportNotice::Configured { .. })
            ) {
                break;
            }
        }
        queue
            .push(Outbound::ProbeTarget {
                request_id: "probe-correlated".into(),
            })
            .unwrap();
        let target_token = loop {
            if let TransportNotice::ProbeResolved {
                request_id,
                target_token: Some(target_token),
                ..
            } = notices.recv_timeout(Duration::from_secs(1)).unwrap()
            {
                assert_eq!(request_id, "probe-correlated");
                break target_token;
            }
        };
        queue
            .push(Outbound::Begin {
                session_id: "probe-gesture".into(),
                sequence: 1,
                target_token,
                config_json: "{}".into(),
                event_json: "{}".into(),
            })
            .unwrap();
        loop {
            if matches!(
                notices.recv_timeout(Duration::from_secs(1)),
                Ok(TransportNotice::BeginAccepted { session_id })
                    if session_id == "probe-gesture"
            ) {
                break;
            }
        }
        queue.close();
        worker.join().unwrap();
    }

    #[test]
    fn modifier_polling_is_demand_driven_instead_of_idle_dbus_traffic() {
        let queue = Arc::new(SharedQueue::new(4));
        let worker_queue = Arc::clone(&queue);
        let (sender, notices) = std::sync::mpsc::channel();
        let capabilities = supported_capabilities();
        let modifier_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let worker_calls = Arc::clone(&modifier_calls);
        let worker = thread::spawn(move || {
            run_worker_with_connector(worker_queue, sender, RingLogger::default(), move || {
                Ok(Box::new(MockBroker {
                    capabilities: capabilities.clone(),
                    reject_begin: false,
                    modifier_calls: Some(Arc::clone(&worker_calls)),
                }))
            });
        });

        wait_for_available(&notices);
        thread::sleep(Duration::from_millis(50));
        assert_eq!(modifier_calls.load(Ordering::Relaxed), 0);

        queue.request_modifier_poll();
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let notice = notices
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("an explicit request should publish modifiers");
            if matches!(notice, TransportNotice::Modifiers { .. }) {
                break;
            }
        }
        assert_eq!(modifier_calls.load(Ordering::Relaxed), 1);
        thread::sleep(Duration::from_millis(50));
        assert_eq!(modifier_calls.load(Ordering::Relaxed), 1);

        queue.close();
        worker.join().unwrap();
    }

    #[test]
    fn gestures_are_rejected_until_configure_is_acknowledged() {
        let queue = Arc::new(SharedQueue::new(4));
        let worker_queue = Arc::clone(&queue);
        let (sender, notices) = std::sync::mpsc::channel();
        let capabilities = supported_capabilities();
        let worker = thread::spawn(move || {
            run_worker_with_connector(worker_queue, sender, RingLogger::default(), move || {
                Ok(Box::new(MockBroker {
                    capabilities: capabilities.clone(),
                    reject_begin: false,
                    modifier_calls: None,
                }))
            });
        });
        wait_for_available(&notices);
        queue
            .push(Outbound::Begin {
                session_id: "before-configure".into(),
                sequence: 1,
                target_token: "target-token".into(),
                config_json: "{}".into(),
                event_json: "{}".into(),
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if let TransportNotice::GestureFailed { session_id, .. } = notices
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("unconfigured Begin should fail closed")
            {
                assert_eq!(session_id, "before-configure");
                break;
            }
        }
        queue.close();
        worker.join().unwrap();
    }

    #[test]
    fn expected_begin_rejection_keeps_broker_available_and_discards_session() {
        let queue = Arc::new(SharedQueue::new(4));
        let worker_queue = Arc::clone(&queue);
        let (sender, notices) = std::sync::mpsc::channel();
        let capabilities = supported_capabilities();
        let worker = thread::spawn(move || {
            run_worker_with_connector(worker_queue, sender, RingLogger::default(), move || {
                Ok(Box::new(MockBroker {
                    capabilities: capabilities.clone(),
                    reject_begin: true,
                    modifier_calls: None,
                }))
            });
        });
        wait_for_available(&notices);
        queue
            .push(Outbound::Configure {
                config_json: r#"{"version":2,"advancedEnabled":true,"settings":{}}"#.into(),
            })
            .unwrap();
        let configured_deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let notice = notices
                .recv_timeout(configured_deadline.saturating_duration_since(Instant::now()))
                .expect("mock broker should acknowledge Configure");
            if matches!(notice, TransportNotice::Configured { .. }) {
                break;
            }
        }
        queue
            .push(Outbound::Begin {
                session_id: "declined".into(),
                sequence: 1,
                target_token: "target-token".into(),
                config_json: "{}".into(),
                event_json: "{}".into(),
            })
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(1);
        let mut rejected = false;
        while Instant::now() < deadline && !rejected {
            match notices.recv_timeout(Duration::from_millis(50)) {
                Ok(TransportNotice::BeginRejected { session_id, .. }) => {
                    assert_eq!(session_id, "declined");
                    rejected = true;
                }
                Ok(TransportNotice::Availability(TransportAvailability::Unavailable(detail))) => {
                    panic!("ordinary arbitration rejection disabled broker: {detail}")
                }
                Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(error) => panic!("worker notice channel failed: {error}"),
            }
        }
        assert!(rejected);
        assert_eq!(
            queue.push(Outbound::Update {
                session_id: "declined".into(),
                sequence: 2,
                event_json: "{}".into(),
            }),
            Err(SubmitError::Closed)
        );
        queue.close();
        worker.join().unwrap();
    }

    #[test]
    fn rejected_terminal_messages_do_not_leak_active_sessions() {
        for terminal in [
            Outbound::Commit {
                session_id: "terminal".into(),
                sequence: 2,
                event_json: "{}".into(),
            },
            Outbound::Cancel {
                session_id: "terminal".into(),
                sequence: 2,
                reason: "test".into(),
            },
        ] {
            let queue = SharedQueue::new(4);
            let (sender, notices) = std::sync::mpsc::channel();
            let mut active_sessions = BTreeSet::from(["terminal".to_owned()]);

            reject_and_discard(
                &queue,
                &sender,
                &terminal,
                "broker rejected terminal message".into(),
                &mut active_sessions,
            );

            assert!(active_sessions.is_empty());
            assert!(matches!(
                notices.recv_timeout(Duration::from_millis(50)),
                Ok(TransportNotice::GestureFailed { session_id, .. })
                    if session_id == "terminal"
            ));
        }
    }

    #[test]
    fn terminal_transport_error_still_publishes_unavailable() {
        let queue = Arc::new(SharedQueue::new(4));
        let worker_queue = Arc::clone(&queue);
        let (sender, notices) = std::sync::mpsc::channel();
        let capabilities = supported_capabilities();
        let worker = thread::spawn(move || {
            run_worker_with_connector(worker_queue, sender, RingLogger::default(), move || {
                Ok(Box::new(FailingTerminalBroker {
                    capabilities: capabilities.clone(),
                }))
            });
        });
        wait_for_available(&notices);
        queue
            .push(Outbound::Configure {
                config_json: r#"{"version":2,"advancedEnabled":true,"settings":{}}"#.into(),
            })
            .unwrap();
        loop {
            if matches!(
                notices.recv_timeout(Duration::from_secs(1)),
                Ok(TransportNotice::Configured { .. })
            ) {
                break;
            }
        }
        queue
            .push(Outbound::Begin {
                session_id: "transport-error".into(),
                sequence: 1,
                target_token: "target-token".into(),
                config_json: "{}".into(),
                event_json: "{}".into(),
            })
            .unwrap();
        loop {
            if matches!(
                notices.recv_timeout(Duration::from_secs(1)),
                Ok(TransportNotice::BeginAccepted { session_id })
                    if session_id == "transport-error"
            ) {
                break;
            }
        }
        queue
            .push(Outbound::Commit {
                session_id: "transport-error".into(),
                sequence: 2,
                event_json: "{}".into(),
            })
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(1);
        let mut saw_gesture_failure = false;
        let mut saw_unavailable = false;
        while Instant::now() < deadline && !(saw_gesture_failure && saw_unavailable) {
            match notices.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(TransportNotice::GestureFailed { session_id, .. }) => {
                    assert_eq!(session_id, "transport-error");
                    saw_gesture_failure = true;
                }
                Ok(TransportNotice::Availability(TransportAvailability::Unavailable(detail))) => {
                    assert!(detail.contains("session bus disconnected"));
                    saw_unavailable = true;
                }
                Ok(_) => {}
                Err(error) => panic!("worker notice channel failed: {error}"),
            }
        }
        assert!(saw_gesture_failure);
        assert!(saw_unavailable);

        queue.close();
        worker.join().unwrap();
    }

    #[test]
    fn event_schema_is_flat_versioned_and_camel_case() {
        let json = GestureEventDto::from_event(&GestureEvent::HoldUpdated {
            direction: Some(DesktopDirection::Right),
            progress: 0.75,
            aim_steps: 2,
        })
        .to_json()
        .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&json).unwrap(),
            serde_json::json!({
                "version": 2,
                "kind": "holdUpdated",
                "direction": "right",
                "progress": 0.75,
                "aimSteps": 2
            })
        );
    }

    #[test]
    fn all_gesture_variants_map_to_protocol_events() {
        let events = vec![
            GestureEvent::Began { contacts: 2 },
            GestureEvent::Raw { dx: 0.1, dy: -0.2 },
            GestureEvent::Updated {
                direction: SwipeDirection::UpLeft,
                progress: 0.5,
            },
            GestureEvent::Completed(SwipeDirection::Right),
            GestureEvent::Cancelled,
            GestureEvent::HoldEngaged,
            GestureEvent::HoldUpdated {
                direction: None,
                progress: 0.0,
                aim_steps: 0,
            },
            GestureEvent::DesktopMove(DesktopDirection::Left),
            GestureEvent::DesktopHoldCommit(-2),
            GestureEvent::MonitorMoveUpdated {
                direction: Some(MonitorDirection::Down),
                progress: 0.4,
            },
            GestureEvent::MonitorMove(MonitorDirection::Up),
            GestureEvent::FreeMoveBegan,
            GestureEvent::FreeMoveDelta {
                dx: 0.01,
                dy: 0.02,
                scale: 1.1,
            },
            GestureEvent::FreeMoveEnded {
                was_tap: false,
                cancelled: false,
            },
            GestureEvent::PinchUpdated {
                outward: true,
                progress: 0.5,
            },
            GestureEvent::PinchOut,
            GestureEvent::PinchIn,
            GestureEvent::AxisResizeBegan { horizontal: true },
            GestureEvent::AxisResizeDelta {
                factor: 1.2,
                horizontal: false,
            },
            GestureEvent::AxisResizeEnded { cancelled: true },
        ];
        for event in events {
            let value = serde_json::to_value(GestureEventDto::from_event(&event)).unwrap();
            assert_eq!(value["version"], PROTOCOL_VERSION);
            assert!(value["kind"].is_string());
        }
    }

    #[test]
    fn only_true_terminal_events_commit() {
        assert!(
            !GestureEventDto::from_event(&GestureEvent::DesktopMove(DesktopDirection::Left))
                .is_commit()
        );
        assert!(
            GestureEventDto::from_event(&GestureEvent::Completed(SwipeDirection::Right))
                .is_commit()
        );
        assert!(GestureEventDto::from_event(&GestureEvent::PinchOut).is_commit());
        assert!(
            GestureEventDto::from_event(&GestureEvent::AxisResizeEnded { cancelled: false })
                .is_commit()
        );
        assert!(
            !GestureEventDto::from_event(&GestureEvent::AxisResizeEnded { cancelled: true })
                .is_commit()
        );
    }

    #[test]
    fn update_reply_carries_the_windows_chooser_rebaseline_direction() {
        assert!(matches!(
            update_outcome(true, "upLeft").unwrap(),
            SendOutcome::AcceptedWithRebaseline(SwipeDirection::UpLeft)
        ));
        assert!(matches!(
            update_outcome(true, "").unwrap(),
            SendOutcome::Accepted
        ));
        assert!(update_outcome(true, "sideways").is_err());
    }

    #[test]
    fn capability_schema_accepts_standard_and_legacy_arbitration_keys() {
        for key in ["contactArbitration", "nativeGestureArbitration"] {
            let value = serde_json::json!({
                "protocolVersion": 2,
                "generation": "g",
                "extensionVersion": "6",
                "inputProxyGeneration": INPUT_PROXY_GENERATION,
                "advancedEvents": true,
                "modifiers": true,
                "rebaselineFeedback": true,
                key: { "twoFinger": true, "fiveFinger": false },
                "actions": {
                    "snapHalves": true,
                    "snapQuarters": true,
                    "maximize": true,
                    "minimize": true,
                    "minimizeAll": true,
                    "close": false,
                    "existingWorkspace": true,
                    "dynamicWorkspace": false,
                    "existingMonitor": true,
                    "freeMove": false,
                    "freeResize": false,
                    "axisResize": false,
                    "pinch": true,
                    "hud": true,
                    "snapPreview": true,
                    "adaptiveSnapAnimation": true
                }
            });
            let parsed: BrokerCapabilities = serde_json::from_value(value).unwrap();
            assert!(parsed.supports_advanced());
            assert!(parsed.actions.snap_halves);
            assert!(parsed.actions.snap_quarters);
            assert!(parsed.actions.maximize);
            assert!(parsed.actions.minimize_all);
            assert!(parsed.actions.workspace);
            assert!(parsed.actions.monitor_move);
            assert!(!parsed.actions.close);
            assert!(parsed.actions.hud);
            assert!(parsed.actions.snap_preview);
            assert!(parsed.actions.adaptive_snap_animation);
        }
    }

    #[test]
    fn capability_schema_fails_closed_for_pre_v5_snap_motion_fields() {
        let value = serde_json::json!({
            "protocolVersion": 2,
            "generation": "v4-runtime",
            "extensionVersion": "6",
            "inputProxyGeneration": INPUT_PROXY_GENERATION,
            "advancedEvents": true,
            "modifiers": true,
            "rebaselineFeedback": true,
            "contactArbitration": { "twoFinger": true, "fiveFinger": false },
            "actions": {}
        });
        let parsed: BrokerCapabilities = serde_json::from_value(value).unwrap();
        assert!(!parsed.actions.snap_preview);
        assert!(!parsed.actions.adaptive_snap_animation);
        assert!(!parsed.supports_advanced());
    }

    #[test]
    fn action_capabilities_accept_equal_aliases_and_reject_conflicts() {
        let equal = serde_json::json!({
            "workspace": true,
            "existingWorkspace": true,
            "monitorMove": true,
            "existingMonitor": true
        });
        let parsed: BrokerActionCapabilities = serde_json::from_value(equal).unwrap();
        assert!(parsed.workspace);
        assert!(parsed.monitor_move);

        for conflicting in [
            serde_json::json!({"workspace": true, "existingWorkspace": false}),
            serde_json::json!({"monitorMove": false, "existingMonitor": true}),
        ] {
            assert!(serde_json::from_value::<BrokerActionCapabilities>(conflicting).is_err());
        }
    }

    #[test]
    fn capability_schema_rejects_duplicate_standard_and_legacy_arbitration_keys() {
        let value = serde_json::json!({
            "protocolVersion": 2,
            "generation": "g",
            "extensionVersion": "6",
            "inputProxyGeneration": INPUT_PROXY_GENERATION,
            "advancedEvents": true,
            "modifiers": true,
            "rebaselineFeedback": true,
            "contactArbitration": { "twoFinger": false, "fiveFinger": false },
            "nativeGestureArbitration": { "twoFinger": false, "fiveFinger": false },
            "actions": {}
        });
        assert!(serde_json::from_value::<BrokerCapabilities>(value).is_err());
    }

    #[test]
    fn config_envelope_has_version_and_settings() {
        let config = AdvancedConfig {
            enabled: true,
            ..AdvancedConfig::default()
        };
        let value = serde_json::to_value(AdvancedConfigEnvelope::new(config)).unwrap();
        assert_eq!(value["version"], PROTOCOL_VERSION);
        assert_eq!(value["advancedEnabled"], true);
        assert_eq!(value["settings"]["enabled"], true);
    }

    #[test]
    fn updates_preserve_protocol_order_and_deltas_until_terminal() {
        let queue = SharedQueue::new(8);
        queue
            .push(Outbound::Begin {
                session_id: "s".into(),
                sequence: 1,
                target_token: "target-token".into(),
                config_json: "{}".into(),
                event_json: "{}".into(),
            })
            .unwrap();
        assert!(matches!(
            queue.pop_timeout(Duration::ZERO),
            QueuePoll::Item(Outbound::Begin { .. })
        ));

        let updates = [
            GestureEvent::Raw { dx: 0.1, dy: 0.2 },
            GestureEvent::Updated {
                direction: SwipeDirection::Right,
                progress: 0.3,
            },
            GestureEvent::AxisResizeBegan { horizontal: true },
            GestureEvent::AxisResizeDelta {
                factor: 1.1,
                horizontal: true,
            },
            GestureEvent::FreeMoveDelta {
                dx: 1.0,
                dy: 2.0,
                scale: 1.0,
            },
            GestureEvent::FreeMoveDelta {
                dx: 3.0,
                dy: 4.0,
                scale: 1.2,
            },
            GestureEvent::DesktopMove(DesktopDirection::Right),
        ];
        let mut expected_updates = Vec::new();
        for (offset, event) in updates.iter().enumerate() {
            let sequence = u64::try_from(offset).unwrap() + 2;
            let event_json = GestureEventDto::from_event(event).to_json().unwrap();
            expected_updates.push((sequence, event_json.clone()));
            queue
                .push(Outbound::Update {
                    session_id: "s".into(),
                    sequence,
                    event_json,
                })
                .unwrap();
        }
        let terminal_json =
            GestureEventDto::from_event(&GestureEvent::Completed(SwipeDirection::Right))
                .to_json()
                .unwrap();
        queue
            .push(Outbound::Commit {
                session_id: "s".into(),
                sequence: 9,
                event_json: terminal_json.clone(),
            })
            .unwrap();

        for (expected_sequence, expected_json) in expected_updates {
            match queue.pop_timeout(Duration::ZERO) {
                QueuePoll::Item(Outbound::Update {
                    session_id,
                    sequence,
                    event_json,
                }) => {
                    assert_eq!(session_id, "s");
                    assert_eq!(sequence, expected_sequence);
                    assert_eq!(event_json, expected_json);
                }
                _ => panic!("every update must remain in bounded FIFO order"),
            }
        }
        assert!(matches!(
            queue.pop_timeout(Duration::ZERO),
            QueuePoll::Item(Outbound::Commit {
                session_id,
                sequence: 9,
                event_json,
            }) if session_id == "s" && event_json == terminal_json
        ));
        assert_eq!(
            queue.push(Outbound::Update {
                session_id: "s".into(),
                sequence: 10,
                event_json: "late".into(),
            }),
            Err(SubmitError::Closed)
        );
    }

    #[test]
    fn full_queue_rejects_critical_messages() {
        let queue = SharedQueue::new(1);
        queue
            .push(Outbound::Begin {
                session_id: "s".into(),
                sequence: 1,
                target_token: "target-token".into(),
                config_json: "{}".into(),
                event_json: "{}".into(),
            })
            .unwrap();
        assert_eq!(
            queue.push(Outbound::Update {
                session_id: "s".into(),
                sequence: 2,
                event_json: "bounded-update".into(),
            }),
            Err(SubmitError::QueueFull)
        );
        assert_eq!(
            queue.push(Outbound::Cancel {
                session_id: "s".into(),
                sequence: 3,
                reason: "test".into(),
            }),
            Err(SubmitError::QueueFull)
        );
    }

    #[test]
    fn update_pressure_reserves_a_terminal_slot() {
        let queue = SharedQueue::new(3);
        queue
            .push(Outbound::Begin {
                session_id: "s".into(),
                sequence: 1,
                target_token: "target-token".into(),
                config_json: "{}".into(),
                event_json: "{}".into(),
            })
            .unwrap();
        queue
            .push(Outbound::Update {
                session_id: "s".into(),
                sequence: 2,
                event_json: "first".into(),
            })
            .unwrap();
        assert_eq!(
            queue.push(Outbound::Update {
                session_id: "s".into(),
                sequence: 3,
                event_json: "overflow".into(),
            }),
            Err(SubmitError::QueueFull)
        );
        queue
            .push(Outbound::Cancel {
                session_id: "s".into(),
                sequence: 4,
                reason: "overflow".into(),
            })
            .expect("terminal slot must remain available");
    }
}
