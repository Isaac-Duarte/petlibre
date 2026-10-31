//! The device settings writer, normal role, in the SDK runtime's service
//! task (`wellfed_store_step`). It opens the store in
//! storage sectors 2 and 3, publishes it for the device API
//! (`wellfed_hal::settings`), arms the flash-lock hooks once as the
//! confirmation does, and then saves each change the device API queues.
//! Each save sets the arming attempt marker and clears it after, so a hang
//! inside a save resets at most once per power cycle.
//!
//! Once armed it also keeps the keys. It mixes the seed stored across
//! boots into the generator and stores the next one (sectors 7 and 8), and
//! opens the identity and client store (sectors 9 and 10), creating the
//! feeder's identity secret on the first boot that has a generator. It
//! publishes the identity for the loop task and applies the client changes
//! the loop queues ([`keys`]): enrollments, revocations and the
//! Reset, the factory reset.
//!
//! The factory reset ([`reset_marker`]) is split across a restart so a
//! power cut anywhere leaves it safe. The Reset change programs the marker
//! (sector 36) and forgets every client; the loop then restarts. A boot
//! that finds the marker opens and publishes nothing the reset clears,
//! erases each cleared sector (`storage::FACTORY_CLEARED`) as its own
//! step, rewrites the lifetime statistics keeping only the flash erase
//! counts (the hardware's wear), erases the marker last, and only then
//! opens the stores, empty (the meal call's clip among them). The feed
//! journal, the calibration, the update
//! store and the seed are never touched.
//!
//! It keeps the time settings too (sectors 27 and 28,
//! `wellfed_feeder::store::time`): the owner's zone and the NTP server,
//! published for the loop ([`time`]), the portion calibration (sectors 34
//! and 35, `wellfed_feeder::store::calibration`, [`calibration`]), and the
//! feeding plans (sectors 11 and 12, `wellfed_feeder::store::plans`,
//! [`plans`]): a client's table, and the loop's claims and passes. And the
//! meal call (sectors 40-47, `wellfed_feeder::store::call`, [`call`]): an
//! upload a chunk at a time, written only while no call plays from it.
//!
//! It also keeps the event history (sectors 13-24,
//! `wellfed_feeder::store::events`). It appends the records the loop queues
//! and answers the device API's reads ([`events`]), polling
//! faster while either waits, so a reconnecting client catches up in
//! seconds.
//! It publishes the ring's fill for the statistics ([`stats`]).
//!
//! And it keeps the per-feed traces (sectors 29-31,
//! `wellfed_feeder::store::traces`): a feed's trace, sealed in RAM once the
//! feed is over, is appended to the ring on the writer's next turn
//! ([`traces`]).
//!
//! It keeps the lifetime statistics too (sectors 32 and 33,
//! `wellfed_feeder::store::stats`): it loads the totals, counts this
//! boot's reset and publishes them for the loop, and saves them with this
//! boot's counts and erases as the boot starts (so a short boot's reset
//! counts), in an intentional restart's grace (before the watchdog's
//! feeding stops), and every [`SAVE_MS`] of uptime; only
//! when nothing else is queued, never per event, and the early ones within
//! the store's tokens.
//!
//! No write starts while a feed may move the outputs: a flash lease then
//! fails its stopped-outputs check and resets the board. So each store write
//! first takes the write turn in `wellfed_hal::interlock`. While a feed
//! holds the outputs the work stays queued (a client change, a Wi-Fi save,
//! the event queue in order) and the writer asks again each poll, so it runs
//! once the owner is at rest with its outputs stopped, before the next feed.
//! Reads of the event ring are answered meanwhile: they take no lease.

use core::fmt;

use wellfed_core::named_enum;

use wellfed_core::store::handover;
use wellfed_core::store::seed::{SeedStatus, SeedStore};
use wellfed_core::store::update::{UpdateStatus, UpdateStore};
use wellfed_device_api::mailbox::{CallChange, CallClip};
use wellfed_feeder::stats::{SAVE_MS, TOTAL_SECTORS, Totals, reset_cause};
use wellfed_feeder::store::calibration::CalibrationStore;
use wellfed_feeder::store::call::CallStore;
use wellfed_feeder::store::clients::{ClientError, ClientStatus, ClientStore};
use wellfed_feeder::store::events::{self as ring, EMPTY, EventLog};
use wellfed_feeder::store::net::{NetStatus, NetStore};
use wellfed_feeder::store::plans::PlansStore;
use wellfed_feeder::store::reset as reset_marker;
use wellfed_feeder::store::settings::{Settings, SettingsStatus, SettingsStore};
use wellfed_feeder::store::stats::{SaveKind, StatsStatus, StatsStore};
use wellfed_feeder::store::time::{TimeStatus, TimeStore};
use wellfed_feeder::store::traces::TraceRing;
use wellfed_hal::flash::park::Mmio;
use wellfed_hal::flash_lock::arming::{self, Arming, Hooks, Poll};
use wellfed_hal::flash_lock::provider::Provider;
use wellfed_hal::interlock::{INTERLOCK, WRITER_POLL_MS, WriteStep, WriteTurn};
use wellfed_hal::plaf103::storage::{FACTORY_CLEARED_SECTORS, factory_cleared};
use wellfed_hal::{audio, rng, sdk, settings, supervision};
use wellfed_sdk::storage::{
    CLIENTS_BASE, EVENTS_BASE, EVENTS_UNITS, FlashOps, Region, SEED_BASE, SETTINGS_BASE,
    SETTINGS_BINDING, Slots, TRACES_BASE, TRACES_UNITS,
};

use crate::calibration::{self, CALIBRATION_BASE};
use crate::call::{self, CALL_BASE, CALL_UNITS, Queued};
use crate::events;
pub use crate::keys::KeyResult;
use crate::keys::{self, Change};
use crate::network::{self, NET_BASE};
use crate::plans::{self, PLANS_BASE};
use crate::stats::{self, STATS_BASE};
use crate::time::{self, TIME_BASE};
use crate::traces;
use wellfed_feeder::feeder::platform::EventsFill;

/// The factory reset's marker sector (`storage::RESET`).
pub const RESET_BASE: u32 = wellfed_hal::plaf103::storage::RESET.first;
const UPDATE_BASE: u32 = wellfed_hal::plaf103::storage::UPDATE.first;
const CONFIRM_SECTOR: u32 = wellfed_hal::plaf103::storage::CONFIRM.first;

pub const POLL_MS: u32 = 200;
/// Between a factory reset's erases, each its own step and write turn.
pub const RESET_POLL_MS: u32 = 1;
/// While events wait to be written or read.
pub const EVENTS_POLL_MS: u32 = 5;
/// Polls at [`EVENTS_POLL_MS`] after the last answered read: about 1 s.
/// A subscriber takes a batch one loop pass a record, and a pass that
/// sends a frame runs about 16 ms (measured), so a batch of 16 outlasts
/// a shorter linger and its next read would wait out the idle poll.
pub const READ_LINGER_POLLS: u32 = 200;
/// While a write waits for a feed to end: within the poll a claim allows a
/// waiting writer before passing it.
pub const WRITE_WAIT_POLL_MS: u32 = POLL_MS;
const _: () = assert!(WRITE_WAIT_POLL_MS <= WRITER_POLL_MS);

/// Where the meal call's queued work stands after the writer's look.
enum CallSave {
    /// Written (or refused), with the record it makes, if any.
    Done(Option<Record>),
    /// A feed holds the outputs.
    Turn,
    /// A call plays from the clip.
    Held,
}

/// The write turn for one store write, given back when dropped.
struct WriteHeld;

impl Drop for WriteHeld {
    fn drop(&mut self) {
        INTERLOCK.end_write();
    }
}

named_enum! {
    /// Why a settings or Wi-Fi save did not happen (`error stage=`).
    pub enum ErrorStage {
        Unarmed = "unarmed",
        Save = "save",
        /// A factory reset's erase failed.
        Erase = "erase",
    }
}

named_enum! {
    /// What became of the feeder's identity secret this boot.
    pub enum Identity {
        Loaded = "loaded",
        Created = "created",
        /// No generator, a store it may not write, or unarmed.
        Missing = "missing",
        Failed = "failed",
    }
}

named_enum! {
    /// A client store change the loop queued.
    pub enum KeyChange {
        Enroll = "enroll",
        Revoke = "revoke",
        Reset = "reset",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    Open {
        status: SettingsStatus,
        generation: u64,
        settings: Settings,
    },
    Arming(arming::Event),
    Saved {
        generation: u64,
        settings: Settings,
    },
    Error {
        stage: ErrorStage,
        status: SettingsStatus,
    },
    /// Wi-Fi credentials (sectors 5 and 6): what the store holds.
    NetOpen {
        status: NetStatus,
        generation: u64,
        stored: bool,
    },
    NetSaved {
        generation: u64,
        stored: bool,
    },
    NetError {
        stage: ErrorStage,
    },
    /// The time settings (sectors 27 and 28).
    TimeOpen {
        status: TimeStatus,
        generation: u64,
    },
    TimeSaved {
        generation: u64,
    },
    TimeError {
        stage: ErrorStage,
    },
    /// The lifetime statistics (sectors 32 and 33).
    StatsOpen {
        status: StatsStatus,
        generation: u64,
        /// The early saves since the last settled one, and whether a run of
        /// them ever spent the budget, so a boot may have been missed.
        early: u8,
        missed: bool,
    },
    StatsSaved {
        generation: u64,
        early: u8,
    },
    StatsError {
        stage: ErrorStage,
    },
    /// The portion calibration (sectors 34 and 35).
    CalibrationSaved {
        generation: u64,
        portion_mg: Option<u32>,
    },
    CalibrationError {
        stage: ErrorStage,
    },
    /// The feeding plans (sectors 11 and 12).
    PlansSaved {
        sequence: u32,
        revision: u32,
    },
    /// A change of the loop's taken against plans that have moved on:
    /// nothing written, and the loop decides again.
    PlansDropped,
    PlansError {
        stage: ErrorStage,
    },
    /// The meal call (sectors 40-47) after an upload's start or end, or a
    /// delete; a chunk mid-upload logs nothing.
    Call {
        clip: CallClip,
    },
    CallError {
        stage: ErrorStage,
    },
    /// The stored seed and the client store.
    Keys {
        seed: SeedStatus,
        /// The stored seed went into the generator.
        mixed: bool,
        /// The next boot's seed was stored.
        saved: bool,
        clients: ClientStatus,
        enrolled: usize,
        identity: Identity,
    },
    KeysChanged {
        change: KeyChange,
        result: KeyResult,
        enrolled: usize,
    },
    /// The event ring as opened.
    Events {
        open: bool,
        last: Option<u64>,
        oldest: Option<u64>,
        epoch: u32,
    },
    /// An append failed; the record is dropped.
    EventError,
    /// A feed's trace went to the ring (`seq`), or did not (`None`).
    Trace {
        feed: u32,
        seq: Option<u32>,
    },
    /// The boot found the factory reset's marker: no cleared store opens
    /// until the reset has erased them all.
    ResetPending,
    /// The statistics' wear-only save failed, so the reset erased them:
    /// the erase counts are lost, and the reset goes on.
    ResetWearLost,
    /// The handover from stock's activation is done: Wi-Fi, plans and clients
    /// went, unless the build has its own Wi-Fi and keeps its bench key (`cleared`).
    HandoverDone {
        cleared: bool,
    },
    /// The handover cannot go on this boot; the next one tries again.
    HandoverStopped {
        stage: u8,
    },
    /// The factory reset erased every cleared store, then its marker.
    ResetDone {
        erased: u32,
    },
    /// The factory reset stopped for this boot: unarmed, or an erase of
    /// `sector` failed. The marker stays, so the next boot tries again.
    ResetError {
        stage: ErrorStage,
        sector: Option<u32>,
        /// The loop was asked for a restart to try again.
        restart: bool,
    },
}

/// The body of a `PLCFG1` line.
pub fn describe(record: Record, f: &mut impl fmt::Write) -> fmt::Result {
    let values = |f: &mut dyn fmt::Write, s: &Settings| {
        write!(
            f,
            "lights={} sound={} volume={} lights_window={} sound_window={} meal_reminder={} \
             desiccant_days={} desiccant_reset=",
            u8::from(s.lights),
            u8::from(s.sound),
            s.volume,
            s.lights_window,
            s.sound_window,
            s.meal_reminder,
            s.desiccant_days,
        )?;
        match s.desiccant_reset {
            Some(day) => write!(f, "{day}"),
            None => f.write_str("none"),
        }
    };
    match record {
        Record::Open {
            status,
            generation,
            settings,
        } => {
            write!(f, "open status={} generation={generation} ", status.code())?;
            values(f, &settings)
        }
        Record::Arming(arming::Event::Ready { reply, inactive }) => {
            write!(
                f,
                "ready reply={} inactive={}",
                reply.code(),
                u8::from(inactive)
            )
        }
        Record::Arming(arming::Event::Armed {
            watchdog,
            installed,
        }) => write!(
            f,
            "armed watchdog={} installed={}",
            u8::from(watchdog),
            u8::from(installed)
        ),
        Record::Arming(arming::Event::Skip { marker }) => write!(f, "skip marker={marker:#x}"),
        Record::Arming(arming::Event::Timeout) => f.write_str("error stage=timeout"),
        Record::Arming(arming::Event::Clock) => f.write_str("error stage=clock"),
        Record::Saved {
            generation,
            settings,
        } => {
            write!(f, "saved generation={generation} ")?;
            values(f, &settings)
        }
        Record::Error { stage, status } => {
            write!(f, "error stage={stage} status={}", status.code())
        }
        // Never the SSID or passphrase.
        Record::NetOpen {
            status,
            generation,
            stored,
        } => write!(
            f,
            "net open status={} generation={generation} stored={}",
            status as u8,
            u8::from(stored)
        ),
        Record::NetSaved { generation, stored } => write!(
            f,
            "net saved generation={generation} stored={}",
            u8::from(stored)
        ),
        Record::NetError { stage } => write!(f, "net error stage={stage}"),
        Record::TimeOpen { status, generation } => write!(
            f,
            "time open status={} generation={generation}",
            status.name()
        ),
        Record::TimeSaved { generation } => write!(f, "time saved generation={generation}"),
        Record::TimeError { stage } => write!(f, "time error stage={stage}"),
        Record::StatsOpen {
            status,
            generation,
            early,
            missed,
        } => write!(
            f,
            "stats open status={} generation={generation} early={early} missed={}",
            status.name(),
            u8::from(missed)
        ),
        Record::StatsSaved { generation, early } => {
            write!(f, "stats saved generation={generation} early={early}")
        }
        Record::StatsError { stage } => write!(f, "stats error stage={stage}"),
        Record::CalibrationSaved {
            generation,
            portion_mg,
        } => {
            write!(f, "calibration saved generation={generation} portion_mg=")?;
            match portion_mg {
                Some(mg) => write!(f, "{mg}"),
                None => f.write_str("none"),
            }
        }
        Record::CalibrationError { stage } => write!(f, "calibration error stage={stage}"),
        Record::PlansSaved { sequence, revision } => {
            write!(f, "plans saved sequence={sequence} revision={revision}")
        }
        Record::PlansDropped => f.write_str("plans dropped"),
        Record::PlansError { stage } => write!(f, "plans error stage={stage}"),
        Record::Call { clip } => match clip {
            CallClip::None => f.write_str("call none"),
            CallClip::Receiving { bytes, .. } => write!(f, "call receiving bytes={bytes}"),
            CallClip::Stored { bytes, crc } => {
                write!(f, "call stored bytes={bytes} crc={crc:08x}")
            }
        },
        Record::CallError { stage } => write!(f, "call error stage={stage}"),
        Record::Keys {
            seed,
            mixed,
            saved,
            clients,
            enrolled,
            identity,
        } => write!(
            f,
            "keys seed={} mixed={} saved={} clients={} enrolled={enrolled} identity={identity}",
            seed_status(seed),
            u8::from(mixed),
            u8::from(saved),
            client_status(clients),
        ),
        Record::KeysChanged {
            change,
            result,
            enrolled,
        } => write!(
            f,
            "keys change={change} result={result} enrolled={enrolled}"
        ),
        Record::Events {
            open,
            last,
            oldest,
            epoch,
        } => write!(
            f,
            "events open={} last={} oldest={} epoch={epoch:08x}",
            u8::from(open),
            last.unwrap_or(0),
            oldest.unwrap_or(0)
        ),
        Record::EventError => f.write_str("events error stage=append"),
        Record::Trace {
            feed,
            seq: Some(seq),
        } => write!(f, "trace saved feed={feed} seq={seq}"),
        Record::Trace { feed, seq: None } => write!(f, "trace error feed={feed}"),
        Record::ResetPending => f.write_str("reset pending"),
        Record::ResetWearLost => f.write_str("reset wear_lost=1"),
        Record::ResetDone { erased } => write!(f, "reset done erased={erased}"),
        Record::HandoverDone { cleared } => {
            write!(f, "handover done cleared={}", u8::from(cleared))
        }
        Record::HandoverStopped { stage } => write!(f, "handover stopped stage={stage}"),
        Record::ResetError {
            stage,
            sector,
            restart,
        } => {
            write!(f, "reset error stage={stage}")?;
            if let Some(sector) = sector {
                write!(f, " sector={sector}")?;
            }
            write!(f, " restart={}", u8::from(restart))
        }
    }
}

fn seed_status(status: SeedStatus) -> &'static str {
    match status {
        SeedStatus::Unopened => "unopened",
        SeedStatus::Empty => "empty",
        SeedStatus::Loaded => "loaded",
        SeedStatus::ReadOnly => "read-only",
        SeedStatus::Io => "io",
    }
}

fn client_status(status: ClientStatus) -> &'static str {
    match status {
        ClientStatus::Unopened => "unopened",
        ClientStatus::Empty => "empty",
        ClientStatus::Loaded => "loaded",
        ClientStatus::ReadOnly => "read-only",
        ClientStatus::Io => "io",
    }
}

/// Why the writer saves the lifetime totals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SaveTrigger {
    /// The boot's own, as it starts, so its reset counts.
    Boot,
    /// An intentional restart was asked for.
    Restart,
    /// The timed save, every `SAVE_MS` of uptime.
    Due,
}

impl SaveTrigger {
    /// Only the timed save gives an early token back, and a restart's
    /// spends one only early in a boot (`SaveKind::restart`).
    const fn kind(self, uptime_ms: u32) -> SaveKind {
        match self {
            Self::Boot => SaveKind::Early,
            Self::Restart => SaveKind::restart(uptime_ms),
            Self::Due => SaveKind::Settled,
        }
    }
}

/// `delay_ms == 0` ends the task; `armed` reports the arming outcome once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub delay_ms: u32,
    pub record: Option<Record>,
    /// Arming finished this step, armed or not: later writers may start.
    pub settled: bool,
}

fn next(delay_ms: u32, record: Option<Record>) -> Step {
    Step {
        delay_ms,
        record,
        settled: false,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Stage {
    #[default]
    Open,
    Arm,
    Idle,
    /// Read-only for this boot (unarmed or a store it may not write).
    Serve,
}

/// Writes through `F`, the storage region's mediated flash.
pub struct Writer<F> {
    stage: Stage,
    arming: Arming,
    store: SettingsStore<Slots<F, SETTINGS_BASE>>,
    net: NetStore<Slots<F, NET_BASE>>,
    /// The credential store's open record, logged on the next step.
    net_opened: bool,
    time: TimeStore<Slots<F, TIME_BASE>>,
    /// The time store's open record, logged on a later step.
    time_opened: bool,
    /// Opened with the others; its status shows in the device API's
    /// calibration view (writable, generation) rather than a record.
    calibration: CalibrationStore<Slots<F, CALIBRATION_BASE>>,
    /// As for the calibration: its status shows in the plans view.
    plans: PlansStore<Slots<F, PLANS_BASE>>,
    /// Its status shows in the call's view.
    call: CallStore<Region<F, CALL_BASE, CALL_UNITS>>,
    armed: bool,
    seed: SeedStore<Slots<F, SEED_BASE>>,
    clients: ClientStore<Slots<F, CLIENTS_BASE>>,
    keys_opened: bool,
    events: EventLog<Region<F, EVENTS_BASE, EVENTS_UNITS>>,
    events_opened: bool,
    /// A read's records, before they are copied to the loop: static, as
    /// the writer is, since the service task's stack has no room for it.
    read_batch: [ring::Record; events::READ_BATCH],
    /// Polls left at the read pace after the last answered read.
    reading: u32,
    traces: TraceRing<Region<F, TRACES_BASE, TRACES_UNITS>>,
    stats: StatsStore<Slots<F, STATS_BASE>>,
    stats_opened: bool,
    /// The stored totals with this boot's reset counted: what each save
    /// adds this boot's figures to.
    stats_base: Totals,
    /// When the next settled save of the totals is due, in uptime ms;
    /// `None` once a save failed (no retry this boot).
    stats_due: Option<u32>,
    /// The boot's own early save, and the intentional restart's, had their
    /// one attempt.
    stats_boot_saved: bool,
    stats_restart_saved: bool,
    /// This writer's wait for the write turn.
    turn: WriteTurn,
    /// A factory reset this boot finishes: the index (in
    /// `storage::factory_cleared`) of the next sector to erase; past the
    /// last, the statistics' wear-only save, then the marker itself. Until
    /// it is `None` no cleared store opens and nothing is published.
    resetting: Option<u32>,
    /// The reset could not go on this boot (unarmed, or an erase failed).
    reset_stopped: bool,
    /// A handover this boot finishes: the next of its steps. Until it is
    /// `None` nothing is published, so no one sees the Wi-Fi it clears.
    handing_over: Option<u8>,
    /// This boot recorded its attempt in the marker sector.
    reset_attempted: bool,
    /// Arming skipped this boot: the boot before reset inside a write.
    skipped: bool,
}

impl<F> Default for Writer<F> {
    fn default() -> Self {
        Self {
            stage: Stage::default(),
            arming: Arming::default(),
            store: SettingsStore::default(),
            net: NetStore::default(),
            net_opened: false,
            time: TimeStore::default(),
            time_opened: false,
            calibration: CalibrationStore::default(),
            plans: PlansStore::default(),
            call: CallStore::default(),
            armed: false,
            seed: SeedStore::default(),
            clients: ClientStore::default(),
            keys_opened: false,
            events: EventLog::default(),
            events_opened: false,
            read_batch: [EMPTY; events::READ_BATCH],
            reading: 0,
            traces: TraceRing::default(),
            stats: StatsStore::default(),
            stats_opened: false,
            stats_base: Totals::ZERO,
            stats_due: Some(SAVE_MS),
            stats_boot_saved: false,
            stats_restart_saved: false,
            turn: WriteTurn::new(),
            resetting: None,
            reset_stopped: false,
            handing_over: None,
            reset_attempted: false,
            skipped: false,
        }
    }
}

/// The epoch a new event ring takes, drawn after the seed is mixed in and
/// never 0, which a ring written before epochs reads as. A boot without a
/// generator holds no session either (every handshake needs one), so its
/// fallback only has to differ from 0; a later ring re-created with a
/// generator draws again.
fn fresh_epoch() -> u32 {
    let mut bytes = [0; 4];
    let drawn = if rng::fill(&mut bytes) {
        u32::from_le_bytes(bytes)
    } else {
        0
    };
    drawn.max(1)
}

/// What became of the lifetime statistics in a factory reset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wear {
    /// Rewritten with only the erase counts, or nothing to rewrite.
    Kept,
    /// The save failed, so the store was erased: only the erase counts
    /// are lost.
    Lost,
    /// Neither the save nor the erase took.
    Failed,
}

/// The factory reset's step for the lifetime statistics: rewrite them
/// keeping only the erase counts ([`Totals::wear_only`]), as one dual-slot
/// save, so a cut leaves the old totals or the wear-only ones and the boot
/// after does it again. A store with nothing in it, or one this build
/// cannot write (a newer schema), is left as it is.
///
/// The statistics pair is the region's most written, so it may be the one
/// to wear out. A save that fails must not stop the reset for good (every
/// boot after would serve nothing): the pair is erased instead, losing the
/// erase counts the reset meant to keep. A cut part-way leaves a pair the
/// next boot saves or erases again.
fn keep_wear<F: FlashOps>() -> Wear {
    let mut stats = StatsStore::default();
    stats.open(Slots::<F, STATS_BASE>::new());
    if stats.status != StatsStatus::Loaded || !stats.writable() {
        return Wear::Kept;
    }
    let wear = stats.totals.wear_only();
    if stats.totals == wear || stats.save(&wear, SaveKind::Restart) {
        return Wear::Kept;
    }

    let sectors = STATS_BASE..STATS_BASE + wellfed_hal::plaf103::storage::STATS.count;
    if sectors
        .into_iter()
        .all(|sector| F::erase(sector) && blank::<F>(sector))
    {
        Wear::Lost
    } else {
        Wear::Failed
    }
}

/// Whether `sector` reads erased, in small reads to spare the stack: an
/// erase that reported success but left bits behind stops the reset.
fn blank<F: FlashOps>(sector: u32) -> bool {
    const CHUNK: u32 = 64;
    let mut chunk = [0u8; CHUNK as usize];
    (0..sdk::SECTOR_BYTES / CHUNK).all(|i| {
        F::read(sector, i * CHUNK, &mut chunk) && chunk.iter().all(|&byte| byte == u8::MAX)
    })
}

impl<F: FlashOps> Writer<F> {
    /// The write turn for one store write, or `None` while a feed holds the
    /// outputs: the caller then leaves its work queued.
    fn take_turn(&mut self) -> Option<WriteHeld> {
        let stopped = || wellfed_hal::stopped::observe().verified();
        match self.turn.begin(&INTERLOCK, sdk::uptime_ms(), stopped) {
            WriteStep::Go(_) => Some(WriteHeld),
            WriteStep::Wait => None,
        }
    }

    /// Save a queued calibration: the record, or `None` while a feed holds
    /// the outputs (the change stays queued).
    fn save_calibration<H: Hooks>(
        &mut self,
        change: &wellfed_device_api::mailbox::Calibration,
    ) -> Option<Record> {
        let saved = |store: &CalibrationStore<_>| Record::CalibrationSaved {
            generation: store.generation,
            portion_mg: store.calibration.portion_mg,
        };
        if self.stage == Stage::Serve || !H::armed() {
            return Some(Record::CalibrationError {
                stage: ErrorStage::Unarmed,
            });
        }
        if *change == self.calibration.calibration && self.calibration.generation != 0 {
            return Some(saved(&self.calibration));
        }

        let _turn = self.take_turn()?;
        arming::set_attempt();
        let ok = self.calibration.save(change);
        arming::clear_attempt();
        Some(if ok {
            saved(&self.calibration)
        } else {
            Record::CalibrationError {
                stage: ErrorStage::Save,
            }
        })
    }

    /// Apply a queued plans change and save it: the record, or `None`
    /// while a feed holds the outputs (the change stays queued).
    fn save_plans<H: Hooks>(
        &mut self,
        change: &wellfed_device_api::mailbox::PlansChange,
    ) -> Option<Record> {
        if self.stage == Stage::Serve || !H::armed() {
            return Some(Record::PlansError {
                stage: ErrorStage::Unarmed,
            });
        }
        // Checked before the turn, so a dropped change waits for no feed.
        if self.plans.plans.changed(change).is_none() {
            return Some(Record::PlansDropped);
        }

        let _turn = self.take_turn()?;
        arming::set_attempt();
        let ok = self.plans.apply(change) == Some(true);
        arming::clear_attempt();
        Some(if ok {
            Record::PlansSaved {
                sequence: self.plans.sequence,
                revision: self.plans.plans.revision,
            }
        } else {
            Record::PlansError {
                stage: ErrorStage::Save,
            }
        })
    }

    /// Apply the call's queued work, or say why it waits (it stays
    /// queued).
    fn save_call<H: Hooks>(&mut self, queued: Queued, data: &[u8]) -> CallSave {
        if self.stage == Stage::Serve || !H::armed() {
            return CallSave::Done(Some(Record::CallError {
                stage: ErrorStage::Unarmed,
            }));
        }
        // The sectors first: a call that plays reads them.
        if !audio::call_write_begin() {
            return CallSave::Held;
        }
        let Some(_turn) = self.take_turn() else {
            audio::call_write_end();
            return CallSave::Turn;
        };

        audio::publish_call(None);
        arming::set_attempt();
        let ok = match queued {
            Queued::Change(CallChange::Start { bytes, crc }) => self.call.start(bytes, crc),
            Queued::Change(CallChange::Delete) => self.call.delete(),
            Queued::Chunk { offset, .. } => self.call.chunk(offset, data),
        };
        arming::clear_attempt();
        audio::publish_call(self.call.stored());
        audio::call_write_end();

        let clip = self.call.clip;
        CallSave::Done(if !ok {
            Some(Record::CallError {
                stage: ErrorStage::Save,
            })
        } else if matches!(
            (queued, clip),
            (Queued::Chunk { .. }, CallClip::Receiving { .. })
        ) {
            None
        } else {
            Some(Record::Call { clip })
        })
    }

    /// The step while a write waits for its turn: the event ring's readers
    /// are still answered, since a read takes no lease.
    fn waiting(&mut self) -> Step {
        let delay = if self.answer_read() {
            EVENTS_POLL_MS
        } else {
            WRITE_WAIT_POLL_MS
        };
        next(delay, None)
    }

    /// Answer the read the device API asked for, if any; true while a
    /// reader is about, so the next poll comes at the read pace. A reader
    /// catching up asks again within a few loop passes of its last batch,
    /// and the idle poll would make it wait [`POLL_MS`] for each.
    fn answer_read(&mut self) -> bool {
        let Some(after) = events::read_request() else {
            self.reading = self.reading.saturating_sub(1);
            return self.reading > 0;
        };
        let batch = self.events.read_batch(after, &mut self.read_batch);
        events::finish_read(after, batch, &self.read_batch);
        self.reading = READ_LINGER_POLLS;
        true
    }

    /// The keys: mix the stored seed in and store the next, then open
    /// the client store and create the identity if it has none. Writes only
    /// when `writable` (armed, normal stage), each under its own attempt
    /// marker.
    fn keys(&mut self, writable: bool) -> Record {
        self.seed.open(Slots::new());
        let mixed = self.seed.seed().is_some_and(|seed| rng::mix(seed));
        let saved = writable
            && rng::bytes().is_some_and(|next| {
                arming::set_attempt();
                let saved = self.seed.save(&next);
                arming::clear_attempt();
                saved
            });

        self.clients.open(Slots::new());
        let identity = if self.clients.identity().is_some() {
            Identity::Loaded
        } else if let Some(secret) = rng::bytes().filter(|_| writable && self.clients.writable()) {
            arming::set_attempt();
            let created = self.clients.set_identity(&secret).is_ok();
            arming::clear_attempt();
            if created {
                Identity::Created
            } else {
                Identity::Failed
            }
        } else {
            Identity::Missing
        };
        if let Some(secret) = self.clients.identity() {
            keys::publish_identity(secret);
        }
        keys::publish_clients(self.clients.clients().copied());

        Record::Keys {
            seed: self.seed.status,
            mixed,
            saved,
            clients: self.clients.status,
            enrolled: self.clients.clients().count(),
            identity,
        }
    }

    /// Apply one queued client change, each write under its own attempt
    /// marker, and free the mailbox with its result.
    ///
    /// A reset is the factory reset. It draws its new identity from the
    /// generator or does not happen: a weaker key is never written. It
    /// programs the reset's marker first, so whatever happens next the boot
    /// after finishes it, then forgets every client at once (their sessions
    /// end now, not at the restart). Once the marker is down the answer is
    /// `Done`: the loop restarts the feeder, and that boot erases the
    /// cleared stores.
    fn change(&mut self, change: Change, writable: bool) -> Record {
        let kind = match change {
            Change::Enroll { .. } => KeyChange::Enroll,
            Change::Revoke { .. } => KeyChange::Revoke,
            Change::Reset => KeyChange::Reset,
        };
        let result = match change {
            _ if !writable => KeyResult::Unarmed,
            Change::Enroll { key, label, role } => {
                self.apply(|clients| clients.enroll(key, label, role))
            }
            Change::Revoke { key } => self.apply(|clients| clients.revoke(&key)),
            Change::Reset => match rng::bytes() {
                Some(identity) => {
                    arming::set_attempt();
                    let marked = reset_marker::mark(&Region::<F, RESET_BASE, 1>::new());
                    arming::clear_attempt();
                    if marked {
                        // Committed: the next boot erases the client store
                        // whatever this write does, so a store it cannot
                        // write (read-only, torn) still answers `Done` and
                        // the loop restarts. A marker left down unanswered
                        // would clear everything on some later, unrelated
                        // boot.
                        self.apply(|clients| clients.reset(&identity));
                        KeyResult::Done
                    } else {
                        KeyResult::Io
                    }
                }
                None => KeyResult::NoGenerator,
            },
        };

        let reset = kind == KeyChange::Reset && result == KeyResult::Done;
        if reset && let Some(secret) = self.clients.identity() {
            keys::publish_identity(secret);
        }
        if reset {
            // No session outlives a reset, even one the store kept.
            keys::publish_clients(core::iter::empty());
        } else {
            keys::publish_clients(self.clients.clients().copied());
        }
        keys::finish(result);
        Record::KeysChanged {
            change: kind,
            result,
            enrolled: self.clients.clients().count(),
        }
    }

    fn apply(
        &mut self,
        write: impl FnOnce(&mut ClientStore<Slots<F, CLIENTS_BASE>>) -> Result<(), ClientError>,
    ) -> KeyResult {
        arming::set_attempt();
        let result = write(&mut self.clients);
        arming::clear_attempt();
        result.map_or_else(KeyResult::from, |()| KeyResult::Done)
    }

    /// Append one queued event, or answer one read; the faster poll while
    /// either has more waiting.
    fn events_step(&mut self, writable: bool) -> Step {
        let mut record = None;
        let mut deferred = false;
        if let Some(queued) = events::pending() {
            if !writable {
                events::finish_pending(self.events.last_seq());
            } else if let Some(_turn) = self.take_turn() {
                arming::set_attempt();
                let appended = self.events.append(queued.at, &queued.body);
                arming::clear_attempt();
                if appended.is_err() {
                    record = Some(Record::EventError);
                }
                events::finish_pending(self.events.last_seq());
                self.publish_fill();
            } else {
                // Stays at the queue's head, so the records keep their order.
                deferred = true;
            }
        }
        // A reader catching up asks again as soon as it has the answer.
        let read = self.answer_read();

        let more = (events::pending().is_some() && !deferred) || read;
        next(if more { EVENTS_POLL_MS } else { POLL_MS }, record)
    }

    /// Append the sealed trace, if one waits: under the write turn, which a
    /// feed's claim holds until its outputs are stopped. One that cannot be
    /// written (unarmed, read-only) is let go at once, so the next feed's
    /// claim never waits for it.
    fn trace_step(&mut self, writable: bool) -> Option<Step> {
        if !traces::queued() {
            return None;
        }
        let turn = if writable {
            let Some(turn) = self.take_turn() else {
                return Some(self.waiting());
            };
            Some(turn)
        } else {
            None
        };

        let ring = &mut self.traces;
        let mut seq = None;
        let written = traces::write_sealed(|summary, body| {
            if turn.is_none() {
                return false;
            }
            arming::set_attempt();
            seq = ring.append(summary, body).ok();
            arming::clear_attempt();
            seq.is_some()
        });
        drop(turn);
        traces::publish_fill(self.traces.kept(), self.traces.capacity());
        written.map(|(feed, _)| next(POLL_MS, Some(Record::Trace { feed, seq })))
    }

    /// Open the stats store, count this boot's reset and publish the
    /// totals: none from a store this build cannot write.
    fn open_stats(&mut self) -> Record {
        self.stats.open(Slots::new());
        let mut base = self.stats.totals;
        base.count_boot(reset_cause(sdk::boot_reason()));
        self.stats_base = base;
        stats::publish_totals(self.stats.writable().then_some(base));
        Record::StatsOpen {
            status: self.stats.status,
            generation: self.stats.generation,
            early: self.stats.budget.early,
            missed: self.stats.budget.missed,
        }
    }

    /// The save of the totals wanted now, if any: the boot's own as it
    /// starts, an intentional restart's (in its grace, before the feeding
    /// stops), or the timed one. A trigger the store refuses (no early token
    /// left, or a store it cannot write) is dropped for the boot.
    fn stats_wanted(&mut self) -> Option<SaveTrigger> {
        let now = sdk::uptime_ms();
        let trigger = if !self.stats_boot_saved {
            SaveTrigger::Boot
        } else if supervision::restart_grace() && !self.stats_restart_saved {
            SaveTrigger::Restart
        } else if self
            .stats_due
            .is_some_and(|due| wellfed_core::deadline_passed(now, due))
        {
            SaveTrigger::Due
        } else {
            return None;
        };

        if self.stats.may_save(trigger.kind(now)) {
            return Some(trigger);
        }
        self.stats_done(trigger);
        None
    }

    /// `trigger` has had its one attempt.
    fn stats_done(&mut self, trigger: SaveTrigger) {
        match trigger {
            SaveTrigger::Boot => self.stats_boot_saved = true,
            SaveTrigger::Restart => {
                self.stats_restart_saved = true;
                // Saved or not, the restart need not wait any longer.
                supervision::restart_flushed();
            }
            SaveTrigger::Due => {}
        }
    }

    /// Save the totals: the stored ones with this boot's counts, as the
    /// loop last handed them, and this boot's erases, the save's own
    /// included. Only on a turn with the outputs released: the turn is
    /// asked for as never stopped, so a claim past its bound, which lets
    /// another store's write go ahead, keeps this one waiting. A restart
    /// waits for it only through its grace; once the feeding stops no turn
    /// is granted, so no save starts into the reset.
    fn save_stats(&mut self, trigger: SaveTrigger) -> Step {
        let now = sdk::uptime_ms();
        let WriteStep::Go(_) = self.turn.begin(&INTERLOCK, now, || false) else {
            return self.waiting();
        };
        let turn = WriteHeld;

        let mut erases = [0; TOTAL_SECTORS];
        for (sector, count) in (0u32..).zip(erases.iter_mut()) {
            *count = sdk::storage_erase_count(sector);
        }
        let own = STATS_BASE + self.stats.next_slot();
        if let Some(count) = usize::try_from(own)
            .ok()
            .and_then(|own| erases.get_mut(own))
        {
            *count = count.saturating_add(1);
        }
        let boot = stats::counts().unwrap_or_default();
        let totals = self
            .stats_base
            .plus(&boot, &erases, sdk::bank1_erase_count());

        let kind = trigger.kind(now);
        arming::set_attempt();
        let saved = self.stats.save(&totals, kind);
        arming::clear_attempt();
        drop(turn);

        self.stats_done(trigger);
        let record = if saved {
            if kind == SaveKind::Settled {
                self.stats_due = Some(now.wrapping_add(SAVE_MS));
            }
            Record::StatsSaved {
                generation: self.stats.generation,
                early: self.stats.budget.early,
            }
        } else {
            self.stats_due = None;
            Record::StatsError {
                stage: ErrorStage::Save,
            }
        };
        next(POLL_MS, Some(record))
    }

    /// Open the stores the Open stage serves that a factory reset clears;
    /// the settings store's status.
    fn open_cleared(&mut self) -> SettingsStatus {
        let status = self.store.open(Slots::new(), SETTINGS_BINDING);
        self.net.open(Slots::new());
        self.time.open(Slots::new());
        self.plans.open(Slots::new());
        self.call.open(Region::new());
        status
    }

    /// One step of the handover: the Wi-Fi's two sectors, the plans' two, the
    /// clients, the update store's floor, the boot confirmation, then the mark,
    /// each under its own write turn and attempt marker. A cut anywhere repeats
    /// it, since the mark goes last. A build with Wi-Fi of its own is a bench
    /// image, which pairs through the recovery window rather than Bluetooth, so
    /// it keeps its Wi-Fi, plans and clients, but any image installed from stock
    /// has to take the same version again, and earn its floor with a real trial.
    ///
    /// Only bank 0 forgets the confirmation. KM0 never reads it for bank 0, so
    /// a cut there changes nothing about the boot. On bank 1 this build's next
    /// boot would run a trial, with the Wi-Fi it needs to pass just cleared,
    /// and give way. A step that fails leaves the mark for the next boot, and
    /// this one serves what it has.
    fn handover_step<H: Hooks>(&mut self) -> Step {
        let Some(step) = self.handing_over else {
            return next(POLL_MS, None);
        };
        if self.stage == Stage::Serve || !H::armed() {
            return self.handover_end(Record::HandoverStopped { stage: step });
        }
        let Some(turn) = self.take_turn() else {
            return next(WRITE_WAIT_POLL_MS, None);
        };

        let clears = !cfg!(build_wifi);
        arming::set_attempt();
        let done = match step {
            0..=4 if !clears => true,
            0 | 1 => {
                let sector = NET_BASE + u32::from(step);
                F::erase(sector) && blank::<F>(sector)
            }
            2 | 3 => {
                let sector = PLANS_BASE + u32::from(step - 2);
                F::erase(sector) && blank::<F>(sector)
            }
            4 => {
                self.clients.open(Slots::new());
                match self.clients.identity().copied() {
                    Some(identity) => self.clients.reset(&identity).is_ok(),
                    None => true,
                }
            }
            5 => {
                let mut update = UpdateStore::default();
                update.open(Region::<F, UPDATE_BASE, 2>::new());
                let started = match update.status {
                    // A newer schema's floor is not this build's to touch.
                    UpdateStatus::ReadOnly => true,
                    _ => update.start_over().is_ok(),
                };
                if started {
                    wellfed_hal::ota::update::store_started_over();
                }
                started
            }
            6 if sdk::running_bank() != Some(0) => true,
            6 => F::erase(CONFIRM_SECTOR) && blank::<F>(CONFIRM_SECTOR),
            _ => F::erase(RESET_BASE) && blank::<F>(RESET_BASE),
        };
        arming::clear_attempt();
        drop(turn);

        if !done {
            return self.handover_end(Record::HandoverStopped { stage: step });
        }
        if step < 7 {
            self.handing_over = Some(step + 1);
            return next(RESET_POLL_MS, None);
        }
        self.handover_end(Record::HandoverDone { cleared: clears })
    }

    fn handover_end(&mut self, record: Record) -> Step {
        self.handing_over = None;
        if matches!(record, Record::HandoverStopped { .. }) {
            network::handover_put_off();
        }
        self.net = NetStore::default();
        self.net.open(Slots::new());
        self.plans = PlansStore::default();
        self.plans.open(Slots::new());
        self.publish();
        next(POLL_MS, Some(record))
    }

    /// One step of a factory reset this boot finishes: one erase, under
    /// its own write turn (so it waits for a feed's outputs to stop) and
    /// attempt marker; then, after the marker's, the cleared stores open
    /// empty and everything is published. A reset that cannot go on stops
    /// for the boot with nothing served, and the marker brings it back on
    /// the next.
    fn reset_step<H: Hooks>(&mut self) -> Step {
        if self.reset_stopped {
            if supervision::restart_grace() {
                // Nothing to save before a restart.
                supervision::restart_flushed();
            }
            return next(POLL_MS, None);
        }
        if self.stage == Stage::Serve || !H::armed() {
            return self.reset_stop(ErrorStage::Unarmed, None);
        }
        let Some(index) = self.resetting else {
            return next(POLL_MS, None);
        };
        let Some(turn) = self.take_turn() else {
            return next(WRITE_WAIT_POLL_MS, None);
        };

        let marker = Region::<F, RESET_BASE, 1>::new();
        arming::set_attempt();
        let noted = self.reset_attempted || reset_marker::note_attempt(&marker);
        let mut wear = Wear::Kept;
        let step = match factory_cleared(index) {
            _ if !noted => None,
            Some(sector) => Some((sector, F::erase(sector) && blank::<F>(sector))),
            None if index == FACTORY_CLEARED_SECTORS => {
                wear = keep_wear::<F>();
                Some((STATS_BASE, wear != Wear::Failed))
            }
            None => Some((RESET_BASE, F::erase(RESET_BASE) && blank::<F>(RESET_BASE))),
        };
        arming::clear_attempt();
        drop(turn);
        self.reset_attempted = noted;

        // An attempt that did not land is a store this boot cannot count
        // in, so it stops without asking for a restart; so do statistics
        // that took neither their save nor their erase (an erase elsewhere
        // that failed may be retried).
        let Some((sector, done)) = step else {
            return self.reset_stop(ErrorStage::Save, Some(RESET_BASE));
        };
        if !done {
            let stage = if sector == STATS_BASE {
                ErrorStage::Save
            } else {
                ErrorStage::Erase
            };
            return self.reset_stop(stage, Some(sector));
        }
        if index <= FACTORY_CLEARED_SECTORS {
            self.resetting = Some(index + 1);
            let record = (wear == Wear::Lost).then_some(Record::ResetWearLost);
            return next(RESET_POLL_MS, record);
        }
        self.resetting = None;
        self.open_cleared();
        self.publish();
        let record = Record::ResetDone {
            erased: FACTORY_CLEARED_SECTORS,
        };
        next(POLL_MS, Some(record))
    }

    /// The reset cannot go on this boot. A boot that serves nothing has no
    /// identity, API, recovery window or BLE setup, so it may ask the loop
    /// for a restart (the loop waits for a feed in flight), but only once
    /// per reset: while fewer than `reset::ATTEMPTS` armed boots have
    /// started the erases (counted in the marker sector, since a boot's
    /// reset flags cannot tell a hang from an intentional restart), and only
    /// when a restart can help. That is a failed erase, or an arming skip:
    /// the boot before hung inside a write, the skip has cleared the
    /// attempt marker, and the restart arms again. Arming that failed for
    /// any other reason would fail again, so that boot waits for a power
    /// cycle.
    fn reset_stop(&mut self, stage: ErrorStage, sector: Option<u32>) -> Step {
        self.reset_stopped = true;
        let helps = stage == ErrorStage::Erase || self.skipped;
        let marker = Region::<F, RESET_BASE, 1>::new();
        let restart = helps && reset_marker::attempts(&marker) < reset_marker::ATTEMPTS;
        if restart {
            keys::want_restart();
        }
        let record = Record::ResetError {
            stage,
            sector,
            restart,
        };
        next(POLL_MS, Some(record))
    }

    fn publish_fill(&self) {
        stats::publish_fill(EventsFill {
            kept: self.events.kept(),
            capacity: self.events.capacity(),
        });
    }

    /// Publish every store's view; nothing while a factory reset is
    /// unfinished, so no client, plan or credential it clears is served.
    fn publish(&self) {
        if self.resetting.is_some() || self.handing_over.is_some() {
            return;
        }
        let writable = self.armed
            && matches!(
                self.store.status,
                SettingsStatus::Loaded | SettingsStatus::Empty | SettingsStatus::Recovered
            );
        settings::publish(self.store.settings.view(self.store.generation, writable));
        network::publish(&self.net, self.armed);
        time::publish(&self.time, self.armed);
        calibration::publish(&self.calibration, self.armed);
        plans::publish(&self.plans, self.armed);
        call::publish(&self.call, self.armed);
    }

    /// One task iteration.
    ///
    /// # Safety
    /// `provider` is valid for the program's lifetime, stays at a fixed
    /// address and is not otherwise borrowed.
    pub unsafe fn step<M: Mmio, H: Hooks>(&mut self, provider: *mut Provider) -> Step {
        match self.stage {
            Stage::Open => {
                self.calibration.open(Slots::new());
                let record = if reset_marker::pending(&Region::<F, RESET_BASE, 1>::new()) {
                    self.resetting = Some(0);
                    Record::ResetPending
                } else {
                    if handover::pending(&Region::<F, RESET_BASE, 1>::new()) {
                        self.handing_over = Some(0);
                    }
                    let status = self.open_cleared();
                    Record::Open {
                        status,
                        generation: self.store.generation,
                        settings: self.store.settings,
                    }
                };
                self.armed = H::armed();
                self.stage = if self.armed { Stage::Idle } else { Stage::Arm };
                self.publish();
                let mut step = next(POLL_MS, Some(record));
                step.settled = self.armed;
                step
            }

            // SAFETY: the caller's contract.
            Stage::Arm => match unsafe { self.arming.step::<M, H>(provider) } {
                Poll::Pending(event) => next(arming::POLL_MS, event.map(Record::Arming)),
                Poll::Armed(event) => {
                    // Armed for the rest of the boot; each save marks its own attempt.
                    arming::clear_attempt();
                    self.armed = H::armed();
                    self.stage = Stage::Idle;
                    self.publish();
                    Step {
                        delay_ms: POLL_MS,
                        record: Some(Record::Arming(event)),
                        settled: true,
                    }
                }
                Poll::Failed(event) => {
                    self.skipped = matches!(event, arming::Event::Skip { .. });
                    self.stage = Stage::Serve;
                    self.publish();
                    Step {
                        delay_ms: POLL_MS,
                        record: Some(Record::Arming(event)),
                        settled: true,
                    }
                }
            },

            Stage::Idle | Stage::Serve => {
                // Before the keys, the event ring and the totals open: each
                // is a store the reset clears.
                if self.resetting.is_some() {
                    return self.reset_step::<H>();
                }
                if self.handing_over.is_some() {
                    return self.handover_step::<H>();
                }
                if !self.net_opened {
                    self.net_opened = true;
                    return next(
                        POLL_MS,
                        Some(Record::NetOpen {
                            status: self.net.status,
                            generation: self.net.generation,
                            stored: self.net.credentials.is_some(),
                        }),
                    );
                }
                if !self.time_opened {
                    self.time_opened = true;
                    return next(
                        POLL_MS,
                        Some(Record::TimeOpen {
                            status: self.time.status,
                            generation: self.time.generation,
                        }),
                    );
                }
                if !self.keys_opened {
                    let writable = self.stage == Stage::Idle && H::armed();
                    // The seed and, on a first boot, the identity: one turn.
                    let turn = if writable {
                        let Some(turn) = self.take_turn() else {
                            return self.waiting();
                        };
                        Some(turn)
                    } else {
                        None
                    };
                    self.keys_opened = true;
                    let record = self.keys(writable);
                    drop(turn);
                    return next(POLL_MS, Some(record));
                }
                if !self.events_opened {
                    self.events_opened = true;
                    // The trace ring opens with the event ring, unlogged:
                    // each trace it takes logs its own line.
                    self.traces.open(Region::new(), TRACES_UNITS as usize);
                    traces::publish_fill(self.traces.kept(), self.traces.capacity());
                    self.events
                        .open(Region::new(), EVENTS_UNITS as usize, fresh_epoch());
                    events::publish_open(self.events.epoch(), self.events.last_seq());
                    self.publish_fill();
                    let record = Record::Events {
                        open: self.events.is_open(),
                        last: self.events.last_seq(),
                        oldest: self.events.oldest_seq(),
                        epoch: self.events.epoch(),
                    };
                    return next(POLL_MS, Some(record));
                }
                if !self.stats_opened {
                    self.stats_opened = true;
                    let record = self.open_stats();
                    return next(POLL_MS, Some(record));
                }
                if let Some(change) = keys::pending() {
                    let writable = self.stage == Stage::Idle && H::armed();
                    let turn = if writable {
                        let Some(turn) = self.take_turn() else {
                            return self.waiting();
                        };
                        Some(turn)
                    } else {
                        None
                    };
                    let record = self.change(change, writable);
                    drop(turn);
                    return next(POLL_MS, Some(record));
                }
                if let Some(credentials) = network::pending() {
                    let record = if self.stage == Stage::Serve || !H::armed() {
                        Record::NetError {
                            stage: ErrorStage::Unarmed,
                        }
                    } else {
                        let Some(_turn) = self.take_turn() else {
                            return self.waiting();
                        };
                        arming::set_attempt();
                        let saved = self.net.save(credentials.as_ref());
                        arming::clear_attempt();
                        if saved {
                            Record::NetSaved {
                                generation: self.net.generation,
                                stored: self.net.credentials.is_some(),
                            }
                        } else {
                            Record::NetError {
                                stage: ErrorStage::Save,
                            }
                        }
                    };
                    self.publish();
                    network::finish();
                    return next(POLL_MS, Some(record));
                }
                if let Some(settings) = time::pending() {
                    let record = if self.stage == Stage::Serve || !H::armed() {
                        Record::TimeError {
                            stage: ErrorStage::Unarmed,
                        }
                    } else if settings == self.time.settings && self.time.generation != 0 {
                        Record::TimeSaved {
                            generation: self.time.generation,
                        }
                    } else {
                        let Some(_turn) = self.take_turn() else {
                            return self.waiting();
                        };
                        arming::set_attempt();
                        let saved = self.time.save(&settings);
                        arming::clear_attempt();
                        if saved {
                            Record::TimeSaved {
                                generation: self.time.generation,
                            }
                        } else {
                            Record::TimeError {
                                stage: ErrorStage::Save,
                            }
                        }
                    };
                    self.publish();
                    time::finish();
                    return next(POLL_MS, Some(record));
                }
                if let Some(change) = calibration::pending() {
                    let record = self.save_calibration::<H>(&change);
                    let Some(record) = record else {
                        return self.waiting();
                    };
                    self.publish();
                    calibration::finish();
                    return next(POLL_MS, Some(record));
                }
                if let Some(change) = plans::pending() {
                    let Some(record) = self.save_plans::<H>(&change) else {
                        return self.waiting();
                    };
                    self.publish();
                    plans::finish();
                    return next(POLL_MS, Some(record));
                }
                // The chunk's bytes are copied out of the mailbox only as the
                // flash takes them.
                match call::with_pending(|queued, data| self.save_call::<H>(queued, data)) {
                    Some(CallSave::Done(record)) => {
                        self.publish();
                        call::finish();
                        // An upload's next chunk follows at once.
                        return next(EVENTS_POLL_MS, record);
                    }
                    Some(CallSave::Turn) => return self.waiting(),
                    // A call plays from the clip for seconds: the work below
                    // goes on meanwhile, and the change is tried each step.
                    Some(CallSave::Held) | None => {}
                }
                let Some(fields) = settings::pending() else {
                    let writable = self.stage == Stage::Idle && H::armed();
                    if let Some(step) = self.trace_step(writable) {
                        return step;
                    }
                    // The totals go last, behind every queued record.
                    if writable
                        && events::pending().is_none()
                        && let Some(trigger) = self.stats_wanted()
                    {
                        return self.save_stats(trigger);
                    }
                    if !writable && supervision::restart_grace() {
                        // Nothing to save before a restart.
                        supervision::restart_flushed();
                    }
                    return self.events_step(writable);
                };
                let target = self.store.settings.changed(fields);
                let record = if self.stage == Stage::Serve || !H::armed() {
                    Record::Error {
                        stage: ErrorStage::Unarmed,
                        status: self.store.status,
                    }
                } else if target == self.store.settings {
                    Record::Saved {
                        generation: self.store.generation,
                        settings: target,
                    }
                } else {
                    let Some(_turn) = self.take_turn() else {
                        return self.waiting();
                    };
                    arming::set_attempt();
                    let saved = self.store.save(&target);
                    arming::clear_attempt();
                    if saved {
                        Record::Saved {
                            generation: self.store.generation,
                            settings: self.store.settings,
                        }
                    } else {
                        Record::Error {
                            stage: ErrorStage::Save,
                            status: self.store.status,
                        }
                    }
                };
                self.publish();
                settings::finish();
                next(POLL_MS, Some(record))
            }
        }
    }
}

/// The service task's settings stage; only `wellfed_store_step` enters it.
#[cfg(target_os = "none")]
static WRITER: wellfed_sdk::Slot<Writer<wellfed_sdk::SdkStorage>> = wellfed_sdk::Slot::new();

#[cfg(target_os = "none")]
struct Line(Record);

#[cfg(target_os = "none")]
impl fmt::Display for Line {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        describe(self.0, f)
    }
}

/// The SDK runtime's settings stage: one writer step, logged as a `PLCFG1`
/// record; the delay to the next, 0 to end. Its signature must match the
/// runtime's `extern "Rust"` declaration in `wellfed-sdk`'s `runtime.rs`
/// (the linker does not check it); the assertion below pins it.
///
/// # Safety
/// Called only from the service task, with the runtime's fixed provider,
/// never otherwise borrowed.
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub unsafe fn wellfed_store_step(provider: *mut Provider) -> u32 {
    use wellfed_hal::flash::park::Hardware;
    use wellfed_hal::startup;

    let Some(mut writer) = WRITER.enter_or_init(Default::default) else {
        return 0;
    };
    // SAFETY: the caller's contract.
    let step = unsafe { writer.step::<Hardware, wellfed_sdk::SdkHooks>(provider) };

    if let Some(record) = step.record {
        let mut report = startup::Report::new();
        if let Ok(line) = report.record(format_args!("PLCFG1 {}\r\n", Line(record))) {
            sdk::log(line);
        }
    }
    step.delay_ms
}

#[cfg(target_os = "none")]
const _: unsafe fn(*mut Provider) -> u32 = wellfed_store_step;
