use std::collections::VecDeque;
use std::f64::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Contact {
    pub id: i32,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TouchFrame {
    pub timestamp_ms: i64,
    pub contacts: Vec<Contact>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwipeDirection {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopDirection {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonitorDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GestureEvent {
    Began {
        contacts: usize,
    },
    Raw {
        dx: f64,
        dy: f64,
    },
    Updated {
        direction: SwipeDirection,
        progress: f64,
    },
    Completed(SwipeDirection),
    Cancelled,
    HoldEngaged,
    HoldUpdated {
        direction: Option<DesktopDirection>,
        progress: f64,
        aim_steps: i32,
    },
    DesktopMove(DesktopDirection),
    DesktopHoldCommit(i32),
    MonitorMoveUpdated {
        direction: Option<MonitorDirection>,
        progress: f64,
    },
    MonitorMove(MonitorDirection),
    FreeMoveBegan,
    FreeMoveDelta {
        dx: f64,
        dy: f64,
        scale: f64,
    },
    FreeMoveEnded {
        was_tap: bool,
        cancelled: bool,
    },
    PinchUpdated {
        outward: bool,
        progress: f64,
    },
    PinchOut,
    PinchIn,
    AxisResizeBegan {
        horizontal: bool,
    },
    AxisResizeDelta {
        factor: f64,
        horizontal: bool,
    },
    AxisResizeEnded {
        cancelled: bool,
    },
}

#[derive(Debug, Clone)]
pub struct GestureEngine {
    pub enabled: bool,
    pub commit_distance: f64,
    pub dead_zone: f64,
    pub max_duration_ms: i64,
    pub idle_cancel_ms: i64,
    pub hold_delay_ms: i64,
    pub hold_radius: f64,
    pub desktop_move_threshold: f64,
    pub desktop_move_on_release: bool,
    pub free_move_engage_contacts: usize,
    pub five_finger_enabled: bool,
    pub free_move_keep_contacts: usize,
    pub five_tap_max_ms: i64,
    pub five_tap_max_dist: f64,
    pub pinch_engage_delta: f64,
    pub pinch_engage_ratio: f64,
    pub pinch_max_centroid_travel: f64,
    pub pinch_preview_delta: f64,
    pub thirds_mode: bool,
    pub monitor_move_mode: bool,
    pub thirds_dead_zone: f64,
    pub axis_resize_h: bool,
    pub axis_resize_v: bool,
    pub free_resize_dead_zone: f64,

    tracking: bool,
    cancelled: bool,
    suppressed: bool,
    start_x: f64,
    start_y: f64,
    last_x: f64,
    last_y: f64,
    start_time: i64,
    last_frame_ms: i64,
    current_dir: SwipeDirection,
    history: VecDeque<(f64, f64)>,
    last_move_ms: i64,
    idle_anchor_x: f64,
    idle_anchor_y: f64,
    idle_spread: f64,
    hold: bool,
    hold_eligible: bool,
    max_dist: f64,
    hold_anchor_x: f64,
    hold_aim_steps: i32,
    monitor_active: bool,
    monitor_dir: Option<MonitorDirection>,
    start_spread: f64,
    pinch_fired: bool,
    start_gap_x: f64,
    start_gap_y: f64,
    axis_resize_active: bool,
    axis_horizontal: bool,
    axis_last_gap: f64,
    free: bool,
    free_last_x: f64,
    free_last_y: f64,
    free_has_last: bool,
    free_last_count: usize,
    free_start_time: i64,
    free_start_x: f64,
    free_start_y: f64,
    free_max_dist: f64,
    free_start_spread: f64,
    free_last_spread: f64,
}

impl Default for GestureEngine {
    fn default() -> Self {
        Self {
            enabled: true,
            commit_distance: 0.12,
            dead_zone: 0.055,
            max_duration_ms: 8_000,
            idle_cancel_ms: 800,
            hold_delay_ms: 200,
            hold_radius: 0.05,
            desktop_move_threshold: 0.09,
            desktop_move_on_release: false,
            free_move_engage_contacts: 5,
            five_finger_enabled: true,
            free_move_keep_contacts: 4,
            five_tap_max_ms: 350,
            five_tap_max_dist: 0.06,
            pinch_engage_delta: 0.10,
            pinch_engage_ratio: 1.45,
            pinch_max_centroid_travel: 0.06,
            pinch_preview_delta: 0.035,
            thirds_mode: false,
            monitor_move_mode: false,
            thirds_dead_zone: 0.03,
            axis_resize_h: false,
            axis_resize_v: false,
            free_resize_dead_zone: 0.015,
            tracking: false,
            cancelled: false,
            suppressed: false,
            start_x: 0.0,
            start_y: 0.0,
            last_x: 0.0,
            last_y: 0.0,
            start_time: 0,
            last_frame_ms: 0,
            current_dir: SwipeDirection::None,
            history: VecDeque::with_capacity(6),
            last_move_ms: 0,
            idle_anchor_x: 0.0,
            idle_anchor_y: 0.0,
            idle_spread: 0.0,
            hold: false,
            hold_eligible: true,
            max_dist: 0.0,
            hold_anchor_x: 0.0,
            hold_aim_steps: 0,
            monitor_active: false,
            monitor_dir: None,
            start_spread: 0.0,
            pinch_fired: false,
            start_gap_x: 0.0,
            start_gap_y: 0.0,
            axis_resize_active: false,
            axis_horizontal: false,
            axis_last_gap: 0.0,
            free: false,
            free_last_x: 0.0,
            free_last_y: 0.0,
            free_has_last: false,
            free_last_count: 0,
            free_start_time: 0,
            free_start_x: 0.0,
            free_start_y: 0.0,
            free_max_dist: 0.0,
            free_start_spread: 0.0,
            free_last_spread: 0.0,
        }
    }
}

impl GestureEngine {
    const IDLE_MOVE_THRESHOLD: f64 = 0.008;
    const IDLE_SPREAD_THRESHOLD: f64 = 0.012;
    const STALL_GAP_MS: i64 = 60;

    pub fn process(&mut self, frame: &TouchFrame) -> Vec<GestureEvent> {
        let mut events = Vec::new();
        let down = frame.contacts.len();

        if !self.enabled {
            if self.tracking || self.free {
                self.reset_into(&mut events);
            }
            self.suppressed = false;
            return events;
        }
        if self.suppressed {
            if down == 0 {
                self.suppressed = false;
            }
            return events;
        }

        if self.free {
            if down >= self.free_move_engage_contacts {
                let (x, y) = centroid(frame);
                let spread = spread_n(frame);
                self.free_max_dist = self
                    .free_max_dist
                    .max(hypot(x - self.free_start_x, y - self.free_start_y));
                if down != self.free_last_count {
                    self.free_has_last = false;
                }
                self.free_last_count = down;
                if self.free_has_last {
                    let scale = if (spread - self.free_start_spread).abs()
                        < self.free_resize_dead_zone
                        || self.free_last_spread < 1e-4
                    {
                        1.0
                    } else {
                        spread / self.free_last_spread
                    };
                    events.push(GestureEvent::FreeMoveDelta {
                        dx: x - self.free_last_x,
                        dy: y - self.free_last_y,
                        scale,
                    });
                }
                self.free_last_x = x;
                self.free_last_y = y;
                self.free_last_spread = spread;
                self.free_has_last = true;
            } else if down >= self.free_move_keep_contacts {
                self.free_has_last = false;
                self.free_last_count = down;
            } else {
                let was_tap = frame.timestamp_ms - self.free_start_time <= self.five_tap_max_ms
                    && self.free_max_dist <= self.five_tap_max_dist;
                self.free = false;
                self.free_has_last = false;
                events.push(GestureEvent::FreeMoveEnded {
                    was_tap,
                    cancelled: false,
                });
            }
            return events;
        }

        if self.five_finger_enabled && down >= self.free_move_engage_contacts {
            if self.tracking {
                self.tracking = false;
                events.push(GestureEvent::Cancelled);
            }
            self.free = true;
            self.free_has_last = false;
            self.free_last_count = down;
            let (x, y) = centroid(frame);
            self.free_start_time = frame.timestamp_ms;
            self.free_start_x = x;
            self.free_start_y = y;
            self.free_max_dist = 0.0;
            self.free_start_spread = spread_n(frame);
            self.free_last_spread = self.free_start_spread;
            events.push(GestureEvent::FreeMoveBegan);
            return events;
        }

        if down == 2 {
            let (x, y) = centroid(frame);
            if !self.tracking {
                self.begin(frame, x, y);
                events.push(GestureEvent::Began { contacts: 2 });
                if self.monitor_active {
                    events.push(GestureEvent::MonitorMoveUpdated {
                        direction: None,
                        progress: 0.0,
                    });
                }
                return events;
            }
            self.last_x = x;
            self.last_y = y;
            self.push_history(x, y);
            let frame_gap = frame.timestamp_ms - self.last_frame_ms;
            if frame_gap > Self::STALL_GAP_MS {
                self.start_time += frame_gap;
                self.last_move_ms += frame_gap;
            }
            self.last_frame_ms = frame.timestamp_ms;
            let dx = x - self.start_x;
            let dy = y - self.start_y;
            let dist = hypot(dx, dy);
            self.max_dist = self.max_dist.max(dist);
            events.push(GestureEvent::Raw { dx, dy });

            let idle_spread = spread_two(frame);
            let moved = (x - self.idle_anchor_x).abs() >= Self::IDLE_MOVE_THRESHOLD
                || (y - self.idle_anchor_y).abs() >= Self::IDLE_MOVE_THRESHOLD
                || (idle_spread - self.idle_spread).abs() >= Self::IDLE_SPREAD_THRESHOLD;
            if moved {
                self.idle_anchor_x = x;
                self.idle_anchor_y = y;
                self.idle_spread = idle_spread;
                self.last_move_ms = frame.timestamp_ms;
            } else if self.idle_cancel_ms > 0
                && !(self.hold_eligible && !self.hold)
                && frame.timestamp_ms - self.last_move_ms >= self.idle_cancel_ms
            {
                self.suppressed = true;
                self.finalize(frame.timestamp_ms, &mut events);
                return events;
            }

            if self.monitor_active {
                let direction = if dist < self.dead_zone {
                    None
                } else if dx.abs() >= dy.abs() {
                    Some(if dx >= 0.0 {
                        MonitorDirection::Right
                    } else {
                        MonitorDirection::Left
                    })
                } else {
                    Some(if dy >= 0.0 {
                        MonitorDirection::Down
                    } else {
                        MonitorDirection::Up
                    })
                };
                self.monitor_dir = direction;
                events.push(GestureEvent::MonitorMoveUpdated {
                    direction,
                    progress: (dist / self.commit_distance).clamp(0.0, 1.0),
                });
                return events;
            }

            if self.pinch_fired {
                return events;
            }
            let current_spread = spread_two(frame);
            let gain = current_spread - self.start_spread;
            let centroid_fixed = self.max_dist <= self.pinch_max_centroid_travel;

            if (self.axis_resize_h || self.axis_resize_v)
                && !self.hold
                && (self.axis_resize_active
                    || (centroid_fixed && gain.abs() >= self.pinch_preview_delta))
            {
                self.hold_eligible = false;
                let gx = gap_x(frame);
                let gy = gap_y(frame);
                if !self.axis_resize_active {
                    let dx_gap = (gx - self.start_gap_x).abs();
                    let dy_gap = (gy - self.start_gap_y).abs();
                    self.axis_horizontal = if self.axis_resize_h && self.axis_resize_v {
                        dx_gap >= dy_gap
                    } else {
                        self.axis_resize_h
                    };
                    self.axis_last_gap = (if self.axis_horizontal { gx } else { gy }).max(1e-4);
                    self.axis_resize_active = true;
                    events.push(GestureEvent::AxisResizeBegan {
                        horizontal: self.axis_horizontal,
                    });
                } else {
                    let current = if self.axis_horizontal { gx } else { gy };
                    let factor = if current < 1e-4 || self.axis_last_gap < 1e-4 {
                        1.0
                    } else {
                        current / self.axis_last_gap
                    };
                    self.axis_last_gap = current;
                    events.push(GestureEvent::AxisResizeDelta {
                        factor,
                        horizontal: self.axis_horizontal,
                    });
                }
                return events;
            }

            if !self.hold && centroid_fixed && gain.abs() >= self.pinch_preview_delta {
                self.hold_eligible = false;
                let outward = gain > 0.0;
                events.push(GestureEvent::PinchUpdated {
                    outward,
                    progress: (gain.abs() / self.pinch_engage_delta).clamp(0.0, 1.0),
                });
                let ratio_ok = if outward {
                    self.start_spread <= 1e-4
                        || current_spread >= self.start_spread * self.pinch_engage_ratio
                } else {
                    self.start_spread > 1e-4
                        && current_spread <= self.start_spread / self.pinch_engage_ratio
                };
                if gain.abs() >= self.pinch_engage_delta && ratio_ok {
                    self.pinch_fired = true;
                    events.push(if outward {
                        GestureEvent::PinchOut
                    } else {
                        GestureEvent::PinchIn
                    });
                }
                return events;
            }

            if !self.hold {
                if self.max_dist >= self.hold_radius {
                    self.hold_eligible = false;
                } else if self.hold_eligible
                    && frame.timestamp_ms - self.start_time >= self.hold_delay_ms
                {
                    self.hold = true;
                    self.hold_anchor_x = x;
                    self.last_move_ms = frame.timestamp_ms;
                    events.push(GestureEvent::HoldEngaged);
                }
            }
            if self.hold {
                let ddx = x - self.hold_anchor_x;
                let progress = (ddx.abs() / self.desktop_move_threshold).clamp(0.0, 1.0);
                let direction = if ddx > self.desktop_move_threshold * 0.3 {
                    Some(DesktopDirection::Right)
                } else if ddx < -self.desktop_move_threshold * 0.3 {
                    Some(DesktopDirection::Left)
                } else {
                    None
                };
                let aim_steps = round_away(ddx / self.desktop_move_threshold);
                events.push(GestureEvent::HoldUpdated {
                    direction,
                    progress,
                    aim_steps,
                });
                if self.desktop_move_on_release {
                    self.hold_aim_steps = aim_steps;
                } else {
                    let step = if ddx >= self.desktop_move_threshold {
                        Some(DesktopDirection::Right)
                    } else if ddx <= -self.desktop_move_threshold {
                        Some(DesktopDirection::Left)
                    } else {
                        None
                    };
                    if let Some(direction) = step {
                        self.hold_anchor_x = x;
                        events.push(GestureEvent::DesktopMove(direction));
                    }
                }
            } else {
                let dead_zone = if self.thirds_mode {
                    self.thirds_dead_zone
                } else {
                    self.dead_zone
                };
                if dist >= dead_zone {
                    let candidate = Self::classify(dx, dy);
                    self.current_dir = self.stabilize(self.current_dir, candidate);
                    events.push(GestureEvent::Updated {
                        direction: self.current_dir,
                        progress: (dist / self.commit_distance).clamp(0.0, 1.0),
                    });
                } else {
                    self.current_dir = SwipeDirection::None;
                    events.push(GestureEvent::Updated {
                        direction: SwipeDirection::None,
                        progress: 0.0,
                    });
                }
            }
        } else if down > 2 {
            if self.tracking {
                self.tracking = false;
                self.cancelled = true;
                if self.axis_resize_active {
                    self.axis_resize_active = false;
                    events.push(GestureEvent::AxisResizeEnded { cancelled: true });
                } else {
                    events.push(GestureEvent::Cancelled);
                }
            }
        } else if self.tracking {
            self.finalize(frame.timestamp_ms, &mut events);
        }
        events
    }

    fn begin(&mut self, frame: &TouchFrame, x: f64, y: f64) {
        self.tracking = true;
        self.cancelled = false;
        self.current_dir = SwipeDirection::None;
        self.hold = false;
        self.hold_eligible = true;
        self.max_dist = 0.0;
        self.history.clear();
        self.hold_anchor_x = x;
        self.hold_aim_steps = 0;
        self.start_x = x;
        self.start_y = y;
        self.last_x = x;
        self.last_y = y;
        self.start_time = frame.timestamp_ms;
        self.last_frame_ms = frame.timestamp_ms;
        self.start_spread = spread_two(frame);
        self.start_gap_x = gap_x(frame);
        self.start_gap_y = gap_y(frame);
        self.axis_resize_active = false;
        self.pinch_fired = false;
        self.monitor_active = self.monitor_move_mode;
        self.monitor_dir = None;
        self.last_move_ms = frame.timestamp_ms;
        self.idle_anchor_x = x;
        self.idle_anchor_y = y;
        self.idle_spread = self.start_spread;
    }

    fn finalize(&mut self, end_time: i64, events: &mut Vec<GestureEvent>) {
        self.tracking = false;
        let duration = end_time - self.start_time;
        if self.axis_resize_active {
            self.axis_resize_active = false;
            events.push(GestureEvent::AxisResizeEnded { cancelled: false });
            return;
        }
        if self.pinch_fired {
            events.push(GestureEvent::Cancelled);
            return;
        }
        if self.monitor_active {
            events.push(
                self.monitor_dir
                    .map(GestureEvent::MonitorMove)
                    .unwrap_or(GestureEvent::Cancelled),
            );
            return;
        }
        if self.cancelled || duration > self.max_duration_ms {
            events.push(GestureEvent::Cancelled);
            return;
        }
        if self.hold {
            if self.desktop_move_on_release && self.hold_aim_steps != 0 {
                events.push(GestureEvent::DesktopHoldCommit(self.hold_aim_steps));
            } else {
                events.push(GestureEvent::Cancelled);
            }
            return;
        }
        if self.current_dir == SwipeDirection::None {
            events.push(GestureEvent::Cancelled);
        } else {
            events.push(GestureEvent::Completed(self.current_dir));
        }
    }

    pub fn cancel(&mut self) -> Vec<GestureEvent> {
        let mut events = Vec::new();
        let active = self.tracking || self.free;
        if self.tracking {
            self.tracking = false;
            self.cancelled = true;
            if self.axis_resize_active {
                self.axis_resize_active = false;
                events.push(GestureEvent::AxisResizeEnded { cancelled: true });
            } else {
                events.push(GestureEvent::Cancelled);
            }
        }
        if self.free {
            self.free = false;
            self.free_has_last = false;
            events.push(GestureEvent::FreeMoveEnded {
                was_tap: false,
                cancelled: true,
            });
        }
        if active {
            self.suppressed = true;
        }
        events
    }

    pub fn reset(&mut self) -> Vec<GestureEvent> {
        let mut events = Vec::new();
        self.reset_into(&mut events);
        events
    }

    fn reset_into(&mut self, events: &mut Vec<GestureEvent>) {
        if self.tracking {
            events.push(GestureEvent::Cancelled);
        }
        self.tracking = false;
        if self.free {
            self.free = false;
            self.free_has_last = false;
            events.push(GestureEvent::FreeMoveEnded {
                was_tap: false,
                cancelled: true,
            });
        }
    }

    pub fn rebaseline(&mut self) {
        if !self.tracking {
            return;
        }
        self.start_x = self.last_x;
        self.start_y = self.last_y;
        self.start_time = self.last_frame_ms;
        self.max_dist = 0.0;
        self.history.clear();
        self.current_dir = SwipeDirection::None;
        self.hold_eligible = false;
        self.idle_anchor_x = self.last_x;
        self.idle_anchor_y = self.last_y;
        self.last_move_ms = self.last_frame_ms;
    }

    /// Re-anchor an in-flight swipe so its current finger position already
    /// classifies as `seed`. Swoosh uses this when the down-action chooser is
    /// retracted and a deliberately aimed prior snap zone should return.
    pub fn rebaseline_seed(&mut self, seed: SwipeDirection) {
        if !self.tracking {
            return;
        }
        if seed == SwipeDirection::None {
            self.rebaseline();
            return;
        }
        let (unit_x, unit_y) = dir_components(seed);
        let normalization = if unit_x != 0 && unit_y != 0 {
            1.0 / 2.0_f64.sqrt()
        } else {
            1.0
        };
        let pad_dx = unit_x as f64 * normalization * self.commit_distance;
        let pad_dy = -(unit_y as f64) * normalization * self.commit_distance;
        self.start_x = self.last_x - pad_dx;
        self.start_y = self.last_y - pad_dy;
        self.start_time = self.last_frame_ms;
        self.max_dist = self.hold_radius.max(self.commit_distance);
        self.history.clear();
        self.current_dir = seed;
        self.hold_eligible = false;
        self.idle_anchor_x = self.last_x;
        self.idle_anchor_y = self.last_y;
        self.last_move_ms = self.last_frame_ms;
    }

    pub fn classify(dx: f64, dy: f64) -> SwipeDirection {
        let mut angle = (-dy).atan2(dx) * 180.0 / PI;
        if angle < 0.0 {
            angle += 360.0;
        }
        match angle {
            a if (22.5..67.5).contains(&a) => SwipeDirection::UpRight,
            a if (67.5..112.5).contains(&a) => SwipeDirection::Up,
            a if (112.5..157.5).contains(&a) => SwipeDirection::UpLeft,
            a if (157.5..202.5).contains(&a) => SwipeDirection::Left,
            a if (202.5..247.5).contains(&a) => SwipeDirection::DownLeft,
            a if (247.5..292.5).contains(&a) => SwipeDirection::Down,
            a if (292.5..337.5).contains(&a) => SwipeDirection::DownRight,
            _ => SwipeDirection::Right,
        }
    }

    fn push_history(&mut self, x: f64, y: f64) {
        self.history.push_front((x, y));
        self.history.truncate(6);
    }

    fn stabilize(&self, previous: SwipeDirection, candidate: SwipeDirection) -> SwipeDirection {
        if candidate == previous || previous == SwipeDirection::None {
            return candidate;
        }
        let (cx, cy) = dir_components(candidate);
        if !((cx == 0) ^ (cy == 0)) {
            return candidate;
        }
        let Some(&(new_x, new_y)) = self.history.front() else {
            return candidate;
        };
        let Some(&(old_x, old_y)) = self.history.back() else {
            return candidate;
        };
        let ax = (new_x - old_x).abs();
        let ay = (new_y - old_y).abs();
        if (cy != 0 && ax >= ay) || (cx != 0 && ay >= ax) {
            previous
        } else {
            candidate
        }
    }
}

fn centroid(frame: &TouchFrame) -> (f64, f64) {
    if frame.contacts.is_empty() {
        return (0.0, 0.0);
    }
    let (x, y) = frame
        .contacts
        .iter()
        .fold((0.0, 0.0), |(x, y), c| (x + c.x, y + c.y));
    (
        x / frame.contacts.len() as f64,
        y / frame.contacts.len() as f64,
    )
}

fn spread_two(frame: &TouchFrame) -> f64 {
    if frame.contacts.len() < 2 {
        0.0
    } else {
        hypot(
            frame.contacts[1].x - frame.contacts[0].x,
            frame.contacts[1].y - frame.contacts[0].y,
        )
    }
}
fn gap_x(frame: &TouchFrame) -> f64 {
    if frame.contacts.len() < 2 {
        0.0
    } else {
        (frame.contacts[1].x - frame.contacts[0].x).abs()
    }
}
fn gap_y(frame: &TouchFrame) -> f64 {
    if frame.contacts.len() < 2 {
        0.0
    } else {
        (frame.contacts[1].y - frame.contacts[0].y).abs()
    }
}
fn spread_n(frame: &TouchFrame) -> f64 {
    if frame.contacts.len() < 2 {
        return 0.0;
    }
    let (cx, cy) = centroid(frame);
    frame
        .contacts
        .iter()
        .map(|c| hypot(c.x - cx, c.y - cy))
        .sum::<f64>()
        / frame.contacts.len() as f64
}
fn hypot(x: f64, y: f64) -> f64 {
    (x * x + y * y).sqrt()
}
fn round_away(value: f64) -> i32 {
    if value >= 0.0 {
        (value + 0.5).floor() as i32
    } else {
        (value - 0.5).ceil() as i32
    }
}
fn dir_components(direction: SwipeDirection) -> (i32, i32) {
    match direction {
        SwipeDirection::Left => (-1, 0),
        SwipeDirection::Right => (1, 0),
        SwipeDirection::Up => (0, 1),
        SwipeDirection::Down => (0, -1),
        SwipeDirection::UpLeft => (-1, 1),
        SwipeDirection::UpRight => (1, 1),
        SwipeDirection::DownLeft => (-1, -1),
        SwipeDirection::DownRight => (1, -1),
        SwipeDirection::None => (0, 0),
    }
}
