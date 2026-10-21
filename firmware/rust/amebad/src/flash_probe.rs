//! FW06 bench probe on KM4. Stage B: announce a fresh authority session and
//! bind the KM0 peer's READY into a `FlashCoord` only after stopped-output
//! readback. Stage C: under the production watchdog, run `LEASES` park round
//! trips that hold the parked peer for `HOLD_MS` with NO flash operation,
//! checking the peer's heartbeat stays frozen while parked and advances
//! between leases. Then one fault injection, chosen by the backup marker
//! (survives a watchdog reset, cleared by power loss):
//!
//! - marker 0: set marker 1; the peer withholds ACK for lease 101, so the
//!   begin must time out and the watchdog must reset the platform.
//! - marker 1: set marker 2; this core withholds RELEASE, so the parked peer
//!   must publish TIMEOUT and halt, and the watchdog must reset.
//! - marker 2: no injection; clear the marker and keep feeding the watchdog
//!   while outputs read back stopped (it cannot be disarmed without a board).
//! - marker 3 (a lease failed on the previous boot) or any other value: skip
//!   the leases and clear the marker, so a real fault resets at most once.
//!
//! The leases run here directly, not through the SDK flash hooks (stage D).

use petlibro_core::flash_coord::{
    FlashCoord, FlashFault, FlashParty, FlashPhase, FlashReply, FlashToken,
};
use petlibro_core::tick_clock::TickClock;

use crate::config::{FLASH_LIMITS, SYSTIMER_HZ, WATCHDOG};
use crate::flash_park::{self, Mmio, ParkToken, Reply};
use crate::flash_peer::{
    DIAG_REGISTER, HEARTBEAT_REGISTER, INJECT_LEASE, MARKER_REGISTER, PARK_CHANNEL,
};
use crate::sdk::{self, IrqGuard};
use crate::stopped;
use crate::watchdog::Watchdog;

/// KM4 boot count; KM0 keeps its own in backup register 4.
pub const BOOT_COUNT_REGISTER: u32 = 3;
pub const POLL_MS: u32 = 10;
pub const READY_TIMEOUT_MS: u32 = 2000;
pub const LEASES: u64 = 100;
pub const BATCH: u64 = 25;
pub const HOLD_MS: u32 = 20;
pub const LEASE_GAP_MS: u32 = 20;
/// Begin and end waits, inside the coordinator's 50 ms park/release limits.
pub const EXCHANGE_TICKS: u32 = 40 * SYSTIMER_HZ / 1000;
pub const EXCHANGE_POLLS: u32 = 10_000_000;
/// Still running this long after an injection means the watchdog failed.
pub const RESET_DEADLINE_MS: u32 = WATCHDOG.max_expiry_ms + 4000;
pub const FAILED_MARKER: u32 = 3;

// The coordinator numbers leases by request, and the peer's withheld-ACK
// injection names the lease after the clean run by that number.
const _: () = assert!(LEASES + 1 == INJECT_LEASE);
pub const FEED_PERIOD_MS: u32 = 250;

c_enum!(Mode { WITHHELD_ACK = 0, WITHHELD_RELEASE = 1, NONE = 2 });
c_enum!(LeaseStage {
    REQUEST = 0,
    QUIESCE = 1,
    BEGIN = 2,
    PARKED = 3,
    COMPLETE = 4,
    END = 5,
    RELEASED = 6
});

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    Hello {
        authority: u64,
        count: u32,
    },
    Ready {
        peer: u64,
        reply: FlashReply,
        phase: FlashPhase,
        fault: FlashFault,
        inactive: bool,
    },
    Timeout {
        ms: u32,
    },
    Foreign,
    Start {
        mode: Mode,
        marker: u32,
        watchdog: bool,
    },
    /// The previous boot failed a lease (or the marker was unknown).
    Skip {
        marker: u32,
    },
    Batch {
        leases: u64,
        park_max: u32,
        release_max: u32,
        frozen: u64,
        alive: u64,
    },
    LeaseError {
        lease: u64,
        stage: LeaseStage,
        reply: FlashReply,
        phase: FlashPhase,
        fault: FlashFault,
        diag: LeaseDiag,
    },
    Inject {
        mode: Mode,
        lease: u64,
        begin: bool,
        peer: u32,
        ms: u32,
    },
    Done {
        leases: u64,
        marker: u32,
    },
    NoReset {
        ms: u32,
    },
    ClockError,
}

/// Read-only state captured when a lease fails, to locate the fault.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LeaseDiag {
    /// This core's last published park command (a fault code after failure).
    pub own: u32,
    /// The peer's last published park command.
    pub peer: u32,
    /// KM0 park-handler diagnostics (`flash_peer::DIAG_REGISTER`).
    pub km0: u32,
    pub ier: u32,
    pub isr: u32,
    /// SYSTIMER ticks spent in the failed exchange.
    pub ticks: u32,
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
    Arm,
    Lease,
    Inject,
    Stuck,
    Idle,
    Done,
}

/// Keep at a fixed address once started: the watchdog registers itself.
#[derive(Debug, Default)]
pub struct Probe {
    pub coord: FlashCoord,
    pub watchdog: Watchdog,
    clock: TickClock,
    stage: Stage,
    mode: Mode,
    authority: u64,
    peer: u64,
    lease: u64,
    park_max: u32,
    release_max: u32,
    frozen: u64,
    alive: u64,
    last_beat: u32,
    stuck_at: u32,
    acked_at: u32,
    foreign_seen: bool,
}

fn next(delay_ms: u32, record: Option<Record>) -> Step {
    Step { delay_ms, record }
}

fn park_token(t: &FlashToken) -> ParkToken {
    ParkToken {
        authority_lo: t.authority_session as u32,
        authority_hi: (t.authority_session >> 32) as u32,
        peer_lo: t.peer_session as u32,
        peer_hi: (t.peer_session >> 32) as u32,
        lease_lo: t.lease as u32,
        lease_hi: (t.lease >> 32) as u32,
    }
}

/// One park round trip's measurements.
struct Lease {
    park_ticks: u32,
    release_ticks: u32,
    frozen: bool,
    beat_before: u32,
}

enum Outcome {
    Released(Lease),
    /// The peer never acknowledged; ticks spent waiting.
    NoAck(u32),
    /// Parked and deliberately not released; ACK at this clock time.
    Held(u32),
    Failed(LeaseStage, FlashReply),
}

impl Probe {
    fn end(&mut self, record: Record) -> Step {
        self.stage = Stage::Done;
        next(0, Some(record))
    }

    fn now(&mut self) -> Option<u32> {
        self.clock.read(sdk::systimer_ticks())
    }

    fn beat() -> u32 {
        sdk::backup_read(HEARTBEAT_REGISTER)
    }

    fn lease_error<M: Mmio>(&mut self, stage: LeaseStage, reply: FlashReply, ticks: u32) -> Step {
        let (ier, isr) = sdk::ipc_interrupts();
        let diag = LeaseDiag {
            own: M::load(flash_park::KM4_IPC + 0x20 + 10 * 4),
            peer: flash_park::peer_words::<M>().map_or(0, |w| w.0),
            km0: sdk::backup_read(DIAG_REGISTER),
            ier,
            isr,
            ticks,
        };
        // An unverified lease leaves the coordinator holding outputs; stop
        // refreshing so the watchdog resets the platform, and make the next
        // boot skip the leases.
        sdk::backup_write(MARKER_REGISTER, FAILED_MARKER);
        self.stuck_at = self.clock.millis;
        self.stage = Stage::Stuck;
        next(
            POLL_MS,
            Some(Record::LeaseError {
                lease: self.lease,
                stage,
                reply,
                phase: self.coord.phase,
                fault: self.coord.fault,
                diag,
            }),
        )
    }

    /// One lease. `release` false parks the peer and returns without RELEASE.
    fn run_lease<M: Mmio>(&mut self, release: bool) -> Outcome {
        let Some(now) = self.now() else {
            return Outcome::Failed(LeaseStage::REQUEST, FlashReply::FAILED);
        };
        let reply = self
            .coord
            .request(FlashParty::LOCAL, self.authority, self.lease, now);
        if reply != FlashReply::ACCEPTED {
            return Outcome::Failed(LeaseStage::REQUEST, reply);
        }
        let token = self.coord.token;
        let reply = self
            .coord
            .quiesced(&token, stopped::observe().verified(), now);
        if reply != FlashReply::ACCEPTED {
            return Outcome::Failed(LeaseStage::QUIESCE, reply);
        }
        let park = park_token(&token);
        let beat_before = Self::beat();
        let guard = IrqGuard::lock();
        let systick = sdk::systick_ctrl();
        sdk::set_systick_ctrl(systick & !flash_park::SYSTICK_CONTROL_BITS);
        let begin_at = sdk::systimer_ticks();
        let acked =
            flash_park::ram_owner_begin::<M>(&park, PARK_CHANNEL, EXCHANGE_TICKS, EXCHANGE_POLLS);
        let acked_at = sdk::systimer_ticks();
        let restore = |guard: IrqGuard| {
            sdk::set_systick_ctrl(systick);
            drop(guard);
        };
        if !acked {
            restore(guard);
            return Outcome::NoAck(acked_at.wrapping_sub(begin_at));
        }
        let now = self.now().unwrap_or(now);
        let reply = self.coord.parked(&token, FlashParty::PEER, now);
        if reply != FlashReply::ACCEPTED {
            restore(guard);
            return Outcome::Failed(LeaseStage::PARKED, reply);
        }
        let parked_beat = Self::beat();
        let hold = HOLD_MS * SYSTIMER_HZ / 1000;
        while sdk::systimer_ticks().wrapping_sub(acked_at) < hold {
            sdk::spin_once();
        }
        let frozen = Self::beat() == parked_beat;
        if !release {
            restore(guard);
            return Outcome::Held(now);
        }
        let now = self.now().unwrap_or(now);
        let reply = self.coord.complete(&token, FlashParty::LOCAL, now);
        if reply != FlashReply::ACCEPTED {
            restore(guard);
            return Outcome::Failed(LeaseStage::COMPLETE, reply);
        }
        let end_at = sdk::systimer_ticks();
        let ended =
            flash_park::ram_owner_end::<M>(&park, PARK_CHANNEL, EXCHANGE_TICKS, EXCHANGE_POLLS);
        let released_at = sdk::systimer_ticks();
        restore(guard);
        if !ended {
            return Outcome::Failed(LeaseStage::END, FlashReply::FAILED);
        }
        let now = self.now().unwrap_or(now);
        let reply =
            self.coord
                .released(&token, FlashParty::PEER, stopped::observe().verified(), now);
        if reply != FlashReply::ACCEPTED {
            return Outcome::Failed(LeaseStage::RELEASED, reply);
        }
        Outcome::Released(Lease {
            park_ticks: acked_at.wrapping_sub(begin_at),
            release_ticks: released_at.wrapping_sub(end_at),
            frozen,
            beat_before,
        })
    }

    /// One task iteration in task context.
    pub fn step<M: Mmio>(&mut self) -> Step {
        match self.stage {
            Stage::Start => {
                let count = sdk::backup_read(BOOT_COUNT_REGISTER).wrapping_add(1);
                sdk::backup_write(BOOT_COUNT_REGISTER, count);
                let raw = sdk::systimer_ticks();
                self.clock = TickClock::new(SYSTIMER_HZ, raw, 0).unwrap_or_default();
                self.authority = flash_park::session(count, raw, false);
                if !flash_park::hello::<M>(self.authority) {
                    return self.end(Record::ClockError);
                }
                self.stage = Stage::Wait;
                next(
                    POLL_MS,
                    Some(Record::Hello {
                        authority: self.authority,
                        count,
                    }),
                )
            }
            Stage::Wait => {
                let Some(now) = self.now() else {
                    return self.end(Record::ClockError);
                };
                match flash_park::reply::<M>(self.authority) {
                    Reply::Ready(peer) => {
                        // Bind only with verified STOP and infrared OFF readback.
                        let inactive = stopped::observe().verified();
                        let Some(mut coord) = FlashCoord::new(&FLASH_LIMITS, self.authority, now)
                        else {
                            return self.end(Record::ClockError);
                        };
                        let reply = coord.ready(self.authority, peer, inactive, now);
                        self.coord = coord;
                        self.peer = peer;
                        let record = Record::Ready {
                            peer,
                            reply,
                            phase: self.coord.phase,
                            fault: self.coord.fault,
                            inactive,
                        };
                        if reply != FlashReply::ACCEPTED {
                            return self.end(record);
                        }
                        self.stage = Stage::Arm;
                        next(POLL_MS, Some(record))
                    }
                    _ if now >= READY_TIMEOUT_MS => self.end(Record::Timeout { ms: now }),
                    Reply::Foreign if !self.foreign_seen => {
                        self.foreign_seen = true;
                        next(POLL_MS, Some(Record::Foreign))
                    }
                    _ => next(POLL_MS, None),
                }
            }
            Stage::Arm => {
                let marker = sdk::backup_read(MARKER_REGISTER);
                self.mode = match marker {
                    0 => Mode::WITHHELD_ACK,
                    1 => Mode::WITHHELD_RELEASE,
                    2 => Mode::NONE,
                    _ => {
                        sdk::backup_write(MARKER_REGISTER, 0);
                        return self.end(Record::Skip { marker });
                    }
                };
                let watchdog = self.watchdog.open(&WATCHDOG);
                let record = Record::Start {
                    mode: self.mode,
                    marker,
                    watchdog,
                };
                if !watchdog {
                    return self.end(record);
                }
                self.last_beat = Self::beat();
                self.stage = Stage::Lease;
                next(LEASE_GAP_MS, Some(record))
            }
            Stage::Lease => {
                self.lease += 1;
                match self.run_lease::<M>(true) {
                    Outcome::Released(lease) => {
                        self.park_max = self.park_max.max(lease.park_ticks);
                        self.release_max = self.release_max.max(lease.release_ticks);
                        self.frozen += u64::from(lease.frozen);
                        self.alive += u64::from(lease.beat_before != self.last_beat);
                        self.last_beat = Self::beat();
                    }
                    Outcome::NoAck(ticks) => {
                        return self.lease_error::<M>(LeaseStage::BEGIN, FlashReply::FAILED, ticks);
                    }
                    Outcome::Held(_) => {
                        return self.lease_error::<M>(LeaseStage::END, FlashReply::FAILED, 0);
                    }
                    Outcome::Failed(stage, reply) => return self.lease_error::<M>(stage, reply, 0),
                }
                if !self.watchdog.checkpoint(self.lease) {
                    return self.lease_error::<M>(LeaseStage::RELEASED, FlashReply::FAILED, 0);
                }
                if self.lease == LEASES {
                    self.stage = Stage::Inject;
                }
                let record = self.lease.is_multiple_of(BATCH).then_some(Record::Batch {
                    leases: self.lease,
                    park_max: self.park_max,
                    release_max: self.release_max,
                    frozen: self.frozen,
                    alive: self.alive,
                });
                next(LEASE_GAP_MS, record)
            }
            Stage::Inject => {
                let marker = sdk::backup_read(MARKER_REGISTER);
                if self.mode == Mode::NONE {
                    sdk::backup_write(MARKER_REGISTER, 0);
                    self.stage = Stage::Idle;
                    return next(
                        FEED_PERIOD_MS,
                        Some(Record::Done {
                            leases: self.lease,
                            marker,
                        }),
                    );
                }
                sdk::backup_write(MARKER_REGISTER, self.mode.0 + 1);
                self.lease += 1;
                let outcome = self.run_lease::<M>(self.mode != Mode::WITHHELD_RELEASE);
                self.acked_at = match outcome {
                    Outcome::Held(at) => at,
                    _ => u32::MAX,
                };
                self.stuck_at = self.clock.millis;
                self.stage = Stage::Stuck;
                next(
                    POLL_MS,
                    Some(Record::Inject {
                        mode: self.mode,
                        lease: self.coord.token.lease,
                        begin: !matches!(outcome, Outcome::NoAck(_)),
                        peer: flash_park::peer_words::<M>().map_or(0, |w| w.0),
                        ms: 0,
                    }),
                )
            }
            Stage::Stuck => {
                // No checkpoint: the watchdog must reset the platform.
                let Some(now) = self.now() else {
                    return next(POLL_MS, None);
                };
                if self.mode == Mode::WITHHELD_RELEASE
                    && let Some((flash_park::TIMEOUT, ..)) = flash_park::peer_words::<M>()
                    && self.acked_at != u32::MAX
                {
                    let ms = now.wrapping_sub(self.acked_at);
                    self.acked_at = u32::MAX;
                    return next(
                        POLL_MS,
                        Some(Record::Inject {
                            mode: self.mode,
                            lease: self.lease,
                            begin: true,
                            peer: flash_park::TIMEOUT,
                            ms,
                        }),
                    );
                }
                let ms = now.wrapping_sub(self.stuck_at);
                if ms >= RESET_DEADLINE_MS {
                    return self.end(Record::NoReset { ms });
                }
                next(POLL_MS, None)
            }
            Stage::Idle => {
                // Feed only while every iteration re-verifies stopped outputs.
                if stopped::observe().verified() && self.coord.phase == FlashPhase::IDLE {
                    self.lease += 1;
                    let _ = self.watchdog.checkpoint(self.lease);
                }
                next(FEED_PERIOD_MS, None)
            }
            Stage::Done => next(0, None),
        }
    }
}
