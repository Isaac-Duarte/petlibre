//! Offline execution of vendor code the research unit runs, under unicorn.
//!
//! Two pieces of pinned vendor code are executed as Cortex-M4 instructions
//! against supplied state, never against a device:
//! - [`factory`]: the factory loader's flash mapping and bank selection, with
//!   the candidate ROM's RSIP routine, as the image review and the OTA marker
//!   design depend on them;
//! - [`helper`]: the vendor UART flash helper on synthetic NOR, which
//!   `petlibre-flash` drives over [`serve`] in its emulated fixture tests.
//!
//! ROM UART and flash calls, MMIO and caches are modeled. Nothing here
//! establishes physical write acceptance, timing, power-cut behavior or a
//! complete reset; a passing run is emulation evidence only.

#![forbid(unsafe_code)]

pub mod factory;
pub mod helper;
pub mod serve;

/// The verified 8 MiB research-unit backup that the pinned recovery profile,
/// the factory-mapping fixtures and the review's baseline start from.
pub const BASELINE: &str = "private/fixtures/baseline.bin";
pub const BASELINE_SHA256: &str =
    "2751e6c870af7312fe1ff0039831ac5799fb215d629d85af8720b6e50a7479bc";
/// The candidate B-cut KM0 ROM from the pinned SDK checkout.
pub const ROM: &str = "private/upstream/ameba-rtos-d/project/realtek_amebaD_va0_example/GCC-RELEASE/project_lp/asdk/lib/rom_bcut/km0_irom.bin";
pub const ROM_SHA256: &str = "db0fdd8b2232a0d9edaa403715a9992ae36944fbda4dd936e017c1fa531e66d6";
/// The vendor UART flash helper the bench uploads to the loader.
pub const HELPER: &str =
    "private/upstream/ameba-rtos-d/tools/AmebaD/Image_Tool/imgtool_flashloader_amebad.bin";
pub const HELPER_SHA256: &str = "9307121385cb390dfd2da64da2c6c515f17b5a9556b3d04021487c9b9f220b55";

pub const FLASH_BYTES: usize = 8 * 1024 * 1024;
pub const SECTOR: u32 = 4096;
/// Both OTA bank offsets in external flash.
pub const BANKS: [u32; 2] = [0x6000, 0x18_6000];

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A pinned input, reviewed boundary or modeled-call expectation failed.
    #[error("{0}")]
    Refused(String),
    /// The emulator itself failed (unmapped access, invalid instruction, …).
    #[error("Emulation failed: {0:?}")]
    Unicorn(unicorn_engine::uc_error),
}

impl From<unicorn_engine::uc_error> for Error {
    fn from(error: unicorn_engine::uc_error) -> Self {
        Self::Unicorn(error)
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Refuse with `message` unless `condition` holds.
pub fn require(condition: bool, message: impl Into<String>) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(Error::Refused(message.into()))
    }
}

pub(crate) fn word(data: &[u8], offset: usize) -> Option<u32> {
    let bytes = data.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}
