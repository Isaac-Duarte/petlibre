//! KM0 side of the boot handshake: answer the KM4 authority's HELLO
//! with this boot's peer session. Task context from XIP; no flash lease
//! exists yet, so nothing here parks or touches flash.
//!
//! EXPERIMENTAL fault injection: when `INJECT_PEER_LOSS_MS` is set, the peer
//! republishes READY with a new session that long after answering, as a
//! rebooted KM0 would, once per boot.
//!
//! An IPC channel-16 request parks this core in RAM through
//! `park::wait` until the exact RELEASE. A task-driven heartbeat in a
//! backup register lets KM4 see that the core really stopped. When the
//! backup marker reads `WITHHOLD_MARKER`, lease `INJECT_LEASE` is ignored
//! without ACK, once, so the authority's begin must time out.

use crate::flash::park::{self, Mmio};

/// SDK SYSTIMER rate, as measured on the bench.
const TICKS_PER_SECOND: u32 = 32_768;

pub const INJECT_PEER_LOSS_MS: Option<u32> = None;
pub const PARK_CHANNEL: u32 = 16;
pub const INJECT_LEASE: u64 = 101;
pub const MARKER_REGISTER: u32 = 1;
pub const WITHHOLD_MARKER: u32 = 1;
pub const HEARTBEAT_REGISTER: u32 = 5;

/// Park handler diagnostics for KM4 to report: entries (high half), the IPC
/// channel (bits 15:8) and the last `ParkEvent` plus one (bits 7:0).
pub const DIAG_REGISTER: u32 = 2;
/// The KM0 backup registers (SDK `BACKUP_REG_BASE`), readable by both cores.
pub const BACKUP_BASE: u32 = 0x4800_03c0;

/// The whole lease budget (`config::FLASH_LIMITS.total_ms`, 700 ms) in ticks,
/// so a lost RELEASE parks this core no longer than the authority's budget.
pub const PARK_TICK_LIMIT: u32 = 700 * TICKS_PER_SECOND / 1000;
pub const PARK_POLL_LIMIT: u32 = 100_000_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Peer {
    pub session: u64,
    pub answered: u64,
    pub answered_at: u32,
    pub boot_count: u32,
    pub injected: bool,
    pub last_lease: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum PeerEvent {
    #[default]
    None = 0,
    Answered = 1,
    Injected = 2,
}

impl PeerEvent {
    /// The variant's number, as records and reports print it.
    pub const fn code(self) -> u32 {
        self as u32
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum ParkEvent {
    #[default]
    Ignored = 0,
    Released = 1,
    Withheld = 2,
    Refused = 3,
}

impl ParkEvent {
    /// The variant's number, as records and reports print it.
    pub const fn code(self) -> u32 {
        self as u32
    }
}

fn backup<M: Mmio>(index: u32) -> u32 {
    M::load(BACKUP_BASE + index * 4)
}

impl Peer {
    /// One poll. `ticks` is the raw SYSTIMER count.
    pub fn step<M: Mmio>(&mut self, boot_count: u32, ticks: u32) -> PeerEvent {
        let beat = backup::<M>(HEARTBEAT_REGISTER).wrapping_add(1);
        M::store(BACKUP_BASE + HEARTBEAT_REGISTER * 4, beat);

        if self.session == 0 {
            self.boot_count = boot_count;
            self.session = park::session(boot_count, ticks, true);
        }

        if park::answer::<M>(self.session, &mut self.answered) != 0 {
            self.answered_at = ticks;
            return PeerEvent::Answered;
        }

        if let Some(ms) = INJECT_PEER_LOSS_MS
            && self.answered != 0
            && !self.injected
            && u64::from(ticks.wrapping_sub(self.answered_at)) * 1000
                >= u64::from(ms) * u64::from(TICKS_PER_SECOND)
        {
            self.injected = true;
            self.session = park::session(self.boot_count.wrapping_add(1), ticks, true);
            park::reannounce::<M>(self.session, self.answered);
            return PeerEvent::Injected;
        }
        PeerEvent::None
    }
}

impl Peer {
    /// IPC request handler body: IRQs masked and SysTick disabled by the
    /// caller, as `park::wait` requires. Only a REQUEST naming this
    /// boot's sessions and a lease newer than the last one parks the core.
    pub fn on_request<M: Mmio>(&mut self) -> ParkEvent {
        if self.answered == 0 {
            return ParkEvent::Ignored;
        }
        let Some(token) = park::requested::<M>(self.answered, self.session) else {
            return ParkEvent::Ignored;
        };
        let lease = u64::from(token.lease_lo) | (u64::from(token.lease_hi) << 32);
        if lease <= self.last_lease {
            return ParkEvent::Ignored;
        }

        if lease == INJECT_LEASE && backup::<M>(MARKER_REGISTER) == WITHHOLD_MARKER {
            self.last_lease = lease;
            return ParkEvent::Withheld;
        }
        // SAFETY: `token` is a live local.
        if unsafe { park::ram_wait::<M>(&token, PARK_TICK_LIMIT, PARK_POLL_LIMIT) } {
            self.last_lease = lease;
            ParkEvent::Released
        } else {
            ParkEvent::Refused
        }
    }
}

#[cfg(all(target_os = "none", feature = "km0"))]
mod exported {
    use super::{Peer, PeerEvent};
    use crate::cell::TaskCell;
    use crate::flash::park;
    use crate::flash::park::Hardware;

    /// Shared by the KM0 peer task's step and the IPC handler, each under
    /// a critical section.
    static PEER: TaskCell<Peer> = TaskCell::new(Peer {
        session: 0,
        answered: 0,
        answered_at: 0,
        boot_count: 0,
        injected: false,
        last_lease: 0,
    });

    /// Called every few milliseconds by the KM0 peer task only. Masks IRQs so
    /// the park handler never observes a half-updated peer.
    #[unsafe(no_mangle)]
    pub extern "C" fn wellfed_amebad_flash_peer_step(boot_count: u32, ticks: u32) -> u32 {
        cortex_m::interrupt::free(|_| {
            // SAFETY: serialized with the IPC handler by the critical section.
            let peer = unsafe { &mut *PEER.get() };
            let event: PeerEvent = peer.step::<Hardware>(boot_count, ticks);
            event.code()
        })
    }

    /// IPC channel handler (`IPC_IRQ_FUN`). Masks IRQs and stops SysTick
    /// around the park, then restores both.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn wellfed_amebad_flash_peer_irq(
        _data: *mut core::ffi::c_void,
        _status: u32,
        channel: u32,
    ) {
        let systick = park::SYSTICK_CTRL as usize as *mut u32;
        cortex_m::interrupt::free(|_| {
            // SAFETY: fixed SysTick CTRL register; restored below.
            let saved = unsafe { core::ptr::read_volatile(systick) };
            // SAFETY: as above.
            unsafe { core::ptr::write_volatile(systick, saved & !park::SYSTICK_CONTROL_BITS) };

            // SAFETY: serialized with the task step by the critical section.
            let peer = unsafe { &mut *PEER.get() };
            let event = peer.on_request::<Hardware>();
            // SAFETY: as above.
            unsafe { core::ptr::write_volatile(systick, saved) };

            let diag = (super::BACKUP_BASE + super::DIAG_REGISTER * 4) as usize as *mut u32;
            // SAFETY: KM0 user backup register 2.
            let count = (unsafe { core::ptr::read_volatile(diag) } >> 16).wrapping_add(1);
            // SAFETY: as above.
            unsafe {
                core::ptr::write_volatile(
                    diag,
                    (count << 16) | ((channel & 0xff) << 8) | (event.code() + 1),
                )
            };
        })
    }
}
