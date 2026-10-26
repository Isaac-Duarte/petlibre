//! Wi-Fi state shared between its writers and the tasks that read it, as
//! atomics: no lock, and no reference from a reader into a writer. The
//! probe, folding in the shim's events, writes the counters and the
//! generation; the Wi-Fi task's watch ([`crate::link`]) alone writes
//! whether the link is live, so a loss clears it before the event saying so
//! is even drained.

use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};

use crate::sdk::WifiEvent;

/// `LwIP_DHCP` state for an assigned address, in a DHCP event's `result`.
const DHCP_ASSIGNED: i32 = 2;

static RSSI: AtomicI32 = AtomicI32::new(0);
static ATTEMPT: AtomicU32 = AtomicU32::new(0);
static LINK_LOSSES: AtomicU32 = AtomicU32::new(0);
/// Losses the watch noticed (the lease or the gateway) rather than the
/// association.
static CHECK_LOSSES: AtomicU32 = AtomicU32::new(0);
/// 0 none, 1 link, 2 the watch's check.
static LAST_LOSS: AtomicU32 = AtomicU32::new(0);
/// [`wellfed_core::liveness::Liveness::live`] of the current join.
static LIVE: AtomicBool = AtomicBool::new(false);
/// Bumped at every join and loss: a transport opened in an older generation
/// rides on a link that has since gone away.
static GENERATION: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    pub rssi: i32,
    pub attempt: u32,
    pub link_losses: u32,
    pub check_losses: u32,
    pub last_loss: u32,
    /// The link is known good: associated, leased and, once its gateway
    /// has answered this join, answering still. No internet is involved.
    pub live: bool,
    pub generation: u32,
}

/// Fold one shim event in (Wi-Fi probe task only).
pub fn observe(event: &WifiEvent) {
    match event.kind {
        WifiEvent::SCAN if event.result == 0 && event.value == 1 => {
            RSSI.store(event.rssi, Ordering::Relaxed);
            ATTEMPT.store(event.attempt, Ordering::Relaxed);
        }
        WifiEvent::DHCP if event.result == DHCP_ASSIGNED => {
            GENERATION.fetch_add(1, Ordering::Release);
        }
        WifiEvent::LOST => {
            // Any cause but the association's is the watch's check.
            let (counter, cause) = if event.result != 0 {
                (&CHECK_LOSSES, 2)
            } else {
                (&LINK_LOSSES, 1)
            };

            counter.fetch_add(1, Ordering::Relaxed);
            LAST_LOSS.store(cause, Ordering::Relaxed);
            GENERATION.fetch_add(1, Ordering::Release);
        }
        _ => {}
    }
}

/// The current join's liveness (the Wi-Fi task's watch only).
pub(crate) fn set_live(live: bool) {
    LIVE.store(live, Ordering::Release);
}

pub fn read() -> Status {
    Status {
        generation: GENERATION.load(Ordering::Acquire),
        live: LIVE.load(Ordering::Acquire),
        rssi: RSSI.load(Ordering::Relaxed),
        attempt: ATTEMPT.load(Ordering::Relaxed),
        link_losses: LINK_LOSSES.load(Ordering::Relaxed),
        check_losses: CHECK_LOSSES.load(Ordering::Relaxed),
        last_loss: LAST_LOSS.load(Ordering::Relaxed),
    }
}

/// Host tests only: back to the boot state.
#[cfg(not(target_os = "none"))]
pub fn reset() {
    for counter in [
        &ATTEMPT,
        &LINK_LOSSES,
        &CHECK_LOSSES,
        &LAST_LOSS,
        &GENERATION,
    ] {
        counter.store(0, Ordering::Relaxed);
    }
    RSSI.store(0, Ordering::Relaxed);
    LIVE.store(false, Ordering::Relaxed);
}
