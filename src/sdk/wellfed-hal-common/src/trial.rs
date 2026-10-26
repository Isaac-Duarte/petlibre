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
//!   session), bits 16-19 the trial count on a trial boot, or, with bit 19
//!   set ([`STORM_STATE`]), the storm's backoff level on any other, bits
//!   20-23 the storm count and bits 24-31 a tag from the identity, so a count
//!   never carries over to a different image.
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
//! - otherwise (bank 0 itself, no valid bank 0 to fall back to, or a trial
//!   with no fallback that ran out) KM4 boots **held** ([`Storm::Held`]):
//!   the feeding owner stays closed and the outputs stopped, while the
//!   device API stays up to say why and to take an update or a reboot. A
//!   held boot that runs [`hold_ms`] of its level opens the owner and feeds
//!   again ([`released`]), one level up, so each storm in a row holds twice
//!   as long, up to [`MAX_LEVEL`]; every stable boot takes a level back;
//! - at [`HALT_LIMIT`], when held boots keep resetting too, KM4 starts
//!   nothing for [`HALT_MS`] ([`Storm::Halted`]), then resets into a held
//!   boot ([`halt_over`]).
//!
//! Nothing is permanent: a feeder held or halted by a fault that clears
//! (a run of association hangs, a battery brownout) feeds again on its own,
//! and a fault that persists resets at most a few times an hour. Backup
//! registers clear on power loss, so a power cut restarts every count; no
//! flash record is involved, and none can be torn.

use wellfed_core::ota::{self, MARKER, Trial};

/// Unconfirmed boots per power-on before giving way.
pub const LIMIT: u32 = 3;
/// Storage sector holding the confirmation record (`0x344000`), the first
/// of the reserved sectors after the journal and settings pairs.
pub const CONFIRM_SECTOR: u32 = wellfed_board_plaf103::storage::CONFIRM.first;
/// The backup register KM0 counts in; KM4 reads and settles it.
pub const REGISTER: u32 = 4;
pub const BOOT_MASK: u32 = 0xffff;
const NIBBLE_SHIFT: u32 = 16;
const STORM_SHIFT: u32 = 20;
const FIELD_MASK: u32 = 0xf;
/// In the low nibble: what follows is a storm's backoff level, not a trial
/// count. A trial count never reaches it (`LIMIT` is 3).
pub const STORM_STATE: u32 = 0x8;
const LEVEL_MASK: u32 = 0x7;
const TAG_SHIFT: u32 = 24;

/// Resets in a row, each within [`STABLE_MS`] of its boot, that make a
/// storm: an occasional fault (a Wi-Fi driver fault every few dozen boots)
/// rarely reaches it, a build that faults at every boot does within minutes.
pub const STORM_LIMIT: u32 = 5;
/// Further early resets of held boots before KM4 stops for [`HALT_MS`].
pub const HALT_LIMIT: u32 = STORM_LIMIT + 3;
/// A boot that has run this long ends the burst: the trial's deadline, by
/// which a build that starts at all has long since proved itself.
pub const STABLE_MS: u32 = 10 * 60 * 1000;
/// The backoff levels: a held boot holds `STABLE_MS << level`, so 10, 20,
/// 40, 80 and at most 160 minutes.
pub const MAX_LEVEL: u32 = 4;
/// How long a halted boot starts nothing before it resets into a held one.
pub const HALT_MS: u32 = 60 * 60 * 1000;

const _: () = assert!(LIMIT < STORM_STATE && HALT_LIMIT < FIELD_MASK && MAX_LEVEL <= LEVEL_MASK);
const _: () = assert!(STABLE_MS.checked_shl(MAX_LEVEL).is_some());

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
    /// The trial ran out but bank 0 is not valid: keep bank 1, since both
    /// banks invalid leaves nothing to boot, and run it as a build in no
    /// trial, its resets counted as a storm's.
    NoFallback(u32),
    /// A storm with nothing to fall back to: boot KM4, which holds (or, past
    /// `HALT_LIMIT`, halts for a while).
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
    /// The feeding owner stays closed for [`hold_ms`]; everything else runs.
    Held,
    /// Nothing starts for [`HALT_MS`]: outputs stopped, then a reset.
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

/// How long a held boot at `level` keeps the owner closed.
pub const fn hold_ms(level: u32) -> u32 {
    let level = if level > MAX_LEVEL { MAX_LEVEL } else { level };
    STABLE_MS << level
}

fn tag(identity: &[u8; 32]) -> u32 {
    u32::from(identity[0])
}

/// The register's low nibble and storm field, when its tag is this image's.
fn fields(register: u32, identity: &[u8; 32]) -> (u32, u32) {
    if register >> TAG_SHIFT == tag(identity) {
        (
            (register >> NIBBLE_SHIFT) & FIELD_MASK,
            (register >> STORM_SHIFT) & FIELD_MASK,
        )
    } else {
        (0, 0)
    }
}

/// The register with this image's tag, the low nibble and the storm field.
fn stamp(register: u32, identity: &[u8; 32], nibble: u32, storm: u32) -> u32 {
    (tag(identity) << TAG_SHIFT)
        | ((nibble & FIELD_MASK) << NIBBLE_SHIFT)
        | (storm.min(FIELD_MASK) << STORM_SHIFT)
        | (register & BOOT_MASK)
}

/// The register in storm state at `level`, with `storm` for the next boot.
fn storm_stamp(register: u32, identity: &[u8; 32], level: u32, storm: u32) -> u32 {
    stamp(
        register,
        identity,
        STORM_STATE | level.min(MAX_LEVEL),
        storm,
    )
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
    let (nibble, storm) = fields(register, identity);
    let state = nibble & STORM_STATE != 0;
    let level = if state { nibble & LEVEL_MASK } else { 0 };

    // An unconfirmed bank 1 is past its trial only while it has nothing to
    // fall back to: storm state under its tag means either a spent trial
    // with bank 0 invalid, or another build's count that merely shares the
    // tag (a reinstall, or one identity byte in 256), which must never let
    // a new build skip its trial. With bank 0 valid it is on trial, and a
    // nibble in storm state counts as no trial boot yet.
    let past = state && *bank0 != MARKER;
    if running_bank1 && !confirmed && !past {
        let counted = if state { 0 } else { nibble };
        match ota::trial(false, counted, LIMIT) {
            Trial::Count(n) => return (Decision::Counted(n), stamp(register, identity, n, 0)),
            Trial::GiveWay if *bank0 == MARKER => {
                return (Decision::GiveWay, register & BOOT_MASK);
            }
            // No fallback: the build runs on as one in no trial, held at
            // once, so a trial that cannot pass never resets forever.
            Trial::GiveWay => {
                let value = storm_stamp(register, identity, 0, STORM_LIMIT + 1);
                return (Decision::NoFallback(counted), value);
            }
            Trial::Run => {}
        }
    }

    let resets = storm;
    // Bank 0 starts its own count: even the same build there is a
    // different boot path.
    if running_bank1 && confirmed && resets >= STORM_LIMIT && *bank0 == MARKER {
        return (Decision::StormGiveWay { resets }, register & BOOT_MASK);
    }
    let decision = match (running_bank1, confirmed, resets >= STORM_LIMIT) {
        (_, _, true) => Decision::Held { resets },
        (false, _, false) => Decision::NotBank1 { resets },
        (true, true, false) => Decision::Confirmed { resets },
        (true, false, false) => Decision::NoFallback(LIMIT),
    };
    // This boot counts as early until KM4 settles it.
    (decision, storm_stamp(register, identity, level, resets + 1))
}

/// The resets in a row before this boot, as KM4 reads the register KM0
/// stamped: KM0 already counted this boot.
pub fn resets(register: u32, identity: &[u8; 32]) -> u32 {
    fields(register, identity).1.saturating_sub(1)
}

/// The storm's backoff level, as this boot's KM0 left it.
pub fn level(register: u32, identity: &[u8; 32]) -> u32 {
    let (nibble, _) = fields(register, identity);
    if nibble & STORM_STATE != 0 {
        nibble & LEVEL_MASK
    } else {
        0
    }
}

/// Whether this boot runs outside any trial by KM0's count: storm state,
/// which a trial that ran out with no fallback is in too. KM4 then applies
/// no trial deadline, whatever the confirmation record says.
pub fn past_trial(register: u32, identity: &[u8; 32]) -> bool {
    fields(register, identity).0 & STORM_STATE != 0
}

/// The register once a boot that is not held has run `STABLE_MS`: the
/// burst is over, and the backoff steps down a level.
pub fn settled(register: u32, identity: &[u8; 32]) -> u32 {
    let level = level(register, identity).saturating_sub(1);
    storm_stamp(register, identity, level, 0)
}

/// The register once a held boot has run its [`hold_ms`]: the owner opens,
/// the count starts again, and the next storm in a row holds a level
/// longer.
pub fn released(register: u32, identity: &[u8; 32]) -> u32 {
    let level = level(register, identity) + 1;
    storm_stamp(register, identity, level, 0)
}

/// The register just before a halted boot resets after [`HALT_MS`]: the
/// next boot is held, a level longer, rather than halted again at once.
pub fn halt_over(register: u32, identity: &[u8; 32]) -> u32 {
    let level = level(register, identity) + 1;
    storm_stamp(register, identity, level, STORM_LIMIT)
}

/// The register before an activation's or a give-way's restart: the next
/// boot runs another build, which starts with no count of any kind,
/// whatever tag it shares with the build before it.
pub const fn fresh(register: u32) -> u32 {
    register & BOOT_MASK
}

/// The register before a restart someone asked for (a reboot, an
/// activation, a give-way, setup): not a fault, so the burst ends, and a
/// held feeder the owner restarts tries again at once.
pub fn deliberate(register: u32) -> u32 {
    register & !(FIELD_MASK << STORM_SHIFT)
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
