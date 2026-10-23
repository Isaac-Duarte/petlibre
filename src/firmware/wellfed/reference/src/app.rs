//! The reference firmware: the diagnostic image's optional tasks on the
//! Wellfed SDK runtime. In the normal role it adds the FW13 optics probe
//! (which exercises the emitters and reads the food, outlet and feedback
//! inputs, motor STOP throughout) and the FW24 startup chime; the speaker
//! variant adds the signed network audio stream, and the FW16, FW14, FW20
//! and FW11 variants replace the optics probe with a motor pulse, the
//! read-only power-input or button probe, or a trial reset.
//!
//! Everything else, from startup to confirmation, updates and recovery, is
//! the runtime's (`wellfed_sdk`).

use core::ffi::c_void;

use wellfed_hal::flash::park::Hardware;
use wellfed_hal::{audio, sdk, startup};
use wellfed_sdk::Slot;

use crate::probes::optics;

wellfed_sdk::firmware!(start);

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
    wellfed_sdk::startup_done(step.delay_ms == 0);
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
    wellfed_sdk::startup_done(step.delay_ms == 0);
    step.delay_ms
}

/// FW16 builds run the motor pulse in place of the optics probe.
#[cfg(feature = "fw16-pulse")]
fn start_optics() {
    // SAFETY: the step ignores its context and keeps its state in statics.
    let created =
        unsafe { wellfed_sdk::spawn(c"wellfed-pulse", PROBE_STACK_WORDS, 3, pulse_task) };
    sdk::log(if created {
        c"PLMOT1 start created=1\r\n"
    } else {
        c"PLMOT1 start created=0\r\n"
    });
}

/// Above the diagnostic and transport tasks, so their logging cannot hold
/// off the owner lease the supervisor enforces.
#[cfg(not(any(
    feature = "fw16-pulse",
    feature = "fw14-adc",
    feature = "fw20-buttons",
    feature = "fw11-reset"
)))]
fn start_optics() {
    // SAFETY: the step ignores its context and keeps its state in statics.
    let created =
        unsafe { wellfed_sdk::spawn(c"wellfed-optics", PROBE_STACK_WORDS, 3, optics_task) };
    sdk::log(if created {
        c"PLOPT1 start created=1 plan=2\r\n"
    } else {
        c"PLOPT1 start created=0 plan=2\r\n"
    });
    const _: () = assert!(optics::PLAN == 2);
}

/// Touched only by the ADC task; see `adc_task`.
#[cfg(feature = "fw14-adc")]
static ADC: Slot<crate::probes::adc::Probe> = Slot::new();

#[cfg(feature = "fw14-adc")]
fn log_adc(record: crate::probes::adc::Record) {
    use crate::probes::adc::{Efuse, Record, Values};

    let bit = u8::from;
    let pass = |ok: bool| if ok { "pass" } else { "fail" };
    let mut report = startup::Report::new();
    let line = match record {
        Record::Open {
            outputs,
            clock,
            efuse,
            calibration,
            ok,
        } => {
            // Two lines: one record exceeds the report buffer.
            if let Ok(line) = report.record(format_args!(
                "PLADC1 clock function_before={:08x} clock_before={:08x} function_after={:08x} clock_after={:08x} status={}\r\n",
                clock.function_before,
                clock.clock_before,
                clock.function_after,
                clock.clock_after,
                pass(clock.verified()),
            )) {
                sdk::log(line);
            }
            report.record(format_args!(
                "PLADC1 open outputs={} efuse={} offset={} gain={} default_offset={} default_gain={} status={}\r\n",
                bit(outputs),
                Efuse(efuse),
                calibration.offset,
                calibration.gain,
                bit(calibration.default_offset),
                bit(calibration.default_gain),
                pass(ok),
            ))
        }

        Record::Sample {
            n,
            ticks,
            food,
            outlet,
            dc,
            stopped,
            current,
            battery,
            supply,
        } => report.record(format_args!(
            "PLADC1 sample n={n} ticks={ticks} food={} outlet={} dc={} stopped={} current={} battery={} supply={}\r\n",
            bit(food),
            bit(outlet),
            bit(dc),
            bit(stopped),
            Values(current),
            Values(battery),
            Values(supply),
        )),

        Record::Error { n, ticks, stopped } => report.record(format_args!(
            "PLADC1 error n={n} ticks={ticks} stopped={}\r\n",
            bit(stopped)
        )),
    };
    if let Ok(line) = line {
        sdk::log(line);
    }
}

#[cfg(feature = "fw14-adc")]
unsafe extern "C" fn adc_task(_: *mut c_void) -> u32 {
    // SAFETY: only this task touches the slot; the probe stays in place.
    let probe = unsafe { ADC.get_or_init(Default::default) };
    let step = probe.step();
    if let Some(record) = step.record {
        log_adc(record);
    }
    step.delay_ms
}

/// FW14 builds sample the power inputs in place of the optics probe, for
/// as long as the build runs. The probe never binds the KM0 flash
/// handshake, so confirmation need not wait for it.
#[cfg(feature = "fw14-adc")]
fn start_optics() {
    wellfed_sdk::startup_done(true);

    // SAFETY: the step ignores its context and keeps its state in statics.
    let created = unsafe { wellfed_sdk::spawn(c"wellfed-adc", PROBE_STACK_WORDS, 3, adc_task) };
    // Only on failure: every PLADC1 line the audit sees must be a probe
    // record, so this one fails it.
    if !created {
        sdk::log(c"PLADC1 start created=0\r\n");
    }
}
/// Touched only by the button task; see `button_task`.
#[cfg(feature = "fw20-buttons")]
static BUTTONS: Slot<crate::probes::buttons::Probe> = Slot::new();

#[cfg(feature = "fw20-buttons")]
struct ButtonLine(crate::probes::buttons::Record);

#[cfg(feature = "fw20-buttons")]
impl core::fmt::Display for ButtonLine {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        crate::probes::buttons::describe(self.0, f)
    }
}

#[cfg(feature = "fw20-buttons")]
unsafe extern "C" fn button_task(_: *mut c_void) -> u32 {
    // SAFETY: only this task touches the slot.
    let probe = unsafe { BUTTONS.get_or_init(Default::default) };
    let step = probe.step();
    if let Some(record) = step.record {
        let mut report = startup::Report::new();
        if let Ok(line) = report.record(format_args!("PLBTN1 {}\r\n", ButtonLine(record))) {
            sdk::log(line);
        }
    }
    step.delay_ms
}

/// FW20 builds read the panel buttons in place of the optics probe. Like
/// the ADC probe, it never binds the KM0 flash handshake.
#[cfg(feature = "fw20-buttons")]
fn start_optics() {
    wellfed_sdk::startup_done(true);

    // SAFETY: the step ignores its context and keeps its state in statics.
    let created = unsafe { wellfed_sdk::spawn(c"wellfed-btn", 1024, 3, button_task) };
    sdk::log(if created {
        c"PLBTN1 start created=1\r\n"
    } else {
        c"PLBTN1 start created=0\r\n"
    });
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
static STREAMER: Slot<wellfed_hal::stream::Streamer> = Slot::new();

#[cfg(feature = "speaker")]
struct StreamLine(wellfed_hal::stream::Record);

#[cfg(feature = "speaker")]
impl core::fmt::Display for StreamLine {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        wellfed_hal::stream::describe(self.0, f)
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
    let created = unsafe { wellfed_sdk::spawn(c"wellfed-spk", 1024, 2, stream_task) };
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
    let created = unsafe { wellfed_sdk::spawn(c"wellfed-aud", 1024, 2, audio_task) };
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
    wellfed_sdk::request_reset();
    sdk::log(if wellfed_sdk::watchdog_open() {
        c"PLTRL1 test=reset watchdog=1\r\n"
    } else {
        c"PLTRL1 test=reset watchdog=0\r\n"
    });
}
