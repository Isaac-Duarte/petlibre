//! Two-unit NOR model enforcing aligned 1->0 programming, with power-cut and
//! call-failure injection.

use core::ffi::{c_uint, c_void};

use petlibro_core::feed_journal::JournalIo;

pub const UNIT: usize = 256;

#[derive(Clone)]
pub struct FakeJournalFlash {
    pub bytes: [[u8; UNIT]; 2],
    /// Remaining byte mutations before power is cut; negative is unlimited.
    pub budget: i64,
    pub calls: u32,
    /// Fail this 1-based call number; zero disables injection.
    pub fail_call: u32,
    pub mutations: u32,
    pub silent_program: bool,
    pub silent_erase: bool,
    pub after_mutation: Option<fn(*mut c_void)>,
    pub mutation_context: *mut c_void,
}

impl Default for FakeJournalFlash {
    /// Fully erased units with no fault injection.
    fn default() -> Self {
        Self {
            bytes: [[u8::MAX; UNIT]; 2],
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
}

impl FakeJournalFlash {
    /// I/O bound to this flash. The flash must not move while the I/O is used.
    pub fn io(&mut self) -> JournalIo {
        JournalIo {
            context: (self as *mut Self).cast(),
            unit_bytes: UNIT as u32,
            read: Some(read_bytes),
            program: Some(program),
            erase: Some(erase),
        }
    }

    fn call(&mut self) -> bool {
        self.calls += 1;
        self.fail_call == 0 || self.calls != self.fail_call
    }

    fn mutate(&mut self, slot: usize, index: usize, value: u8) -> bool {
        if self.budget == 0 {
            return false;
        }
        self.bytes[slot][index] = value;
        self.mutations += 1;
        if let Some(hook) = self.after_mutation {
            hook(self.mutation_context);
        }
        if self.budget > 0 {
            self.budget -= 1;
        }
        // Includes failure after the last durable byte.
        self.budget != 0
    }
}

fn bounds(slot: c_uint, offset: u32, size: usize) -> (usize, usize) {
    let (slot, offset) = (slot as usize, offset as usize);
    assert!(slot < 2 && offset <= UNIT && size <= UNIT - offset);
    (slot, offset)
}

unsafe extern "C" fn read_bytes(
    context: *mut c_void,
    slot: c_uint,
    offset: u32,
    bytes: *mut c_void,
    size: usize,
) -> bool {
    let f = unsafe { &mut *context.cast::<FakeJournalFlash>() };
    let (slot, offset) = bounds(slot, offset, size);
    if !f.call() {
        return false;
    }
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
    let f = unsafe { &mut *context.cast::<FakeJournalFlash>() };
    let (slot, offset) = bounds(slot, offset, size);
    assert!(offset % 4 == 0 && size.is_multiple_of(4));
    if !f.call() {
        return false;
    }
    if f.silent_program {
        return true;
    }
    let input = unsafe { core::slice::from_raw_parts(bytes.cast::<u8>(), size) };
    for (i, &value) in input.iter().enumerate() {
        assert_eq!(
            f.bytes[slot][offset + i] & value,
            value,
            "NOR programming only clears bits"
        );
        if !f.mutate(slot, offset + i, value) {
            return false;
        }
    }
    true
}

unsafe extern "C" fn erase(context: *mut c_void, slot: c_uint) -> bool {
    let f = unsafe { &mut *context.cast::<FakeJournalFlash>() };
    assert!(slot < 2);
    if !f.call() {
        return false;
    }
    if f.silent_erase {
        return true;
    }
    (0..UNIT).all(|i| f.mutate(slot as usize, i, u8::MAX))
}
