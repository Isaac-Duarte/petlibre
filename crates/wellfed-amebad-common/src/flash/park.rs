//! Experimental cross-core MMIO parking substrate, NOT SDK flash interception.
//!
//! Reserve IPC USR[2,3,4,5,6,9,10] on BOTH cores before use. The stock flash
//! channel writes USR[2], so old hooks MUST be replaced before activation.
//! Slots 0/1, logging 7/8 and SDK message-pointer slot 11 are left alone.
//!
//! The exported routines are leaf code retained in application RAM text:
//! every helper is `#[inline(always)]`, arithmetic wraps and there is no
//! indexing, so no call, panic path or XIP literal can appear. The firmware
//! build audits the emitted instructions on both cores.

use core::ptr;

pub const REQUEST: u32 = 0x504b_0001;
pub const ACK: u32 = 0x504b_0002;
pub const RELEASE: u32 = 0x504b_0003;
pub const RELEASED: u32 = 0x504b_0004;
pub const TIMEOUT: u32 = 0x504b_0081;
pub const PEER_LOST: u32 = 0x504b_0082;
pub const CLOCK: u32 = 0x504b_0083;

/// Boot handshake: KM4 names its authority session; KM0 echoes it with its own.
pub const HELLO: u32 = 0x504b_0010;
pub const READY: u32 = 0x504b_0011;

// Wrapping tick deltas at or above half the counter range read as a clock
// that went backward; tick and poll limits must stay below it.
const HALF: u32 = wellfed_core::HALF_RANGE;
pub const KM0_IPC: u32 = 0x4800_6000;
pub const KM4_IPC: u32 = 0x4000_6000;
pub const SYSTICK_CTRL: u32 = crate::sdk::SYSTICK_CTRL;
pub const TICKS: u32 = 0x4800_2014;
pub const SEM: u32 = 0x4800_6018;

/// Explicit little/high 32-bit words, independent of native u64 alignment.
/// Validated boot binding and unique coordinator lease are caller obligations.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ParkToken {
    pub authority_lo: u32,
    pub authority_hi: u32,
    pub peer_lo: u32,
    pub peer_hi: u32,
    pub lease_lo: u32,
    pub lease_hi: u32,
}

/// Register access for one core. `KM0` selects the core-specific IPC bank,
/// counter sampling and semaphore owner encoding.
pub trait Mmio {
    const KM0: bool;
    fn load(address: u32) -> u32;
    fn store(address: u32, value: u32);
    fn barrier();
    fn mask() -> u32;
    fn halt() -> !;
}

#[inline(always)]
const fn local<M: Mmio>() -> u32 {
    if M::KM0 { KM0_IPC } else { KM4_IPC }
}

#[inline(always)]
const fn peer<M: Mmio>() -> u32 {
    if M::KM0 { KM4_IPC } else { KM0_IPC }
}

/// IPC USR word `index` of a core's bank: USR[10] carries the command and
/// USR[2..6] plus USR[9] the six token payload words.
#[inline(always)]
const fn slot(base: u32, index: u32) -> u32 {
    base + 0x20 + index * 4
}

/// Semaphore owner field value naming this core.
#[inline(always)]
const fn ours<M: Mmio>() -> u32 {
    if M::KM0 { 1 } else { 3 }
}

/// # Safety
/// `t` is null or valid for reading a `ParkToken`.
#[inline(always)]
unsafe fn read(t: *const ParkToken) -> Option<ParkToken> {
    if t.is_null() {
        None
    } else {
        // SAFETY: non-null, so readable by the caller's contract.
        Some(unsafe { ptr::read(t) })
    }
}

#[inline(always)]
fn valid(t: &ParkToken) -> bool {
    (t.authority_lo | t.authority_hi) != 0
        && (t.peer_lo | t.peer_hi) != 0
        && (t.lease_lo | t.lease_hi) != 0
}

/// SysTick CTRL ENABLE and TICKINT: clear means SysTick neither counts nor
/// interrupts. CLKSOURCE (bit 2) is excluded: on the KM4 Cortex-M33 it is
/// read-only 1 without an external reference clock (physical FW06 capture:
/// a 0-2 mask refused every park before publishing). COUNTFLAG (bit 16) is
/// excluded too: it stays set after a CTRL=0 write until CTRL is read.
pub const SYSTICK_CONTROL_BITS: u32 = 3;

#[inline(always)]
fn context<M: Mmio>() -> bool {
    M::mask() == 1 && M::load(SYSTICK_CTRL) & SYSTICK_CONTROL_BITS == 0
}

#[inline(always)]
fn ticks<M: Mmio>() -> u32 {
    if M::KM0 {
        // Pinned KM0 ROM RTIM_GetCount reads twice, then once more on change.
        // Keep that bounded asynchronous-counter sampling without calling ROM.
        let first = M::load(TICKS);
        let second = M::load(TICKS);
        if first == second {
            second
        } else {
            M::load(TICKS)
        }
    } else {
        M::load(TICKS)
    }
}

#[inline(always)]
fn publish_raw<M: Mmio>(t: &ParkToken, command: u32) {
    let base = local::<M>();
    M::store(slot(base, 10), 0);
    M::barrier();
    M::store(slot(base, 2), t.authority_lo);
    M::store(slot(base, 3), t.authority_hi);
    M::store(slot(base, 4), t.peer_lo);
    M::store(slot(base, 5), t.peer_hi);
    M::store(slot(base, 6), t.lease_lo);
    M::store(slot(base, 9), t.lease_hi);
    M::barrier();
    M::store(slot(base, 10), command);
    M::barrier();
}

/// Stable command plus full payload. The lifecycle forbids another token
/// until this token's RELEASED; this is not a general multiwriter mailbox.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Sample {
    Publishing,
    Matching(u32),
    Foreign(u32),
}

#[inline(always)]
fn sample<M: Mmio>(t: &ParkToken) -> Sample {
    let base = peer::<M>();
    let first = M::load(slot(base, 10));
    M::barrier();
    let equal = M::load(slot(base, 2)) == t.authority_lo
        && M::load(slot(base, 3)) == t.authority_hi
        && M::load(slot(base, 4)) == t.peer_lo
        && M::load(slot(base, 5)) == t.peer_hi
        && M::load(slot(base, 6)) == t.lease_lo
        && M::load(slot(base, 9)) == t.lease_hi;
    M::barrier();
    if first == 0 || M::load(slot(base, 10)) != first {
        Sample::Publishing
    } else if equal {
        Sample::Matching(first)
    } else {
        Sample::Foreign(first)
    }
}

/// Caller must retain token/stack in internal RAM; IRQs masked and SysTick
/// disabled. One publisher per core. REQUEST/RELEASE from owner; ACK/RELEASED
/// from the parked core. Publication is invalidation, barrier, six payload
/// words, barrier, command, barrier.
///
/// # Safety
/// `t` is null (refused) or valid for reading a `ParkToken`.
#[inline(always)]
pub unsafe fn publish<M: Mmio>(t: *const ParkToken, command: u32) -> bool {
    // SAFETY: the caller's contract.
    let Some(t) = (unsafe { read(t) }) else {
        return false;
    };
    if !valid(&t) || !context::<M>() || (command != REQUEST && command != RELEASE) {
        return false;
    }
    publish_raw::<M>(&t, command);
    true
}

/// Before entry: real handshake, inactive outputs and PARKING established;
/// raw tick frequency/limits and independent reset supervision established.
/// Returns false WITHOUT ACK for bad context/request. Once ACK is published,
/// only this exact token's RELEASE permits a true return (IRQs remain masked).
/// Timeout, frozen/backward clock or peer mismatch publishes a fault and
/// stays in RAM forever for independent reset; it NEVER returns into XIP.
///
/// # Safety
/// `t` is null (refused) or valid for reading a `ParkToken`.
#[inline(always)]
pub unsafe fn wait<M: Mmio>(t: *const ParkToken, tick_limit: u32, poll_limit: u32) -> bool {
    // SAFETY: the caller's contract.
    let Some(input) = (unsafe { read(t) }) else {
        return false;
    };
    if !valid(&input)
        || tick_limit == 0
        || tick_limit >= HALF
        || poll_limit == 0
        || poll_limit >= HALF
        || !context::<M>()
        || sample::<M>(&input) != Sample::Matching(REQUEST)
    {
        return false;
    }

    // The private copy is materialized before acknowledgement; never depend
    // on shared payload mutation or an XIP token while the owner uses flash.
    let expected = input;
    let start = ticks::<M>();
    let mut previous = start;
    publish_raw::<M>(&expected, ACK);

    let mut reason = TIMEOUT;
    let mut polls = 0;
    while polls < poll_limit {
        polls += 1;
        let now = ticks::<M>();
        if now.wrapping_sub(previous) >= HALF {
            reason = CLOCK;
            break;
        }
        previous = now;
        if now.wrapping_sub(start) >= tick_limit {
            break;
        }

        match sample::<M>(&expected) {
            Sample::Matching(RELEASE) => {
                let checked_at = ticks::<M>();
                if checked_at.wrapping_sub(previous) >= HALF {
                    reason = CLOCK;
                    break;
                }
                if checked_at.wrapping_sub(start) >= tick_limit {
                    break;
                }
                publish_raw::<M>(&expected, RELEASED);
                return true;
            }
            Sample::Foreign(_) => {
                reason = PEER_LOST;
                break;
            }
            Sample::Matching(command) if command != REQUEST => {
                reason = PEER_LOST;
                break;
            }
            _ => {}
        }
    }

    publish_raw::<M>(&expected, reason);
    M::halt()
}

/// One bounded hardware-semaphore attempt. IRQs MUST already be masked.
/// Only application indices 8..15 are accepted (AN0400). Successful claim
/// must retain IRQ exclusion through local lease publication. This follows
/// the candidate SDK ROM's 2-bit owner/lock protocol; silicon behavior and
/// exclusive index allocation remain unverified.
#[inline(always)]
pub fn sem_try_claim<M: Mmio>(index: u32) -> bool {
    if !(8..=15).contains(&index) || M::mask() != 1 {
        return false;
    }
    let shift = index * 2;
    let bit = 1u32 << shift;
    if M::load(SEM) & bit != 0 {
        return false;
    }
    M::store(SEM, bit);
    M::barrier();
    (M::load(SEM) >> shift) & 3 == ours::<M>()
}

/// Release verifies this core owns the selected semaphore before writing it.
#[inline(always)]
pub fn sem_release<M: Mmio>(index: u32) -> bool {
    if !(8..=15).contains(&index) || M::mask() != 1 {
        return false;
    }
    let shift = index * 2;
    if (M::load(SEM) >> shift) & 3 != ours::<M>() {
        return false;
    }
    M::store(SEM, 1 << shift);
    M::barrier();
    // The peer may claim immediately after release. With local IRQs masked,
    // only ownership still naming THIS core indicates a failed release.
    (M::load(SEM) >> shift) & 3 != ours::<M>()
}

/// # Safety
/// `t` is null (refused) or valid for reading a `ParkToken`.
#[inline(always)]
unsafe fn owner_exchange<M: Mmio>(
    t: *const ParkToken,
    command: u32,
    expected: u32,
    channel: u32,
    tick_limit: u32,
    poll_limit: u32,
) -> bool {
    // SAFETY: the caller's contract.
    let Some(input) = (unsafe { read(t) }) else {
        return false;
    };
    if !valid(&input)
        || !context::<M>()
        || !(16..=30).contains(&channel)
        || tick_limit == 0
        || tick_limit >= HALF
        || poll_limit == 0
        || poll_limit >= HALF
    {
        return false;
    }

    // Private RAM token: a caller/context mutation cannot retarget this wait.
    let token = input;
    let start = ticks::<M>();
    let mut previous = start;
    publish_raw::<M>(&token, command);
    // A parked core polls with IRQs masked. Ringing again for RELEASE would
    // leave an unnecessary pending IRQ after it resumes.
    if command == REQUEST {
        M::store(local::<M>() + 8, 1 << channel);
        M::barrier();
    }

    let mut fault = TIMEOUT;
    let mut polls = 0;
    while polls < poll_limit {
        polls += 1;
        let now = ticks::<M>();
        if now.wrapping_sub(previous) >= HALF {
            fault = CLOCK;
            break;
        }
        previous = now;
        if now.wrapping_sub(start) >= tick_limit {
            break;
        }

        let reply = match sample::<M>(&token) {
            Sample::Publishing => continue,
            // Before a new ACK the peer can still expose an old lease's reply.
            // Never grant it; bounded waiting lets the new request arrive.
            // During release, changing the live token is a protocol failure.
            Sample::Foreign(_) if command == REQUEST => continue,
            Sample::Foreign(_) => {
                fault = PEER_LOST;
                break;
            }
            Sample::Matching(reply) => reply,
        };

        if reply == expected {
            let checked_at = ticks::<M>();
            if checked_at.wrapping_sub(previous) >= HALF {
                fault = CLOCK;
                break;
            }
            if checked_at.wrapping_sub(start) >= tick_limit {
                break;
            }
            return true;
        }

        // Owner end can observe the peer's earlier ACK before RELEASED.
        // Owner begin must never accept an old RELEASED as a new ACK.
        if !(command == RELEASE && reply == ACK) {
            fault = PEER_LOST;
            break;
        }
    }

    publish_raw::<M>(&token, fault);
    false
}

/// Owner-side RAM handshake after authority reservation and verified
/// quiescence; the caller owns arbitration, masks IRQs and disables SysTick.
/// Channel must be an exclusively reserved application channel 16..30.
/// Publishes REQUEST and waits for the full-token ACK.
///
/// # Safety
/// `t` is null (refused) or valid for reading a `ParkToken`.
#[inline(always)]
pub unsafe fn owner_begin<M: Mmio>(
    t: *const ParkToken,
    channel: u32,
    tick_limit: u32,
    poll_limit: u32,
) -> bool {
    // SAFETY: the caller's contract.
    unsafe { owner_exchange::<M>(t, REQUEST, ACK, channel, tick_limit, poll_limit) }
}

/// Call only AFTER verified physical flash/controller idle. Publishes
/// RELEASE and waits for RELEASED. False never permits I/O or release of the
/// output/arbitration hold; the caller must fault/halt for reset.
///
/// # Safety
/// `t` is null (refused) or valid for reading a `ParkToken`.
#[inline(always)]
pub unsafe fn owner_end<M: Mmio>(
    t: *const ParkToken,
    channel: u32,
    tick_limit: u32,
    poll_limit: u32,
) -> bool {
    // SAFETY: the caller's contract.
    unsafe { owner_exchange::<M>(t, RELEASE, RELEASED, channel, tick_limit, poll_limit) }
}

/// `wait` through the RAM-resident export on target, so the parked loop never
/// executes from XIP; the generic routine on the host.
///
/// # Safety
/// As for `wait`.
#[inline(always)]
pub unsafe fn ram_wait<M: Mmio>(t: *const ParkToken, tick_limit: u32, poll_limit: u32) -> bool {
    #[cfg(target_os = "none")]
    {
        // SAFETY: the export takes the same arguments and contract; `M`
        // is the hardware here.
        unsafe { wellfed_amebad_flash_park_wait(t, tick_limit, poll_limit) }
    }
    #[cfg(not(target_os = "none"))]
    {
        // SAFETY: the caller's contract.
        unsafe { wait::<M>(t, tick_limit, poll_limit) }
    }
}

/// `owner_begin` through the RAM-resident export on target.
///
/// # Safety
/// As for `owner_begin`.
#[inline(always)]
pub unsafe fn ram_owner_begin<M: Mmio>(
    t: *const ParkToken,
    channel: u32,
    ticks: u32,
    polls: u32,
) -> bool {
    #[cfg(target_os = "none")]
    {
        // SAFETY: as for `ram_wait`.
        unsafe { wellfed_amebad_flash_owner_begin(t, channel, ticks, polls) }
    }
    #[cfg(not(target_os = "none"))]
    {
        // SAFETY: the caller's contract.
        unsafe { owner_begin::<M>(t, channel, ticks, polls) }
    }
}

/// `owner_end` through the RAM-resident export on target.
///
/// # Safety
/// As for `owner_end`.
#[inline(always)]
pub unsafe fn ram_owner_end<M: Mmio>(
    t: *const ParkToken,
    channel: u32,
    ticks: u32,
    polls: u32,
) -> bool {
    #[cfg(target_os = "none")]
    {
        // SAFETY: as for `ram_wait`.
        unsafe { wellfed_amebad_flash_owner_end(t, channel, ticks, polls) }
    }
    #[cfg(not(target_os = "none"))]
    {
        // SAFETY: the caller's contract.
        unsafe { owner_end::<M>(t, channel, ticks, polls) }
    }
}

/// Nonzero per-boot session: a boot count (kept by the caller in a backup
/// register, which survives a single-core reset) above a SYSTIMER sample
/// mixed with a core tag. Distinct per core; two cold boots may repeat one,
/// which is harmless because a cold boot clears both cores' IPC words.
pub const fn session(boot_count: u32, ticks: u32, km0: bool) -> u64 {
    let tag = if km0 { 0x4b30_0000 } else { 0x4b34_0000 };
    ((boot_count as u64) << 32) | ((ticks ^ tag) | 1) as u64
}

#[inline(always)]
fn handshake(authority: u64, peer: u64) -> ParkToken {
    ParkToken {
        authority_lo: authority as u32,
        authority_hi: (authority >> 32) as u32,
        peer_lo: peer as u32,
        peer_hi: (peer >> 32) as u32,
        lease_lo: 0,
        lease_hi: 0,
    }
}

/// Stable peer publication: command and payload unchanged across one read.
pub fn peer_words<M: Mmio>() -> Option<(u32, u64, u64, u64)> {
    let base = peer::<M>();
    let first = M::load(slot(base, 10));
    M::barrier();
    let word = |i| u64::from(M::load(slot(base, i)));
    let authority = word(2) | (word(3) << 32);
    let peer = word(4) | (word(5) << 32);
    let lease = word(6) | (word(9) << 32);
    M::barrier();
    (first != 0 && M::load(slot(base, 10)) == first).then_some((first, authority, peer, lease))
}

/// The peer's REQUEST as a park token, when it names `authority` and `peer`.
pub fn requested<M: Mmio>(authority: u64, peer: u64) -> Option<ParkToken> {
    match peer_words::<M>() {
        Some((REQUEST, a, p, lease)) if a == authority && p == peer && lease != 0 => {
            Some(ParkToken {
                lease_lo: lease as u32,
                lease_hi: (lease >> 32) as u32,
                ..handshake(a, p)
            })
        }
        _ => None,
    }
}

/// KM4 authority: announce a fresh session before any lease. Task context;
/// the coordinator is created only once the matching READY is in hand.
pub fn hello<M: Mmio>(authority: u64) -> bool {
    if M::KM0 || authority == 0 {
        return false;
    }
    publish_raw::<M>(&handshake(authority, 0), HELLO);
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply {
    Waiting,
    /// KM0 echoed this authority with its nonzero session.
    Ready(u64),
    /// A READY for another authority, or a malformed one.
    Foreign,
}

/// KM4: read KM0's current handshake reply for `authority`.
pub fn reply<M: Mmio>(authority: u64) -> Reply {
    if M::KM0 {
        return Reply::Foreign;
    }
    match peer_words::<M>() {
        Some((READY, a, p, 0)) if a == authority && p != 0 => Reply::Ready(p),
        Some((READY, ..)) => Reply::Foreign,
        _ => Reply::Waiting,
    }
}

/// KM0 peer: answer a new HELLO with READY naming `peer`. Returns the newly
/// answered authority, or zero when there is nothing new. `answered` keeps
/// the last authority answered by this core.
pub fn answer<M: Mmio>(peer: u64, answered: &mut u64) -> u64 {
    if !M::KM0 || peer == 0 {
        return 0;
    }
    match peer_words::<M>() {
        Some((HELLO, a, 0, 0)) if a != 0 && a != *answered => {
            publish_raw::<M>(&handshake(a, peer), READY);
            *answered = a;
            a
        }
        _ => 0,
    }
}

/// KM0 peer: republish READY for the answered authority with `peer`, as a
/// rebooted peer would. Fault injection only.
pub fn reannounce<M: Mmio>(peer: u64, answered: u64) -> bool {
    if !M::KM0 || peer == 0 || answered == 0 {
        return false;
    }
    publish_raw::<M>(&handshake(answered, peer), READY);
    true
}

#[cfg(target_os = "none")]
mod exported;
#[cfg(target_os = "none")]
pub use exported::*;
