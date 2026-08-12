use std::{
    ffi::c_void,
    io, mem,
    sync::{
        atomic::{AtomicIsize, Ordering},
        mpsc::{self, Sender},
        Arc, RwLock,
    },
    thread::{self, JoinHandle},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use windows_sys::Win32::{
    Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM},
    System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentProcessId},
    UI::{
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, VK_CONTROL, VK_ESCAPE, VK_MBUTTON, VK_MENU, VK_SHIFT,
        },
        Input::{
            RegisterRawInputDevices, HRAWINPUT, RAWINPUTDEVICE, RIDEV_DEVNOTIFY, RIDEV_INPUTSINK,
        },
        WindowsAndMessaging::{
            CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
            GetMessageW, GetWindowLongPtrW, KillTimer, PostMessageW, PostQuitMessage,
            RegisterClassExW, SetTimer, SetWindowLongPtrW, SetWindowsHookExW, TranslateMessage,
            UnhookWindowsHookEx, UnregisterClassW, CREATESTRUCTW, GWLP_USERDATA, HHOOK,
            HWND_MESSAGE, LLMHF_INJECTED, MSG, MSLLHOOKSTRUCT, WH_MOUSE_LL, WM_CLOSE, WM_DESTROY,
            WM_INPUT, WM_INPUT_DEVICE_CHANGE, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_TIMER,
            WNDCLASSEXW,
        },
    },
};

use crate::{
    advanced::{AdvancedRuntime, AdvancedRuntimeStatus},
    contacts::{AssembledContacts, ContactAssembler},
    gesture::{Contact, DragEngine, TimerDirective},
    logging::RingLogger,
    settings::AppSettings,
};

use super::{
    advanced_actions::AdvancedWindowController,
    contact_report::parse_contact_report,
    device_catalog::{DeviceCatalog, DeviceDescriptor},
    mouse_output::MouseOutput,
    phantom_filter::PhantomFilter,
};

const DRAG_RELEASE_TIMER: usize = 1;
const GIDC_ARRIVAL: usize = 1;
const GIDC_REMOVAL: usize = 2;
static MOUSE_HOOK_STATE: AtomicIsize = AtomicIsize::new(0);

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
    window: isize,
    thread: Option<JoinHandle<()>>,
}

impl InputService {
    pub fn start(
        settings: Arc<RwLock<AppSettings>>,
        logger: RingLogger,
        event_sender: Sender<BackendEvent>,
    ) -> io::Result<Self> {
        Self::start_with_advanced(
            settings,
            Arc::new(RwLock::new(
                better_touch_advanced_gestures::config::AdvancedConfig::default(),
            )),
            logger,
            event_sender,
        )
    }

    pub fn start_with_advanced(
        settings: Arc<RwLock<AppSettings>>,
        advanced_settings: Arc<RwLock<better_touch_advanced_gestures::config::AdvancedConfig>>,
        logger: RingLogger,
        event_sender: Sender<BackendEvent>,
    ) -> io::Result<Self> {
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("ThreeFingerDrag Raw Input".into())
            .spawn(move || {
                run_message_loop(
                    settings,
                    advanced_settings,
                    logger,
                    event_sender,
                    ready_sender,
                )
            })?;

        match ready_receiver.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(window)) => Ok(Self {
                window,
                thread: Some(worker),
            }),
            Ok(Err(message)) => {
                let _ = worker.join();
                Err(io::Error::other(message))
            }
            Err(error) => Err(io::Error::new(io::ErrorKind::TimedOut, error)),
        }
    }

    pub fn stop(&mut self) {
        if self.window != 0 {
            unsafe {
                PostMessageW(self.window as HWND, WM_CLOSE, 0, 0);
            }
            self.window = 0;
        }
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

struct WorkerState {
    window: HWND,
    devices: DeviceCatalog,
    assembler: ContactAssembler,
    phantom_filter: PhantomFilter,
    drag: DragEngine,
    mouse: MouseOutput,
    settings: Arc<RwLock<AppSettings>>,
    advanced_settings: Arc<RwLock<better_touch_advanced_gestures::config::AdvancedConfig>>,
    advanced: AdvancedRuntime,
    advanced_actions: AdvancedWindowController,
    logger: RingLogger,
    events: Sender<BackendEvent>,
    receiver_installed: bool,
    preview_count: u32,
    preview_period_started_ms: u64,
    last_preview_sent_ms: u64,
    mouse_hook: HHOOK,
    mouse_tracking: bool,
    mouse_started_ms: u64,
}

impl WorkerState {
    fn publish_status(&self) {
        let devices = self.devices.all();
        let _ = self
            .events
            .send(BackendEvent::Status(TouchpadRuntimeStatus {
                initialized: true,
                touchpad_exists: !devices.is_empty(),
                receiver_installed: self.receiver_installed,
                devices,
            }));
    }

    unsafe fn handle_raw_input(&mut self, raw_input: HRAWINPUT) {
        let Some(report) = parse_contact_report(raw_input) else {
            self.logger
                .record("Unable to parse a touchpad input report.");
            return;
        };
        let ranges = report.ranges;
        let handle = report.device_handle as HANDLE;
        let Some(device) = self.devices.inspect(handle) else {
            return;
        };

        let assembled = self
            .assembler
            .accept(report.contacts, report.reported_count);
        match assembled {
            AssembledContacts::Ignored | AssembledContacts::Pending => {}
            AssembledContacts::Complete(contacts) => {
                self.process_contacts(&device, contacts, ranges);
            }
            AssembledContacts::CompleteThenPending { complete, .. } => {
                self.process_contacts(&device, complete, ranges);
            }
            AssembledContacts::CompleteThenComplete { first, second } => {
                self.process_contacts(&device, first, ranges);
                self.process_contacts(&device, second, ranges);
            }
        }
    }

    fn process_contacts(
        &mut self,
        device: &DeviceDescriptor,
        contacts: Vec<Contact>,
        ranges: crate::advanced::AxisRanges,
    ) {
        let now_ms = current_time_ms();
        let phantom_rejection = self
            .advanced_settings
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .phantom_rejection;
        let contacts = self
            .phantom_filter
            .filter(&device.id, contacts, phantom_rejection, now_ms);
        let settings = self
            .settings
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if settings.three_finger_drag {
            let result = self
                .drag
                .process_frame(&device.id, contacts.clone(), now_ms, &settings);
            self.mouse.apply_all(&result.mouse, &self.logger);
            if let TimerDirective::Arm(milliseconds) = result.timer {
                unsafe {
                    KillTimer(self.window, DRAG_RELEASE_TIMER);
                    SetTimer(self.window, DRAG_RELEASE_TIMER, milliseconds, None);
                }
            }
        } else {
            self.drag.observe_inactive_frame(contacts.clone(), now_ms);
        }

        let advanced_config = self
            .advanced_settings
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if advanced_config != *self.advanced.config() {
            self.advanced.set_config(advanced_config);
        }
        let advanced_contacts = contacts
            .iter()
            .map(|contact| (contact.id, contact.x, contact.y))
            .collect::<Vec<_>>();
        let thirds = is_modifier_down(self.advanced.config().grid_modifier);
        let monitor = is_modifier_down(self.advanced.config().monitor_move_modifier);
        self.advanced.set_modifier_modes(thirds, monitor);
        self.advanced_actions
            .apply_config(self.advanced.config().clone());
        self.advanced_actions.set_modifier_modes(thirds, monitor);
        // Match Swoosh's hard-cancel contract: Escape cancels the active
        // advanced gesture and latches suppression until all fingers lift.
        if unsafe { GetAsyncKeyState(VK_ESCAPE as i32) < 0 } {
            for event in self.advanced.cancel() {
                self.advanced_actions.handle(&event);
            }
            self.advanced_actions.cancel();
            return;
        }
        let advanced_events = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.advanced
                .process(&advanced_contacts, ranges, now_ms as i64)
        }));
        match advanced_events {
            Ok(events) => {
                if !events.is_empty() {
                    let action_result =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            for event in &events {
                                self.advanced_actions.handle(event);
                            }
                            if let Some(direction) = self.advanced_actions.take_rebaseline_request()
                            {
                                if direction
                                    == better_touch_advanced_gestures::gesture::SwipeDirection::None
                                {
                                    self.advanced.rebaseline();
                                } else {
                                    self.advanced.rebaseline_seed(direction);
                                }
                            }
                        }));
                    if action_result.is_err() {
                        self.advanced
                            .disable_after_error("Rust 窗口动作发生异常，已自动停用进阶手势。 ");
                        self.advanced_actions.cancel();
                        self.logger
                            .record("Advanced Rust window action panicked; disabled safely.");
                    }
                    // The action executor is intentionally kept behind this
                    // boundary. It will only receive events from the Rust
                    // engine; a panic here disables advanced gestures while
                    // preserving the basic drag service.
                    let _ = self
                        .events
                        .send(BackendEvent::Advanced(self.advanced.status()));
                }
            }
            Err(_) => {
                self.advanced
                    .disable_after_error("Rust 进阶手势处理线程发生异常，已自动停用。 ");
                let _ = self
                    .events
                    .send(BackendEvent::Advanced(self.advanced.status()));
                self.logger
                    .record("Advanced Rust gesture processing panicked; disabled safely.");
            }
        }

        self.preview_count += 1;
        if self.preview_period_started_ms == 0 {
            self.preview_period_started_ms = now_ms;
        }
        if now_ms.saturating_sub(self.last_preview_sent_ms) >= 50 {
            let elapsed = now_ms.saturating_sub(self.preview_period_started_ms);
            let interval = if self.preview_count == 0 {
                0
            } else {
                elapsed / self.preview_count as u64
            };
            let _ = self.events.send(BackendEvent::Contacts(ContactPreview {
                device: device.clone(),
                contacts,
                event_interval_ms: interval,
            }));
            self.last_preview_sent_ms = now_ms;
            if self.preview_count >= 20 {
                self.preview_count = 0;
                self.preview_period_started_ms = now_ms;
            }
        }
    }

    fn release_drag(&mut self) {
        let settings = self
            .settings
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let actions = self.drag.release_timeout(&settings);
        self.mouse.apply_all(&actions, &self.logger);
    }

    unsafe fn handle_mouse_hook(&mut self, message: u32, data: &MSLLHOOKSTRUCT) {
        // Settings are normally refreshed on the next touchpad report.  A
        // middle-button gesture can be the first input after a UI change,
        // however, so refresh the Rust advanced controller at the hook boundary
        // as well.  This keeps the toggle responsive without touching the basic
        // three-finger path.
        let advanced_config = self
            .advanced_settings
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if advanced_config != *self.advanced.config() {
            self.advanced.set_config(advanced_config);
        }
        let thirds = is_modifier_down(self.advanced.config().grid_modifier);
        let monitor = is_modifier_down(self.advanced.config().monitor_move_modifier);
        self.advanced_actions
            .apply_config(self.advanced.config().clone());
        self.advanced_actions.set_modifier_modes(thirds, monitor);

        if self.mouse_tracking
            && message != WM_MBUTTONUP
            && current_time_ms().saturating_sub(self.mouse_started_ms) > 150
            && GetAsyncKeyState(VK_MBUTTON as i32) >= 0
        {
            self.advanced_actions.mouse_middle_up(data.pt);
            self.mouse_tracking = false;
        }
        if data.flags & LLMHF_INJECTED != 0 {
            return;
        }
        match message {
            WM_MBUTTONDOWN => {
                if self.advanced_actions.mouse_middle_down(data.pt) {
                    self.mouse_tracking = true;
                    self.mouse_started_ms = current_time_ms();
                }
            }
            WM_MOUSEMOVE if self.mouse_tracking => {
                self.advanced_actions.mouse_middle_move(data.pt);
            }
            WM_MBUTTONUP if self.mouse_tracking => {
                self.advanced_actions.mouse_middle_up(data.pt);
                self.mouse_tracking = false;
            }
            _ => {}
        }
    }
}

fn run_message_loop(
    settings: Arc<RwLock<AppSettings>>,
    advanced_settings: Arc<RwLock<better_touch_advanced_gestures::config::AdvancedConfig>>,
    logger: RingLogger,
    events: Sender<BackendEvent>,
    ready: mpsc::SyncSender<Result<isize, String>>,
) {
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        if instance.is_null() {
            let _ = ready.send(Err("无法获取应用模块句柄。".into()));
            return;
        }

        let class_name = wide(&format!(
            "ThreeFingerDragRust.RawInput.{}",
            GetCurrentProcessId()
        ));
        let window_class = WNDCLASSEXW {
            cbSize: mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(window_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: std::ptr::null_mut(),
            hCursor: std::ptr::null_mut(),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class_name.as_ptr(),
            hIconSm: std::ptr::null_mut(),
        };
        if RegisterClassExW(&window_class) == 0 {
            let _ = ready.send(Err("无法注册触摸板输入窗口类。".into()));
            return;
        }

        let window = CreateWindowExW(
            0,
            class_name.as_ptr(),
            std::ptr::null(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            instance,
            std::ptr::null::<CREATESTRUCTW>().cast::<c_void>(),
        );
        if window.is_null() {
            UnregisterClassW(class_name.as_ptr(), instance);
            let _ = ready.send(Err("无法创建触摸板输入窗口。".into()));
            return;
        }

        let initial_advanced_config = advanced_settings
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let mut state = Box::new(WorkerState {
            window,
            devices: DeviceCatalog::default(),
            assembler: ContactAssembler::default(),
            phantom_filter: PhantomFilter::default(),
            drag: DragEngine::default(),
            mouse: MouseOutput::default(),
            settings,
            advanced: AdvancedRuntime::new(initial_advanced_config),
            advanced_actions: AdvancedWindowController::new(logger.clone()),
            advanced_settings,
            logger,
            events,
            receiver_installed: false,
            preview_count: 0,
            preview_period_started_ms: 0,
            last_preview_sent_ms: 0,
            mouse_hook: std::ptr::null_mut(),
            mouse_tracking: false,
            mouse_started_ms: 0,
        });
        state.devices.enumerate();
        state.receiver_installed = register_touchpad_input(window);
        let state_pointer = Box::into_raw(state);
        SetWindowLongPtrW(window, GWLP_USERDATA, state_pointer as isize);
        (*state_pointer).mouse_hook =
            SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook_proc), instance, 0);
        MOUSE_HOOK_STATE.store(state_pointer as isize, Ordering::Release);
        (*state_pointer).publish_status();
        let _ = (*state_pointer)
            .events
            .send(BackendEvent::Advanced((*state_pointer).advanced.status()));
        let _ = ready.send(Ok(window as isize));

        let mut message = MSG::default();
        while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }

        UnregisterClassW(class_name.as_ptr(), instance);
    }
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let state_pointer = GetWindowLongPtrW(window, GWLP_USERDATA) as *mut WorkerState;
    match message {
        WM_INPUT => {
            if !state_pointer.is_null() {
                (*state_pointer).handle_raw_input(lparam as HRAWINPUT);
            }
            DefWindowProcW(window, message, wparam, lparam)
        }
        WM_INPUT_DEVICE_CHANGE => {
            if !state_pointer.is_null() {
                let handle = lparam as HANDLE;
                match wparam {
                    GIDC_ARRIVAL => {
                        (*state_pointer).devices.refresh(handle);
                    }
                    GIDC_REMOVAL => {
                        (*state_pointer).devices.remove(handle);
                    }
                    _ => {}
                }
                (*state_pointer).publish_status();
            }
            0
        }
        WM_TIMER if wparam == DRAG_RELEASE_TIMER => {
            if !state_pointer.is_null() {
                (*state_pointer).release_drag();
                KillTimer(window, DRAG_RELEASE_TIMER);
            }
            0
        }
        WM_CLOSE => {
            if !state_pointer.is_null() {
                SetWindowLongPtrW(window, GWLP_USERDATA, 0);
                let mut state = Box::from_raw(state_pointer);
                if !state.mouse_hook.is_null() {
                    UnhookWindowsHookEx(state.mouse_hook);
                    state.mouse_hook = std::ptr::null_mut();
                }
                MOUSE_HOOK_STATE.store(0, Ordering::Release);
                state.release_drag();
                state.advanced.cancel();
                state.advanced_actions.cancel();
                KillTimer(window, DRAG_RELEASE_TIMER);
            }
            DestroyWindow(window);
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(window, message, wparam, lparam),
    }
}

unsafe fn register_touchpad_input(window: HWND) -> bool {
    let device = RAWINPUTDEVICE {
        usUsagePage: 0x000D,
        usUsage: 0x0005,
        dwFlags: RIDEV_INPUTSINK | RIDEV_DEVNOTIFY,
        hwndTarget: window,
    };
    RegisterRawInputDevices(&device, 1, mem::size_of::<RAWINPUTDEVICE>() as u32) != 0
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

unsafe extern "system" fn mouse_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let pointer = MOUSE_HOOK_STATE.load(Ordering::Acquire) as *mut WorkerState;
        if !pointer.is_null() && !lparam_cast_is_null(lparam) {
            let data = &*(lparam as *const MSLLHOOKSTRUCT);
            (*pointer).handle_mouse_hook(wparam as u32, data);
        }
    }
    CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
}

fn lparam_cast_is_null(value: LPARAM) -> bool {
    value == 0
}

fn is_modifier_down(modifier: better_touch_advanced_gestures::config::GridModifier) -> bool {
    let key = match modifier {
        better_touch_advanced_gestures::config::GridModifier::Shift => VK_SHIFT,
        better_touch_advanced_gestures::config::GridModifier::Ctrl => VK_CONTROL,
        better_touch_advanced_gestures::config::GridModifier::Alt => VK_MENU,
    };
    unsafe { GetAsyncKeyState(key as i32) < 0 }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
