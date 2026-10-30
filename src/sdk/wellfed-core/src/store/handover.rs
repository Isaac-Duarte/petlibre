//! The mark bank 0 leaves when it activates a stock image: whichever Wellfed
//! build next boots on this feeder finds the owner went back to stock, and
//! starts over as a first install does, with no Wi-Fi and no clients but the
//! same identity (the owner's backup is filed by it), and no meals, so setup
//! starts by itself.
//!
//! Stock runs nothing of ours, so the clearing cannot happen until a Wellfed
//! build boots again: a later install, or the loader falling back because
//! stock did not boot. Either way the next boot clears before it opens the
//! stores to anyone, then erases this mark, so a cut anywhere in that repeats
//! the clearing.
//!
//! It shares the factory reset's sector, with a pattern of its own: only the
//! exact bytes count, so a torn program reads as no handover and the feeder
//! keeps its owner. A reset asked for later erases the sector and clears more,
//! and one already down stays: the writer takes a reset at any time, and its
//! marker can be in place while an activation finishes, ahead of the restart.

use crate::store::slot::SlotIo;

/// The mark's bytes, at the start of its sector.
pub const MARKER: [u8; 16] = *b"PLFRST01HANDOVER";

/// The factory reset's marker, which never gives way to this mark.
pub const RESET_MARKER: [u8; 16] = *b"PLFRST01FACTORY!";

/// Whether `bytes`, read from the mark's sector, say a handover is pending.
pub fn marked(bytes: &[u8; MARKER.len()]) -> bool {
    *bytes == MARKER
}

/// Whether the sector `io` (one unit) holds a pending handover. A sector that
/// cannot be read holds none.
pub fn pending<I: SlotIo>(io: &I) -> bool {
    let mut bytes = [0; MARKER.len()];
    io.read(0, 0, &mut bytes) && marked(&bytes)
}

/// Program the mark, erasing first unless the sector is blank or already
/// marked, and read it back. With a factory reset pending there is nothing to
/// do: the next boot clears all of this and more.
pub fn mark<I: SlotIo>(io: &I) -> bool {
    let mut bytes = [0; MARKER.len()];
    if !io.read(0, 0, &mut bytes) {
        return false;
    }
    if marked(&bytes) || bytes == RESET_MARKER {
        return true;
    }

    if bytes.iter().any(|&byte| byte != u8::MAX) && !io.erase(0) {
        return false;
    }
    io.program(0, 0, &MARKER) && pending(io)
}

/// Erase the mark: the handover is over, or never happened. A factory reset's
/// marker stays.
pub fn clear<I: SlotIo>(io: &I) -> bool {
    let mut bytes = [0; MARKER.len()];
    if io.read(0, 0, &mut bytes) && bytes == RESET_MARKER {
        return true;
    }
    io.erase(0)
}
