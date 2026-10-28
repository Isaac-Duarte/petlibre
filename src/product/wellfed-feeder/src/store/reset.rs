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

/// Erase the marker: the reset is over.
pub fn clear<I: SlotIo>(io: &I) -> bool {
    io.erase(0)
}
