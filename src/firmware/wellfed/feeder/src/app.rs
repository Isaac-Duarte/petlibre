//! The firmware's start hook and its loop task; the button service is the
//! other of its two tasks.

use wellfed_core::clock::wall::Restart;
use wellfed_feeder::feeder::{Feeder, Feeds, Marks};
use wellfed_feeder::panel::BUTTONS;
use wellfed_feeder::stats::reset_cause;
use wellfed_proto::api::{Blocked, EventBody, Refusal, ResetCause, Source, TraceSummary};
use wellfed_sdk::hal::sdk;
use wellfed_sdk::hal::supervision::config::SYSTIMER_HZ;
use wellfed_sdk::{Slot, Startup, log};

use crate::feeding::Feeding;
use crate::platform::Plaf103;

#[wellfed_sdk::start(runtime = wellfed_feeder_runtime)]
fn start() -> Startup {
    // The feeding owner needs the ADC to itself (and the entropy
    // measurement samples it too), so the power monitor stays off it; the
    // owner publishes power health from its own captures.
    wellfed_sdk::claim_adc();
    // A trial build of this firmware confirms only once the feeding owner
    // has proved it can feed (`feeding::Proof`).
    wellfed_sdk::claim_feeding();
    // Before the loop or the device API can read the clock.
    wellfed_sdk::hal::clock::resume(restart());

    let created = wellfed_sdk::spawn(loop_task);
    sdk::log(if created {
        c"PLPANEL1 task created=1\r\n"
    } else {
        c"PLPANEL1 task created=0\r\n"
    });
    wellfed_sdk::buttons::start(BUTTONS);
    wellfed_sdk::hal::clock::log_rtc();

    #[cfg(feature = "gap-probe")]
    gap_probe::start();

    // Nothing here holds the KM0 flash handshake, so the runtime may confirm
    // once it is otherwise healthy. Nothing that reads this runs before the
    // hook returns: the runtime's service task starts after it.
    Startup::Done
}

/// Whether this boot followed a reset that kept the KM0 RTC counting: a
/// watchdog or software reset. A power-on, a brownout or a deep-sleep wake
/// did not, or may not have.
fn restart() -> Restart {
    match reset_cause(sdk::boot_reason()) {
        ResetCause::Watchdog | ResetCause::Software => Restart::Warm,
        ResetCause::PowerOn | ResetCause::Brownout | ResetCause::Other => Restart::Cold,
    }
}

/// The feeding owner's step, then the pass with a Noise operation and one
/// formatted record; the build's stack audit checks the bound. A handshake
/// operation's X25519 is the deepest path, about 5 KiB with the device API
/// above it, inside 6 KiB's margin, so the loop has 8 KiB.
const LOOP_STACK_WORDS: u16 = 2048;

/// The loop task's: the product's loop on this board. Built at compile
/// time: it holds the device API's session table and setup mode's state,
/// which the loop's stack cannot afford to move.
static FEEDER: Slot<Feeder<Plaf103>> = Slot::filled(Feeder::new(Plaf103::new()));
/// The loop task's half of the feeding owner; the owner itself is
/// `feeding`'s.
static FEEDING: Slot<Feeding> = Slot::new();
#[cfg(feature = "entropy")]
static CAPTURE: Slot<crate::entropy::Capture> = Slot::filled(crate::entropy::Capture::new());

impl Feeds for Feeding {
    fn feeding(&self) -> bool {
        Feeding::feeding(self)
    }

    fn busy(&self) -> bool {
        Feeding::busy(self)
    }

    fn blocked(&self) -> Option<Blocked> {
        Feeding::blocked(self)
    }

    #[cfg(not(feature = "gap-probe"))]
    fn submit(&mut self, portions: u8, by: Source) -> Result<u32, Refusal> {
        Feeding::submit(self, portions, by, |record| {
            log(format_args!("PLOWN1 {record}\r\n"));
        })
    }

    /// The scheduling measurement runs with nobody at the motor, so its
    /// image cannot start a feed: the owner stays at rest.
    #[cfg(feature = "gap-probe")]
    fn submit(&mut self, _: u8, _: Source) -> Result<u32, Refusal> {
        Err(Refusal::Unavailable)
    }

    fn reserved(&self, feed: u32) -> Option<Result<(), Refusal>> {
        Feeding::reserved(self, feed)
    }

    fn cancel(&self, feed: u32) -> bool {
        Feeding::cancel(self, feed)
    }

    fn food(&self) -> Option<bool> {
        Feeding::food(self)
    }

    fn marks(&self) -> Option<Marks> {
        Feeding::marks(self)
    }
}

#[wellfed_sdk::task(name = "wellfed-loop", stack = LOOP_STACK_WORDS, priority = 2)]
fn loop_task() -> u32 {
    let (Some(mut feeder), Some(mut feeding)) =
        (FEEDER.enter(), FEEDING.enter_or_init(Feeding::default))
    else {
        return 0;
    };

    #[cfg(feature = "gap-probe")]
    gap_probe::LOOP.wake(sdk::systimer_ticks());
    #[cfg(feature = "owner-stall")]
    stall::step();
    #[cfg(any(feature = "feed-stall", feature = "stop-fail"))]
    inject::step(&feeding);
    #[cfg(any(feature = "feed-jam", feature = "sensor-fault"))]
    show_faults::step(&feeding);
    let began = sdk::systimer_ticks();
    let mut feed_delay = feeding.step(
        |record| log(format_args!("PLOWN1 {record}\r\n")),
        |body, trace| outcome(&mut feeder, body, trace),
    );

    let now_ms = sdk::uptime_ms();
    let mut delay_ms = feeder.pass(now_ms, &mut *feeding, |record| {
        log(format_args!("{} {record}\r\n", record.tag()));
    });
    feeder.pass_took(ticks_us(sdk::systimer_ticks().wrapping_sub(began)));
    wellfed_sdk::hal::clock::keep();
    // A handshake operation is the pass's costly part (one or two X25519s,
    // 25-28 ms each): during a feed the owner is stepped again at once, so
    // the gap between its steps is the pass, not the pass and the delay.
    if feeding.feeding() && feeder.handshook() {
        feeding.handshook();
        feed_delay = feeding.step(
            |record| log(format_args!("PLOWN1 {record}\r\n")),
            |body, trace| outcome(&mut feeder, body, trace),
        );
    }
    if let Some(feed_delay) = feed_delay {
        delay_ms = delay_ms.min(feed_delay);
    }

    #[cfg(feature = "entropy")]
    if let Some(mut capture) = CAPTURE.enter()
        && let Some(delay) = capture.step(|record| {
            log(format_args!("PLRNG1 {record}\r\n"));
        })
    {
        delay_ms = delay_ms.min(delay);
    }

    #[cfg(feature = "gap-probe")]
    gap_probe::end_pass(delay_ms);
    delay_ms
}

/// A feed's end: its `Fed` event, and its trace's figures for the
/// statistics.
fn outcome(feeder: &mut Feeder<Plaf103>, body: EventBody, trace: Option<&TraceSummary>) {
    feeder.record_outcome(sdk::uptime_ms(), body);
    if let Some(trace) = trace {
        feeder.traced(trace);
    }
}

/// `ticks` of the 32 kHz system timer in microseconds (a tick is about
/// 30.5 us, the loop metric's resolution).
fn ticks_us(ticks: u32) -> u32 {
    let us = u64::from(ticks) * 1_000_000 / u64::from(SYSTIMER_HZ);
    u32::try_from(us).unwrap_or(u32::MAX)
}

/// The `owner-stall` variant: the loop stops stepping for a while, once each, so
/// the bench can watch what a hung loop does to the owner's beat. The owner
/// is at rest, where its beat allows 10 s (`LIVENESS_MS`; 1 s only while
/// outputs may move). Two stalls: one past the moving gap but inside the
/// rest gap, which must change nothing, and one past the rest gap, which
/// must end in a watchdog reset. Both wait until the build has confirmed
/// itself: a reset during a trial would count against it. Every boot
/// repeats them, so after confirmation the image resets about 3 minutes
/// into every boot: a reset storm, which KM0 ends by giving way to bank 0
/// at the sixth boot after the one that confirmed. It goes back out after
/// the run.
#[cfg(feature = "owner-stall")]
mod stall {
    use core::sync::atomic::{AtomicU8, Ordering};

    use wellfed_sdk::hal::sdk;

    /// Uptime of each stall, and how long it spins.
    const STALLS: [(u32, u32); 2] = [(120_000, 3000), (180_000, 12_000)];

    static DONE: AtomicU8 = AtomicU8::new(0);

    /// Spins with interrupts enabled, so the supervision timer and the
    /// watchdog feeder keep running while the loop does not.
    pub fn step() {
        let done = usize::from(DONE.load(Ordering::Relaxed));
        let Some(&(at_ms, spin_ms)) = STALLS.get(done) else {
            return;
        };
        let now = sdk::uptime_ms();
        if now < at_ms || !wellfed_sdk::storage_armed() || wellfed_sdk::confirming() {
            return;
        }

        sdk::log(c"PLOWN1 stall begin=1\r\n");
        while sdk::uptime_ms().wrapping_sub(now) < spin_ms {
            core::hint::spin_loop();
        }
        sdk::log(c"PLOWN1 stall end=1\r\n");
        DONE.fetch_add(1, Ordering::Relaxed);
    }
}

/// The `feed-stall` and `stop-fail` variants: one fault in each feed's
/// drive, chosen by `faults::Trigger`, injected before the owner's step so
/// a cancel it asks for is taken in the same pass. Every boot is armed,
/// so the image goes back out after the run.
#[cfg(any(feature = "feed-stall", feature = "stop-fail"))]
mod inject {
    use wellfed_sdk::hal::sdk;
    use wellfed_sdk::{Slot, log};

    use crate::faults::{Fault, Injection, Trigger};
    use crate::feeding::Feeding;

    #[cfg(all(feature = "feed-stall", feature = "stop-fail"))]
    compile_error!("feed-stall and stop-fail are separate variants");

    #[cfg(feature = "feed-stall")]
    const FAULT: Fault = Fault::Stall;
    #[cfg(feature = "stop-fail")]
    const FAULT: Fault = Fault::FailedStop;

    /// The loop task's.
    static TRIGGER: Slot<Trigger> = Slot::filled(Trigger::new(FAULT));

    pub fn step(feeding: &Feeding) {
        let Some(mut trigger) = TRIGGER.enter() else {
            return;
        };
        #[cfg(feature = "stop-fail")]
        hold::report();

        let Some(injection) = trigger.step(feeding.driving(), sdk::uptime_ms()) else {
            return;
        };
        log(format_args!(
            "PLINJ1 inject id={} fault={} severity={} ms={}\r\n",
            injection.feed,
            injection.fault.code(),
            injection.severity.code(),
            injection.ms()
        ));
        match injection.fault {
            Fault::Stall => stall(&injection),
            #[cfg(feature = "stop-fail")]
            Fault::FailedStop => hold::fail_stop(feeding, &injection),
            #[cfg(not(feature = "stop-fail"))]
            Fault::FailedStop => {}
        }
    }

    /// Spins with interrupts enabled, so the supervision timer and the
    /// runtime's watchdog feeder run while the owner is not stepped.
    fn stall(injection: &Injection) {
        let from = sdk::uptime_ms();
        while sdk::uptime_ms().wrapping_sub(from) < injection.ms() {
            core::hint::spin_loop();
        }

        // After a long stall, only a loop the watchdog did not reset gets here.
        log(format_args!(
            "PLINJ1 stall id={} end ms={}\r\n",
            injection.feed,
            sdk::uptime_ms().wrapping_sub(from)
        ));
    }

    /// The failed stop: STOP writes held, then the feed cancelled, so the
    /// stop the owner asks for, and every retry until the hold lifts, fails
    /// its readback.
    #[cfg(feature = "stop-fail")]
    mod hold {
        use core::sync::atomic::{AtomicBool, Ordering};

        use wellfed_sdk::hal::inject;
        use wellfed_sdk::log;

        use crate::faults::Injection;
        use crate::feeding::Feeding;

        /// A hold is armed whose end is not yet logged.
        static WATCHING: AtomicBool = AtomicBool::new(false);

        pub fn fail_stop(feeding: &Feeding, injection: &Injection) {
            inject::hold_stops(injection.ms());
            WATCHING.store(true, Ordering::Relaxed);

            let cancelled = u32::try_from(injection.feed).is_ok_and(|feed| feeding.cancel(feed));
            log(format_args!(
                "PLINJ1 stop id={} cancel={}\r\n",
                injection.feed,
                u8::from(cancelled)
            ));
        }

        /// Logs the hold's end, with how many STOP writes it skipped: the
        /// timer interrupt's retries, one a tick, and the owner's own.
        pub fn report() {
            if !WATCHING.load(Ordering::Relaxed) {
                return;
            }
            let hold = inject::status();
            if hold.armed {
                return;
            }

            WATCHING.store(false, Ordering::Relaxed);
            log(format_args!(
                "PLINJ1 hold lifted={} skipped={}\r\n",
                u8::from(hold.lifted),
                hold.skipped
            ));
        }
    }
}

/// The `feed-jam` and `sensor-fault` variants: each feed shown its jam or
/// sensor fault, chosen by `feed_faults::Planner`, before the owner's step,
/// so the owner's next capture already reads it. Every boot is armed, so
/// the image goes back out after the run.
#[cfg(any(feature = "feed-jam", feature = "sensor-fault"))]
mod show_faults {
    use wellfed_sdk::hal::feed_faults::{self, Injected};
    use wellfed_sdk::hal::sdk;
    use wellfed_sdk::{Slot, log};

    use crate::feed_faults::{Action, Case, Family, Planner};
    use crate::feeding::Feeding;

    #[cfg(all(feature = "feed-jam", feature = "sensor-fault"))]
    compile_error!("feed-jam and sensor-fault are separate variants");

    #[cfg(feature = "feed-jam")]
    const FAMILY: Family = Family::Jam;
    #[cfg(feature = "sensor-fault")]
    const FAMILY: Family = Family::Sensor;

    /// The loop task's.
    static PLANNER: Slot<Planner> = Slot::filled(Planner::new(FAMILY));

    const fn injected(case: Case) -> Injected {
        match case {
            Case::JamFrees => Injected::JamFrees,
            Case::JamHolds => Injected::JamHolds,
            Case::MarkStuckLow => Injected::MarkStuckLow,
            Case::MarksMissing => Injected::MarksMissing,
            Case::OutletBlocked => Injected::OutletBlocked,
            Case::AdcLost => Injected::AdcFails,
        }
    }

    pub fn step(feeding: &Feeding) {
        let Some(mut planner) = PLANNER.enter() else {
            return;
        };
        match planner.step(feeding.phase(), sdk::uptime_ms()) {
            Some(Action::Arm { feed, case }) => {
                let fault = injected(case);
                feed_faults::arm(fault);
                log(format_args!(
                    "PLINJ1 show id={feed} fault={}\r\n",
                    fault.code()
                ));
            }
            Some(Action::Clear { feed }) => {
                // What the fault changed, before clearing the counts with it.
                let counts = feed_faults::counts();
                let left = feed_faults::armed();
                feed_faults::arm(Injected::None);
                log(format_args!(
                    "PLINJ1 clear id={feed} armed={} readings={} edges={}\r\n",
                    left.code(),
                    counts.readings,
                    counts.edges
                ));
            }
            None => {}
        }
    }
}

/// The `gap-probe` variant: a priority-4 task that only measures its own
/// wakes, beside the loop measuring its passes, for the bench to compare
/// across Wi-Fi rejoins (`gaps`). The probe task touches nothing but its
/// probe's atomics: not the owner, the board, flash or the watchdog.
#[cfg(feature = "gap-probe")]
mod gap_probe {
    use wellfed_sdk::Slot;
    use wellfed_sdk::hal::{net_status, sdk};

    use crate::gaps::{PERIOD_MS, Probe, Windows};

    /// The probe's step is a timer read and a few atomic updates.
    const PROBE_STACK_WORDS: u16 = 256;

    /// The loop task's passes; only the loop wakes it.
    pub static LOOP: Probe = Probe::new(0);
    /// The probe task's wakes; only the probe task wakes it.
    static HIGH: Probe = Probe::new(PERIOD_MS);
    /// The loop task's.
    static WINDOWS: Slot<Windows> = Slot::filled(Windows::new());

    pub fn start() {
        let created = wellfed_sdk::spawn(gap_task);
        sdk::log(if created {
            c"PLGAP1 task created=1\r\n"
        } else {
            c"PLGAP1 task created=0\r\n"
        });
    }

    /// The loop asked for `delay_ms` before its next pass; logs a window
    /// once one has run.
    pub fn end_pass(delay_ms: u32) {
        LOOP.asked(delay_ms);
        let Some(mut windows) = WINDOWS.enter() else {
            return;
        };
        windows.step(
            sdk::uptime_ms(),
            &LOOP,
            &HIGH,
            net_status::read(),
            |record| {
                wellfed_sdk::log(format_args!("PLGAP1 {record}\r\n"));
            },
        );
    }

    #[wellfed_sdk::task(name = "wellfed-gap", stack = PROBE_STACK_WORDS, priority = 4)]
    fn gap_task() -> u32 {
        HIGH.wake(sdk::systimer_ticks());
        PERIOD_MS
    }
}
