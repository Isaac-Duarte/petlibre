//! The `speaker` variant's start hook and its one task: the chime and tunes.

use wellfed_probes::speaker::Audio;
use wellfed_sdk::hal::{audio, sdk};
use wellfed_sdk::{Slot, Startup, log};

#[wellfed_sdk::start(runtime = wellfed_feeder_runtime)]
fn start() -> Startup {
    let created = wellfed_sdk::spawn(audio_task);
    sdk::log(if created {
        c"PLAUD1 start created=1\r\n"
    } else {
        c"PLAUD1 start created=0\r\n"
    });

    // The task does not bind the KM0 flash handshake.
    Startup::Done
}

/// Touched only by the audio task; see `audio_task`.
static SPEAKER: Slot<audio::Speaker> = Slot::new();

/// The startup chime, then any tune queued with `audio::request`. Below the
/// feeder: a late page is an audible gap, never a missed lease.
#[wellfed_sdk::task(name = "wellfed-aud", stack = 1024, priority = 2)]
fn audio_task() -> u32 {
    let Some(mut speaker) = SPEAKER.enter_or_init(Default::default) else {
        return 0;
    };

    let step = speaker.step();
    if let Some(record) = step.record {
        log(format_args!("PLAUD1 {}\r\n", Audio(record)));
    }
    step.delay_ms
}
