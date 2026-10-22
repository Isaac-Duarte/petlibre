//! Wi-Fi state shared between the FW07 probe (the only writer) and the FW08
//! transport task (a reader), as atomics: no lock, and no reference from the
//! transport into the probe.

use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};

use crate::sdk::WifiEvent;

// Event kinds from the shim (`example_entry.c`).
const SCAN: u32 = 2;
const DHCP: u32 = 4;
const LOST: u32 = 5;
const DNS: u32 = 7;
const DHCP_ASSIGNED: i32 = 2;

static RSSI: AtomicI32 = AtomicI32::new(0);
static ATTEMPT: AtomicU32 = AtomicU32::new(0);
static LINK_LOSSES: AtomicU32 = AtomicU32::new(0);
static DNS_LOSSES: AtomicU32 = AtomicU32::new(0);
/// 0 none, 1 link, 2 DNS.
static LAST_LOSS: AtomicU32 = AtomicU32::new(0);
/// True from a successful DNS lookup until the next loss or failed lookup.
static DNS_OK: AtomicBool = AtomicBool::new(false);
/// Bumped at every join and loss: a transport opened in an older generation
/// rides on a link that has since gone away.
static GENERATION: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    pub rssi: i32,
    pub attempt: u32,
    pub link_losses: u32,
    pub dns_losses: u32,
    pub last_loss: u32,
    pub dns_ok: bool,
    pub generation: u32,
}

/// Fold one shim event in (FW07 probe task only).
pub fn observe(event: &WifiEvent) {
    match event.kind {
        SCAN if event.result == 0 && event.value == 1 => {
            RSSI.store(event.rssi, Ordering::Relaxed);
            ATTEMPT.store(event.attempt, Ordering::Relaxed);
        }
        DHCP if event.result == DHCP_ASSIGNED => {
            GENERATION.fetch_add(1, Ordering::Release);
        }
        DNS => DNS_OK.store(event.result == 0 && event.value != 0, Ordering::Release),
        LOST => {
            DNS_OK.store(false, Ordering::Release);
            let (counter, cause) = if event.result == 1 {
                (&DNS_LOSSES, 2)
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

pub fn read() -> Status {
    Status {
        generation: GENERATION.load(Ordering::Acquire),
        dns_ok: DNS_OK.load(Ordering::Acquire),
        rssi: RSSI.load(Ordering::Relaxed),
        attempt: ATTEMPT.load(Ordering::Relaxed),
        link_losses: LINK_LOSSES.load(Ordering::Relaxed),
        dns_losses: DNS_LOSSES.load(Ordering::Relaxed),
        last_loss: LAST_LOSS.load(Ordering::Relaxed),
    }
}

/// Host tests only: back to the boot state.
#[cfg(not(target_os = "none"))]
pub fn reset() {
    for counter in [&ATTEMPT, &LINK_LOSSES, &DNS_LOSSES, &LAST_LOSS, &GENERATION] {
        counter.store(0, Ordering::Relaxed);
    }
    RSSI.store(0, Ordering::Relaxed);
    DNS_OK.store(false, Ordering::Relaxed);
}
