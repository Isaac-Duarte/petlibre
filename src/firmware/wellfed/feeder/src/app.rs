//! The firmware's start hook and its loop task; the button service is the
//! other of its two tasks.

use core::ffi::c_void;

use wellfed_feeder::panel::{BUTTONS, Event, Source};
use wellfed_proto::api::{EventBody, Refusal, Source as EventSource};
use wellfed_sdk::Slot;
use wellfed_sdk::hal::{panel, sdk, settings, startup};

use crate::api::{self, Api};
use crate::feeding::{self, Feeding};
use crate::keys::{self, Identity};
use crate::owner::{self, Inputs, Owner};
use crate::setup;

wellfed_sdk::firmware!(start);

fn start() {
    // The feeding owner needs the ADC to itself (and the entropy
    // measurement samples it too), so the power monitor stays off it; the
    // owner publishes power health from its own captures.
    wellfed_sdk::claim_adc();

    // Nothing here holds the KM0 flash handshake, so the runtime may confirm
    // once it is otherwise healthy.
    wellfed_sdk::startup_done(true);

    // SAFETY: the step ignores its context and keeps its state in statics.
    let created = unsafe { wellfed_sdk::spawn(c"wellfed-loop", LOOP_STACK_WORDS, 2, loop_task) };
    sdk::log(if created {
        c"PLPANEL1 task created=1\r\n"
    } else {
        c"PLPANEL1 task created=0\r\n"
    });
    wellfed_sdk::buttons::start(BUTTONS);

    #[cfg(feature = "gap-probe")]
    gap_probe::start();
}

/// The step, a Noise operation and one formatted record; the build's stack
/// audit checks the bound. A handshake operation's X25519 is the deepest
/// path (3820 bytes). The device API took the bound to 4,552 bytes,
/// just inside 6 KiB's margin, so the loop has 8 KiB: the feeding
/// owner joins it next.
const LOOP_STACK_WORDS: u16 = 2048;

/// The loop task's; see `loop_task`. Built at compile time: it holds setup
/// mode's state, which the loop's stack cannot afford to move.
static OWNER: Slot<Owner> = Slot::filled(Owner::new());
/// The loop task's.
static IDENTITY: Slot<Identity> = Slot::new();
/// The device API's session table is larger than the loop's stack, so it is
/// built at compile time. The loop task's.
static API: Slot<Api> = Slot::filled(Api::new());
/// The loop task's half of the feeding owner; the owner itself is
/// `feeding`'s.
static FEEDING: Slot<Feeding> = Slot::new();
#[cfg(feature = "entropy")]
static CAPTURE: Slot<crate::entropy::Capture> = Slot::filled(crate::entropy::Capture::new());

/// Writes one `PL…1` record line.
fn log(args: core::fmt::Arguments<'_>) {
    let mut report = startup::Report::new();
    if let Ok(line) = report.record(args) {
        sdk::log(line);
    }
}

struct Panel(owner::Record);

impl core::fmt::Display for Panel {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        owner::describe(self.0, f)
    }
}

struct Key(keys::Record);

impl core::fmt::Display for Key {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        keys::describe(self.0, f)
    }
}

struct FeedingRecord(feeding::Record);

impl core::fmt::Display for FeedingRecord {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        feeding::describe(self.0, f)
    }
}

struct ApiRecord(api::Record);

impl core::fmt::Display for ApiRecord {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        api::describe(self.0, f)
    }
}

impl api::Feeds for Feeding {
    fn feeding(&self) -> bool {
        Feeding::feeding(self)
    }

    fn busy(&self) -> bool {
        Feeding::busy(self)
    }

    #[cfg(not(feature = "gap-probe"))]
    fn submit(&mut self, portions: u8, by: EventSource) -> Result<u32, Refusal> {
        Feeding::submit(self, portions, by, |record| {
            log(format_args!("PLOWN1 {}\r\n", FeedingRecord(record)));
        })
    }

    /// The scheduling measurement runs with nobody at the motor, so its
    /// image cannot start a feed: the owner stays at rest.
    #[cfg(feature = "gap-probe")]
    fn submit(&mut self, _: u8, _: EventSource) -> Result<u32, Refusal> {
        Err(Refusal::Unavailable)
    }

    fn reserved(&self, feed: u32) -> Option<Result<(), Refusal>> {
        Feeding::reserved(self, feed)
    }

    fn cancel(&self, feed: u32) -> bool {
        Feeding::cancel(self, feed)
    }
}

unsafe extern "C" fn loop_task(_: *mut c_void) -> u32 {
    let (Some(mut owner), Some(mut api), Some(mut identity), Some(mut feeding)) = (
        OWNER.enter(),
        API.enter(),
        IDENTITY.enter_or_init(Identity::default),
        FEEDING.enter_or_init(Feeding::default),
    ) else {
        return 0;
    };

    #[cfg(feature = "gap-probe")]
    gap_probe::LOOP.wake(sdk::systimer_ticks());
    #[cfg(feature = "owner-stall")]
    stall::step();
    let feed_delay = feeding.step(|record| {
        log(format_args!("PLOWN1 {}\r\n", FeedingRecord(record)));
    });

    if let Some(record) = identity.step() {
        log(format_args!("PLID1 {}\r\n", Key(record)));
    }

    let now_ms = sdk::uptime_ms();
    let inputs = Inputs {
        now_ms,
        gestures: core::iter::from_fn(wellfed_sdk::buttons::next),
        remote: panel::take(),
        lights: settings::current().is_none_or(|view| view.lights),
        identity: identity.keypair(),
    };
    let mut delay_ms = owner.step(inputs, |record| match record {
        owner::Record::Setup(setup::Record::Ble(_)) => {
            log(format_args!("PLBLE1 {}\r\n", Panel(record)));
        }
        owner::Record::Setup(setup::Record::Audio(_)) => {
            log(format_args!("PLAUD1 {}\r\n", Panel(record)));
        }
        owner::Record::Setup(_) => log(format_args!("PLSET1 {}\r\n", Panel(record))),
        owner::Record::Panel(Event::Changed { lock, source }) => {
            // A client's change is recorded by the device API, with who made it.
            if source != Source::Remote {
                let by = match source {
                    Source::Button => EventSource::Panel,
                    _ => EventSource::Feeder,
                };
                api::record_event(EventBody::Lock {
                    locked: lock.locked(),
                    by,
                });
            }
            log(format_args!("PLPANEL1 {}\r\n", Panel(record)));
        }
        _ => log(format_args!("PLPANEL1 {}\r\n", Panel(record))),
    });

    if owner.reboot_due(now_ms) {
        wellfed_sdk::request_reset();
    }

    // Requests read what the owner holds after this step.
    if let Some(lock) = owner.lock() {
        panel::publish(lock.locked());
    }

    let locked = owner.lock().map(wellfed_feeder::panel::Lock::locked);
    let identity = identity.keypair();
    let recovery = owner.take_recovery();
    let signer = owner.take_signer();
    let inputs = api::Inputs {
        now_ms,
        pairing: owner.pairing(),
        identity,
        locked,
        recovery,
        signer,
        feeds: &mut *feeding,
    };
    let api_delay = api.step(inputs, |record| {
        log(format_args!("PLAPI1 {}\r\n", ApiRecord(record)));
    });
    delay_ms = delay_ms.min(api_delay);
    if let Some(feed_delay) = feed_delay {
        delay_ms = delay_ms.min(feed_delay);
    }

    #[cfg(feature = "entropy")]
    if let Some(mut capture) = CAPTURE.enter()
        && let Some(delay) = capture.step(|record| {
            log(format_args!("PLRNG1 {}\r\n", Rng(record)));
        })
    {
        delay_ms = delay_ms.min(delay);
    }

    #[cfg(feature = "gap-probe")]
    gap_probe::end_pass(delay_ms);
    delay_ms
}

#[cfg(feature = "entropy")]
struct Rng(crate::entropy::Record);

#[cfg(feature = "entropy")]
impl core::fmt::Display for Rng {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        crate::entropy::describe(self.0, f)
    }
}

/// The `owner-stall` variant: the loop stops stepping for a while, once each, so
/// the bench can watch what a hung loop does to the owner's beat. The owner
/// is at rest, where its beat allows 10 s (`LIVENESS_MS`; 1 s only while
/// outputs may move). Two stalls: one past the moving gap but inside the
/// rest gap, which must change nothing, and one past the rest gap, which
/// must end in a watchdog reset. Both wait for the armed hooks, which come only after the build
/// confirmed itself: a reset during a trial would count against it. Every
/// boot repeats them, so the image goes back out after the run.
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
        if now < at_ms || !wellfed_sdk::storage_armed() {
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

/// The `gap-probe` variant: a priority-4 task that only measures its own
/// wakes, beside the loop measuring its passes, for the bench to compare
/// across Wi-Fi rejoins (`gaps`). The probe task touches nothing but its
/// probe's atomics: not the owner, the board, flash or the watchdog.
#[cfg(feature = "gap-probe")]
mod gap_probe {
    use core::ffi::c_void;

    use wellfed_sdk::Slot;
    use wellfed_sdk::hal::{net_status, sdk};

    use crate::gaps::{self, PERIOD_MS, Probe, Windows};

    /// The probe's step is a timer read and a few atomic updates.
    const PROBE_STACK_WORDS: u16 = 256;

    /// The loop task's passes; only the loop wakes it.
    pub static LOOP: Probe = Probe::new(0);
    /// The probe task's wakes; only the probe task wakes it.
    static HIGH: Probe = Probe::new(PERIOD_MS);
    /// The loop task's.
    static WINDOWS: Slot<Windows> = Slot::filled(Windows::new());

    struct Line(gaps::Record);

    impl core::fmt::Display for Line {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            gaps::describe(self.0, f)
        }
    }

    pub fn start() {
        // SAFETY: the step ignores its context and keeps its state in statics.
        let created =
            unsafe { wellfed_sdk::spawn(c"wellfed-gap", PROBE_STACK_WORDS, 4, gap_task) };
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
                super::log(format_args!("PLGAP1 {}\r\n", Line(record)));
            },
        );
    }

    unsafe extern "C" fn gap_task(_: *mut c_void) -> u32 {
        HIGH.wake(sdk::systimer_ticks());
        PERIOD_MS
    }
}
