//! Bench probe on KM4: food (PA17), outlet (PA16) and feedback (PA18)
//! inputs with the infrared emitters on, motor STOP throughout.
//!
//! Opens the board (motor STOP, emitters OFF, sensor inputs), binds a fresh
//! flash authority to the KM0 peer only after stopped-output readback, and
//! attaches it. No flash hook provider is installed. The supervisor then
//! owns TIM2: each emitter window arms a non-renewable on-time budget, and
//! the timer interrupt turns the emitters off if the probe stops renewing
//! its owner lease (one `observe` per sample).
//!
//! Sequence (plan 2): a baseline window with both emitters off, then
//! `CYCLES` cycles of PA15 alone, PB3 alone and both, each followed by an
//! off window, every window 5 s so an operator can read a meter in each.
//! Plan 1 (2/2/4 s windows, 4 s off, six cycles) ran the acceptance. Every
//! `POLL_MS` the probe reads PA16..PA19 in one port access and records, per
//! window, high counts, the time to each input's first change (settling),
//! the shortest complete LOW run (pulse capture) and up to `EDGE_CAP` edges.
//! Food and outlet states come from the stock-derived filter in
//! `SensorInputs`, qualified only while both emitters are on. Records are
//! logged at least `QUIET_MS` into the next window, so logging never delays
//! the settling samples.
//!
//! The probe ends with a deliberate owner stall: both emitters on, then the
//! task spins without renewing its lease. The timer interrupt must cut the
//! emitters about one lease (`LEASE_MS`) after the last renewal.

use core::fmt;

use wellfed_core::feed::controller::{FeedController, FeedLimits};
use wellfed_core::feed::sensor_inputs::{SensorFilter, SensorInputs, SensorLimits, SensorReading};
use wellfed_core::flash_coord::{FlashCoord, FlashReply};

use crate::{elapsed_ms, ticks_ms};
use wellfed_hal::board::{self, Board, MOTOR_MASK};
use wellfed_hal::flash::park::{self, Mmio, Reply};
use wellfed_hal::flash_lock::provider::BOOT_COUNT_REGISTER;
use wellfed_hal::plaf103;
use wellfed_hal::sdk::{self, GPIO_PORT_A};
use wellfed_hal::stopped;
use wellfed_hal::supervision::config::{FLASH_LIMITS, SYSTIMER_HZ};
use wellfed_hal::supervision::events::{Events, EventsConfig, EventsFault, EventsSnapshot};

pub const POLL_MS: u32 = 2;
/// Handshake polling before the supervisor (and its clock) is open.
pub const WAIT_MS: u32 = 10;
pub const READY_TIMEOUT_MS: u32 = 2000;

const DEADTIME_MS: u32 = 50;
/// Motor lease, and with it the owner lease the timer interrupt enforces.
pub const LEASE_MS: u32 = 300;
pub const ADC_POLLS: u32 = 10_000;

pub const EVENTS: EventsConfig = EventsConfig {
    ticks_per_second: SYSTIMER_HZ,
    timer_period_ms: 10,
    max_feedback_age_ms: 100,
    feedback_capacity: 8,
};

/// Stock: 100 ticks of settling after enable, 21 bad samples at nominal
/// 50 ms polling (about 1 s) before a bad state.
pub const SENSORS: SensorLimits = SensorLimits {
    optical_settle_ms: 100,
    food_empty_delay_ms: 1000,
    outlet_block_delay_ms: 1000,
    power_settle_ms: 100,
    max_observation_gap_ms: 100,
    max_age_ms: 100,
    battery_min_scaled: 1650,
};

/// Never started: `observe` needs an idle controller.
pub const FEED: FeedLimits = FeedLimits {
    feedback_inhibit_ms: 200,
    feedback_timeout_ms: 3000,
    direction_pause_ms: 500,
    reverse_ms: 500,
    max_duration_ms: 10_000,
    portion_ms: 10_000,
    max_observation_gap_ms: 100,
    mark_gap_ms: 3000,
    mark_low_ms: 3000,
    current_limit_ma: 300,
    current_max_ma: 1000,
    surge_ms: 0,
    max_portions: 1,
    max_reverse_attempts: 0,
};

pub const BASELINE_MS: u32 = 5000;
/// One cycle's emitter windows, each followed by `OFF_MS` with both off.
pub const CYCLE: [(Emitters, u32); 3] = [
    (Emitters::A, 5000),
    (Emitters::B, 5000),
    (Emitters::Both, 5000),
];
pub const OFF_MS: u32 = 5000;
pub const CYCLES: u32 = 3;
/// Announced in the start record and bound to the startup policy.
pub const PLAN: u32 = 2;
#[expect(clippy::cast_possible_truncation, reason = "a handful of cycle steps")]
pub const WINDOWS: u32 = 1 + CYCLES * 2 * CYCLE.len() as u32;

/// Budget over a window's length; the probe turns the emitters off first.
pub const BUDGET_MARGIN_MS: u32 = 500;
pub const QUIET_MS: u32 = 300;
pub const EDGE_CAP: usize = 16;

pub const STALL_WARM_MS: u32 = 500;
pub const STALL_MS: u32 = 1000;
pub const STALL_BUDGET_MS: u32 = 4000;

/// Inputs in record order.
pub const INPUTS: [&str; 3] = ["food", "outlet", "feedback"];
const INPUT_PINS: [u32; 3] = [
    plaf103::pins::FOOD,
    plaf103::pins::OUTLET,
    plaf103::pins::FEEDBACK,
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Emitters {
    #[default]
    Off,
    /// PA15 only.
    A,
    /// PB3 only.
    B,
    Both,
}

impl Emitters {
    pub fn levels(self) -> (bool, bool) {
        match self {
            Self::Off => (false, false),
            Self::A => (true, false),
            Self::B => (false, true),
            Self::Both => (true, true),
        }
    }

    /// Bit 0 PA15, bit 1 PB3, as `stopped::set_supervised` takes it.
    pub fn mask(self) -> u32 {
        let (a, b) = self.levels();
        u32::from(a) | u32::from(b) << 1
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::A => "a",
            Self::B => "b",
            Self::Both => "both",
        }
    }
}

/// A filtered state as `SensorInputs` holds it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum State {
    #[default]
    Unknown,
    Good,
    Bad,
}

impl State {
    fn of(filter: &SensorFilter) -> Self {
        match (filter.known, filter.good) {
            (false, _) => Self::Unknown,
            (true, true) => Self::Good,
            (true, false) => Self::Bad,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Window {
    pub n: u32,
    pub emitters: Emitters,
    pub start_ms: u32,
    pub len_ms: u32,
    pub samples: u32,
    pub max_gap_ms: u32,
    pub highs: [u32; 3],
    pub begin: [bool; 3],
    pub end: [bool; 3],

    /// From the window start to each input's first change.
    pub first_change: [Option<u32>; 3],
    /// Shortest LOW run that both began and ended inside the window.
    pub low_min: [Option<u32>; 3],
    pub edges: u32,
    pub dropped: u32,

    /// Stock-derived labels: food Good is food present, outlet Good is clear.
    pub food: State,
    pub outlet: State,
    /// Every sample read motor STOP and exactly the selected emitters.
    pub stopped: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Edge {
    pub n: u32,
    pub input: u8,
    pub level: bool,
    /// From the window start.
    pub ms: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// The supervisor latched a fault (for example a lost owner lease).
    Supervisor,
    Clock,
    Capture,
    Observe,
    Optics,
}

impl ErrorKind {
    fn name(self) -> &'static str {
        match self {
            Self::Supervisor => "supervisor",
            Self::Clock => "clock",
            Self::Capture => "capture",
            Self::Observe => "observe",
            Self::Optics => "optics",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    Open {
        board: bool,
        ready: bool,
        attached: bool,
        events: bool,
        inputs: bool,
        ms: u32,
    },

    /// No READY from the KM0 peer.
    Timeout {
        ms: u32,
    },
    /// A finished window's timing, filtered states and stopped readback.
    Window(Window),
    /// The same window's per-input statistics.
    Inputs(Window),
    Edge(Edge),

    Stall {
        /// From the last lease renewal to emitters read OFF.
        cut_ms: Option<u32>,
        fault: u32,
        off: bool,
        stopped: bool,
        /// The supervisor closed with STOP and OFF verified.
        closed: bool,
    },
    Done {
        windows: u32,
    },
    Error {
        kind: ErrorKind,
        n: u32,
        fault: u32,
        /// Milliseconds since the last good sample: how long the task was
        /// away when the owner lease (300 ms) ran out. `None` without a
        /// readable clock.
        stall_ms: Option<u32>,
    },
}

fn bits(levels: [bool; 3]) -> [u8; 3] {
    levels.map(u8::from)
}

struct Times([Option<u32>; 3]);

impl fmt::Display for Times {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, value) in self.0.iter().enumerate() {
            if i != 0 {
                f.write_str(",")?;
            }
            match value {
                Some(ms) => write!(f, "{ms}")?,
                None => f.write_str("-")?,
            }
        }
        Ok(())
    }
}

impl fmt::Display for Record {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        describe(*self, f)
    }
}

/// The body of a `PLOPT1` line.
pub fn describe(record: Record, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match record {
        Record::Open {
            board,
            ready,
            attached,
            events,
            inputs,
            ms,
        } => write!(
            f,
            "open board={} ready={} attached={} events={} inputs={} ms={ms}",
            u8::from(board),
            u8::from(ready),
            u8::from(attached),
            u8::from(events),
            u8::from(inputs)
        ),

        Record::Timeout { ms } => write!(f, "timeout ms={ms}"),
        Record::Window(w) => {
            let food = match w.food {
                State::Unknown => "unknown",
                State::Good => "present",
                State::Bad => "empty",
            };
            let outlet = match w.outlet {
                State::Unknown => "unknown",
                State::Good => "clear",
                State::Bad => "blocked",
            };

            write!(
                f,
                "window n={} emitters={} start={} len={} samples={} gap={} edges={} dropped={} \
                 food={food} outlet={outlet} stopped={}",
                w.n,
                w.emitters.name(),
                w.start_ms,
                w.len_ms,
                w.samples,
                w.max_gap_ms,
                w.edges,
                w.dropped,
                u8::from(w.stopped)
            )
        }

        Record::Inputs(w) => {
            let [b0, b1, b2] = bits(w.begin);
            let [e0, e1, e2] = bits(w.end);
            write!(
                f,
                "inputs n={} highs={},{},{} begin={b0}{b1}{b2} end={e0}{e1}{e2} first={} low_min={}",
                w.n,
                w.highs[0],
                w.highs[1],
                w.highs[2],
                Times(w.first_change),
                Times(w.low_min),
            )
        }

        Record::Edge(e) => write!(
            f,
            "edge n={} input={} level={} ms={}",
            e.n,
            INPUTS[usize::from(e.input)],
            u8::from(e.level),
            e.ms
        ),

        Record::Stall {
            cut_ms,
            fault,
            off,
            stopped,
            closed,
        } => {
            f.write_str("stall cut=")?;
            match cut_ms {
                Some(ms) => write!(f, "{ms}")?,
                None => f.write_str("none")?,
            }
            write!(
                f,
                " fault={fault} off={} stopped={} closed={}",
                u8::from(off),
                u8::from(stopped),
                u8::from(closed)
            )
        }

        Record::Done { windows } => write!(f, "done windows={windows}"),
        Record::Error {
            kind,
            n,
            fault,
            stall_ms,
        } => {
            write!(f, "error kind={} n={n} fault={fault} stall=", kind.name())?;
            match stall_ms {
                Some(ms) => write!(f, "{ms}"),
                None => f.write_str("-"),
            }
        }
    }
}

/// `delay_ms == 0` ends the probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub delay_ms: u32,
    pub record: Option<Record>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Stage {
    #[default]
    Start,
    Wait,
    Run,
    Warm,
    Stall,
    Done,
}

/// The emitters and length of window `n`.
pub fn plan(n: u32) -> (Emitters, u32) {
    if n == 0 {
        return (Emitters::Off, BASELINE_MS);
    }
    let i = (n - 1) as usize % (2 * CYCLE.len());
    if i % 2 == 1 {
        (Emitters::Off, OFF_MS)
    } else {
        CYCLE[i / 2]
    }
}

/// Keep at a fixed address once started: the board, its authority and the
/// supervisor refer to each other.
#[derive(Debug, Default)]
pub struct Probe {
    pub board: Board,
    pub events: Events,
    pub flash: Option<FlashCoord>,
    pub inputs: SensorInputs,
    pub controller: FeedController,
    stage: Stage,
    authority: u64,
    begin_raw: u32,
    window: Window,
    last_ms: u32,
    low_since: [Option<u32>; 3],
    seeded: bool,
    edges: [Edge; EDGE_CAP],
    /// Records of the last finished window: its two summaries, then its edges.
    pending: [Option<Record>; EDGE_CAP + 2],
    pending_len: usize,
    pending_next: usize,
}

fn next(delay_ms: u32, record: Option<Record>) -> Step {
    Step { delay_ms, record }
}

fn levels(pins: u32) -> [bool; 3] {
    INPUT_PINS.map(|pin| pins & (1 << pin) != 0)
}

impl Probe {
    /// The window under way (the stall's warm-up reuses the last one).
    pub fn window(&self) -> &Window {
        &self.window
    }

    fn end(&mut self, record: Record) -> Step {
        self.stage = Stage::Done;
        next(0, Some(record))
    }

    /// Emitters off and the supervisor closed (or, if it cannot verify
    /// that, left retrying from its timer); then the error record.
    fn fail(&mut self, kind: ErrorKind) -> Step {
        let fault = self.events.view().fault.code();
        let stall_ms = self
            .events
            .now_ms()
            .map(|now| now.wrapping_sub(self.last_ms));
        if self.events.opened() {
            self.events.close();
        } else {
            self.board.close();
        }

        stopped::set_supervised(0);
        self.end(Record::Error {
            kind,
            n: self.window.n,
            fault,
            stall_ms,
        })
    }

    /// One task iteration in task context, interrupts enabled.
    pub fn step<M: Mmio>(&mut self) -> Step {
        match self.stage {
            Stage::Start => {
                stopped::set_supervised(0);
                self.begin_raw = sdk::systimer_ticks();
                if !self.board.open(DEADTIME_MS, LEASE_MS, ADC_POLLS, 0) {
                    return self.end(self.open_record(false, false, false, false, false));
                }

                let count = sdk::backup_read(BOOT_COUNT_REGISTER).wrapping_add(1);
                sdk::backup_write(BOOT_COUNT_REGISTER, count);
                self.authority = park::session(count, self.begin_raw, false);
                if !park::hello::<M>(self.authority) {
                    self.board.close();
                    return self.end(self.open_record(true, false, false, false, false));
                }

                self.stage = Stage::Wait;
                next(WAIT_MS, None)
            }

            Stage::Wait => self.wait::<M>(),
            Stage::Run => self.run(),
            Stage::Warm => self.warm(),
            Stage::Stall => self.stall(),
            Stage::Done => match self.take_pending() {
                Some(record) => next(POLL_MS, Some(record)),
                None => next(0, None),
            },
        }
    }

    fn open_record(
        &self,
        board: bool,
        ready: bool,
        attached: bool,
        events: bool,
        inputs: bool,
    ) -> Record {
        Record::Open {
            board,
            ready,
            attached,
            events,
            inputs,
            ms: elapsed_ms(self.begin_raw),
        }
    }

    fn wait<M: Mmio>(&mut self) -> Step {
        let ms = elapsed_ms(self.begin_raw);
        let peer = match park::reply::<M>(self.authority) {
            Reply::Ready(peer) => peer,
            _ if ms >= READY_TIMEOUT_MS => {
                self.board.close();
                return self.end(Record::Timeout { ms });
            }
            _ => return next(WAIT_MS, None),
        };

        // Bind only with verified STOP and infrared OFF readback.
        let inactive = stopped::observe().verified();
        let ready = FlashCoord::new(&FLASH_LIMITS, self.authority, ms).and_then(|mut coord| {
            (coord.ready(self.authority, peer, inactive, ms) == FlashReply::Accepted)
                .then_some(coord)
        });

        self.flash = ready;
        let flash = self
            .flash
            .as_mut()
            .map_or(core::ptr::null_mut(), |c| &raw mut *c);
        // SAFETY: the authority lives in this fixed-address probe, under the
        // board's IRQ serialization, for the board's lifetime.
        let attached = !flash.is_null() && unsafe { self.board.attach_flash(flash) };
        let board = &raw mut self.board;
        // SAFETY: the board stays at this fixed address while events is open.
        let events = attached && unsafe { self.events.open(board, &EVENTS) };

        let inputs = SensorInputs::new(&SENSORS, &self.board.view().calibration);
        let controller = FeedController::new(&FEED, 0);
        let ok = events && inputs.is_some() && controller.is_some();
        let record = self.open_record(true, self.flash.is_some(), attached, events, ok);
        if !ok {
            if events {
                self.events.close();
            } else {
                self.board.close();
            }
            return self.end(record);
        }

        self.inputs = inputs.unwrap_or_default();
        self.controller = controller.unwrap_or_default();
        let Some(now) = self.events.now_ms() else {
            return self.fail(ErrorKind::Clock);
        };

        self.begin_window(0, now, None);
        self.stage = Stage::Run;
        next(POLL_MS, Some(record))
    }

    /// `levels`: the inputs read just before this window's emitters were
    /// applied, so a change in its first poll still counts as one.
    fn begin_window(&mut self, n: u32, now: u32, levels: Option<[bool; 3]>) {
        let (emitters, len_ms) = plan(n);
        self.window = Window {
            n,
            emitters,
            start_ms: now,
            len_ms,
            begin: levels.unwrap_or_default(),
            end: levels.unwrap_or_default(),
            stopped: true,
            ..Window::default()
        };
        self.seeded = levels.is_some();
        self.last_ms = now;
        self.low_since = [None; 3];
    }

    /// Apply window `n`'s emitters; publish them as supervised only while
    /// their budget is armed.
    fn switch(&mut self, n: u32) -> bool {
        let (emitters, len_ms) = plan(n);
        let (a, b) = emitters.levels();
        if emitters == Emitters::Off {
            let ok = self.events.optics(false, 0);
            stopped::set_supervised(0);
            return ok;
        }

        stopped::set_supervised(emitters.mask());
        let ok = self.events.optics_select(a, b, len_ms + BUDGET_MARGIN_MS);
        if !ok {
            stopped::set_supervised(0);
        }
        ok
    }

    /// Sample once, renew the owner lease, and return the clock and inputs.
    fn sample(&mut self) -> Result<(u32, u32), ErrorKind> {
        if self.events.view().fault != EventsFault::Ok {
            return Err(ErrorKind::Supervisor);
        }

        let now = self.events.now_ms().ok_or(ErrorKind::Clock)?;
        let mut reading = SensorReading::default();
        let pins = self
            .board
            .capture_gpio(&mut reading)
            .ok_or(ErrorKind::Capture)?;

        self.inputs.update(now, Some(&reading));
        if !self.events.observe(&mut self.controller, &self.inputs) {
            return Err(ErrorKind::Observe);
        }

        Ok((now, pins))
    }

    fn record_sample(&mut self, now: u32, pins: u32) {
        let w = &mut self.window;
        let at = now.wrapping_sub(w.start_ms);
        let levels = levels(pins);
        if !self.seeded {
            // A run already LOW at the start is not complete: no `low_since`.
            self.seeded = true;
            w.begin = levels;
            w.end = levels;
        }

        w.max_gap_ms = w.max_gap_ms.max(now.wrapping_sub(self.last_ms));
        self.last_ms = now;
        w.samples += 1;
        let (a, b) = w.emitters.levels();
        w.stopped &=
            sdk::gpio_port_read(GPIO_PORT_A, MOTOR_MASK) == MOTOR_MASK && board::optics_are(a, b);

        for ((i, &high), input) in levels.iter().enumerate().zip(0u8..) {
            w.highs[i] += u32::from(high);
            if high == w.end[i] {
                continue;
            }
            if w.first_change[i].is_none() && high != w.begin[i] {
                w.first_change[i] = Some(at);
            }

            if high {
                if let Some(since) = self.low_since[i].take() {
                    let run = now.wrapping_sub(since);
                    w.low_min[i] = Some(w.low_min[i].map_or(run, |m| m.min(run)));
                }
            } else {
                self.low_since[i] = Some(now);
            }

            w.end[i] = high;
            let index = w.edges as usize;
            w.edges += 1;
            if index < EDGE_CAP {
                self.edges[index] = Edge {
                    n: w.n,
                    input,
                    level: high,
                    ms: at,
                };
            } else {
                w.dropped += 1;
            }
        }
    }

    fn take_pending(&mut self) -> Option<Record> {
        if self.pending_next >= self.pending_len {
            return None;
        }
        let record = self.pending[self.pending_next].take();
        self.pending_next += 1;
        record
    }

    fn finish_window(&mut self, now: u32) {
        self.window.len_ms = now.wrapping_sub(self.window.start_ms);
        self.window.food = State::of(&self.inputs.food);
        self.window.outlet = State::of(&self.inputs.outlet);
        self.pending[0] = Some(Record::Window(self.window));
        self.pending[1] = Some(Record::Inputs(self.window));

        let kept = (self.window.edges as usize).min(EDGE_CAP);
        for (slot, edge) in self.pending[2..kept + 2].iter_mut().zip(self.edges) {
            *slot = Some(Record::Edge(edge));
        }
        self.pending_len = kept + 2;
        self.pending_next = 0;
    }

    fn run(&mut self) -> Step {
        let (now, pins) = match self.sample() {
            Ok(sample) => sample,
            Err(kind) => return self.fail(kind),
        };

        self.record_sample(now, pins);
        let age = now.wrapping_sub(self.window.start_ms);
        if age >= self.window.len_ms {
            // Never leave the last window's records behind.
            if let Some(record) = self.take_pending() {
                return next(POLL_MS, Some(record));
            }

            self.finish_window(now);
            let n = self.window.n + 1;
            if n == WINDOWS {
                self.stage = Stage::Warm;
                if !self.switch_both(STALL_BUDGET_MS) {
                    return self.fail(ErrorKind::Optics);
                }
                self.window.start_ms = now;
                return next(POLL_MS, None);
            }

            if !self.switch(n) {
                return self.fail(ErrorKind::Optics);
            }
            self.begin_window(n, now, Some(levels(pins)));
            return next(POLL_MS, None);
        }

        let record = if age >= QUIET_MS {
            self.take_pending()
        } else {
            None
        };
        next(POLL_MS, record)
    }

    fn switch_both(&mut self, budget_ms: u32) -> bool {
        stopped::set_supervised(Emitters::Both.mask());
        let ok = self.events.optics(true, budget_ms);
        if !ok {
            stopped::set_supervised(0);
        }
        ok
    }

    /// Both emitters on with the lease renewed, then the stall.
    fn warm(&mut self) -> Step {
        let now = match self.sample() {
            Ok((now, _)) => now,
            Err(kind) => return self.fail(kind),
        };
        let record = self.take_pending();
        if record.is_none() && now.wrapping_sub(self.window.start_ms) >= STALL_WARM_MS {
            self.stage = Stage::Stall;
        }
        next(POLL_MS, record)
    }

    /// Renew once, then keep the CPU without renewing; only interrupts run.
    fn stall(&mut self) -> Step {
        if let Err(kind) = self.sample() {
            return self.fail(kind);
        }

        let renewed = sdk::systimer_ticks();
        let ticks = (STALL_MS * SYSTIMER_HZ).div_ceil(1000);
        let mut cut = None;

        loop {
            let raw = sdk::systimer_ticks();
            if cut.is_none() && board::optics_off() {
                cut = Some(raw.wrapping_sub(renewed));
            }
            if raw.wrapping_sub(renewed) >= ticks {
                break;
            }
            sdk::spin_once();
        }

        let mut snapshot = EventsSnapshot::default();
        self.events.snapshot(&mut snapshot);
        stopped::set_supervised(0);
        let closed = self.events.close();

        let cut_ms = cut.map(ticks_ms);
        let record = Record::Stall {
            cut_ms,
            fault: snapshot.fault.code(),
            off: board::optics_off(),
            stopped: snapshot.stopped && sdk::gpio_port_read(GPIO_PORT_A, MOTOR_MASK) == MOTOR_MASK,
            closed,
        };

        self.pending = [None; EDGE_CAP + 2];
        self.pending[0] = Some(Record::Done { windows: WINDOWS });
        self.pending_len = 1;
        self.pending_next = 0;
        self.stage = Stage::Done;
        next(POLL_MS, Some(record))
    }
}
