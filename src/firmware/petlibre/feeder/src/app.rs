//! The firmware's start hook and its loop task; the button service is the
//! other of its two tasks.

use core::ffi::c_void;

use petlibre_feeder::panel::BUTTONS;
use petlibre_sdk::Slot;
use petlibre_sdk::hal::{panel, sdk, settings, startup};

use crate::budget::{self, Budget};
use crate::keys::{self, Identity};
use crate::owner::{self, Inputs, Owner};
use crate::setup;

petlibre_sdk::firmware!(start);

fn start() {
    // The measurement samples the ADC, so the power monitor stays off it.
    #[cfg(feature = "entropy")]
    petlibre_sdk::claim_adc();

    // Nothing here holds the KM0 flash handshake, so the runtime may confirm
    // once it is otherwise healthy.
    petlibre_sdk::startup_done(true);

    // SAFETY: the step ignores its context and keeps its state in statics.
    let created = unsafe { petlibre_sdk::spawn(c"petlibre-loop", LOOP_STACK_WORDS, 2, loop_task) };
    sdk::log(if created {
        c"PLPANEL1 task created=1\r\n"
    } else {
        c"PLPANEL1 task created=0\r\n"
    });
    petlibre_sdk::buttons::start(BUTTONS);
}

/// The step, a Noise operation and one formatted record; the build's stack
/// audit checks the bound. A handshake operation's X25519 is the deepest
/// path (FW44: 3820 bytes), so 4 KiB would not keep the audit's margin.
const LOOP_STACK_WORDS: u16 = 1536;

/// Touched only by the loop task; see `loop_task`.
static OWNER: Slot<Owner> = Slot::new();
/// Touched only by the loop task.
static IDENTITY: Slot<Identity> = Slot::new();
/// The session table is larger than the loop's stack, so it is built at
/// compile time. Touched only by the loop task.
static BUDGET: Slot<Budget> = Slot::filled(Budget::new());
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

struct Api(budget::Record);

impl core::fmt::Display for Api {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        budget::describe(self.0, f)
    }
}

unsafe extern "C" fn loop_task(_: *mut c_void) -> u32 {
    // SAFETY: only this task touches the slots.
    let (owner, Some(budget)) = (unsafe { OWNER.get_or_init(Owner::default) }, unsafe {
        BUDGET.get()
    }) else {
        return 0;
    };

    // SAFETY: only this task touches the slot.
    let identity = unsafe { IDENTITY.get_or_init(Identity::default) };
    if let Some(record) = identity.step() {
        log(format_args!("PLID1 {}\r\n", Key(record)));
    }

    let now_ms = sdk::uptime_ms();
    let inputs = Inputs {
        now_ms,
        gestures: core::iter::from_fn(petlibre_sdk::buttons::next),
        remote: panel::take(),
        lights: settings::current().is_none_or(|view| view.lights),
        identity: identity.keypair(),
    };
    let mut delay_ms = owner.step(inputs, |record| match record {
        owner::Record::Setup(setup::Record::Ble(_)) => {
            log(format_args!("PLBLE1 {}\r\n", Panel(record)));
        }
        owner::Record::Setup(_) => log(format_args!("PLSET1 {}\r\n", Panel(record))),
        _ => log(format_args!("PLPANEL1 {}\r\n", Panel(record))),
    });

    if owner.reboot_due(now_ms) {
        petlibre_sdk::request_reset();
    }

    // Requests read what the owner holds after this step.
    if let Some(lock) = owner.lock() {
        panel::publish(lock.locked());
    }

    // A timed operation ran: come straight back for the next one.
    if budget.step(now_ms, |record| {
        log(format_args!("PLAPI1 {}\r\n", Api(record)))
    }) {
        delay_ms = 1;
    }

    #[cfg(feature = "entropy")]
    // SAFETY: only this task touches the slot.
    if let Some(capture) = unsafe { CAPTURE.get() }
        && let Some(delay) = capture.step(|record| {
            log(format_args!("PLRNG1 {}\r\n", Rng(record)));
        })
    {
        delay_ms = delay_ms.min(delay);
    }

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
