//! The supply in the loop: its readings on the wire with their
//! uncertainty, its changes as rate-limited events, battery mode's radio,
//! the brownout hold and how it lifts, and where the battery's reason sits
//! in the one readiness decision.

use wellfed_core::supply::{Pack, Reading};
use wellfed_feeder::feeder::feeding::readiness;
use wellfed_feeder::feeder::supply::{
    EVENT_MS, RADIO_OFF_MS, RELEASE_MS, RELEASE_PERCENT, Radio, Record, Released, Watch, supply,
};
use wellfed_feeder::feeder::{Feeds, NoFeeds};
use wellfed_proto::api::{Battery, Blocked, Readiness, Refusal, ResetCause, Source, Supply};

const FRESH: Battery = Battery::Reading {
    mv: 4757,
    percent: 100,
};

fn on(adapter: Option<bool>, below_floor: Option<bool>) -> Supply {
    Supply {
        adapter,
        battery: FRESH,
        below_floor,
    }
}

fn pack(adapter: Option<bool>, battery: Battery) -> Supply {
    Supply {
        adapter,
        battery,
        below_floor: Some(false),
    }
}

/// A pack at `percent` on stock's curve.
fn charged(percent: u8) -> Battery {
    Battery::Reading { mv: 4000, percent }
}

/// Note `supply` with no interest in the records.
fn note(watch: &mut Watch, at: u32, reset: ResetCause, supply: Supply) -> Option<Supply> {
    watch.note(at, reset, supply, |_| {})
}

/// The radio after noting `supply` at `at`, outside setup mode.
fn radio_at(watch: &mut Watch, at: u32, supply: Supply) -> Radio {
    note(watch, at, ResetCause::PowerOn, supply);
    watch.radio(false, |_| {})
}

#[test]
fn every_reading_keeps_its_uncertainty_on_the_wire() {
    let cases = [
        (Pack::Absent, Battery::Absent),
        (
            Pack::Reading {
                mv: 4757,
                percent: 100,
            },
            FRESH,
        ),
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
    assert_eq!(note(&mut watch, 0, reset, on(Some(true), None)), None);
    // The owner's first verdict, above the floor, is no change.
    assert_eq!(
        note(&mut watch, 20, reset, on(Some(true), Some(false))),
        None
    );
    assert_eq!(
        note(&mut watch, 40, reset, on(Some(true), Some(false))),
        None
    );

    let pulled = on(Some(false), Some(false));
    assert_eq!(note(&mut watch, 60, reset, pulled), Some(pulled));
    assert_eq!(note(&mut watch, 80, reset, pulled), None, "once a change");
}

/// An adapter that chatters costs one event a minute, recorded as the
/// supply stands once the minute is up; a return to what was recorded
/// inside the minute records nothing.
#[test]
fn changes_inside_a_minute_are_recorded_together_at_its_end() {
    let mut watch = Watch::new();
    let reset = ResetCause::PowerOn;
    note(&mut watch, 0, reset, on(Some(true), Some(false)));
    let pulled = on(Some(false), Some(false));
    assert_eq!(note(&mut watch, 1000, reset, pulled), Some(pulled));

    for at in (1020..1000 + EVENT_MS).step_by(500) {
        let adapter = (at / 500) % 2 == 0;
        assert_eq!(
            note(&mut watch, at, reset, on(Some(adapter), Some(false))),
            None
        );
    }
    let low = on(Some(true), Some(true));
    assert_eq!(note(&mut watch, 1000 + EVENT_MS, reset, low), Some(low));

    let back = on(Some(false), Some(true));
    assert_eq!(note(&mut watch, 2000 + EVENT_MS, reset, back), None);
    assert_eq!(note(&mut watch, 3000 + EVENT_MS, reset, low), None);
    assert_eq!(note(&mut watch, 1000 + 3 * EVENT_MS, reset, low), None);
}

#[test]
fn the_floor_crossing_is_recorded_and_a_stale_sample_is_not() {
    let mut watch = Watch::new();
    let reset = ResetCause::PowerOn;
    note(&mut watch, 0, reset, on(Some(false), Some(false)));

    let stale = Supply {
        adapter: None,
        battery: Battery::Stale { age_ms: 15_001 },
        below_floor: Some(true),
    };
    assert_eq!(note(&mut watch, 100, reset, stale), None);

    let low = on(Some(false), Some(true));
    assert_eq!(note(&mut watch, 200, reset, low), Some(low));
    // A firmware without an owner never judges: no floor change.
    let unjudged = on(Some(false), None);
    assert_eq!(note(&mut watch, 200 + EVENT_MS, reset, unjudged), None);
}

/// Stock's battery mode: the radio goes off once the adapter has been gone
/// for 5 s, and comes back the moment it returns.
#[test]
fn the_radio_goes_off_five_seconds_after_the_adapter_and_back_with_it() {
    let mut watch = Watch::new();
    let mut records = Vec::new();
    let mut step = |watch: &mut Watch, at: u32, adapter: bool| {
        let supply = on(Some(adapter), Some(false));
        watch.note(at, ResetCause::PowerOn, supply, |r| records.push(r));
        watch.radio(false, |r| records.push(r))
    };

    assert_eq!(step(&mut watch, 0, true), Radio::On);
    assert_eq!(step(&mut watch, 1_000, false), Radio::On);
    assert_eq!(step(&mut watch, 1_000 + RADIO_OFF_MS - 1, false), Radio::On);
    assert!(!watch.on_battery());
    assert_eq!(step(&mut watch, 1_000 + RADIO_OFF_MS, false), Radio::Off);
    assert!(watch.on_battery());
    assert_eq!(step(&mut watch, 60_000, false), Radio::Off);

    assert_eq!(step(&mut watch, 60_020, true), Radio::On, "at once");
    assert!(!watch.on_battery());

    // A blip shorter than the wait leaves the radio alone, and the next
    // loss waits its own 5 s.
    assert_eq!(step(&mut watch, 61_000, false), Radio::On);
    assert_eq!(step(&mut watch, 63_000, true), Radio::On);
    assert_eq!(step(&mut watch, 64_000, false), Radio::On);
    assert_eq!(
        step(&mut watch, 64_000 + RADIO_OFF_MS - 1, false),
        Radio::On
    );
    assert_eq!(step(&mut watch, 64_000 + RADIO_OFF_MS, false), Radio::Off);
    assert_eq!(
        records,
        [
            Record::Radio(Radio::Off),
            Record::Radio(Radio::On),
            Record::Radio(Radio::Off),
        ],
        "a record per change"
    );
}

/// A boot on the batteries alone goes to battery mode too, and the wait
/// survives the uptime's wrap.
#[test]
fn a_boot_on_batteries_turns_the_radio_off_after_the_wait() {
    let mut watch = Watch::new();
    let start = u32::MAX - 1_000;
    assert_eq!(
        radio_at(&mut watch, start, on(Some(false), None)),
        Radio::On
    );
    let later = start.wrapping_add(RADIO_OFF_MS);
    assert_eq!(
        radio_at(&mut watch, later, on(Some(false), None)),
        Radio::Off
    );
}

/// Only a fresh sample with a pack reading moves battery mode: an unknown
/// adapter, or a pack that could not run the board on its own, neither
/// starts the wait nor ends it.
#[test]
fn only_a_fresh_pack_reading_moves_battery_mode() {
    let mut watch = Watch::new();
    radio_at(&mut watch, 0, on(Some(true), None));
    let unjudged = [
        (1_000, Battery::Absent),
        (2_000, Battery::Implausible { mv: 2_000 }),
        (3_000, Battery::Unread),
    ];
    for (at, battery) in unjudged {
        assert_eq!(
            radio_at(&mut watch, at, pack(Some(false), battery)),
            Radio::On
        );
    }
    let absent = pack(Some(false), Battery::Absent);
    assert_eq!(radio_at(&mut watch, 60_000, absent), Radio::On, "no wait");

    // Once started, the wait runs on through unknowns.
    radio_at(&mut watch, 70_000, on(Some(false), None));
    let stale = pack(None, Battery::Stale { age_ms: 15_001 });
    assert_eq!(radio_at(&mut watch, 72_000, stale), Radio::On);
    let due = 70_000 + RADIO_OFF_MS;
    assert_eq!(radio_at(&mut watch, due, on(Some(false), None)), Radio::Off);

    // On the batteries an unknown changes nothing either.
    assert_eq!(radio_at(&mut watch, 80_000, stale), Radio::Off);
    assert_eq!(radio_at(&mut watch, 81_000, absent), Radio::Off);
}

/// Setup mode keeps the radio on: the vendor's BLE stack needs the WLAN
/// driver up. It goes off again once setup mode ends.
#[test]
fn setup_mode_keeps_the_radio_on() {
    let mut watch = Watch::new();
    let reset = ResetCause::PowerOn;
    note(&mut watch, 0, reset, on(Some(false), None));
    note(&mut watch, RADIO_OFF_MS, reset, on(Some(false), None));
    assert!(watch.on_battery());
    assert_eq!(watch.radio(true, |_| {}), Radio::On);
    assert_eq!(watch.radio(false, |_| {}), Radio::Off);
}

/// The phantom battery at a power cut: with no cells fitted, cutting the
/// adapter reads the decaying 5 V rail as a plausible pack for the moment
/// before the board stops. It must not count as battery mode, nor arm or
/// lift anything that outlives it.
#[test]
fn a_decaying_rail_at_a_cut_is_not_battery_mode() {
    let mut watch = Watch::new();
    let mut records = Vec::new();
    let reset = ResetCause::PowerOn;
    let mains = pack(Some(true), Battery::Absent);
    watch.note(0, reset, mains, |r| records.push(r));

    // The rail as it decays, on the owner's 20 ms polls, then gone.
    let mut at = 1_000;
    for mv in [3861, 3500, 3100, 2700] {
        let rail = pack(Some(false), Battery::Reading { mv, percent: 100 });
        watch.note(at, reset, rail, |r| records.push(r));
        assert_eq!(watch.radio(false, |r| records.push(r)), Radio::On);
        at += 20;
    }
    let gone = pack(Some(false), Battery::Implausible { mv: 2_100 });
    watch.note(1_100, reset, gone, |r| records.push(r));
    assert_eq!(watch.radio(false, |r| records.push(r)), Radio::On);
    assert!(!watch.holds());
    assert!(records.is_empty(), "{records:?}");

    // A relay blip the board rode through: back on the adapter, nothing
    // left behind.
    watch.note(1_200, reset, mains, |r| records.push(r));
    assert_eq!(watch.radio(false, |r| records.push(r)), Radio::On);
    assert!(!watch.on_battery());

    // A stale brownout bit on the next boot, on the adapter with no cells:
    // the first pass lifts it.
    let mut watch = Watch::new();
    note(&mut watch, 0, ResetCause::Brownout, mains);
    assert!(!watch.holds());
}

#[test]
fn a_brownout_holds_feeding_until_the_adapter_is_seen() {
    let mut watch = Watch::new();
    let mut records = Vec::new();
    let reset = ResetCause::Brownout;
    assert!(!watch.holds(), "unarmed before the first pass");
    watch.note(0, reset, on(Some(false), Some(false)), |r| records.push(r));
    assert!(watch.holds());
    note(&mut watch, 20, reset, on(None, None));
    assert!(watch.holds(), "an unknown adapter is not seen");
    watch.note(40, reset, on(Some(true), Some(false)), |r| records.push(r));
    assert!(!watch.holds());
    note(&mut watch, 60, reset, on(Some(false), Some(false)));
    assert!(!watch.holds(), "lifted for the rest of the boot");
    assert_eq!(records, [Record::Held, Record::Released(Released::Adapter)]);

    let mut watch = Watch::new();
    for cause in [
        ResetCause::PowerOn,
        ResetCause::Watchdog,
        ResetCause::Software,
        ResetCause::Other,
    ] {
        note(&mut watch, 0, cause, on(Some(false), Some(false)));
        assert!(!watch.holds(), "{cause:?}");
    }
}

/// On the batteries alone the hold lifts once every fresh reading has
/// been at the release charge or more for three minutes: fresh cells.
#[test]
fn a_brownout_hold_lifts_on_a_pack_that_stays_charged() {
    let mut watch = Watch::new();
    let mut records = Vec::new();
    let mut step = |watch: &mut Watch, at: u32, supply: Supply| {
        watch.note(at, ResetCause::Brownout, supply, |r| records.push(r));
        watch.holds()
    };
    let good = pack(Some(false), charged(RELEASE_PERCENT));

    assert!(step(&mut watch, 0, good));
    for at in (5_000..RELEASE_MS).step_by(5_000) {
        assert!(step(&mut watch, at, good), "at {at} ms");
    }
    assert!(step(&mut watch, RELEASE_MS - 1, good));
    assert!(!step(&mut watch, RELEASE_MS, good));
    let weak = pack(Some(false), charged(10));
    assert!(!step(&mut watch, RELEASE_MS + 5_000, weak), "for good");
    assert_eq!(records, [Record::Held, Record::Released(Released::Pack)]);
}

/// Any reading under the release charge, or any sample that is not a
/// fresh pack reading, starts the three minutes again.
#[test]
fn a_dip_or_an_unknown_restarts_the_release_wait() {
    let interruptions = [
        pack(Some(false), charged(RELEASE_PERCENT - 10)),
        pack(None, Battery::Stale { age_ms: 15_001 }),
        pack(Some(false), Battery::Unread),
        pack(Some(false), Battery::Implausible { mv: 2_000 }),
        pack(Some(false), Battery::Absent),
    ];
    let good = pack(Some(false), charged(100));
    let reset = ResetCause::Brownout;
    let resumed = RELEASE_MS / 2 + 5_000;
    for interruption in interruptions {
        let mut watch = Watch::new();
        note(&mut watch, 0, reset, good);
        note(&mut watch, RELEASE_MS / 2, reset, interruption);
        note(&mut watch, resumed, reset, good);
        note(&mut watch, RELEASE_MS, reset, good);
        assert!(watch.holds(), "{interruption:?}");
        note(&mut watch, resumed + RELEASE_MS, reset, good);
        assert!(!watch.holds(), "{interruption:?}");
    }
}

/// A recovered pack lifts the hold only on the batteries: a brownout that
/// left the radio off still releases, and the radio rule is unaffected.
#[test]
fn a_held_battery_feeder_still_drops_its_radio() {
    let mut watch = Watch::new();
    let good = pack(Some(false), charged(100));
    note(&mut watch, 0, ResetCause::Brownout, good);
    note(&mut watch, RADIO_OFF_MS, ResetCause::Brownout, good);
    assert!(watch.holds());
    assert_eq!(watch.radio(false, |_| {}), Radio::Off);
}

#[test]
fn records_name_what_happened() {
    struct Line(Record);
    impl std::fmt::Display for Line {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            wellfed_feeder::feeder::supply::describe(self.0, f)
        }
    }
    let lines = [
        (Record::Radio(Radio::Off), "radio off"),
        (Record::Radio(Radio::On), "radio on"),
        (Record::Held, "hold brownout"),
        (
            Record::Released(Released::Adapter),
            "hold released by=adapter",
        ),
        (Record::Released(Released::Pack), "hold released by=pack"),
    ];
    for (record, line) in lines {
        assert_eq!(Line(record).to_string(), line);
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
