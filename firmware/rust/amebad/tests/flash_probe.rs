//! KM4 handshake and lease probe against the fake SDK and a shared IPC model.
//! The KM0 side is modelled inline: it answers HELLO through the real
//! `Peer::step`, and acknowledges/releases park requests on SYSTIMER reads,
//! as the parked core's RAM loop would. Watchdog expiry ends a boot; backup
//! registers survive it and are cleared by power loss.

use std::cell::RefCell;

use petlibro_amebad::flash_park::{
    self, ACK, Mmio, RELEASE, RELEASED, REQUEST, SYSTICK_CTRL, TICKS, TIMEOUT,
};
use petlibro_amebad::flash_peer::{
    BACKUP_BASE, HEARTBEAT_REGISTER, INJECT_LEASE, MARKER_REGISTER, PARK_CHANNEL, PARK_TICK_LIMIT,
    Peer,
};
use petlibro_amebad::flash_probe::{FAILED_MARKER, LEASES, LeaseStage, Mode, Probe, Record};
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
}

fn km0<R>(f: impl FnOnce(&mut Km0) -> R) -> R {
    KM0.with(|m| f(&mut m.borrow_mut()))
}

/// The parked KM0's behaviour, run whenever KM4 samples the shared counter.
fn react() {
    let marker = fake::with(|h| h.backup[MARKER_REGISTER as usize]);
    km0(|m| {
        if m.halted {
            return;
        }
        let km4 = m.regs[1];
        match m.parked_lease {
            None if km4[10] == REQUEST && !m.deaf => {
                let lease = km4[6];
                let fresh = m.regs[0][6] != lease || m.regs[0][10] == RELEASED;
                if fresh && !(u64::from(lease) == INJECT_LEASE && marker == 1) {
                    let mut reply = km4;
                    reply[10] = ACK;
                    for i in [2, 3, 4, 5, 6, 9, 10] {
                        m.regs[0][i] = reply[i];
                    }
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

/// Fresh hardware for one boot with the given backup registers. Host
/// registration pointers are thread-local, so each boot runs in a new thread
/// (see `platform_boot`), as a platform reset clears RAM.
fn boot(backup: [u32; 8], deaf: bool) -> (Box<Probe>, Peer) {
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
    (Box::default(), Peer::default())
}

struct Boot {
    records: Vec<Record>,
    reset: bool,
    backup: [u32; 8],
    rings: u32,
    km0_halted: bool,
    watchdog_armed: bool,
}

fn platform_boot(backup: [u32; 8], deaf: bool) -> Boot {
    std::thread::spawn(move || {
        let (mut probe, mut peer) = boot(backup, deaf);
        let (records, reset) = run(&mut probe, &mut peer);
        Boot {
            records,
            reset,
            backup: fake::with(|h| h.backup),
            rings: km0(|m| m.rings),
            km0_halted: km0(|m| m.halted),
            watchdog_armed: probe.watchdog.armed,
        }
    })
    .join()
    .unwrap()
}

/// Run one boot until the probe ends, the watchdog expires, or it idles.
fn run(probe: &mut Probe, peer: &mut Peer) -> (Vec<Record>, bool) {
    let mut records = Vec::new();
    let mut idle_steps = 0;
    for _ in 0..200_000 {
        let step = probe.step::<Core<false>>();
        if matches!(records.last(), Some(Record::Done { .. })) {
            idle_steps += 1;
        }
        records.extend(step.record);
        if step.delay_ms == 0 || idle_steps > 40 {
            break;
        }
        for _ in 0..step.delay_ms {
            fake::advance(33, false);
            react();
            if fake::with(|h| h.watchdog_expired) {
                return (records, true);
            }
            if !km0(|m| m.halted || m.parked_lease.is_some()) {
                peer.step::<Core<true>>(1, sdk::systimer_ticks());
            }
        }
    }
    (records, false)
}

fn batches(records: &[Record]) -> Vec<Record> {
    records
        .iter()
        .copied()
        .filter(|r| matches!(r, Record::Batch { .. }))
        .collect()
}

fn assert_clean_leases(records: &[Record]) {
    let b = batches(records);
    assert_eq!(b.len(), (LEASES / 25) as usize, "{records:?}");
    let Record::Batch {
        leases,
        frozen,
        alive,
        park_max,
        release_max,
    } = b[3]
    else {
        unreachable!()
    };
    assert_eq!((leases, frozen), (LEASES, LEASES));
    assert!(alive >= LEASES - 1, "{records:?}");
    assert!(park_max > 0 && release_max > 0);
}

#[test]
fn three_boots_inject_both_faults_then_run_clean() {
    let first = platform_boot([0; 8], false);
    let records = &first.records;
    assert!(matches!(records[0], Record::Hello { count: 1, .. }));
    assert!(matches!(
        records[1],
        Record::Ready {
            reply: FlashReply::ACCEPTED,
            phase: FlashPhase::IDLE,
            fault: FlashFault::OK,
            inactive: true,
            ..
        }
    ));
    assert_eq!(
        records[2],
        Record::Start {
            mode: Mode::WITHHELD_ACK,
            marker: 0,
            watchdog: true
        }
    );
    assert_clean_leases(records);
    let Some(Record::Inject {
        mode: Mode::WITHHELD_ACK,
        lease,
        begin: false,
        ..
    }) = records.last()
    else {
        panic!("{records:?}");
    };
    assert_eq!(*lease, INJECT_LEASE);
    assert!(first.reset && first.backup[MARKER_REGISTER as usize] == 1);
    assert_eq!(first.rings, INJECT_LEASE as u32);

    let second = platform_boot(first.backup, false);
    let records = &second.records;
    assert!(matches!(records[0], Record::Hello { count: 2, .. }));
    assert!(matches!(
        records[2],
        Record::Start {
            mode: Mode::WITHHELD_RELEASE,
            marker: 1,
            ..
        }
    ));
    assert_clean_leases(records);
    let injections: Vec<_> = records
        .iter()
        .filter(|r| matches!(r, Record::Inject { .. }))
        .collect();
    assert!(
        matches!(
            injections[..],
            [
                Record::Inject {
                    mode: Mode::WITHHELD_RELEASE,
                    begin: true,
                    ..
                },
                Record::Inject {
                    mode: Mode::WITHHELD_RELEASE,
                    begin: true,
                    peer: TIMEOUT,
                    ..
                },
            ]
        ),
        "{records:?}"
    );
    assert!(second.reset && second.km0_halted && second.backup[MARKER_REGISTER as usize] == 2);

    let third = platform_boot(second.backup, false);
    let records = &third.records;
    assert!(matches!(
        records[2],
        Record::Start {
            mode: Mode::NONE,
            marker: 2,
            ..
        }
    ));
    assert_clean_leases(records);
    assert_eq!(
        records.last(),
        Some(&Record::Done {
            leases: LEASES,
            marker: 2
        })
    );
    // Idle keeps feeding: no reset, and the marker is cleared for the next power-on.
    assert!(!third.reset && third.backup[MARKER_REGISTER as usize] == 0);
}

#[test]
fn lease_failure_resets_once_then_the_next_boot_skips() {
    let first = platform_boot([0; 8], true);
    assert!(
        matches!(
            first.records.last(),
            Some(Record::LeaseError {
                lease: 1,
                stage: LeaseStage::BEGIN,
                ..
            })
        ),
        "{:?}",
        first.records
    );
    assert!(first.reset && first.backup[MARKER_REGISTER as usize] == FAILED_MARKER);
    let second = platform_boot(first.backup, true);
    assert_eq!(
        second.records.last(),
        Some(&Record::Skip {
            marker: FAILED_MARKER
        })
    );
    assert!(!second.reset && second.backup[MARKER_REGISTER as usize] == 0);
    assert!(!second.watchdog_armed);
}

#[test]
fn peer_heartbeat_register_is_the_one_km4_reads() {
    let (_, mut peer) = boot([0; 8], false);
    peer.step::<Core<true>>(1, 0);
    peer.step::<Core<true>>(1, 1);
    assert_eq!(sdk::backup_read(HEARTBEAT_REGISTER), 2);
    assert_eq!(flash_park::peer_words::<Core<false>>(), None);
}
