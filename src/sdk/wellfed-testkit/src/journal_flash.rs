//! NOR model of equal erase units (two by default) enforcing aligned 1->0 programming, with power-cut and
//! call-failure injection.
//!
//! Programming is byte-ordered: a cut leaves a durable prefix of the bytes.
//! Erase raises every bit of the unit in a deterministic pseudo-random order,
//! so a cut during erase leaves old data with an arbitrary subset of bits
//! raised rather than a clean 0xFF prefix. Neither model covers weakly
//! programmed or weakly erased cells that read back inconsistently.

use core::cell::RefCell;
use core::ffi::{c_uint, c_void};
use std::rc::Rc;

use wellfed_core::store::slot::JournalIo;

/// Default unit size, as used by the virtual feeder image.
pub const UNIT: usize = 256;

#[derive(Clone)]
pub struct FakeJournalFlash {
    /// Equal-sized units; see [`FakeJournalFlash::with_units`].
    pub bytes: Vec<Vec<u8>>,
    /// Remaining mutations before power is cut; negative is unlimited. A
    /// programmed byte is one mutation; erase is `unit` mutations of eight
    /// raised bits each.
    pub budget: i64,

    pub calls: u32,
    /// Fail this 1-based call number; zero disables injection.
    pub fail_call: u32,
    pub mutations: u32,

    pub silent_program: bool,
    pub silent_erase: bool,

    /// Runs after every durable mutation. No reference to the flash is live
    /// while it runs, so it may read the flash through the I/O context.
    pub after_mutation: Option<fn(*mut c_void)>,
    pub mutation_context: *mut c_void,
}

impl Default for FakeJournalFlash {
    /// Fully erased 256-byte units with no fault injection.
    fn default() -> Self {
        Self::with_unit(UNIT)
    }
}

impl FakeJournalFlash {
    /// Two fully erased units of `unit` bytes with no fault injection.
    pub fn with_unit(unit: usize) -> Self {
        Self::with_units(unit, 2)
    }

    /// `count` fully erased units of `unit` bytes, for a ring of sectors.
    pub fn with_units(unit: usize, count: usize) -> Self {
        assert!(unit >= 4 && unit.is_multiple_of(4) && u32::try_from(unit).is_ok());
        Self {
            bytes: vec![vec![u8::MAX; unit]; count],
            budget: -1,
            calls: 0,
            fail_call: 0,
            mutations: 0,
            silent_program: false,
            silent_erase: false,
            after_mutation: None,
            mutation_context: core::ptr::null_mut(),
        }
    }

    pub fn unit(&self) -> usize {
        self.bytes[0].len()
    }

    /// I/O bound to this flash. The flash must not move while the I/O is used,
    /// and writing its fields directly invalidates the I/O: bind a new one.
    pub fn io(&mut self) -> JournalIo {
        // SAFETY: `self` is a live, exclusive flash.
        unsafe { Self::io_at(self) }
    }

    /// I/O whose context is exactly `flash`, without an intermediate
    /// reference, so the owner of `flash` can keep reading it through the
    /// same pointer between calls.
    ///
    /// # Safety
    /// `flash` must point to a live flash that stays in place, and no
    /// reference to it may be live while the returned callbacks run.
    pub unsafe fn io_at(flash: *mut Self) -> JournalIo {
        JournalIo {
            context: flash.cast(),
            // SAFETY: the caller guarantees a live flash; this is a read only.
            unit_bytes: u32::try_from(unsafe { (*flash).unit() }).expect("a unit below 4 GiB"),
            read: Some(read_bytes),
            program: Some(program),
            erase: Some(erase),
        }
    }

    fn call(&mut self) -> bool {
        self.calls += 1;
        self.fail_call == 0 || self.calls != self.fail_call
    }

    fn bounds(&self, slot: c_uint, offset: u32, size: usize) -> (usize, usize) {
        let (slot, offset) = (slot as usize, offset as usize);
        let unit = self.unit();
        assert!(slot < self.bytes.len() && offset <= unit && size <= unit - offset);
        (slot, offset)
    }
}

/// Apply one mutation, then run the hook with no reference to the flash live.
///
/// # Safety
/// `flash` is a callback context created by [`FakeJournalFlash::io_at`].
unsafe fn mutate(flash: *mut FakeJournalFlash, apply: impl FnOnce(&mut FakeJournalFlash)) -> bool {
    let (hook, context) = {
        // SAFETY: the context is live; this reference ends before the hook.
        let f = unsafe { &mut *flash };
        if f.budget == 0 {
            return false;
        }
        apply(f);
        f.mutations += 1;
        (f.after_mutation, f.mutation_context)
    };

    if let Some(hook) = hook {
        hook(context);
    }

    // SAFETY: as above; a fresh reference after the hook returned.
    let f = unsafe { &mut *flash };
    if f.budget > 0 {
        f.budget -= 1;
    }

    // Includes failure after the last durable mutation.
    f.budget != 0
}

/// Slot, unit size and erase order.
type CachedOrder = (usize, usize, Rc<Vec<u32>>);

/// Deterministic order in which erase raises the unit's bits, cached per
/// slot and unit size.
fn erase_order(slot: usize, unit: usize) -> Rc<Vec<u32>> {
    thread_local! {
        static ORDERS: RefCell<Vec<CachedOrder>> = const { RefCell::new(Vec::new()) };
    }

    ORDERS.with_borrow_mut(|orders| {
        if let Some((_, _, order)) = orders.iter().find(|(s, u, _)| *s == slot && *u == unit) {
            return order.clone();
        }
        let order = Rc::new(shuffled_bits(slot, unit));
        orders.push((slot, unit, order.clone()));
        order
    })
}

fn shuffled_bits(slot: usize, unit: usize) -> Vec<u32> {
    let bits = u32::try_from(unit * 8).expect("a unit below 512 MiB");
    let mut order: Vec<u32> = (0..bits).collect();
    let mut state =
        0x9e37_79b9_7f4a_7c15u64 ^ (slot as u64 + 1).wrapping_mul(0xbf58_476d_1ce4_e5b9);

    for i in (1..order.len()).rev() {
        // xorshift64*: reproducible across runs and platforms.
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let j = (state.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 32) as usize % (i + 1);
        order.swap(i, j);
    }

    order
}

unsafe extern "C" fn read_bytes(
    context: *mut c_void,
    slot: c_uint,
    offset: u32,
    bytes: *mut c_void,
    size: usize,
) -> bool {
    // SAFETY: the context was created by `io_at`; no other reference is live.
    let f = unsafe { &mut *context.cast::<FakeJournalFlash>() };
    let (slot, offset) = f.bounds(slot, offset, size);
    if !f.call() {
        return false;
    }

    // SAFETY: the journal passes a writable buffer of `size` bytes, distinct
    // from the flash.
    let out = unsafe { core::slice::from_raw_parts_mut(bytes.cast::<u8>(), size) };
    out.copy_from_slice(&f.bytes[slot][offset..offset + size]);
    true
}

unsafe extern "C" fn program(
    context: *mut c_void,
    slot: c_uint,
    offset: u32,
    bytes: *const c_void,
    size: usize,
) -> bool {
    let flash = context.cast::<FakeJournalFlash>();
    let (slot, offset) = {
        // SAFETY: the context was created by `io_at`; ends before any hook.
        let f = unsafe { &mut *flash };
        let (slot, offset) = f.bounds(slot, offset, size);
        assert!(offset.is_multiple_of(4) && size.is_multiple_of(4));
        if !f.call() {
            return false;
        }
        if f.silent_program {
            return true;
        }
        (slot, offset)
    };

    // SAFETY: the journal passes `size` readable bytes, distinct from the flash.
    let input = unsafe { core::slice::from_raw_parts(bytes.cast::<u8>(), size) };
    for (i, &value) in input.iter().enumerate() {
        let index = offset + i;
        // SAFETY: no reference into the flash is live across this call.
        let more = unsafe {
            mutate(flash, |f| {
                assert_eq!(
                    f.bytes[slot][index] & value,
                    value,
                    "NOR programming only clears bits"
                );
                f.bytes[slot][index] = value;
            })
        };
        if !more {
            return false;
        }
    }

    true
}

unsafe extern "C" fn erase(context: *mut c_void, slot: c_uint) -> bool {
    let flash = context.cast::<FakeJournalFlash>();
    let slot = slot as usize;

    let unit = {
        // SAFETY: the context was created by `io_at`; ends before any hook.
        let f = unsafe { &mut *flash };
        assert!(slot < f.bytes.len());
        if !f.call() {
            return false;
        }
        if f.silent_erase {
            return true;
        }
        f.unit()
    };

    let order = erase_order(slot, unit);
    // SAFETY: no reference into the flash is live across these calls.
    order.chunks(8).all(|bits| unsafe {
        mutate(flash, |f| {
            for &bit in bits {
                f.bytes[slot][bit as usize / 8] |= 1 << (bit % 8);
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn erase_with_budget(unit: usize, budget: i64) -> (FakeJournalFlash, bool) {
        let mut f = FakeJournalFlash::with_unit(unit);
        f.bytes[0].fill(0);
        f.budget = budget;
        let io = f.io();
        // SAFETY: `f` stays in place and is not otherwise used during the call.
        let ok = unsafe { io.erase.unwrap()(io.context, 0) };

        (f, ok)
    }

    #[test]
    fn complete_erase_and_programming_rules() {
        let (f, ok) = erase_with_budget(4096, -1);
        assert!(ok && f.bytes[0].iter().all(|&b| b == u8::MAX));
        assert_eq!(f.mutations, 4096);

        let mut f = FakeJournalFlash::default();
        let io = f.io();
        let data = [0x0fu8, 0xf0, 0x00, 0xff];
        // SAFETY: the context is this test's live flash and `data` is 4 bytes.
        assert!(unsafe { io.program.unwrap()(io.context, 1, 4, data.as_ptr().cast(), 4) });

        assert_eq!(f.bytes[1][4..8], data);
        assert_eq!(f.io().unit_bytes, u32::try_from(UNIT).unwrap());
    }

    #[test]
    fn torn_erase_is_deterministic_and_not_a_prefix() {
        for unit in [256, 4096] {
            for cut in [1, unit as i64 / 3, unit as i64 - 1] {
                let (f, ok) = erase_with_budget(unit, cut);
                let (again, _) = erase_with_budget(unit, cut);
                assert!(!ok && f.bytes[0] == again.bytes[0]);

                let raised: u32 = f.bytes[0].iter().map(|b| b.count_ones()).sum();
                assert_eq!(u64::from(raised), cut as u64 * 8);

                // Raised bits are spread across the unit, not a byte prefix.
                let prefix = f.bytes[0].iter().take_while(|&&b| b == u8::MAX).count();
                assert!(prefix < unit / 2 && f.bytes[0].iter().any(|&b| b != 0 && b != u8::MAX));
            }
        }

        // The two units tear differently.
        assert_ne!(erase_order(0, UNIT), erase_order(1, UNIT));
    }

    #[test]
    fn hook_runs_without_live_reference_and_cut_stops_mutation() {
        fn hook(context: *mut c_void) {
            // Reads the flash through the same pointer as the I/O context.
            let f = context.cast::<FakeJournalFlash>();
            // SAFETY: the flash outlives the hook, and the I/O holds no
            // reference while it runs.
            let seen = unsafe { (&(*f).bytes)[0][0] };
            assert!(seen == 0x0f || seen == u8::MAX);
        }

        let mut f = Box::new(FakeJournalFlash::default());
        let raw: *mut FakeJournalFlash = &raw mut *f;
        // SAFETY: `raw` points at the boxed flash, which outlives this test.
        unsafe {
            (*raw).after_mutation = Some(hook);
            (*raw).mutation_context = raw.cast();
            (*raw).budget = 2;

            let io = FakeJournalFlash::io_at(raw);
            let data = [0x0fu8, 0x0f, 0x0f, 0x0f];
            assert!(!io.program.unwrap()(
                io.context,
                0,
                0,
                data.as_ptr().cast(),
                4
            ));

            assert_eq!((&(*raw).bytes)[0][..4], [0x0f, 0x0f, 0xff, 0xff]);
            assert!(!io.program.unwrap()(
                io.context,
                0,
                4,
                data.as_ptr().cast(),
                4
            ));

            assert_eq!((*raw).mutations, 2);
        }
    }
}
