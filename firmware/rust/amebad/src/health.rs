//! The FW08 health snapshot, built from data the firmware already has. This
//! is the transport's only view of the rest of the firmware: it reads, and
//! returns a plain value.

use petlibre_core::transport::{Health, Loss, Outputs};

use crate::{net_status, sdk, startup, stopped};

/// KM0 user backup register 4: the FW06 peer's boot count.
const BOOT_COUNT_REGISTER: u32 = 4;
const FAULT_STOPPED_READBACK: u32 = 1 << 2;

/// Startup facts read once, when the transport task starts.
#[derive(Clone, Copy, Debug)]
pub struct Boot {
    pub bank: u8,
    pub reset_flags: u32,
    pub boot_count: u32,
}

impl Boot {
    pub fn read(snapshot: startup::Snapshot) -> Self {
        Self {
            bank: snapshot.bank().unwrap_or(u8::MAX),
            reset_flags: snapshot.reset,
            boot_count: sdk::backup_read(BOOT_COUNT_REGISTER),
        }
    }
}

/// The transport's own counters, supplied by it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Link {
    pub seq: u32,
    pub connects: u32,
    pub last_error: Option<&'static str>,
}

pub fn snapshot(boot: Boot, link: Link, online: bool) -> Health {
    let wifi = net_status::read();
    let observed = stopped::observe();
    let outputs = if observed.verified() {
        Outputs::Stopped
    } else if observed.sensing() {
        Outputs::Sensing
    } else {
        Outputs::Unverified
    };
    Health {
        online,
        seq: link.seq,
        uptime_ms: sdk::uptime_ms(),
        build: startup::BUILD_ID,
        bank: boot.bank,
        reset_flags: boot.reset_flags,
        boot_count: boot.boot_count,
        outputs,
        faults: if outputs != Outputs::Unverified {
            0
        } else {
            FAULT_STOPPED_READBACK
        },
        rssi: wifi.rssi,
        wifi_attempt: wifi.attempt,
        link_losses: wifi.link_losses,
        dns_losses: wifi.dns_losses,
        last_loss: match wifi.last_loss {
            1 => Loss::Link,
            2 => Loss::Dns,
            _ => Loss::None,
        },
        connects: link.connects,
        last_error: link.last_error,
    }
}
