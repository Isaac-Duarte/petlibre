//! A feed's outcome keeps the event queue's reserved slot on this board:
//! once the shared slots are full a plain record is lost and counted, and
//! the feed's `Fed` event still queues.

use wellfed_feeder::api::Device;
use wellfed_feeder::feeder::{Feeder, NoFeeds};
use wellfed_feeder_runtime::events::{self, QUEUE, RESERVED};
use wellfed_fw_feeder::platform::Plaf103;
use wellfed_proto::api::{EventBody, Source};
use wellfed_sdk::hal::sdk::fake;

const LOCK: EventBody = EventBody::Lock {
    locked: true,
    by: Source::Panel,
};

#[test]
fn the_reserved_slot_takes_only_a_feed_s_outcome() {
    fake::reset_hardware();
    events::reset();
    let mut feeder = Box::new(Feeder::new(Plaf103::new()));
    let mut feeds = NoFeeds;
    // The boot's own event takes the first shared slot.
    {
        let mut device = feeder.device(0, &mut feeds);
        for _ in 1..QUEUE - RESERVED {
            device.record(0, LOCK);
        }
    }
    assert_eq!(events::take_lost(), 0, "the shared slots hold every record");

    feeder.device(0, &mut feeds).record(0, LOCK);
    assert_eq!(events::take_lost(), 1, "a plain record past them is lost");

    feeder.record_outcome(0, LOCK);
    assert_eq!(
        events::take_lost(),
        0,
        "the outcome takes the reserved slot"
    );
    feeder.record_outcome(0, LOCK);
    assert_eq!(events::take_lost(), 1, "and the queue is full");
}
