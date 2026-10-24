//! The firmware's start hook and its loop task; the button service is the
//! other of its two tasks.

use core::ffi::c_void;

use wellfed_feeder::panel::{BUTTONS, Event, Source};
use wellfed_proto::api::{EventBody, Source as EventSource};
use wellfed_sdk::Slot;
use wellfed_sdk::hal::{panel, sdk, settings, startup};

use crate::api::{self, Api};
use crate::keys::{self, Identity};
use crate::owner::{self, Inputs, Owner};
use crate::setup;

wellfed_sdk::firmware!(start);

fn start() {
    // The measurement samples the ADC, so the power monitor stays off it.
    #[cfg(feature = "entropy")]
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
}

/// The step, a Noise operation and one formatted record; the build's stack
/// audit checks the bound. A handshake operation's X25519 is the deepest
/// path (FW44: 3820 bytes). The device API took the bound to 4,552 bytes,
/// just inside 6 KiB's margin (FW47), so the loop has 8 KiB: the feeding
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

struct ApiRecord(api::Record);

impl core::fmt::Display for ApiRecord {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        api::describe(self.0, f)
    }
}

unsafe extern "C" fn loop_task(_: *mut c_void) -> u32 {
    let (Some(mut owner), Some(mut api), Some(mut identity)) = (
        OWNER.enter(),
        API.enter(),
        IDENTITY.enter_or_init(Identity::default),
    ) else {
        return 0;
    };

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
    let api_delay = api.step(
        now_ms,
        owner.pairing(),
        identity,
        locked,
        recovery,
        |record| {
            log(format_args!("PLAPI1 {}\r\n", ApiRecord(record)));
        },
    );
    delay_ms = delay_ms.min(api_delay);

    #[cfg(feature = "entropy")]
    if let Some(mut capture) = CAPTURE.enter()
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
