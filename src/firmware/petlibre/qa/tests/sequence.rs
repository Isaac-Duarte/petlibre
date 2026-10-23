//! The QA sequence against the fake SDK, with a scripted operator: checks
//! run in order, each prompt waits for its press or answer, wrong presses,
//! timeouts and "no" answers fail their item, and the motor never leaves
//! STOP.
//!
//! The KM0 peer is absent, so the optics probe always ends at its handshake
//! and the optics check fails; its own tests cover a full run.

use std::sync::{Mutex, MutexGuard, PoisonError};

use petlibre_core::transport::SettingsView;
use petlibre_fw_qa::sequence::{
    ANSWER_MS, Button, Check, Fault, Item, Line, NO, POWER_SAMPLES, Probes, Qa, Record, Results,
    YES,
};
use petlibre_probes::buttons::{self, MASK};
use petlibre_probes::{adc, optics};
use petlibre_sdk::hal::board::MOTOR_MASK;
use petlibre_sdk::hal::flash::park::Mmio;
use petlibre_sdk::hal::leds::{self, Led};
use petlibre_sdk::hal::sdk::fake;
use petlibre_sdk::hal::{audio, settings, stopped};

/// A KM0-side view of the IPC registers: `hello` refuses, as it would with
/// no KM4 authority to announce, so the optics probe ends at once.
struct Absent;

impl Mmio for Absent {
    const KM0: bool = true;
    fn load(_: u32) -> u32 {
        0
    }
    fn store(_: u32, _: u32) {}
    fn barrier() {}
    fn mask() -> u32 {
        0
    }
    fn halt() -> ! {
        panic!("halt")
    }
}

/// How long the scripted operator takes to react to a prompt, and then
/// holds the button.
const REACTION_MS: u32 = 300;
const HOLD_MS: u32 = 100;
// A cooperative operator always answers in time.
const _: () = assert!(REACTION_MS + HOLD_MS < ANSWER_MS);

/// The settings view and the audio mailbox are process-wide statics (the
/// fake GPIO model is per thread), so the tests take turns.
static SERIAL: Mutex<()> = Mutex::new(());

fn ready(sound: bool) -> (MutexGuard<'static, ()>, Qa) {
    let guard = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    fake::reset_hardware();
    fake::with(|h| {
        h.interrupts = 1;
        h.input[1] |= MASK;
        h.direction[1] |= MASK;
    });
    assert!(stopped::initialize().verified());
    assert!(leds::initialize());
    audio::reset();
    settings::publish(SettingsView {
        lights: true,
        sound,
        volume: 40,
        generation: 1,
        writable: true,
    });
    (guard, Qa::default())
}

fn hold(button: Button, down: bool) {
    fake::with(|h| {
        if down {
            h.input[1] &= !button.bit();
        } else {
            h.input[1] |= button.bit();
        }
    });
}

/// What the operator does for a prompt: the button to press, or nothing.
type Operator = dyn FnMut(Record) -> Option<Button>;

/// Runs the sequence to its end, answering prompts through `operator`, and
/// returns every line logged.
fn run(qa: &mut Qa, operator: &mut Operator) -> Vec<Line> {
    // Boxed so they stay at one address for the whole run, as the
    // firmware's statics do.
    let mut optics = Box::<optics::Probe>::default();
    let mut power = Box::<adc::Probe>::default();
    let mut buttons = Box::<buttons::Probe>::default();
    let mut lines = Vec::new();
    // The press the operator is about to make, and the one held down.
    let mut planned: Option<(Button, u32)> = None;
    let mut held: Option<(Button, u32)> = None;

    for _ in 0..1_000_000 {
        let step = qa.step::<Absent>(&mut Probes {
            optics: &mut optics,
            power: &mut power,
            buttons: &mut buttons,
        });
        if let Some(line) = step.line {
            lines.push(line);
        }
        if step.delay_ms == 0 {
            return lines;
        }
        let now = fake::with(|h| {
            h.uptime_ms += step.delay_ms;
            h.uptime_ms
        });

        if let Some((button, until)) = held
            && now >= until
        {
            hold(button, false);
            held = None;
        }
        if let Some((button, at)) = planned
            && now >= at
            && held.is_none()
        {
            hold(button, true);
            held = Some((button, now + HOLD_MS));
            planned = None;
        }

        let Some(Line::Qa(record)) = step.line else {
            continue;
        };
        if matches!(record, Record::Press { .. }) {
            // The cue tune would have played out by the next prompt.
            audio::reset();
        }
        if let Some(button) = operator(record) {
            planned = Some((button, now + REACTION_MS));
        }
    }
    panic!("the sequence never ended");
}

fn qa_records(lines: &[Line]) -> Vec<Record> {
    lines
        .iter()
        .filter_map(|line| match line {
            Line::Qa(record) => Some(*record),
            _ => None,
        })
        .collect()
}

fn faults(records: &[Record], check: Check) -> Vec<(Item, Option<Fault>)> {
    records
        .iter()
        .filter_map(|record| match *record {
            Record::Item {
                check: c,
                item,
                fault,
            } if c == check => Some((item, fault)),
            _ => None,
        })
        .collect()
}

/// Presses each prompted button and answers yes.
fn cooperative(record: Record) -> Option<Button> {
    match record {
        Record::Press { button } => Some(button),
        Record::Look { .. } | Record::Listen { .. } => Some(YES),
        _ => None,
    }
}

fn result(results: Results) -> String {
    Record::Result(results).to_string()
}

#[test]
fn runs_every_check_in_order_and_ends_with_the_result() {
    let (_serial, mut qa) = ready(true);

    let lines = run(&mut qa, &mut cooperative);

    let records = qa_records(&lines);
    let starts: Vec<Check> = records
        .iter()
        .filter_map(|record| match record {
            Record::Start { check } => Some(*check),
            _ => None,
        })
        .collect();
    assert_eq!(starts, Check::ALL);
    assert!(matches!(lines.get(1), Some(Line::Optics(_))));
    assert!(qa.optics_done());

    let samples = lines
        .iter()
        .filter(|line| matches!(line, Line::Power(adc::Record::Sample { .. })))
        .count();
    assert_eq!(samples, POWER_SAMPLES as usize);

    assert_eq!(
        faults(&records, Check::Buttons),
        Button::ALL
            .iter()
            .map(|b| (Item::Button(*b), None))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        faults(&records, Check::Indicators),
        Led::ALL
            .iter()
            .map(|led| (Item::Led(*led), None))
            .collect::<Vec<_>>()
    );

    let results = qa.results();
    assert_eq!(records.last(), Some(&Record::Result(results)));
    assert_eq!(
        result(results),
        "result status=fail optics=fail power=pass buttons=pass indicators=pass speaker=pass"
    );

    // Indicators back LOW and the motor at STOP throughout.
    assert!(leds::all_low());
    assert_eq!(fake::with(|h| h.output[0]) & MOTOR_MASK, MOTOR_MASK);
}

#[test]
fn a_wrong_press_or_none_fails_that_button() {
    let (_serial, mut qa) = ready(true);

    let lines = run(&mut qa, &mut |record| match record {
        Record::Press {
            button: Button::Reset,
        } => Some(Button::Mic),
        Record::Press {
            button: Button::Mic,
        } => None,
        other => cooperative(other),
    });

    let records = qa_records(&lines);
    assert_eq!(
        faults(&records, Check::Buttons),
        vec![
            (Item::Button(Button::Reset), Some(Fault::Wrong)),
            (Item::Button(Button::Mic), Some(Fault::Timeout)),
            (Item::Button(Button::Dispense), None),
            (Item::Button(Button::Lock), None),
        ]
    );
    assert_eq!(qa.results().get(Check::Buttons), Some(false));
    assert_eq!(qa.results().get(Check::Indicators), Some(true));
}

#[test]
fn a_no_answer_fails_the_indicator_or_speaker() {
    let (_serial, mut qa) = ready(true);

    let lines = run(&mut qa, &mut |record| match record {
        Record::Look { led: Led::Food } | Record::Listen { .. } => Some(NO),
        // Neither yes nor no: ignored, so the next yes still answers.
        Record::Look { led: Led::Lock } => Some(Button::Reset),
        other => cooperative(other),
    });

    let records = qa_records(&lines);
    let indicators = faults(&records, Check::Indicators);
    assert_eq!(indicators[1], (Item::Led(Led::Food), Some(Fault::No)));
    assert_eq!(indicators[2], (Item::Led(Led::Lock), Some(Fault::Timeout)));
    assert_eq!(qa.results().get(Check::Indicators), Some(false));
    assert_eq!(qa.results().get(Check::Speaker), Some(false));
    assert!(leds::all_low());
}

#[test]
fn a_muted_speaker_fails_without_a_prompt() {
    let (_serial, mut qa) = ready(false);

    let lines = run(&mut qa, &mut cooperative);

    let records = qa_records(&lines);
    assert!(!records.iter().any(|r| matches!(r, Record::Listen { .. })));
    assert_eq!(
        faults(&records, Check::Speaker),
        vec![(
            Item::Tune(petlibre_fw_qa::sequence::SPEAKER_TUNE),
            Some(Fault::Muted)
        )]
    );
}

#[test]
fn a_held_button_at_open_fails_every_guided_check() {
    let (_serial, mut qa) = ready(true);
    hold(Button::Lock, true);

    let lines = run(&mut qa, &mut |_| None);

    let records = qa_records(&lines);
    assert!(
        faults(&records, Check::Buttons)
            .iter()
            .all(|(_, fault)| *fault == Some(Fault::Lines))
    );
    assert_eq!(
        result(qa.results()),
        "result status=fail optics=fail power=pass buttons=fail indicators=fail speaker=fail"
    );
}

#[test]
fn records_print_as_plqa1_bodies() {
    let cases = [
        (
            Record::Start {
                check: Check::Buttons,
            },
            "start stage=buttons",
        ),
        (
            Record::Press {
                button: Button::Mic,
            },
            "prompt press=mic",
        ),
        (
            Record::Look { led: Led::Fault },
            "prompt look=PB22 yes=dispense no=lock",
        ),
        (
            Record::Item {
                check: Check::Buttons,
                item: Item::Button(Button::Lock),
                fault: Some(Fault::Timeout),
            },
            "item stage=buttons press=lock status=fail reason=timeout",
        ),
        (
            Record::Item {
                check: Check::Indicators,
                item: Item::Led(Led::Food),
                fault: None,
            },
            "item stage=indicators led=PA2 status=pass",
        ),
        (
            Record::Finish {
                check: Check::Power,
                passed: true,
            },
            "check stage=power status=pass",
        ),
        (
            Record::Result(Results::default()),
            "result status=fail optics=none power=none buttons=none indicators=none speaker=none",
        ),
    ];

    for (record, text) in cases {
        assert_eq!(record.to_string(), text);
    }
}
