//! The QA sequence: every bring-up check in one guided pass, ending in one
//! `PLQA1 result` line a bench audit can grade.
//!
//! Two unattended checks run first, then three guided ones:
//!
//! 1. **Optics**: the optics probe, whole (about 100 s). It holds the
//!    KM0 flash handshake, so the firmware reports `startup_done` only once
//!    it ends.
//! 2. **Power**: `POWER_SAMPLES` samples of the ADC probe, then its
//!    board closes. The values are reported, never judged: the divider and
//!    the limits are not established, so a pass means the probe opened,
//!    every sample converted and the outputs read back stopped.
//! 3. **Buttons**: the operator presses each button in panel order when
//!    prompted. The press must arrive on that button's pin (the bench's mapping:
//!    PB4..PB7, active LOW) within `ANSWER_MS`.
//! 4. **Indicators**: each indicator is driven HIGH with readback, and the
//!    operator answers whether it lit: Dispense yes, Lock no.
//! 5. **Speaker**: a known tune plays, and the operator answers the same way.
//!
//! The probes log their own records (`PLOPT1`, `PLADC1`, `PLBTN1`) so their
//! parsers keep working; QA adds its own `PLQA1` lines around them. The
//! sequence never drives the motor: the optics and power probes hold it at
//! STOP, and pressing Dispense only answers a prompt.

use core::fmt;

use petlibre_core::named_enum;
use petlibre_core::tone::Tune;
use petlibre_core::transport::PlayResult;
use petlibre_probes::{adc, buttons, optics};
use petlibre_sdk::hal::flash::park::Mmio;
use petlibre_sdk::hal::leds::{self, Led};
use petlibre_sdk::hal::{audio, sdk};

/// How long the operator has for each press or answer.
pub const ANSWER_MS: u32 = 30_000;
/// Power samples, one a second (`adc::SAMPLE_MS`).
pub const POWER_SAMPLES: u32 = 5;
/// Played with each button prompt so the operator need not watch the log.
pub const PROMPT_TUNE: Tune = Tune::Coin;
/// One second at 1 kHz: the output path, not a melody.
pub const SPEAKER_TUNE: Tune = Tune::Test;
/// Between queued `PLQA1` lines.
pub const LINE_MS: u32 = 2;

/// The answer buttons.
pub const YES: Button = Button::Dispense;
pub const NO: Button = Button::Lock;

named_enum! {
    /// A check, in sequence order (`stage=`).
    pub enum Check {
        Optics = "optics",
        Power = "power",
        Buttons = "buttons",
        Indicators = "indicators",
        Speaker = "speaker",
    }
}

named_enum! {
    /// A panel button, in panel order (`press=`).
    pub enum Button {
        Reset = "reset",
        Mic = "mic",
        Dispense = "dispense",
        Lock = "lock",
    }
}

impl Button {
    /// Its bit in the port B input word (`buttons::MASK`); the line idles
    /// HIGH and reads LOW while pressed.
    pub const fn bit(self) -> u32 {
        match self {
            Self::Reset => 1 << 4,
            Self::Mic => 1 << 5,
            Self::Dispense => 1 << 6,
            Self::Lock => 1 << 7,
        }
    }

    /// The one button pressed by a settled change, if exactly one went LOW.
    fn pressed(from: u32, to: u32) -> Option<Self> {
        let down = from & !to & buttons::MASK;
        Self::ALL.iter().copied().find(|b| b.bit() == down)
    }
}

const _: () = assert!(
    Button::Reset.bit() | Button::Mic.bit() | Button::Dispense.bit() | Button::Lock.bit()
        == buttons::MASK
);

named_enum! {
    /// Why an item failed (`reason=`).
    pub enum Fault {
        /// No press or answer within `ANSWER_MS`.
        Timeout = "timeout",
        /// Another button's press arrived first.
        Wrong = "wrong",
        /// The operator answered no.
        No = "no",
        /// The indicator request was refused (outputs not stopped).
        Refused = "refused",
        /// The indicator's settled readback did not accept HIGH.
        Unverified = "unverified",
        /// Sound is off in the settings.
        Muted = "muted",
        /// The speaker was busy or the settings were not loaded.
        Busy = "busy",
        /// The button lines did not open as released inputs.
        Lines = "lines",
    }
}

/// What a guided item tested.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Button(Button),
    Led(Led),
    Tune(Tune),
}

/// The checks' verdicts, `None` until each has run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Results([Option<bool>; 5]);

impl Results {
    pub fn get(&self, check: Check) -> Option<bool> {
        self.0[check as usize]
    }

    fn set(&mut self, check: Check, passed: bool) {
        self.0[check as usize] = Some(passed);
    }

    /// Every check ran and passed.
    pub fn passed(&self) -> bool {
        self.0.iter().all(|r| *r == Some(true))
    }
}

/// A `PLQA1` record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    /// A check begins.
    Start { check: Check },
    /// Press this button now.
    Press { button: Button },
    /// Is this indicator lit? Answer `YES` or `NO`.
    Look { led: Led },
    /// Did this tune play? Answer `YES` or `NO`.
    Listen { tune: Tune },
    /// One guided item's outcome.
    Item {
        check: Check,
        item: Item,
        fault: Option<Fault>,
    },
    /// A check's verdict.
    Finish { check: Check, passed: bool },
    /// Every verdict; the sequence's last line.
    Result(Results),
}

fn status(passed: bool) -> &'static str {
    if passed { "pass" } else { "fail" }
}

/// The body of a `PLQA1` line.
impl fmt::Display for Record {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Start { check } => write!(f, "start stage={check}"),
            Self::Press { button } => write!(f, "prompt press={button}"),
            Self::Look { led } => write!(f, "prompt look={} yes={YES} no={NO}", led.name()),
            Self::Listen { tune } => {
                write!(f, "prompt listen={} yes={YES} no={NO}", tune.name())
            }

            Self::Item { check, item, fault } => {
                write!(f, "item stage={check} ")?;
                match item {
                    Item::Button(button) => write!(f, "press={button}")?,
                    Item::Led(led) => write!(f, "led={}", led.name())?,
                    Item::Tune(tune) => write!(f, "tune={}", tune.name())?,
                }
                match fault {
                    None => f.write_str(" status=pass"),
                    Some(fault) => write!(f, " status=fail reason={fault}"),
                }
            }

            Self::Finish { check, passed } => {
                write!(f, "check stage={check} status={}", status(passed))
            }

            Self::Result(results) => {
                write!(f, "result status={}", status(results.passed()))?;
                for check in Check::ALL {
                    let verdict = results.get(*check).map_or("none", status);
                    write!(f, " {check}={verdict}")?;
                }
                Ok(())
            }
        }
    }
}

/// One line to log: a probe's record under its own tag, or QA's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line {
    Optics(optics::Record),
    Power(adc::Record),
    Button(buttons::Record),
    Qa(Record),
}

/// `delay_ms == 0` ends the sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub delay_ms: u32,
    pub line: Option<Line>,
}

fn next(delay_ms: u32, line: Option<Line>) -> Step {
    Step { delay_ms, line }
}

/// Whether an optics record leaves the run passing: it opened in full,
/// every window read stopped outputs, and the owner stall cut the emitters
/// and closed the supervisor with STOP and OFF verified.
fn optics_ok(record: optics::Record) -> bool {
    use optics::Record as R;

    match record {
        R::Open {
            board,
            ready,
            attached,
            events,
            inputs,
            ..
        } => board && ready && attached && events && inputs,
        R::Window(window) => window.stopped,
        R::Inputs(_) | R::Edge(_) | R::Done { .. } => true,
        R::Stall {
            cut_ms,
            off,
            stopped,
            closed,
            ..
        } => cut_ms.is_some() && off && stopped && closed,
        R::Timeout { .. } | R::Error { .. } => false,
    }
}

/// Whether a power record leaves the check passing.
fn power_ok(record: adc::Record) -> bool {
    match record {
        adc::Record::Open {
            outputs, clock, ok, ..
        } => outputs && ok && clock.verified(),
        adc::Record::Sample { stopped, .. } => stopped,
        adc::Record::Error { .. } => false,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Stage {
    #[default]
    Begin,
    Optics,
    Power,
    /// Opening the button lines.
    Lines,
    /// Waiting for `Button::ALL[index]`.
    Press {
        index: usize,
        since: u32,
    },
    /// Driving indicator `Led::ALL[index]` HIGH.
    Light {
        index: usize,
    },
    /// Reading back indicator `index` after `leds::SETTLE_MS`.
    Settle {
        index: usize,
    },
    /// Waiting for the answer about indicator `index`.
    Look {
        index: usize,
        since: u32,
    },
    /// Starting the speaker tune.
    Play,
    /// Waiting for the answer about the speaker tune.
    Listen {
        since: u32,
    },
    Done,
}

/// The pending `PLQA1` lines. The most any step queues is ten, when the
/// button lines fail to open: four items, then the last three checks'
/// verdicts with their starts and the result.
const QUEUE: usize = 10;

/// The probes the sequence steps. Each lives in fixed-address storage of
/// its own and is built in its own frame: built together inside the
/// sequence, the three temporaries shared one frame and left the task's
/// stack within the audit's margin.
pub struct Probes<'a> {
    pub optics: &'a mut optics::Probe,
    pub power: &'a mut adc::Probe,
    pub buttons: &'a mut buttons::Probe,
}

#[derive(Debug, Default)]
pub struct Qa {
    stage: Stage,
    results: Results,
    /// The check under way has failed an item or a probe record.
    failed: bool,
    /// The optics probe logged its last record.
    optics_complete: bool,
    optics_done: bool,
    power_samples: u32,
    queue: [Option<Record>; QUEUE],
}

impl Qa {
    /// The optics probe has ended, so nothing holds the KM0 flash handshake
    /// any more and the runtime may confirm the build.
    pub fn optics_done(&self) -> bool {
        self.optics_done
    }

    pub fn results(&self) -> Results {
        self.results
    }

    /// One task iteration in task context, always with the same probes.
    pub fn step<M: Mmio>(&mut self, probes: &mut Probes<'_>) -> Step {
        if let Some(record) = self.pop() {
            return next(LINE_MS, Some(Line::Qa(record)));
        }

        let now = sdk::uptime_ms();
        match self.stage {
            Stage::Begin => {
                self.stage = Stage::Optics;
                self.queued(Record::Start {
                    check: Check::Optics,
                })
            }
            Stage::Optics => self.optics::<M>(probes.optics),
            Stage::Power => self.power(probes.power),
            Stage::Lines => self.lines(probes.buttons, now),
            Stage::Press { index, since } => self.press(probes.buttons, index, since, now),
            Stage::Light { index } => self.light(index),
            Stage::Settle { index } => self.settle(index, now),
            Stage::Look { index, since } => self.look(probes.buttons, index, since, now),
            Stage::Play => self.play(now),
            Stage::Listen { since } => self.listen(probes.buttons, since, now),
            Stage::Done => next(0, None),
        }
    }

    fn push(&mut self, record: Record) {
        // No step queues more than `QUEUE` lines; were one to, the newest
        // line is dropped rather than overwriting an older one.
        if let Some(slot) = self.queue.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(record);
        }
    }

    fn pop(&mut self) -> Option<Record> {
        let record = self.queue[0].take()?;
        self.queue.rotate_left(1);
        Some(record)
    }

    /// Queues `record` and logs the oldest queued line now.
    fn queued(&mut self, record: Record) -> Step {
        self.push(record);
        next(LINE_MS, self.pop().map(Line::Qa))
    }

    /// Records the check's verdict and starts the next one.
    fn finish(&mut self, check: Check, then: Stage) {
        let passed = !self.failed;
        self.failed = false;
        self.results.set(check, passed);
        self.push(Record::Finish { check, passed });

        self.stage = then;
        match Check::ALL.get(check as usize + 1) {
            Some(&check) => self.push(Record::Start { check }),
            None => {
                self.push(Record::Result(self.results));
                self.stage = Stage::Done;
            }
        }
    }

    fn item(&mut self, check: Check, item: Item, fault: Option<Fault>) {
        self.failed |= fault.is_some();
        self.push(Record::Item { check, item, fault });
    }

    fn optics<M: Mmio>(&mut self, probe: &mut optics::Probe) -> Step {
        let step = probe.step::<M>();
        if let Some(record) = step.record {
            self.failed |= !optics_ok(record);
            self.optics_complete |= matches!(record, optics::Record::Done { .. });
        }

        if step.delay_ms == 0 {
            self.optics_done = true;
            self.failed |= !self.optics_complete;
            self.finish(Check::Optics, Stage::Power);
            return next(LINE_MS, step.record.map(Line::Optics));
        }
        next(step.delay_ms, step.record.map(Line::Optics))
    }

    fn power(&mut self, probe: &mut adc::Probe) -> Step {
        let step = probe.step();
        if let Some(record) = step.record {
            self.failed |= !power_ok(record);
            self.power_samples += u32::from(matches!(record, adc::Record::Sample { .. }));
        }

        if step.delay_ms == 0 || self.power_samples >= POWER_SAMPLES {
            probe.board.close();
            self.failed |= self.power_samples < POWER_SAMPLES;
            self.finish(Check::Power, Stage::Lines);
            return next(LINE_MS, step.record.map(Line::Power));
        }
        next(step.delay_ms, step.record.map(Line::Power))
    }

    /// Opens the button lines; all four must read back as released inputs,
    /// or no press could be told apart and the guided checks cannot run.
    fn lines(&mut self, probe: &mut buttons::Probe, now: u32) -> Step {
        let step = probe.step();
        let open = match step.record {
            Some(buttons::Record::Open {
                inputs,
                levels,
                stopped,
                ..
            }) => inputs && stopped && levels == buttons::MASK,
            _ => false,
        };

        if open {
            self.prompt(0, now);
        } else {
            for button in Button::ALL {
                self.item(Check::Buttons, Item::Button(*button), Some(Fault::Lines));
            }
            // Without the lines nothing can be answered either.
            self.finish(Check::Buttons, Stage::Done);
            self.failed = true;
            self.finish(Check::Indicators, Stage::Done);
            self.failed = true;
            self.finish(Check::Speaker, Stage::Done);
        }
        next(LINE_MS, step.record.map(Line::Button))
    }

    fn prompt(&mut self, index: usize, now: u32) {
        let Some(&button) = Button::ALL.get(index) else {
            self.finish(Check::Buttons, Stage::Light { index: 0 });
            return;
        };
        // A busy or muted speaker only loses the cue; the log still prompts.
        let _ = audio::request(PROMPT_TUNE);
        self.push(Record::Press { button });
        self.stage = Stage::Press { index, since: now };
    }

    /// Steps the button probe and reports the press that ends a wait.
    fn wait(probe: &mut buttons::Probe, since: u32, now: u32) -> (Step, Option<Option<Button>>) {
        let step = probe.step();
        let pressed = match step.record {
            Some(buttons::Record::Change {
                from,
                to,
                settled: true,
                ..
            }) => Button::pressed(from, to),
            _ => None,
        };
        let outcome = match pressed {
            Some(button) => Some(Some(button)),
            None if now.wrapping_sub(since) >= ANSWER_MS => Some(None),
            None => None,
        };
        (next(step.delay_ms, step.record.map(Line::Button)), outcome)
    }

    fn press(&mut self, probe: &mut buttons::Probe, index: usize, since: u32, now: u32) -> Step {
        let (step, outcome) = Self::wait(probe, since, now);
        let Some(pressed) = outcome else {
            return step;
        };

        let button = Button::ALL[index];
        let fault = match pressed {
            Some(got) if got == button => None,
            Some(_) => Some(Fault::Wrong),
            None => Some(Fault::Timeout),
        };
        self.item(Check::Buttons, Item::Button(button), fault);
        self.prompt(index + 1, now);
        step
    }

    fn light(&mut self, index: usize) -> Step {
        let Some(&led) = Led::ALL.get(index) else {
            self.finish(Check::Indicators, Stage::Play);
            return next(LINE_MS, None);
        };

        if leds::request(led, true) {
            self.stage = Stage::Settle { index };
            return next(leds::SETTLE_MS, None);
        }
        // A refused request has already driven every indicator LOW.
        self.item(Check::Indicators, Item::Led(led), Some(Fault::Refused));
        self.stage = Stage::Light { index: index + 1 };
        next(LINE_MS, None)
    }

    fn settle(&mut self, index: usize, now: u32) -> Step {
        let led = Led::ALL[index];
        if !leds::observe(led).accepted(true) {
            let _ = leds::request(led, false);
            self.item(Check::Indicators, Item::Led(led), Some(Fault::Unverified));
            self.stage = Stage::Light { index: index + 1 };
            return next(LINE_MS, None);
        }

        self.stage = Stage::Look { index, since: now };
        self.queued(Record::Look { led })
    }

    fn look(&mut self, probe: &mut buttons::Probe, index: usize, since: u32, now: u32) -> Step {
        let (step, outcome) = Self::wait(probe, since, now);
        let Some(answer) = answer(outcome) else {
            return step;
        };

        let led = Led::ALL[index];
        let _ = leds::request(led, false);
        self.item(Check::Indicators, Item::Led(led), answer);
        self.stage = Stage::Light { index: index + 1 };
        step
    }

    fn play(&mut self, now: u32) -> Step {
        let fault = match audio::request(SPEAKER_TUNE) {
            PlayResult::Started => None,
            PlayResult::Muted => Some(Fault::Muted),
            PlayResult::Busy => Some(Fault::Busy),
        };
        if let Some(fault) = fault {
            self.item(Check::Speaker, Item::Tune(SPEAKER_TUNE), Some(fault));
            self.finish(Check::Speaker, Stage::Done);
            return next(LINE_MS, None);
        }

        self.stage = Stage::Listen { since: now };
        self.queued(Record::Listen { tune: SPEAKER_TUNE })
    }

    fn listen(&mut self, probe: &mut buttons::Probe, since: u32, now: u32) -> Step {
        let (step, outcome) = Self::wait(probe, since, now);
        let Some(answer) = answer(outcome) else {
            return step;
        };

        self.item(Check::Speaker, Item::Tune(SPEAKER_TUNE), answer);
        self.finish(Check::Speaker, Stage::Done);
        step
    }
}

/// A yes/no wait's outcome as an item fault: `None` while still waiting,
/// `Some(None)` for yes. Buttons other than `YES` and `NO` are ignored.
fn answer(outcome: Option<Option<Button>>) -> Option<Option<Fault>> {
    match outcome? {
        Some(YES) => Some(None),
        Some(NO) => Some(Some(Fault::No)),
        Some(_) => None,
        None => Some(Some(Fault::Timeout)),
    }
}
