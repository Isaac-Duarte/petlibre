//! The reference firmware: the diagnostic image's optional tasks on the
//! PetLibre SDK runtime. In the normal role it adds the FW13 optics probe
//! (which exercises the emitters and reads the food, outlet and feedback
//! inputs, motor STOP throughout) and the FW24 startup chime; the speaker
//! variant adds the signed network audio stream, and the FW16 and FW11
//! variants replace the optics probe with a motor pulse or a trial reset.
//!
//! Everything else, from startup to confirmation, updates and recovery, is
//! the runtime's (`petlibre_sdk`).

use core::ffi::c_void;

use petlibre_hal::flash::park::Hardware;
use petlibre_hal::{audio, sdk, startup};
use petlibre_sdk::Slot;

use crate::probes::optics;

petlibre_sdk::firmware!(start);

/// The firmware's tasks, in the normal role.
fn start() {
    start_optics();
    start_audio();
    start_stream();
}

/// Touched only by the optics task; see `optics_task`.
static OPTICS: Slot<optics::Probe> = Slot::new();

struct OpticsLine(optics::Record);

impl core::fmt::Display for OpticsLine {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        optics::describe(self.0, f)
    }
}

unsafe extern "C" fn optics_task(_: *mut c_void) -> u32 {
    // SAFETY: only this task touches the slot; the probe stays in place.
    let probe = unsafe { OPTICS.get_or_init(optics::Probe::default) };
    let step = probe.step::<Hardware>();
    if let Some(record) = step.record {
        let mut report = startup::Report::new();
        if let Ok(line) = report.record(format_args!("PLOPT1 {}\r\n", OpticsLine(record))) {
            sdk::log(line);
        }
    }
    petlibre_sdk::startup_done(step.delay_ms == 0);
    step.delay_ms
}

/// The optics and pulse probes' stacks. Building a probe in place copies it
/// through two constructor frames (about 3.6 KiB), so 4 KiB overflowed once
/// LTO inlined differently after the crate split.
const PROBE_STACK_WORDS: u16 = 2048;

/// Touched only by the pulse task; see `pulse_task`.
#[cfg(feature = "fw16-pulse")]
static PULSE: Slot<crate::probes::pulse::Probe> = Slot::new();

#[cfg(feature = "fw16-pulse")]
struct PulseLine(crate::probes::pulse::Record);

#[cfg(feature = "fw16-pulse")]
impl core::fmt::Display for PulseLine {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        crate::probes::pulse::describe(self.0, f)
    }
}

#[cfg(feature = "fw16-pulse")]
unsafe extern "C" fn pulse_task(_: *mut c_void) -> u32 {
    // SAFETY: only this task touches the slot; the probe stays in place.
    let probe = unsafe { PULSE.get_or_init(Default::default) };
    let step = probe.step::<Hardware>();
    if let Some(record) = step.record {
        let mut report = startup::Report::new();
        if let Ok(line) = report.record(format_args!("PLMOT1 {}\r\n", PulseLine(record))) {
            sdk::log(line);
        }
    }
    petlibre_sdk::startup_done(step.delay_ms == 0);
    step.delay_ms
}

/// FW16 builds run the motor pulse in place of the optics probe.
#[cfg(feature = "fw16-pulse")]
fn start_optics() {
    // SAFETY: the step ignores its context and keeps its state in statics.
    let created =
        unsafe { petlibre_sdk::spawn(c"petlibre-pulse", PROBE_STACK_WORDS, 3, pulse_task) };
    sdk::log(if created {
        c"PLMOT1 start created=1\r\n"
    } else {
        c"PLMOT1 start created=0\r\n"
    });
}

/// Above the diagnostic and transport tasks, so their logging cannot hold
/// off the owner lease the supervisor enforces.
#[cfg(not(any(feature = "fw16-pulse", feature = "fw11-reset")))]
fn start_optics() {
    // SAFETY: the step ignores its context and keeps its state in statics.
    let created =
        unsafe { petlibre_sdk::spawn(c"petlibre-optics", PROBE_STACK_WORDS, 3, optics_task) };
    sdk::log(if created {
        c"PLOPT1 start created=1 plan=2\r\n"
    } else {
        c"PLOPT1 start created=0 plan=2\r\n"
    });
    const _: () = assert!(optics::PLAN == 2);
}
/// Touched only by the audio task; see `audio_task`.
static SPEAKER: Slot<audio::Speaker> = Slot::new();

struct AudioLine(audio::Record);

impl core::fmt::Display for AudioLine {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        audio::describe(self.0, f)
    }
}

/// FW24: the startup chime, then tunes the transport queues.
unsafe extern "C" fn audio_task(_: *mut c_void) -> u32 {
    // SAFETY: only this task touches the slot.
    let speaker = unsafe { SPEAKER.get_or_init(Default::default) };
    let step = speaker.step();
    if let Some(record) = step.record {
        let mut report = startup::Report::new();
        if let Ok(line) = report.record(format_args!("PLAUD1 {}\r\n", AudioLine(record))) {
            sdk::log(line);
        }
    }
    step.delay_ms
}

/// Speaker variant: touched only by the stream task; see `stream_task`.
#[cfg(feature = "speaker")]
static STREAMER: Slot<petlibre_hal::stream::Streamer> = Slot::new();

#[cfg(feature = "speaker")]
struct StreamLine(petlibre_hal::stream::Record);

#[cfg(feature = "speaker")]
impl core::fmt::Display for StreamLine {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        petlibre_hal::stream::describe(self.0, f)
    }
}

/// Speaker variant: plays signed network audio streams.
#[cfg(feature = "speaker")]
unsafe extern "C" fn stream_task(_: *mut c_void) -> u32 {
    // SAFETY: only this task touches the slot.
    let streamer = unsafe { STREAMER.get_or_init(Default::default) };
    let step = streamer.step();
    if let Some(record) = step.record {
        let mut report = startup::Report::new();
        if let Ok(line) = report.record(format_args!("PLSPK1 {}\r\n", StreamLine(record))) {
            sdk::log(line);
        }
    }
    step.delay_ms
}

#[cfg(feature = "speaker")]
fn start_stream() {
    // SAFETY: the step ignores its context and keeps its state in statics.
    let created = unsafe { petlibre_sdk::spawn(c"petlibre-spk", 1024, 2, stream_task) };
    sdk::log(if created {
        c"PLSPK1 start created=1\r\n"
    } else {
        c"PLSPK1 start created=0\r\n"
    });
}

#[cfg(not(feature = "speaker"))]
fn start_stream() {}

/// Below the feeder and the optics probe: a late page is an audible gap,
/// never a missed lease.
fn start_audio() {
    // SAFETY: the step ignores its context and keeps its state in statics.
    let created = unsafe { petlibre_sdk::spawn(c"petlibre-aud", 1024, 2, audio_task) };
    sdk::log(if created {
        c"PLAUD1 start created=1\r\n"
    } else {
        c"PLAUD1 start created=0\r\n"
    });
}
/// FW11 trial test, in place of the optics probe: stop feeding the
/// watchdog, as a hung candidate would, so KM0 counts the reset. Outputs
/// stay stopped.
#[cfg(feature = "fw11-reset")]
fn start_optics() {
    petlibre_sdk::request_reset();
    sdk::log(if petlibre_sdk::watchdog_open() {
        c"PLTRL1 test=reset watchdog=1\r\n"
    } else {
        c"PLTRL1 test=reset watchdog=0\r\n"
    });
}
