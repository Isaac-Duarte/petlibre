//! Boot handshake between the KM4 authority and the KM0 peer over the
//! reserved IPC USR words, with both cores sharing one register model.

use std::cell::RefCell;

use petlibre_amebad_common::flash::park::{
    HELLO, Mmio, READY, REQUEST, Reply, answer, hello, reannounce, reply, session,
};

thread_local! {
    /// USR words 0..11 per core: index 0 KM0 (0x4800_6000), 1 KM4 (0x4000_6000).
    static REGS: RefCell<[[u32; 12]; 2]> = const { RefCell::new([[0; 12]; 2]) };
}

fn locate(address: u32) -> (usize, usize) {
    let core = match address & 0xffff_f000 {
        0x4800_6000 => 0,
        0x4000_6000 => 1,
        _ => panic!("unexpected address {address:#x}"),
    };
    let offset = address & 0xfff;
    assert!((0x20..0x50).contains(&offset) && offset & 3 == 0);
    (core, ((offset - 0x20) / 4) as usize)
}

struct Core<const KM0: bool>;

impl<const KM0: bool> Mmio for Core<KM0> {
    const KM0: bool = KM0;
    fn load(address: u32) -> u32 {
        let (c, i) = locate(address);
        REGS.with(|r| r.borrow()[c][i])
    }
    fn store(address: u32, value: u32) {
        let (c, i) = locate(address);
        REGS.with(|r| r.borrow_mut()[c][i] = value);
    }
    fn barrier() {}
    fn mask() -> u32 {
        0
    }
    fn halt() -> ! {
        panic!("halt")
    }
}

type Km0 = Core<true>;
type Km4 = Core<false>;

fn reset() {
    REGS.with(|r| *r.borrow_mut() = [[0; 12]; 2]);
}

fn usr(core: usize, index: usize) -> u32 {
    REGS.with(|r| r.borrow()[core][index])
}

const A: u64 = 0x0000_0001_1234_5679;
const P: u64 = 0x0000_0001_4b30_abcd;

#[test]
fn sessions_are_nonzero_and_distinct_per_core() {
    for ticks in [0, 1, 0x4b30_0000, 0x4b34_0000, u32::MAX] {
        for count in [0, 1, u32::MAX] {
            let (a, b) = (session(count, ticks, false), session(count, ticks, true));
            assert!(a != 0 && b != 0 && a != b);
        }
    }
    // A single-core reset bumps the count, so the session changes.
    assert_ne!(session(1, 77, true), session(2, 77, true));
}

#[test]
fn hello_ready_round_trip() {
    reset();
    let mut answered = 0;
    assert_eq!(reply::<Km4>(A), Reply::Waiting);
    assert_eq!(answer::<Km0>(P, &mut answered), 0);
    assert!(hello::<Km4>(A));
    assert_eq!(
        (usr(1, 10), usr(1, 2), usr(1, 3)),
        (HELLO, A as u32, (A >> 32) as u32)
    );
    assert_eq!(answer::<Km0>(P, &mut answered), A);
    assert_eq!(answered, A);
    assert_eq!(usr(0, 10), READY);
    assert_eq!(reply::<Km4>(A), Reply::Ready(P));
    // The same HELLO is answered once.
    assert_eq!(answer::<Km0>(P, &mut answered), 0);
}

#[test]
fn stale_or_foreign_replies_are_never_ready() {
    reset();
    let mut answered = 0;
    assert!(hello::<Km4>(A));
    assert_eq!(answer::<Km0>(P, &mut answered), A);
    // A new KM4 boot announces a different authority: the old READY is foreign.
    let b = A + 2;
    assert_eq!(reply::<Km4>(b), Reply::Foreign);
    assert!(hello::<Km4>(b));
    assert_eq!(answer::<Km0>(P, &mut answered), b);
    assert_eq!(reply::<Km4>(b), Reply::Ready(P));
    // A non-handshake command or a lease word never reads as READY.
    REGS.with(|r| r.borrow_mut()[0][10] = REQUEST);
    assert_eq!(reply::<Km4>(b), Reply::Waiting);
    REGS.with(|r| {
        let mut r = r.borrow_mut();
        r[0][10] = READY;
        r[0][6] = 1;
    });
    assert_eq!(reply::<Km4>(b), Reply::Foreign);
}

#[test]
fn roles_and_zero_sessions_are_refused() {
    reset();
    let mut answered = 0;
    assert!(!hello::<Km0>(A) && !hello::<Km4>(0));
    assert_eq!(answer::<Km4>(P, &mut answered), 0);
    assert_eq!(answer::<Km0>(0, &mut answered), 0);
    assert_eq!(reply::<Km0>(A), Reply::Foreign);
    // A HELLO naming a zero authority is not answered.
    REGS.with(|r| r.borrow_mut()[1][10] = HELLO);
    assert_eq!(answer::<Km0>(P, &mut answered), 0);
    assert!(!reannounce::<Km0>(P, 0) && !reannounce::<Km4>(P, A));
}

#[test]
fn reannounced_peer_session_is_visible_to_the_authority() {
    reset();
    let mut answered = 0;
    assert!(hello::<Km4>(A));
    assert_eq!(answer::<Km0>(P, &mut answered), A);
    let rebooted = P + 0x1_0000_0000;
    assert!(reannounce::<Km0>(rebooted, answered));
    assert_eq!(reply::<Km4>(A), Reply::Ready(rebooted));
}
