//! Boot trial and reset-storm breaker on KM0, before KM4 starts.
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
//!   this build's identity. KM4 writes it once the image proves it can feed.
//! - **Count:** backup register 4 ([`REGISTER`]), which survives a watchdog
//!   reset but not power loss. Bits 0-15 hold the KM0 boot count (the peer
//!   session), bits 16-19 the trial count, bits 20-23 the storm count and
//!   bits 24-31 a tag from the identity, so a count never carries over to a
//!   different image.
//!
//! **Reset storms.** Confirmation proves one boot, not every later one: a
//! confirmed build can still fault into a reset at every boot. So on every
//! boot that runs no trial, KM0 also counts the boot in the storm field, and
//! KM4 takes the count back once the boot has run [`STABLE_MS`]
//! ([`settled`]) or restarts on purpose ([`deliberate`]). What KM0 finds at
//! boot is therefore the number of boots in a row that ended in a reset no
//! one asked for within `STABLE_MS` of starting. At [`STORM_LIMIT`]:
//! - a confirmed bank 1 with a valid bank 0 gives way, as a failed trial
//!   does: bank 0 is the full feeder, and a build that faults this often is
//!   the likeliest cause;
//! - otherwise (bank 0 itself, or no valid bank 0 to fall back to) KM4
//!   boots **held** ([`Storm::Held`]): the feeding owner never opens, the
//!   outputs stay stopped, and the device API stays up to say why and to
//!   take an update or a reboot. A held boot that runs `STABLE_MS` keeps the
//!   count at the limit, so it stays held until power is lost or the owner
//!   restarts it;
//! - at [`HALT_LIMIT`], when held boots keep resetting too, KM4 starts
//!   nothing at all and holds the outputs stopped until power is lost
//!   ([`Storm::Halted`]), so no cause can reset the board forever.
//!
//! Backup registers clear on power loss, so a power cut restarts every
//! count; no flash record is involved, and none can be torn.

use wellfed_core::ota::{self, MARKER, Trial};

/// Unconfirmed boots per power-on before giving way.
pub const LIMIT: u32 = 3;
/// Storage sector holding the confirmation record (`0x344000`), the first
/// of the reserved sectors after the journal and settings pairs.
pub const CONFIRM_SECTOR: u32 = wellfed_board_plaf103::storage::CONFIRM.first;
/// The backup register KM0 counts in; KM4 reads and settles it.
pub const REGISTER: u32 = 4;
pub const BOOT_MASK: u32 = 0xffff;
const COUNT_SHIFT: u32 = 16;
const STORM_SHIFT: u32 = 20;
const FIELD_MASK: u32 = 0xf;
const TAG_SHIFT: u32 = 24;

/// Resets in a row, each within [`STABLE_MS`] of its boot, that make a
/// storm: an occasional fault (a Wi-Fi driver fault every few dozen boots)
/// never reaches it, a build that faults at every boot does within minutes.
pub const STORM_LIMIT: u32 = 5;
/// Further early resets of held boots before KM4 starts nothing.
pub const HALT_LIMIT: u32 = STORM_LIMIT + 3;
/// A boot that has run this long ends the burst: the trial's deadline, by
/// which a build that starts at all has long since proved itself.
pub const STABLE_MS: u32 = 10 * 60 * 1000;

const _: () = assert!(LIMIT <= FIELD_MASK && HALT_LIMIT < FIELD_MASK);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Running bank 0 (or bank 1's marker is not intact): no trial. `resets`
    /// in the burst before this boot.
    NotBank1 {
        resets: u32,
    },
    Confirmed {
        resets: u32,
    },
    /// Unconfirmed boot number `n` of this image.
    Counted(u32),
    /// Clear bank 1's first marker byte, then reset.
    GiveWay,
    /// The limit is reached but bank 0 is not valid: keep bank 1, since
    /// both banks invalid leaves nothing to boot.
    NoFallback(u32),
    /// A storm with nothing to fall back to: boot KM4, which holds (or, past
    /// `HALT_LIMIT`, halts).
    Held {
        resets: u32,
    },
    /// A storm on a confirmed bank 1 with bank 0 valid: give way.
    StormGiveWay {
        resets: u32,
    },
}

impl Decision {
    /// The code `wellfed_amebad_trial_decide` returns to the C glue.
    pub fn code(self) -> u32 {
        match self {
            Self::NotBank1 { resets } => resets << 8,
            Self::Confirmed { resets } => 1 | (resets << 8),
            Self::Counted(n) => 2 | (n << 8),
            Self::GiveWay => 3,
            Self::NoFallback(n) => 4 | (n << 8),
            Self::Held { resets } => 5 | (resets << 8),
            Self::StormGiveWay { resets } => 6 | (resets << 8),
        }
    }
}

/// What KM4 does with a storm count, from [`resets`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Storm {
    Normal,
    /// The feeding owner never opens; everything else runs.
    Held,
    /// Nothing starts: outputs stopped, no task, until power is lost.
    Halted,
}

impl Storm {
    pub const fn of(resets: u32) -> Self {
        if resets >= HALT_LIMIT {
            Self::Halted
        } else if resets >= STORM_LIMIT {
            Self::Held
        } else {
            Self::Normal
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Held => "held",
            Self::Halted => "halted",
        }
    }
}

fn tag(identity: &[u8; 32]) -> u32 {
    u32::from(identity[0])
}

/// The storm field, when the register's tag is this image's.
fn storm_field(register: u32, identity: &[u8; 32]) -> u32 {
    if register >> TAG_SHIFT == tag(identity) {
        (register >> STORM_SHIFT) & FIELD_MASK
    } else {
        0
    }
}

/// The register with this image's tag and the given counts.
fn stamp(register: u32, identity: &[u8; 32], trials: u32, storm: u32) -> u32 {
    (tag(identity) << TAG_SHIFT)
        | (trials.min(FIELD_MASK) << COUNT_SHIFT)
        | (storm.min(FIELD_MASK) << STORM_SHIFT)
        | (register & BOOT_MASK)
}

/// The trial decision and the new backup-register value.
pub fn decide(
    bank1: &[u8; 8],
    bank0: &[u8; 8],
    record: &[u8; ota::CONFIRM_LEN],
    register: u32,
    identity: &[u8; 32],
) -> (Decision, u32) {
    let running_bank1 = *bank1 == MARKER;
    let confirmed = ota::confirmed(record, identity).is_some();
    if !running_bank1 || confirmed {
        let resets = storm_field(register, identity);
        // Bank 0 starts its own count: even the same build there is a
        // different boot path.
        if running_bank1 && resets >= STORM_LIMIT && *bank0 == MARKER {
            return (Decision::StormGiveWay { resets }, register & BOOT_MASK);
        }
        let decision = match (running_bank1, resets >= STORM_LIMIT) {
            (_, true) => Decision::Held { resets },
            (false, false) => Decision::NotBank1 { resets },
            (true, false) => Decision::Confirmed { resets },
        };
        // This boot counts as early until KM4 settles it.
        return (decision, stamp(register, identity, 0, resets + 1));
    }

    let counted = if register >> TAG_SHIFT == tag(identity) {
        (register >> COUNT_SHIFT) & FIELD_MASK
    } else {
        0
    };
    match ota::trial(false, counted, LIMIT) {
        Trial::Run => (Decision::Confirmed { resets: 0 }, register & BOOT_MASK),
        Trial::Count(n) => (Decision::Counted(n), stamp(register, identity, n, 0)),
        Trial::GiveWay if *bank0 == MARKER => (Decision::GiveWay, register & BOOT_MASK),
        Trial::GiveWay => (
            Decision::NoFallback(counted),
            stamp(register, identity, counted, 0),
        ),
    }
}

/// The resets in a row before this boot, as KM4 reads the register KM0
/// stamped: KM0 already counted this boot.
pub fn resets(register: u32, identity: &[u8; 32]) -> u32 {
    storm_field(register, identity).saturating_sub(1)
}

/// The register once this boot has run `STABLE_MS`: the burst is over. A
/// held boot stays held: its count goes back to the limit, so only a burst
/// of new early resets moves it on to a halt.
pub fn settled(register: u32, identity: &[u8; 32]) -> u32 {
    let storm = match Storm::of(resets(register, identity)) {
        Storm::Normal => 0,
        Storm::Held | Storm::Halted => STORM_LIMIT,
    };
    with_storm(register, storm)
}

/// The register before a restart someone asked for (a reboot, an
/// activation, a give-way, setup): not a fault, so the burst ends, and a
/// held feeder the owner restarts tries again.
pub fn deliberate(register: u32) -> u32 {
    with_storm(register, 0)
}

fn with_storm(register: u32, storm: u32) -> u32 {
    (register & !(FIELD_MASK << STORM_SHIFT)) | (storm << STORM_SHIFT)
}

/// The build identity, from the reviewed build wrapper (all zeros on host).
pub fn identity() -> [u8; 32] {
    wellfed_session::hex::decode_array(env!("WELLFED_BUILD_ID").as_bytes()).unwrap_or([0; 32])
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
