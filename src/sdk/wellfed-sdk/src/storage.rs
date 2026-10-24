//! Mediated storage-region flash for the runtime's record stores.
//!
//! Every erase and program goes through the hooked SDK flash lock, so the
//! flash provider's lease sequence and the KM0 park apply. Sectors are
//! indexes into the owner-approved storage region (`sdk::STORAGE_OFFSET`),
//! never absolute addresses.

use core::ffi::{c_uint, c_void};

use wellfed_core::store::slot::JournalIo;
use wellfed_hal::flash_lock::provider::Provider;
use wellfed_hal::sdk;

/// Settings slots within the storage region: sectors 2 and 3 (0x342000).
pub const SETTINGS_BASE: u32 = sdk::storage::SETTINGS.first;
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

/// Slot `slot` of the pair starting at region sector `BASE`.
fn slot_sector<const BASE: u32>(slot: c_uint) -> Option<u32> {
    (slot < 2).then_some(BASE + slot)
}

// Slot I/O callbacks: the context is unused; slots map to fixed sectors.
unsafe extern "C" fn io_read<F: FlashOps, const BASE: u32>(
    _context: *mut c_void,
    slot: c_uint,
    offset: u32,
    bytes: *mut c_void,
    size: usize,
) -> bool {
    let Some(sector) = slot_sector::<BASE>(slot) else {
        return false;
    };
    // SAFETY: the store passes a live buffer of `size` bytes.
    let bytes = unsafe { core::slice::from_raw_parts_mut(bytes.cast::<u8>(), size) };
    F::read(sector, offset, bytes)
}

unsafe extern "C" fn io_program<F: FlashOps, const BASE: u32>(
    _context: *mut c_void,
    slot: c_uint,
    offset: u32,
    bytes: *const c_void,
    size: usize,
) -> bool {
    let Some(sector) = slot_sector::<BASE>(slot) else {
        return false;
    };
    // SAFETY: as for `io_read`.
    let bytes = unsafe { core::slice::from_raw_parts(bytes.cast::<u8>(), size) };
    F::program(sector, offset, bytes)
}

unsafe extern "C" fn io_erase<F: FlashOps, const BASE: u32>(
    _context: *mut c_void,
    slot: c_uint,
) -> bool {
    slot_sector::<BASE>(slot).is_some_and(F::erase)
}

/// Two-unit slot I/O for the sector pair starting at `BASE`.
pub fn slot_io<F: FlashOps, const BASE: u32>() -> JournalIo {
    JournalIo {
        context: core::ptr::null_mut(),
        unit_bytes: sdk::SECTOR_BYTES,
        read: Some(io_read::<F, BASE>),
        program: Some(io_program::<F, BASE>),
        erase: Some(io_erase::<F, BASE>),
    }
}
