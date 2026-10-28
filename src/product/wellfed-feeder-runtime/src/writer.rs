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
//! Reset, which forgets every client and draws a new identity.
//!
//! It keeps the time settings too (sectors 27 and 28,
//! `wellfed_feeder::store::time`): the owner's zone and the NTP server,
//! published for the loop ([`time`]), the portion calibration (sectors 34
//! and 35, `wellfed_feeder::store::calibration`, [`calibration`]), and the
//! feeding plans (sectors 11 and 12, `wellfed_feeder::store::plans`,
//! [`plans`]): a client's table, and the loop's claims and passes.
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

use wellfed_core::store::seed::{SeedStatus, SeedStore};
use wellfed_feeder::stats::{SAVE_MS, TOTAL_SECTORS, Totals, reset_cause};
use wellfed_feeder::store::calibration::CalibrationStore;
use wellfed_feeder::store::clients::{ClientError, ClientStatus, ClientStore};
use wellfed_feeder::store::events::EventLog;
use wellfed_feeder::store::net::{NetStatus, NetStore};
use wellfed_feeder::store::plans::PlansStore;
use wellfed_feeder::store::settings::{Settings, SettingsStatus, SettingsStore};
use wellfed_feeder::store::stats::{SaveKind, StatsStatus, StatsStore};
use wellfed_feeder::store::time::{TimeStatus, TimeStore};
use wellfed_feeder::store::traces::TraceRing;
use wellfed_hal::flash::park::Mmio;
use wellfed_hal::flash_lock::arming::{self, Arming, Hooks, Poll};
use wellfed_hal::flash_lock::provider::Provider;
use wellfed_hal::interlock::{INTERLOCK, WRITER_POLL_MS, WriteStep, WriteTurn};
use wellfed_hal::{rng, sdk, settings, supervision};
use wellfed_sdk::storage::{
    CLIENTS_BASE, EVENTS_BASE, EVENTS_UNITS, FlashOps, Region, SEED_BASE, SETTINGS_BASE,
    SETTINGS_BINDING, Slots, TRACES_BASE, TRACES_UNITS,
};

use crate::calibration::{self, CALIBRATION_BASE};
use crate::events;
pub use crate::keys::KeyResult;
use crate::keys::{self, Change};
use crate::network::{self, NET_BASE};
use crate::plans::{self, PLANS_BASE};
use crate::stats::{self, STATS_BASE};
use crate::time::{self, TIME_BASE};
use crate::traces;
use wellfed_feeder::feeder::platform::EventsFill;

pub const POLL_MS: u32 = 200;
/// While events wait to be written or read.
pub const EVENTS_POLL_MS: u32 = 5;
/// While a write waits for a feed to end: within the poll a claim allows a
/// waiting writer before passing it.
pub const WRITE_WAIT_POLL_MS: u32 = POLL_MS;
const _: () = assert!(WRITE_WAIT_POLL_MS <= WRITER_POLL_MS);

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
    armed: bool,
    seed: SeedStore<Slots<F, SEED_BASE>>,
    clients: ClientStore<Slots<F, CLIENTS_BASE>>,
    keys_opened: bool,
    events: EventLog<Region<F, EVENTS_BASE, EVENTS_UNITS>>,
    events_opened: bool,
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
            armed: false,
            seed: SeedStore::default(),
            clients: ClientStore::default(),
            keys_opened: false,
            events: EventLog::default(),
            events_opened: false,
            traces: TraceRing::default(),
            stats: StatsStore::default(),
            stats_opened: false,
            stats_base: Totals::ZERO,
            stats_due: Some(SAVE_MS),
            stats_boot_saved: false,
            stats_restart_saved: false,
            turn: WriteTurn::new(),
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

    /// The step while a write waits for its turn: the event ring's readers
    /// are still answered, since a read takes no lease.
    fn waiting(&self) -> Step {
        let delay = if self.answer_read() {
            EVENTS_POLL_MS
        } else {
            WRITE_WAIT_POLL_MS
        };
        next(delay, None)
    }

    /// Answer the read the device API asked for, if any.
    fn answer_read(&self) -> bool {
        let Some(after) = events::read_request() else {
            return false;
        };
        events::finish_read(after, self.events.read_after(after));
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
    /// marker, and free the mailbox with its result. A reset draws its new identity from the generator or does not
    /// happen: a weaker key is never written.
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
                Some(identity) => self.apply(|clients| clients.reset(&identity)),
                None => KeyResult::NoGenerator,
            },
        };

        if kind == KeyChange::Reset
            && result == KeyResult::Done
            && let Some(secret) = self.clients.identity()
        {
            keys::publish_identity(secret);
        }
        keys::publish_clients(self.clients.clients().copied());
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

    fn publish_fill(&self) {
        stats::publish_fill(EventsFill {
            kept: self.events.kept(),
            capacity: self.events.capacity(),
        });
    }

    fn publish(&self) {
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
    }

    /// One task iteration.
    ///
    /// # Safety
    /// `provider` is valid for the program's lifetime, stays at a fixed
    /// address and is not otherwise borrowed.
    pub unsafe fn step<M: Mmio, H: Hooks>(&mut self, provider: *mut Provider) -> Step {
        match self.stage {
            Stage::Open => {
                let status = self.store.open(Slots::new(), SETTINGS_BINDING);
                self.net.open(Slots::new());
                self.time.open(Slots::new());
                self.calibration.open(Slots::new());
                self.plans.open(Slots::new());
                self.armed = H::armed();
                self.stage = if self.armed { Stage::Idle } else { Stage::Arm };
                self.publish();
                let mut step = next(
                    POLL_MS,
                    Some(Record::Open {
                        status,
                        generation: self.store.generation,
                        settings: self.store.settings,
                    }),
                );
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
