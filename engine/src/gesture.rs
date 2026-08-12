use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::{
    settings::{AppSettings, DeviceDragSettings, DragButton},
    speed::{apply_cursor_response, apply_threshold_speed},
};

pub const RELEASE_GAP_MS: u64 = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contact {
    pub id: i32,
    pub x: i32,
    pub y: i32,
}

impl Contact {
    fn distance_from(self, other: Self) -> f32 {
        let delta_x = (self.x - other.x) as f32;
        let delta_y = (self.y - other.y) as f32;
        delta_x.hypot(delta_y)
    }
}

pub fn same_contact_ids(old_contacts: &[Contact], new_contacts: &[Contact]) -> bool {
    old_contacts.len() == new_contacts.len()
        && new_contacts.iter().all(|new_contact| {
            old_contacts
                .iter()
                .any(|old_contact| old_contact.id == new_contact.id)
        })
}

#[derive(Debug, Clone, PartialEq)]
pub enum MouseAction {
    ButtonDown(DragButton),
    ButtonUp(DragButton),
    Move { dx: f32, dy: f32 },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TimerDirective {
    #[default]
    Keep,
    Arm(u32),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FrameResult {
    pub mouse: Vec<MouseAction>,
    pub timer: TimerDirective,
}

#[derive(Debug, Default)]
struct DistanceTracker {
    quarantined: HashMap<i32, u64>,
    trusted: HashSet<i32>,
}

impl DistanceTracker {
    fn longest_delta(
        &mut self,
        old_contacts: &[Contact],
        new_contacts: &[Contact],
        fingers_released: bool,
        now_ms: u64,
    ) -> ((f32, f32), f32) {
        if fingers_released {
            self.quarantined.clear();
            self.trusted.clear();
            return ((0.0, 0.0), 0.0);
        }

        self.trusted
            .retain(|id| new_contacts.iter().any(|contact| contact.id == *id));
        self.quarantined
            .retain(|id, _| new_contacts.iter().any(|contact| contact.id == *id));

        for contact in new_contacts {
            if let Some(entered_at) = self.quarantined.get(&contact.id).copied() {
                if now_ms.saturating_sub(entered_at) > RELEASE_GAP_MS {
                    self.trusted.insert(contact.id);
                    self.quarantined.remove(&contact.id);
                }
            } else {
                self.quarantined.insert(contact.id, now_ms);
            }
        }

        let mut longest_distance = 0.0;
        let mut longest_delta = (0.0, 0.0);
        for new_contact in new_contacts {
            if !self.trusted.contains(&new_contact.id) {
                continue;
            }
            if let Some(old_contact) = old_contacts
                .iter()
                .find(|old_contact| old_contact.id == new_contact.id)
            {
                let distance = new_contact.distance_from(*old_contact);
                if distance > longest_distance {
                    longest_distance = distance;
                    longest_delta = (
                        (new_contact.x - old_contact.x) as f32,
                        (new_contact.y - old_contact.y) as f32,
                    );
                }
            }
        }
        (longest_delta, longest_distance)
    }
}

#[derive(Debug, Default)]
struct FingerCounter {
    original_count: usize,
    short_count: usize,
    short_distance: f32,
    long_count: usize,
    long_distance: f32,
}

impl FingerCounter {
    fn update(
        &mut self,
        contacts: &[Contact],
        ids_common: bool,
        longest_distance: f32,
        fingers_released: bool,
        device: &DeviceDragSettings,
        settings: &AppSettings,
    ) -> (usize, usize, usize, usize) {
        if !ids_common && (contacts.len() <= 1 || fingers_released) {
            self.original_count = 0;
        }
        if !ids_common || fingers_released {
            self.short_distance = 0.0;
            self.long_distance = 0.0;
            return (0, self.short_count, self.long_count, self.original_count);
        }

        let distance = apply_threshold_speed(longest_distance, device);
        if distance >= 1.0 {
            self.short_distance += distance;
            self.long_distance += distance;
        }

        if self.short_distance >= settings.stop_threshold as f32 {
            self.short_count = contacts.len();
            self.short_distance = 0.0;
        }
        if self.long_distance > settings.start_threshold as f32 {
            self.long_count = contacts.len();
            self.long_distance = 0.0;
            if self.original_count <= 1 {
                self.original_count = contacts.len();
            }
        }

        (
            contacts.len(),
            self.short_count,
            self.long_count,
            self.original_count,
        )
    }
}

#[derive(Debug, Default)]
pub struct DragEngine {
    distance: DistanceTracker,
    fingers: FingerCounter,
    old_contacts: Vec<Contact>,
    last_frame_ms: Option<u64>,
    dragging: bool,
    averaging_x: f32,
    averaging_y: f32,
    averaging_count: u32,
}

impl DragEngine {
    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    pub fn observe_inactive_frame(&mut self, contacts: Vec<Contact>, timestamp_ms: u64) {
        self.old_contacts = contacts;
        self.last_frame_ms = Some(timestamp_ms);
    }

    pub fn process_frame(
        &mut self,
        device_id: &str,
        contacts: Vec<Contact>,
        timestamp_ms: u64,
        settings: &AppSettings,
    ) -> FrameResult {
        let elapsed = self
            .last_frame_ms
            .map(|previous| timestamp_ms.saturating_sub(previous))
            .unwrap_or_default();
        let fingers_released = elapsed > RELEASE_GAP_MS;
        let ids_common = same_contact_ids(&self.old_contacts, &contacts);
        let (longest_delta, longest_distance) = self.distance.longest_delta(
            &self.old_contacts,
            &contacts,
            fingers_released,
            timestamp_ms,
        );
        let default_device = DeviceDragSettings::default();
        let threshold_device = settings.devices.get(device_id).unwrap_or(&default_device);
        let (finger_count, short_count, long_count, original_count) = self.fingers.update(
            &contacts,
            ids_common,
            longest_distance,
            fingers_released,
            threshold_device,
            settings,
        );

        let mut result = FrameResult::default();
        if finger_count >= 3
            && ids_common
            && long_count == 3
            && original_count == 3
            && !self.dragging
        {
            self.dragging = true;
            if settings.drag_button != DragButton::None {
                result
                    .mouse
                    .push(MouseAction::ButtonDown(settings.drag_button));
            }
        } else if self.dragging && (short_count < 2 || (original_count != 3 && original_count >= 2))
        {
            self.dragging = false;
            if settings.drag_button != DragButton::None {
                result
                    .mouse
                    .push(MouseAction::ButtonUp(settings.drag_button));
            }
        } else if finger_count >= 2 && original_count == 3 && ids_common && self.dragging {
            if let Some(device) = settings.devices.get(device_id) {
                if device.cursor_move {
                    let excessive_movement = settings.max_finger_move_distance != 0
                        && longest_distance > settings.max_finger_move_distance as f32;
                    if !excessive_movement && (longest_delta.0 != 0.0 || longest_delta.1 != 0.0) {
                        let delta = apply_cursor_response(longest_delta, elapsed, device);
                        if settings.cursor_averaging > 1 {
                            self.averaging_x += delta.0;
                            self.averaging_y += delta.1;
                            self.averaging_count += 1;
                            if self.averaging_count >= settings.cursor_averaging {
                                result.mouse.push(MouseAction::Move {
                                    dx: self.averaging_x,
                                    dy: self.averaging_y,
                                });
                                self.averaging_x = 0.0;
                                self.averaging_y = 0.0;
                                self.averaging_count = 0;
                            }
                        } else {
                            result.mouse.push(MouseAction::Move {
                                dx: delta.0,
                                dy: delta.1,
                            });
                        }
                    }

                    result.timer = TimerDirective::Arm(if settings.allow_release_and_restart {
                        settings.release_delay_ms.max(RELEASE_GAP_MS as u32)
                    } else {
                        RELEASE_GAP_MS as u32
                    });
                }
            }
        }

        self.old_contacts = contacts;
        self.last_frame_ms = Some(timestamp_ms);
        result
    }

    pub fn release_timeout(&mut self, settings: &AppSettings) -> Vec<MouseAction> {
        if !self.dragging {
            return Vec::new();
        }
        self.dragging = false;
        if settings.drag_button == DragButton::None {
            Vec::new()
        } else {
            vec![MouseAction::ButtonUp(settings.drag_button)]
        }
    }

    pub fn force_release(&mut self, settings: &AppSettings) -> Vec<MouseAction> {
        self.release_timeout(settings)
    }
}
