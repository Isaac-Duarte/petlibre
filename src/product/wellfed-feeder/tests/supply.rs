//! The supply in the loop: its readings on the wire with their
//! uncertainty, its changes as rate-limited events, the brownout hold, and
//! where the battery's reason sits in the one readiness decision.

use wellfed_core::supply::{Pack, Reading};
use wellfed_feeder::feeder::feeding::readiness;
use wellfed_feeder::feeder::supply::{EVENT_MS, Watch, supply};
use wellfed_feeder::feeder::{Feeds, NoFeeds};
use wellfed_proto::api::{Battery, Blocked, Readiness, Refusal, ResetCause, Source, Supply};

fn on(adapter: Option<bool>, below_floor: Option<bool>) -> Supply {
    Supply {
        adapter,
        battery: Battery::Reading { mv: 4757 },
        below_floor,
    }
}

#[test]
fn every_reading_keeps_its_uncertainty_on_the_wire() {
    let cases = [
        (Pack::Absent, Battery::Absent),
        (Pack::Reading { mv: 4757 }, Battery::Reading { mv: 4757 }),
        (
            Pack::Implausible { mv: 2475 },
            Battery::Implausible { mv: 2475 },
        ),
        (Pack::Unread, Battery::Unread),
        (
            Pack::Stale { age_ms: 15_001 },
            Battery::Stale { age_ms: 15_001 },
        ),
    ];
    for (pack, battery) in cases {
        let reading = Reading {
            adapter: None,
            pack,
        };
        assert_eq!(
            supply(reading, Some(true)),
            Supply {
                adapter: None,
                battery,
                below_floor: Some(true),
            }
        );
    }
}

#[test]
fn the_first_reading_is_a_baseline_and_a_change_is_recorded_at_once() {
    let mut watch = Watch::new();
    let reset = ResetCause::PowerOn;
    assert_eq!(watch.note(0, reset, on(Some(true), None)), None);
    // The owner's first verdict, above the floor, is no change.
    assert_eq!(watch.note(20, reset, on(Some(true), Some(false))), None);
    assert_eq!(watch.note(40, reset, on(Some(true), Some(false))), None);

    let pulled = on(Some(false), Some(false));
    assert_eq!(watch.note(60, reset, pulled), Some(pulled));
    assert_eq!(watch.note(80, reset, pulled), None, "once a change");
}

/// An adapter that chatters costs one event a minute, recorded as the
/// supply stands once the minute is up; a return to what was recorded
/// inside the minute records nothing.
#[test]
fn changes_inside_a_minute_are_recorded_together_at_its_end() {
    let mut watch = Watch::new();
    let reset = ResetCause::PowerOn;
    watch.note(0, reset, on(Some(true), Some(false)));
    let pulled = on(Some(false), Some(false));
    assert_eq!(watch.note(1000, reset, pulled), Some(pulled));

    for at in (1020..1000 + EVENT_MS).step_by(500) {
        let adapter = (at / 500) % 2 == 0;
        assert_eq!(watch.note(at, reset, on(Some(adapter), Some(false))), None);
    }
    let low = on(Some(true), Some(true));
    assert_eq!(watch.note(1000 + EVENT_MS, reset, low), Some(low));

    let back = on(Some(false), Some(true));
    assert_eq!(watch.note(2000 + EVENT_MS, reset, back), None);
    assert_eq!(watch.note(3000 + EVENT_MS, reset, low), None);
    assert_eq!(watch.note(1000 + 3 * EVENT_MS, reset, low), None);
}

#[test]
fn the_floor_crossing_is_recorded_and_a_stale_sample_is_not() {
    let mut watch = Watch::new();
    let reset = ResetCause::PowerOn;
    watch.note(0, reset, on(Some(false), Some(false)));

    let stale = Supply {
        adapter: None,
        battery: Battery::Stale { age_ms: 15_001 },
        below_floor: Some(true),
    };
    assert_eq!(watch.note(100, reset, stale), None);

    let low = on(Some(false), Some(true));
    assert_eq!(watch.note(200, reset, low), Some(low));
    // A firmware without an owner never judges: no floor change.
    let unjudged = on(Some(false), None);
    assert_eq!(watch.note(200 + EVENT_MS, reset, unjudged), None);
}

#[test]
fn a_brownout_holds_feeding_until_the_adapter_is_seen() {
    let mut watch = Watch::new();
    assert!(!watch.holds(), "unarmed before the first pass");
    watch.note(0, ResetCause::Brownout, on(Some(false), Some(false)));
    assert!(watch.holds());
    watch.note(20, ResetCause::Brownout, on(None, None));
    assert!(watch.holds(), "an unknown adapter is not seen");
    watch.note(40, ResetCause::Brownout, on(Some(true), Some(false)));
    assert!(!watch.holds());
    watch.note(60, ResetCause::Brownout, on(Some(false), Some(false)));
    assert!(!watch.holds(), "lifted for the rest of the boot");

    let mut watch = Watch::new();
    for cause in [
        ResetCause::PowerOn,
        ResetCause::Watchdog,
        ResetCause::Software,
        ResetCause::Other,
    ] {
        watch.note(0, cause, on(Some(false), Some(false)));
        assert!(!watch.holds(), "{cause:?}");
    }
}

/// A feeding owner that answers with a reason.
struct Owner(Option<Blocked>);

impl Feeds for Owner {
    fn feeding(&self) -> bool {
        false
    }

    fn busy(&self) -> bool {
        false
    }

    fn blocked(&self) -> Option<Blocked> {
        self.0
    }

    fn submit(&mut self, _: u8, _: Source) -> Result<u32, Refusal> {
        Ok(1)
    }

    fn reserved(&self, _: u32) -> Option<Result<(), Refusal>> {
        Some(Ok(()))
    }

    fn cancel(&self, _: u32) -> bool {
        false
    }
}

#[test]
fn the_battery_reason_sits_after_the_lasting_ones_and_refuses_busy() {
    let battery = Readiness::Blocked(Blocked::Battery);
    assert_eq!(
        readiness(&Owner(None), false, false, false),
        Readiness::Ready
    );
    assert_eq!(readiness(&Owner(None), false, false, true), battery);
    assert_eq!(
        readiness(&Owner(Some(Blocked::Battery)), false, false, false),
        battery
    );
    assert_eq!(
        readiness(&Owner(Some(Blocked::Fault)), false, false, true),
        Readiness::Blocked(Blocked::Fault)
    );
    assert_eq!(
        readiness(&Owner(None), true, false, true),
        Readiness::Blocked(Blocked::Setup)
    );
    // The owner's passing reason, the more specific one, wins over the hold.
    assert_eq!(
        readiness(&Owner(Some(Blocked::Feeding)), false, false, true),
        Readiness::Blocked(Blocked::Feeding)
    );
    assert_eq!(
        readiness(&NoFeeds, false, false, true),
        Readiness::Blocked(Blocked::Unserved)
    );

    assert_eq!(Blocked::Battery.refusal(), Refusal::Busy);
    assert!(!Blocked::Battery.lasting());
}
