//! FW16 first supervised motor pulse, built only as the `fw16-pulse` variant
//! (feature `fw16-pulse`, policy `fw16-pulse-v1`). Never the default image.
//!
//! Opens the board (motor STOP, emitters OFF, sensors), binds a fresh flash
//! authority to the KM0 peer and opens the supervisor, exactly as the FW13
//! probe does. It then waits `DELAY_MS` renewing its owner lease, so the
//! operator can be ready, and drives once: `DIRECTION` for `PULSE_MS`, then
//! STOP, then watches for `AFTER_MS`. Polling every `POLL_MS`, it counts PA18
//! feedback edges and samples motor current (ADC channel 4).
//!
//! Bounds that hold even if this task stalls: the motor gate's command lease
//! (`LEASE_MS`) and the owner lease, both enforced by the TIM2 interrupt.
//! A drive is applied only by `Events::motor`, which requires a running
//! timer, a live owner and enabled feedback capture.
//!
//! One pulse per cold boot: any nonzero reset reason, or the marker this
//! probe sets in a backup register before driving, skips the pulse. The
//! marker survives a watchdog reset but not power loss.

use core::fmt;

use petlibre_core::adc_scale::SAMPLES;
use petlibre_core::feed::controller::{FeedController, FeedSample, MotorCommand};
use petlibre_core::feed::sensor_inputs::SensorReading;
use petlibre_core::flash_coord::{FlashCoord, FlashReply};

use crate::elapsed_ms;
use crate::optics::FEED;
use petlibre_hal::board::{Board, MOTOR_MASK};
use petlibre_hal::flash::park::{self, Mmio, Reply};
use petlibre_hal::flash_lock::provider::BOOT_COUNT_REGISTER;
use petlibre_hal::sdk::{self, GPIO_PORT_A};
use petlibre_hal::stopped;
use petlibre_hal::supervision::config::{FLASH_LIMITS, SYSTIMER_HZ};
use petlibre_hal::supervision::events::{Events, EventsConfig, EventsFault};

pub const POLL_MS: u32 = 2;
pub const WAIT_MS: u32 = 10;
pub const READY_TIMEOUT_MS: u32 = 2000;

const DEADTIME_MS: u32 = 50;
/// Motor command lease and owner lease: a stalled task stops the motor
/// within this, plus one timer period.
pub const LEASE_MS: u32 = 200;
pub const ADC_POLLS: u32 = 10_000;

pub const EVENTS: EventsConfig = EventsConfig {
    ticks_per_second: SYSTIMER_HZ,
    timer_period_ms: 10,
    max_feedback_age_ms: 100,
    feedback_capacity: 8,
};

pub const DELAY_MS: u32 = 60_000;
pub const DIRECTION: MotorCommand = MotorCommand::DRIVE_0;
pub const PULSE_MS: u32 = 100;
pub const AFTER_MS: u32 = 1000;
/// Current is sampled at most this often (16 conversions take about 5 ms).
pub const CURRENT_PERIOD_MS: u32 = 10;

/// Written to the user backup register before driving. It survives a
/// watchdog reset but not power loss, so the pulse runs once per cold boot.
pub const MARKER: u32 = 0x5046_5716;
const FEEDBACK: u32 = sdk::board::bit(sdk::board::FEEDBACK);

/// The idle controller's view; only owner-lease renewal reads it.
const IDLE: FeedSample = FeedSample {
    power_healthy: true,
    optical_valid: true,
    outlet_clear: true,
    current_valid: true,
    current_ma: 0,
    feedback_edge: false,
    feedback_high: false,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Current {
    pub samples: u32,
    pub min: u32,
    pub max: u32,
    pub sum: u64,
}

impl Current {
    fn add(&mut self, value: u32) {
        if self.samples == 0 {
            self.min = value;
        }
        self.samples += 1;
        self.min = self.min.min(value);
        self.max = self.max.max(value);
        self.sum += u64::from(value);
    }

    pub fn mean(&self) -> u32 {
        if self.samples == 0 {
            0
        } else {
            // The mean of u32 samples fits a u32.
            u32::try_from(self.sum / u64::from(self.samples)).unwrap_or(u32::MAX)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    Skip {
        reset: u32,
        marker: u32,
    },
    Open {
        board: bool,
        ready: bool,
        attached: bool,
        events: bool,
        capture: bool,
        ms: u32,
    },
    Timeout {
        ms: u32,
    },

    /// The pulse is armed and will start after `delay_ms`.
    Armed {
        delay_ms: u32,
    },
    Pulse {
        direction: u32,
        /// From the request to the verified drive (deadtime included).
        applied_ms: u32,
        /// From the verified drive to the verified STOP.
        on_ms: u32,
        edges: u32,
        first_edge_ms: Option<u32>,
        current: Current,
    },

    After {
        edges: u32,
        current: Current,
        stopped: bool,
    },
    Done,
    Error {
        stage: &'static str,
        fault: u32,
        stopped: bool,
    },
}

struct OptMs(Option<u32>);

impl fmt::Display for OptMs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(ms) => write!(f, "{ms}"),
            None => f.write_str("none"),
        }
    }
}

impl fmt::Display for Record {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        describe(*self, f)
    }
}

/// The body of a `PLMOT1` line.
pub fn describe(record: Record, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let bit = u8::from;
    match record {
        Record::Skip { reset, marker } => write!(f, "skip reset={reset:08x} marker={marker:08x}"),
        Record::Open {
            board,
            ready,
            attached,
            events,
            capture,
            ms,
        } => write!(
            f,
            "open board={} ready={} attached={} events={} capture={} ms={ms}",
            bit(board),
            bit(ready),
            bit(attached),
            bit(events),
            bit(capture)
        ),

        Record::Timeout { ms } => write!(f, "timeout ms={ms}"),
        Record::Armed { delay_ms } => write!(
            f,
            "armed delay_ms={delay_ms} pulse_ms={PULSE_MS} lease_ms={LEASE_MS}"
        ),

        Record::Pulse {
            direction,
            applied_ms,
            on_ms,
            edges,
            first_edge_ms,
            current: c,
        } => write!(
            f,
            "pulse dir={direction} applied_ms={applied_ms} on_ms={on_ms} edges={edges} first_edge_ms={} current={},{},{} samples={}",
            OptMs(first_edge_ms),
            c.min,
            c.max,
            c.mean(),
            c.samples
        ),

        Record::After {
            edges,
            current: c,
            stopped,
        } => write!(
            f,
            "after ms={AFTER_MS} edges={edges} current={},{},{} stopped={}",
            c.min,
            c.max,
            c.mean(),
            bit(stopped)
        ),

        Record::Done => f.write_str("done"),
        Record::Error {
            stage,
            fault,
            stopped,
        } => write!(
            f,
            "error stage={stage} fault={fault} stopped={}",
            bit(stopped)
        ),
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
    Delay,
    Driving,
    After,
    Done,
}

#[derive(Debug, Default)]
pub struct Probe {
    pub board: Board,
    pub events: Events,
    pub flash: Option<FlashCoord>,
    pub controller: FeedController,
    stage: Stage,
    authority: u64,
    begin_raw: u32,
    armed_at: u32,
    requested_at: u32,
    applied_at: Option<u32>,
    stopped_at: u32,
    last_current_at: Option<u32>,
    feedback_high: bool,
    edges: u32,
    first_edge_ms: Option<u32>,
    current: Current,
    pulse: Option<Record>,
}

fn next(delay_ms: u32, record: Option<Record>) -> Step {
    Step { delay_ms, record }
}

fn motor_pins_stopped() -> bool {
    sdk::gpio_port_read(GPIO_PORT_A, MOTOR_MASK) == MOTOR_MASK
}

impl Probe {
    fn end(&mut self, record: Record) -> Step {
        self.stage = Stage::Done;
        self.pulse = None;
        next(0, Some(record))
    }

    /// STOP and close the supervisor (or leave its timer retrying STOP).
    fn fail(&mut self, stage: &'static str) -> Step {
        let fault = self.events.fault.0;
        if self.events.opened {
            let _ = self.events.motor(MotorCommand::STOP);
            self.events.close();
        } else {
            self.board.close();
        }

        let stopped = motor_pins_stopped();
        self.end(Record::Error {
            stage,
            fault,
            stopped,
        })
    }

    pub fn step<M: Mmio>(&mut self) -> Step {
        match self.stage {
            Stage::Start => self.start::<M>(),
            Stage::Wait => self.wait::<M>(),
            Stage::Delay => self.delay(),
            Stage::Driving => self.driving(),
            Stage::After => self.after(),
            // `after` leaves `Done` to log on its own line, then the probe ends.
            Stage::Done => next(0, self.pulse.take()),
        }
    }

    fn start<M: Mmio>(&mut self) -> Step {
        let reset = sdk::boot_reason();
        let marker = sdk::backup_read(sdk::BACKUP_USER);
        if reset != 0 || marker != 0 {
            sdk::backup_write(sdk::BACKUP_USER, 0);
            return self.end(Record::Skip { reset, marker });
        }

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

    fn open_record(
        &self,
        board: bool,
        ready: bool,
        attached: bool,
        events: bool,
        capture: bool,
    ) -> Record {
        Record::Open {
            board,
            ready,
            attached,
            events,
            capture,
            ms: elapsed_ms(self.begin_raw),
        }
    }

    /// Renew the owner lease; true while the supervisor is healthy.
    fn renew(&mut self) -> bool {
        self.events.fault == EventsFault::OK
            && self.events.process(&mut self.controller, Some(&IDLE))
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

        let inactive = stopped::observe().verified();
        self.flash = FlashCoord::new(&FLASH_LIMITS, self.authority, ms).and_then(|mut coord| {
            (coord.ready(self.authority, peer, inactive, ms) == FlashReply::ACCEPTED)
                .then_some(coord)
        });

        let flash = self
            .flash
            .as_mut()
            .map_or(core::ptr::null_mut(), |c| &raw mut *c);
        // SAFETY: the authority lives in this fixed-address probe for the board's lifetime.
        let attached = !flash.is_null() && unsafe { self.board.attach_flash(flash) };
        let board = &raw mut self.board;
        // SAFETY: the board stays at this fixed address while events is open.
        let events = attached && unsafe { self.events.open(board, &EVENTS) };

        let controller = FeedController::new(&FEED, 0);
        self.controller = controller.unwrap_or_default();

        // A timer interrupt must be seen before capture can be enabled.
        let mut capture = false;
        if events && controller.is_some() {
            let begin = sdk::systimer_ticks();
            let ticks = (3 * EVENTS.timer_period_ms * SYSTIMER_HZ).div_ceil(1000);
            while sdk::systimer_ticks().wrapping_sub(begin) < ticks && !self.events.timer_seen {
                sdk::spin_once();
            }
            capture = self.renew() && self.events.capture(true);
        }

        let record = self.open_record(true, self.flash.is_some(), attached, events, capture);
        if !capture {
            if events {
                self.events.close();
            } else {
                self.board.close();
            }
            return self.end(record);
        }

        self.armed_at = self.events.now_ms().unwrap_or_default();
        self.feedback_high = sdk::gpio_port_read(GPIO_PORT_A, FEEDBACK) != 0;
        self.stage = Stage::Delay;
        self.pulse = Some(Record::Armed { delay_ms: DELAY_MS });
        next(POLL_MS, Some(record))
    }

    fn delay(&mut self) -> Step {
        if let Some(record) = self.pulse.take() {
            return next(POLL_MS, Some(record));
        }

        if !self.renew() {
            return self.fail("delay");
        }
        let Some(now) = self.events.now_ms() else {
            return self.fail("clock");
        };
        if now.wrapping_sub(self.armed_at) < DELAY_MS {
            return next(POLL_MS, None);
        }

        // One pulse per cold boot: mark before driving.
        sdk::backup_write(sdk::BACKUP_USER, MARKER);
        if sdk::backup_read(sdk::BACKUP_USER) != MARKER {
            return self.fail("marker");
        }

        self.requested_at = now;
        if !self.events.motor(DIRECTION) {
            return self.fail("drive");
        }
        self.stage = Stage::Driving;
        next(POLL_MS, None)
    }

    fn sample(&mut self, now: u32, since: u32) -> bool {
        let mut reading = SensorReading::default();
        let Some(pins) = self.board.capture_gpio(&mut reading) else {
            return false;
        };

        let high = pins & FEEDBACK != 0;
        if high != self.feedback_high {
            self.feedback_high = high;
            self.edges += 1;
            if self.first_edge_ms.is_none() {
                self.first_edge_ms = Some(now.wrapping_sub(since));
            }
        }

        if self
            .last_current_at
            .is_none_or(|at| now.wrapping_sub(at) >= CURRENT_PERIOD_MS)
        {
            let mut samples = [0u16; SAMPLES];
            if !self
                .board
                .sample_channel(sdk::board::ADC_CURRENT, &mut samples)
            {
                return false;
            }
            if let Some(value) = self.board.calibration.scale_16(&samples) {
                self.current.add(value);
            }
            self.last_current_at = Some(now);
        }

        true
    }

    fn driving(&mut self) -> Step {
        let Some(now) = self.events.now_ms() else {
            return self.fail("clock");
        };
        if !self.renew() {
            return self.fail("driving");
        }

        let gate = self.board.motor;
        if self.applied_at.is_none() && gate.applied == DIRECTION && gate.applied_known {
            self.applied_at = Some(gate.applied_at);
        }

        let Some(applied) = self.applied_at else {
            if now.wrapping_sub(self.requested_at) > DEADTIME_MS + 2 * EVENTS.timer_period_ms {
                return self.fail("apply");
            }
            return next(POLL_MS, None);
        };

        if !self.sample(now, applied) {
            return self.fail("sample");
        }
        if now.wrapping_sub(applied) < PULSE_MS {
            return next(POLL_MS, None);
        }

        if !self.events.motor(MotorCommand::STOP) || !motor_pins_stopped() {
            return self.fail("stop");
        }

        self.stopped_at = self.board.motor.applied_at;
        let record = Record::Pulse {
            direction: DIRECTION.0,
            applied_ms: applied.wrapping_sub(self.requested_at),
            on_ms: self.stopped_at.wrapping_sub(applied),
            edges: self.edges,
            first_edge_ms: self.first_edge_ms,
            current: self.current,
        };

        self.edges = 0;
        self.first_edge_ms = None;
        self.current = Current::default();
        self.last_current_at = None;
        self.stage = Stage::After;
        // Log after the stop, so logging never delays it.
        next(POLL_MS, Some(record))
    }

    fn after(&mut self) -> Step {
        let Some(now) = self.events.now_ms() else {
            return self.fail("clock");
        };
        if !self.renew() || !self.sample(now, self.stopped_at) {
            return self.fail("after");
        }
        if now.wrapping_sub(self.stopped_at) < AFTER_MS {
            return next(POLL_MS, None);
        }

        let stopped = motor_pins_stopped()
            && self.board.motor.applied == MotorCommand::STOP
            && self.events.close()
            && stopped::observe().verified();

        let record = Record::After {
            edges: self.edges,
            current: self.current,
            stopped,
        };

        self.stage = Stage::Done;
        self.pulse = Some(Record::Done);
        next(POLL_MS, Some(record))
    }
}
