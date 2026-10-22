//! RAM interposer for the SDK's `FLASH_Write_Lock`, `FLASH_Write_Unlock` and
//! `FLASH_Write_IPC_Int`. The pinned SDK definitions are renamed to
//! `wellfed_sdk_flash_legacy_*` by the build overlay; all existing callers and
//! the IPC configuration table keep using the public symbols.
//!
//! Until a provider is installed every call routes to the legacy SDK code.
//! Installation is irreversible until a stopped platform reset. No default
//! provider exists and the synthetic diagnostic never installs one.

use core::ffi::c_void;
use core::ptr;

c_enum!(HookFault { CONTEXT = 1, ACQUIRE = 2, RELEASE = 3, PEER = 4, ORDER = 5 });

/// SysTick CTRL ENABLE and TICKINT, as in `park`: CLKSOURCE is
/// read-only 1 on the KM4 Cortex-M33 and says nothing about whether SysTick
/// runs. COUNTFLAG (bit 16) is ignored as well.
pub const SYSTICK_CONTROL_BITS: u32 = 3;
/// Bits a saved CTRL value may hold for restore: ENABLE, TICKINT, CLKSOURCE.
pub const SYSTICK_SAVED_BITS: u32 = 7;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HookTicket {
    /// Strictly increasing, nonzero provider-owned lease identity.
    pub id: u64,
    /// Only the original writable SysTick CTRL bits 0..2.
    pub saved_systick: u32,
}

// Target callbacks are plain C (no unwind edges, hence no landing-pad calls
// out of RAM). Host tests model the RAM halt as an unwinding panic.
#[cfg(target_os = "none")]
macro_rules! callback {
    ($($arg:ty),*) => { unsafe extern "C" fn($($arg),*) };
    ($($arg:ty),* => $ret:ty) => { unsafe extern "C" fn($($arg),*) -> $ret };
}
#[cfg(not(target_os = "none"))]
macro_rules! callback {
    ($($arg:ty),*) => { unsafe extern "C-unwind" fn($($arg),*) };
    ($($arg:ty),* => $ret:ty) => { unsafe extern "C-unwind" fn($($arg),*) -> $ret };
}

pub type ReadyFn = callback!(*mut c_void => bool);
pub type AcquireFn = callback!(*mut c_void, *mut HookTicket => bool);
pub type ReleaseFn = callback!(*mut c_void, *const HookTicket => bool);
pub type PeerFn = callback!(*mut c_void, u32, u32 => bool);
pub type FaultFn = callback!(*mut c_void, HookFault);

/// Transport provider, not an implementation of cross-core exclusion. Retain
/// context in internal RAM for the entire armed lifetime. All callback
/// contracts require independent review before activation:
///
/// - ready: true only while holding a negotiated boot/handoff fence.
/// - acquire: may wait with IRQs enabled; serialize local tasks and both
///   cores, quiesce outputs, establish the lease and real RAM park ACK, and
///   return with IRQs masked, SysTick CTRL=0 and a ticket.
/// - release: verify flash idle, release the EXACT ticket, return with IRQs
///   still masked and SysTick CTRL=0.
/// - peer: handle a flash IPC request from RAM; restore entry IRQ/tick state.
/// - fault: RAM-only; latch the authority fault and attempt inactive outputs.
///   Must not restore interrupts or refresh supervision.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct HookProvider {
    pub context: *mut c_void,
    pub ready: Option<ReadyFn>,
    pub acquire: Option<AcquireFn>,
    pub release: Option<ReleaseFn>,
    pub peer: Option<PeerFn>,
    pub fault: Option<FaultFn>,
}

impl Default for HookProvider {
    fn default() -> Self {
        Self {
            context: ptr::null_mut(),
            ready: None,
            acquire: None,
            release: None,
            peer: None,
            fault: None,
        }
    }
}

/// Pointers to the interposer's private RAM state.
#[derive(Clone, Copy)]
pub struct HookCells {
    pub transport: *mut HookProvider,
    pub ticket: *mut HookTicket,
    pub last_id: *mut u64,
    pub armed: *mut bool,
    pub faulted: *mut bool,
    pub legacy_locked: *mut bool,
    pub lease_held: *mut bool,
}

/// CPU, SysTick and legacy-SDK access for one core.
pub trait HookCpu {
    fn cells() -> HookCells;
    fn mask() -> u32;
    fn ipsr() -> u32;
    fn disable();
    fn restore(value: u32);
    fn tick_read() -> u32;
    fn tick_write(value: u32);
    /// Latch the fault once and halt in RAM for independent reset.
    fn fail(reason: HookFault) -> !;
    /// One iteration of the RAM halt loop.
    fn spin();
    fn legacy_lock();
    fn legacy_unlock();
    fn legacy_ipc(data: *mut c_void, status: u32, channel: u32);
}

// SAFETY (helpers below): the cells point at the interposer's static state,
// accessed only with interrupts masked or from the single installing task.

#[inline(always)]
fn get<T: Copy>(cell: *mut T) -> T {
    unsafe { ptr::read_volatile(cell) }
}

#[inline(always)]
fn set<T>(cell: *mut T, value: T) {
    unsafe { ptr::write_volatile(cell, value) }
}

/// Field-wise volatile copy: never lowered to a memcpy call from RAM text.
#[inline(always)]
fn copy_provider(from: *const HookProvider) -> HookProvider {
    unsafe {
        HookProvider {
            context: ptr::read_volatile(&raw const (*from).context),
            ready: ptr::read_volatile(&raw const (*from).ready),
            acquire: ptr::read_volatile(&raw const (*from).acquire),
            release: ptr::read_volatile(&raw const (*from).release),
            peer: ptr::read_volatile(&raw const (*from).peer),
            fault: ptr::read_volatile(&raw const (*from).fault),
        }
    }
}

#[inline(always)]
fn store_provider(to: *mut HookProvider, p: &HookProvider) {
    unsafe {
        ptr::write_volatile(&raw mut (*to).context, p.context);
        ptr::write_volatile(&raw mut (*to).ready, p.ready);
        ptr::write_volatile(&raw mut (*to).acquire, p.acquire);
        ptr::write_volatile(&raw mut (*to).release, p.release);
        ptr::write_volatile(&raw mut (*to).peer, p.peer);
        ptr::write_volatile(&raw mut (*to).fault, p.fault);
    }
}

/// Body of `hook_fail`: mask, stop SysTick, report once, re-mask and halt.
#[inline(always)]
pub fn fail_body<C: HookCpu>(reason: HookFault) -> ! {
    let s = C::cells();
    C::disable();
    C::tick_write(0);
    let first = !get(s.faulted);
    set(s.faulted, true);
    if first {
        let t = copy_provider(s.transport);
        if let Some(fault) = t.fault {
            // SAFETY: the installed provider's contract.
            unsafe { fault(t.context, reason) };
        }
    }
    // A provider must never make a failed void SDK lock look successful.
    C::disable();
    C::tick_write(0);
    loop {
        C::spin();
    }
}

/// Task context with IRQs enabled, no local legacy operation in progress.
/// Copies callbacks into private RAM. The provider must keep the both-core
/// handoff fence through installation; this API does not negotiate it.
#[inline(always)]
pub fn install<C: HookCpu>(p: *const HookProvider) -> bool {
    if p.is_null() || C::mask() != 0 || C::ipsr() != 0 {
        return false;
    }
    let s = C::cells();
    let candidate = copy_provider(p);
    let Some(ready) = candidate.ready else {
        return false;
    };
    if candidate.acquire.is_none()
        || candidate.release.is_none()
        || candidate.peer.is_none()
        || candidate.fault.is_none()
    {
        return false;
    }
    C::disable();
    let available = !get(s.armed) && !get(s.legacy_locked) && !get(s.faulted);
    C::restore(0);
    // SAFETY: the provider's contract.
    if !available || !unsafe { ready(candidate.context) } || C::mask() != 0 || C::ipsr() != 0 {
        return false;
    }
    C::disable();
    let ok = !get(s.armed) && !get(s.legacy_locked) && !get(s.faulted);
    if ok {
        store_provider(s.transport, &candidate);
        set(s.armed, true);
    }
    C::restore(0);
    ok
}

#[inline(always)]
pub fn armed<C: HookCpu>() -> bool {
    let s = C::cells();
    let saved = C::mask();
    C::disable();
    let result = get(s.armed) && !get(s.faulted);
    C::restore(saved);
    result
}

#[inline(always)]
pub fn write_lock<C: HookCpu>() {
    let s = C::cells();
    let saved = C::mask();
    C::disable();
    if !get(s.armed) {
        set(s.legacy_locked, true);
        C::restore(saved);
        C::legacy_lock();
        return;
    }
    if get(s.faulted) || get(s.lease_held) {
        C::fail(HookFault::ORDER);
    }
    if saved != 0 || C::ipsr() != 0 {
        C::fail(HookFault::CONTEXT);
    }
    C::restore(saved);
    // Pending acquisitions are provider-owned, so another local task can wait
    // without overwriting a global ticket or starving an IRQ-mediated request.
    // A successful acquire masks IRQs before it returns ownership here.
    let t = copy_provider(s.transport);
    let mut ticket = HookTicket::default();
    // SAFETY: the installed provider's contract; `ticket` is a live local.
    let ok = t
        .acquire
        .is_some_and(|acquire| unsafe { acquire(t.context, &mut ticket) });
    let returned_mask = C::mask();
    C::disable();
    if !ok {
        C::fail(HookFault::ACQUIRE);
    }
    // Check the provider's actual exit context, not our own disable above.
    if returned_mask != 1
        || C::ipsr() != 0
        || ticket.id == 0
        || ticket.id <= get(s.last_id)
        || ticket.saved_systick & !SYSTICK_SAVED_BITS != 0
        || C::tick_read() & SYSTICK_CONTROL_BITS != 0
        || get(s.lease_held)
    {
        C::fail(HookFault::ACQUIRE);
    }
    set(s.ticket, ticket);
    set(s.last_id, ticket.id);
    set(s.lease_held, true);
}

#[inline(always)]
pub fn write_unlock<C: HookCpu>() {
    let s = C::cells();
    let saved = C::mask();
    C::disable();
    if !get(s.armed) {
        C::legacy_unlock();
        set(s.legacy_locked, false);
        return;
    }
    if get(s.faulted) || !get(s.lease_held) {
        C::fail(HookFault::ORDER);
    }
    if saved != 1 || C::ipsr() != 0 || C::tick_read() & SYSTICK_CONTROL_BITS != 0 {
        C::fail(HookFault::CONTEXT);
    }
    let t = copy_provider(s.transport);
    // SAFETY: the installed provider's contract; the ticket is private RAM.
    let ok = t
        .release
        .is_some_and(|release| unsafe { release(t.context, s.ticket) });
    let returned_mask = C::mask();
    C::disable();
    if !ok || returned_mask != 1 || C::ipsr() != 0 || C::tick_read() & SYSTICK_CONTROL_BITS != 0 {
        C::fail(HookFault::RELEASE);
    }
    let systick = get(s.ticket).saved_systick;
    set(s.lease_held, false);
    set(s.ticket, HookTicket::default());
    C::tick_write(systick);
    C::restore(0);
}

#[inline(always)]
pub fn write_ipc<C: HookCpu>(data: *mut c_void, status: u32, channel: u32) {
    let s = C::cells();
    let saved = C::mask();
    let entered_ipsr = C::ipsr();
    C::disable();
    if !get(s.armed) {
        C::restore(saved);
        C::legacy_ipc(data, status, channel);
        return;
    }
    if get(s.faulted) || get(s.lease_held) {
        C::fail(HookFault::ORDER);
    }
    let systick = C::tick_read() & SYSTICK_CONTROL_BITS;
    C::restore(saved);
    let t = copy_provider(s.transport);
    // SAFETY: the installed provider's contract.
    let ok = t
        .peer
        .is_some_and(|peer| unsafe { peer(t.context, status, channel) });
    let returned_mask = C::mask();
    let returned_tick = C::tick_read() & SYSTICK_CONTROL_BITS;
    C::disable();
    if !ok || returned_mask != saved || returned_tick != systick || C::ipsr() != entered_ipsr {
        C::fail(HookFault::PEER);
    }
    C::restore(saved);
}

#[cfg(target_os = "none")]
mod exported;
#[cfg(target_os = "none")]
pub use exported::*;
