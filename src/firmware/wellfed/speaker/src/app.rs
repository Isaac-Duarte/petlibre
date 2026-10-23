//! The firmware's start hook and its two tasks: tunes and the stream.

use core::ffi::c_void;

use wellfed_probes::speaker::{Audio, Stream};
use wellfed_sdk::Slot;
use wellfed_sdk::hal::{audio, sdk, stream};

wellfed_sdk::firmware!(start);

fn start() {
    // Neither task binds the KM0 flash handshake.
    wellfed_sdk::startup_done(true);

    start_audio();
    start_stream();
}

/// Touched only by the audio task; see `audio_task`.
static SPEAKER: Slot<audio::Speaker> = Slot::new();

/// The startup chime, then tunes the transport queues.
unsafe extern "C" fn audio_task(_: *mut c_void) -> u32 {
    // SAFETY: only this task touches the slot.
    let speaker = unsafe { SPEAKER.get_or_init(Default::default) };
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

/// Touched only by the stream task; see `stream_task`.
static STREAMER: Slot<stream::Streamer> = Slot::new();

/// Plays signed network audio streams.
unsafe extern "C" fn stream_task(_: *mut c_void) -> u32 {
    // SAFETY: only this task touches the slot.
    let streamer = unsafe { STREAMER.get_or_init(Default::default) };
    let step = streamer.step();
    if let Some(record) = step.record {
        wellfed_probes::log(format_args!("PLSPK1 {}\r\n", Stream(record)));
    }
    step.delay_ms
}

fn start_stream() {
    // SAFETY: the step ignores its context and keeps its state in statics.
    let created = unsafe { wellfed_sdk::spawn(c"wellfed-spk", 1024, 2, stream_task) };
    sdk::log(if created {
        c"PLSPK1 start created=1\r\n"
    } else {
        c"PLSPK1 start created=0\r\n"
    });
}
