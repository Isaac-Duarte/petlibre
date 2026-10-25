//! Confirmation write against the fake SDK storage model, with the
//! KM0 peer answering HELLO through the real `park::answer`, and the
//! update stage's floor raised before the record.

use std::cell::{Cell, RefCell};
use std::sync::{Mutex, MutexGuard};

use wellfed_core::ota::offer::ImageKind;
use wellfed_core::ota::{CONFIRM_LEN, confirm_record, confirmed};
use wellfed_core::store::update::{Candidate, State, UpdateStore};
use wellfed_hal::flash::park::{self, Mmio};
use wellfed_hal::flash::peer::MARKER_REGISTER;
use wellfed_hal::flash_lock::arming::{self, Hooks};
use wellfed_hal::flash_lock::provider::{ATTEMPT_BIT, Provider};
use wellfed_hal::interlock::INTERLOCK;
use wellfed_hal::ota::confirm::*;
use wellfed_hal::ota::update::{Floor, StoreSlots, Update};
use wellfed_hal::sdk::fake;
use wellfed_hal::stopped;
use wellfed_hal::trial::{CONFIRM_SECTOR, identity};

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

/// The interlock is one static: tests that claim it run one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

fn start() -> MutexGuard<'static, ()> {
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    INTERLOCK.reset();
    fake::reset_hardware();
    REGS.with(|r| *r.borrow_mut() = [[0; 12]; 2]);
    ARMED.with(|a| a.set(false));
    fake::with(|h| h.interrupts = 1);
    assert!(stopped::initialize().verified());
    guard
}

fn record() -> Vec<u8> {
    let at = CONFIRM_SECTOR as usize * 4096;
    fake::with(|h| h.storage[at..at + CONFIRM_LEN].to_vec())
}

/// Step with `healthy(ms)` deciding health, 500 ms apart.
fn run(confirm: &mut Confirm, healthy: impl Fn(u32) -> bool, steps: usize) -> Vec<Record> {
    run_with(
        confirm,
        &mut HookedStorage::<FakeHooks>::new(),
        &mut || Floor::Unchanged,
        healthy,
        steps,
    )
}

fn run_with(
    confirm: &mut Confirm,
    storage: &mut impl StorageFlash,
    raise: &mut impl FnMut() -> Floor,
    healthy: impl Fn(u32) -> bool,
    steps: usize,
) -> Vec<Record> {
    let mut provider = Provider::default();
    let mut answered = 0;
    let mut now = 0;
    let mut out = Vec::new();

    for _ in 0..steps {
        park::answer::<Core<true>>(0x0000_0001_4b30_abcd, &mut answered);
        // SAFETY: `provider` outlives the run.
        let step = unsafe {
            confirm.step::<Core<false>, FakeHooks>(
                &mut provider,
                storage,
                &mut *raise,
                healthy(now),
                now,
            )
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
    let _guard = start();
    let mut confirm = Confirm::default();
    let records = run(&mut confirm, |_| true, 200);

    let Some(Record::Confirmed { healthy_ms, .. }) = records.last() else {
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
    let _guard = start();
    let mut confirm = Confirm::default();
    // Health drops every 20 s: never 30 s in a row.
    let records = run(&mut confirm, |ms| ms % 20_000 >= 1_000, 400);

    assert!(records.is_empty(), "{records:?}");
    assert!(!FakeHooks::armed());
    assert_eq!(fake::with(|h| h.storage_erases[CONFIRM_SECTOR as usize]), 0);
}

#[test]
fn an_existing_record_for_this_build_writes_nothing() {
    let _guard = start();
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
    let _guard = start();
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
    let _guard = start();
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

#[test]
fn arming_and_the_write_wait_for_a_feed_to_release_the_outputs() {
    let _guard = start();
    INTERLOCK.claim_feed(u32::MAX).unwrap();
    let mut confirm = Confirm::default();

    // Healthy, but a feed holds the outputs: not even arming starts, so the
    // attempt marker is never left set through the wait.
    let records = run(&mut confirm, |_| true, 200);
    assert!(records.is_empty(), "{records:?}");
    assert!(!FakeHooks::armed());
    assert_eq!(fake::with(|h| h.backup[MARKER_REGISTER as usize]), 0);
    assert_eq!(fake::with(|h| h.storage_erases[CONFIRM_SECTOR as usize]), 0);

    INTERLOCK.release_feed();
    let records = run(&mut confirm, |_| true, 10);
    assert!(matches!(records.last(), Some(Record::Confirmed { .. })));
    assert_eq!(fake::with(|h| h.backup[MARKER_REGISTER as usize]), 0);
    assert_eq!(confirmed(&record(), &identity()), Some(0));
    assert!(!INTERLOCK.writing(), "the turn is given back");
    assert_eq!(INTERLOCK.status().writes_deferred, 1);
}

type Slots = StoreSlots<FakeHooks>;

fn update_store() -> UpdateStore<Slots> {
    let mut store = UpdateStore::default();
    store.open(Slots::new());
    store
}

/// Storage that checks the floor whenever the confirmation sector is
/// erased, and can fail a write there.
struct Watched {
    inner: HookedStorage<FakeHooks>,
    floor_at_erase: Option<u32>,
    fail_program: bool,
}

impl StorageFlash for Watched {
    fn erase(&mut self, sector: u32) -> bool {
        if sector == CONFIRM_SECTOR {
            self.floor_at_erase = Some(update_store().record().min_version);
        }
        self.inner.erase(sector)
    }
    fn program(&mut self, sector: u32, offset: u32, data: &[u8]) -> bool {
        !(self.fail_program && sector == CONFIRM_SECTOR) && self.inner.program(sector, offset, data)
    }
    fn read(&mut self, sector: u32, offset: u32, bytes: &mut [u8]) -> bool {
        self.inner.read(sector, offset, bytes)
    }
}

fn watched() -> Watched {
    Watched {
        inner: HookedStorage::new(),
        floor_at_erase: None,
        fail_program: false,
    }
}

/// Bank 0 activated this build as version 12.
fn activated() {
    ARMED.with(|a| a.set(true));
    update_store()
        .set_activated(Candidate {
            digest: [3; 32],
            version: 12,
            build_id: identity(),
            kind: ImageKind::Replacement,
        })
        .unwrap();
    ARMED.with(|a| a.set(false));
}

#[test]
fn the_floor_rises_before_the_confirmation_sector_is_erased() {
    let _guard = start();
    activated();
    let mut stage = Update::new(1, identity(), None, Slots::new());
    let mut storage = watched();
    let mut confirm = Confirm::default();
    let records = run_with(
        &mut confirm,
        &mut storage,
        &mut || stage.raise_floor(),
        |_| true,
        200,
    );

    assert!(matches!(
        records.last(),
        Some(Record::Confirmed { version: 12, .. })
    ));
    assert_eq!(storage.floor_at_erase, Some(12), "the floor first");
    assert_eq!(confirmed(&record(), &identity()), Some(12));
    assert_eq!(update_store().record().state, State::None);
}

#[test]
fn a_floor_that_cannot_rise_leaves_the_build_unconfirmed() {
    let _guard = start();
    let mut confirm = Confirm::default();
    let records = run_with(
        &mut confirm,
        &mut watched(),
        &mut || Floor::Failed,
        |_| true,
        200,
    );
    assert_eq!(
        records.last(),
        Some(&Record::Error {
            stage: ErrorStage::Floor
        })
    );
    assert_eq!(fake::with(|h| h.storage_erases[CONFIRM_SECTOR as usize]), 0);
    assert_eq!(confirmed(&record(), &identity()), None);
    assert_eq!(fake::with(|h| h.backup[MARKER_REGISTER as usize]), 0);
}

#[test]
fn a_torn_record_after_the_floor_rose_confirms_on_the_next_boot() {
    let _guard = start();
    activated();
    let mut stage = Update::new(1, identity(), None, Slots::new());
    let mut storage = watched();
    storage.fail_program = true;
    let mut confirm = Confirm::default();
    let records = run_with(
        &mut confirm,
        &mut storage,
        &mut || stage.raise_floor(),
        |_| true,
        200,
    );
    assert_eq!(
        records.last(),
        Some(&Record::Error {
            stage: ErrorStage::Write
        })
    );
    assert_eq!(update_store().record().min_version, 12, "kept");
    assert_eq!(
        confirmed(&record(), &identity()),
        None,
        "KM0 keeps counting"
    );

    // The next boot: nothing names the build any more, the floor stays.
    REGS.with(|r| *r.borrow_mut() = [[0; 12]; 2]);
    ARMED.with(|a| a.set(false));
    let mut stage = Update::new(1, identity(), None, Slots::new());
    let mut confirm = Confirm::default();
    let records = run_with(
        &mut confirm,
        &mut watched(),
        &mut || stage.raise_floor(),
        |_| true,
        200,
    );
    assert!(matches!(
        records.last(),
        Some(Record::Confirmed { version: 0, .. })
    ));
    assert_eq!(confirmed(&record(), &identity()), Some(0));
    assert_eq!(update_store().record().min_version, 12);
}
