//! KM4 handshake and FW15 journal probe against the fake SDK (with a NOR
//! model of the storage region) and a shared IPC model. KM0 is modelled
//! inline: it answers HELLO through the real `Peer::step` and acknowledges or
//! releases park requests on SYSTIMER reads, as the parked core's RAM loop
//! would. The fake hooks enforce the interposer's acquire/release context
//! checks around every mediated operation.

use std::cell::{Cell, RefCell};

use petlibro_amebad::flash_hooks::HookTicket;
use petlibro_amebad::flash_park::{
    self, ACK, Mmio, RELEASE, RELEASED, REQUEST, SYSTICK_CTRL, TICKS, TIMEOUT,
};
use petlibro_amebad::flash_peer::{
    BACKUP_BASE, MARKER_REGISTER, PARK_CHANNEL, PARK_TICK_LIMIT, Peer,
};
use petlibro_amebad::flash_probe::{
    ATTEMPT_BIT, EXPECTED_ID, FlashOps, JournalOp, LeaseStage, Probe, Provider, Record,
};
use petlibro_amebad::sdk::{self, fake};
use petlibro_amebad::stopped;
use petlibro_core::feed_journal::JournalError;
use petlibro_core::feed_storage::OwnerOutcome;
use petlibro_core::flash_coord::{FlashFault, FlashPhase, FlashReply};

#[derive(Default)]
struct Km0 {
    regs: [[u32; 12]; 2],
    rings: u32,
    parked_lease: Option<u32>,
    parked_at: u32,
    halted: bool,
    /// Never acknowledge (an IPC interrupt that never arrives).
    deaf: bool,
}

thread_local! {
    static KM0: RefCell<Km0> = RefCell::new(Km0::default());
    static PROVIDER: Cell<*mut Provider> = const { Cell::new(std::ptr::null_mut()) };
    static READ_ID: Cell<[u8; 3]> = const { Cell::new(EXPECTED_ID) };
    static ERASES: Cell<u32> = const { Cell::new(0) };
}

fn km0<R>(f: impl FnOnce(&mut Km0) -> R) -> R {
    KM0.with(|m| f(&mut m.borrow_mut()))
}

/// The parked KM0's behaviour, run whenever KM4 samples the shared counter.
fn react() {
    km0(|m| {
        if m.halted {
            return;
        }
        let km4 = m.regs[1];
        match m.parked_lease {
            None if km4[10] == REQUEST && !m.deaf => {
                let lease = km4[6];
                if m.regs[0][6] != lease || m.regs[0][10] == RELEASED {
                    for i in [2, 3, 4, 5, 6, 9] {
                        m.regs[0][i] = km4[i];
                    }
                    m.regs[0][10] = ACK;
                    m.parked_lease = Some(lease);
                    m.parked_at = sdk::systimer_ticks();
                }
            }
            Some(lease) => {
                if km4[10] == RELEASE && km4[6] == lease {
                    m.regs[0][10] = RELEASED;
                    m.parked_lease = None;
                } else if sdk::systimer_ticks().wrapping_sub(m.parked_at) >= PARK_TICK_LIMIT {
                    m.regs[0][10] = TIMEOUT;
                    m.halted = true;
                }
            }
            None => {}
        }
    });
}

struct Core<const KM0: bool>;

impl<const KM0: bool> Mmio for Core<KM0> {
    const KM0: bool = KM0;
    fn load(address: u32) -> u32 {
        if address == TICKS {
            react();
            fake::advance(1, false);
            return sdk::systimer_ticks();
        }
        if address == SYSTICK_CTRL {
            return sdk::systick_ctrl();
        }
        if (BACKUP_BASE..BACKUP_BASE + 32).contains(&address) {
            return fake::with(|h| h.backup[((address - BACKUP_BASE) / 4) as usize]);
        }
        let (core, index) = locate(address);
        km0(|m| m.regs[core][index])
    }
    fn store(address: u32, value: u32) {
        if (BACKUP_BASE..BACKUP_BASE + 32).contains(&address) {
            fake::with(|h| h.backup[((address - BACKUP_BASE) / 4) as usize] = value);
            return;
        }
        if address == 0x4000_6008 {
            assert_eq!(value, 1 << PARK_CHANNEL);
            km0(|m| m.rings += 1);
            return;
        }
        let (core, index) = locate(address);
        km0(|m| m.regs[core][index] = value);
    }
    fn barrier() {}
    fn mask() -> u32 {
        sdk::primask()
    }
    fn halt() -> ! {
        panic!("halt")
    }
}

fn locate(address: u32) -> (usize, usize) {
    let core = match address & 0xffff_f000 {
        0x4800_6000 => 0,
        0x4000_6000 => 1,
        _ => panic!("unexpected address {address:#x}"),
    };
    (core, (((address & 0xfff) - 0x20) / 4) as usize)
}

/// A hook fault: the target halts in RAM; the model stops the boot here.
struct HookFault;

/// The interposer's contract around one SDK lock/unlock pair.
struct Hooks;

fn leased<R>(op: impl FnOnce() -> R) -> R {
    let provider = PROVIDER.with(Cell::get);
    assert!(!provider.is_null());
    let mut ticket = HookTicket::default();
    // SAFETY: the provider is live; the probe does not hold it across this.
    let p = unsafe { &mut *provider };
    if !p.acquire::<Core<false>>(&mut ticket) {
        std::panic::panic_any(HookFault);
    }
    assert_eq!(sdk::primask(), 1, "acquire returns with IRQs masked");
    assert_eq!(sdk::systick_ctrl() & flash_park::SYSTICK_CONTROL_BITS, 0);
    assert!(ticket.id != 0 && ticket.saved_systick & !7 == 0);
    // The operation runs with XIP unavailable: KM0 must be parked.
    assert!(km0(|m| m.parked_lease.is_some()));
    let result = op();
    if !p.release::<Core<false>>(&ticket) {
        std::panic::panic_any(HookFault);
    }
    assert_eq!(sdk::primask(), 1, "release keeps IRQs masked");
    sdk::set_systick_ctrl(ticket.saved_systick);
    sdk::set_primask(0);
    result
}

impl FlashOps for Hooks {
    fn install(provider: *mut Provider) -> bool {
        // SAFETY: the test's provider outlives the boot.
        let ready = unsafe { (*provider).ready() };
        if ready {
            PROVIDER.with(|p| p.set(provider));
        }
        ready && sdk::primask() == 0
    }

    fn read_id() -> [u8; 3] {
        leased(|| READ_ID.with(Cell::get))
    }

    fn armed() -> bool {
        !PROVIDER.with(Cell::get).is_null()
    }

    fn erase(sector: u32) -> bool {
        ERASES.with(|e| e.set(e.get() + 1));
        leased(|| sdk::storage_erase(sector))
    }

    fn program(sector: u32, offset: u32, data: &[u8]) -> bool {
        leased(|| sdk::storage_program(sector, offset, data))
    }

    fn read(sector: u32, offset: u32, bytes: &mut [u8]) -> bool {
        sdk::storage_read(sector, offset, bytes)
    }
}

struct Boot {
    records: Vec<Record>,
    faulted: bool,
    backup: [u32; 8],
    rings: u32,
    provider: Provider,
    storage: Vec<u8>,
    erases: u32,
    reset: bool,
}

/// One boot in a fresh thread (registration pointers are thread-local, and
/// a platform reset clears RAM); backup registers carry across.
fn platform_boot(backup: [u32; 8], storage: Vec<u8>, deaf: bool, id: [u8; 3]) -> Boot {
    std::thread::spawn(move || {
        fake::reset_hardware();
        fake::with(|h| {
            h.backup = backup;
            h.interrupts = 1;
        });
        assert!(stopped::initialize().verified());
        fake::with(|h| h.interrupts = 0);
        km0(|m| {
            *m = Km0 {
                deaf,
                ..Km0::default()
            }
        });
        READ_ID.with(|c| c.set(id));
        fake::with(|h| h.storage = storage);
        let mut probe = Box::<Probe>::default();
        let mut provider = Box::<Provider>::default();
        let mut peer = Peer::default();
        let mut records = Vec::new();
        let mut faulted = false;
        let mut reset = false;
        // Stop after 40 quiet iterations: idle (fed) or holding for a reset.
        let mut quiet = 0;
        'boot: for _ in 0..100_000 {
            let raw: *mut Provider = &mut *provider;
            let step = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
                probe.step::<Core<false>, Hooks>(raw)
            }));
            let Ok(step) = step else {
                faulted = true;
                break;
            };
            quiet = if step.record.is_some() { 0 } else { quiet + 1 };
            records.extend(step.record);
            if step.delay_ms == 0 || quiet > 40 {
                break;
            }
            for _ in 0..step.delay_ms {
                fake::advance(33, false);
                react();
                if fake::with(|h| h.watchdog_expired) {
                    reset = true;
                    break 'boot;
                }
                if !km0(|m| m.halted || m.parked_lease.is_some()) {
                    peer.step::<Core<true>>(1, sdk::systimer_ticks());
                }
            }
        }
        Boot {
            records,
            faulted,
            backup: fake::with(|h| h.backup),
            rings: km0(|m| m.rings),
            provider: std::mem::take(&mut *provider),
            storage: fake::with(|h| h.storage.clone()),
            erases: ERASES.with(Cell::get),
            reset,
        }
    })
    .join()
    .unwrap()
}

fn blank() -> Vec<u8> {
    vec![u8::MAX; 16 * 4096]
}

fn op(kind: JournalOp, id: u64) -> Record {
    Record::Op {
        op: kind,
        id,
        ok: true,
        error: JournalError::OK,
        ticks: 0,
    }
}

/// Records with timing zeroed, for exact comparison.
fn shape(records: &[Record]) -> Vec<Record> {
    records
        .iter()
        .map(|r| match *r {
            Record::Op {
                op, id, ok, error, ..
            } => Record::Op {
                op,
                id,
                ok,
                error,
                ticks: 0,
            },
            Record::End {
                mode,
                reset,
                erases,
                phase,
                fault,
                ..
            } => Record::End {
                mode,
                reset,
                leases: 0,
                frozen: 0,
                erases,
                phase,
                fault,
            },
            other => other,
        })
        .collect()
}

fn view(sequence: u64, pending: bool, result: u64, outcome: OwnerOutcome, slot: u32) -> Record {
    Record::View {
        sequence,
        high_water: 3,
        pending,
        result_id: result,
        outcome,
        slot,
    }
}

fn end(mode: u32, reset: bool, erases: u32) -> Record {
    Record::End {
        mode,
        reset,
        leases: 0,
        frozen: 0,
        erases,
        phase: FlashPhase::IDLE,
        fault: FlashFault::OK,
    }
}

#[test]
fn journal_survives_resets_blocks_pending_and_ignores_a_torn_record() {
    let first = platform_boot([0; 8], blank(), false, EXPECTED_ID);
    let r = shape(&first.records);
    assert!(matches!(r[0], Record::Hello { count: 1, .. }));
    assert_eq!(
        r[2..],
        [
            Record::Start {
                mode: 0,
                marker: 0,
                watchdog: true,
                installed: true
            },
            Record::Preimage {
                id: EXPECTED_ID,
                blank_sectors: 16
            },
            op(JournalOp::CREATE, 0),
            op(JournalOp::RESERVE, 1),
            op(JournalOp::FINISH, 1),
            op(JournalOp::RESERVE, 2),
            op(JournalOp::FINISH, 2),
            op(JournalOp::RESERVE, 3),
            view(6, true, 3, OwnerOutcome::PENDING, 1),
            end(0, true, 5),
        ]
    );
    assert!(first.reset && first.backup[MARKER_REGISTER as usize] == 1);

    let second = platform_boot(first.backup, first.storage, false, EXPECTED_ID);
    let r = shape(&second.records);
    assert_eq!(
        r[2..],
        [
            Record::Start {
                mode: 1,
                marker: 1,
                watchdog: true,
                installed: true
            },
            op(JournalOp::OPEN, 0),
            view(6, true, 3, OwnerOutcome::PENDING, 1),
            Record::Refuse {
                id: 4,
                reserved: false
            },
            op(JournalOp::TORN, 0),
            end(1, true, 1),
        ],
        "{:?}",
        second.records
    );
    assert!(second.reset && second.backup[MARKER_REGISTER as usize] == 2);

    let third = platform_boot(second.backup, second.storage, false, EXPECTED_ID);
    let r = shape(&third.records);
    assert_eq!(
        r[2..],
        [
            Record::Start {
                mode: 2,
                marker: 2,
                watchdog: true,
                installed: true
            },
            op(JournalOp::OPEN, 0),
            view(6, true, 3, OwnerOutcome::PENDING, 1),
            Record::Refuse {
                id: 4,
                reserved: false
            },
            op(JournalOp::WIPE, 0),
            end(2, false, 2),
        ],
        "{:?}",
        third.records
    );
    assert!(!third.reset && third.backup[MARKER_REGISTER as usize] == 0);
    assert!(third.storage.iter().all(|&b| b == u8::MAX));
    let Some(Record::End { leases, frozen, .. }) = third.records.last() else {
        unreachable!()
    };
    assert!(*leases > 0 && leases == frozen);
    // Every park request KM0 saw is one recorded lease.
    assert_eq!(u64::from(third.rings), *leases);
}

#[test]
fn non_blank_region_or_wrong_part_writes_nothing() {
    let mut storage = blank();
    storage[0x5000] = 0;
    let boot = platform_boot([0; 8], storage.clone(), false, EXPECTED_ID);
    assert_eq!(
        boot.records.last(),
        Some(&Record::Preimage {
            id: EXPECTED_ID,
            blank_sectors: 15
        })
    );
    assert!(boot.erases == 0 && boot.storage == storage && !boot.reset);
    assert_eq!(boot.backup[MARKER_REGISTER as usize], 0);
    let boot = platform_boot([0; 8], blank(), false, [0xef, 0x40, 0x18]);
    assert!(matches!(boot.records.last(), Some(Record::Preimage { .. })));
    assert_eq!(boot.erases, 0);
}

#[test]
fn unknown_marker_skips() {
    let boot = platform_boot([0, 7, 0, 0, 0, 0, 0, 0], blank(), false, EXPECTED_ID);
    assert_eq!(boot.records.last(), Some(&Record::Skip { marker: 7 }));
    assert_eq!(boot.backup[MARKER_REGISTER as usize], 0);
}

#[test]
fn silent_peer_faults_the_hook_then_the_next_boot_skips() {
    let first = platform_boot([0; 8], blank(), true, EXPECTED_ID);
    assert!(first.faulted, "{:?}", first.records);
    assert_eq!(
        first.provider.failure,
        Some((LeaseStage::BEGIN, FlashReply::FAILED))
    );
    assert_eq!(first.backup[MARKER_REGISTER as usize], ATTEMPT_BIT);
    let second = platform_boot(first.backup, first.storage, false, EXPECTED_ID);
    assert_eq!(
        second.records.last(),
        Some(&Record::Skip {
            marker: ATTEMPT_BIT
        })
    );
    assert_eq!(second.backup[MARKER_REGISTER as usize], 0);
}

#[test]
fn stale_ticket_is_refused() {
    let boot = platform_boot([0; 8], blank(), false, EXPECTED_ID);
    let mut provider = boot.provider;
    let ticket = HookTicket {
        id: provider.coord.token.lease + 7,
        saved_systick: 0,
    };
    assert!(!provider.release::<Core<false>>(&ticket));
    assert_eq!(
        provider.failure,
        Some((LeaseStage::TICKET, FlashReply::STALE))
    );
}
