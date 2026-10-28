//! The factory reset's marker on NOR: one sector (`storage::RESET`) that
//! says a reset was asked for and has not finished.
//!
//! A reset spans many sectors, and a power cut can fall between any two
//! erases. So the settings writer programs the marker before it changes
//! anything, the boot that finds it erases every cleared store before it
//! opens one, and the marker is erased last. A cut before the marker lands
//! leaves the feeder as it was (the owner holds Reset again); a cut after it
//! leaves a reset the next boot finishes, however far it got.
//!
//! The marker is a fixed 16-byte pattern at the sector's start. Only that
//! exact pattern counts: a torn program reads as no reset (nothing was
//! erased yet), and a torn erase of the marker at the very end either still
//! reads as the pattern, so the boot erases the cleared stores again, or as
//! no reset, which is right because they are already erased. Bits a sector
//! never written may hold are not a reset either.
//!
//! After the marker come [`ATTEMPTS`] attempt words, each programmed (no
//! erase) by an armed boot as it starts the erases. A boot's reset flags
//! cannot tell an intentional restart from a hang (both are the watchdog's
//! reset on this board), and every backup register is taken, so the count
//! lives here: it bounds the restarts a boot that cannot finish asks for.

use wellfed_core::store::slot::SlotIo;

/// The marker's bytes, at the start of its sector.
pub const MARKER: [u8; 16] = *b"PLFRST01FACTORY!";

/// Whether `bytes`, read from the marker's sector, say a reset is pending.
pub fn marked(bytes: &[u8; MARKER.len()]) -> bool {
    *bytes == MARKER
}

/// Whether the marker sector `io` (one unit) holds a pending reset. A
/// sector that cannot be read holds none: the stores open as they are.
pub fn pending<I: SlotIo>(io: &I) -> bool {
    let mut bytes = [0; MARKER.len()];
    io.read(0, 0, &mut bytes) && marked(&bytes)
}

/// Program the marker: erase the sector first unless it is blank (or
/// already marked), then program and read the pattern back.
pub fn mark<I: SlotIo>(io: &I) -> bool {
    let mut bytes = [0; MARKER.len()];
    if !io.read(0, 0, &mut bytes) {
        return false;
    }
    if marked(&bytes) {
        return true;
    }

    if bytes.iter().any(|&byte| byte != u8::MAX) && !io.erase(0) {
        return false;
    }
    io.program(0, 0, &MARKER) && pending(io)
}

/// Erase the marker and its attempts: the reset is over.
pub fn clear<I: SlotIo>(io: &I) -> bool {
    io.erase(0)
}

/// Armed boots that may start the reset's erases: after this many, a boot
/// that cannot finish waits for a power cycle instead of asking for a
/// restart. The first is the boot the hold restarts into; the second is the
/// one escape restart.
pub const ATTEMPTS: u32 = 2;

/// What an attempt programs, in its own 16 bytes after the marker.
pub const ATTEMPT: [u8; 16] = *b"PLFRST01ATTEMPT!";

/// The byte offset of attempt `index`.
fn attempt_at(index: u32) -> u32 {
    (index + 1) * 16
}

/// How many boots started the erases, up to [`ATTEMPTS`]. Any programmed
/// bit counts, so a torn attempt is one: it can only stop a restart, never
/// start one. Unreadable reads as all used.
pub fn attempts<I: SlotIo>(io: &I) -> u32 {
    (0..ATTEMPTS)
        .filter(|&index| {
            let mut bytes = [0; ATTEMPT.len()];
            !io.read(0, attempt_at(index), &mut bytes) || bytes.iter().any(|&b| b != u8::MAX)
        })
        .count()
        .try_into()
        .unwrap_or(ATTEMPTS)
}

/// Record that this boot starts the erases: program the next blank
/// attempt, with no erase, so it survives every reset until the marker is
/// cleared. True once recorded, or when every attempt is already used.
pub fn note_attempt<I: SlotIo>(io: &I) -> bool {
    let used = attempts(io);
    if used >= ATTEMPTS {
        return true;
    }
    io.program(0, attempt_at(used), &ATTEMPT)
}
