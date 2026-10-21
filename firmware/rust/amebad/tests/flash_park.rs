//! MMIO-model tests of the RAM park protocol, run for both core encodings.

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};

use petlibro_amebad::flash_park::*;

#[derive(Default)]
struct Model {
    regs: [[u32; 12]; 2],
    mask: u32,
    systick: u32,
    ticks: u32,
    stores: u32,
    barriers: u32,
    tick_reads: u32,
    release_at: u32,
    tear_at: u32,
    peer_reads: u32,
    /// 0 advancing, 1 frozen, 2 backward, 3 foreign, 4 torn, 5 bad command.
    mode: u32,
    foreign_slot: usize,
    semaphore: u32,
    semaphore_writes: u32,
    irq_requests: u32,
    response_at: u32,
    response_delay: u32,
    snapshot_delay: u32,
    race_claim: bool,
    race_release: bool,
    blocked_release: bool,
    owner_wait: bool,
    owner_releasing: bool,
    owner_foreign: bool,
    acked: bool,
    mutate_input: bool,
    input: ParkToken,
    original: ParkToken,
    own: usize,
}

thread_local! {
    static MODEL: RefCell<Model> = RefCell::new(Model::default());
}

fn with<R>(f: impl FnOnce(&mut Model) -> R) -> R {
    MODEL.with(|m| f(&mut m.borrow_mut()))
}

struct Halted;

impl Model {
    fn other(&self) -> usize {
        1 - self.own
    }

    fn message(&mut self, side: usize, command: u32) {
        let o = self.original;
        let r = &mut self.regs[side];
        (r[2], r[3], r[4], r[5], r[6], r[9], r[10]) = (
            o.authority_lo,
            o.authority_hi,
            o.peer_lo,
            o.peer_hi,
            o.lease_lo,
            o.lease_hi,
            command,
        );
    }
}

fn bank(address: u32) -> usize {
    assert!(address & 0xffff_0000 == 0x4800_0000 || address & 0xffff_0000 == 0x4000_0000);
    if address & 0x0800_0000 != 0 { 0 } else { 1 }
}

fn index_of(address: u32) -> usize {
    let offset = address & 0xffff;
    assert!((0x6020..=0x604c).contains(&offset) && offset & 3 == 0);
    ((offset - 0x6020) / 4) as usize
}

struct Fake<const KM0: bool>;

impl<const KM0: bool> Mmio for Fake<KM0> {
    const KM0: bool = KM0;

    fn load(address: u32) -> u32 {
        with(|m| {
            if address == SYSTICK_CTRL {
                return m.systick;
            }
            if address == SEM {
                return m.semaphore;
            }
            if address == TICKS {
                m.tick_reads += 1;
                if m.owner_wait && m.tick_reads == m.response_at {
                    let other = m.other();
                    m.message(other, if m.owner_releasing { RELEASED } else { ACK });
                    if m.owner_foreign {
                        m.regs[other][4] ^= 1;
                    }
                }
                if m.acked && m.tick_reads == m.release_at {
                    let other = m.other();
                    m.message(other, if m.mode == 5 { ACK } else { RELEASE });
                    if m.mode == 3 {
                        let slot = m.foreign_slot;
                        m.regs[other][slot] ^= 1;
                    }
                }
                if m.mode == 1 {
                    return m.ticks;
                }
                if m.mode == 2 && m.acked {
                    m.ticks = m.ticks.wrapping_sub(1);
                    return m.ticks;
                }
                let value = m.ticks;
                m.ticks = m.ticks.wrapping_add(1);
                return value;
            }
            let (side, slot) = (bank(address), index_of(address));
            if side == m.other() && slot == 2 && (m.acked || m.owner_wait) {
                m.ticks = m.ticks.wrapping_add(m.snapshot_delay);
            }
            if side == m.other() && m.acked && m.mode == 4 {
                m.peer_reads += 1;
                if m.peer_reads == m.tear_at {
                    // A command publication interrupted midway through a snapshot.
                    m.regs[side][10] = 0;
                }
            }
            m.regs[side][slot]
        })
    }

    fn store(address: u32, value: u32) {
        with(|m| {
            if address == SEM {
                m.semaphore_writes += 1;
                let index = (8..=15)
                    .find(|i| value == 1 << (i * 2))
                    .expect("one application semaphore");
                let shift = index * 2;
                let field = (m.semaphore >> shift) & 3;
                let (ours, theirs) = if m.own == 0 { (1, 3) } else { (3, 1) };
                if field & 1 == 0 && m.race_claim {
                    m.semaphore = (m.semaphore & !(3 << shift)) | (theirs << shift);
                } else if field & 1 == 0 {
                    m.semaphore = (m.semaphore & !(3 << shift)) | (ours << shift);
                } else if field == ours && !m.blocked_release {
                    m.semaphore &= !(3 << shift);
                    if m.race_release {
                        m.semaphore |= theirs << shift;
                    }
                }
                return;
            }
            if address == if m.own == 0 { 0x4800_6008 } else { 0x4000_6008 } {
                assert!((1 << 16..=1 << 30).contains(&value) && value.is_power_of_two());
                m.irq_requests += 1;
                return;
            }
            let (side, slot) = (bank(address), index_of(address));
            assert!(side == m.own && [2, 3, 4, 5, 6, 9, 10].contains(&slot));
            m.stores += 1;
            m.regs[side][slot] = value;
            if slot == 10 && value != 0 {
                let (r, o) = (m.regs[side], m.original);
                assert_eq!(
                    (r[2], r[3], r[4], r[5], r[6], r[9]),
                    (
                        o.authority_lo,
                        o.authority_hi,
                        o.peer_lo,
                        o.peer_hi,
                        o.lease_lo,
                        o.lease_hi
                    )
                );
                if value == ACK {
                    m.acked = true;
                    if m.mutate_input {
                        m.input = ParkToken::default();
                    }
                }
                if value == REQUEST || value == RELEASE {
                    m.owner_wait = true;
                    m.owner_releasing = value == RELEASE;
                    m.response_at = m.tick_reads + m.response_delay;
                    if m.mutate_input {
                        m.input = ParkToken::default();
                    }
                }
            }
        });
    }

    fn barrier() {
        with(|m| m.barriers += 1);
    }

    fn mask() -> u32 {
        with(|m| m.mask)
    }

    fn halt() -> ! {
        std::panic::panic_any(Halted)
    }
}

fn reset<const KM0: bool>() {
    with(|m| {
        let token = ParkToken {
            authority_lo: 0x1234_5678,
            authority_hi: 0x9abc_def0,
            peer_lo: 0x1357_2468,
            peer_hi: 0xfedc_ba98,
            lease_lo: 1,
            lease_hi: 2,
        };
        *m = Model {
            mask: 1,
            foreign_slot: 5,
            response_delay: 3,
            release_at: if KM0 { 10 } else { 4 },
            tear_at: 3,
            input: token,
            original: token,
            own: if KM0 { 0 } else { 1 },
            ..Model::default()
        };
        let other = m.other();
        m.message(other, REQUEST);
    });
}

/// The input token lives in the model so the routines see caller mutations.
fn input() -> *const ParkToken {
    MODEL.with(|m| &raw const m.borrow().input)
}

fn m<R>(f: impl FnOnce(&Model) -> R) -> R {
    MODEL.with(|m| f(&m.borrow()))
}

fn release_and_private_token<const KM0: bool>() {
    for change in 0..3 {
        reset::<KM0>();
        with(|m| {
            m.mutate_input = change == 1;
            if change == 2 {
                m.ticks = u32::MAX - 2;
            }
        });
        assert!(wait::<Fake<KM0>>(input(), 20, 30));
        assert!(m(|m| m.acked
            && m.regs[m.own][10] == RELEASED
            && m.mask == 1
            && m.systick == 0));
        assert!(m(|m| m.stores == 16 && m.barriers >= 10));
    }
}

fn refuse_before_acknowledgement<const KM0: bool>() {
    for change in 0..13 {
        reset::<KM0>();
        let (mut time_limit, mut polls) = (10, 30);
        with(|m| {
            let other = m.other();
            match change {
                0 => m.mask = 0,
                1 => m.systick = 1,
                2 => time_limit = 0,
                3 => time_limit = 0x8000_0000,
                4 => polls = 0,
                5 => polls = 0x8000_0000,
                6 => (m.input.authority_lo, m.input.authority_hi) = (0, 0),
                7 => (m.input.peer_lo, m.input.peer_hi) = (0, 0),
                8 => (m.input.lease_lo, m.input.lease_hi) = (0, 0),
                9 => m.regs[other][10] = RELEASE,
                10 => m.regs[other][10] = 0,
                11 => m.regs[other][9] ^= 1,
                _ => {}
            }
        });
        let token = if change == 12 {
            core::ptr::null()
        } else {
            input()
        };
        assert!(!wait::<Fake<KM0>>(token, time_limit, polls));
        assert!(m(|m| !m.acked && m.stores == 0));
    }
}

fn faults_after_ack_never_return<const KM0: bool>() {
    for change in 0..12 {
        reset::<KM0>();
        with(|m| match change {
            0 => {
                m.mode = 1; // Frozen timer, finite poll budget.
                m.release_at = 100;
            }
            1 => m.mode = 2,
            2..=7 => {
                m.mode = 3;
                m.foreign_slot = [2, 3, 4, 5, 6, 9][change - 2];
            }
            8 => m.release_at = 100, // No release before elapsed bound.
            9 => m.release_at = 4,   // Release exactly at elapsed bound.
            10 => m.mode = 5,
            _ => {
                m.release_at = if KM0 { 4 } else { 2 };
                m.snapshot_delay = 20;
            }
        });
        let (ticks, polls) = (
            if change == 9 { 3 } else { 10 },
            if change == 8 { 30 } else { 3 },
        );
        let token = input();
        let outcome = catch_unwind(AssertUnwindSafe(|| wait::<Fake<KM0>>(token, ticks, polls)));
        let halted = outcome.err().is_some_and(|e| e.is::<Halted>());
        assert!(halted, "an acknowledged fault must stay in RAM for reset");
        let expected = match change {
            1 => CLOCK,
            2..=7 | 10 => PEER_LOST,
            _ => TIMEOUT,
        };
        assert!(m(|m| m.acked
            && m.regs[m.own][10] == expected
            && m.mask == 1
            && m.systick == 0));
    }
}

fn publication_tearing_and_reserved_slots<const KM0: bool>() {
    for cut in 1..=8 {
        reset::<KM0>();
        with(|m| {
            m.mode = 4;
            m.tear_at = cut;
            for bank in &mut m.regs {
                (bank[0], bank[1], bank[7], bank[8], bank[11]) = (0x10, 0x20, 0x30, 0x40, 0x50);
            }
        });
        assert!(wait::<Fake<KM0>>(input(), 15, 40));
        assert!(m(|m| m.regs.iter().all(|b| (
            b[0], b[1], b[7], b[8], b[11]
        ) == (
            0x10, 0x20, 0x30, 0x40, 0x50
        ))));
    }
    reset::<KM0>();
    assert!(!publish::<Fake<KM0>>(input(), ACK));
    assert!(!publish::<Fake<KM0>>(core::ptr::null(), REQUEST));
    assert_eq!(m(|m| m.stores), 0);
    assert!(publish::<Fake<KM0>>(input(), REQUEST));
    assert!(m(|m| m.regs[m.own][10] == REQUEST
        && m.stores == 8
        && m.barriers == 3));
    assert!(publish::<Fake<KM0>>(input(), RELEASE));
    assert!(m(|m| m.regs[m.own][10] == RELEASE
        && m.stores == 16
        && m.barriers == 6));
}

fn bounded_semaphore_attempts_and_ownership<const KM0: bool>() {
    for index in 0..32 {
        reset::<KM0>();
        with(|m| m.semaphore = 0x5555); // Realtek-reserved fields must not be touched.
        if !(8..=15).contains(&index) {
            assert!(!sem_try_claim::<Fake<KM0>>(index));
            assert!(!sem_release::<Fake<KM0>>(index) && m(|m| m.semaphore_writes) == 0);
            continue;
        }
        with(|m| m.mask = 0);
        assert!(!sem_try_claim::<Fake<KM0>>(index) && m(|m| m.semaphore_writes) == 0);
        with(|m| m.mask = 1);
        assert!(sem_try_claim::<Fake<KM0>>(index));
        assert!(!sem_try_claim::<Fake<KM0>>(index) && m(|m| m.semaphore_writes) == 1);
        assert_eq!(m(|m| m.semaphore) & 0xffff, 0x5555);
        with(|m| m.blocked_release = true);
        assert!(!sem_release::<Fake<KM0>>(index));
        with(|m| {
            m.blocked_release = false;
            m.race_release = true;
        });
        assert!(sem_release::<Fake<KM0>>(index)); // Peer claims immediately.
        let writes = m(|m| m.semaphore_writes);
        assert!(!sem_release::<Fake<KM0>>(index) && m(|m| m.semaphore_writes) == writes);
        reset::<KM0>();
        with(|m| m.race_claim = true);
        assert!(!sem_try_claim::<Fake<KM0>>(index));
        assert!(m(|m| m.semaphore_writes == 1 && m.mask == 1));
        reset::<KM0>();
        with(|m| m.semaphore = 2 << (index * 2)); // Free lock with stale owner bits.
        assert!(sem_try_claim::<Fake<KM0>>(index));
        assert!(sem_release::<Fake<KM0>>(index));
    }
}

fn older_lease_reply(m: &mut Model) {
    let other = m.other();
    m.message(other, RELEASED);
    m.regs[other][6] = 0;
}

fn owner_requires_current_ack_and_verified_release<const KM0: bool>() {
    for change in 0..3 {
        reset::<KM0>();
        with(|m| {
            older_lease_reply(m);
            if change == 1 {
                m.ticks = u32::MAX - 2;
            }
            m.mutate_input = change == 2;
        });
        assert!(sem_try_claim::<Fake<KM0>>(8));
        assert!(owner_begin::<Fake<KM0>>(input(), 16, 20, 30));
        assert!(m(
            |m| m.regs[m.own][10] == REQUEST && m.regs[m.other()][10] == ACK
        ));
        assert!(m(|m| m.irq_requests == 1 && m.mask == 1 && m.systick == 0));
        with(|m| {
            m.mutate_input = false;
            m.input = m.original;
        });
        assert!(owner_end::<Fake<KM0>>(input(), 30, 20, 30));
        assert!(m(
            |m| m.regs[m.own][10] == RELEASE && m.regs[m.other()][10] == RELEASED
        ));
        assert!(m(|m| m.irq_requests == 1 && m.mask == 1 && m.systick == 0));
        assert!(sem_release::<Fake<KM0>>(8));
    }
}

fn owner_bad_context_stale_reply_and_deadlines_never_grant<const KM0: bool>() {
    for change in 0..9 {
        reset::<KM0>();
        let mut channel = 16;
        with(|m| {
            older_lease_reply(m);
            match change {
                0 => channel = 15,
                1 => channel = 31,
                2 => m.mask = 0,
                3 => m.systick = 7,
                4 => m.response_delay = 100,
                5 => {
                    m.response_delay = 100;
                    m.mode = 1;
                }
                6 => m.owner_foreign = true,
                7 => {
                    let other = m.other();
                    m.message(other, RELEASED);
                    m.response_delay = 100;
                }
                _ => {
                    m.snapshot_delay = 20;
                    m.response_delay = 1;
                }
            }
        });
        assert!(!owner_begin::<Fake<KM0>>(input(), channel, 10, 8));
        if change < 4 {
            assert!(m(|m| m.stores == 0 && m.irq_requests == 0));
        } else {
            assert!(m(|m| m.irq_requests == 1 && m.regs[m.own][10] >= TIMEOUT));
        }
    }
    reset::<KM0>();
    with(|m| {
        let other = m.other();
        m.message(other, ACK);
        m.owner_foreign = true;
    });
    assert!(!owner_end::<Fake<KM0>>(input(), 16, 20, 30));
    assert!(m(|m| m.regs[m.own][10] == PEER_LOST && m.mask == 1));
}

macro_rules! both_cores {
    ($($name:ident),*) => {
        mod km0 { $(#[test] fn $name() { super::$name::<true>(); })* }
        mod km4 { $(#[test] fn $name() { super::$name::<false>(); })* }
    };
}

both_cores!(
    release_and_private_token,
    refuse_before_acknowledgement,
    faults_after_ack_never_return,
    publication_tearing_and_reserved_slots,
    bounded_semaphore_attempts_and_ownership,
    owner_requires_current_ack_and_verified_release,
    owner_bad_context_stale_reply_and_deadlines_never_grant
);
