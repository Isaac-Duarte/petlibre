//! KM4 handshake and hooked flash-read probe against the fake SDK and a shared
//! IPC model. KM0 is modelled inline: it answers HELLO through the real
//! `Peer::step` and acknowledges/releases park requests on SYSTIMER reads, as
//! the parked core's RAM loop would. The fake hooks enforce the interposer's
//! acquire/release context checks around each read.

use std::cell::{Cell, RefCell};

use petlibro_amebad::flash_hooks::HookTicket;
use petlibro_amebad::flash_park::{
    self, ACK, Mmio, RELEASE, RELEASED, REQUEST, SYSTICK_CTRL, TICKS, TIMEOUT,
};
use petlibro_amebad::flash_peer::{
    BACKUP_BASE, MARKER_REGISTER, PARK_CHANNEL, PARK_TICK_LIMIT, Peer,
};
use petlibro_amebad::flash_probe::{
    ATTEMPT_MARKER, EXPECTED_ID, FlashOps, LeaseStage, Probe, Provider, READS, Record,
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
        // The ROM command runs with XIP unavailable: KM0 must be parked.
        assert!(km0(|m| m.parked_lease.is_some()));
        let id = READ_ID.with(Cell::get);
        if !p.release::<Core<false>>(&ticket) {
            std::panic::panic_any(HookFault);
        }
        assert_eq!(sdk::primask(), 1, "release keeps IRQs masked");
        sdk::set_systick_ctrl(ticket.saved_systick);
        sdk::set_primask(0);
        id
    }

    fn armed() -> bool {
        !PROVIDER.with(Cell::get).is_null()
    }
}

struct Boot {
    records: Vec<Record>,
    faulted: bool,
    backup: [u32; 8],
    rings: u32,
    provider: Provider,
}

/// One boot in a fresh thread (registration pointers are thread-local, and
/// a platform reset clears RAM); backup registers carry across.
fn platform_boot(backup: [u32; 8], deaf: bool, id: [u8; 3]) -> Boot {
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
        }
    })
    .join()
    .unwrap()
}

#[test]
fn hooked_reads_lease_park_and_release_every_time() {
    let boot = platform_boot([0; 8], false, EXPECTED_ID);
    let r = &boot.records;
    assert!(matches!(r[0], Record::Hello { count: 1, .. }));
    assert!(matches!(
        r[1],
        Record::Ready {
            reply: FlashReply::ACCEPTED,
            phase: FlashPhase::IDLE,
            fault: FlashFault::OK,
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
    let batches: Vec<_> = r
        .iter()
        .filter_map(|x| match x {
            Record::Batch {
                reads,
                id,
                mismatches,
                frozen,
                alive,
                park_max,
                release_max,
            } => Some((
                *reads,
                *id,
                *mismatches,
                *frozen,
                *alive,
                *park_max,
                *release_max,
            )),
            _ => None,
        })
        .collect();
    assert_eq!(batches.len(), 4, "{r:?}");
    let (reads, id, mismatches, frozen, alive, park, release) = batches[3];
    assert_eq!(
        (reads, id, mismatches, frozen),
        (READS, EXPECTED_ID, 0, u64::from(READS))
    );
    assert!(alive >= READS - 1 && park > 0 && release > 0);
    assert_eq!(
        r.last(),
        Some(&Record::Done {
            reads: READS,
            leases: u64::from(READS),
            phase: FlashPhase::IDLE,
            fault: FlashFault::OK
        })
    );
    assert!(!boot.faulted && boot.backup[MARKER_REGISTER as usize] == 0);
    assert_eq!(boot.rings, READS);
}

#[test]
fn wrong_id_is_counted_not_hidden() {
    let boot = platform_boot([0; 8], false, [0xef, 0x40, 0x17]);
    let Some(Record::Batch { mismatches, .. }) = boot
        .records
        .iter()
        .rev()
        .find(|x| matches!(x, Record::Batch { .. }))
    else {
        panic!("{:?}", boot.records);
    };
    assert_eq!(*mismatches, READS);
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
