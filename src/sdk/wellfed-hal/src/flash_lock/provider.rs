//! The KM4 lease provider for the SDK flash-lock hooks: each SDK flash
//! operation reserves a `FlashCoord` lease, verifies stopped outputs, parks
//! KM0 in RAM and releases it afterwards. The probes proved
//! the sequence on hardware; the updater, confirmation and the
//! offer watch arm it through `arming`.
//!
//! Every failed step is final: the hook returns false, which halts in RAM
//! rather than retrying, and [`Provider::failure`] keeps the first step that
//! failed for the report.

use wellfed_core::clock::tick::TickClock;
use wellfed_core::flash_coord::{
    FlashCoord, FlashFault, FlashParty, FlashPhase, FlashReply, FlashToken,
};

use crate::flash::hooks::HookTicket;
use crate::flash::park::{self, Mmio, ParkToken};
use crate::flash::peer::{HEARTBEAT_REGISTER, PARK_CHANNEL};
use crate::sdk;
use crate::stopped;
use crate::supervision::config::SYSTIMER_HZ;

/// KM4 boot count; KM0 keeps its own in backup register 4.
pub const BOOT_COUNT_REGISTER: u32 = 3;
/// How long arming waits for the KM0 peer's READY after announcing.
pub const READY_TIMEOUT_MS: u32 = 2000;
/// Set in the peer marker register before the hooks open; a boot that finds
/// it set skips arming.
pub const ATTEMPT_BIT: u32 = 0x100;

/// Begin and end waits, inside the coordinator's 50 ms park/release limits.
pub const EXCHANGE_TICKS: u32 = 40 * SYSTIMER_HZ / 1000;
pub const EXCHANGE_POLLS: u32 = 10_000_000;
/// A lease that holds IRQs masked longer than this (100 ms) is counted: the
/// Wi-Fi driver misses its interrupts for that long, and the recovery role's
/// download runs beside it.
pub const LONG_LEASE_TICKS: u32 = 100 * SYSTIMER_HZ / 1000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum LeaseStage {
    #[default]
    Request = 0,
    Quiesce = 1,
    Begin = 2,
    Parked = 3,
    Complete = 4,
    End = 5,
    Released = 6,
    Ticket = 7,
}

impl LeaseStage {
    /// The variant's number, as records and reports print it.
    pub const fn code(self) -> u32 {
        self as u32
    }
}

fn park_token(t: &FlashToken) -> ParkToken {
    ParkToken::new(t.authority_session, t.peer_session, t.lease)
}

fn beat() -> u32 {
    sdk::backup_read(HEARTBEAT_REGISTER)
}

/// The KM4 side of the SDK flash-lock hooks. Keep at a fixed address for the
/// armed lifetime; callbacks reach it only through the hook context pointer.
#[derive(Debug)]
pub struct Provider {
    pub coord: FlashCoord,
    clock: TickClock,
    authority: u64,
    saved_systick: u32,
    parked_beat: u32,
    /// When the current lease masked IRQs, in SysTimer ticks.
    masked_at: u32,

    // Report counters: leases completed, the slowest park and release
    // exchanges in SysTimer ticks, and leases during which KM0's heartbeat
    // stayed frozen.
    pub leases: u64,
    pub park_max: u32,
    pub release_max: u32,
    pub frozen: u64,
    /// The longest any lease held IRQs masked, from masking to KM0's
    /// release, in SysTimer ticks; and how many held them past
    /// [`LONG_LEASE_TICKS`].
    pub masked_max: u32,
    pub masked_long: u64,
    /// The first failed step, for the report after reset or halt.
    pub failure: Option<(LeaseStage, FlashReply)>,
}

impl Default for Provider {
    fn default() -> Self {
        Self::UNBOUND
    }
}

impl Provider {
    /// Not yet bound: `ready` is false until arming replaces it with
    /// [`Self::bound`]. The runtime's static provider starts here.
    pub const UNBOUND: Self = Self {
        coord: FlashCoord::UNSET,
        clock: TickClock::UNSET,
        authority: 0,
        saved_systick: 0,
        parked_beat: 0,
        masked_at: 0,
        leases: 0,
        park_max: 0,
        release_max: 0,
        frozen: 0,
        masked_max: 0,
        masked_long: 0,
        failure: None,
    };

    /// A provider for `coord`, already bound to the KM0 peer.
    pub fn bound(coord: FlashCoord, clock: TickClock, authority: u64) -> Self {
        Self {
            coord,
            clock,
            authority,
            ..Self::default()
        }
    }

    fn now(&mut self) -> u32 {
        self.clock
            .read(sdk::systimer_ticks())
            .unwrap_or(self.clock.millis)
    }

    fn fail(&mut self, stage: LeaseStage, reply: FlashReply) -> bool {
        self.failure.get_or_insert((stage, reply));
        false
    }

    /// Hook `ready`: a bound, idle, unfaulted coordinator.
    pub fn ready(&self) -> bool {
        self.coord.phase == FlashPhase::Idle && self.coord.fault == FlashFault::Ok
    }

    /// Hook `acquire`, task context with IRQs enabled: reserve the lease,
    /// verify stopped outputs, mask IRQs, stop SysTick and park KM0 in RAM.
    /// Returns true with IRQs masked and SysTick stopped. A false return is a
    /// hook fault, which halts in RAM.
    pub fn acquire<M: Mmio>(&mut self, ticket: &mut HookTicket) -> bool {
        let now = self.now();
        let id = self.coord.token.lease + 1;
        let reply = self
            .coord
            .request(FlashParty::Local, self.authority, id, now);
        if reply != FlashReply::Accepted {
            return self.fail(LeaseStage::Request, reply);
        }

        let token = self.coord.token;
        let reply = self
            .coord
            .quiesced(&token, stopped::observe().verified(), now);
        if reply != FlashReply::Accepted {
            return self.fail(LeaseStage::Quiesce, reply);
        }

        sdk::disable_irq();
        let systick = sdk::systick_ctrl();
        sdk::set_systick_ctrl(systick & !park::SYSTICK_CONTROL_BITS);
        let begin_at = sdk::systimer_ticks();
        self.masked_at = begin_at;
        let ram_token = park_token(&token);
        // SAFETY: `ram_token` is a live local.
        let acked = unsafe {
            park::ram_owner_begin::<M>(&ram_token, PARK_CHANNEL, EXCHANGE_TICKS, EXCHANGE_POLLS)
        };
        let acked_at = sdk::systimer_ticks();
        if !acked {
            return self.fail(LeaseStage::Begin, FlashReply::Failed);
        }

        let now = self.now();
        let reply = self.coord.parked(&token, FlashParty::Peer, now);
        if reply != FlashReply::Accepted {
            return self.fail(LeaseStage::Parked, reply);
        }

        self.parked_beat = beat();
        self.park_max = self.park_max.max(acked_at.wrapping_sub(begin_at));
        self.saved_systick = systick & crate::flash::hooks::SYSTICK_SAVED_BITS;
        *ticket = HookTicket {
            id: token.lease,
            saved_systick: self.saved_systick,
        };

        true
    }

    /// Hook `release`, IRQs masked and SysTick stopped, after the flash
    /// operation returned the controller to XIP: release the exact ticket's
    /// lease and verify stopped outputs. Returns with IRQs still masked.
    pub fn release<M: Mmio>(&mut self, ticket: &HookTicket) -> bool {
        let token = self.coord.token;
        if ticket.id != token.lease || self.coord.phase != FlashPhase::Active {
            return self.fail(LeaseStage::Ticket, FlashReply::Stale);
        }

        // A parked KM0 runs nothing, so its heartbeat should not have moved
        // since the park; count the leases where it held still.
        self.frozen += u64::from(beat() == self.parked_beat);
        let now = self.now();
        let reply = self.coord.complete(&token, FlashParty::Local, now);
        if reply != FlashReply::Accepted {
            return self.fail(LeaseStage::Complete, reply);
        }

        let ram_token = park_token(&token);
        let end_at = sdk::systimer_ticks();
        // SAFETY: `ram_token` is a live local.
        let ended = unsafe {
            park::ram_owner_end::<M>(&ram_token, PARK_CHANNEL, EXCHANGE_TICKS, EXCHANGE_POLLS)
        };
        let released_at = sdk::systimer_ticks();
        if !ended {
            return self.fail(LeaseStage::End, FlashReply::Failed);
        }

        let now = self.now();
        let reply =
            self.coord
                .released(&token, FlashParty::Peer, stopped::observe().verified(), now);
        if reply != FlashReply::Accepted {
            return self.fail(LeaseStage::Released, reply);
        }

        self.release_max = self.release_max.max(released_at.wrapping_sub(end_at));
        let masked = released_at.wrapping_sub(self.masked_at);
        self.masked_max = self.masked_max.max(masked);
        self.masked_long += u64::from(masked > LONG_LEASE_TICKS);
        self.leases += 1;
        true
    }
}
