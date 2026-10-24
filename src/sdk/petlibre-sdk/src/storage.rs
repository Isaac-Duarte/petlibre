//! Mediated storage-region flash for the runtime's record stores.
//!
//! Every erase and program goes through the hooked SDK flash lock, so the
//! flash provider's lease sequence and the KM0 park apply. Sectors are
//! indexes into the owner-approved storage region (`sdk::STORAGE_OFFSET`),
//! never absolute addresses.

use core::ffi::{c_uint, c_void};

use petlibre_core::store::slot::JournalIo;
use petlibre_hal::flash_lock::provider::Provider;
use petlibre_hal::sdk;

/// Settings slots within the storage region: sectors 2 and 3 (0x342000).
pub const SETTINGS_BASE: u32 = sdk::storage::SETTINGS.first;
/// FW45: the generator's seed across boots, and the identity and clients.
pub const SEED_BASE: u32 = sdk::storage::SEED.first;
pub const CLIENTS_BASE: u32 = sdk::storage::CLIENTS.first;
/// FW47: the event ring.
pub const EVENTS_BASE: u32 = sdk::storage::EVENTS.first;
pub const EVENTS_UNITS: u32 = sdk::storage::EVENTS.count;
/// Installation binding for the research unit's settings.
pub const SETTINGS_BINDING: u64 = 0x5046_5731_3502_0001;

/// Mediated flash operations through the hooked SDK lock, bounded to the
/// owner-approved storage region by sector index.
pub trait FlashOps {
    /// Install `provider` into the hooks; task context, IRQs enabled.
    fn install(provider: *mut Provider) -> bool;
    fn read_id() -> [u8; 3];
    fn armed() -> bool;
    fn erase(sector: u32) -> bool;
    fn program(sector: u32, offset: u32, data: &[u8]) -> bool;
    fn read(sector: u32, offset: u32, bytes: &mut [u8]) -> bool;
}

/// Unit `slot` of the `COUNT` starting at region sector `BASE`.
fn slot_sector<const BASE: u32, const COUNT: u32>(slot: c_uint) -> Option<u32> {
    (slot < COUNT).then_some(BASE + slot)
}

// Slot I/O callbacks: the context is unused; slots map to fixed sectors.
unsafe extern "C" fn io_read<F: FlashOps, const BASE: u32, const COUNT: u32>(
    _context: *mut c_void,
    slot: c_uint,
    offset: u32,
    bytes: *mut c_void,
    size: usize,
) -> bool {
    let Some(sector) = slot_sector::<BASE, COUNT>(slot) else {
        return false;
    };
    // SAFETY: the store passes a live buffer of `size` bytes.
    let bytes = unsafe { core::slice::from_raw_parts_mut(bytes.cast::<u8>(), size) };
    F::read(sector, offset, bytes)
}

unsafe extern "C" fn io_program<F: FlashOps, const BASE: u32, const COUNT: u32>(
    _context: *mut c_void,
    slot: c_uint,
    offset: u32,
    bytes: *const c_void,
    size: usize,
) -> bool {
    let Some(sector) = slot_sector::<BASE, COUNT>(slot) else {
        return false;
    };
    // SAFETY: as for `io_read`.
    let bytes = unsafe { core::slice::from_raw_parts(bytes.cast::<u8>(), size) };
    F::program(sector, offset, bytes)
}

unsafe extern "C" fn io_erase<F: FlashOps, const BASE: u32, const COUNT: u32>(
    _context: *mut c_void,
    slot: c_uint,
) -> bool {
    slot_sector::<BASE, COUNT>(slot).is_some_and(F::erase)
}

/// Two-unit slot I/O for the sector pair starting at `BASE`.
pub fn slot_io<F: FlashOps, const BASE: u32>() -> JournalIo {
    ring_io::<F, BASE, 2>()
}

/// I/O for the `COUNT` sectors starting at `BASE`, one unit each.
pub fn ring_io<F: FlashOps, const BASE: u32, const COUNT: u32>() -> JournalIo {
    JournalIo {
        context: core::ptr::null_mut(),
        unit_bytes: sdk::SECTOR_BYTES,
        read: Some(io_read::<F, BASE, COUNT>),
        program: Some(io_program::<F, BASE, COUNT>),
        erase: Some(io_erase::<F, BASE, COUNT>),
    }
}
