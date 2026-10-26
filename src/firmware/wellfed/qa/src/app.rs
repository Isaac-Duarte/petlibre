//! The firmware's start hook and its two tasks: the QA sequence and the
//! speaker that plays its chime, cues and test tune.

use core::ffi::c_void;

use wellfed_probes::speaker::Audio;
use wellfed_probes::{adc, buttons, optics};
use wellfed_sdk::Slot;
use wellfed_sdk::hal::flash::park::Hardware;
use wellfed_sdk::hal::{audio, sdk};

use crate::sequence::{Line, Probes, Qa};

wellfed_sdk::firmware!(start);

fn start() {
    // The power check samples the ADC after optics has reported
    // `startup_done`, so the runtime's power monitor must stay off it.
    wellfed_sdk::claim_adc();
    start_qa();
    start_audio();
}

/// The QA task hosts the optics probe, whose in-place construction copies
/// it through two constructor frames (about 3.6 KiB).
const QA_STACK_WORDS: u16 = 2048;

// Touched only by the QA task; see `qa_task`. One slot per probe, so each
// is built in a frame of its own.
static QA: Slot<Qa> = Slot::new();
static OPTICS: Slot<optics::Probe> = Slot::new();
static POWER: Slot<adc::Probe> = Slot::new();
static BUTTONS: Slot<buttons::Probe> = Slot::new();

unsafe extern "C" fn qa_task(_: *mut c_void) -> u32 {
    let (Some(mut qa), Some(mut optics), Some(mut power), Some(mut buttons)) = (
        QA.enter_or_init(Qa::default),
        OPTICS.enter_or_init(optics::Probe::default),
        POWER.enter_or_init(adc::Probe::default),
        BUTTONS.enter_or_init(buttons::Probe::default),
    ) else {
        return 0;
    };
    let mut probes = Probes {
        optics: &mut optics,
        power: &mut power,
        buttons: &mut buttons,
    };
    let step = qa.step::<Hardware>(&mut probes);
    match step.line {
        Some(Line::Optics(record)) => wellfed_probes::log(format_args!("PLOPT1 {record}\r\n")),
        Some(Line::Power(record)) => adc::log(record),
        Some(Line::Button(record)) => wellfed_probes::log(format_args!("PLBTN1 {record}\r\n")),
        Some(Line::Qa(record)) => wellfed_probes::log(format_args!("PLQA1 {record}\r\n")),
        None => {}
    }
    wellfed_sdk::startup_done(qa.optics_done());
    step.delay_ms
}

/// Above the diagnostic tasks, so their logging cannot hold
/// off the owner lease the supervisor enforces while optics runs.
fn start_qa() {
    // SAFETY: the step ignores its context and keeps its state in statics.
    let created = unsafe { wellfed_sdk::spawn(c"wellfed-qa", QA_STACK_WORDS, 3, qa_task) };
    // The optics start record carries the plan the startup audit binds to
    // `fw13-optics-v2`.
    sdk::log(if created {
        c"PLOPT1 start created=1 plan=2\r\n"
    } else {
        c"PLOPT1 start created=0 plan=2\r\n"
    });
    const _: () = assert!(optics::PLAN == 2);
}

/// Touched only by the audio task; see `audio_task`.
static SPEAKER: Slot<audio::Speaker> = Slot::new();

/// The startup chime, then the tunes QA queues.
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

/// Below the QA task: a late page is an audible gap, never a missed lease.
fn start_audio() {
    // SAFETY: the step ignores its context and keeps its state in statics.
    let created = unsafe { wellfed_sdk::spawn(c"wellfed-aud", 1024, 2, audio_task) };
    sdk::log(if created {
        c"PLAUD1 start created=1\r\n"
    } else {
        c"PLAUD1 start created=0\r\n"
    });
}
