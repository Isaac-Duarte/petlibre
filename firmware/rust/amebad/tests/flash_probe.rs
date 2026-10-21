//! KM4 handshake and hooked scratch-sector probe against the fake SDK (with a
//! NOR model of the scratch sector) and a shared IPC model. KM0 is modelled
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
    ATTEMPT_MARKER, CYCLES, EXPECTED_ID, FlashOps, LeaseStage, Probe, Provider, Record,
};
use petlibro_amebad::sdk::{self, fake};
use petlibro_amebad::stopped;
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
    /// Bits that read 1 after programming (a stuck cell), applied per program.
    static STUCK: Cell<u32> = const { Cell::new(0) };
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

    fn erase_scratch() {
        ERASES.with(|e| e.set(e.get() + 1));
        leased(sdk::scratch_erase);
    }

    fn program_scratch(page: u32, data: &[u8; 256]) -> bool {
        let ok = leased(|| sdk::scratch_program(page, data));
        let stuck = STUCK.with(Cell::get);
        fake::with(|h| h.scratch[page as usize * 64] |= stuck);
        ok
    }

    fn read_scratch(word: u32) -> u32 {
        sdk::scratch_read(word)
    }
}

struct Boot {
    records: Vec<Record>,
    faulted: bool,
    backup: [u32; 8],
    rings: u32,
    provider: Provider,
    scratch: [u32; 1024],
    erases: u32,
}

/// One boot in a fresh thread (registration pointers are thread-local, and
/// a platform reset clears RAM); backup registers carry across.
fn platform_boot(backup: [u32; 8], deaf: bool, id: [u8; 3]) -> Boot {
    platform_boot_with(backup, deaf, id, |_| {}, 0)
}

fn platform_boot_with(
    backup: [u32; 8],
    deaf: bool,
    id: [u8; 3],
    prepare: fn(&mut fake::Hardware),
    stuck: u32,
) -> Boot {
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
        STUCK.with(|c| c.set(stuck));
        fake::with(prepare);
        let mut probe = Box::<Probe>::default();
        let mut provider = Box::<Provider>::default();
        let mut peer = Peer::default();
        let mut records = Vec::new();
        let mut faulted = false;
        let mut idle = 0;
        'boot: for _ in 0..100_000 {
            let raw: *mut Provider = &mut *provider;
            let step = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
                probe.step::<Core<false>, Hooks>(raw)
            }));
            let Ok(step) = step else {
                faulted = true;
                break;
            };
            if matches!(
                records.last(),
                Some(
                    Record::Done { .. }
                        | Record::Start {
                            installed: false,
                            ..
                        }
                )
            ) {
                idle += 1;
            }
            records.extend(step.record);
            if step.delay_ms == 0 || idle > 20 {
                break;
            }
            for _ in 0..step.delay_ms {
                fake::advance(33, false);
                react();
                if fake::with(|h| h.watchdog_expired) {
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
            scratch: fake::with(|h| h.scratch),
            erases: ERASES.with(Cell::get),
        }
    })
    .join()
    .unwrap()
}

fn cycles(records: &[Record]) -> Vec<Record> {
    records
        .iter()
        .copied()
        .filter(|r| matches!(r, Record::Cycle { .. }))
        .collect()
}

#[test]
fn scratch_cycles_verify_and_restore_blank_through_leases() {
    let boot = platform_boot([0; 8], false, EXPECTED_ID);
    let r = &boot.records;
    assert!(matches!(r[0], Record::Hello { count: 1, .. }));
    assert!(matches!(
        r[1],
        Record::Ready {
            reply: FlashReply::ACCEPTED,
            phase: FlashPhase::IDLE,
            inactive: true,
            ..
        }
    ));
    assert_eq!(
        r[2],
        Record::Start {
            marker: 0,
            watchdog: true,
            installed: true
        }
    );
    assert_eq!(
        r[3],
        Record::Preimage {
            id: EXPECTED_ID,
            blank: 1024
        }
    );
    let c = cycles(r);
    assert_eq!(c.len() as u32, CYCLES, "{r:?}");
    for (i, record) in c.iter().enumerate() {
        let Record::Cycle {
            cycle,
            blank,
            errors,
            ..
        } = *record
        else {
            unreachable!()
        };
        assert_eq!((cycle, blank, errors), (i as u32 + 1, 1024, 0));
    }
    let restore = r
        .iter()
        .position(|x| matches!(x, Record::Restore { .. }))
        .unwrap();
    assert!(matches!(r[restore], Record::Restore { blank: 1024, .. }));
    // One ID read, then per cycle one erase and 16 programs, then the restore.
    let leases = 1 + u64::from(CYCLES) * 17 + 1;
    let Record::Done {
        cycles,
        leases: done_leases,
        phase,
        fault,
        frozen,
        ..
    } = r[restore + 1]
    else {
        panic!("{r:?}");
    };
    assert_eq!(
        (cycles, done_leases, phase, fault, frozen),
        (CYCLES, leases, FlashPhase::IDLE, FlashFault::OK, leases)
    );
    assert!(boot.scratch.iter().all(|&w| w == u32::MAX));
    assert!(!boot.faulted && boot.backup[MARKER_REGISTER as usize] == 0);
    assert_eq!(u64::from(boot.rings), leases);
    assert_eq!(boot.erases, CYCLES + 1);
}

#[test]
fn non_blank_preimage_or_wrong_part_writes_nothing() {
    let boot = platform_boot_with([0; 8], false, EXPECTED_ID, |h| h.scratch[5] = 0x1234, 0);
    assert_eq!(
        boot.records.last(),
        Some(&Record::Preimage {
            id: EXPECTED_ID,
            blank: 1023
        })
    );
    assert_eq!((boot.erases, boot.scratch[5]), (0, 0x1234));
    assert_eq!(boot.backup[MARKER_REGISTER as usize], 0);
    let boot = platform_boot([0; 8], false, [0xef, 0x40, 0x18]);
    assert!(matches!(boot.records.last(), Some(Record::Preimage { .. })));
    assert_eq!(boot.erases, 0);
}

#[test]
fn verify_mismatch_stops_all_writing_and_keeps_feeding() {
    let boot = platform_boot_with([0; 8], false, EXPECTED_ID, |_| {}, 1 << 31);
    let c = cycles(&boot.records);
    assert_eq!(c.len(), 1, "{:?}", boot.records);
    assert!(matches!(c[0], Record::Cycle { cycle: 1, errors, .. } if errors > 0));
    // No restore erase: the evidence stays; the marker is cleared and the
    // watchdog stays fed (no reset).
    assert_eq!(boot.erases, 1);
    assert!(
        !boot
            .records
            .iter()
            .any(|x| matches!(x, Record::Restore { .. }))
    );
    assert!(!boot.faulted && boot.backup[MARKER_REGISTER as usize] == 0);
}

#[test]
fn silent_peer_faults_the_hook_then_the_next_boot_skips() {
    let first = platform_boot([0; 8], true, EXPECTED_ID);
    assert!(first.faulted, "{:?}", first.records);
    assert_eq!(
        first.provider.failure,
        Some((LeaseStage::BEGIN, FlashReply::FAILED))
    );
    assert_eq!(first.backup[MARKER_REGISTER as usize], ATTEMPT_MARKER);
    let second = platform_boot(first.backup, false, EXPECTED_ID);
    assert_eq!(
        second.records.last(),
        Some(&Record::Skip {
            marker: ATTEMPT_MARKER
        })
    );
    assert_eq!(second.backup[MARKER_REGISTER as usize], 0);
}

#[test]
fn stale_ticket_is_refused() {
    let boot = platform_boot([0; 8], false, EXPECTED_ID);
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
