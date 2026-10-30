//! The handover mark shares its sector with the factory reset's marker.

use std::cell::RefCell;

use wellfed_core::store::handover::{MARKER, RESET_MARKER, clear, mark, pending};
use wellfed_core::store::slot::SlotIo;

struct Sector(RefCell<Vec<u8>>);

impl Sector {
    fn blank() -> Self {
        Self(RefCell::new(vec![0xff; 4096]))
    }

    fn holding(bytes: &[u8]) -> Self {
        let sector = Self::blank();
        sector.0.borrow_mut()[..bytes.len()].copy_from_slice(bytes);
        sector
    }

    fn start(&self) -> Vec<u8> {
        self.0.borrow()[..MARKER.len()].to_vec()
    }
}

impl SlotIo for Sector {
    fn unit_bytes(&self) -> u32 {
        4096
    }
    fn read(&self, _slot: u32, offset: u32, bytes: &mut [u8]) -> bool {
        let at = offset as usize;
        bytes.copy_from_slice(&self.0.borrow()[at..at + bytes.len()]);
        true
    }
    fn program(&self, _slot: u32, offset: u32, bytes: &[u8]) -> bool {
        let at = offset as usize;
        for (cell, byte) in self.0.borrow_mut()[at..].iter_mut().zip(bytes) {
            *cell &= byte;
        }
        true
    }
    fn erase(&self, _slot: u32) -> bool {
        self.0.borrow_mut().fill(0xff);
        true
    }
}

#[test]
fn a_mark_goes_down_and_comes_off() {
    let sector = Sector::blank();
    assert!(mark(&sector));
    assert!(pending(&sector));
    assert!(clear(&sector));
    assert!(!pending(&sector));
}

/// A torn program counts as no handover, and the next mark replaces it.
#[test]
fn torn_bytes_are_no_handover_and_give_way() {
    let sector = Sector::holding(&MARKER[..8]);
    assert!(!pending(&sector));
    assert!(mark(&sector));
    assert!(pending(&sector));
}

/// A factory reset's marker can be down as an activation finishes, ahead of
/// its restart: neither the mark nor taking it back may erase it.
#[test]
fn a_pending_factory_reset_survives_the_mark_and_its_clearing() {
    let sector = Sector::holding(&RESET_MARKER);

    assert!(mark(&sector));
    assert_eq!(sector.start(), RESET_MARKER);
    assert!(clear(&sector));
    assert_eq!(sector.start(), RESET_MARKER);
}
