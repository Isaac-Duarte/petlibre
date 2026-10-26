//! The panel's child lock between the device API and the panel's owner,
//! both in the loop. The owner publishes the lock it holds with
//! [`LockBox::publish`] and takes a queued change with [`LockBox::take`] on
//! its next step; a signed `lock` request reaches only [`LockBox::request`],
//! which reads the published lock or queues one change, so a second change
//! before the owner takes the first is `busy`, never queued behind it.
//! Until the owner's first step there is no lock, and every request is
//! `unserved`.

use crate::mailbox::{LockFields, LockResult};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LockBox {
    /// The lock the owner last published.
    published: Option<bool>,
    /// The queued change: true to lock.
    queued: Option<bool>,
}

impl LockBox {
    pub const fn new() -> Self {
        Self {
            published: None,
            queued: None,
        }
    }

    /// The owner's half: the lock it now holds. The first call starts
    /// serving requests.
    pub fn publish(&mut self, locked: bool) {
        self.published = Some(locked);
    }

    /// The owner's half: the queued change, if any, which frees the slot.
    pub fn take(&mut self) -> Option<bool> {
        self.queued.take()
    }

    /// The device API's half: read the lock, or queue a change for the owner.
    pub fn request(&mut self, fields: LockFields) -> LockResult {
        let Some(locked) = self.published else {
            return LockResult::Unserved;
        };
        let Some(want) = fields.locked else {
            return LockResult::Current {
                locked,
                pending: self.queued.is_some(),
            };
        };
        if self.queued.is_some() {
            return LockResult::Busy;
        }
        self.queued = Some(want);
        LockResult::Current {
            locked,
            pending: true,
        }
    }
}
