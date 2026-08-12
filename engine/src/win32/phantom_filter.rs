//! Small, allocation-light equivalent of Swoosh's firmware phantom-contact
//! rejection.  HID reports from some precision pads leave a frozen contact
//! behind after a multi-finger lift; feeding it into the gesture engine looks
//! like an unexplained hold and is a common source of "stuck" gestures.

use std::collections::{HashMap, HashSet};

use crate::gesture::Contact;

const LEARN_MS: u64 = 1_200;
const RESET_GAP_MS: u64 = 2_000;
const PHANTOM_TOL: i32 = 3;

#[derive(Clone, Copy)]
struct Track {
    x: i32,
    y: i32,
    since_ms: u64,
}

#[derive(Default)]
struct DeviceState {
    tracks: HashMap<i32, Track>,
    phantoms: Vec<(i32, i32)>,
    peak_ids: HashSet<i32>,
    peak_down: usize,
    last_seen_ms: u64,
}

#[derive(Default)]
pub(super) struct PhantomFilter {
    devices: HashMap<String, DeviceState>,
}

impl PhantomFilter {
    /// Apply duplicate-id collapse unconditionally and the learned phantom
    /// heuristics when `enabled` is true.  This is called only after the raw
    /// report assembler has produced a complete frame, so a partial HID batch
    /// cannot teach the filter a false residue.
    pub fn filter(
        &mut self,
        device_id: &str,
        contacts: Vec<Contact>,
        enabled: bool,
        now_ms: u64,
    ) -> Vec<Contact> {
        let state = self.devices.entry(device_id.to_owned()).or_default();
        if state.last_seen_ms != 0 && now_ms.saturating_sub(state.last_seen_ms) > RESET_GAP_MS {
            *state = DeviceState::default();
        }
        state.last_seen_ms = now_ms;

        let mut unique = Vec::with_capacity(contacts.len());
        for contact in contacts {
            if let Some(index) = unique
                .iter()
                .position(|existing: &Contact| existing.id == contact.id)
            {
                // Prefer the newer/moving representation when a firmware slot
                // duplicates a contact id in the same frame.
                let previous = unique[index];
                let moved = state
                    .tracks
                    .get(&contact.id)
                    .map(|track| track.x != contact.x || track.y != contact.y)
                    .unwrap_or(true);
                let previous_moved = state
                    .tracks
                    .get(&previous.id)
                    .map(|track| track.x != previous.x || track.y != previous.y)
                    .unwrap_or(true);
                if moved && !previous_moved {
                    unique[index] = contact;
                }
            } else {
                unique.push(contact);
            }
        }

        // A fully lifted report is the hard boundary between touch sequences.
        if unique.is_empty() {
            *state = DeviceState::default();
            return unique;
        }

        let present: HashSet<i32> = unique.iter().map(|contact| contact.id).collect();
        state.tracks.retain(|id, _| present.contains(id));

        let mut frozen = HashMap::with_capacity(unique.len());
        for contact in &unique {
            let previous = state.tracks.get(&contact.id).copied();
            let moved = previous
                .map(|track| track.x != contact.x || track.y != contact.y)
                .unwrap_or(true);
            let since_ms = if moved {
                now_ms
            } else {
                previous.map(|track| track.since_ms).unwrap_or(now_ms)
            };
            state.tracks.insert(
                contact.id,
                Track {
                    x: contact.x,
                    y: contact.y,
                    since_ms,
                },
            );
            frozen.insert(contact.id, (!moved, now_ms.saturating_sub(since_ms)));
        }

        let current_count = unique.len();
        if current_count >= 2 {
            if current_count > state.peak_down {
                state.peak_down = current_count;
                state.peak_ids = present.clone();
            } else {
                state.peak_ids.extend(present.iter().copied());
            }
        }

        if enabled {
            // Learn a sole contact that remained byte-identical for long enough
            // to be firmware residue.  A real two-finger hold is never affected.
            if current_count == 1 {
                let contact = unique[0];
                if let Some((still, duration)) = frozen.get(&contact.id) {
                    if *still && *duration >= LEARN_MS {
                        learn(state, contact.x, contact.y);
                    }
                }
            }

            // When a multi-finger press collapses, frozen members of the peak
            // are the slots that failed to report tip-up.  Moving members stay.
            if state.peak_down > current_count {
                let mut kept = Vec::with_capacity(unique.len());
                for contact in unique {
                    let residue = state.peak_ids.contains(&contact.id);
                    let still = frozen
                        .get(&contact.id)
                        .map(|value| value.0)
                        .unwrap_or(false);
                    if residue && still {
                        learn(state, contact.x, contact.y);
                    } else {
                        kept.push(contact);
                    }
                }
                unique = kept;
            }

            // Learned coordinates remain suppressed until a moving finger
            // reclaims them, matching the source's durable residue block.
            unique.retain(|contact| {
                let still = frozen
                    .get(&contact.id)
                    .map(|value| value.0)
                    .unwrap_or(false);
                if !still {
                    state
                        .phantoms
                        .retain(|(x, y)| !near(*x, contact.x) || !near(*y, contact.y));
                    true
                } else {
                    !state
                        .phantoms
                        .iter()
                        .any(|(x, y)| near(*x, contact.x) && near(*y, contact.y))
                }
            });
        } else {
            // Turning the diagnostic switch off must not leave learned state to
            // surprise the next gesture after it is turned back on.
            state.phantoms.clear();
            state.peak_ids.clear();
            state.peak_down = current_count;
        }

        if unique.is_empty() {
            state.peak_ids.clear();
            state.peak_down = 0;
        }
        unique
    }
}

fn near(left: i32, right: i32) -> bool {
    left.saturating_sub(right).unsigned_abs() <= PHANTOM_TOL as u32
}

fn learn(state: &mut DeviceState, x: i32, y: i32) {
    if !state
        .phantoms
        .iter()
        .any(|(px, py)| near(*px, x) && near(*py, y))
    {
        state.phantoms.push((x, y));
        if state.phantoms.len() > 8 {
            state.phantoms.remove(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PhantomFilter;
    use crate::gesture::Contact;

    fn contact(id: i32, x: i32, y: i32) -> Contact {
        Contact { id, x, y }
    }

    #[test]
    fn duplicate_ids_are_collapsed_even_when_rejection_is_off() {
        let mut filter = PhantomFilter::default();
        let result = filter.filter(
            "touchpad",
            vec![contact(1, 10, 10), contact(1, 20, 20)],
            false,
            1,
        );
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn frozen_lone_contact_is_learned_and_removed() {
        let mut filter = PhantomFilter::default();
        assert_eq!(
            filter
                .filter("touchpad", vec![contact(1, 10, 10)], true, 0)
                .len(),
            1
        );
        assert!(filter
            .filter("touchpad", vec![contact(1, 10, 10)], true, 1_300)
            .is_empty());
        assert!(filter
            .filter("touchpad", vec![contact(1, 10, 10)], true, 1_400)
            .is_empty());
    }
}
