//! A feed's trace as text: the summary, then one line an entry, each at its
//! milliseconds from the drive's start, so it lines up with the UART's
//! `PLOWN1 fed` and `current` records and a logic analyzer's capture of
//! the drive.

use std::fmt;

use wellfed_model::{CurrentUnit, Trace, TraceEntry, TraceMotor, TraceSummary};

/// The text form of a trace.
pub(crate) struct Text<'a>(pub(crate) &'a Trace);

impl fmt::Display for Text<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Trace {
            summary,
            stored,
            entries,
        } = self.0;
        summary_lines(summary, *stored, f)?;
        for entry in entries {
            entry_line(entry, f)?;
        }
        Ok(())
    }
}

fn summary_lines(s: &TraceSummary, stored: bool, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    writeln!(
        f,
        "feed {}: {:?} ({:?}), {} of {} portions, drive {} ms, {}",
        s.feed,
        s.outcome,
        s.reason,
        s.completed,
        s.requested,
        s.drive_ms,
        if stored { "stored" } else { "in RAM only" }
    )?;
    let unit = match s.unit {
        CurrentUnit::Scaled => "scaled, not yet mA",
        CurrentUnit::MilliAmps => "mA",
    };
    write!(f, "current: peak {}, mean ", s.current_peak)?;
    optional(s.current_mean, f)?;
    writeln!(
        f,
        " ({unit}) over {} polls; every {} kept{}",
        s.samples,
        ordinal(s.stride),
        if s.truncated {
            ", the entries end early (truncated)"
        } else {
            ""
        }
    )?;
    write!(
        f,
        "marks: {} rising, {} falling, first at ",
        s.rising, s.falling
    )?;
    optional(s.first_edge_ms, f)?;
    writeln!(
        f,
        " ms; beam breaks {}; reversals {}",
        s.beam_breaks, s.reversals
    )?;
    write!(f, "battery: ")?;
    optional(s.battery_rest_mv, f)?;
    write!(f, " mV at rest, ")?;
    optional(s.battery_min_mv, f)?;
    writeln!(f, " mV lowest")
}

fn entry_line(entry: &TraceEntry, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match *entry {
        TraceEntry::Current { at_ms, value } => writeln!(f, "{at_ms:>8} current {value}"),
        TraceEntry::Mark { at_ms, edges, high } => writeln!(
            f,
            "{at_ms:>8} mark {} edges={edges}",
            if high { "high" } else { "low" }
        ),
        TraceEntry::Beams {
            at_ms,
            outlet_clear,
            food,
        } => writeln!(
            f,
            "{at_ms:>8} beams outlet={} food={}",
            if outlet_clear { "clear" } else { "blocked" },
            if food { "seen" } else { "none" }
        ),
        TraceEntry::Battery { at_ms, mv } => writeln!(f, "{at_ms:>8} battery {mv} mV"),
        TraceEntry::Motor { at_ms, motor } => writeln!(
            f,
            "{at_ms:>8} motor {}",
            match motor {
                TraceMotor::Stop => "stop",
                TraceMotor::Drive0 => "drive0",
                TraceMotor::Drive1 => "drive1",
            }
        ),
    }
}

fn optional(value: Option<impl fmt::Display>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match value {
        Some(value) => write!(f, "{value}"),
        None => f.write_str("none"),
    }
}

fn ordinal(stride: u16) -> String {
    match stride {
        1 => "poll's reading".to_owned(),
        2 => "2nd reading".to_owned(),
        3 => "3rd reading".to_owned(),
        n => format!("{n}th reading"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wellfed_model::{FeedOutcome, FeedReason};

    #[test]
    fn a_trace_reads_as_a_summary_and_its_entries() {
        let trace = Trace {
            summary: TraceSummary {
                feed: 9,
                requested: 3,
                completed: 3,
                outcome: FeedOutcome::Done,
                reason: FeedReason::Done,
                drive_ms: 27_241,
                samples: 903,
                unit: CurrentUnit::Scaled,
                current_peak: 168,
                current_mean: Some(59),
                rising: 3,
                falling: 3,
                first_edge_ms: Some(7_840),
                beam_breaks: 0,
                reversals: 0,
                battery_rest_mv: None,
                battery_min_mv: None,
                stride: 4,
                truncated: false,
            },
            stored: true,
            entries: vec![
                TraceEntry::Motor {
                    at_ms: 0,
                    motor: TraceMotor::Drive0,
                },
                TraceEntry::Mark {
                    at_ms: 7_840,
                    edges: 1,
                    high: false,
                },
            ],
        };
        assert_eq!(
            Text(&trace).to_string(),
            "feed 9: Done (Done), 3 of 3 portions, drive 27241 ms, stored\n\
             current: peak 168, mean 59 (scaled, not yet mA) over 903 polls; every 4th reading kept\n\
             marks: 3 rising, 3 falling, first at 7840 ms; beam breaks 0; reversals 0\n\
             battery: none mV at rest, none mV lowest\n\
             \x20      0 motor drive0\n\
             \x20   7840 mark low edges=1\n"
        );
    }
}
