//! A feed's trace as the loop records it: one [`Observation`] per owner
//! poll goes into a body ([`wellfed_proto::trace`]) of at most
//! [`BODY_MAX`] bytes, and the feed's result and the owner's own drive
//! figures seal it with its [`TraceSummary`].
//!
//! The recorder runs between the owner's steps, on what the step left
//! behind, so it never delays a stop or a motor decision; it costs a few
//! dozen instructions a poll, and when the body fills it halves the body's
//! current readings in place (one pass over under a kilobyte) and keeps
//! every second poll's reading from then on. Marks, beams, battery and
//! motor changes are always kept until the body has room for nothing but
//! them, when it ends there (`truncated`). The summary's figures never
//! depend on the body: the peak, mean, drive time and edge counts are the
//! owner's own, the same that `PLOWN1 fed` logs.
//!
//! Nothing is written to flash here: a sealed trace waits in RAM until the
//! settings writer copies it into the storage ring ([`crate::store::traces`])
//! once it has the write turn, which a feed never leaves it while the
//! motor may move.

use wellfed_core::HALF_RANGE;
use wellfed_core::feed::controller::MotorCommand;
use wellfed_core::feed::observe::{Beams, Observation};
use wellfed_proto::api::{CurrentUnit, EventBody, Refusal, TraceSummary};
use wellfed_proto::trace::{BODY_MAX, Entry, Motor, Pushed, Writer};

use crate::feeder::platform::TraceRead;

/// A battery reading this far from the last one kept is kept too; nearer
/// ones are the ADC's own wander.
pub const BATTERY_STEP_MV: u32 = 20;

/// What the feeding owner measured over the whole drive, as it logs it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DriveFigures {
    pub drive_ms: u32,
    pub samples: u32,
    pub current_peak: u32,
    pub current_mean: Option<u32>,
    pub rising: u16,
    pub falling: u16,
    pub first_edge_ms: Option<u32>,
    /// Times the drive backed off a jam, as the controller counts its
    /// reverse attempts: the one count the log, the trace and the
    /// statistics share. A double bowl's change of direction is none.
    pub reversals: u16,
}

/// The whole recorder, body and bookkeeping, stays under a kilobyte of
/// RAM (on the host, with its wider `usize`, too).
const _: () = assert!(core::mem::size_of::<Recorder>() < 1024);

/// One feed's trace, from the request to its result.
#[derive(Clone, Debug)]
pub struct Recorder {
    feed: u32,
    body: Writer<BODY_MAX>,
    /// The body keeps every `stride`th poll's current reading.
    stride: u16,
    /// Driven polls so far, which pick the readings `stride` keeps.
    polls: u32,
    truncated: bool,
    beam_breaks: u16,
    battery_rest: Option<u32>,
    battery_min: Option<u32>,
    /// The last battery reading the body kept.
    battery_kept: Option<u32>,
    beams: Option<Beams>,
    edges: u32,
    motor: MotorCommand,
    summary: Option<TraceSummary>,
}

impl Default for Recorder {
    fn default() -> Self {
        Self::new()
    }
}

impl Recorder {
    /// Const, so the firmware keeps it in a static: it is most of a
    /// kilobyte, which the loop's stack cannot spare.
    pub const fn new() -> Self {
        Self {
            feed: 0,
            body: Writer::new(),
            stride: 1,
            polls: 0,
            truncated: false,
            beam_breaks: 0,
            battery_rest: None,
            battery_min: None,
            battery_kept: None,
            beams: None,
            edges: 0,
            motor: MotorCommand::Stop,
            summary: None,
        }
    }

    /// Start the trace of feed `feed`, forgetting the last one. Field by
    /// field, so no second recorder is built on the loop's stack.
    pub fn begin(&mut self, feed: u32) {
        self.feed = feed;
        self.body.clear();
        self.stride = 1;
        self.polls = 0;
        self.truncated = false;
        self.beam_breaks = 0;
        self.battery_rest = None;
        self.battery_min = None;
        self.battery_kept = None;
        self.beams = None;
        self.edges = 0;
        self.motor = MotorCommand::Stop;
        self.summary = None;
    }

    /// The feed this trace is of.
    pub const fn feed(&self) -> u32 {
        self.feed
    }

    /// The summary, once the trace is sealed.
    pub const fn summary(&self) -> Option<&TraceSummary> {
        self.summary.as_ref()
    }

    /// The body so far.
    pub fn body(&self) -> &[u8] {
        self.body.bytes()
    }

    /// One poll of the feed. Before the drive only the battery counts, as
    /// its level at rest; from the drive's start every change is an entry,
    /// timed from it.
    pub fn observe(&mut self, seen: &Observation) {
        if self.summary.is_some() {
            return;
        }
        let Some(drive_at) = seen.drive_at_ms else {
            if seen.battery_mv.is_some() {
                self.battery_rest = seen.battery_mv;
            }
            return;
        };
        let since = |at: u32| {
            let since = at.wrapping_sub(drive_at);
            if since < HALF_RANGE { since } else { 0 }
        };
        let at_ms = since(seen.at_ms);

        let edges = u32::from(seen.rising) + u32::from(seen.falling);
        if edges > self.edges
            && let Some((at, high)) = seen.last_edge
        {
            let count = edges - self.edges;
            self.edges = edges;
            self.push(Entry::Mark {
                at_ms: since(at),
                edges: count,
                high,
            });
        }

        if let Some(beams) = seen.beams
            && self.beams != Some(beams)
        {
            if self.beams.is_some_and(|last| last.outlet_clear) && !beams.outlet_clear {
                self.beam_breaks = self.beam_breaks.saturating_add(1);
            }
            self.beams = Some(beams);
            self.push(Entry::Beams {
                at_ms,
                outlet_clear: beams.outlet_clear,
                food: beams.food,
            });
        }

        if let Some(mv) = seen.battery_mv {
            self.battery_min = Some(self.battery_min.map_or(mv, |min| min.min(mv)));
            if self
                .battery_kept
                .is_none_or(|kept| kept.abs_diff(mv) >= BATTERY_STEP_MV)
            {
                self.battery_kept = Some(mv);
                self.push(Entry::Battery { at_ms, mv });
            }
        }

        // After the marks, beams and battery: an entry timed before the
        // last one is held at its time, so the poll's own entries go after
        // the edge the interrupt stamped earlier (the counting edge and the
        // stop it causes fall in one poll).
        if seen.motor != self.motor {
            self.motor = seen.motor;
            let motor = match seen.motor {
                MotorCommand::Stop => Motor::Stop,
                MotorCommand::Drive0 => Motor::Drive0,
                MotorCommand::Drive1 => Motor::Drive1,
            };
            self.push(Entry::Motor { at_ms, motor });
        }

        if let Some(value) = seen.current {
            let poll = self.polls;
            self.polls = self.polls.wrapping_add(1);
            if poll.is_multiple_of(u32::from(self.stride)) {
                self.push(Entry::Current { at_ms, value });
            }
        }
    }

    /// Append one entry, halving the current readings while the body is
    /// full; with none left to halve, the body ends here.
    fn push(&mut self, entry: Entry) {
        while !self.truncated {
            match self.body.push(entry) {
                Pushed::Kept => return,
                Pushed::Full if self.body.halve() => {
                    self.stride = self.stride.saturating_mul(2);
                }
                Pushed::Full => self.truncated = true,
            }
        }
    }

    /// One page of the sealed trace from `offset` into `out`: `Busy` while
    /// it is still recorded, `Invalid` for an offset past its body.
    pub fn page(&self, offset: u16, out: &mut [u8]) -> Result<TraceRead, Refusal> {
        let summary = self.summary.ok_or(Refusal::Busy)?;
        let body = self.body.bytes();
        let rest = body.get(usize::from(offset)..).ok_or(Refusal::Invalid)?;
        let len = rest.len().min(out.len());
        out[..len].copy_from_slice(&rest[..len]);
        Ok(TraceRead {
            summary,
            stored: false,
            total: u16::try_from(body.len()).unwrap_or(u16::MAX),
            len,
        })
    }

    /// End the trace with the feed's `Fed` event and the owner's figures;
    /// its summary. A trace is sealed once: later polls change nothing.
    pub fn seal(&mut self, fed: &EventBody, drive: &DriveFigures) -> Option<TraceSummary> {
        let EventBody::Fed {
            feed,
            requested,
            completed,
            outcome,
            reason,
            ..
        } = *fed
        else {
            return None;
        };
        if feed != self.feed || self.summary.is_some() {
            return None;
        }

        let mv = |value: Option<u32>| value.map(|mv| u16::try_from(mv).unwrap_or(u16::MAX));
        let summary = TraceSummary {
            feed,
            requested,
            completed,
            outcome,
            reason,
            drive_ms: drive.drive_ms,
            samples: drive.samples,
            unit: CurrentUnit::Scaled,
            current_peak: drive.current_peak,
            current_mean: drive.current_mean,
            rising: drive.rising,
            falling: drive.falling,
            first_edge_ms: drive.first_edge_ms,
            beam_breaks: self.beam_breaks,
            reversals: drive.reversals,
            battery_rest_mv: mv(self.battery_rest),
            battery_min_mv: mv(self.battery_min),
            stride: self.stride,
            truncated: self.truncated || self.body.truncated(),
        };
        self.summary = Some(summary);
        Some(summary)
    }
}
