//! Mediated storage-region flash for the runtime's record stores.
//!
//! Every erase and program goes through the hooked SDK flash lock, so the
//! flash provider's lease sequence and the KM0 park apply. Sectors are
//! indexes into the owner-approved storage region (`plaf103::STORAGE_OFFSET`),
//! never absolute addresses.

use core::marker::PhantomData;

use wellfed_core::store::slot::SlotIo;
use wellfed_hal::flash_lock::provider::Provider;
use wellfed_hal::plaf103;
use wellfed_hal::sdk;

/// The feed journal's slots: sectors 0 and 1 (0x340000).
pub const JOURNAL_BASE: u32 = plaf103::storage::JOURNAL.first;
/// Settings slots within the storage region: sectors 2 and 3 (0x342000).
pub const SETTINGS_BASE: u32 = plaf103::storage::SETTINGS.first;
/// The generator's seed across boots, and the identity and clients.
pub const SEED_BASE: u32 = plaf103::storage::SEED.first;
pub const CLIENTS_BASE: u32 = plaf103::storage::CLIENTS.first;
/// The event ring.
pub const EVENTS_BASE: u32 = plaf103::storage::EVENTS.first;
pub const EVENTS_UNITS: u32 = plaf103::storage::EVENTS.count;
/// The per-feed trace ring.
pub const TRACES_BASE: u32 = plaf103::storage::TRACES.first;
pub const TRACES_UNITS: u32 = plaf103::storage::TRACES.count;
/// Installation binding for the research unit's settings.
pub const SETTINGS_BINDING: u64 = 0x5046_5731_3502_0001;
/// Installation binding for the research unit's feed journal. Not a key: a
/// journal written under another binding reads as corrupt, never as this
/// unit's history.
pub const JOURNAL_BINDING: u64 = 0x5046_5731_3502_0002;

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

/// The `COUNT` sectors starting at region sector `BASE`, one erase unit
/// each, through `F`.
pub struct Region<F, const BASE: u32, const COUNT: u32>(PhantomData<fn() -> F>);

/// Two-unit slot I/O for the sector pair starting at `BASE`.
pub type Slots<F, const BASE: u32> = Region<F, BASE, 2>;

impl<F, const BASE: u32, const COUNT: u32> Region<F, BASE, COUNT> {
    pub const fn new() -> Self {
        Self(PhantomData)
    }

    /// The sector holding unit `slot`.
    fn sector(slot: u32) -> Option<u32> {
        (slot < COUNT).then_some(BASE + slot)
    }
}

impl<F, const BASE: u32, const COUNT: u32> Clone for Region<F, BASE, COUNT> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<F, const BASE: u32, const COUNT: u32> Copy for Region<F, BASE, COUNT> {}

impl<F, const BASE: u32, const COUNT: u32> core::fmt::Debug for Region<F, BASE, COUNT> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Region({BASE}, {COUNT})")
    }
}

impl<F, const BASE: u32, const COUNT: u32> Default for Region<F, BASE, COUNT> {
    fn default() -> Self {
        Self::new()
    }
}

impl<F: FlashOps, const BASE: u32, const COUNT: u32> SlotIo for Region<F, BASE, COUNT> {
    fn unit_bytes(&self) -> u32 {
        sdk::SECTOR_BYTES
    }

    fn read(&self, slot: u32, offset: u32, bytes: &mut [u8]) -> bool {
        Self::sector(slot).is_some_and(|sector| F::read(sector, offset, bytes))
    }

    fn program(&self, slot: u32, offset: u32, bytes: &[u8]) -> bool {
        Self::sector(slot).is_some_and(|sector| F::program(sector, offset, bytes))
    }

    fn erase(&self, slot: u32) -> bool {
        Self::sector(slot).is_some_and(F::erase)
    }
}
