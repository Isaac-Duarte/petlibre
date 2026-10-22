//! FW05 bench probe: SYSTIMER and TIM2 supervision timing, a task stall with
//! the timer still running, and one supervised watchdog reset. Motion stays
//! disabled: the board opens stopped and no drive or emitter is requested.
//!
//! EXPERIMENTAL. Opens only the board's outputs (motor STOP, emitters OFF; no
//! sensor inputs or ADC) and claims TIM2, the GPIOA vector and the KM4
//! watchdog. A backup-register marker is written before the
//! watchdog is armed; any boot that sees the marker or a nonzero boot reason
//! skips the whole probe, so a reset cannot repeat it until power is cut.
//! Clock rates are nominal; the bench capture measures them.

use wellfed_amebad::board::Board;
use wellfed_amebad::events::{Events, EventsConfig, EventsSnapshot};
use wellfed_amebad::sdk;
use wellfed_amebad::stopped;
use wellfed_amebad::watchdog::{Watchdog, WatchdogConfig};

/// Backup-register marker (`PFW5` in ASCII) written before the watchdog is armed.
pub const MARKER: u32 = 0x5046_5735;

/// SDK-documented 31 us SYSTIMER tick; TIM2 is assumed to share the 32 kHz source.
pub const TICKS_PER_SECOND: u32 = 32_768;
pub const EVENTS: EventsConfig = EventsConfig {
    ticks_per_second: TICKS_PER_SECOND,
    timer_period_ms: 10,
    max_feedback_age_ms: 100,
    feedback_capacity: 8,
};

/// Nominal 33.3 kHz watchdog clock (`WDG_Scalar`): 4095 x 32 counts is about
/// 3.9 s, bounded to 2.9-5.3 s by the wide clock limits.
pub const WATCHDOG: WatchdogConfig = WatchdogConfig {
    systimer_hz: TICKS_PER_SECOND,
    clock_min_hz: 25_000,
    clock_max_hz: 45_000,
    min_refresh_ms: 200,
    max_progress_gap_ms: 1000,
    max_expiry_ms: 6000,
    divisor: 31,
    count_id: 11,
};

pub const FIRST_SAMPLE_MS: u32 = 100;
pub const SAMPLE_MS: u32 = 5000;
pub const SAMPLES: u32 = 12;
pub const STALL_MS: u32 = 2000;
pub const FEED_MS: u32 = 15_000;
pub const FEED_PERIOD_MS: u32 = 250;
pub const STALLED_REPORT_MS: u32 = 1000;

/// Still running this long after the stall began means the watchdog failed.
pub const RESET_DEADLINE_MS: u32 = WATCHDOG.max_expiry_ms + 2000;

const DEADTIME_MS: u32 = 50;
const LEASE_MS: u32 = 300;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    /// A marker or boot reason was present; the probe did nothing else.
    Skip {
        reset: u32,
        marker: u32,
    },
    Open {
        board: bool,
        events: bool,
    },

    Sample(Sample),
    StallBegin(Sample),
    StallEnd(Sample),

    Arm {
        ok: bool,
        vendor: u32,
        marker: u32,
    },
    Fed {
        ms: u32,
        checkpoints: u32,
        ok: bool,
    },
    WatchdogStall {
        ms: u32,
    },

    Stalled {
        ms: u32,
        fault: u32,
        enabled: bool,
    },
    NoReset {
        ms: u32,
    },
    ClockError,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    pub n: u32,
    pub raw: u32,
    pub ms: u32,
    pub irqs: u32,
    pub ready: bool,
    pub healthy: bool,
    pub stopped: bool,
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
    Sample(u32),
    Stall,
    Arm,
    Feed,
    Stuck,
    Done,
}

/// Keep at a fixed address once started: the board, events and watchdog
/// register pointers to themselves.
#[derive(Debug, Default)]
pub struct Probe {
    pub board: Board,
    pub events: Events,
    pub watchdog: Watchdog,
    stage: Stage,
    progress: u64,
    checkpoints: u32,
    phase_at: u32,
    reported_at: u32,
    /// A record held for the next iteration, so each iteration logs at most one.
    pending: Option<Record>,
}

fn next(delay_ms: u32, record: Record) -> Step {
    Step {
        delay_ms,
        record: Some(record),
    }
}

impl Probe {
    fn end(&mut self, record: Record) -> Step {
        self.stage = Stage::Done;
        next(0, record)
    }

    fn sample(&mut self, n: u32) -> Option<Sample> {
        let mut snapshot = EventsSnapshot::default();
        let raw = sdk::systimer_ticks();
        if !self.events.snapshot(&mut snapshot) {
            return None;
        }

        Some(Sample {
            n,
            raw,
            ms: snapshot.now_ms,
            irqs: self.events.timer_count,
            ready: snapshot.timer_ready,
            healthy: snapshot.healthy,
            stopped: snapshot.stopped && snapshot.optics_off && stopped::observe().verified(),
        })
    }

    /// One task iteration. Requires task context with interrupts enabled.
    pub fn step(&mut self) -> Step {
        match self.stage {
            Stage::Start => {
                let reset = sdk::boot_reason();
                let marker = sdk::backup_read(sdk::BACKUP_USER);
                if reset != 0 || marker != 0 {
                    sdk::backup_write(sdk::BACKUP_USER, 0);
                    return self.end(Record::Skip { reset, marker });
                }

                let board = self.board.open_outputs(DEADTIME_MS, LEASE_MS, 0);
                // SAFETY: the probe, and with it the board, stays at a fixed address.
                let events = board && unsafe { self.events.open(&mut self.board, &EVENTS) };
                if !events {
                    return self.end(Record::Open { board, events });
                }

                self.stage = Stage::Sample(0);
                // Several timer periods, so every sample must see a ready timer.
                next(FIRST_SAMPLE_MS, Record::Open { board, events })
            }

            Stage::Sample(n) => {
                let Some(sample) = self.sample(n) else {
                    return self.end(Record::ClockError);
                };
                let (stage, delay) = if n < SAMPLES {
                    (Stage::Sample(n + 1), SAMPLE_MS)
                } else {
                    (Stage::Stall, 1)
                };
                self.stage = stage;
                next(delay, Record::Sample(sample))
            }

            Stage::Stall => {
                let Some(begin) = self.sample(0) else {
                    return self.end(Record::ClockError);
                };

                // The task keeps the CPU without yielding; only interrupts run.
                let ticks = (STALL_MS * TICKS_PER_SECOND).div_ceil(1000);
                while sdk::systimer_ticks().wrapping_sub(begin.raw) < ticks {
                    sdk::spin_once();
                }

                let Some(end) = self.sample(1) else {
                    return self.end(Record::ClockError);
                };
                self.stage = Stage::Arm;
                // Report after the stall so the log itself does not yield inside it.
                self.pending = Some(Record::StallEnd(end));
                next(1, Record::StallBegin(begin))
            }

            Stage::Arm => {
                if let Some(record) = self.pending.take() {
                    return next(1, record);
                }

                sdk::backup_write(sdk::BACKUP_USER, MARKER);
                let marker = sdk::backup_read(sdk::BACKUP_USER);
                let ok = marker == MARKER && self.watchdog.open(&WATCHDOG);
                let vendor = sdk::wdg_vendor();
                if !ok {
                    if !self.watchdog.claimed {
                        sdk::backup_write(sdk::BACKUP_USER, 0);
                    }
                    return self.end(Record::Arm { ok, vendor, marker });
                }

                let Some(now) = self.events.now_ms() else {
                    return self.end(Record::ClockError);
                };

                self.phase_at = now;
                self.stage = Stage::Feed;
                next(FEED_PERIOD_MS, Record::Arm { ok, vendor, marker })
            }

            Stage::Feed => {
                self.progress += 1;
                self.checkpoints += 1;
                let ok = self.watchdog.checkpoint(self.progress);
                let Some(now) = self.events.now_ms() else {
                    return self.end(Record::ClockError);
                };

                let ms = now.wrapping_sub(self.phase_at);
                if ok && ms < FEED_MS {
                    return Step {
                        delay_ms: FEED_PERIOD_MS,
                        record: None,
                    };
                }

                // A failed checkpoint no longer refreshes: the reset follows either way.
                self.phase_at = now;
                self.reported_at = now;
                self.stage = Stage::Stuck;
                self.pending = Some(Record::WatchdogStall { ms: now });
                next(
                    1,
                    Record::Fed {
                        ms,
                        checkpoints: self.checkpoints,
                        ok,
                    },
                )
            }

            Stage::Stuck => {
                if let Some(record) = self.pending.take() {
                    return next(FEED_PERIOD_MS, record);
                }

                // Alive but without progress: the repeated token must never refresh.
                let _ = self.watchdog.checkpoint(self.progress);
                let Some(now) = self.events.now_ms() else {
                    return self.end(Record::ClockError);
                };

                let ms = now.wrapping_sub(self.phase_at);
                if ms >= RESET_DEADLINE_MS {
                    let _ = self.watchdog.close(Some(&self.board));
                    if !self.watchdog.claimed {
                        sdk::backup_write(sdk::BACKUP_USER, 0);
                    }
                    return self.end(Record::NoReset { ms });
                }

                if now.wrapping_sub(self.reported_at) < STALLED_REPORT_MS {
                    return Step {
                        delay_ms: FEED_PERIOD_MS,
                        record: None,
                    };
                }

                self.reported_at = now;
                next(
                    FEED_PERIOD_MS,
                    Record::Stalled {
                        ms,
                        fault: self.watchdog.fault.0,
                        enabled: sdk::wdg_vendor() & sdk::WDG_BIT_ENABLE != 0,
                    },
                )
            }

            Stage::Done => Step {
                delay_ms: 0,
                record: None,
            },
        }
    }
}
