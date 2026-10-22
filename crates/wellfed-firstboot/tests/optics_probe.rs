//! FW13 optics probe against the fake SDK, with the KM0 peer's handshake
//! answered through a shared IPC register model and the receivers driven
//! from the emitter latches by a synthetic optical path.

use std::cell::RefCell;

use wellfed_amebad::board::MOTOR_MASK;
use wellfed_amebad::events::EventsFault;
use wellfed_amebad::flash::park::{self, Mmio};
use wellfed_amebad::sdk::fake;
use wellfed_amebad::stopped;
use wellfed_firstboot::probes::optics::{
    self, CYCLES, Emitters, LEASE_MS, Probe, Record, State, WINDOWS, Window, plan,
};

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

type Km0 = Core<true>;
type Km4 = Core<false>;

/// About 1 ms of SYSTIMER (32 768 Hz).
const MS_TICKS: u32 = 33;

/// Receivers follow their emitter after `settle_ms`: PA17 food sees PA15,
/// PA16 outlet sees PB3. `block` forces an input LOW (beam interrupted).
#[derive(Default)]
struct Optics {
    settle_ms: u32,
    a_since: Option<u32>,
    b_since: Option<u32>,
    block: [bool; 2],
    ms: u32,
}

/// Whether a receiver is lit `settle_ms` after its emitter turned on.
fn lit(on: bool, since: &mut Option<u32>, ms: u32, settle_ms: u32) -> bool {
    if !on {
        *since = None;
        return false;
    }
    ms.wrapping_sub(*since.get_or_insert(ms)) >= settle_ms
}

impl Optics {
    fn tick(&mut self) {
        self.ms += 1;
        let (a, b) = fake::with(|h| (h.output[0] & 1 << 15 != 0, h.output[1] & 1 << 3 != 0));
        let food = lit(a, &mut self.a_since, self.ms, self.settle_ms) && !self.block[0];
        let outlet = lit(b, &mut self.b_since, self.ms, self.settle_ms) && !self.block[1];
        fake::with(|h| {
            h.input[0] &= !(1 << 16 | 1 << 17);
            h.input[0] |= u32::from(food) << 17 | u32::from(outlet) << 16;
        });
    }
}

struct Bench {
    probe: Box<Probe>,
    optics: Optics,
    answered: u64,
    peer: bool,
    records: Vec<Record>,
}

fn bench(settle_ms: u32, peer: bool) -> Bench {
    fake::reset_hardware();
    REGS.with(|r| *r.borrow_mut() = [[0; 12]; 2]);
    assert!(stopped::initialize().verified());
    Bench {
        probe: Box::default(),
        optics: Optics {
            settle_ms,
            ..Optics::default()
        },
        answered: 0,
        peer,
        records: Vec::new(),
    }
}

impl Bench {
    /// One probe iteration, then its delay in 1 ms steps. Returns the delay.
    fn step(&mut self, during: &mut impl FnMut(&mut Optics, &Probe)) -> u32 {
        if self.peer {
            park::answer::<Km0>(0x0000_0001_4b30_abcd, &mut self.answered);
        }
        let step = self.probe.step::<Km4>();
        if let Some(record) = step.record {
            self.records.push(record);
        }
        for _ in 0..step.delay_ms {
            fake::advance(MS_TICKS, true);
            self.optics.tick();
            during(&mut self.optics, &self.probe);
        }
        step.delay_ms
    }

    fn run(&mut self, mut during: impl FnMut(&mut Optics, &Probe)) {
        for _ in 0..1_000_000 {
            if self.step(&mut during) == 0 {
                return;
            }
        }
        panic!("probe did not finish");
    }

    fn windows(&self) -> Vec<Window> {
        self.records
            .iter()
            .filter_map(|r| match r {
                Record::Window(w) => Some(*w),
                _ => None,
            })
            .collect()
    }
}

fn line(record: Record) -> String {
    struct Line(Record);
    impl std::fmt::Display for Line {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            optics::describe(self.0, f)
        }
    }
    format!("PLOPT1 {}\r\n", Line(record))
}

#[test]
fn plan_alternates_each_emitter_with_off_windows() {
    assert_eq!(plan(0), (Emitters::Off, optics::BASELINE_MS));
    let cycle: Vec<_> = (1..=6).map(|n| plan(n).0).collect();
    use Emitters::*;
    assert_eq!(cycle, [A, Off, B, Off, Both, Off]);
    assert_eq!(plan(7), plan(1));
    assert_eq!(WINDOWS, 1 + CYCLES * 6);
    assert_eq!(plan(WINDOWS - 1).0, Off);
}

#[test]
fn full_run_pairs_emitters_measures_settling_and_cuts_a_stalled_owner() {
    let mut b = bench(40, true);
    let mut sensing_seen = [false; 2];
    let mut max_on = 0u32;
    let mut on_for = 0u32;
    b.run(|_, probe| {
        // Health's view: motor STOP throughout; emitters on only as published.
        let s = stopped::observe();
        assert!(s.motor_verified());
        let on = fake::with(|h| h.output[0] & 1 << 15 != 0 || h.output[1] & 1 << 3 != 0);
        if on {
            on_for += 1;
            max_on = max_on.max(on_for);
            assert!(probe.events.optics_armed && probe.board.optics_enabled);
            sensing_seen[0] |= s.sensing();
            assert!(s.sensing() || !s.verified());
        } else {
            on_for = 0;
            sensing_seen[1] |= s.verified();
        }
        fake::with(|h| assert_eq!(h.output[0] & MOTOR_MASK, MOTOR_MASK));
    });
    assert!(sensing_seen[0] && sensing_seen[1]);
    // The longest on-time is a both window or the stall's warm-up and cut.
    assert!(max_on <= optics::CYCLE[2].1 + 20, "{max_on}");

    let Record::Open {
        board,
        ready,
        attached,
        events,
        inputs,
        ..
    } = b.records[0]
    else {
        panic!("{:?}", b.records[0]);
    };
    assert!(board && ready && attached && events && inputs);

    let windows = b.windows();
    assert_eq!(windows.len(), WINDOWS as usize);
    let inputs: Vec<Window> = b
        .records
        .iter()
        .filter_map(|r| match r {
            Record::Inputs(w) => Some(*w),
            _ => None,
        })
        .collect();
    assert_eq!(inputs, windows);
    for w in &windows {
        let (emitters, len) = plan(w.n);
        assert_eq!(w.emitters, emitters);
        assert!(w.len_ms >= len && w.len_ms < len + 40, "{w:?}");
        assert!(w.stopped && w.dropped == 0, "{w:?}");
        assert!(w.samples >= len / 3, "{w:?}");
        assert_eq!(w.highs[2], 0);
        let (a, bb) = emitters.levels();
        assert_eq!(w.highs[0] > 0, a, "{w:?}");
        assert_eq!(w.highs[1] > 0, bb, "{w:?}");
        if a {
            // Rises after the 40 ms settle, within a poll or two.
            let first = w.first_change[0].unwrap();
            assert!((40..=46).contains(&first), "{w:?}");
        }
        if emitters == Emitters::Both {
            // Clear outlet at once; lit food beam reads empty after 1 s.
            assert_eq!((w.food, w.outlet), (State::Bad, State::Good), "{w:?}");
        } else {
            assert_eq!(
                (w.food, w.outlet),
                (State::Unknown, State::Unknown),
                "{w:?}"
            );
        }
        if emitters == Emitters::Off && w.n > 0 {
            // Receivers fall within a poll of the emitters turning off.
            let lit = plan(w.n - 1).0.levels();
            for (i, on) in [lit.0, lit.1].into_iter().enumerate() {
                assert_eq!(w.first_change[i].is_some(), on, "{w:?}");
                assert!(w.first_change[i].is_none_or(|ms| ms <= 3), "{w:?}");
            }
        }
    }

    let stall = b
        .records
        .iter()
        .find_map(|r| match r {
            Record::Stall {
                cut_ms,
                fault,
                off,
                stopped,
                closed,
            } => Some((*cut_ms, *fault, *off, *stopped, *closed)),
            _ => None,
        })
        .unwrap();
    let (cut, fault, off, stopped_, closed) = stall;
    let cut = cut.unwrap();
    assert!((LEASE_MS..=LEASE_MS + 12).contains(&cut), "{cut}");
    assert_eq!(fault, EventsFault::OWNER.0);
    assert!(off && stopped_ && closed);
    assert_eq!(b.records.last(), Some(&Record::Done { windows: WINDOWS }));
    assert!(!stopped::observe().sensing() && stopped::observe().verified());
    // Every record fits the startup report buffer.
    for record in &b.records {
        assert!(line(*record).len() < 192, "{}", line(*record));
    }
}

#[test]
fn records_are_logged_only_after_the_settling_window() {
    let mut b = bench(0, true);
    let mut during = |_: &mut Optics, _: &Probe| {};
    let mut logged = 0;
    for _ in 0..30_000 {
        // The age of the window under way when this iteration runs.
        let w = *b.probe.window();
        let age = b.probe.events.clock.millis.wrapping_sub(w.start_ms);
        let before = b.records.len();
        if b.step(&mut during) == 0 {
            break;
        }
        let record = b.records[before..].first().copied();
        // Summaries and edges of window n-1, inside window n past its quiet
        // period, unless window n is itself ending.
        if let Some(Record::Window(_) | Record::Inputs(_) | Record::Edge(_)) = record {
            logged += 1;
            // `age` is read one poll before the probe's own clock read.
            let age = age + optics::POLL_MS + 1;
            assert!(
                age >= optics::QUIET_MS || age >= w.len_ms,
                "{record:?} {age}"
            );
        }
    }
    assert!(logged > 10);
}

#[test]
fn a_blocked_outlet_and_a_short_pulse_are_measured_in_both_windows() {
    let mut b = bench(0, true);
    // Block the outlet for 20 ms in the first both window, then for 2 s in
    // the second: a pulse, then a qualified block.
    let both = |n: u32| 1 + 6 * n + 4;
    b.run(|optics, probe| {
        let w = probe.window();
        let age = probe.events.clock.millis.wrapping_sub(w.start_ms);
        optics.block[1] = if w.n == both(0) {
            (1000..1020).contains(&age)
        } else if w.n == both(1) {
            (500..2500).contains(&age)
        } else {
            false
        };
    });
    let windows = b.windows();
    let pulse = windows[both(0) as usize];
    let low = pulse.low_min[1].unwrap();
    assert!((18..=24).contains(&low), "{pulse:?}");
    assert_eq!(pulse.outlet, State::Good);
    let blocked = windows[both(1) as usize];
    assert!(blocked.low_min[1].unwrap() >= 1990, "{blocked:?}");
    // Released 1.5 s before the window ended: clear again at once.
    assert_eq!(blocked.outlet, State::Good);
    let edges: Vec<_> = b
        .records
        .iter()
        .filter_map(|r| match r {
            Record::Edge(e) if e.n == both(1) && e.input == 1 => Some((e.level, e.ms)),
            _ => None,
        })
        .collect();
    // Rise after enable, fall at 500, rise at 2500, fall when the window ends.
    assert_eq!(edges.len(), 3, "{edges:?}");
    assert!(!edges[1].0 && (500..=504).contains(&edges[1].1));
    assert!(edges[2].0 && (2500..=2504).contains(&edges[2].1));
}

#[test]
fn edges_beyond_the_cap_are_counted_not_kept() {
    let mut b = bench(0, true);
    let mut toggle = false;
    let mut ms = 0u32;
    b.run(|_, probe| {
        ms += 1;
        // Every 3 ms, so consecutive 2 ms polls see changes.
        if probe.window().n == 1 && ms.is_multiple_of(3) {
            toggle = !toggle;
            fake::with(|h| {
                if toggle {
                    h.input[0] |= 1 << 18;
                } else {
                    h.input[0] &= !(1 << 18);
                }
            });
        } else if probe.window().n != 1 {
            fake::with(|h| h.input[0] &= !(1 << 18));
        }
    });
    let w = b.windows()[1];
    assert!(w.edges > optics::EDGE_CAP as u32 * 4, "{w:?}");
    assert_eq!(w.dropped, w.edges - optics::EDGE_CAP as u32);
    let kept = b
        .records
        .iter()
        .filter(|r| matches!(r, Record::Edge(e) if e.n == 1))
        .count();
    assert_eq!(kept, optics::EDGE_CAP);
    assert!(w.low_min[2].unwrap() <= 6, "{w:?}");
}

#[test]
fn no_peer_times_out_with_emitters_never_on() {
    let mut b = bench(0, false);
    b.run(|_, _| {
        assert!(fake::with(
            |h| h.output[0] & 1 << 15 == 0 && h.output[1] & 1 << 3 == 0
        ));
    });
    let [Record::Timeout { ms }] = b.records[..] else {
        panic!("{:?}", b.records);
    };
    assert!((optics::READY_TIMEOUT_MS..optics::READY_TIMEOUT_MS + 20).contains(&ms));
    assert!(!b.probe.board.opened);
    assert!(stopped::observe().verified());
}

#[test]
fn a_lost_owner_lease_mid_window_ends_the_probe_stopped() {
    let mut b = bench(0, true);
    let mut during = |_: &mut Optics, _: &Probe| {};
    // Into the first PA15 window.
    while b.probe.events.clock.millis < optics::BASELINE_MS + 500 {
        assert_ne!(b.step(&mut during), 0);
    }
    assert!(fake::with(|h| h.output[0] & 1 << 15 != 0));
    // The task stalls past its lease: the timer cuts the emitter.
    for _ in 0..LEASE_MS + 20 {
        fake::advance(MS_TICKS, true);
    }
    assert!(fake::with(|h| h.output[0] & 1 << 15 == 0));
    assert_eq!(b.probe.events.fault, EventsFault::OWNER);
    assert_eq!(b.step(&mut during), 0);
    let Some(Record::Error { kind, n, fault }) = b.records.last().copied() else {
        panic!("{:?}", b.records.last());
    };
    assert_eq!(
        (kind, n, fault),
        (optics::ErrorKind::Supervisor, 1, EventsFault::OWNER.0)
    );
    assert!(stopped::observe().verified() && !stopped::observe().sensing());
}

#[test]
fn sensing_requires_exactly_the_published_emitters() {
    fake::reset_hardware();
    assert!(stopped::initialize().verified());
    stopped::set_supervised(0);
    assert!(!stopped::observe().sensing());
    fake::with(|h| h.output[0] |= 1 << 15);
    assert!(!stopped::observe().verified() && !stopped::observe().sensing());
    stopped::set_supervised(Emitters::A.mask());
    assert!(stopped::observe().sensing());
    stopped::set_supervised(Emitters::Both.mask());
    assert!(!stopped::observe().sensing());
    fake::with(|h| h.output[0] &= !MOTOR_MASK);
    stopped::set_supervised(Emitters::A.mask());
    assert!(!stopped::observe().sensing());
    stopped::set_supervised(0);
}

#[test]
fn describe_formats() {
    let w = Window {
        n: 48,
        emitters: Emitters::Both,
        start_ms: 160_000,
        len_ms: 4002,
        samples: 1600,
        max_gap_ms: 20,
        highs: [1600, 0, 3],
        begin: [true, false, false],
        end: [true, true, false],
        first_change: [None, Some(100), Some(4000)],
        low_min: [None, None, Some(4)],
        edges: 20,
        dropped: 4,
        food: State::Good,
        outlet: State::Bad,
        stopped: true,
    };
    assert_eq!(
        line(Record::Window(w)),
        "PLOPT1 window n=48 emitters=both start=160000 len=4002 samples=1600 gap=20 edges=20 \
         dropped=4 food=present outlet=blocked stopped=1\r\n"
    );
    assert_eq!(
        line(Record::Inputs(w)),
        "PLOPT1 inputs n=48 highs=1600,0,3 begin=100 end=110 first=-,100,4000 low_min=-,-,4\r\n"
    );
    assert_eq!(
        line(Record::Stall {
            cut_ms: None,
            fault: 7,
            off: false,
            stopped: true,
            closed: false
        }),
        "PLOPT1 stall cut=none fault=7 off=0 stopped=1 closed=0\r\n"
    );
    let worst = Window {
        n: u32::MAX,
        start_ms: u32::MAX,
        len_ms: u32::MAX,
        samples: u32::MAX,
        max_gap_ms: u32::MAX,
        highs: [u32::MAX; 3],
        first_change: [Some(u32::MAX); 3],
        low_min: [Some(u32::MAX); 3],
        edges: u32::MAX,
        dropped: u32::MAX,
        food: State::Unknown,
        outlet: State::Unknown,
        ..w
    };
    assert!(line(Record::Window(worst)).len() < 192);
    assert!(line(Record::Inputs(worst)).len() < 192);
}
