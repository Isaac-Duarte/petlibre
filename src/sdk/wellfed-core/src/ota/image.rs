//! How long the image in a bank is, from its own headers, so the feeder
//! can hash exactly the image the factory loader maps.
//!
//! A bank holds five core images back to back, each a 32-byte header (the
//! loader marker, the body's length, its load address and four erased
//! words) and its body: KM0's XIP and RAM parts, then, from the next 4 KiB
//! boundary, KM4's XIP, RAM and PSRAM parts. The image ends at the 4 KiB
//! boundary after the last body. This is the one profile the reviewed
//! builds and stock 1.1.15 use (`wellfed-flash`'s `ota::images`); a bank
//! that does not match it has no extent, so nothing trusts it.

use super::{MARKER, SECTOR_BYTES};

/// Each part's load address, in bank order; KM4's XIP part starts a sector.
const PARTS: [(u32, bool); 5] = [
    (0x0C00_0020, false),
    (0x0008_3000, false),
    (0x0E00_0020, true),
    (0x1000_5000, false),
    (0x0200_0020, false),
];
const HEADER: u32 = 32;

fn word(header: &[u8; 32], index: usize) -> u32 {
    let at = index * 4;
    u32::from_le_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]])
}

/// The image's length in bytes, at most `limit`, reading 32-byte headers at
/// bank offsets through `read`. `None` when the headers do not match the
/// profile or the image would pass `limit`.
pub fn extent(mut read: impl FnMut(u32, &mut [u8; 32]) -> bool, limit: u32) -> Option<u32> {
    let mut offset = 0u32;
    for (address, aligned) in PARTS {
        if aligned {
            offset = offset.checked_next_multiple_of(SECTOR_BYTES)?;
        }
        let mut header = [0u8; 32];
        if offset.checked_add(HEADER)? > limit || !read(offset, &mut header) {
            return None;
        }
        let erased = (4..8).all(|i| word(&header, i) == u32::MAX);
        if header[..8] != MARKER || word(&header, 3) != address || !erased {
            return None;
        }

        let body = word(&header, 2);
        // Only the PSRAM part is empty, and it always is.
        if (address == PARTS[4].0) != (body == 0) {
            return None;
        }
        offset = offset.checked_add(HEADER)?.checked_add(body)?;
    }

    let end = offset.checked_next_multiple_of(SECTOR_BYTES)?;
    (end <= limit).then_some(end)
}
