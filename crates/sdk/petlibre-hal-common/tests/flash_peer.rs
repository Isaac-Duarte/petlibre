//! KM0 park-request handler against an MMIO model in which KM4 publishes
//! REQUEST and later RELEASE while the handler polls the shared counter.

use std::cell::RefCell;

use petlibre_hal_common::flash::park::{
    ACK, HELLO, Mmio, RELEASE, RELEASED, REQUEST, SYSTICK_CTRL, TICKS,
};
use petlibre_hal_common::flash::peer::{
    BACKUP_BASE, INJECT_LEASE, MARKER_REGISTER, ParkEvent, Peer,
};

#[derive(Default)]
struct Model {
    regs: [[u32; 12]; 2],
    backup: [u32; 8],
    ticks: u32,
    release_at: u32,
    mask: u32,
    systick: u32,
}

thread_local! {
    static M: RefCell<Model> = RefCell::new(Model::default());
}

fn with<R>(f: impl FnOnce(&mut Model) -> R) -> R {
    M.with(|m| f(&mut m.borrow_mut()))
}

struct Km0;

impl Mmio for Km0 {
    const KM0: bool = true;
    fn load(address: u32) -> u32 {
        with(|m| {
            if address == TICKS {
                m.ticks += 1;
                if m.ticks == m.release_at {
                    m.regs[1][10] = RELEASE;
                }
                return m.ticks;
            }

            if address == SYSTICK_CTRL {
                return m.systick;
            }

            if (BACKUP_BASE..BACKUP_BASE + 32).contains(&address) {
                return m.backup[((address - BACKUP_BASE) / 4) as usize];
            }

            // USR words per core: index 0 KM0 (0x4800_6000), 1 KM4 (0x4000_6000).
            let core = usize::from(address & 0xffff_f000 == 0x4000_6000);
            m.regs[core][((address & 0xfff) - 0x20) as usize / 4]
        })
    }
    fn store(address: u32, value: u32) {
        with(|m| {
            if (BACKUP_BASE..BACKUP_BASE + 32).contains(&address) {
                m.backup[((address - BACKUP_BASE) / 4) as usize] = value;
                return;
            }

            let core = usize::from(address & 0xffff_f000 == 0x4000_6000);
            m.regs[core][((address & 0xfff) - 0x20) as usize / 4] = value;
        })
    }
    fn barrier() {}
    fn mask() -> u32 {
        with(|m| m.mask)
    }
    fn halt() -> ! {
        panic!("halted")
    }
}

const A: u64 = 0x0000_0001_1111_1111;

/// A peer that has answered HELLO for `A`, in park context (IRQs masked,
/// SysTick off), with KM4 publishing REQUEST for `lease`.
fn requested(lease: u64) -> Peer {
    with(|m| *m = Model::default());
    with(|m| {
        m.regs[1][2] = A as u32;
        m.regs[1][3] = (A >> 32) as u32;
        m.regs[1][10] = HELLO;
    });

    let mut peer = Peer::default();
    peer.step::<Km0>(1, 5);
    assert_ne!(peer.answered, 0);
    let session = peer.session;

    with(|m| {
        let r = &mut m.regs[1];
        (r[4], r[5], r[6], r[9], r[10]) = (
            session as u32,
            (session >> 32) as u32,
            lease as u32,
            (lease >> 32) as u32,
            REQUEST,
        );
        m.mask = 1;
        m.release_at = 50;
    });

    peer
}

#[test]
fn request_parks_until_the_exact_release() {
    let mut peer = requested(1);
    assert_eq!(peer.on_request::<Km0>(), ParkEvent::RELEASED);
    assert_eq!(with(|m| m.regs[0][10]), RELEASED);
    assert_eq!(peer.last_lease, 1);
    // The same lease again is ignored.
    with(|m| m.regs[1][10] = REQUEST);
    assert_eq!(peer.on_request::<Km0>(), ParkEvent::IGNORED);
}

#[test]
fn foreign_or_unanswered_requests_are_ignored() {
    let mut peer = requested(1);
    with(|m| m.regs[1][4] ^= 1);
    assert_eq!(peer.on_request::<Km0>(), ParkEvent::IGNORED);
    assert_eq!(Peer::default().on_request::<Km0>(), ParkEvent::IGNORED);
    assert_ne!(with(|m| m.regs[0][10]), ACK);
}

#[test]
fn marker_withholds_only_the_injection_lease() {
    let mut peer = requested(INJECT_LEASE);
    with(|m| m.backup[MARKER_REGISTER as usize] = 1);
    assert_eq!(peer.on_request::<Km0>(), ParkEvent::WITHHELD);
    assert_ne!(with(|m| m.regs[0][10]), ACK);

    let mut peer = requested(INJECT_LEASE);
    with(|m| m.backup[MARKER_REGISTER as usize] = 2);
    assert_eq!(peer.on_request::<Km0>(), ParkEvent::RELEASED);
}

#[test]
fn unmasked_context_is_refused_without_ack() {
    let mut peer = requested(1);
    with(|m| m.mask = 0);
    assert_eq!(peer.on_request::<Km0>(), ParkEvent::REFUSED);

    with(|m| {
        m.mask = 1;
        m.systick = 1;
    });
    assert_eq!(peer.on_request::<Km0>(), ParkEvent::REFUSED);
    assert_ne!(with(|m| m.regs[0][10]), ACK);
}
