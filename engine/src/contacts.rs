use crate::gesture::Contact;

#[derive(Debug, Clone, PartialEq)]
pub enum AssembledContacts {
    Ignored,
    Pending,
    Complete(Vec<Contact>),
    CompleteThenPending {
        complete: Vec<Contact>,
        pending: Vec<Contact>,
        target: u32,
    },
    CompleteThenComplete {
        first: Vec<Contact>,
        second: Vec<Contact>,
    },
}

#[derive(Debug, Default)]
pub struct ContactAssembler {
    pending: Vec<Contact>,
    target_count: u32,
}

impl ContactAssembler {
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    pub fn accept(&mut self, mut contacts: Vec<Contact>, reported_count: u32) -> AssembledContacts {
        if contacts.is_empty() {
            return AssembledContacts::Ignored;
        }

        if reported_count as usize == contacts.len() {
            self.pending.clear();
            return AssembledContacts::Complete(contacts);
        }

        if reported_count == 0 {
            self.pending.extend(contacts);
            remove_duplicate_ids(&mut self.pending);

            if self.target_count == 0 {
                return AssembledContacts::Pending;
            }

            if self.pending.len() > self.target_count as usize {
                self.pending.truncate(self.target_count as usize);
            }
            if self.pending.len() == self.target_count as usize {
                return AssembledContacts::Complete(std::mem::take(&mut self.pending));
            }
            return AssembledContacts::Pending;
        }

        let previous = if self.pending.is_empty() {
            None
        } else {
            if self.pending.len() < self.target_count as usize {
                let last = *self.pending.last().expect("pending is non-empty");
                let maximum_id = self
                    .pending
                    .iter()
                    .map(|contact| contact.id)
                    .max()
                    .unwrap_or(last.id);
                let missing = self.target_count as usize - self.pending.len();
                for offset in 1..=missing {
                    self.pending.push(Contact {
                        id: maximum_id + offset as i32,
                        x: last.x,
                        y: last.y,
                    });
                }
            } else if self.pending.len() > self.target_count as usize {
                self.pending.truncate(self.target_count as usize);
            }
            Some(std::mem::take(&mut self.pending))
        };

        if reported_count as usize <= contacts.len() {
            contacts.truncate(reported_count as usize);
            self.pending.clear();
            return match previous {
                Some(first) => AssembledContacts::CompleteThenComplete {
                    first,
                    second: contacts,
                },
                None => AssembledContacts::Complete(contacts),
            };
        }

        self.target_count = reported_count;
        self.pending = contacts;
        match previous {
            Some(complete) => AssembledContacts::CompleteThenPending {
                complete,
                pending: self.pending.clone(),
                target: self.target_count,
            },
            None => AssembledContacts::Pending,
        }
    }
}

fn remove_duplicate_ids(contacts: &mut Vec<Contact>) {
    let mut unique = Vec::with_capacity(contacts.len());
    for contact in contacts.drain(..) {
        if !unique
            .iter()
            .any(|existing: &Contact| existing.id == contact.id)
        {
            unique.push(contact);
        }
    }
    *contacts = unique;
}
