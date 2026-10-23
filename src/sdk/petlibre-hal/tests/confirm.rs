//! FW11 confirmation write against the fake SDK storage model, with the
//! KM0 peer answering HELLO through the real `park::answer`.

use std::cell::{Cell, RefCell};

use petlibre_core::ota::{CONFIRM_LEN, confirm_record, confirmed};
use petlibre_hal::flash::park::{self, Mmio};
use petlibre_hal::flash::peer::MARKER_REGISTER;
use petlibre_hal::flash_lock::arming::{self, Hooks};
use petlibre_hal::flash_lock::provider::{ATTEMPT_BIT, Provider};
use petlibre_hal::ota::confirm::*;
use petlibre_hal::sdk::fake;
use petlibre_hal::stopped;
use petlibre_hal::trial::{CONFIRM_SECTOR, identity};

thread_local! {
    static REGS: RefCell<[[u32; 12]; 2]> = const { RefCell::new([[0; 12]; 2]) };
    static ARMED: Cell<bool> = const { Cell::new(false) };
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

/// USR words per core: index 0 KM0 (0x4800_6000), 1 KM4 (0x4000_6000).
fn locate(address: u32) -> (usize, usize) {
    let core = if address & 0xffff_f000 == 0x4800_6000 {
        0
    } else {
        1
    };
    (core, (((address & 0xfff) - 0x20) / 4) as usize)
}

struct FakeHooks;

impl Hooks for FakeHooks {
    fn watchdog() -> bool {
        true
    }
    fn install(provider: *mut Provider) -> bool {
        // SAFETY: the test's provider outlives the run.
        let ready = unsafe { (*provider).ready() };
        ARMED.with(|a| a.set(ready));
        ready
    }
    fn armed() -> bool {
        ARMED.with(Cell::get)
    }
}

fn start() {
    fake::reset_hardware();
    REGS.with(|r| *r.borrow_mut() = [[0; 12]; 2]);
    ARMED.with(|a| a.set(false));
    fake::with(|h| h.interrupts = 1);
    assert!(stopped::initialize().verified());
}

fn record() -> Vec<u8> {
    let at = CONFIRM_SECTOR as usize * 4096;
    fake::with(|h| h.storage[at..at + CONFIRM_LEN].to_vec())
}

/// Step with `healthy(ms)` deciding health, 500 ms apart.
fn run(confirm: &mut Confirm, healthy: impl Fn(u32) -> bool, steps: usize) -> Vec<Record> {
    let mut provider = Provider::default();
    let mut storage = HookedStorage::<FakeHooks>::new();
    let mut answered = 0;
    let mut now = 0;
    let mut out = Vec::new();

    for _ in 0..steps {
        park::answer::<Core<true>>(0x0000_0001_4b30_abcd, &mut answered);
        // SAFETY: `provider` outlives the run.
        let step = unsafe {
            confirm.step::<Core<false>, FakeHooks>(&mut provider, &mut storage, healthy(now), now)
        };
        out.extend(step.record);
        if step.delay_ms == 0 {
            break;
        }
        fake::advance(step.delay_ms * 32, false);
        now += step.delay_ms;
    }

    out
}

#[test]
fn confirms_after_continuous_health_with_the_attempt_marker_cleared() {
    start();
    let mut confirm = Confirm::default();
    let records = run(&mut confirm, |_| true, 200);

    let Some(Record::Confirmed { healthy_ms }) = records.last() else {
        panic!("{records:?}");
    };
    assert!(*healthy_ms >= HEALTHY_MS);
    assert!(records.contains(&Record::Arming(arming::Event::Armed {
        watchdog: true,
        installed: true
    })));
    assert_eq!(confirmed(&record(), &identity()), Some(0));
    assert_eq!(fake::with(|h| h.backup[MARKER_REGISTER as usize]), 0);
    assert_eq!(fake::with(|h| h.storage_erases[CONFIRM_SECTOR as usize]), 1);
}

#[test]
fn health_must_hold_for_the_whole_window() {
    start();
    let mut confirm = Confirm::default();
    // Health drops every 20 s: never 30 s in a row.
    let records = run(&mut confirm, |ms| ms % 20_000 >= 1_000, 400);

    assert!(records.is_empty(), "{records:?}");
    assert!(!FakeHooks::armed());
    assert_eq!(fake::with(|h| h.storage_erases[CONFIRM_SECTOR as usize]), 0);
}

#[test]
fn an_existing_record_for_this_build_writes_nothing() {
    start();
    let at = CONFIRM_SECTOR as usize * 4096;
    let existing = confirm_record(&identity(), 0);
    fake::with(|h| h.storage[at..at + CONFIRM_LEN].copy_from_slice(&existing));
    let mut confirm = Confirm::default();

    assert_eq!(run(&mut confirm, |_| true, 10), [Record::Already]);
    assert!(!FakeHooks::armed());
    assert_eq!(fake::with(|h| h.storage_erases[CONFIRM_SECTOR as usize]), 0);
}

#[test]
fn another_builds_record_is_replaced() {
    start();
    let at = CONFIRM_SECTOR as usize * 4096;
    let other = confirm_record(&[1; 32], 0);
    fake::with(|h| h.storage[at..at + CONFIRM_LEN].copy_from_slice(&other));
    let mut confirm = Confirm::default();
    let records = run(&mut confirm, |_| true, 200);

    assert!(matches!(records.last(), Some(Record::Confirmed { .. })));
    assert_eq!(confirmed(&record(), &identity()), Some(0));
}

#[test]
fn a_set_attempt_marker_skips() {
    start();
    fake::with(|h| h.backup[MARKER_REGISTER as usize] = ATTEMPT_BIT);
    let mut confirm = Confirm::default();
    let records = run(&mut confirm, |_| true, 200);

    assert_eq!(
        records.last(),
        Some(&Record::Arming(arming::Event::Skip {
            marker: ATTEMPT_BIT
        }))
    );
    assert_eq!(fake::with(|h| h.storage_erases[CONFIRM_SECTOR as usize]), 0);
}
