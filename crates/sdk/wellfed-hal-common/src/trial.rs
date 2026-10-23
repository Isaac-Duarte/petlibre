//! FW11 boot trial on KM0, before KM4 starts.
//!
//! The factory loader keeps a valid bank 1 on every reset, so a candidate
//! that crashes after boot would be selected forever. KM0 therefore counts
//! the boots of an unconfirmed bank-1 image within one power-on. When the
//! count reaches `LIMIT` and bank 0 holds a valid image, it clears bank 1's
//! first marker byte and resets, and the loader falls back to bank 0.
//!
//! - **Running bank:** bank 1 is running exactly when its physical marker is
//!   intact. If its signatures had failed, the loader would have chosen bank
//!   0, and clearing an unbootable bank's marker changes nothing.
//! - **Confirmation:** a record in storage sector `CONFIRM_SECTOR` naming
//!   this build's identity. KM4 writes it once the image proves healthy.
//! - **Count:** backup register 4, which survives a watchdog reset but not
//!   power loss. Bits 0-15 hold the KM0 boot count (the FW06 peer session),
//!   bits 16-23 the trial count, and bits 24-31 a tag from the identity, so
//!   a count never carries over to a different image.

use wellfed_core::ota::{self, MARKER, Trial};

/// Unconfirmed boots per power-on before giving way.
pub const LIMIT: u32 = 3;
/// Storage sector holding the confirmation record (`0x344000`), the first
/// of the reserved sectors after the journal and settings pairs.
pub const CONFIRM_SECTOR: u32 = 4;
pub const BOOT_MASK: u32 = 0xffff;
const COUNT_SHIFT: u32 = 16;
const TAG_SHIFT: u32 = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Running bank 0 (or bank 1's marker is not intact): no trial.
    NotBank1,
    Confirmed,
    /// Unconfirmed boot number `n` of this image.
    Counted(u32),
    /// Clear bank 1's first marker byte, then reset.
    GiveWay,
    /// The limit is reached but bank 0 is not valid: keep bank 1, since
    /// both banks invalid leaves nothing to boot.
    NoFallback(u32),
}

impl Decision {
    /// The code `wellfed_amebad_trial_decide` returns to the C glue.
    pub fn code(self) -> u32 {
        match self {
            Self::NotBank1 => 0,
            Self::Confirmed => 1,
            Self::Counted(n) => 2 | (n << 8),
            Self::GiveWay => 3,
            Self::NoFallback(n) => 4 | (n << 8),
        }
    }
}

/// The trial decision and the new backup-register value.
pub fn decide(
    bank1: &[u8; 8],
    bank0: &[u8; 8],
    record: &[u8; ota::CONFIRM_LEN],
    register: u32,
    identity: &[u8; 32],
) -> (Decision, u32) {
    let boots = register & BOOT_MASK;
    if *bank1 != MARKER {
        return (Decision::NotBank1, boots);
    }
    if ota::confirmed(record, identity).is_some() {
        return (Decision::Confirmed, boots);
    }

    let tag = u32::from(identity[0]);
    let counted = if register >> TAG_SHIFT == tag {
        (register >> COUNT_SHIFT) & 0xff
    } else {
        0
    };
    let stamp = |n: u32| (tag << TAG_SHIFT) | (n.min(0xff) << COUNT_SHIFT) | boots;

    match ota::trial(false, counted, LIMIT) {
        Trial::Run => (Decision::Confirmed, boots),
        Trial::Count(n) => (Decision::Counted(n), stamp(n)),
        Trial::GiveWay if *bank0 == MARKER => (Decision::GiveWay, boots),
        Trial::GiveWay => (Decision::NoFallback(counted), stamp(counted)),
    }
}

/// The build identity, from the reviewed build wrapper (all zeros on host).
pub fn identity() -> [u8; 32] {
    let text = env!("WELLFED_BUILD_ID").as_bytes();
    let mut out = [0u8; 32];
    if text.len() == 64 {
        for (i, byte) in out.iter_mut().enumerate() {
            let nibble = |c: u8| match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'f' => c - b'a' + 10,
                _ => 0,
            };
            *byte = (nibble(text[2 * i]) << 4) | nibble(text[2 * i + 1]);
        }
    }
    out
}

#[cfg(all(target_os = "none", feature = "km0"))]
mod exported {
    use wellfed_core::ota::CONFIRM_LEN;

    /// Called once by the KM0 main before `km4_boot_on`, with bank 1's and
    /// bank 0's first 8 physical bytes and the confirmation record. Writes
    /// the new backup-register value to `register` and returns
    /// `Decision::code`. Flash and the register are written by the caller.
    ///
    /// # Safety
    /// Each pointer is valid for its length (8, 8, `CONFIRM_LEN`, 4 bytes).
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn wellfed_amebad_trial_decide(
        bank1: *const [u8; 8],
        bank0: *const [u8; 8],
        record: *const [u8; CONFIRM_LEN],
        register: *mut u32,
    ) -> u32 {
        // SAFETY: the caller's contract.
        let (decision, value) =
            unsafe { super::decide(&*bank1, &*bank0, &*record, *register, &super::identity()) };
        // SAFETY: as above.
        unsafe { *register = value };
        decision.code()
    }
}
