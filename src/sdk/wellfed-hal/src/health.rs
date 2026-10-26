//! Startup facts the device API's status reports: the bank this build runs
//! from, the reset cause and KM0's boot count, read once.

use crate::{sdk, startup};

/// KM0 user backup register 4: the peer's boot count in bits 0-15; bits
/// 16-31 are the trial's state, which a raw read reported as the count.
const BOOT_COUNT_REGISTER: u32 = 4;
const BOOT_COUNT_MASK: u32 = 0xFFFF;

/// Startup facts read once, when the device API starts.
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
            boot_count: sdk::backup_read(BOOT_COUNT_REGISTER) & BOOT_COUNT_MASK,
        }
    }
}
