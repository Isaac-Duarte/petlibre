//! Arming the SDK flash-lock hooks for one KM4 writer (the updater, the
//! confirmation), as the probe proved on hardware: announce a fresh
//! authority, bind the KM0 peer's READY with verified stopped outputs, set
//! the attempt marker, then open the watchdog and install the lease
//! provider. A boot that finds the attempt marker set skips: a hook fault
//! or hang resets at most once per power cycle.
//!
//! The same backup register holds the update-attempt bit ([`UPDATE_BIT`]):
//! set while bank 0 installs an update, cleared when the install ends in
//! order. A boot that finds it set refuses offers until power is lost, but
//! still arms, so a reset during an install, outside any one write, never
//! stops the feeder from saving or feeding. A hang inside one of the
//! install's writes is the flash attempt marker's case like any writer's:
//! the write ran with [`ATTEMPT_BIT`] set, so the boot after it skips
//! arming (clearing the marker) and saves nothing until it resets again,
//! the once-per-power-cycle rule every writer gets.

use wellfed_core::clock::tick::TickClock;
use wellfed_core::flash_coord::{FlashCoord, FlashReply};

use crate::flash::park::{self, Mmio, Reply};
use crate::flash::peer::MARKER_REGISTER;
use crate::flash_lock::provider::{ATTEMPT_BIT, BOOT_COUNT_REGISTER, Provider, READY_TIMEOUT_MS};
use crate::sdk;
use crate::stopped;
use crate::supervision::config::{FLASH_LIMITS, SYSTIMER_HZ};

pub const POLL_MS: u32 = 50;

/// In the attempt register beside [`ATTEMPT_BIT`]: an install was writing
/// bank 1. Arming leaves it alone.
pub const UPDATE_BIT: u32 = 0x200;

/// The SDK flash-lock hooks and the watchdog, as the target glue wires them.
pub trait Hooks {
    /// Open the watchdog (or confirm it is open); the glue feeds it.
    fn watchdog() -> bool;
    /// Install `provider` into the hooks; task context, IRQs enabled.
    fn install(provider: *mut Provider) -> bool;
    fn armed() -> bool;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Ready {
        reply: FlashReply,
        inactive: bool,
    },
    Timeout,
    /// The attempt marker was set: an earlier attempt this power cycle reset.
    Skip {
        marker: u32,
    },
    Armed {
        watchdog: bool,
        installed: bool,
    },
    Clock,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Poll {
    /// Not yet: poll again after `POLL_MS`.
    Pending(Option<Event>),
    /// The hooks are armed and the attempt marker is set.
    Armed(Event),
    /// Stopped without arming; the attempt marker is clear.
    Failed(Event),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Stage {
    #[default]
    Hello,
    Wait,
    Arm,
    Done,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Arming {
    stage: Stage,
    clock: TickClock,
    authority: u64,
}

impl Arming {
    fn fail(&mut self, event: Event) -> Poll {
        self.stage = Stage::Done;
        Poll::Failed(event)
    }

    /// One poll in task context.
    ///
    /// # Safety
    /// `provider` is valid for the program's lifetime, stays at a fixed
    /// address and is not otherwise borrowed.
    pub unsafe fn step<M: Mmio, H: Hooks>(&mut self, provider: *mut Provider) -> Poll {
        match self.stage {
            Stage::Hello => {
                let count = sdk::backup_read(BOOT_COUNT_REGISTER).wrapping_add(1);
                sdk::backup_write(BOOT_COUNT_REGISTER, count);

                let raw = sdk::systimer_ticks();
                self.clock = TickClock::new(SYSTIMER_HZ, raw, 0).unwrap_or_default();
                self.authority = park::session(count, raw, false);
                if !park::hello::<M>(self.authority) {
                    return self.fail(Event::Clock);
                }

                self.stage = Stage::Wait;
                Poll::Pending(None)
            }

            Stage::Wait => {
                let Some(now) = self.clock.read(sdk::systimer_ticks()) else {
                    return self.fail(Event::Clock);
                };

                match park::reply::<M>(self.authority) {
                    Reply::Ready(peer) => {
                        // Bind only with verified STOP and infrared OFF readback.
                        let inactive = stopped::observe().verified();
                        let Some(mut coord) = FlashCoord::new(&FLASH_LIMITS, self.authority, now)
                        else {
                            return self.fail(Event::Clock);
                        };
                        let reply = coord.ready(self.authority, peer, inactive, now);
                        // SAFETY: the caller's contract; no hook is armed yet.
                        unsafe { *provider = Provider::bound(coord, self.clock, self.authority) };

                        let event = Event::Ready { reply, inactive };
                        if reply != FlashReply::Accepted {
                            return self.fail(event);
                        }
                        self.stage = Stage::Arm;
                        Poll::Pending(Some(event))
                    }
                    _ if now >= READY_TIMEOUT_MS => self.fail(Event::Timeout),
                    _ => Poll::Pending(None),
                }
            }

            Stage::Arm => {
                let marker = sdk::backup_read(MARKER_REGISTER);
                if marker & !UPDATE_BIT != 0 {
                    sdk::backup_write(MARKER_REGISTER, marker & UPDATE_BIT);
                    return self.fail(Event::Skip { marker });
                }

                set_attempt();
                let watchdog = H::watchdog();
                let installed = watchdog && H::install(provider);
                let event = Event::Armed {
                    watchdog,
                    installed,
                };

                self.stage = Stage::Done;
                if !installed {
                    clear_attempt();
                    return Poll::Failed(event);
                }
                Poll::Armed(event)
            }

            Stage::Done => Poll::Failed(Event::Clock),
        }
    }
}

fn update_bit() -> u32 {
    sdk::backup_read(MARKER_REGISTER) & UPDATE_BIT
}

/// Clear the attempt marker after an orderly stop.
pub fn clear_attempt() {
    sdk::backup_write(MARKER_REGISTER, update_bit());
}

/// Mark a write attempt on hooks armed earlier this boot, so a hang inside
/// it is skipped after the reset, as during arming.
pub fn set_attempt() {
    sdk::backup_write(MARKER_REGISTER, update_bit() | ATTEMPT_BIT);
}

/// Whether an install was writing bank 1 when the feeder last reset.
pub fn update_attempted() -> bool {
    update_bit() != 0
}

/// An install starts writing bank 1 (`true`) or ends in order (`false`).
pub fn set_update_attempt(writing: bool) {
    let marker = sdk::backup_read(MARKER_REGISTER) & !UPDATE_BIT;
    let bit = if writing { UPDATE_BIT } else { 0 };
    sdk::backup_write(MARKER_REGISTER, marker | bit);
}
