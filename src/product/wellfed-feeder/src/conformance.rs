//! The device conformance suite: what the device API's view of the feeder
//! ([`Device`]) answers on every [`Platform`], so the PLAF103 firmware and
//! the simulator cannot drift apart where a client would see it.
//!
//! A platform's tests implement [`Rig`] (its platform, its settings
//! writer's turn and the outputs' claim) and expand [`conformance!`] into
//! one test per check:
//!
//! ```ignore
//! wellfed_feeder::conformance!(FirmwareRig::start());
//! ```
//!
//! Each check starts from a rig whose writer has opened its stores (the
//! event ring's epoch known) and nothing queued, with the panel never
//! stepped. Built only with the `conformance` feature, for tests.

use wellfed_core::store::slot::crc32;
use wellfed_proto::api::{
    Blocked, Capabilities, CurrentUnit, Event, EventBody, FeedOutcome, FeedReason, Readiness,
    Refusal, Resets, Source, Stats, StatsSection, TraceSummary,
};

use crate::feeder::{Feeder, Feeds, NoFeeds, Platform};
use wellfed_device_api::clients::{Label, Role};
use wellfed_device_api::mailbox::{
    Calibration, CalibrationResult, LockResult, NtpServer, Origin, SettingsFields, SettingsResult,
    SettingsView, TimeFields, TimeResult, TimeSettings, TimeSettingsResult, TimeSettingsView,
    WifiResult, WifiView, Zone,
};
use wellfed_device_api::wifi::Credentials;
use wellfed_device_api::{Device, EventRead, KeyChange, KeyOutcome};

/// Writer turns a check waits for one result, at most.
const TURNS: usize = 64;

/// One platform's feeder, as the suite drives it.
pub trait Rig {
    type Platform: Platform;

    fn feeder(&mut self) -> &mut Feeder<Self::Platform>;
    /// The settings writer's turns, until nothing waits for it.
    fn write(&mut self);
    /// Hold the outputs as a running feed does, or give them back.
    fn claim_outputs(&mut self, claimed: bool);
}

/// One loop pass at `now_ms`, with no feeding owner.
fn pass(rig: &mut impl Rig, now_ms: u32) {
    rig.feeder().pass(now_ms, &mut NoFeeds, |_| {});
}

/// One call on the device API's view at `now_ms`.
fn device<R: Rig, T>(rig: &mut R, now_ms: u32, call: impl FnOnce(&mut dyn Device) -> T) -> T {
    let mut feeds = NoFeeds;
    let mut device = rig.feeder().device(now_ms, &mut feeds);
    call(&mut device)
}

fn settings(rig: &mut impl Rig, change: SettingsFields) -> SettingsResult {
    device(rig, 0, |device| device.settings(change))
}

fn settings_view(rig: &mut impl Rig) -> SettingsView {
    match settings(rig, SettingsFields::default()) {
        SettingsResult::Current {
            view,
            pending: false,
        } => view,
        other => panic!("the settings with nothing queued: {other:?}"),
    }
}

fn wifi(rig: &mut impl Rig, change: Option<Option<Credentials>>) -> WifiResult {
    device(rig, 0, |device| device.wifi(change))
}

fn wifi_view(rig: &mut impl Rig) -> WifiView {
    match wifi(rig, None) {
        WifiResult::Current {
            view,
            pending: false,
        } => view,
        other => panic!("the Wi-Fi store with nothing queued: {other:?}"),
    }
}

fn keys(rig: &mut impl Rig, change: KeyChange) -> Option<u32> {
    device(rig, 0, |device| device.keys(change))
}

fn keys_outcome(rig: &mut impl Rig, ticket: u32) -> Option<KeyOutcome> {
    device(rig, 0, |device| device.keys_outcome(ticket))
}

/// Queue `change`, let the writer take it, and return its outcome.
fn key_change(rig: &mut impl Rig, change: KeyChange) -> KeyOutcome {
    let ticket = keys(rig, change).expect("the client mailbox is free");
    rig.write();
    keys_outcome(rig, ticket).expect("the writer finished the change")
}

fn volume(volume: u8) -> SettingsFields {
    SettingsFields {
        volume: Some(volume),
        ..SettingsFields::default()
    }
}

fn enroll(key: u8) -> KeyChange {
    KeyChange::Enroll {
        key: [key; 32],
        label: Label::new(b"phone").expect("a short label"),
        role: Role::Member,
    }
}

fn credentials() -> Credentials {
    Credentials::new(b"home-net", b"passphrase").expect("valid credentials")
}

/// A settings change answers pending at once and lands on the writer's
/// turn under the next generation; a second change before then is `Busy`.
pub fn a_settings_change_lands_on_the_writer_turn(rig: &mut impl Rig) {
    let before = settings_view(rig);
    assert!(before.writable, "the store opened writable");
    let target = (before.volume + 1) % 100;

    assert!(matches!(
        settings(rig, volume(target)),
        SettingsResult::Current { pending: true, .. }
    ));
    assert_eq!(settings(rig, volume(target)), SettingsResult::Busy);
    rig.write();

    let after = settings_view(rig);
    assert_eq!(after.volume, target);
    assert_eq!(after.generation, before.generation + 1);
    let status = device(rig, 0, |device| device.status(0));
    assert_eq!(status.settings_generation, after.generation);
}

/// Wi-Fi credentials are saved and cleared on the writer's turn, and the
/// store reports only the SSID's checksum.
pub fn wifi_credentials_are_saved_and_cleared(rig: &mut impl Rig) {
    let before = wifi_view(rig);
    assert!(before.writable, "the store opened writable");
    assert_eq!(before.stored_ssid_crc, None);

    assert!(matches!(
        wifi(rig, Some(Some(credentials()))),
        WifiResult::Current { pending: true, .. }
    ));
    assert_eq!(wifi(rig, Some(None)), WifiResult::Busy);
    rig.write();
    let saved = wifi_view(rig);
    assert_eq!(saved.stored_ssid_crc, Some(crc32(b"home-net")));
    assert_eq!(saved.generation, before.generation + 1);

    assert!(matches!(
        wifi(rig, Some(None)),
        WifiResult::Current { pending: true, .. }
    ));
    rig.write();
    let cleared = wifi_view(rig);
    assert_eq!(cleared.stored_ssid_crc, None);
    assert_eq!(cleared.generation, saved.generation + 1);
    let status = device(rig, 0, |device| device.status(0));
    assert_eq!(status.wifi_generation, cleared.generation);
}

/// A client store change gets a ticket whose outcome appears once the
/// writer has made it; one change waits at a time.
pub fn client_changes_answer_by_ticket(rig: &mut impl Rig) {
    let ticket = keys(rig, enroll(7)).expect("the client mailbox is free");
    assert_eq!(keys_outcome(rig, ticket), None, "not written yet");
    assert_eq!(keys(rig, enroll(8)), None, "one change waits at a time");
    rig.write();
    assert_eq!(keys_outcome(rig, ticket), Some(KeyOutcome::Done));

    assert_eq!(key_change(rig, enroll(7)), KeyOutcome::Duplicate);
    assert_eq!(
        key_change(rig, KeyChange::Revoke { key: [9; 32] }),
        KeyOutcome::NotFound
    );
    assert_eq!(
        key_change(rig, KeyChange::Revoke { key: [7; 32] }),
        KeyOutcome::Done
    );
}

/// While a feed holds the outputs, every store write is refused at once
/// (it would wait out the feed, past a client's answer) and reads answer.
pub fn writes_are_refused_while_the_outputs_are_claimed(rig: &mut impl Rig) {
    rig.claim_outputs(true);
    assert_eq!(settings(rig, volume(3)), SettingsResult::Busy);
    assert_eq!(wifi(rig, Some(Some(credentials()))), WifiResult::Busy);
    assert_eq!(keys(rig, enroll(7)), None);
    settings_view(rig);
    wifi_view(rig);

    rig.claim_outputs(false);
    assert!(matches!(
        settings(rig, volume(3)),
        SettingsResult::Current { pending: true, .. }
    ));
    rig.write();
}

/// The lock is served once the panel's owner has stepped: a change is
/// queued for its next step, and a second one before then is `Busy`.
pub fn the_lock_follows_the_panel_owner(rig: &mut impl Rig) {
    assert_eq!(
        device(rig, 0, |device| device.lock(false)),
        LockResult::Unserved
    );

    pass(rig, 0);
    let status = device(rig, 0, |device| device.status(0));
    assert_eq!(status.locked, Some(true), "the panel starts locked");
    assert_eq!(
        device(rig, 0, |device| device.lock(false)),
        LockResult::Current {
            locked: true,
            pending: true
        }
    );
    assert_eq!(device(rig, 0, |device| device.lock(true)), LockResult::Busy);

    pass(rig, 20);
    let status = device(rig, 20, |device| device.status(20));
    assert_eq!(status.locked, Some(false));
}

/// The clock is unknown until a sync, which a bad offset never sets.
pub fn a_time_sync_sets_the_clock(rig: &mut impl Rig) {
    let status = device(rig, 0, |device| device.status(0));
    assert_eq!(status.clock_ms, None);

    let bad = TimeFields {
        unix_ms: 1_800_000_000_000,
        offset_s: i32::MAX,
        next_change_s: 0,
        next_offset_s: 0,
    };
    assert_eq!(
        device(rig, 0, |device| device.time_sync(bad, Origin::Owner)),
        TimeResult::Refused("offset")
    );
    let sync = TimeFields {
        offset_s: 3600,
        next_offset_s: 3600,
        ..bad
    };
    assert_eq!(
        device(rig, 0, |device| device.time_sync(sync, Origin::Owner)),
        TimeResult::Synced { jump_ms: None }
    );

    let status = device(rig, 0, |device| device.status(0));
    let clock = status.clock_ms.expect("the clock runs");
    assert!(clock >= sync.unix_ms, "{clock}");

    // Anyone but an owner may not step a valid clock by an hour.
    let hour_on = TimeFields {
        unix_ms: clock + 3_600_000,
        ..sync
    };
    assert_eq!(
        device(rig, 0, |device| device.time_sync(hour_on, Origin::Client)),
        TimeResult::Refused("bounded")
    );
    assert!(matches!(
        device(rig, 0, |device| device.time_sync(hour_on, Origin::Owner)),
        TimeResult::Synced { jump_ms: Some(_) }
    ));
}

fn time_settings(rig: &mut impl Rig, change: Option<TimeSettings>) -> TimeSettingsResult {
    device(rig, 0, |device| device.time_settings(change))
}

fn time_view(rig: &mut impl Rig) -> TimeSettingsView {
    match time_settings(rig, None) {
        TimeSettingsResult::Current {
            view,
            pending: false,
        } => view,
        other => panic!("the time settings with nothing queued: {other:?}"),
    }
}

/// The time settings start at their defaults, a change lands on the
/// writer's turn, and the next pass gives the clock the zone's rule.
pub fn time_settings_land_and_set_the_zone(rig: &mut impl Rig) {
    let before = time_view(rig);
    assert!(before.writable, "the store opened writable");
    assert_eq!(before.settings, TimeSettings::default());

    let zone = Zone::parse(b"<+0530>-5:30").expect("a fixed rule");
    let target = TimeSettings {
        zone: Some(zone),
        ntp: NtpServer::Off,
    };
    assert!(matches!(
        time_settings(rig, Some(target)),
        TimeSettingsResult::Current { pending: true, .. }
    ));
    assert_eq!(time_settings(rig, Some(target)), TimeSettingsResult::Busy);
    rig.write();
    let after = time_view(rig);
    assert_eq!(after.settings, target);
    assert_eq!(after.generation, before.generation + 1);

    // An NTP fix gives no offset; the zone's rule does, once applied.
    pass(rig, 0);
    let sync = TimeFields {
        unix_ms: 1_800_000_000_000,
        offset_s: 0,
        next_change_s: 0,
        next_offset_s: 0,
    };
    device(rig, 0, |device| device.time_sync(sync, Origin::Owner));
    let state = device(rig, 0, |device| device.time_state(0)).expect("the clock's state");
    let reading = state.clock.reading.expect("a valid clock");
    assert_eq!(reading.offset_s, Some(5 * 3600 + 1800));

    rig.claim_outputs(true);
    assert_eq!(
        time_settings(rig, Some(before.settings)),
        TimeSettingsResult::Busy
    );
    rig.claim_outputs(false);
}

fn calibration(rig: &mut impl Rig, change: Option<Calibration>) -> CalibrationResult {
    device(rig, 0, |device| device.calibration(change))
}

/// The calibration starts with nothing measured, a weight lands on the
/// writer's turn and weighs the portions statistics, and none is queued
/// while the outputs are claimed.
pub fn a_calibration_lands_and_weighs_the_portions(rig: &mut impl Rig) {
    let CalibrationResult::Current {
        view: before,
        pending: false,
    } = calibration(rig, None)
    else {
        panic!("the calibration with nothing queued");
    };
    assert!(before.writable, "the store opened writable");
    assert_eq!(before.calibration, Calibration::default());
    assert!(device(rig, 0, |device| device.capabilities()).contains(Capabilities::CALIBRATION));

    let weighed = Calibration {
        portion_mg: Some(9_800),
    };
    assert!(matches!(
        calibration(rig, Some(weighed)),
        CalibrationResult::Current { pending: true, .. }
    ));
    assert_eq!(calibration(rig, Some(weighed)), CalibrationResult::Busy);
    rig.write();
    let CalibrationResult::Current {
        view: after,
        pending: false,
    } = calibration(rig, None)
    else {
        panic!("the calibration after the writer's turn");
    };
    assert_eq!(after.calibration, weighed);
    assert_eq!(after.generation, before.generation + 1);

    let portions = device(rig, 0, |device| {
        match device.stats(StatsSection::Portions, 0) {
            Stats::Portions(portions) => portions,
            other => panic!("the portions section: {other:?}"),
        }
    });
    assert_eq!(portions.portion_mg, Some(9_800));

    rig.claim_outputs(true);
    assert_eq!(
        calibration(rig, Some(Calibration::default())),
        CalibrationResult::Busy
    );
    rig.claim_outputs(false);
}

/// A recorded event reads back from the ring, after the boot's, under the
/// ring's epoch.
pub fn a_recorded_event_reads_back(rig: &mut impl Rig) {
    assert!(
        device(rig, 0, |device| device.epoch()).is_some(),
        "the ring is open"
    );
    let body = EventBody::Lock {
        locked: false,
        by: Source::Panel,
    };
    device(rig, 0, |device| device.record(0, body));

    let mut after = None;
    let mut found = None;
    for _ in 0..TURNS {
        match device(rig, 0, |device| device.event_after(after)) {
            EventRead::Event(Event {
                seq, body: read, ..
            }) if read == body => {
                found = Some(seq);
                break;
            }
            EventRead::Event(Event { seq, .. }) => after = Some(seq),
            EventRead::Wait | EventRead::None => rig.write(),
            EventRead::Gap { from, to } => panic!("events {from}..={to} lost"),
        }
    }
    let seq = found.expect("the event reads back");
    let status = device(rig, 0, |device| device.status(0));
    assert_eq!(status.last_event, Some(seq));
}

/// Status names the build the welcome does, and the time it was asked at.
pub fn status_reports_the_build_and_the_time(rig: &mut impl Rig) {
    let (build, status) = device(rig, 1234, |device| (device.build(), device.status(1234)));
    assert_eq!(status.build, build);
    assert!(!build.is_empty());
    assert_eq!(status.uptime_ms, 1234);
    assert_eq!(status.sessions, 0, "the server counts its own sessions");
}

/// Without a feeding owner a feed is refused before it reaches the motor,
/// whatever the platform claims.
pub fn a_feeder_without_a_feeding_owner_refuses_feeds(rig: &mut impl Rig) {
    let (capabilities, readiness, fed) = device(rig, 0, |device| {
        (
            device.capabilities(),
            device.readiness(),
            device.feed(1, Source::Feeder),
        )
    });
    assert!(capabilities.contains(Capabilities::UPDATE));
    assert!(capabilities.contains(Capabilities::TIME));
    assert!(capabilities.contains(Capabilities::READINESS));
    assert_eq!(readiness, Readiness::Blocked(Blocked::Unserved));
    assert_eq!(fed, Err(Refusal::Unavailable));
    assert_eq!(
        device(rig, 0, |device| device.feed_reserved(1)),
        Some(Err(Refusal::Unavailable))
    );
}

/// Every platform serves the statistics: each section answers as itself,
/// the counters follow what the loop saw (a feed's result, the boot's
/// uptime), the ring's fill counts the boot event, and the lifetime figures
/// of a new stats store are this boot's alone, its power-on counted.
pub fn the_statistics_follow_the_loop(rig: &mut impl Rig) {
    pass(rig, 0);
    rig.write();
    let fed = EventBody::Fed {
        feed: 1,
        requested: 3,
        completed: 2,
        outcome: FeedOutcome::Failed,
        reason: FeedReason::NoFeedback,
        by: Source::Feeder,
    };
    rig.feeder().record_outcome(0, fed);
    rig.feeder().pass_took(900);
    rig.write();

    assert!(device(rig, 0, |device| device.capabilities()).contains(Capabilities::STATS));
    for section in StatsSection::ALL {
        let answered = device(rig, 61_000, |device| match device.stats(section, 61_000) {
            Stats::Counters(counters) => {
                assert_eq!(counters.boot.uptime_s, 61);
                assert_eq!(
                    (
                        counters.boot.feeds,
                        counters.boot.feeds_failed,
                        counters.boot.portions
                    ),
                    (1, 1, 2)
                );
                assert_eq!(counters.lifetime, Some(counters.boot), "a new store");
                assert_eq!(
                    counters.resets,
                    Some(Resets {
                        power_on: 1,
                        ..Resets::default()
                    })
                );
                StatsSection::Counters
            }
            Stats::Memory(_) => StatsSection::Memory,
            Stats::Flash(flash) => {
                assert!(flash.events_kept >= 1, "the boot event: {flash:?}");
                assert!(flash.events_capacity >= flash.events_kept, "{flash:?}");
                assert_eq!(flash.lifetime, Some(flash.boot), "a new store");
                StatsSection::Flash
            }
            Stats::Power(_) => StatsSection::Power,
            Stats::Food(_) => StatsSection::Food,
            Stats::Feeds(feeds) => {
                assert_eq!((feeds.traced, feeds.last), (0, None));
                StatsSection::Feeds
            }
            Stats::Portions(portions) => {
                assert_eq!(portions.portions, 2, "the credited portions");
                assert_eq!(
                    portions.delivered_mg,
                    portions.portion_mg.map(|mg| u64::from(mg) * 2)
                );
                StatsSection::Portions
            }
        });
        assert_eq!(answered, section);
    }
}

/// Every platform serves the stock backup's reads, and only its bank-0
/// role answers them: running from bank 1, whose image is the running
/// build, it refuses every page.
pub fn bank_1_is_read_only_from_bank_0(rig: &mut impl Rig) {
    let (capabilities, bank, page) = device(rig, 0, |device| {
        let bank = device.status(0).bank;
        let page = device.bank1_page(0).map(|page| page.offset);
        (device.capabilities(), bank, page)
    });
    assert!(capabilities.contains(Capabilities::BACKUP));
    assert_eq!(bank, 1, "the suite's rigs run from bank 1");
    assert_eq!(page, Err(Refusal::Unavailable));
}

/// Every platform serves the traces beside the statistics: a feed it keeps
/// no trace of is `NotFound`, the storage ring keeps at least eight, and a
/// sealed trace's figures join the feeds section.
pub fn traces_are_served_beside_the_statistics(rig: &mut impl Rig) {
    pass(rig, 0);
    rig.write();
    let both = Capabilities::STATS.union(Capabilities::TRACES);
    assert!(device(rig, 0, |device| device.capabilities()).contains(both));
    let missing = device(rig, 0, |device| {
        device.feed_trace(77, 0).map(|page| page.total)
    });
    assert_eq!(missing, Err(Refusal::NotFound));

    let summary = TraceSummary {
        feed: 77,
        requested: 1,
        completed: 1,
        outcome: FeedOutcome::Done,
        reason: FeedReason::Done,
        drive_ms: 9_072,
        samples: 300,
        unit: CurrentUnit::Scaled,
        current_peak: 164,
        current_mean: Some(60),
        rising: 1,
        falling: 1,
        first_edge_ms: Some(7_888),
        beam_breaks: 2,
        reversals: 0,
        battery_rest_mv: Some(4_810),
        battery_min_mv: Some(4_700),
        stride: 1,
        truncated: false,
    };
    rig.feeder().traced(&summary);
    let feeds = device(rig, 0, |device| {
        match device.stats(StatsSection::Feeds, 0) {
            Stats::Feeds(feeds) => feeds,
            other => panic!("{other:?}"),
        }
    });
    assert_eq!(
        (
            feeds.traced,
            feeds.drive_ms,
            feeds.current_peak,
            feeds.current_mean
        ),
        (1, 9_072, 164, Some(60))
    );
    assert_eq!((feeds.rising, feeds.falling, feeds.beam_breaks), (1, 1, 2));
    assert_eq!((feeds.capacity, feeds.last), (8, Some(summary)));
}

/// An owner that stopped on a fault and serves no feed.
struct Faulted;

impl Feeds for Faulted {
    fn feeding(&self) -> bool {
        false
    }

    fn busy(&self) -> bool {
        false
    }

    fn blocked(&self) -> Option<Blocked> {
        Some(Blocked::Fault)
    }

    fn submit(&mut self, _: u8, _: Source) -> Result<u32, Refusal> {
        panic!("a feed reached a faulted owner past readiness")
    }

    fn reserved(&self, _: u32) -> Option<Result<(), Refusal>> {
        None
    }

    fn cancel(&self, _: u32) -> bool {
        false
    }
}

/// A lasting reason not to feed is recorded in the history once, however
/// many passes it holds, and a feed asked for meanwhile is refused with it.
pub fn a_lasting_reason_not_to_feed_is_recorded_once(rig: &mut impl Rig) {
    for now_ms in [0, 20, 40] {
        rig.feeder().pass(now_ms, &mut Faulted, |_| {});
    }
    let fed = {
        let mut feeds = Faulted;
        rig.feeder().device(60, &mut feeds).feed(1, Source::Panel)
    };
    assert_eq!(fed, Err(Refusal::Fault));
    rig.write();

    let blocked = EventBody::FeedingBlocked {
        reason: Some(Blocked::Fault),
    };
    let mut after = None;
    let mut seen = 0;
    for _ in 0..TURNS {
        match device(rig, 0, |device| device.event_after(after)) {
            EventRead::Event(Event { seq, body, .. }) => {
                seen += usize::from(body == blocked);
                after = Some(seq);
            }
            EventRead::Wait => rig.write(),
            EventRead::None => break,
            EventRead::Gap { from, to } => panic!("events {from}..={to} lost"),
        }
    }
    assert_eq!(seen, 1);
}

/// One test per check, each on a fresh rig from `$rig`.
#[macro_export]
macro_rules! conformance {
    ($rig:expr) => {
        $crate::conformance!(@tests $rig;
            a_settings_change_lands_on_the_writer_turn,
            wifi_credentials_are_saved_and_cleared,
            client_changes_answer_by_ticket,
            writes_are_refused_while_the_outputs_are_claimed,
            the_lock_follows_the_panel_owner,
            a_time_sync_sets_the_clock,
            time_settings_land_and_set_the_zone,
            a_calibration_lands_and_weighs_the_portions,
            a_recorded_event_reads_back,
            status_reports_the_build_and_the_time,
            a_feeder_without_a_feeding_owner_refuses_feeds,
            the_statistics_follow_the_loop,
            traces_are_served_beside_the_statistics,
            a_lasting_reason_not_to_feed_is_recorded_once,
            bank_1_is_read_only_from_bank_0,
        );
    };
    (@tests $rig:expr; $($check:ident),* $(,)?) => {
        $(
            #[test]
            fn $check() {
                let mut rig = $rig;
                $crate::conformance::$check(&mut rig);
            }
        )*
    };
}
