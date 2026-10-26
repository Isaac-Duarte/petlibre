//! The loop's own state: the panel-lock box between the device API and the
//! panel's owner, and the record lines' tags.

use core::fmt;

use wellfed_device_api::mailbox::Lock;
use wellfed_device_api::mailbox::{LockFields, LockResult};
use wellfed_feeder::feeder::Record;
use wellfed_feeder::feeder::identity;
use wellfed_feeder::feeder::lock::LockBox;
use wellfed_feeder::feeder::owner;
use wellfed_feeder::feeder::platform::DriverRecord;
use wellfed_feeder::feeder::serve;
use wellfed_feeder::feeder::setup;

const READ: LockFields = LockFields { locked: None };

fn set(locked: bool) -> LockFields {
    LockFields {
        locked: Some(locked),
    }
}

#[test]
fn the_lock_box_holds_one_change_for_the_owner() {
    let mut lock = LockBox::new();
    assert_eq!(lock.request(READ), LockResult::Unserved);
    assert_eq!(lock.request(set(false)), LockResult::Unserved);
    assert_eq!(lock.take(), None, "nothing queues without an owner");

    lock.publish(true);
    assert_eq!(
        lock.request(set(false)),
        LockResult::Current {
            locked: true,
            pending: true
        }
    );
    assert_eq!(lock.request(set(true)), LockResult::Busy);
    assert_eq!(
        lock.request(READ),
        LockResult::Current {
            locked: true,
            pending: true
        }
    );

    assert_eq!(lock.take(), Some(false));
    assert_eq!(lock.take(), None);
    lock.publish(false);
    assert_eq!(
        lock.request(READ),
        LockResult::Current {
            locked: false,
            pending: false
        }
    );
}

/// A driver's own record, as a platform's BLE link would make one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Link;

impl DriverRecord for Link {
    fn tag(&self) -> &'static str {
        "PLBLE1"
    }
}

impl fmt::Display for Link {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ready result=0")
    }
}

/// Each record as the firmware writes its line: the tag, a space, the body.
fn line(record: Record<Link>) -> String {
    format!("{} {record}", record.tag())
}

#[test]
fn records_carry_the_tags_of_their_lines() {
    let identity = identity::Record {
        generation: 3,
        fingerprint: [0xab; identity::FINGERPRINT],
    };
    assert_eq!(
        line(Record::Identity(identity)),
        "PLID1 identity generation=3 fingerprint=abababababababab"
    );
    assert_eq!(
        line(Record::Owner(owner::Record::Start { lock: Lock::Locked })),
        "PLPANEL1 start lock=locked"
    );
    assert_eq!(
        line(Record::Owner(owner::Record::Setup(setup::Record::Start))),
        "PLSET1 start"
    );
    assert_eq!(
        line(Record::Owner(owner::Record::Setup(setup::Record::Driver(
            Link
        )))),
        "PLBLE1 ready result=0"
    );
    assert_eq!(
        line(Record::Serve(serve::Record::Address { ip: 0x0a00_0002 })),
        "PLAPI1 mdns address=10.0.0.2 port=4747"
    );
}
