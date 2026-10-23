//! FW16 pulse probe against the fake SDK, with the KM0 peer's handshake
//! answered through a shared IPC register model.

use std::cell::RefCell;

use wellfed_hal::board::MOTOR_MASK;
use wellfed_hal::events::EventsFault;
use wellfed_hal::flash::park::{self, Mmio};
use wellfed_hal::sdk::{self, fake};
use wellfed_hal::stopped;
use wellfed_probes::pulse::{self, DELAY_MS, LEASE_MS, MARKER, PULSE_MS, Probe, Record};

thread_local! {
    static REGS: RefCell<[[u32; 12]; 2]> = const { RefCell::new([[0; 12]; 2]) };
}

fn locate(address: u32) -> (usize, usize) {
    let core = match address & 0xffff_f000 {
        0x4800_6000 => 0,
        0x4000_6000 => 1,
        _ => panic!("unexpected address {address:#x}"),
    };
    (core, ((address & 0xfff) - 0x20) as usize / 4)
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

const MS_TICKS: u32 = 33;

fn motor() -> u32 {
    fake::with(|h| h.output[0] & MOTOR_MASK)
}

struct Bench {
    probe: Box<Probe>,
    answered: u64,
    records: Vec<Record>,
    /// Motor pin states seen at every simulated millisecond.
    trace: Vec<u32>,
}

fn bench() -> Bench {
    fake::reset_hardware();
    REGS.with(|r| *r.borrow_mut() = [[0; 12]; 2]);
    assert!(stopped::initialize().verified());
    Bench {
        probe: Box::default(),
        answered: 0,
        records: Vec::new(),
        trace: Vec::new(),
    }
}

impl Bench {
    fn step(&mut self, during: &mut impl FnMut(u32)) -> u32 {
        park::answer::<Core<true>>(0x0000_0001_4b30_abcd, &mut self.answered);
        let step = self.probe.step::<Core<false>>();
        if let Some(record) = step.record {
            self.records.push(record);
        }

        for _ in 0..step.delay_ms {
            fake::advance(MS_TICKS, true);
            self.trace.push(motor());
            during(self.trace.len() as u32);
        }
        step.delay_ms
    }

    fn run(&mut self, mut during: impl FnMut(u32)) {
        for _ in 0..2_000_000 {
            if self.step(&mut during) == 0 {
                return;
            }
        }
        panic!("probe did not finish");
    }
}

fn line(record: Record) -> String {
    struct Line(Record);
    impl std::fmt::Display for Line {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            pulse::describe(self.0, f)
        }
    }
    format!("PLMOT1 {}\r\n", Line(record))
}

#[test]
fn one_bounded_pulse_after_the_delay_with_feedback_and_current() {
    let mut b = bench();
    // The mechanism's feedback toggles every 20 ms while driven.
    let mut driven_ms = 0u32;
    b.run(|_| {
        if motor() != MOTOR_MASK {
            driven_ms += 1;
            if driven_ms.is_multiple_of(20) {
                fake::with(|h| h.input[0] ^= 1 << 18);
            }
        }
    });

    let opened = b.records.iter().find_map(|r| match r {
        Record::Open {
            board,
            ready,
            attached,
            events,
            capture,
            ..
        } => Some(*board && *ready && *attached && *events && *capture),
        _ => None,
    });
    assert_eq!(opened, Some(true), "{:?}", b.records);

    // Exactly one drive interval, in direction 0 (PA13 high, PA14 low).
    let driven: Vec<usize> = (0..b.trace.len())
        .filter(|&i| b.trace[i] != MOTOR_MASK)
        .collect();
    assert!(!driven.is_empty());
    assert!(
        driven.windows(2).all(|w| w[1] == w[0] + 1),
        "drive must be one interval"
    );
    assert!(driven.iter().all(|&i| b.trace[i] == 1 << 13));
    assert!(
        (PULSE_MS as usize..=PULSE_MS as usize + 4).contains(&driven.len()),
        "{}",
        driven.len()
    );

    // Never before the operator delay. Each trace step is 33 SYSTIMER ticks
    // (about 1.007 ms), so the probe's 60 s arrive about 0.7 % sooner here.
    assert!(driven[0] as u32 >= DELAY_MS * 99 / 100, "{}", driven[0]);

    let Some(Record::Pulse {
        direction,
        applied_ms,
        on_ms,
        edges,
        first_edge_ms,
        current,
    }) = b
        .records
        .iter()
        .copied()
        .find(|r| matches!(r, Record::Pulse { .. }))
    else {
        panic!("{:?}", b.records);
    };

    assert_eq!(direction, 1);
    assert!(applied_ms <= 2, "{applied_ms}");
    assert!((PULSE_MS..PULSE_MS + 4).contains(&on_ms), "{on_ms}");
    assert!((4..=5).contains(&edges), "{edges}");
    assert!(first_edge_ms.is_some_and(|ms| (18..=22).contains(&ms)));
    // Fake ADC 500 with default calibration: (5000 - 2480) * 1000 / 12050.
    assert!(current.samples >= 8 && current.max == 209, "{current:?}");

    let after = b.records.iter().find_map(|r| match r {
        Record::After { edges, stopped, .. } => Some((*edges, *stopped)),
        _ => None,
    });
    assert_eq!(after, Some((0, true)));

    assert_eq!(b.records.last(), Some(&Record::Done));
    assert_eq!(sdk::backup_read(sdk::BACKUP_USER), MARKER);
    assert!(stopped::observe().verified());

    for record in &b.records {
        assert!(line(*record).len() < 192, "{}", line(*record));
    }
}

#[test]
fn a_stalled_task_mid_pulse_is_stopped_by_the_timer_within_the_lease() {
    let mut b = bench();
    let mut during = |_| {};
    while motor() == MOTOR_MASK {
        assert_ne!(b.step(&mut during), 0, "{:?}", b.records);
    }

    // The task stops running: only the timer interrupt is left.
    let mut ms = 0;
    while motor() != MOTOR_MASK {
        fake::advance(MS_TICKS, true);
        ms += 1;
        assert!(ms <= LEASE_MS + 12, "motor still driven after {ms} ms");
    }

    assert!(ms >= LEASE_MS - 4, "stopped early at {ms} ms");
    assert_ne!(b.probe.events.fault, EventsFault::OK);

    // The probe then ends with an error and the motor stopped.
    assert_eq!(b.step(&mut during), 0);
    assert!(matches!(
        b.records.last(),
        Some(Record::Error { stopped: true, .. })
    ));
}

#[test]
fn only_a_clean_cold_boot_pulses() {
    for (reset, marker) in [(0, MARKER), (0x40, 0)] {
        let mut b = bench();
        fake::with(|h| {
            h.boot_reason = reset;
            h.backup[sdk::BACKUP_USER as usize] = marker;
        });

        b.run(|_| {});

        assert_eq!(b.records, [Record::Skip { reset, marker }]);
        assert!(b.trace.iter().all(|&pins| pins == MOTOR_MASK));
        assert_eq!(sdk::backup_read(sdk::BACKUP_USER), 0);
    }
}

#[test]
fn no_peer_never_drives() {
    let mut b = bench();
    let during = |_| {};

    for _ in 0..1000 {
        // No KM0 answer.
        let step = b.probe.step::<Core<false>>();
        if let Some(r) = step.record {
            b.records.push(r);
        }
        if step.delay_ms == 0 {
            break;
        }
        for _ in 0..step.delay_ms {
            fake::advance(MS_TICKS, true);
            during(0);
            assert_eq!(motor(), MOTOR_MASK);
        }
    }

    assert!(
        matches!(b.records.as_slice(), [Record::Timeout { .. }]),
        "{:?}",
        b.records
    );
}
