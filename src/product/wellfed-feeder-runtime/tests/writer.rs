//! Settings writer against the fake SDK storage, with the hooks already
//! armed (arming itself is covered by the confirmation and updater tests).

use std::cell::Cell;
use std::sync::{Mutex, MutexGuard};

use wellfed_core::record::{self, Event, Record as Line, Tag};
use wellfed_core::store::handover;
use wellfed_core::store::seed::SeedStatus;
use wellfed_device_api::Reader;
use wellfed_device_api::clients::{Label, Role};
use wellfed_device_api::mailbox::{SettingsFields, SettingsResult};
use wellfed_feeder::stats::{EARLY_SAVES, RESTART_SETTLED_MS, SAVE_MS, Totals};
use wellfed_feeder::store::clients::ClientStatus;
use wellfed_feeder::store::events::Read;
use wellfed_feeder::store::net::NetStatus;
use wellfed_feeder::store::settings::{Settings, SettingsStatus};
use wellfed_feeder::store::stats::StatsStatus;
use wellfed_feeder::store::time::TimeStatus;
use wellfed_feeder_runtime::keys::{self, Change, Requested};
use wellfed_feeder_runtime::writer::{
    self as writer, ErrorStage, Identity, KeyChange, KeyResult, Record, Writer,
};
use wellfed_feeder_runtime::{events, network, stats};
use wellfed_hal::flash::park::Mmio;
use wellfed_hal::flash::peer::MARKER_REGISTER;
use wellfed_hal::flash_lock::arming::Hooks;
use wellfed_hal::flash_lock::provider::Provider;
use wellfed_hal::interlock::INTERLOCK;
use wellfed_hal::sdk::{self, fake};
use wellfed_hal::{rng, settings};
use wellfed_hal::{stopped, supervision};
use wellfed_proto::api::{Counts, Resets};
use wellfed_sdk::storage::{FlashOps, SETTINGS_BASE};

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(true) };
    static ATTEMPT_SEEN: Cell<bool> = const { Cell::new(false) };
    /// Programs fail, as a write cut short reports.
    static PROGRAM_FAILS: Cell<bool> = const { Cell::new(false) };
    /// Erases of this sector fail.
    static ERASE_FAILS: Cell<Option<u32>> = const { Cell::new(None) };
    /// Programs of the statistics' sectors fail, as a worn-out pair's do.
    static STATS_PROGRAMS_FAIL: Cell<bool> = const { Cell::new(false) };
}

struct NoMmio;

impl Mmio for NoMmio {
    const KM0: bool = false;
    fn load(_: u32) -> u32 {
        unreachable!("no arming with the hooks armed")
    }
    fn store(_: u32, _: u32) {
        unreachable!("no arming with the hooks armed")
    }
    fn barrier() {}
    fn mask() -> u32 {
        0
    }
    fn halt() -> ! {
        panic!("halt")
    }
}

struct Armed;

impl Hooks for Armed {
    fn watchdog() -> bool {
        true
    }
    fn install(_: *mut Provider) -> bool {
        true
    }
    fn armed() -> bool {
        ARMED.with(Cell::get)
    }
}

impl FlashOps for Armed {
    fn install(_: *mut Provider) -> bool {
        true
    }
    fn read_id() -> [u8; 3] {
        [0; 3]
    }
    fn armed() -> bool {
        ARMED.with(Cell::get)
    }
    fn erase(sector: u32) -> bool {
        // Every write runs inside a marked attempt.
        ATTEMPT_SEEN.with(|s| s.set(sdk::backup_read(MARKER_REGISTER) != 0));
        if ERASE_FAILS.with(Cell::get) == Some(sector) {
            return false;
        }
        // SAFETY: the fake's storage, which this test owns.
        unsafe { sdk::storage_erase(sector) }
    }
    fn program(sector: u32, offset: u32, data: &[u8]) -> bool {
        let stats = wellfed_hal::plaf103::storage::STATS;
        let worn = STATS_PROGRAMS_FAIL.with(Cell::get)
            && (stats.first..stats.first + stats.count).contains(&sector);
        if PROGRAM_FAILS.with(Cell::get) || worn {
            return false;
        }
        // SAFETY: as in `erase`.
        unsafe { sdk::storage_program(sector, offset, data) }
    }
    fn read(sector: u32, offset: u32, bytes: &mut [u8]) -> bool {
        sdk::storage_read(sector, offset, bytes)
    }
}

/// The settings and network views are process-wide; the fake hardware is per thread.
static SERIAL: Mutex<()> = Mutex::new(());

/// The subscriber most tests read for.
const A: Reader = Reader::slot(0);
/// A second subscriber, in another slot.
const B: Reader = Reader::slot(1);

fn start() -> (MutexGuard<'static, ()>, Writer<Armed>, Provider) {
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    fake::reset_hardware();
    settings::reset();
    // SAFETY: the test holds `SERIAL` and runs no Wi-Fi task.
    unsafe { network::reset() };
    keys::reset();
    events::reset();
    stats::reset();
    INTERLOCK.reset();
    supervision::reset();
    ARMED.with(|a| a.set(true));
    PROGRAM_FAILS.with(|f| f.set(false));
    ERASE_FAILS.with(|f| f.set(None));
    STATS_PROGRAMS_FAIL.with(|f| f.set(false));
    (guard, Writer::default(), Provider::default())
}

fn step(writer: &mut Writer<Armed>, provider: &mut Provider) -> Option<Record> {
    // SAFETY: the provider outlives the run.
    unsafe { writer.step::<NoMmio, Armed>(provider) }.record
}

fn current() -> (bool, bool, u8, u32, bool) {
    match settings::request(SettingsFields::default()) {
        SettingsResult::Current { view, .. } => (
            view.lights,
            view.sound,
            view.volume,
            view.generation,
            view.writable,
        ),
        other => panic!("{other:?}"),
    }
}

#[test]
fn changes_are_saved_once_each_inside_a_marked_attempt() {
    let (_guard, mut writer, mut provider) = start();
    assert_eq!(
        settings::request(SettingsFields::default()),
        SettingsResult::Busy,
        "nothing published before the store opens"
    );

    let Some(Record::Open {
        status, generation, ..
    }) = step(&mut writer, &mut provider)
    else {
        panic!("open first")
    };
    assert_eq!((status, generation), (SettingsStatus::Empty, 0));
    assert_eq!(current(), (true, true, 50, 0, true));
    assert!(matches!(
        step(&mut writer, &mut provider),
        Some(Record::NetOpen { stored: false, .. })
    ));
    assert_eq!(
        step(&mut writer, &mut provider),
        Some(Record::TimeOpen {
            status: TimeStatus::Empty,
            generation: 0
        })
    );
    assert!(matches!(
        step(&mut writer, &mut provider),
        Some(Record::Keys { .. })
    ));
    assert_eq!(
        step(&mut writer, &mut provider),
        Some(Record::Events {
            open: true,
            last: None,
            oldest: None,
            epoch: events::epoch().unwrap(),
        }),
        "the event ring opens empty"
    );
    assert_eq!(
        step(&mut writer, &mut provider),
        Some(Record::StatsOpen {
            status: StatsStatus::Empty,
            generation: 0,
            early: 0,
            missed: false,
        })
    );
    assert_eq!(
        step(&mut writer, &mut provider),
        Some(Record::StatsSaved {
            generation: 1,
            early: 1
        }),
        "the boot's own save, so its reset counts"
    );
    assert_eq!(
        step(&mut writer, &mut provider),
        None,
        "idle without a change"
    );

    let change = SettingsFields {
        lights: Some(false),
        volume: Some(20),
        ..Default::default()
    };
    assert!(matches!(
        settings::request(change),
        SettingsResult::Current { pending: true, .. }
    ));
    assert_eq!(
        settings::request(change),
        SettingsResult::Busy,
        "one change at a time"
    );

    let Some(Record::Saved {
        generation,
        settings: saved,
    }) = step(&mut writer, &mut provider)
    else {
        panic!("saved")
    };
    assert_eq!(
        (generation, saved.lights, saved.sound, saved.volume),
        (1, false, true, 20)
    );
    assert!(ATTEMPT_SEEN.with(Cell::get));
    assert_eq!(
        sdk::backup_read(MARKER_REGISTER),
        0,
        "the attempt is cleared after"
    );
    assert_eq!(current(), (false, true, 20, 1, true));
    assert_eq!(settings::pending(), None);

    let base = SETTINGS_BASE as usize * 4096;
    assert_eq!(
        fake::with(|h| h.storage[base..base + 8].to_vec()),
        b"PLFSET01"
    );

    // The next change goes to the other slot; a reopen sees it.
    settings::request(SettingsFields {
        sound: Some(false),
        ..Default::default()
    });
    assert!(matches!(
        step(&mut writer, &mut provider),
        Some(Record::Saved { generation: 2, .. })
    ));
    let mut reopened = Writer::default();
    settings::reset();
    assert!(matches!(
        step(&mut reopened, &mut provider),
        Some(Record::Open { generation: 2, .. })
    ));
    assert_eq!(current(), (false, false, 20, 2, true));
}

#[test]
fn an_unwritable_store_reads_but_refuses_changes() {
    let (_guard, mut writer, mut provider) = start();
    // Garbage in both slots: CORRUPT, defaults, read-only until a reset.
    let base = SETTINGS_BASE as usize * 4096;
    fake::with(|h| h.storage[base..base + 2 * 4096].fill(0x5a));
    let Some(Record::Open { status, .. }) = step(&mut writer, &mut provider) else {
        panic!("open")
    };
    assert_eq!(status, SettingsStatus::Corrupt);
    assert!(!current().4);
    assert_eq!(
        settings::request(SettingsFields {
            lights: Some(false),
            ..Default::default()
        }),
        SettingsResult::ReadOnly
    );
    assert_eq!(fake::with(|h| h.storage[base]), 0x5a, "nothing written");
}

#[test]
fn wifi_credentials_are_saved_inside_a_marked_attempt_and_cleared() {
    use wellfed_device_api::mailbox::WifiResult;
    use wellfed_device_api::wifi::Credentials;
    let (_guard, mut writer, mut provider) = start();
    step(&mut writer, &mut provider); // open
    step(&mut writer, &mut provider); // net open
    step(&mut writer, &mut provider); // time open
    step(&mut writer, &mut provider); // keys
    step(&mut writer, &mut provider); // events
    step(&mut writer, &mut provider); // stats
    step(&mut writer, &mut provider); // the boot's stats save

    let set = Credentials::new(b"net", b"password").unwrap();
    assert!(matches!(
        network::change(Some(set)),
        WifiResult::Current { pending: true, .. }
    ));
    assert_eq!(
        step(&mut writer, &mut provider),
        Some(Record::NetSaved {
            generation: 1,
            stored: true
        })
    );
    assert!(ATTEMPT_SEEN.with(Cell::get));
    assert_eq!(sdk::backup_read(MARKER_REGISTER), 0);
    assert!(matches!(
        network::current(),
        WifiResult::Current { view, pending: false } if view.stored_ssid_crc.is_some()
    ));

    network::change(None);
    assert_eq!(
        step(&mut writer, &mut provider),
        Some(Record::NetSaved {
            generation: 2,
            stored: false
        })
    );
}

/// The `PLCFG1` error words, as they were when the stage was a string.
#[test]
fn error_records_format_as_before() {
    let words: Vec<_> = ErrorStage::ALL.iter().map(|s| s.as_str()).collect();
    assert_eq!(words, ["unarmed", "save", "erase"]);

    let mut text = String::new();
    writer::describe(
        Record::Error {
            stage: ErrorStage::Save,
            status: SettingsStatus::Corrupt,
        },
        &mut text,
    )
    .unwrap();
    assert_eq!(
        text,
        format!("error stage=save status={}", SettingsStatus::Corrupt.code())
    );

    text.clear();
    writer::describe(
        Record::NetError {
            stage: ErrorStage::Unarmed,
        },
        &mut text,
    )
    .unwrap();
    assert_eq!(text, "net error stage=unarmed");
}

/// The writer's first records on one boot: open, net open, time open, then
/// keys.
fn boot_keys() -> Record {
    let (mut writer, mut provider) = (Writer::default(), Provider::default());
    step(&mut writer, &mut provider);
    step(&mut writer, &mut provider);
    step(&mut writer, &mut provider);
    step(&mut writer, &mut provider).expect("the keys record")
}

#[test]
fn keys_carry_the_seed_and_create_the_identity_once() {
    let (_guard, _, _) = start();
    rng::reset();
    assert_eq!(
        boot_keys(),
        Record::Keys {
            seed: SeedStatus::Empty,
            mixed: false,
            saved: false,
            clients: ClientStatus::Empty,
            enrolled: 0,
            identity: Identity::Missing,
        },
        "no generator: nothing written"
    );

    fake::with(|h| h.adc_noise = 0x2468_ace1);
    assert_eq!(rng::start().result, Ok(()));
    let first = boot_keys();
    assert!(matches!(
        first,
        Record::Keys {
            seed: SeedStatus::Empty,
            mixed: false,
            saved: true,
            identity: Identity::Created,
            ..
        }
    ));
    assert!(ATTEMPT_SEEN.with(Cell::get));
    assert_eq!(sdk::backup_read(MARKER_REGISTER), 0);

    // The next boot finds both, and stores another seed.
    assert!(matches!(
        boot_keys(),
        Record::Keys {
            seed: SeedStatus::Loaded,
            mixed: true,
            saved: true,
            clients: ClientStatus::Loaded,
            identity: Identity::Loaded,
            ..
        }
    ));
}

/// One queued key change, applied on the writer's next step.
fn change(writer: &mut Writer<Armed>, provider: &mut Provider, change: Change) -> Record {
    assert_eq!(keys::request(change), Requested::Queued);
    let record = step(writer, provider).expect("the change record");
    assert_eq!(keys::pending(), None, "the slot is free again");
    record
}

fn changed(change: KeyChange, result: KeyResult, enrolled: usize) -> Record {
    Record::KeysChanged {
        change,
        result,
        enrolled,
    }
}

#[test]
fn key_changes_apply_and_a_reset_publishes_a_new_identity() {
    let (_guard, mut writer, mut provider) = start();
    fake::with(|h| h.adc_noise = 0x1357_9bdf);
    assert_eq!(rng::start().result, Ok(()));
    for _ in 0..7 {
        step(&mut writer, &mut provider); // open, net open, time open, keys, events, stats, its save
    }
    let (first, identity) = keys::identity_since(0).expect("the identity is published");

    let phone = Change::Enroll {
        key: [7; 32],
        label: Label::new(b"phone").unwrap(),
        role: Role::Owner,
    };
    assert_eq!(
        change(&mut writer, &mut provider, phone),
        changed(KeyChange::Enroll, KeyResult::Done, 1)
    );
    let enrolled = keys::ticket();
    assert_eq!(keys::outcome(enrolled), Some(KeyResult::Done));
    assert!(ATTEMPT_SEEN.with(Cell::get));
    assert_eq!(sdk::backup_read(MARKER_REGISTER), 0);
    assert_eq!(
        change(&mut writer, &mut provider, phone),
        changed(KeyChange::Enroll, KeyResult::Duplicate, 1)
    );
    // A caller holding the earlier ticket no longer sees an outcome.
    assert_eq!(keys::outcome(enrolled), None);
    assert_eq!(keys::outcome(keys::ticket()), Some(KeyResult::Duplicate));
    assert_eq!(
        keys::identity_since(first),
        None,
        "an enrollment keeps the identity"
    );

    let revoke = Change::Revoke { key: [8; 32] };
    assert_eq!(
        change(&mut writer, &mut provider, revoke),
        changed(KeyChange::Revoke, KeyResult::NotFound, 1)
    );

    assert_eq!(
        change(&mut writer, &mut provider, Change::Reset),
        changed(KeyChange::Reset, KeyResult::Done, 0)
    );
    let (_, new) = keys::identity_since(first).expect("the new identity");
    assert_ne!(new, identity);
    assert!(reset_marked(), "the next boot finishes the factory reset");
}

#[test]
fn a_reset_needs_the_generator_and_an_armed_writer() {
    let (_guard, mut writer, mut provider) = start();
    fake::with(|h| h.adc_noise = 0x0bad_cafe);
    assert_eq!(rng::start().result, Ok(()));
    for _ in 0..7 {
        step(&mut writer, &mut provider);
    }
    let (first, _) = keys::identity_since(0).unwrap();

    rng::reset();
    assert_eq!(
        change(&mut writer, &mut provider, Change::Reset),
        changed(KeyChange::Reset, KeyResult::NoGenerator, 0)
    );

    assert_eq!(rng::start().result, Ok(()));
    ARMED.with(|a| a.set(false));
    assert_eq!(
        change(&mut writer, &mut provider, Change::Reset),
        changed(KeyChange::Reset, KeyResult::Unarmed, 0)
    );
    assert_eq!(keys::identity_since(first), None, "the identity stands");
}

#[test]
fn key_change_records_format() {
    let mut text = String::new();
    writer::describe(
        changed(KeyChange::Reset, KeyResult::NoGenerator, 0),
        &mut text,
    )
    .unwrap();
    assert_eq!(text, "keys change=reset result=no-generator enrolled=0");
}

/// The loop's queued events reach the ring, survive a reboot, and
/// come back through the read mailbox in order.
#[test]
fn queued_events_are_appended_and_read_back_after_a_reboot() {
    let (_guard, mut writer, mut provider) = start();
    for _ in 0..7 {
        step(&mut writer, &mut provider); // open, net open, time open, keys, events, stats, its save
    }
    assert!(events::record(Some(1_700_000_000), [1; 16]));
    assert!(events::record(None, [2; 16]));
    step(&mut writer, &mut provider);
    step(&mut writer, &mut provider);
    assert_eq!(events::last_seq(), Some(2));

    // The next boot finds both; a reader asks, the writer answers.
    events::reset();
    let mut writer = Writer::default();
    for _ in 0..4 {
        step(&mut writer, &mut provider);
    }
    assert_eq!(
        step(&mut writer, &mut provider),
        Some(Record::Events {
            open: true,
            last: Some(2),
            oldest: Some(1),
            epoch: events::epoch().unwrap(),
        })
    );
    step(&mut writer, &mut provider); // stats
    step(&mut writer, &mut provider); // the boot's stats save
    assert_eq!(events::read_result(A, None), None, "asked");
    step(&mut writer, &mut provider);
    let Some(Read::Record(first)) = events::read_result(A, None) else {
        panic!("the first event");
    };
    assert_eq!(
        (first.seq, first.at, first.body),
        (1, Some(1_700_000_000), [1; 16])
    );
    // One answer is a batch: the next record needs no other read.
    assert!(matches!(
        events::read_result(A, Some(1)),
        Some(Read::Record(r)) if r.seq == 2
    ));

    // At the newest record there is nothing to read, until the next append.
    assert_eq!(events::read_result(A, Some(2)), Some(Read::End));
    assert!(events::record(None, [3; 16]));
    step(&mut writer, &mut provider);
    assert_eq!(
        events::read_result(A, Some(2)),
        None,
        "stale once a record lands"
    );
    step(&mut writer, &mut provider);
    assert!(matches!(
        events::read_result(A, Some(2)),
        Some(Read::Record(r)) if r.seq == 3
    ));

    // A subscriber caught up does not take the slot from one catching up:
    // the answer for the first waits however often the second polls.
    assert_eq!(events::read_result(A, Some(3)), Some(Read::End));
    assert_eq!(events::read_result(B, None), None, "asked");
    step(&mut writer, &mut provider);
    for _ in 0..3 {
        assert_eq!(events::read_result(A, Some(3)), Some(Read::End));
    }
    assert!(matches!(
        events::read_result(B, None),
        Some(Read::Record(r)) if r.seq == 1
    ));
}

/// A reader catching up gets a batch a read: it walks through it without
/// asking again, a second reader inside it shares it, and the writer keeps
/// the read pace between batches rather than falling back to its idle poll.
#[test]
fn a_reader_catching_up_takes_a_batch_a_read() {
    let (_guard, mut writer, mut provider) = start();
    for _ in 0..7 {
        step(&mut writer, &mut provider); // open, net open, time open, keys, events, stats, its save
    }
    let total = events::READ_BATCH + 4;
    for n in 0..total {
        assert!(events::record(None, [u8::try_from(n).unwrap(); 16]));
        step(&mut writer, &mut provider);
    }
    assert_eq!(events::last_seq(), Some(total as u64));

    assert_eq!(events::read_result(A, None), None, "asked");
    // SAFETY: the provider outlives the run.
    let answered = unsafe { writer.step::<NoMmio, Armed>(&mut provider) };
    assert_eq!(
        answered.delay_ms,
        writer::EVENTS_POLL_MS,
        "a reader is about"
    );

    // The whole batch, one record a call, with no read between.
    let mut after = None;
    for seq in 1..=events::READ_BATCH as u64 {
        let Some(Read::Record(record)) = events::read_result(A, after) else {
            panic!("record {seq} from the batch");
        };
        assert_eq!(record.seq, seq);
        after = Some(seq);
    }
    // A second reader inside the batch shares it.
    assert!(matches!(
        events::read_result(B, Some(3)),
        Some(Read::Record(r)) if r.seq == 4
    ));

    // Past the batch the reader asks again; the writer, still at the read
    // pace, answers on its next poll.
    // SAFETY: as above.
    let idle = unsafe { writer.step::<NoMmio, Armed>(&mut provider) };
    assert_eq!(
        idle.delay_ms,
        writer::EVENTS_POLL_MS,
        "lingers after a read"
    );
    assert_eq!(events::read_result(A, after), None, "asked");
    step(&mut writer, &mut provider);
    for seq in events::READ_BATCH as u64 + 1..=total as u64 {
        let Some(Read::Record(record)) = events::read_result(A, after) else {
            panic!("record {seq} from the next batch");
        };
        assert_eq!(record.seq, seq);
        after = Some(seq);
    }
    assert_eq!(events::read_result(A, after), Some(Read::End));

    // With no reader the writer goes back to its idle poll.
    let delay = (0..=writer::READ_LINGER_POLLS)
        // SAFETY: as above.
        .map(|_| unsafe { writer.step::<NoMmio, Armed>(&mut provider) }.delay_ms)
        .last();
    assert_eq!(delay, Some(writer::POLL_MS));
}

/// One loop pass's reads for the subscriber in `reader`'s slot at `after`:
/// everything the slot answers it now; true once it is at the newest
/// record.
fn drain(reader: usize, after: &mut Option<u64>, got: &mut Vec<u64>) -> bool {
    loop {
        match events::read_result(Reader::slot(reader), *after) {
            Some(Read::Record(record)) => {
                got.push(record.seq);
                *after = Some(record.seq);
            }
            Some(Read::Gap { to, .. }) => *after = Some(to),
            Some(Read::End) => return true,
            None => return false,
        }
    }
}

/// A writer with `total` records appended, ready for reads.
fn with_records(total: u8) -> (MutexGuard<'static, ()>, Writer<Armed>, Provider) {
    let (guard, mut writer, mut provider) = start();
    for _ in 0..7 {
        step(&mut writer, &mut provider); // open, net open, time open, keys, events, stats, its save
    }
    for n in 0..total {
        assert!(events::record(None, [n; 16]));
        step(&mut writer, &mut provider);
    }
    assert_eq!(events::last_seq(), Some(u64::from(total)));
    (guard, writer, provider)
}

/// Subscribers in slots 0.. at `start`, visited in `order` each pass with
/// a writer round trip between passes, until each has every record after
/// its cursor: the most passes in a row each went without a record.
fn catch_up(total: u8, start: &[Option<u64>], order: &[usize]) -> Vec<usize> {
    let (_guard, mut writer, mut provider) = with_records(total);
    let n = start.len();
    let mut cursors = start.to_vec();
    let mut got = vec![Vec::new(); n];
    let mut done = vec![false; n];
    let mut idle = vec![0; n];
    let mut worst = vec![0; n];

    let mut passes = 0;
    while done.contains(&false) {
        for &reader in order {
            let before = got[reader].len();
            done[reader] = drain(reader, &mut cursors[reader], &mut got[reader]);
            idle[reader] = if done[reader] || got[reader].len() > before {
                0
            } else {
                idle[reader] + 1
            };
            worst[reader] = worst[reader].max(idle[reader]);
        }
        step(&mut writer, &mut provider);
        passes += 1;
        assert!(passes < 200, "no progress: {start:?} in {order:?}");
    }

    for (reader, cursor) in start.iter().enumerate() {
        let from = cursor.map_or(1, |c| c + 1);
        assert_eq!(got[reader], (from..=u64::from(total)).collect::<Vec<_>>());
    }
    worst
}

/// Each subscriber goes at most one pass more than there are subscribers
/// without a record: a round of the others' batches, the pass that fetches
/// its own, and one more when its turn came round just before it asked.
fn assert_turns(total: u8, start: &[Option<u64>]) {
    let forward: Vec<usize> = (0..start.len()).collect();
    let backward: Vec<usize> = forward.iter().rev().copied().collect();
    for order in [forward, backward] {
        let worst = catch_up(total, start, &order);
        let bound = start.len() + 1;
        assert!(
            worst.iter().all(|&w| w <= bound),
            "{start:?} in {order:?}: idle {worst:?}, bound {bound}"
        );
    }
}

/// Two subscribers catching up from different units take turns, whichever
/// the loop visits first.
#[test]
fn two_readers_in_different_units_take_turns() {
    assert_turns(200, &[None, Some(150)]);
}

/// Two subscribers inside one unit, near each other or a batch apart,
/// share what they can and take turns for the rest, across the unit's
/// end, in either order.
#[test]
fn overlapping_readers_in_one_unit_take_turns() {
    for second in [5, 10, 20, 31, 40] {
        assert_turns(255, &[None, Some(second)]);
    }
    assert_turns(255, &[Some(100), Some(120)]);
    assert_turns(120, &[Some(10), None]);
}

/// Three and four subscribers at spread cursors each get a turn a round.
#[test]
fn three_and_four_readers_each_get_a_turn() {
    // Orders a random search found at the bound.
    for (start, order) in [
        (
            vec![Some(250), None, Some(127), Some(128)],
            vec![2, 1, 3, 0],
        ),
        (vec![Some(93), Some(229), Some(42)], vec![0, 2, 1]),
    ] {
        let worst = catch_up(255, &start, &order);
        assert!(
            worst.iter().all(|&w| w <= start.len() + 1),
            "{start:?} in {order:?}: idle {worst:?}"
        );
    }

    assert_turns(200, &[None, Some(150), Some(60)]);
    assert_turns(200, &[None, Some(150), Some(60), Some(100)]);
    assert_turns(255, &[Some(200), Some(10), Some(130), None]);
}

/// Once caught up, each new record reaches every subscriber within a few
/// passes.
#[test]
fn caught_up_readers_each_get_a_new_record() {
    let (_guard, mut writer, mut provider) = with_records(120);
    let mut cursors = [None, Some(10), Some(60)];
    let mut got = [Vec::new(), Vec::new(), Vec::new()];
    for _ in 0..100 {
        for reader in 0..3 {
            drain(reader, &mut cursors[reader], &mut got[reader]);
        }
        step(&mut writer, &mut provider);
    }
    assert_eq!(cursors, [Some(120); 3]);

    for round in 0..3 {
        assert!(events::record(None, [round; 16]));
        step(&mut writer, &mut provider);
        let want = Some(121 + u64::from(round));
        let mut passes = 0;
        while cursors != [want; 3] {
            for reader in 0..3 {
                drain(reader, &mut cursors[reader], &mut got[reader]);
            }
            step(&mut writer, &mut provider);
            passes += 1;
            assert!(passes <= 4, "round {round}: {cursors:?}");
        }
    }
}

/// A batch fetched for a subscriber that ended is nobody's: the next ask
/// replaces it at once.
#[test]
fn a_departed_reader_s_batch_gives_way_at_once() {
    let (_guard, mut writer, mut provider) = with_records(200);
    assert_eq!(events::read_result(Reader::slot(0), None), None, "asked");
    step(&mut writer, &mut provider);

    events::reader_left(Reader::slot(0));
    assert_eq!(
        events::read_result(Reader::slot(1), Some(150)),
        None,
        "asked"
    );
    step(&mut writer, &mut provider);
    assert!(matches!(
        events::read_result(Reader::slot(1), Some(150)),
        Some(Read::Record(r)) if r.seq == 151
    ));
}

/// A batch whose subscriber stopped asking without ending (its last frame
/// still going out) holds the slot for a bounded number of other asks;
/// then another subscriber's read replaces it.
#[test]
fn a_stalled_reader_s_batch_gives_way_after_its_bound() {
    let (_guard, mut writer, mut provider) = with_records(200);
    assert_eq!(events::read_result(Reader::slot(0), None), None, "asked");
    step(&mut writer, &mut provider);

    for _ in 0..events::MISSES_MAX {
        assert_eq!(
            events::read_result(Reader::slot(1), Some(150)),
            None,
            "held for its subscriber"
        );
        step(&mut writer, &mut provider);
    }
    assert_eq!(
        events::read_result(Reader::slot(1), Some(150)),
        None,
        "asked"
    );
    step(&mut writer, &mut provider);
    assert!(matches!(
        events::read_result(Reader::slot(1), Some(150)),
        Some(Read::Record(r)) if r.seq == 151
    ));
}

/// Every store the writer keeps defers its write while a feed holds the
/// outputs, and completes it after the release, in order, with nothing lost.
#[test]
fn every_store_write_waits_for_a_feed_and_completes_after_it() {
    use wellfed_device_api::mailbox::WifiResult;
    use wellfed_device_api::wifi::Credentials;

    let (_guard, mut writer, mut provider) = start();
    fake::with(|h| h.adc_noise = 0x5a5a_1234);
    assert_eq!(rng::start().result, Ok(()));
    step(&mut writer, &mut provider); // open
    step(&mut writer, &mut provider); // net open
    step(&mut writer, &mut provider); // time open
    let storage = || fake::with(|h| h.storage.clone());

    // A feed holds the outputs before the first boot's seed and identity.
    INTERLOCK.claim_feed(u32::MAX).unwrap();
    let before = storage();
    for _ in 0..5 {
        assert_eq!(step(&mut writer, &mut provider), None);
    }
    assert_eq!(storage(), before, "no write while the outputs are claimed");
    assert_eq!(keys::identity_since(0), None);
    INTERLOCK.release_feed();
    assert!(matches!(
        step(&mut writer, &mut provider),
        Some(Record::Keys {
            saved: true,
            identity: Identity::Created,
            ..
        })
    ));
    step(&mut writer, &mut provider); // events
    step(&mut writer, &mut provider); // stats
    step(&mut writer, &mut provider); // the boot's stats save

    // A feed, and during it every kind of write queued.
    INTERLOCK.claim_feed(u32::MAX).unwrap();
    for body in [[1; 16], [2; 16], [3; 16]] {
        assert!(events::record(None, body));
    }
    assert!(matches!(
        settings::request(SettingsFields {
            volume: Some(7),
            ..SettingsFields::default()
        }),
        SettingsResult::Current { pending: true, .. }
    ));
    let set = Credentials::new(b"net", b"password").unwrap();
    assert!(matches!(
        network::change(Some(set)),
        WifiResult::Current { pending: true, .. }
    ));
    let phone = Change::Enroll {
        key: [7; 32],
        label: Label::new(b"phone").unwrap(),
        role: Role::Owner,
    };
    assert_eq!(keys::request(phone), Requested::Queued);

    let before = storage();
    for _ in 0..20 {
        assert_eq!(step(&mut writer, &mut provider), None);
    }
    assert_eq!(storage(), before, "no write while the outputs are claimed");
    assert_eq!(events::last_seq(), None);
    // Reads take no lease: answered during the feed.
    assert_eq!(events::read_result(A, None), None, "asked");
    step(&mut writer, &mut provider);
    assert_eq!(events::read_result(A, None), Some(Read::End));

    // At rest: each write in turn, the first right after the release.
    INTERLOCK.release_feed();
    assert_eq!(
        step(&mut writer, &mut provider),
        Some(changed(KeyChange::Enroll, KeyResult::Done, 1))
    );
    assert!(matches!(
        step(&mut writer, &mut provider),
        Some(Record::NetSaved { stored: true, .. })
    ));
    assert!(matches!(
        step(&mut writer, &mut provider),
        Some(Record::Saved { settings, .. }) if settings.volume == 7
    ));
    for _ in 0..3 {
        step(&mut writer, &mut provider);
    }
    assert_eq!(events::last_seq(), Some(3));
    assert_eq!(events::pending(), None);
    assert!(!INTERLOCK.writing(), "every turn given back");
    assert_eq!(INTERLOCK.status().writes_deferred, 2);

    // The appends kept their order.
    let mut after = None;
    for body in [[1; 16], [2; 16], [3; 16]] {
        events::read_result(A, after);
        step(&mut writer, &mut provider);
        let Some(Read::Record(record)) = events::read_result(A, after) else {
            panic!("event after {after:?}");
        };
        assert_eq!(record.body, body);
        after = Some(record.seq);
    }
}

/// Events pile up while a feed holds the outputs; the last slot is kept for
/// the feed's own outcome, which is appended after the release with the rest.
#[test]
fn a_feeds_outcome_has_a_slot_however_many_events_pile_up() {
    let (_guard, mut writer, mut provider) = start();
    for _ in 0..7 {
        step(&mut writer, &mut provider); // open, net open, time open, keys, events, stats, its save
    }
    INTERLOCK.claim_feed(u32::MAX).unwrap();

    let mut queued = 0_u8;
    while events::record(None, [queued; 16]) {
        queued += 1;
    }
    assert_eq!(usize::from(queued), events::QUEUE - events::RESERVED);
    assert_eq!(events::take_lost(), 1);
    assert!(
        events::record_outcome(None, [0xfe; 16]),
        "the outcome's slot"
    );
    assert!(!events::record_outcome(None, [0xff; 16]), "one slot");
    step(&mut writer, &mut provider);
    assert_eq!(events::last_seq(), None, "held off by the feed");

    INTERLOCK.release_feed();
    for _ in 0..events::QUEUE {
        step(&mut writer, &mut provider);
    }
    assert_eq!(events::last_seq(), Some(u64::from(queued) + 1));
    assert_eq!(
        events::read_result(A, Some(u64::from(queued))),
        None,
        "asked"
    );
    step(&mut writer, &mut provider);
    let Some(Read::Record(last)) = events::read_result(A, Some(u64::from(queued))) else {
        panic!("the outcome");
    };
    assert_eq!(last.body, [0xfe; 16]);
}

/// A writer record, as its `PLCFG1` line, decodes to `expected` through the
/// host's parser.
fn decodes(record: Record, expected: Option<Event<'_>>) {
    let mut text = String::new();
    writer::describe(record, &mut text).unwrap();
    let line = format!("{} {text}", Tag::Settings);
    let event = Line::parse(&line).and_then(|line| line.event());
    assert_eq!(event, expected, "{line}");
}

/// The writes the bench waits on decode as such; the rest decode as
/// nothing.
#[test]
fn store_writes_decode_for_the_bench() {
    decodes(
        Record::Saved {
            generation: 7,
            settings: Settings::default(),
        },
        Some(Event::Settings(record::Settings::Saved {
            generation: Some(7),
        })),
    );
    decodes(
        Record::NetSaved {
            generation: 3,
            stored: false,
        },
        Some(Event::Settings(record::Settings::NetSaved {
            stored: false,
        })),
    );
    decodes(
        changed(KeyChange::Enroll, KeyResult::Done, 1),
        Some(Event::Settings(record::Settings::KeysChanged)),
    );
    decodes(
        Record::NetOpen {
            status: NetStatus::Loaded,
            generation: 3,
            stored: true,
        },
        None,
    );
}

#[test]
fn time_settings_are_saved_inside_a_marked_attempt_and_published() {
    use wellfed_device_api::mailbox::{NtpServer, TimeSettings, TimeSettingsResult, Zone};
    use wellfed_feeder_runtime::time;

    let (_guard, mut writer, mut provider) = start();
    time::reset();
    assert_eq!(time::current(), TimeSettingsResult::Busy, "not opened yet");
    for _ in 0..7 {
        step(&mut writer, &mut provider); // open, net open, time open, keys, events, stats, its save
    }
    let TimeSettingsResult::Current { view, pending } = time::current() else {
        panic!("the store is published");
    };
    assert_eq!(
        (view.settings, view.generation, pending),
        (TimeSettings::default(), 0, false)
    );
    assert!(view.writable);

    let settings = TimeSettings {
        zone: Some(Zone::parse(b"MST7MDT,M3.2.0,M11.1.0").unwrap()),
        ntp: NtpServer::Address {
            ip: [10, 42, 103, 1],
            port: 1123,
        },
    };
    assert!(matches!(
        time::change(settings),
        TimeSettingsResult::Current { pending: true, .. }
    ));
    assert_eq!(time::change(settings), TimeSettingsResult::Busy);
    ATTEMPT_SEEN.with(|s| s.set(false));
    assert_eq!(
        step(&mut writer, &mut provider),
        Some(Record::TimeSaved { generation: 1 })
    );
    assert!(ATTEMPT_SEEN.with(Cell::get));
    assert_eq!(sdk::backup_read(MARKER_REGISTER), 0);
    let TimeSettingsResult::Current { view, pending } = time::current() else {
        panic!("the store is published");
    };
    assert_eq!(
        (view.settings, view.generation, pending),
        (settings, 1, false)
    );

    // The next boot reads them back.
    let mut writer = Writer::default();
    for _ in 0..2 {
        step(&mut writer, &mut provider);
    }
    assert_eq!(
        step(&mut writer, &mut provider),
        Some(Record::TimeOpen {
            status: TimeStatus::Loaded,
            generation: 1
        })
    );
    let mut text = String::new();
    writer::describe(Record::TimeSaved { generation: 1 }, &mut text).unwrap();
    assert_eq!(text, "time saved generation=1");
}

/// Every opening step, the stats store's last; its record. The boot's own
/// save is the next step.
fn open_all(writer: &mut Writer<Armed>, provider: &mut Provider) -> Option<Record> {
    for _ in 0..5 {
        step(writer, provider); // open, net open, time open, keys, events
    }
    step(writer, provider)
}

/// The stats store's sector pair, as the fake holds it.
fn stats_sectors() -> Vec<u8> {
    let first = wellfed_hal::plaf103::storage::STATS.first as usize * 0x1000;
    fake::with(|h| h.storage[first..first + 0x2000].to_vec())
}

/// Step `n` times with nothing written to the stats store.
fn idle(writer: &mut Writer<Armed>, provider: &mut Provider, n: usize) {
    let before = stats_sectors();
    for _ in 0..n {
        assert_eq!(step(writer, provider), None);
    }
    assert_eq!(stats_sectors(), before, "no save");
}

fn saved(generation: u64, early: u8) -> Option<Record> {
    Some(Record::StatsSaved { generation, early })
}

/// The stats store's erases so far, both sectors.
fn stats_erases() -> u32 {
    fake::with(|h| h.storage_erases[32] + h.storage_erases[33])
}

/// One boot: a fresh writer opened, its own early save taken; the writer,
/// the open record and the save's.
fn boot(provider: &mut Provider) -> (Writer<Armed>, Option<Record>, Option<Record>) {
    stats::reset();
    supervision::reset();
    let mut writer = Writer::default();
    let open = open_all(&mut writer, provider);
    let save = step(&mut writer, provider);
    (writer, open, save)
}

/// The totals load at boot with this boot's reset counted and are saved at
/// once inside a marked attempt (an early save, taking a token), then every
/// `SAVE_MS` of uptime and no more often (settled, giving one back), each
/// save carrying this boot's counts and erases, its own erase included;
/// the next boot reads them back.
#[test]
fn lifetime_totals_count_the_boot_and_save_at_a_bounded_rate() {
    let (_guard, mut writer, mut provider) = start();
    fake::with(|h| h.boot_reason = 1 << 17);
    assert_eq!(
        open_all(&mut writer, &mut provider),
        Some(Record::StatsOpen {
            status: StatsStatus::Empty,
            generation: 0,
            early: 0,
            missed: false,
        })
    );
    let mut base = Totals::ZERO;
    base.count_boot(wellfed_proto::api::ResetCause::Watchdog);
    assert_eq!(stats::totals(), Some(base), "published at once");
    ATTEMPT_SEEN.with(|s| s.set(false));
    assert_eq!(step(&mut writer, &mut provider), saved(1, 1));
    assert!(ATTEMPT_SEEN.with(Cell::get));
    assert_eq!(sdk::backup_read(MARKER_REGISTER), 0);

    let counts = Counts {
        uptime_s: 3_600,
        feeds: 3,
        ..Counts::default()
    };
    stats::hand_counts(counts);
    idle(&mut writer, &mut provider, 3);
    sdk::delay_ms(SAVE_MS - 1);
    idle(&mut writer, &mut provider, 1);
    sdk::delay_ms(1);
    assert_eq!(step(&mut writer, &mut provider), saved(2, 0), "settled");
    assert_eq!(stats::totals(), Some(base), "the base stays this boot's");

    // The rate: nothing until a full interval later, however much changed.
    stats::hand_counts(Counts { feeds: 4, ..counts });
    idle(&mut writer, &mut provider, 5);
    sdk::delay_ms(SAVE_MS - 1);
    idle(&mut writer, &mut provider, 5);
    sdk::delay_ms(1);
    assert_eq!(step(&mut writer, &mut provider), saved(3, 0));
    let erases = |sector: usize| fake::with(|h| h.storage_erases[sector]);
    assert_eq!((erases(32), erases(33)), (2, 1), "one erase per save");

    // The next boot, a power-on, adds itself to what was saved.
    fake::with(|h| h.boot_reason = 0);
    let (_, open, save) = boot(&mut provider);
    assert_eq!(
        open,
        Some(Record::StatsOpen {
            status: StatsStatus::Loaded,
            generation: 3,
            early: 0,
            missed: false,
        })
    );
    assert_eq!(save, saved(4, 1));
    let totals = stats::totals().expect("the stored totals");
    assert_eq!(totals.counts.feeds, 4);
    assert_eq!(totals.counts.uptime_s, 3_600);
    assert_eq!(
        totals.resets,
        Resets {
            power_on: 1,
            watchdog: 1,
            ..Resets::default()
        }
    );
    assert_eq!(
        (totals.storage[32], totals.storage[33]),
        (2, 1),
        "each save counts its own erase"
    );

    let mut text = String::new();
    writer::describe(saved(2, 0).unwrap(), &mut text).unwrap();
    assert_eq!(text, "stats saved generation=2 early=0");
}

/// A watchdog reset loop whose boots end within seconds still counts each
/// reset while early tokens last; the save that takes the last sets the
/// sticky flag, later short boots save nothing, and each settled save of a
/// boot that lasts gives back one token.
#[test]
fn short_boots_count_their_resets_while_tokens_last() {
    let (_guard, _, mut provider) = start();
    fake::with(|h| h.boot_reason = 1 << 17);

    for n in 1..=EARLY_SAVES {
        let (_, _, save) = boot(&mut provider);
        assert_eq!(save, saved(u64::from(n), n), "boot {n}");
    }
    let before = stats_sectors();
    let (mut writer, open, save) = boot(&mut provider);
    let generation = u64::from(EARLY_SAVES);
    assert_eq!(
        open,
        Some(Record::StatsOpen {
            status: StatsStatus::Loaded,
            generation,
            early: EARLY_SAVES,
            missed: true,
        })
    );
    assert_eq!(save, None, "no token left");
    assert_eq!(stats_sectors(), before);
    let watchdog = stats::totals().unwrap().resets.watchdog;
    assert_eq!(watchdog, u32::from(EARLY_SAVES) + 1, "this boot, in RAM");

    // This boot lasts an hour: its settled save counts it and gives one
    // token back, which the next short boot takes.
    sdk::delay_ms(SAVE_MS);
    assert_eq!(
        step(&mut writer, &mut provider),
        saved(generation + 1, EARLY_SAVES - 1)
    );
    let (_, open, save) = boot(&mut provider);
    assert!(
        matches!(open, Some(Record::StatsOpen { missed: true, .. })),
        "the flag stays"
    );
    assert_eq!(save, saved(generation + 2, EARLY_SAVES));
    assert_eq!(
        stats::totals().unwrap().resets.watchdog,
        u32::from(EARLY_SAVES) + 2
    );
    let (_, _, save) = boot(&mut provider);
    assert_eq!(save, None);
}

/// Whatever the crash pattern, the store's erases stay within the early
/// tokens plus two per settled save: here bursts of short boots, each
/// followed by one boot that lasts a single save interval.
#[test]
fn a_mixed_crash_pattern_wears_the_store_at_most_two_erases_an_hour() {
    let (_guard, _, mut provider) = start();
    fake::with(|h| h.boot_reason = 1 << 17);
    let rounds = 20;
    for _ in 0..rounds {
        for _ in 0..u32::from(EARLY_SAVES) {
            boot(&mut provider);
        }
        let (mut writer, _, _) = boot(&mut provider);
        sdk::delay_ms(SAVE_MS);
        assert!(matches!(
            step(&mut writer, &mut provider),
            Some(Record::StatsSaved { .. })
        ));
    }
    let settled = rounds;
    assert!(
        stats_erases() <= u32::from(EARLY_SAVES) + 2 * settled,
        "{} erases",
        stats_erases()
    );
}

/// An intentional restart's grace: the writer saves at once with the
/// counts the loop handed over (an early save) and says it is done, so the
/// restart need not wait out the grace; once per boot.
#[test]
fn an_intentional_restart_saves_in_its_grace() {
    let (_guard, mut writer, mut provider) = start();
    open_all(&mut writer, &mut provider);
    assert_eq!(step(&mut writer, &mut provider), saved(1, 1));

    stats::hand_counts(Counts {
        feeds: 2,
        ..Counts::default()
    });
    let asked = sdk::uptime_ms();
    supervision::begin_restart_grace(asked);
    assert!(!supervision::restart_grace_over(asked), "the writer first");
    assert_eq!(step(&mut writer, &mut provider), saved(2, 2));
    assert!(supervision::restart_grace_over(asked), "flushed: at once");
    assert!(!supervision::restart_grace_over(asked), "only once");
    idle(&mut writer, &mut provider, 3);

    let (_, _, save) = boot(&mut provider);
    assert_eq!(stats::totals().unwrap().counts.feeds, 2);
    assert_eq!(save, saved(3, 3));
}

/// With the budget spent, a restart once the boot has settled still saves
/// its counts, neither taking a token nor giving one back.
#[test]
fn a_settled_restart_saves_with_the_budget_spent() {
    let (_guard, _, mut provider) = start();
    for _ in 1..=EARLY_SAVES {
        boot(&mut provider);
    }
    let (mut writer, _, save) = boot(&mut provider);
    assert_eq!(save, None, "no token left");

    sdk::delay_ms(RESTART_SETTLED_MS);
    stats::hand_counts(Counts {
        feeds: 2,
        ..Counts::default()
    });
    let asked = sdk::uptime_ms();
    supervision::begin_restart_grace(asked);
    assert_eq!(
        step(&mut writer, &mut provider),
        saved(u64::from(EARLY_SAVES) + 1, EARLY_SAVES)
    );
    assert!(supervision::restart_grace_over(asked), "flushed: at once");

    let (_, _, save) = boot(&mut provider);
    assert_eq!(stats::totals().unwrap().counts.feeds, 2);
    assert_eq!(save, None, "still no token");
}

/// A restart's save held by a feed waits only through the grace; then the
/// feeding stops and no write turn is granted, so it never starts into the
/// reset (which would leave the arming marker set and the next boot
/// without its stores). An unarmed writer has nothing to save and lets the
/// restart go at once.
#[test]
fn a_restart_save_never_starts_after_the_grace() {
    let (_guard, mut writer, mut provider) = start();
    open_all(&mut writer, &mut provider);
    assert_eq!(step(&mut writer, &mut provider), saved(1, 1));

    INTERLOCK.claim_feed(u32::MAX).unwrap();
    let asked = sdk::uptime_ms();
    supervision::begin_restart_grace(asked);
    idle(&mut writer, &mut provider, 3);
    let over = asked + supervision::RESTART_GRACE_MS;
    assert!(!supervision::restart_grace_over(over - 1));
    assert!(supervision::restart_grace_over(over), "ran out");
    // The runtime's restart: the feeding stops, and writes are held.
    supervision::stopping();
    INTERLOCK.release_feed();
    idle(&mut writer, &mut provider, 5);

    let (_guard2, mut writer, mut provider) = {
        drop(_guard);
        start()
    };
    open_all(&mut writer, &mut provider);
    ARMED.with(|a| a.set(false));
    supervision::begin_restart_grace(0);
    idle(&mut writer, &mut provider, 1);
    assert!(supervision::restart_grace_over(0), "nothing to wait for");
}

/// A save that falls due during a feed waits for the release and never
/// writes beside a claim, not even one past its bound.
#[test]
fn a_due_save_waits_for_a_feed_and_never_writes_beside_a_claim() {
    let (_guard, mut writer, mut provider) = start();
    assert!(stopped::initialize().verified());
    open_all(&mut writer, &mut provider);

    // The boot's own save, due at once, waits for a feed already running.
    INTERLOCK.claim_feed(u32::MAX).unwrap();
    idle(&mut writer, &mut provider, 5);
    INTERLOCK.release_feed();
    assert_eq!(step(&mut writer, &mut provider), saved(1, 1));

    sdk::delay_ms(SAVE_MS);
    INTERLOCK.claim_feed(u32::MAX).unwrap();
    idle(&mut writer, &mut provider, 10);
    INTERLOCK.release_feed();
    assert_eq!(step(&mut writer, &mut provider), saved(2, 0));

    // A claim never released: past its bound another store's write may go
    // ahead with the outputs stopped, but the totals keep waiting.
    sdk::delay_ms(SAVE_MS);
    INTERLOCK.claim_feed(1_000).unwrap();
    idle(&mut writer, &mut provider, 1);
    sdk::delay_ms(10_000);
    idle(&mut writer, &mut provider, 3);
    assert_eq!(INTERLOCK.status().writes_forced, 0);
    assert!(!INTERLOCK.writing());
    INTERLOCK.release_feed();
    assert_eq!(step(&mut writer, &mut provider), saved(3, 0));
}

/// An unarmed writer never saves the totals, though it reports them; a
/// failed save is not retried for the rest of the boot.
#[test]
fn the_totals_are_saved_only_while_armed_and_never_retried() {
    let (_guard, mut writer, mut provider) = start();
    open_all(&mut writer, &mut provider);
    ARMED.with(|a| a.set(false));
    sdk::delay_ms(SAVE_MS);
    idle(&mut writer, &mut provider, 3);
    assert!(stats::totals().is_some());

    ARMED.with(|a| a.set(true));
    PROGRAM_FAILS.with(|f| f.set(true));
    assert_eq!(
        step(&mut writer, &mut provider),
        Some(Record::StatsError {
            stage: ErrorStage::Save
        })
    );
    PROGRAM_FAILS.with(|f| f.set(false));
    sdk::delay_ms(SAVE_MS);
    idle(&mut writer, &mut provider, 3);
}

#[test]
fn a_calibration_is_saved_inside_a_marked_attempt_and_published() {
    use wellfed_device_api::mailbox::{Calibration, CalibrationResult};
    use wellfed_feeder_runtime::calibration;

    let (_guard, mut writer, mut provider) = start();
    calibration::reset();
    assert_eq!(
        calibration::current(),
        CalibrationResult::Busy,
        "not opened yet"
    );
    open_all(&mut writer, &mut provider);
    step(&mut writer, &mut provider); // the stats store's early save
    let CalibrationResult::Current { view, pending } = calibration::current() else {
        panic!("the store is published");
    };
    assert_eq!(
        (view.calibration, view.generation, pending, view.writable),
        (Calibration::default(), 0, false, true)
    );

    let measured = Calibration {
        portion_mg: Some(9_800),
    };
    assert!(matches!(
        calibration::change(measured),
        CalibrationResult::Current { pending: true, .. }
    ));
    assert_eq!(calibration::change(measured), CalibrationResult::Busy);
    ATTEMPT_SEEN.with(|s| s.set(false));
    let saved = Record::CalibrationSaved {
        generation: 1,
        portion_mg: Some(9_800),
    };
    assert_eq!(step(&mut writer, &mut provider), Some(saved));
    assert!(ATTEMPT_SEEN.with(Cell::get));
    assert_eq!(sdk::backup_read(MARKER_REGISTER), 0);

    // The same value again writes nothing.
    assert!(matches!(
        calibration::change(measured),
        CalibrationResult::Current { pending: true, .. }
    ));
    ATTEMPT_SEEN.with(|s| s.set(false));
    assert_eq!(step(&mut writer, &mut provider), Some(saved));
    assert!(!ATTEMPT_SEEN.with(Cell::get));

    // The next boot reads it back.
    calibration::reset();
    let mut writer = Writer::default();
    step(&mut writer, &mut provider);
    let CalibrationResult::Current { view, .. } = calibration::current() else {
        panic!("the store is published");
    };
    assert_eq!((view.calibration, view.generation), (measured, 1));

    let mut text = String::new();
    writer::describe(saved, &mut text).unwrap();
    assert_eq!(text, "calibration saved generation=1 portion_mg=9800");
}

#[test]
fn a_plans_change_is_applied_saved_inside_a_marked_attempt_and_published() {
    use wellfed_device_api::mailbox::{Claim, PLANS, Plan, PlansChange, PlansResult, Weekdays};
    use wellfed_feeder_runtime::plans;

    let (_guard, mut writer, mut provider) = start();
    plans::reset();
    assert_eq!(plans::current(), PlansResult::Busy, "not opened yet");
    open_all(&mut writer, &mut provider);
    step(&mut writer, &mut provider); // the stats store's early save
    let PlansResult::Current { view, pending } = plans::current() else {
        panic!("the store is published");
    };
    assert_eq!(
        (view.table, view.revision, pending, view.writable),
        ([None; PLANS], 0, false, true)
    );

    let mut table = [None; PLANS];
    table[0] = Some(Plan {
        minute: 7 * 60 + 30,
        portions: 2,
        days: Weekdays::EVERY_DAY,
        enabled: true,
        call: 0,
    });
    let replace = PlansChange::Replace {
        table,
        at_s: Some(1_000),
    };
    assert!(matches!(
        plans::change(replace),
        PlansResult::Current { pending: true, .. }
    ));
    assert_eq!(plans::change(replace), PlansResult::Busy);
    ATTEMPT_SEEN.with(|s| s.set(false));
    let saved = Record::PlansSaved {
        sequence: 1,
        revision: 1,
    };
    assert_eq!(step(&mut writer, &mut provider), Some(saved));
    assert!(ATTEMPT_SEEN.with(Cell::get));
    assert_eq!(sdk::backup_read(MARKER_REGISTER), 0);

    // A claim against a revision the table has left is dropped unwritten.
    let claim = Claim {
        plan: 0,
        due_s: 2_000,
        high_water: 7,
        disabled: false,
    };
    let stale = PlansChange::Claim { claim, revision: 0 };
    assert!(matches!(
        plans::change(stale),
        PlansResult::Current { pending: true, .. }
    ));
    ATTEMPT_SEEN.with(|s| s.set(false));
    assert_eq!(step(&mut writer, &mut provider), Some(Record::PlansDropped));
    assert!(!ATTEMPT_SEEN.with(Cell::get));

    let current = PlansChange::Claim { claim, revision: 1 };
    assert!(matches!(
        plans::change(current),
        PlansResult::Current { pending: true, .. }
    ));
    step(&mut writer, &mut provider);

    // The next boot reads both back.
    plans::reset();
    let mut writer = Writer::default();
    step(&mut writer, &mut provider);
    let PlansResult::Current { view, .. } = plans::current() else {
        panic!("the store is published");
    };
    assert_eq!((view.table, view.claim), (table, Some(claim)));

    let mut text = String::new();
    writer::describe(saved, &mut text).unwrap();
    assert_eq!(text, "plans saved sequence=1 revision=1");
}

/// The meal call's upload lands chunk by chunk inside marked attempts. A
/// change queued while a call plays from the clip waits without holding
/// up the writer's other work, and one asked for meanwhile is refused.
#[test]
fn a_meal_call_change_waits_for_a_call_without_holding_up_other_work() {
    use wellfed_core::store::slot::crc32;
    use wellfed_device_api::mailbox::{CallChange, CallClip, CallResult};
    use wellfed_feeder_runtime::call;
    use wellfed_hal::audio::{self, PlayResult, Record as Audio, Speaker};

    let (_guard, mut writer, mut provider) = start();
    call::reset();
    audio::reset();
    open_all(&mut writer, &mut provider);
    step(&mut writer, &mut provider); // the stats store's early save

    let stream: Vec<u8> = (0..600u32).map(|i| (i * 7) as u8).collect();
    let crc = crc32(&stream);
    let start = CallChange::Start { bytes: 600, crc };
    assert!(matches!(
        call::change(start),
        CallResult::Current { pending: true, .. }
    ));
    ATTEMPT_SEEN.with(|s| s.set(false));
    let receiving = CallClip::Receiving {
        bytes: 600,
        crc,
        received: 0,
    };
    assert_eq!(
        step(&mut writer, &mut provider),
        Some(Record::Call { clip: receiving })
    );
    assert!(ATTEMPT_SEEN.with(Cell::get));
    assert_eq!(call::chunk(0, &stream[..512]), Ok(()));
    assert_eq!(step(&mut writer, &mut provider), None, "mid-upload");
    assert_eq!(call::chunk(512, &stream[512..]), Ok(()));
    let stored = CallClip::Stored { bytes: 600, crc };
    assert_eq!(
        step(&mut writer, &mut provider),
        Some(Record::Call { clip: stored })
    );

    // A delete queued, then a call takes the clip before the writer's turn.
    assert!(matches!(
        call::change(CallChange::Delete),
        CallResult::Current { pending: true, .. }
    ));
    assert_eq!(audio::request_call(1), PlayResult::Started);
    let mut speaker = Speaker::quiet();
    assert!(matches!(
        speaker.step().record,
        Some(Audio::CallPlay { plays: 1, .. })
    ));
    assert!(audio::call_playing());

    // The settings change behind it is saved meanwhile; the delete waits.
    let quieter = SettingsFields {
        volume: Some(10),
        ..SettingsFields::default()
    };
    assert!(matches!(
        settings::request(quieter),
        SettingsResult::Current { pending: true, .. }
    ));
    assert!(matches!(
        step(&mut writer, &mut provider),
        Some(Record::Saved { .. })
    ));
    let CallResult::Current { view, pending } = call::current() else {
        panic!("the call is published");
    };
    assert_eq!((view.clip, pending), (stored, true));

    // Once the call ends, the delete lands.
    for _ in 0..400 {
        let step = speaker.step();
        fake::with(|h| h.uptime_ms += step.delay_ms);
        fake::play_audio(2);
        if matches!(step.record, Some(Audio::CallDone { .. })) {
            break;
        }
    }
    assert!(!audio::call_playing());
    assert_eq!(
        step(&mut writer, &mut provider),
        Some(Record::Call {
            clip: CallClip::None
        })
    );
    // Refused, not queued, while a call plays.
    assert!(matches!(
        call::change(start),
        CallResult::Current { pending: true, .. }
    ));
    step(&mut writer, &mut provider);
    assert_eq!(call::chunk(0, &stream[..512]), Ok(()));
    step(&mut writer, &mut provider);
    assert_eq!(call::chunk(512, &stream[512..]), Ok(()));
    step(&mut writer, &mut provider);
    assert_eq!(audio::request_call(1), PlayResult::Started);
    let mut speaker = Speaker::quiet();
    speaker.step();
    assert_eq!(call::change(CallChange::Delete), CallResult::Busy);
}

/// Whether the factory reset's marker is down.
fn reset_marked() -> bool {
    use wellfed_feeder::store::reset::{MARKER, marked};

    let first = wellfed_hal::plaf103::storage::RESET.first as usize * 0x1000;
    let bytes: [u8; MARKER.len()] =
        fake::with(|h| h.storage[first..first + MARKER.len()].try_into().unwrap());
    marked(&bytes)
}

/// The bytes of `sectors`, as the fake holds them.
fn sectors(sectors: wellfed_hal::plaf103::storage::Sectors) -> Vec<u8> {
    let first = sectors.first as usize * 0x1000;
    let end = first + sectors.count as usize * 0x1000;
    fake::with(|h| h.storage[first..end].to_vec())
}

/// Every mailbox the writer publishes to, as a restart leaves them.
fn restart_mailboxes() {
    use wellfed_feeder_runtime::{calibration, plans, time, traces};

    settings::reset();
    // SAFETY: the test holds `SERIAL` and runs no Wi-Fi task.
    unsafe { network::reset() };
    keys::reset();
    events::reset();
    stats::reset();
    calibration::reset();
    plans::reset();
    wellfed_feeder_runtime::call::reset();
    time::reset();
    traces::reset();
    supervision::reset();
}

/// A first boot with a generator that fills the stores the reset clears
/// and the calibration it keeps, then takes an unlocked Reset hold's
/// change. Sentinels stand in for the journal, the confirmation record and
/// the update store, which this writer never writes, and for a trace.
fn filled_then_reset() -> (MutexGuard<'static, ()>, Provider) {
    use wellfed_device_api::mailbox::{Calibration, PLANS, Plan, PlansChange, Weekdays};
    use wellfed_device_api::wifi::Credentials;
    use wellfed_feeder_runtime::{calibration, plans};
    use wellfed_hal::plaf103::storage;

    let (guard, mut writer, mut provider) = start();
    restart_mailboxes();
    fake::with(|h| {
        h.adc_noise = 0x0f0f_5a5a;
        for kept in [storage::JOURNAL, storage::CONFIRM, storage::UPDATE] {
            let first = kept.first as usize * 0x1000;
            h.storage[first..first + 64].fill(0x42);
        }
        let traces = storage::TRACES.first as usize * 0x1000;
        h.storage[traces..traces + 64].fill(0x24);
        // Bytes in the meal call's first and last sectors.
        for sector in [storage::CALL.first, storage::CALL.first + 7] {
            let first = sector as usize * 0x1000;
            h.storage[first..first + 64].fill(0x77);
        }
    });
    assert_eq!(rng::start().result, Ok(()));
    for _ in 0..7 {
        step(&mut writer, &mut provider);
    }

    settings::request(SettingsFields {
        volume: Some(7),
        ..SettingsFields::default()
    });
    step(&mut writer, &mut provider);
    network::change(Some(Credentials::new(b"home", b"passphrase").unwrap()));
    step(&mut writer, &mut provider);
    calibration::change(Calibration {
        portion_mg: Some(9_800),
    });
    step(&mut writer, &mut provider);
    let mut table = [None; PLANS];
    table[0] = Some(Plan {
        minute: 8 * 60,
        portions: 2,
        days: Weekdays::EVERY_DAY,
        enabled: true,
        call: 0,
    });
    plans::change(PlansChange::Replace {
        table,
        at_s: Some(1_000),
    });
    step(&mut writer, &mut provider);
    let phone = Change::Enroll {
        key: [7; 32],
        label: Label::new(b"phone").unwrap(),
        role: Role::Owner,
    };
    change(&mut writer, &mut provider, phone);
    assert!(events::record(Some(1_700_000_000), [1; 16]));
    step(&mut writer, &mut provider);
    for cleared in [
        storage::SETTINGS,
        storage::NET,
        storage::PLANS,
        storage::EVENTS,
        storage::TRACES,
        storage::STATS,
    ] {
        assert!(
            sectors(cleared).iter().any(|&byte| byte != 0xff),
            "{cleared:?} holds something to clear"
        );
    }

    assert_eq!(
        change(&mut writer, &mut provider, Change::Reset),
        changed(KeyChange::Reset, KeyResult::Done, 0)
    );
    assert!(reset_marked());
    (guard, provider)
}

/// What the reset keeps, byte for byte: the journal (its high water and
/// any uncertain feed's record), the confirmation record, the update store
/// and the calibration.
fn kept() -> Vec<Vec<u8>> {
    use wellfed_hal::plaf103::storage;

    [
        storage::JOURNAL,
        storage::CONFIRM,
        storage::UPDATE,
        storage::CALIBRATION,
    ]
    .into_iter()
    .map(sectors)
    .collect()
}

/// A boot that finds the marker: `steps` writer steps, or until the stores
/// are open and the boot's stats save is done; its records.
fn reset_boot(provider: &mut Provider, steps: Option<usize>) -> Vec<Record> {
    restart_mailboxes();
    let mut writer = Writer::default();
    let mut records = Vec::new();
    let mut left: Option<usize> = None;
    for taken in 0..100 {
        if steps == Some(taken) || left == Some(0) {
            return records;
        }
        let record = step(&mut writer, provider);
        left = left.map(|n| n - 1);
        if matches!(
            record,
            Some(Record::ResetDone { .. } | Record::Open { .. } | Record::HandoverDone { .. })
        ) {
            // Net, time, keys, events, stats and the stats save, or the
            // handover's seven steps.
            left = Some(7);
        }
        records.extend(record);
    }
    panic!("the boot never settled: {records:?}");
}

/// A paired feeder on Wi-Fi with one more boot to go: the identity it has.
fn paired_with_wifi() -> (MutexGuard<'static, ()>, Provider, [u8; 32]) {
    use wellfed_device_api::mailbox::{PLANS, Plan, PlansChange, Weekdays};
    use wellfed_device_api::wifi::Credentials;
    use wellfed_feeder_runtime::plans;

    let (guard, mut writer, mut provider) = start();
    fake::with(|h| h.adc_noise = 0x0f0f_5a5a);
    assert_eq!(rng::start().result, Ok(()));
    for _ in 0..7 {
        step(&mut writer, &mut provider);
    }
    network::change(Some(Credentials::new(b"home", b"passphrase").unwrap()));
    step(&mut writer, &mut provider);
    let mut table = [None; PLANS];
    table[0] = Some(Plan {
        minute: 8 * 60,
        portions: 2,
        days: Weekdays::EVERY_DAY,
        enabled: true,
        call: 0,
    });
    plans::change(PlansChange::Replace {
        table,
        at_s: Some(1_000),
    });
    step(&mut writer, &mut provider);
    assert!(
        sectors(wellfed_hal::plaf103::storage::PLANS)
            .iter()
            .any(|&b| b != 0xff)
    );
    let phone = Change::Enroll {
        key: [7; 32],
        label: Label::new(b"phone").unwrap(),
        role: Role::Owner,
    };
    change(&mut writer, &mut provider, phone);
    let (_, identity) = keys::identity_since(0).expect("the identity is published");
    (guard, provider, identity)
}

fn plant_handover() {
    let at = storage_reset_first();
    fake::with(|h| h.storage[at..at + 16].copy_from_slice(&handover::MARKER));
}

fn storage_reset_first() -> usize {
    wellfed_hal::plaf103::storage::RESET.first as usize * 0x1000
}

/// The boot that finds the handover mark clears the Wi-Fi, plans and clients,
/// keeps the identity the backup is filed by, publishes nothing stale, and
/// erases the mark, so a later boot keeps whatever pairing comes next.
#[test]
fn a_handover_clears_wifi_and_clients_and_keeps_the_identity() {
    use wellfed_hal::plaf103::storage;

    let (_guard, mut provider, identity) = paired_with_wifi();
    plant_handover();

    let records = reset_boot(&mut provider, None);

    assert!(
        records.contains(&Record::HandoverDone { cleared: true }),
        "{records:?}"
    );
    assert!(sectors(storage::NET).iter().all(|&byte| byte == 0xff));
    assert!(sectors(storage::PLANS).iter().all(|&byte| byte == 0xff));
    assert!(sectors(storage::RESET).iter().all(|&byte| byte == 0xff));
    assert_eq!(keys::identity_since(0).map(|(_, key)| key), Some(identity));
    assert!(
        records.iter().any(|record| matches!(
            record,
            Record::Keys {
                enrolled: 0,
                identity: Identity::Loaded,
                ..
            }
        )),
        "{records:?}"
    );
}

/// The floor holds across an ordinary boot, and only the handover forgets it.
#[test]
fn a_handover_forgets_the_update_floor_and_an_ordinary_boot_does_not() {
    use wellfed_core::ota::offer::ImageKind;
    use wellfed_core::store::update::{Candidate, UpdateStore};
    use wellfed_hal::plaf103::storage;

    type Slots = wellfed_sdk::storage::Region<Armed, { storage::UPDATE.first }, 2>;
    let floor = || {
        let mut update = UpdateStore::default();
        update.open(Slots::new());
        update.record().min_version
    };

    let (_guard, mut provider, _) = paired_with_wifi();
    let mut update = UpdateStore::default();
    update.open(Slots::new());
    let build = [0x55; 32];
    update
        .set_activated(Candidate {
            digest: [1; 32],
            version: 5,
            build_id: build,
            kind: ImageKind::Replacement,
        })
        .unwrap();
    update.confirm(&build).unwrap();

    reset_boot(&mut provider, None);
    assert_eq!(floor(), 5);

    plant_handover();
    let records = reset_boot(&mut provider, None);
    assert!(
        records.contains(&Record::HandoverDone { cleared: true }),
        "{records:?}"
    );
    assert_eq!(floor(), 0);
}

/// A cut at each step of the clearing leaves the mark, and the next boot does
/// the whole of it, keeping the identity.
#[test]
fn a_cut_during_a_handover_repeats_it() {
    use wellfed_hal::plaf103::storage;

    for cut in 1..8 {
        let (guard, mut provider, identity) = paired_with_wifi();
        plant_handover();

        let first = reset_boot(&mut provider, Some(cut));
        assert!(
            !first.contains(&Record::HandoverDone { cleared: true }),
            "the mark goes last: {first:?}"
        );
        let second = reset_boot(&mut provider, None);

        assert!(
            second.contains(&Record::HandoverDone { cleared: true }),
            "after {cut} steps: {second:?}"
        );
        assert!(sectors(storage::NET).iter().all(|&byte| byte == 0xff));
        assert_eq!(keys::identity_since(0).map(|(_, key)| key), Some(identity));
        drop(guard);
    }
}

/// An erase that fails leaves the mark for the next boot, and this boot goes
/// on rather than serving nothing.
#[test]
fn a_handover_that_cannot_finish_waits_for_the_next_boot() {
    let (_guard, mut provider, _) = paired_with_wifi();
    plant_handover();
    PROGRAM_FAILS.with(|f| f.set(true));

    let records = reset_boot(&mut provider, None);

    assert!(
        records
            .iter()
            .any(|record| matches!(record, Record::HandoverStopped { .. })),
        "{records:?}"
    );
    let at = storage_reset_first();
    assert_eq!(
        fake::with(|h| h.storage[at..at + 16].to_vec()),
        handover::MARKER
    );
}

/// After a finished reset: every cleared store opens empty and writable,
/// the clients are gone under a new identity, the history starts again
/// under a new epoch, the totals start from this boot, and what it keeps is
/// unchanged.
fn assert_cleared(kept_before: &[Vec<u8>], old_epoch: u32) {
    use wellfed_device_api::mailbox::{
        Calibration, CalibrationResult, PLANS, PlansResult, TimeSettingsResult, WifiResult,
    };
    use wellfed_feeder_runtime::{calibration, plans, time};
    use wellfed_hal::plaf103::storage;

    assert!(!reset_marked());
    assert_eq!(current(), (true, true, 50, 0, true), "default settings");
    let WifiResult::Current { view, .. } = network::current() else {
        panic!("the Wi-Fi store is published");
    };
    assert_eq!((view.stored_ssid_crc, view.generation), (None, 0));
    let TimeSettingsResult::Current { view, .. } = time::current() else {
        panic!("the time settings are published");
    };
    assert_eq!((view.generation, view.writable), (0, true));
    let PlansResult::Current { view, .. } = plans::current() else {
        panic!("the plans are published");
    };
    assert_eq!((view.table, view.revision), ([None; PLANS], 0));
    let CalibrationResult::Current { view, .. } = calibration::current() else {
        panic!("the calibration is published");
    };
    let measured = Calibration {
        portion_mg: Some(9_800),
    };
    assert_eq!((view.calibration, view.generation), (measured, 1));

    let mut table = [None; wellfed_device_api::clients::CLIENTS];
    assert!(keys::clients_since(0, &mut table).is_some());
    assert!(table.iter().all(Option::is_none), "every client forgotten");
    assert!(keys::identity_since(0).is_some(), "a new identity");
    let epoch = events::epoch().expect("the ring is open");
    assert_ne!(epoch, old_epoch, "a new history, under its own epoch");
    assert_eq!(events::last_seq(), None, "the history is empty");
    let totals = stats::totals().expect("the totals are published");
    assert_eq!(totals.resets.power_on, 1, "the totals start from this boot");
    assert_eq!(totals.counts.uptime_s, 0);
    assert!(
        totals.storage.iter().any(|&erases| erases > 0),
        "the flash erase counts, the hardware's wear, are kept"
    );

    for cleared in [
        storage::SETTINGS,
        storage::NET,
        storage::TIME,
        storage::PLANS,
        storage::TRACES,
        storage::EVENTS,
        storage::CALL,
    ] {
        assert!(
            sectors(cleared).iter().all(|&byte| byte == 0xff),
            "{cleared:?} erased"
        );
    }
    assert_eq!(kept(), kept_before, "the kept stores are unchanged");
}

#[test]
fn a_factory_reset_clears_the_decided_stores_on_the_next_boot_and_keeps_the_rest() {
    let (_guard, mut provider) = filled_then_reset();
    let old_epoch = events::epoch().unwrap();
    let kept_before = kept();

    ATTEMPT_SEEN.with(|s| s.set(false));
    let records = reset_boot(&mut provider, None);
    assert_eq!(records.first(), Some(&Record::ResetPending));
    assert!(
        records.contains(&Record::ResetDone { erased: 33 }),
        "{records:?}"
    );
    assert!(
        records.iter().any(|record| matches!(
            record,
            Record::Keys {
                clients: ClientStatus::Empty,
                identity: Identity::Created,
                ..
            }
        )),
        "{records:?}"
    );
    assert_cleared(&kept_before, old_epoch);
    assert!(ATTEMPT_SEEN.with(Cell::get));
    assert_eq!(sdk::backup_read(MARKER_REGISTER), 0);

    // The boot after is an ordinary one.
    let records = reset_boot(&mut provider, None);
    assert!(
        matches!(records.first(), Some(Record::Open { .. })),
        "{records:?}"
    );
}

/// A power cut after any number of the reset's erases, the marker's
/// included, with the erase it cut torn: the next boot finishes the reset
/// with the same result. Nothing is published while it runs.
#[test]
fn a_power_cut_at_any_step_of_the_reset_leaves_it_to_the_next_boot() {
    use wellfed_hal::plaf103::storage;

    let (_guard, mut provider) = filled_then_reset();
    let old_epoch = events::epoch().unwrap();
    let kept_before = kept();
    let marked = fake::with(|h| h.storage.clone());

    // `done` steps landed (each sector's erase, then the statistics'
    // wear-only save, then the marker's erase) and the next was cut: an
    // erase left half its sector, the save is all or nothing.
    let erases = storage::FACTORY_CLEARED_SECTORS;
    for done in 0..=erases + 1 {
        fake::with(|h| h.storage.clone_from(&marked));
        let steps = usize::try_from(done).unwrap() + 1;
        let records = reset_boot(&mut provider, Some(steps));
        assert_eq!(records, [Record::ResetPending], "cut after {done} steps");
        assert_eq!(
            settings::request(SettingsFields::default()),
            SettingsResult::Busy,
            "nothing is served mid-reset"
        );
        assert_eq!(keys::identity_since(0), None, "no identity mid-reset");

        let torn = match done {
            _ if done < erases => storage::factory_cleared(done),
            _ if done == erases => None,
            _ => Some(storage::RESET.first),
        };
        if let Some(torn) = torn {
            fake::with(|h| {
                let first = torn as usize * 0x1000;
                h.storage[first..first + 0x800].fill(0xff);
            });
        }

        // A torn marker reads as no reset: every step before it is done.
        let records = reset_boot(&mut provider, None);
        let finished = done <= erases;
        assert_eq!(
            records.contains(&Record::ResetDone { erased: 33 }),
            finished,
            "cut after {done} steps: {records:?}"
        );
        assert_cleared(&kept_before, old_epoch);
    }
}

/// A cut while the marker is programmed leaves it torn, which is no reset:
/// nothing had been erased, and the stores open as they were.
#[test]
fn a_torn_marker_is_no_reset() {
    use wellfed_feeder::store::reset::MARKER;
    use wellfed_hal::plaf103::storage;

    let (_guard, mut writer, mut provider) = start();
    for _ in 0..7 {
        step(&mut writer, &mut provider);
    }
    settings::request(SettingsFields {
        volume: Some(9),
        ..SettingsFields::default()
    });
    step(&mut writer, &mut provider);
    fake::with(|h| {
        let first = storage::RESET.first as usize * 0x1000;
        h.storage[first..first + 8].copy_from_slice(&MARKER[..8]);
    });

    let records = reset_boot(&mut provider, None);
    assert!(
        matches!(records.first(), Some(Record::Open { generation: 1, .. })),
        "{records:?}"
    );
    assert_eq!(current().2, 9);
}

/// An unarmed boot that no arming skip explains cannot erase and could
/// not after a restart either: it stops the reset, serves nothing, keeps
/// the marker and waits for a power cycle, asking for no restart.
#[test]
fn an_unarmed_boot_serves_nothing_and_keeps_the_marker() {
    let (_guard, mut provider) = filled_then_reset();
    restart_mailboxes();
    let mut writer = Writer::default();
    assert_eq!(step(&mut writer, &mut provider), Some(Record::ResetPending));
    // The hooks went away after the open (arming itself is the
    // confirmation's to test).
    ARMED.with(|a| a.set(false));

    assert_eq!(
        step(&mut writer, &mut provider),
        Some(Record::ResetError {
            stage: ErrorStage::Unarmed,
            sector: None,
            restart: false,
        })
    );
    assert!(!keys::restart_wanted());
    assert_eq!(step(&mut writer, &mut provider), None, "stopped");
    assert_eq!(
        settings::request(SettingsFields::default()),
        SettingsResult::Busy
    );
    assert_eq!(keys::identity_since(0), None);
    assert!(reset_marked());
}

/// A boot whose erase fails asks for one restart; the restarted boot (the
/// watchdog's flags, as every intentional restart on this board) fails
/// again and asks for none, since the marker sector counts both attempts.
/// A later boot that can erase finishes the reset and clears the marker
/// with its attempts.
#[test]
fn a_reset_that_cannot_finish_asks_for_one_restart_only() {
    use wellfed_feeder::store::reset::{ATTEMPT, MARKER};
    use wellfed_hal::plaf103::storage;

    let (_guard, mut provider) = filled_then_reset();
    let stuck = storage::factory_cleared(3).unwrap();
    ERASE_FAILS.with(|f| f.set(Some(stuck)));
    let error = |restart| Record::ResetError {
        stage: ErrorStage::Erase,
        sector: Some(stuck),
        restart,
    };

    for (flags, restart) in [(0, true), (0x0010_0000, false), (0x0010_0000, false)] {
        fake::with(|h| h.boot_reason = flags);
        let records = reset_boot(&mut provider, Some(10));
        assert_eq!(
            records,
            [Record::ResetPending, error(restart)],
            "flags {flags:#x}"
        );
        assert_eq!(keys::restart_wanted(), restart);
        assert!(reset_marked());
    }
    let first = storage::RESET.first as usize * 0x1000;
    let words = fake::with(|h| h.storage[first..first + 48].to_vec());
    assert_eq!(words, [MARKER, ATTEMPT, ATTEMPT].concat(), "two attempts");

    ERASE_FAILS.with(|f| f.set(None));
    let records = reset_boot(&mut provider, None);
    assert!(
        records.contains(&Record::ResetDone { erased: 33 }),
        "{records:?}"
    );
    assert!(!keys::restart_wanted());
    let words = fake::with(|h| h.storage[first..first + 48].to_vec());
    assert!(
        words.iter().all(|&b| b == 0xff),
        "marker and attempts cleared"
    );
}

/// A client store the writer cannot write (a newer schema reads as
/// read-only) is when an owner reaches for a reset: once the marker is down
/// the answer is `Done`, so the loop restarts, every session ends, and the
/// next boot erases the store and draws a new identity.
#[test]
fn a_reset_over_an_unwritable_client_store_still_completes() {
    use wellfed_feeder::store::clients::RECORD_BYTES;
    use wellfed_hal::plaf103::storage;

    let (_guard, mut writer, mut provider) = start();
    fake::with(|h| h.adc_noise = 0x3c3c_a5a5);
    assert_eq!(rng::start().result, Ok(()));
    for _ in 0..7 {
        step(&mut writer, &mut provider);
    }
    let phone = Change::Enroll {
        key: [7; 32],
        label: Label::new(b"phone").unwrap(),
        role: Role::Owner,
    };
    change(&mut writer, &mut provider, phone);

    // Every valid slot claims a newer schema.
    fake::with(|h| {
        for slot in 0..2 {
            let first = (storage::CLIENTS.first as usize + slot) * 0x1000;
            let record = &mut h.storage[first..first + RECORD_BYTES];
            if record[..4] == [0xff; 4] {
                continue;
            }
            record[8] = 0x7f;
            let crc_at = RECORD_BYTES - 8;
            let crc = wellfed_core::store::slot::crc32(&record[..crc_at]);
            record[crc_at..crc_at + 4].copy_from_slice(&crc.to_le_bytes());
        }
    });
    restart_mailboxes();
    let mut writer = Writer::default();
    let records: Vec<Record> = (0..7)
        .filter_map(|_| step(&mut writer, &mut provider))
        .collect();
    assert!(
        records.iter().any(|record| matches!(
            record,
            Record::Keys {
                clients: ClientStatus::ReadOnly,
                ..
            }
        )),
        "{records:?}"
    );

    let Record::KeysChanged {
        change: kind,
        result,
        ..
    } = change(&mut writer, &mut provider, Change::Reset)
    else {
        panic!("the reset's record");
    };
    assert_eq!((kind, result), (KeyChange::Reset, KeyResult::Done));
    assert!(reset_marked());
    let mut table = [None; wellfed_device_api::clients::CLIENTS];
    assert!(keys::clients_since(0, &mut table).is_some());
    assert!(table.iter().all(Option::is_none), "no session outlives it");

    let records = reset_boot(&mut provider, None);
    assert!(
        records.contains(&Record::ResetDone { erased: 33 }),
        "{records:?}"
    );
    assert!(
        records.iter().any(|record| matches!(
            record,
            Record::Keys {
                clients: ClientStatus::Empty,
                identity: Identity::Created,
                ..
            }
        )),
        "{records:?}"
    );
}

/// Events the loop records while the boot erases (its `Boot` and `Supply`,
/// a resolved uncertain feed) wait in the queue, and land in the new ring
/// once it opens, in order.
#[test]
fn events_recorded_during_the_reset_wait_for_the_new_ring() {
    let (_guard, mut provider) = filled_then_reset();
    restart_mailboxes();
    let mut writer = Writer::default();
    assert_eq!(step(&mut writer, &mut provider), Some(Record::ResetPending));
    assert!(events::record(None, [1; 16]));
    step(&mut writer, &mut provider);
    assert!(events::record_outcome(None, [2; 16]));

    let mut done = false;
    for _ in 0..100 {
        let record = step(&mut writer, &mut provider);
        done |= matches!(record, Some(Record::ResetDone { .. }));
        if done && events::last_seq() == Some(2) {
            break;
        }
    }
    assert!(done);
    assert_eq!(events::last_seq(), Some(2), "both landed");
    assert_eq!(events::take_lost(), 0);
}

#[test]
fn reset_records_format() {
    let line = |record: Record| {
        let mut text = String::new();
        writer::describe(record, &mut text).unwrap();
        text
    };
    assert_eq!(line(Record::ResetPending), "reset pending");
    assert_eq!(line(Record::ResetWearLost), "reset wear_lost=1");
    assert_eq!(
        line(Record::ResetDone { erased: 33 }),
        "reset done erased=33"
    );
    assert_eq!(
        line(Record::ResetError {
            stage: ErrorStage::Erase,
            sector: Some(13),
            restart: true,
        }),
        "reset error stage=erase sector=13 restart=1"
    );
    assert_eq!(
        line(Record::ResetError {
            stage: ErrorStage::Unarmed,
            sector: None,
            restart: false,
        }),
        "reset error stage=unarmed restart=0"
    );
}

/// Flash that takes no program cannot count an attempt: the boot stops
/// the reset without asking for a restart, so dead flash never restarts
/// the feeder over and over.
#[test]
fn an_attempt_that_does_not_land_asks_for_no_restart() {
    use wellfed_hal::plaf103::storage;

    let (_guard, mut provider) = filled_then_reset();
    PROGRAM_FAILS.with(|f| f.set(true));
    let records = reset_boot(&mut provider, Some(5));
    assert_eq!(
        records,
        [
            Record::ResetPending,
            Record::ResetError {
                stage: ErrorStage::Save,
                sector: Some(storage::RESET.first),
                restart: false,
            }
        ]
    );
    assert!(!keys::restart_wanted());
    assert!(reset_marked());

    PROGRAM_FAILS.with(|f| f.set(false));
    let records = reset_boot(&mut provider, None);
    assert!(
        records.contains(&Record::ResetDone { erased: 33 }),
        "{records:?}"
    );
}

/// Statistics whose pair takes no program (worn out) do not stop the reset
/// for good: the wear-only save fails, the pair is erased instead (only the
/// erase counts are lost), and the marker is erased. A cut after the pair's
/// first erase, or after both, still converges on the next boot.
#[test]
fn a_worn_statistics_pair_loses_the_wear_counts_but_not_the_reset() {
    use wellfed_hal::plaf103::storage;

    let (_guard, mut provider) = filled_then_reset();
    let marked = fake::with(|h| h.storage.clone());
    STATS_PROGRAMS_FAIL.with(|f| f.set(true));

    let records = reset_boot(&mut provider, None);
    let lost = records
        .iter()
        .position(|record| *record == Record::ResetWearLost);
    let done = records
        .iter()
        .position(|record| *record == Record::ResetDone { erased: 33 });
    assert!(lost.is_some() && lost < done, "{records:?}");
    assert!(!reset_marked());
    assert!(
        sectors(storage::STATS).iter().all(|&byte| byte == 0xff),
        "the worn pair is erased, and its boot save fails"
    );

    // Cut after the pair's first erase, and after both: the next boot
    // erases what is left, or finds nothing to keep, and finishes.
    let erases = usize::try_from(storage::FACTORY_CLEARED_SECTORS).unwrap();
    for erased in 1..=storage::STATS.count {
        fake::with(|h| h.storage.clone_from(&marked));
        let records = reset_boot(&mut provider, Some(erases + 1));
        assert_eq!(records, [Record::ResetPending]);
        for sector in storage::STATS.first..storage::STATS.first + erased {
            // SAFETY: the fake's storage, which this test owns.
            assert!(unsafe { sdk::storage_erase(sector) });
        }

        let records = reset_boot(&mut provider, None);
        assert!(
            records.contains(&Record::ResetDone { erased: 33 }),
            "{erased} erased: {records:?}"
        );
        if erased == storage::STATS.count {
            assert!(!records.contains(&Record::ResetWearLost), "nothing to keep");
        }
        assert!(!reset_marked());
    }

    // A pair that takes neither its save nor its erase stops the reset,
    // as before.
    fake::with(|h| h.storage.clone_from(&marked));
    ERASE_FAILS.with(|f| f.set(Some(storage::STATS.first)));
    let records = reset_boot(&mut provider, Some(erases + 5));
    assert_eq!(
        records,
        [
            Record::ResetPending,
            Record::ResetError {
                stage: ErrorStage::Save,
                sector: Some(storage::STATS.first),
                restart: false,
            }
        ]
    );
    assert!(reset_marked());
}
