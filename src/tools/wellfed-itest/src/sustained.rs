//! `sustained`: the defined unattended long run. Half-hour cycles, each
//! opening with one fault from a fixed rotation of eight, then feeding the
//! baseline every 10 minutes while the cycle lasts, with the device API
//! polled every 30 s in between. The default 16 cycles are 8 hours, each
//! fault twice.
//!
//! The motion rules are the owner's for unattended runs: the hopper empty,
//! the D cells out so relay 1 is the kill switch, and at most 3 portions a
//! feed. Every feed is asked for once and never resent; a cut, a stall or
//! a lost answer leaves it to the feeder to resolve, and the run checks
//! that it did, once.
//!
//! Every check is a halt: any failure, the operator's interrupt included,
//! cuts relay 1 and leaves it off, so a feeder the run can no longer
//! account for stops moving and stays stopped until someone looks.
//!
//! The run is evidence only as a whole: the UART captures, the record, and
//! the notes this module writes at each cycle's end.

use std::collections::BTreeSet;
use std::time::Duration;

use wellfed_core::feed::controller::FeedReason;
use wellfed_core::feed::owner::OwnerState;
use wellfed_core::feed::storage::OwnerOutcome;
use wellfed_core::record::{Event, Owner, Trial};
use wellfed_formats::require;
use wellfed_model::{Counters, Counts, Memory, Refusal, ResetCause, Stats, StatsSection, TimeView};
use wellfed_rig::commands::ota::{Outcome, Staged};
use wellfed_rig::commands::{Firmware, Image, Variant};
use wellfed_rig::device::{Answer, Ask, DeviceStatus};

use super::feeds::{self, FEED_WAIT, Fed, PREPARE_WAIT, STOP_WAIT};
use super::scenarios::{self, BOOT_WAIT, DeviceState, OUTAGE, REJOIN_WAIT};
use super::{Armed, Ctx, Error, Fault, Hotspot, Result, STATUS_WAIT, compact, short};

/// One cycle: its fault, then feeds at the cadence until it is spent.
const CYCLE: Duration = Duration::from_secs(30 * 60);
/// From one feed's request to the next one's, within a cycle.
const FEED_PERIOD: Duration = Duration::from_secs(10 * 60);
/// A device API status between feeds, as `soak` asks.
const POLL_PERIOD: Duration = Duration::from_secs(30);
/// Into the drive before a cut or an outage: past the start-up surge and
/// before the first portion's mark, about 7.8 s in on the bench.
const INTO_DRIVE: Duration = Duration::from_secs(4);
/// The most a feed may ask for, by the owner's unattended rule.
const MAX_PORTIONS: u8 = 3;
/// The least free heap the feeder may report at rest. It had about 141 KB
/// least free in normal mode (October 23); half of that is a floor any
/// leak or fragmentation worth stopping for crosses well before failing.
const HEAP_FLOOR: u32 = 64 * 1024;
/// How far free heap may fall within one boot of one build, from its first
/// reading after a feed: sessions and the event ring settle well inside it.
const HEAP_DRIFT: u32 = 8 * 1024;
/// The clock's error limit: past it the feeder reports no time at all.
const CLOCK_LIMIT_MS: u32 = 120_000;
/// A request's round trip and the host's own error, beside the feeder's
/// stated bound.
const CLOCK_SLACK_MS: u64 = 2_000;

/// Why the sustained run halted. Each is a halt: relay 1 goes off.
#[derive(Debug, thiserror::Error)]
pub enum SustainedError {
    /// `lines` are the UART records nobody planned: a restart, a panic, a
    /// reset storm or a give-way, or the motor driving with no feed asked.
    #[error("Unplanned during {during}: {}", lines.join(" | "))]
    Unplanned {
        during: &'static str,
        lines: Vec<String>,
    },
    #[error(
        "{during} should restart the feeder once; the boot count went from {before} to {after}"
    )]
    Restarts {
        during: &'static str,
        before: u32,
        after: u32,
    },
    #[error("Feed {feed} ended {got:?}, not {want:?}")]
    Outcome { feed: u32, want: Expect, got: Fed },
    #[error("Feed {feed} reuses an id at or below {highest}")]
    IdReused { feed: u32, highest: u32 },
    #[error("Feed {feed} has {submitted} owner records and {results} results; each needs one")]
    Journal {
        feed: u32,
        submitted: usize,
        results: usize,
    },
    #[error("Results for feeds the run never asked for: {ids:?}")]
    Unasked { ids: Vec<u64> },
    #[error(
        "This boot's counters read {got:?} (feeds, failed, portions); its results add up to {want:?}"
    )]
    Counters {
        got: (u32, u32, u32),
        want: (u32, u32, u32),
    },
    #[error("A lifetime counter went backwards: {before:?} then {after:?}")]
    Lifetime { before: Counts, after: Counts },
    #[error("Least free heap {heap_min} B is under the {HEAP_FLOOR} B floor")]
    Heap { heap_min: u32 },
    #[error("Free heap fell from {first} B to {now} B within one boot")]
    HeapDrift { first: u32, now: u32 },
    #[error("The clock's error bound reads {error_ms} ms, past its {CLOCK_LIMIT_MS} ms limit")]
    ClockLimit { error_ms: u32 },
    #[error("The clock reads valid with no error bound")]
    NoBound,
    #[error("The clock is {skew_ms} ms from this host's, outside its {error_ms} ms bound")]
    ClockOutside { skew_ms: u64, error_ms: u32 },
    #[error("The clock did not survive a reboot (the warm carry)")]
    ClockLost,
    #[error("This boot's reset cause reads {got:?}, not {want:?}")]
    ResetCause { want: ResetCause, got: ResetCause },
    #[error("The {what} request answered {}", compact(answer))]
    Answer { what: &'static str, answer: Answer },
    #[error(
        "A feed to a faulted owner answered {}, not refused as a fault",
        compact(answer)
    )]
    NotRefused { answer: Answer },
    #[error("{} did not install and confirm: {outcome:?}", short(build))]
    Install { build: String, outcome: Outcome },
    #[error("The feeder is not as the run began: {now} (was {want})")]
    StateChanged { now: String, want: String },
    #[error("The Wi-Fi link was lost at rest ({before} losses, then {after})")]
    LinkLoss { before: u32, after: u32 },
    #[error("An outage mid-feed restarted the feeder (boot {before} then {after})")]
    OutageRestart { before: u32, after: u32 },
    #[error("The outage was not seen ({before} losses, then {after})")]
    OutageUnseen { before: u32, after: u32 },
}

/// One cycle's fault, in the order the run takes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Step {
    /// `feed-writes`: settings, lock and clock writes during a feed, then a
    /// cancel during the next.
    FeedWrites,
    /// A reboot request at rest: one restart, the clock carried.
    Reboot,
    /// A power cut once the journal has reserved a feed, before its drive.
    CutReserved,
    /// The `feed-jam` variant: a jam that frees, one that holds, the mark
    /// input stuck LOW.
    Jam,
    /// A power cut 4 s into a 3-portion drive.
    CutDriving,
    /// The `sensor-fault` variant: marks lost, the outlet blocked, the ADC
    /// lost and the owner's hold until a reboot.
    Sensor,
    /// The hotspot down for a minute from 4 s into a 3-portion drive.
    Outage,
    /// The `feed-stall` variant (a loop stall in each drive), then the two
    /// update power cuts of `ota-power-cuts`.
    Stalls,
}

impl Step {
    pub(super) const ROTATION: [Self; 8] = [
        Self::FeedWrites,
        Self::Reboot,
        Self::CutReserved,
        Self::Jam,
        Self::CutDriving,
        Self::Sensor,
        Self::Outage,
        Self::Stalls,
    ];

    fn about(self) -> &'static str {
        match self {
            Self::FeedWrites => "writes during a feed, a cancel during the next",
            Self::Reboot => "a reboot at rest",
            Self::CutReserved => "a power cut with a feed reserved",
            Self::Jam => "the feed-jam variant",
            Self::CutDriving => "a power cut mid-drive",
            Self::Sensor => "the sensor-fault variant",
            Self::Outage => "the hotspot down mid-feed",
            Self::Stalls => "the feed-stall variant, then update power cuts",
        }
    }
}

/// How a feed must end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Expect {
    outcome: OwnerOutcome,
    /// `None`: any reason.
    reason: Option<FeedReason>,
    /// `None`: any count.
    completed: Option<u16>,
}

impl Expect {
    fn done(portions: u8) -> Self {
        Self {
            outcome: OwnerOutcome::Done,
            reason: None,
            completed: Some(u16::from(portions)),
        }
    }

    fn failed(reason: FeedReason) -> Self {
        Self {
            outcome: OwnerOutcome::Failed,
            reason: Some(reason),
            completed: None,
        }
    }

    /// A reservation a reset interrupted, resolved at the next boot.
    fn uncertain() -> Self {
        Self {
            outcome: OwnerOutcome::Uncertain,
            reason: None,
            completed: None,
        }
    }

    fn met_by(self, fed: Fed) -> bool {
        fed.outcome == Some(self.outcome)
            && self.reason.is_none_or(|reason| fed.reason == Some(reason))
            && self
                .completed
                .is_none_or(|completed| fed.completed == completed)
    }

    fn check(self, feed: u32, got: Fed) -> Result<()> {
        require(self.met_by(got), || {
            SustainedError::Outcome {
                feed,
                want: self,
                got,
            }
            .into()
        })
    }
}

/// An injected-fault variant: its build, its label in evidence names, and
/// how each of its three feeds must end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Injected {
    Jam,
    Sensor,
    Stall,
}

impl Injected {
    fn variant(self) -> Variant {
        match self {
            Self::Jam => Variant::FeedJam,
            Self::Sensor => Variant::SensorFault,
            Self::Stall => Variant::FeedStall,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Jam => "jam",
            Self::Sensor => "sensor",
            Self::Stall => "stall",
        }
    }

    /// As `wellfed-build --variant` names it.
    fn name(self) -> &'static str {
        match self {
            Self::Jam => "feed-jam",
            Self::Sensor => "sensor-fault",
            Self::Stall => "feed-stall",
        }
    }
}

/// Whether a reading of the clock must find it valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Carry {
    /// After a warm restart: the carry keeps it.
    Required,
    /// Anything else: an invalid clock is synced from the host and counted.
    Optional,
}

/// The boot the counters are compared within: the UART line of its start
/// record (`None`: it began before the run), and its counters there. A boot
/// is told by its start record, not the status boot count, which lives in a
/// backup register that a power cut clears.
#[derive(Debug, Clone, Copy)]
struct Boot {
    start: Option<usize>,
    base: Counts,
}

/// The first free-heap reading of one boot of one build.
#[derive(Debug, Clone)]
struct HeapBase {
    start: Option<usize>,
    build: String,
    free: u32,
}

/// What the run measured, for its notes.
#[derive(Debug, Default)]
struct Figures {
    feeds: u32,
    polls: u32,
    slowest: Duration,
    heap_min: Option<u32>,
    skew_max_ms: u64,
    error_max_ms: u32,
    syncs: u32,
}

struct Run {
    /// The baseline every cycle returns to.
    build: String,
    /// The UART position when the run began: every journal check reads from
    /// here.
    since: usize,
    /// Every feed id the run asked for.
    asked: BTreeSet<u32>,
    feeds: u32,
    boot: Boot,
    /// The last lifetime figures, with the boot counts read beside them.
    lifetime: Option<Lifetime>,
    heap: Option<HeapBase>,
    figures: Figures,
    /// The injected-fault variant bank 1 holds until its restore confirms.
    variant: Option<Injected>,
}

pub(super) fn sustained(ctx: &mut Ctx) -> Result<()> {
    // Until the run ends cleanly, the rig cuts relay 1 however it is torn
    // down, a host panic's unwinding included.
    ctx.rig.arm_kill(Armed::On);
    let mut run = match Run::open(ctx) {
        Ok(run) => run,
        Err(error) => return halt(ctx, error, None),
    };
    match run.cycles(ctx) {
        Ok(()) => {
            ctx.rig.arm_kill(Armed::Off);
            Ok(())
        }
        Err(error) => halt(ctx, error, run.variant),
    }
}

/// Cut relay 1 and leave it off, and say what bank 1 may still hold.
fn halt(ctx: &mut Ctx, error: Error, variant: Option<Injected>) -> Result<()> {
    let how = match ctx.rig.kill() {
        Ok(()) => "relay 1 is off".to_string(),
        Err(cut) => format!("cutting relay 1 failed ({cut}): cut it by hand"),
    };
    let left = variant.map_or_else(String::new, |variant| {
        format!(
            "; bank 1 may still hold the {} variant: install the baseline before any other run",
            variant.name()
        )
    });
    ctx.note(&format!("  Halted: {error}; {how}{left}"));
    ctx.halted = Some(format!("the sustained run halted; {how}{left}"));
    Err(error)
}

impl Run {
    fn open(ctx: &mut Ctx) -> Result<Self> {
        let build = ctx.baseline()?.build.clone();
        let since = ctx.mark()?;
        let counters = counters(ctx)?;

        Ok(Self {
            build,
            since,
            asked: BTreeSet::new(),
            feeds: 0,
            boot: Boot {
                start: None,
                base: counters.boot,
            },
            lifetime: Lifetime::of(&counters),
            heap: None,
            variant: None,
            figures: Figures::default(),
        })
    }

    fn cycles(&mut self, ctx: &mut Ctx) -> Result<()> {
        let want = scenarios::device_state(ctx)?;
        let cycles = ctx.args.sustained_cycles;

        for (cycle, step) in (1..=cycles).zip(Step::ROTATION.iter().copied().cycle()) {
            let start = ctx.rig.clock();
            let end = start + CYCLE;
            ctx.note(&format!("  Cycle {cycle}/{cycles}: {}", step.about()));
            self.step(ctx, cycle, step)?;
            self.settled(ctx, &want)?;

            // A feed opens the cycle's feeding, then one every period
            // while the cycle lasts.
            loop {
                let at = ctx.rig.clock();
                let portions = self.next_portions();
                self.feed(ctx, portions, Expect::done(portions), OwnerState::Idle)?;
                self.checks(ctx)?;
                let next = at + FEED_PERIOD;
                if next >= end {
                    break;
                }
                self.rest(ctx, next)?;
            }
            self.rest(ctx, end)?;
            self.journal(ctx)?;
            // A step that outlasts its half hour (the update cuts can)
            // stretches its cycle rather than squeezing the next.
            let took = ctx.rig.clock().saturating_sub(start);
            self.report(ctx, cycle, took);
        }
        Ok(())
    }

    /// 1, 2, 3 portions in turn: never more than the unattended limit.
    fn next_portions(&mut self) -> u8 {
        let turn = u8::try_from(self.feeds % u32::from(MAX_PORTIONS)).unwrap_or(0);
        self.feeds += 1;
        turn + 1
    }

    fn step(&mut self, ctx: &mut Ctx, cycle: u32, step: Step) -> Result<()> {
        match step {
            Step::FeedWrites => {
                let mark = ctx.mark()?;
                feeds::feed_writes(ctx)?;
                self.adopt(ctx, mark)
            }
            Step::Reboot => {
                let build = self.build.clone();
                self.reboot(ctx, &build)
            }
            Step::CutReserved => self.cut(ctx, Phase::Reserved),
            Step::Jam => self.injected(ctx, cycle, Injected::Jam),
            Step::CutDriving => self.cut(ctx, Phase::Driving),
            Step::Sensor => self.injected(ctx, cycle, Injected::Sensor),
            Step::Outage => self.outage(ctx),
            Step::Stalls => {
                self.injected(ctx, cycle, Injected::Stall)?;
                scenarios::update_power_cuts(ctx, &format!("s{cycle:02}-cut"))
            }
        }
    }

    /// Ask for a feed once and see it end as `expect` says, with the owner
    /// then in `rest`, and nothing restarting in between.
    fn feed(
        &mut self,
        ctx: &mut Ctx,
        portions: u8,
        expect: Expect,
        rest: OwnerState,
    ) -> Result<()> {
        let (feed, start) = self.ask_feed(ctx, portions)?;

        let (_, fed) = result(ctx, feed, start, FEED_WAIT + STOP_WAIT)?;
        expect.check(feed, fed)?;
        // From the feed's start, not its result: a fault's state change is
        // logged as the supervisor stops the drive, before the result.
        ctx.expect("the owner at rest", start, STOP_WAIT, |e| {
            feeds::owner_state(e, rest)
        })?;
        no_restart(ctx, start, "a feed")
    }

    /// One feed request, never resent; its id and the mark past the
    /// owner's record of it.
    fn ask_feed(&mut self, ctx: &mut Ctx, portions: u8) -> Result<(u32, usize)> {
        let mut issued = None;
        let (feed, start) = feeds::start_feed(ctx, &mut issued, portions.min(MAX_PORTIONS))?;
        self.asked(feed)?;
        self.figures.feeds += 1;
        Ok((feed, start))
    }

    /// Ids only grow: the journal never hands one out twice.
    fn asked(&mut self, feed: u32) -> Result<()> {
        if let Some(&highest) = self.asked.last() {
            require(feed > highest, || SustainedError::IdReused {
                feed,
                highest,
            })?;
        }
        self.asked.insert(feed);
        Ok(())
    }

    /// Take on the feeds another scenario asked for since `mark`.
    fn adopt(&mut self, ctx: &mut Ctx, mark: usize) -> Result<()> {
        ctx.mark()?;
        let ids = ctx.log.records(mark, |e| feeds::submitted(&e));
        for (_, id) in ids {
            let feed = u32::try_from(id).unwrap_or(u32::MAX);
            self.asked(feed)?;
            self.figures.feeds += 1;
        }
        Ok(())
    }

    /// A reboot request at rest: `build` starts once more. The firmware restarts
    /// by letting its watchdog lapse, so the cause reads `watchdog`; the clock
    /// is carried and nothing drives after it.
    fn reboot(&mut self, ctx: &mut Ctx, build: &str) -> Result<()> {
        // An install's activation may have left the clock unset: set it, so
        // that only the reboot can lose it.
        self.clock(ctx, Carry::Optional)?;
        let before = ctx.device_status()?;
        let since = ctx.mark()?;
        match ctx.ask(&Ask::Reboot)? {
            Answer::Done { .. } => {}
            answer => {
                return Err(SustainedError::Answer {
                    what: "reboot",
                    answer,
                }
                .into());
            }
        }

        let booted = ctx.expect("the build starting again", since, BOOT_WAIT, |e| {
            scenarios::started(e, build)
        })?;
        let after = ctx.reachable(STATUS_WAIT)?;
        restarted_once(&before, &after, "a reboot")?;
        reset_cause(ctx, ResetCause::Watchdog)?;
        self.clock(ctx, Carry::Required)?;
        no_drive(ctx, booted, "the boot after a reboot")
    }

    /// A 3-portion feed cut off by the supply at `phase`: the next boot
    /// resolves it as uncertain, once, and never drives it again.
    fn cut(&mut self, ctx: &mut Ctx, phase: Phase) -> Result<()> {
        // The feeder holds the `Feeding` answer until the owner has reserved
        // the feed in its journal, and `ask_feed` returns only after that
        // answer: the reserved cut lands after the reservation, not on the
        // earlier `feed id=` record.
        let (feed, start) = self.ask_feed(ctx, MAX_PORTIONS)?;
        if phase == Phase::Driving {
            ctx.expect("the motor driving", start, PREPARE_WAIT, |e| {
                feeds::owner_state(e, OwnerState::Feeding)
            })?;
            ctx.idle(INTO_DRIVE)?;
        }
        let driving = ctx
            .log
            .count(start, |e| feeds::owner_state(e, OwnerState::Feeding))
            > 0;
        ctx.note(&format!(
            "  Cutting the supply with feed {feed} {}",
            if driving {
                "driving"
            } else {
                "reserved, not yet driving"
            }
        ));

        let since = ctx.mark()?;
        ctx.rig.power_cycle()?;
        let build = self.build.clone();
        let booted = ctx.expect("the baseline after the cut", since, BOOT_WAIT, |e| {
            scenarios::started(e, &build)
        })?;
        let (_, fed) = result(ctx, feed, booted, STATUS_WAIT)?;
        Expect::uncertain().check(feed, fed)?;

        ctx.reachable(STATUS_WAIT)?;
        one_start(ctx, since, "a power cut")?;
        reset_cause(ctx, ResetCause::PowerOn)?;
        // A power cut starts the clock invalid: the run sets it again.
        self.clock(ctx, Carry::Optional)?;
        no_drive(ctx, booted, "the boot after a power cut")
    }

    /// The hotspot down for a minute from 4 s into a 3-portion drive: the
    /// feed ends Done regardless, and the feeder rejoins without a restart.
    fn outage(&mut self, ctx: &mut Ctx) -> Result<()> {
        let before = ctx.device_status()?;
        let (feed, start) = self.ask_feed(ctx, MAX_PORTIONS)?;
        ctx.expect("the motor driving", start, PREPARE_WAIT, |e| {
            feeds::owner_state(e, OwnerState::Feeding)
        })?;
        ctx.idle(INTO_DRIVE)?;

        let back_at = ctx.rig.clock() + OUTAGE;
        ctx.rig.hotspot(Hotspot::Down)?;
        let ended = ctx.offline(|ctx| {
            let ended = result(ctx, feed, start, FEED_WAIT + STOP_WAIT);
            let waited = ctx.idle(back_at.saturating_sub(ctx.rig.clock()));
            ended.and_then(|ended| waited.map(|()| ended))
        });
        let restored = ctx.rig.hotspot(Hotspot::Up);
        let (_, fed) = ended?;
        restored?;
        Expect::done(MAX_PORTIONS).check(feed, fed)?;

        let after = ctx.offline(|ctx| {
            ctx.until_device("the device API after the outage", REJOIN_WAIT, |_| {
                Ok(String::new())
            })?;
            ctx.device_status()
        })?;
        require(after.boot_count == before.boot_count, || {
            SustainedError::OutageRestart {
                before: before.boot_count,
                after: after.boot_count,
            }
        })?;
        require(after.losses > before.losses, || {
            SustainedError::OutageUnseen {
                before: before.losses,
                after: after.losses,
            }
        })?;
        no_restart(ctx, start, "an outage mid-feed")
    }

    /// Install `injected`'s build over the air (a trial, then its
    /// confirmation), run its three feeds, then put the baseline back.
    fn injected(&mut self, ctx: &mut Ctx, cycle: u32, injected: Injected) -> Result<()> {
        let label = format!("s{cycle:02}-{}", injected.label());
        let image = Image {
            firmware: Firmware::Feeder,
            variant: Some(injected.variant()),
        };
        let staged = ctx.image(&label, image)?;
        // From the offer on, bank 1 may hold the variant.
        self.variant = Some(injected);
        install(ctx, &staged)?;
        let build = staged.build.clone();

        match injected {
            Injected::Jam => {
                self.feed(ctx, 1, Expect::done(1), OwnerState::Idle)?;
                self.feed(
                    ctx,
                    2,
                    Expect::failed(FeedReason::Overcurrent),
                    OwnerState::Idle,
                )?;
                self.feed(ctx, 3, Expect::failed(FeedReason::Sensor), OwnerState::Idle)?;
            }
            Injected::Sensor => {
                self.feed(
                    ctx,
                    1,
                    Expect::failed(FeedReason::FeedbackTimeout),
                    OwnerState::Idle,
                )?;
                let mark = ctx.mark()?;
                self.feed(ctx, 2, Expect::failed(FeedReason::Outlet), OwnerState::Idle)?;
                no_drive(ctx, mark, "a feed with the outlet blocked")?;
                self.feed(
                    ctx,
                    3,
                    Expect::failed(FeedReason::Sensor),
                    OwnerState::Fault,
                )?;
                self.held(ctx)?;
                self.reboot(ctx, &build)?;
            }
            Injected::Stall => {
                self.stalled(ctx, 1, &build, Expect::failed(FeedReason::Output))?;
                self.stalled(ctx, 2, &build, Expect::uncertain())?;
                reset_cause(ctx, ResetCause::Watchdog)?;
            }
        }

        let staged = ctx.resign(&format!("{label}-restore"))?;
        install(ctx, &staged)?;
        self.variant = None;
        Ok(())
    }

    /// The owner holds its fault: a feed is refused, and nothing drives.
    fn held(&mut self, ctx: &mut Ctx) -> Result<()> {
        let since = ctx.mark()?;
        let answer = ctx.ask(&Ask::Feed { portions: 1 })?;
        if let Answer::Feeding { feed } = answer {
            self.asked(feed)?;
        }
        require(answer == Answer::Refused(Refusal::Fault), || {
            SustainedError::NotRefused { answer }
        })?;
        no_drive(ctx, since, "the owner's fault hold")
    }

    /// A `feed-stall` feed: its loop stall ends in one restart, and its
    /// result is `expect`, whichever side of the restart it lands.
    fn stalled(&mut self, ctx: &mut Ctx, portions: u8, build: &str, expect: Expect) -> Result<()> {
        let before = ctx.device_status()?;
        let (feed, start) = self.ask_feed(ctx, portions)?;

        let booted = ctx.expect(
            "the restart after the stall",
            start,
            FEED_WAIT + BOOT_WAIT,
            |e| scenarios::started(e, build),
        )?;
        let (_, fed) = result(ctx, feed, start, STATUS_WAIT)?;
        expect.check(feed, fed)?;
        let after = ctx.reachable(STATUS_WAIT)?;
        restarted_once(&before, &after, "a stalled feed")?;
        no_drive(ctx, booted, "the boot after a stalled feed")
    }

    /// The baseline runs on bank 1 with the settings and Wi-Fi source the
    /// run began with.
    fn settled(&mut self, ctx: &mut Ctx, want: &DeviceState) -> Result<()> {
        let now = scenarios::device_state(ctx)?;
        require(now == *want, || SustainedError::StateChanged {
            now: format!("{now:?}"),
            want: format!("{want:?}"),
        })?;
        self.clock(ctx, Carry::Optional)
    }

    /// After a feed, at rest: counters, memory and the clock.
    fn checks(&mut self, ctx: &mut Ctx) -> Result<()> {
        let status = ctx.device_status()?;
        self.counters(ctx)?;
        self.memory(ctx, &status)?;
        self.clock(ctx, Carry::Optional)
    }

    /// This boot's counters agree with its results on the UART, and the
    /// lifetime counters never go back.
    fn counters(&mut self, ctx: &mut Ctx) -> Result<()> {
        let start = last_start(ctx, self.since)?;
        if start != self.boot.start {
            self.boot = Boot {
                start,
                base: Counts::default(),
            };
        }
        let counters = counters(ctx)?;

        let from = self.boot.start.unwrap_or(self.since);
        let results = ctx.log.records(from, |e| match e {
            Event::Owner(Owner::Fed {
                outcome, completed, ..
            }) => Some((outcome, completed)),
            _ => None,
        });
        let (mut feeds, mut failed, mut portions) = (0u32, 0u32, 0u32);
        for (_, (outcome, completed)) in results {
            feeds += 1;
            failed += u32::from(outcome != Some(OwnerOutcome::Done));
            portions += u32::from(completed);
        }
        let base = self.boot.base;
        let want = (
            base.feeds + feeds,
            base.feeds_failed + failed,
            base.portions + portions,
        );
        let got = (
            counters.boot.feeds,
            counters.boot.feeds_failed,
            counters.boot.portions,
        );
        require(got == want, || SustainedError::Counters { got, want })?;

        let now = Lifetime::of(&counters);
        if let (Some(before), Some(after)) = (self.lifetime, now) {
            require(after.holds_after(&before), || SustainedError::Lifetime {
                before: before.total,
                after: after.total,
            })?;
        }
        self.lifetime = now.or(self.lifetime);
        Ok(())
    }

    /// Least free heap above the floor, and free heap within its drift of
    /// this boot's first reading.
    fn memory(&mut self, ctx: &mut Ctx, status: &DeviceStatus) -> Result<()> {
        let memory = memory(ctx)?;
        require(memory.heap_min >= HEAP_FLOOR, || SustainedError::Heap {
            heap_min: memory.heap_min,
        })?;
        self.figures.heap_min = Some(
            self.figures
                .heap_min
                .map_or(memory.heap_min, |least| least.min(memory.heap_min)),
        );

        let start = last_start(ctx, self.since)?;
        match &self.heap {
            Some(base) if base.start == start && base.build == status.build => {
                let first = base.free;
                require(memory.heap_free + HEAP_DRIFT >= first, || {
                    SustainedError::HeapDrift {
                        first,
                        now: memory.heap_free,
                    }
                    .into()
                })
            }
            _ => {
                self.heap = Some(HeapBase {
                    start,
                    build: status.build.clone(),
                    free: memory.heap_free,
                });
                Ok(())
            }
        }
    }

    /// A valid clock lies within its stated error bound of the host's, and
    /// the bound within its limit; an invalid one is set again, unless
    /// `carry` says a warm restart should have kept it.
    fn clock(&mut self, ctx: &mut Ctx, carry: Carry) -> Result<()> {
        let view = time(ctx)?;
        let host = ctx.rig.unix_ms();

        let Some(clock) = view.clock_ms else {
            require(carry == Carry::Optional, || SustainedError::ClockLost)?;
            self.figures.syncs += 1;
            return ctx.sync_clock();
        };
        let error_ms = view.error_ms.ok_or(SustainedError::NoBound)?;
        let skew_ms = host.abs_diff(i64::try_from(clock).unwrap_or(i64::MAX));
        require(error_ms <= CLOCK_LIMIT_MS, || SustainedError::ClockLimit {
            error_ms,
        })?;
        require(skew_ms <= u64::from(error_ms) + CLOCK_SLACK_MS, || {
            SustainedError::ClockOutside { skew_ms, error_ms }
        })?;

        self.figures.skew_max_ms = self.figures.skew_max_ms.max(skew_ms);
        self.figures.error_max_ms = self.figures.error_max_ms.max(error_ms);
        Ok(())
    }

    /// Poll the device API until `until`: the same boot, no link loss, and
    /// nothing restarting or driving.
    fn rest(&mut self, ctx: &mut Ctx, until: Duration) -> Result<()> {
        let since = ctx.mark()?;
        let before = ctx.device_status()?;

        while ctx.rig.clock() < until {
            let at = ctx.rig.clock();
            let status = match ctx.device_status() {
                Ok(status) => status,
                Err(error) if error.interrupted() => return Err(error),
                // A restart is the likelier cause, and the better reason.
                Err(error) => {
                    no_restart(ctx, since, "rest")?;
                    return Err(error);
                }
            };
            self.figures.slowest = self.figures.slowest.max(ctx.rig.clock().saturating_sub(at));
            self.figures.polls += 1;
            require(status.boot_count == before.boot_count, || {
                SustainedError::Restarts {
                    during: "rest",
                    before: before.boot_count,
                    after: status.boot_count,
                }
            })?;
            require(status.losses == before.losses, || {
                SustainedError::LinkLoss {
                    before: before.losses,
                    after: status.losses,
                }
            })?;
            ctx.idle(POLL_PERIOD.min(until.saturating_sub(ctx.rig.clock())))?;
        }
        no_drive(ctx, since, "rest")?;
        no_restart(ctx, since, "rest")
    }

    /// Every feed the run asked for has one owner record and one result,
    /// no result belongs to a feed it did not ask for, and nothing
    /// panicked.
    fn journal(&mut self, ctx: &mut Ctx) -> Result<()> {
        ctx.mark()?;
        let submitted = ctx.log.records(self.since, |e| feeds::submitted(&e));
        let results = ctx.log.records(self.since, |e| match e {
            Event::Owner(Owner::Fed { id, .. }) => Some(id),
            _ => None,
        });

        for &feed in &self.asked {
            let id = u64::from(feed);
            let submitted = submitted.iter().filter(|(_, s)| *s == id).count();
            let results_for = results.iter().filter(|(_, r)| *r == id).count();
            require(submitted == 1 && results_for == 1, || {
                SustainedError::Journal {
                    feed,
                    submitted,
                    results: results_for,
                }
            })?;
        }
        let ids: Vec<u64> = results
            .iter()
            .map(|(_, id)| *id)
            .filter(|id| u32::try_from(*id).map_or(true, |id| !self.asked.contains(&id)))
            .collect();
        require(ids.is_empty(), || SustainedError::Unasked { ids })?;

        let panics = ctx
            .log
            .between(self.since, usize::MAX, |e| matches!(e, Event::Panic { .. }));
        require(panics.is_empty(), || {
            SustainedError::Unplanned {
                during: "the run",
                lines: panics,
            }
            .into()
        })
    }

    fn report(&self, ctx: &mut Ctx, cycle: u32, took: Duration) {
        let f = &self.figures;
        ctx.note(&format!(
            "  Cycle {cycle} passed in {:.1} min: {} feeds so far, {} polls (slowest {:.1} s), \
             least free heap {}, clock skew up to {} ms within bounds up to {} ms, {} clock syncs",
            took.as_secs_f64() / 60.0,
            f.feeds,
            f.polls,
            f.slowest.as_secs_f64(),
            f.heap_min
                .map_or_else(|| "unread".to_string(), |b| format!("{b} B")),
            f.skew_max_ms,
            f.error_max_ms,
            f.syncs
        ));
    }
}

/// When a power cut lands in a feed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// As soon as the feed is answered: the journal has reserved it.
    Reserved,
    /// Into its drive.
    Driving,
}

/// Feed `feed`'s result record at or after `since`; the mark past it.
fn result(ctx: &mut Ctx, feed: u32, since: usize, within: Duration) -> Result<(usize, Fed)> {
    let mark = ctx.expect(&format!("PLOWN1 fed id={feed}"), since, within, |e| {
        feeds::fed_of(e, feed).is_some()
    })?;
    Ok((mark, feeds::result_at(ctx, mark, feed)?))
}

/// Install `staged` and see it confirm.
fn install(ctx: &mut Ctx, staged: &Staged) -> Result<()> {
    let outcome = ctx.install(staged)?;
    ctx.rig.offer(None, Fault::None)?;
    require(outcome == Outcome::Passed, || {
        SustainedError::Install {
            build: staged.build.clone(),
            outcome,
        }
        .into()
    })
}

/// No start, panic, reset storm or give-way since `since`.
fn no_restart(ctx: &mut Ctx, since: usize, during: &'static str) -> Result<()> {
    ctx.mark()?;
    let lines = ctx.log.between(since, usize::MAX, |e| {
        matches!(
            e,
            Event::Start { .. }
                | Event::Panic { .. }
                | Event::Trial(Trial::Storm { .. } | Trial::GiveWay)
        )
    });
    require(lines.is_empty(), || {
        SustainedError::Unplanned { during, lines }.into()
    })
}

/// The motor not driving since `since`.
fn no_drive(ctx: &mut Ctx, since: usize, during: &'static str) -> Result<()> {
    ctx.mark()?;
    let lines = ctx.log.between(since, usize::MAX, |e| {
        feeds::owner_state(e, OwnerState::Feeding)
    });
    require(lines.is_empty(), || {
        SustainedError::Unplanned { during, lines }.into()
    })
}

/// The UART line of the last start record since `since`: the running
/// boot's, or `None` when it began before.
fn last_start(ctx: &mut Ctx, since: usize) -> Result<Option<usize>> {
    ctx.mark()?;
    let starts = ctx
        .log
        .records(since, |e| scenarios::is_start(&e).then_some(()));
    Ok(starts.last().map(|(line, ())| *line))
}

/// Exactly one start record since `since`, and no panic.
fn one_start(ctx: &mut Ctx, since: usize, during: &'static str) -> Result<()> {
    ctx.mark()?;
    let lines = ctx.log.between(since, usize::MAX, |e| {
        matches!(e, Event::Start { .. } | Event::Panic { .. })
    });
    let starts = ctx.log.count(since, scenarios::is_start);
    let panics = ctx.log.count(since, |e| matches!(e, Event::Panic { .. }));
    require(starts == 1 && panics == 0, || {
        SustainedError::Unplanned { during, lines }.into()
    })
}

/// The boot count, which a warm restart keeps, moved by exactly one.
fn restarted_once(before: &DeviceStatus, after: &DeviceStatus, during: &'static str) -> Result<()> {
    require(after.boot_count == before.boot_count + 1, || {
        SustainedError::Restarts {
            during,
            before: before.boot_count,
            after: after.boot_count,
        }
        .into()
    })
}

/// This boot's reset cause, as the feeder reports it.
fn reset_cause(ctx: &mut Ctx, want: ResetCause) -> Result<()> {
    let got = counters(ctx)?.reset;
    require(got == want, || {
        SustainedError::ResetCause { want, got }.into()
    })
}

/// A lifetime reading and the boot's own counts from the same answer. The
/// feeder reports its lifetime figures as the totals its store held when
/// the boot opened plus this boot's counts, so `total` less `boot` is that
/// stored base, fixed for the boot.
#[derive(Clone, Copy, Debug)]
struct Lifetime {
    total: Counts,
    boot: Counts,
}

impl Lifetime {
    fn of(counters: &Counters) -> Option<Self> {
        counters.lifetime.map(|total| Self {
            total,
            boot: counters.boot,
        })
    }

    /// Within one boot the figures only grow. Across a reset only the
    /// stored base must: once the store's early-save budget is spent, a
    /// restart's save is refused and what the last boot counted since its
    /// last save is lost by design (the totals are then lower bounds), but
    /// totals the store held are never lost. A boot is the same while its
    /// uptime has not gone back.
    fn holds_after(&self, before: &Self) -> bool {
        if self.boot.uptime_s >= before.boot.uptime_s {
            never_back(&before.total, &self.total)
        } else {
            never_back(&before.stored(), &self.stored())
        }
    }

    fn stored(&self) -> Counts {
        let (t, b) = (&self.total, &self.boot);
        Counts {
            uptime_s: t.uptime_s.saturating_sub(b.uptime_s),
            feeds: t.feeds.saturating_sub(b.feeds),
            feeds_failed: t.feeds_failed.saturating_sub(b.feeds_failed),
            portions: t.portions.saturating_sub(b.portions),
            presses: t.presses.saturating_sub(b.presses),
            sessions: t.sessions.saturating_sub(b.sessions),
            reconnects: t.reconnects.saturating_sub(b.reconnects),
            dhcp_renewals: t.dhcp_renewals.saturating_sub(b.dhcp_renewals),
            outages: t.outages.saturating_sub(b.outages),
            battery_s: t.battery_s.saturating_sub(b.battery_s),
        }
    }
}

/// No lifetime count is below what it was.
fn never_back(before: &Counts, after: &Counts) -> bool {
    after.uptime_s >= before.uptime_s
        && after.feeds >= before.feeds
        && after.feeds_failed >= before.feeds_failed
        && after.portions >= before.portions
        && after.sessions >= before.sessions
        && after.reconnects >= before.reconnects
        && after.outages >= before.outages
}

fn counters(ctx: &mut Ctx) -> Result<Counters> {
    let answer = ctx.ask(&Ask::Stats {
        section: StatsSection::Counters,
    })?;
    if let Answer::Stats(stats) = &answer
        && let Stats::Counters(counters) = stats.as_ref()
    {
        return Ok(*counters);
    }
    Err(SustainedError::Answer {
        what: "counters",
        answer,
    }
    .into())
}

fn memory(ctx: &mut Ctx) -> Result<Memory> {
    let answer = ctx.ask(&Ask::Stats {
        section: StatsSection::Memory,
    })?;
    if let Answer::Stats(stats) = &answer
        && let Stats::Memory(memory) = stats.as_ref()
    {
        return Ok(memory.clone());
    }
    Err(SustainedError::Answer {
        what: "memory",
        answer,
    }
    .into())
}

fn time(ctx: &mut Ctx) -> Result<TimeView> {
    match ctx.ask(&Ask::GetTime)? {
        Answer::Time(view) => Ok(*view),
        answer => Err(SustainedError::Answer {
            what: "time",
            answer,
        }
        .into()),
    }
}

#[cfg(test)]
mod lifetime_tests {
    use super::*;

    fn reading(stored_feeds: u32, boot_feeds: u32, boot_uptime_s: u32) -> Lifetime {
        let boot = Counts {
            feeds: boot_feeds,
            uptime_s: boot_uptime_s,
            ..Counts::default()
        };
        Lifetime {
            total: Counts {
                feeds: stored_feeds + boot_feeds,
                uptime_s: 400 + boot_uptime_s,
                ..Counts::default()
            },
            boot,
        }
    }

    #[test]
    fn within_a_boot_the_lifetime_figures_only_grow() {
        let before = reading(4, 3, 1200);

        assert!(reading(4, 4, 1230).holds_after(&before));
        assert!(!reading(3, 3, 1230).holds_after(&before));
    }

    #[test]
    fn a_reset_may_lose_the_unsaved_boot_but_never_the_stored_totals() {
        // October 27: a reboot at rest with the early-save budget spent.
        let before = reading(4, 4, 1300);

        assert!(reading(4, 0, 20).holds_after(&before));
        assert!(!reading(3, 0, 20).holds_after(&before));
    }
}
