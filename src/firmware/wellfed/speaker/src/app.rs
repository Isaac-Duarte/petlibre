//! The firmware's start hook and its one task: the chime and tunes.

use core::ffi::c_void;

use wellfed_probes::speaker::Audio;
use wellfed_sdk::Slot;
use wellfed_sdk::hal::{audio, sdk};

wellfed_sdk::firmware!(start);

fn start() {
    // The task does not bind the KM0 flash handshake.
    wellfed_sdk::startup_done(true);

    start_audio();
}

/// Touched only by the audio task; see `audio_task`.
static SPEAKER: Slot<audio::Speaker> = Slot::new();

/// The startup chime, then any tune queued with `audio::request`.
unsafe extern "C" fn audio_task(_: *mut c_void) -> u32 {
    let Some(mut speaker) = SPEAKER.enter_or_init(Default::default) else {
        return 0;
    };
    let step = speaker.step();
    if let Some(record) = step.record {
        wellfed_probes::log(format_args!("PLAUD1 {}\r\n", Audio(record)));
    }
    step.delay_ms
}

/// Below the feeder: a late page is an audible gap, never a missed lease.
fn start_audio() {
    // SAFETY: the step ignores its context and keeps its state in statics.
    let created = unsafe { wellfed_sdk::spawn(c"wellfed-aud", 1024, 2, audio_task) };
    sdk::log(if created {
        c"PLAUD1 start created=1\r\n"
    } else {
        c"PLAUD1 start created=0\r\n"
    });
}
