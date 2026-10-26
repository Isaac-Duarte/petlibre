//! The panel-lock mailbox: unserved until an owner publishes, one queued
//! change at a time, and a read that reports whether a change is pending.

use wellfed_feeder::mailbox::{LockFields, LockResult};
use wellfed_feeder_runtime::panel;

const READ: LockFields = LockFields { locked: None };

fn set(locked: bool) -> LockFields {
    LockFields {
        locked: Some(locked),
    }
}

#[test]
fn the_mailbox_holds_one_change_for_the_owner() {
    panel::reset();
    assert_eq!(panel::request(READ), LockResult::Unserved);
    assert_eq!(panel::request(set(false)), LockResult::Unserved);
    assert_eq!(panel::take(), None, "nothing queues without an owner");

    panel::publish(true);
    assert_eq!(
        panel::request(set(false)),
        LockResult::Current {
            locked: true,
            pending: true
        }
    );
    assert_eq!(panel::request(set(true)), LockResult::Busy);
    assert_eq!(
        panel::request(READ),
        LockResult::Current {
            locked: true,
            pending: true
        }
    );

    assert_eq!(panel::take(), Some(false));
    assert_eq!(panel::take(), None);
    panel::publish(false);
    assert_eq!(
        panel::request(READ),
        LockResult::Current {
            locked: false,
            pending: false
        }
    );
    panel::reset();
}
