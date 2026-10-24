//! KM4 handshake and settings stress probe against the fake SDK (with a
//! NOR model of the storage region) and a shared IPC model. Power cuts are
//! injected at a chosen mutating flash call: that call takes half effect and
//! the boot ends with backup registers cleared, as unplugging would. KM0 is modelled
//! inline: it answers HELLO through the real `Peer::step` and acknowledges or
//! releases park requests on SYSTIMER reads, as the parked core's RAM loop
//! would. The fake hooks enforce the interposer's acquire/release context
//! checks around every mediated operation.

use std::cell::{Cell, RefCell};

use wellfed_core::flash_coord::{FlashFault, FlashPhase, FlashReply};
use wellfed_core::store::settings::{SettingsStatus, SlotState};
use wellfed_hal::flash::hooks::HookTicket;
use wellfed_hal::flash::park::{
    self, ACK, Mmio, RELEASE, RELEASED, REQUEST, SYSTICK_CTRL, TICKS, TIMEOUT,
};
use wellfed_hal::flash::peer::{
    BACKUP_BASE, MARKER_REGISTER, PARK_CHANNEL, PARK_TICK_LIMIT, Peer,
};
use wellfed_hal::flash_lock::provider::{ATTEMPT_BIT, LeaseStage, Provider};
use wellfed_hal::sdk::{self, fake};
use wellfed_hal::stopped;
use wellfed_probes::flash::{EXPECTED_ID, FlashOps, Probe, Record, STRESS_SAVES};

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
    /// Mutating calls left before a power cut; `None` never cuts.
    static CUT: Cell<Option<u32>> = const { Cell::new(None) };
    /// Power is gone: flash calls have no effect and fail.
    static POWER_LOST: Cell<bool> = const { Cell::new(false) };
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

/// True when this mutating call is the one the power cut interrupts; it then
/// marks power lost. Callbacks are `extern "C"`, so the cut cannot unwind.
fn cut_now() -> bool {
    let hit = CUT.with(|c| match c.get() {
        Some(0) => true,
        Some(n) => {
            c.set(Some(n - 1));
            false
        }
        None => false,
    });

    if hit {
        POWER_LOST.with(|p| p.set(true));
        CUT.with(|c| c.set(None));
    }
    hit
}

fn power_lost() -> bool {
    POWER_LOST.with(Cell::get)
}

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
    assert_eq!(sdk::systick_ctrl() & park::SYSTICK_CONTROL_BITS, 0);
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
        if power_lost() {
            return false;
        }

        ERASES.with(|e| e.set(e.get() + 1));
        if cut_now() {
            // An interrupted erase: half the sector raised to ones.
            fake::with(|h| {
                let at = sector as usize * 4096;
                h.storage[at..at + 2048].fill(u8::MAX);
            });
            return false;
        }
        leased(|| sdk::storage_erase(sector))
    }

    fn program(sector: u32, offset: u32, data: &[u8]) -> bool {
        if power_lost() {
            return false;
        }

        if cut_now() {
            // An interrupted program: the first half of the bytes landed.
            let half = data.len() / 2 / 4 * 4;
            let _ = sdk::storage_program(sector, offset, &data[..half]);
            return false;
        }
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
    power_cut: bool,
}

/// One boot in a fresh thread (registration pointers are thread-local, and
/// a platform reset clears RAM); backup registers carry across.
fn platform_boot(backup: [u32; 8], storage: Vec<u8>, deaf: bool, id: [u8; 3]) -> Boot {
    platform_boot_cut(backup, storage, deaf, id, None)
}

fn platform_boot_cut(
    backup: [u32; 8],
    storage: Vec<u8>,
    deaf: bool,
    id: [u8; 3],
    cut: Option<u32>,
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
        fake::with(|h| h.storage = storage);
        CUT.with(|c| c.set(cut));
        POWER_LOST.with(|p| p.set(false));

        let mut probe = Box::<Probe<Hooks>>::default();
        let mut provider = Box::<Provider>::default();
        let mut peer = Peer::default();
        let mut records = Vec::new();
        let mut faulted = false;
        let mut power_cut = false;
        let mut reset = false;
        // Stop after 40 quiet iterations: idle (fed) or holding for a reset.
        let mut quiet = 0;

        'boot: for _ in 0..100_000 {
            let raw: *mut Provider = &mut *provider;
            // SAFETY: the probe's pointers are the fixture's live locals.
            let step = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
                probe.step::<Core<false>>(raw)
            }));
            let Ok(step) = step else {
                faulted = true;
                break;
            };

            if power_lost() {
                // Records printed after the cut never left the board.
                power_cut = true;
                break;
            }

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
            power_cut,
        }
    })
    .join()
    .unwrap()
}

fn blank() -> Vec<u8> {
    vec![u8::MAX; 16 * 4096]
}

fn opened(boot: &Boot) -> (SettingsStatus, u64, bool) {
    let (status, generation, _, consistent) = opened_other(boot);
    (status, generation, consistent)
}

fn opened_other(boot: &Boot) -> (SettingsStatus, u64, SlotState, bool) {
    let Some(Record::Open {
        status,
        generation,
        other,
        consistent,
        ..
    }) = boot
        .records
        .iter()
        .find(|r| matches!(r, Record::Open { .. }))
        .copied()
    else {
        panic!("no open: {:?}", boot.records);
    };
    (status, generation, other, consistent)
}

/// The generation of the last save the probe reported complete.
fn last_reported(boot: &Boot) -> u64 {
    boot.records
        .iter()
        .filter_map(|r| match r {
            Record::Batch { generation, .. } | Record::End { generation, .. } => Some(*generation),
            Record::Open { generation, .. } => Some(*generation),
            _ => None,
        })
        .max()
        .unwrap()
}

#[test]
fn stress_saves_survive_power_loss_and_reopen_consistently() {
    let first = platform_boot([0; 8], blank(), false, EXPECTED_ID);
    assert_eq!(opened(&first), (SettingsStatus::Empty, 0, true));
    let Some(Record::End {
        saves,
        generation,
        leases,
        frozen,
        phase,
        fault,
    }) = first.records.last().copied()
    else {
        panic!("{:?}", first.records);
    };

    assert_eq!((saves, generation), (STRESS_SAVES, u64::from(STRESS_SAVES)));
    assert!(leases > 0 && leases == frozen && phase == FlashPhase::Idle && fault == FlashFault::Ok);
    // Every park request KM0 saw is one recorded lease.
    assert_eq!(u64::from(first.rings), leases);
    assert!(!first.reset && first.backup[MARKER_REGISTER as usize] == 0);

    // Only sectors 2 and 3 were touched.
    assert!(first.storage[..2 * 4096].iter().all(|&b| b == u8::MAX));
    assert!(first.storage[4 * 4096..].iter().all(|&b| b == u8::MAX));

    // Power loss clears the backup registers; the settings remain.
    let second = platform_boot([0; 8], first.storage, false, EXPECTED_ID);
    assert_eq!(
        opened(&second),
        (SettingsStatus::Loaded, u64::from(STRESS_SAVES), true)
    );

    assert_eq!(last_reported(&second), u64::from(2 * STRESS_SAVES));
}

/// Cut power at each of the first erase/program calls: every next boot opens
/// consistently, at the last completed save or the one in progress.
#[test]
fn every_cut_point_reopens_at_the_last_or_the_interrupted_save() {
    // Each save is one erase and two programs.
    for cut in 0..30 {
        let cut_boot = platform_boot_cut([0; 8], blank(), false, EXPECTED_ID, Some(cut));
        assert!(
            cut_boot.power_cut,
            "cut {cut} did not land: {:?}",
            cut_boot.records
        );

        let completed = u64::from(cut / 3);
        let next = platform_boot([0; 8], cut_boot.storage, false, EXPECTED_ID);
        let (status, generation, other, consistent) = opened_other(&next);
        assert!(consistent, "cut {cut}: {:?}", next.records);

        // A cut inside a save that did not complete shows in the other slot.
        if cut % 3 != 0 && generation == completed {
            assert_ne!(other, SlotState::Valid, "cut {cut}");
        }

        assert!(
            generation == completed || generation == completed + 1,
            "cut {cut}: generation {generation}, completed {completed}"
        );
        assert!(
            matches!(
                status,
                SettingsStatus::Loaded | SettingsStatus::Recovered | SettingsStatus::Empty
            ),
            "cut {cut}: {status:?}"
        );

        // The interrupted boot's stress continues on the next one.
        assert!(matches!(next.records.last(), Some(Record::End { .. })));
    }
}

#[test]
fn unknown_data_in_the_settings_sectors_is_never_written_over() {
    let mut storage = blank();
    storage[2 * 4096 + 9] = 0x12;
    let boot = platform_boot([0; 8], storage.clone(), false, EXPECTED_ID);

    assert_eq!(opened(&boot).0, SettingsStatus::Corrupt);
    assert!(boot.erases == 0 && boot.storage == storage && !boot.reset);
    assert_eq!(boot.backup[MARKER_REGISTER as usize], 0);
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
        Some((LeaseStage::Begin, FlashReply::Failed))
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
        Some((LeaseStage::Ticket, FlashReply::Stale))
    );
}
