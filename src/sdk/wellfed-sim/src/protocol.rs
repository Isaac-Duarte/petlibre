//! The worker protocol, shared by `virtual-feeder-worker` and the harness:
//! one JSON [`Command`] per line on the worker's stdin, one JSON [`Event`]
//! per line on its stdout.
//!
//! The worker reports the owner's status codes as indexes (as the owner's C
//! enums hold them) and the harness's reports keep them that way; scenarios
//! and the report's `enums` table use the names. Every command argument is
//! range-checked on deserialization, so the worker refuses whatever the
//! scenario validation would.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// An owner status code: its index on the wire, its name in scenarios.
pub trait Code: Copy + 'static {
    /// Every code, in index order.
    const ALL: &'static [Self];

    fn index(self) -> u64;

    fn from_index(index: u64) -> Option<Self> {
        Self::ALL.get(usize::try_from(index).ok()?).copied()
    }
}

macro_rules! codes {
    ($(#[$meta:meta])* $name:ident { $($variant:ident),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            #[default]
            $($variant),+
        }

        impl Code for $name {
            const ALL: &'static [Self] = &[$(Self::$variant),+];

            fn index(self) -> u64 {
                self as u64
            }
        }
    };
}

codes!(State {
    Starting,
    Idle,
    Queued,
    Preparing,
    Feeding,
    Reporting,
    Terminal,
    Fault,
    Stopped,
});

codes!(Fault {
    Ok,
    Setup,
    Recovery,
    Storage,
    Hardware,
    Watchdog,
    Flash,
});

codes!(Outcome {
    Pending,
    Done,
    Cancelled,
    Expired,
    Failed,
    Uncertain,
});

codes!(Reason {
    None,
    Done,
    Cancelled,
    Clock,
    ObservationGap,
    TotalTimeout,
    FeedbackTimeout,
    Power,
    Sensor,
    Outlet,
    Overcurrent,
    FeedbackStream,
    Output,
});

codes!(
    /// The owner's answer to a `submit`, carried in the status `response`.
    Submission {
        Enqueued,
        RejectBusy,
        RejectId,
        RejectRequest,
        RejectPhase,
        RejectStopped,
    }
);

codes!(
    /// The journal operation in progress around a storage callback; `resolve`
    /// records an interrupted reservation as uncertain at boot.
    StoragePhase {
        None,
        Reserve,
        Finish,
        Recover,
        Resolve,
    }
);

/// Names of the worker's `motor` codes, from the two drive bits.
pub const MOTORS: [&str; 4] = ["stop", "drive_0", "drive_1", "invalid"];

/// Serde for a [`Code`] field as its index.
pub mod index {
    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer, Serializer};

    use super::Code;

    pub fn serialize<C: Code, S: Serializer>(code: &C, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u64(code.index())
    }

    pub fn deserialize<'de, C: Code, D: Deserializer<'de>>(deserializer: D) -> Result<C, D::Error> {
        let index = u64::deserialize(deserializer)?;
        C::from_index(index).ok_or_else(|| D::Error::custom(format!("unknown code {index}")))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    #[default]
    Single,
    Double,
}

/// Whether the worker creates a new journal image or recovers from the one
/// a previous boot left.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Journal {
    Provision,
    Recover,
}

/// The worker's command line.
#[derive(Debug, clap::Parser)]
#[command(about = "Virtual feeder worker; driven by the virtual-feeder harness")]
pub struct WorkerArgs {
    /// The synthetic journal image.
    pub journal: PathBuf,
    pub profile: Profile,
    pub start: Journal,
}

/// An integer argument in `MIN..=MAX`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct Bounded<const MIN: u32, const MAX: u32>(pub u32);

impl<const MIN: u32, const MAX: u32> TryFrom<u32> for Bounded<MIN, MAX> {
    type Error = String;

    fn try_from(value: u32) -> Result<Self, String> {
        if (MIN..=MAX).contains(&value) {
            Ok(Self(value))
        } else {
            Err(format!("{value} is outside {MIN}..={MAX}"))
        }
    }
}

impl<const MIN: u32, const MAX: u32> From<Bounded<MIN, MAX>> for u32 {
    fn from(value: Bounded<MIN, MAX>) -> Self {
        value.0
    }
}

/// A 12-bit ADC reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub struct Adc(pub u16);

impl TryFrom<u16> for Adc {
    type Error = String;

    fn try_from(value: u16) -> Result<Self, String> {
        if value <= 4095 {
            Ok(Self(value))
        } else {
            Err(format!("ADC reading {value} exceeds 4095"))
        }
    }
}

impl From<Adc> for u16 {
    fn from(value: Adc) -> Self {
        value.0
    }
}

/// A 0/1 switch, written as an integer like the other arguments.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct Flag(pub bool);

impl TryFrom<u8> for Flag {
    type Error = String;

    fn try_from(value: u8) -> Result<Self, String> {
        match value {
            0 => Ok(Self(false)),
            1 => Ok(Self(true)),
            _ => Err(format!("flag {value} is not 0 or 1")),
        }
    }
}

impl From<Flag> for u8 {
    fn from(value: Flag) -> Self {
        value.0.into()
    }
}

/// Feedback edge period of the synthetic plant, in ms; 0 turns it off.
pub type Period = Bounded<0, 100_000>;

/// One worker command, `{"op": .., "args": [..]}`. Scenario actions use the
/// same shape, so commands without arguments still accept an empty `args`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    /// Request id and portions.
    Submit {
        args: (u64, u16),
    },
    Cancel {
        args: [u64; 1],
    },
    Ack {
        args: [u64; 1],
    },

    /// Advance the device clock by one millisecond.
    Tick {
        #[serde(default)]
        args: [u64; 0],
    },
    /// Step the synthetic plant and run the owner task once.
    Poll {
        #[serde(default)]
        args: [u64; 0],
    },

    /// Cut power after this many flash mutations in storage phase 1
    /// (reserve) or 2 (finish).
    Cut {
        args: (Bounded<1, 2>, Bounded<0, 100_000>),
    },
    PowerCut {
        #[serde(default)]
        args: [u64; 0],
    },

    Auto {
        args: [Period; 1],
    },
    Edge {
        args: [Flag; 1],
    },
    Owner {
        args: [Flag; 1],
    },
    Timer {
        args: [Flag; 1],
    },
    AdcStuck {
        args: [Flag; 1],
    },
    Current {
        args: [Adc; 1],
    },
    Battery {
        args: [Adc; 1],
    },
    /// Bit 0 food present, bit 1 power, bit 2 outlet blocked.
    Inputs {
        args: [Bounded<0, 7>; 1],
    },

    Status {
        #[serde(default)]
        args: [u64; 0],
    },
    Stop {
        #[serde(default)]
        args: [u64; 0],
    },
}

impl Command {
    pub const TICK: Self = Self::Tick { args: [] };
    pub const POLL: Self = Self::Poll { args: [] };

    /// Whether this command removes the worker's power.
    pub fn cuts_power(&self) -> bool {
        matches!(self, Self::Cut { .. } | Self::PowerCut { .. })
    }
}

/// The owner's status after every command.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub device_ms: u64,
    /// The command's own result: a [`Submission`] index for `submit`, 0/1
    /// for `cancel` and `ack`, otherwise 0.
    pub response: u64,
    #[serde(with = "index")]
    pub state: State,
    #[serde(with = "index")]
    pub fault: Fault,
    pub supervisor_fault: u32,
    pub watchdog_fault: u32,
    pub id: u64,
    pub completed: u16,
    #[serde(with = "index")]
    pub outcome: Outcome,
    #[serde(with = "index")]
    pub reason: Reason,
    pub durable: bool,
    pub stopped: bool,
    pub optics_off: bool,
    pub phase_known: bool,
    pub high_water_id: u64,
    pub pending: bool,
    pub journal_error: u32,
    pub motor: u32,
    pub optics: u32,
    pub watchdog_resets: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IoOperation {
    Read,
    Program,
    Erase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IoStage {
    Begin,
    End,
}

/// One worker output line. A `status` ends each command's response, and a
/// `power_cut` ends the worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Status(Status),
    Outputs {
        device_ms: u64,
        motor: u32,
        optics: u32,
    },
    StorageBegin {
        device_ms: u64,
        #[serde(with = "index")]
        phase: StoragePhase,
    },
    StorageEnd {
        device_ms: u64,
        #[serde(with = "index")]
        phase: StoragePhase,
        mutations: u32,
        ok: bool,
    },
    Feedback {
        device_ms: u64,
        high: bool,
    },
    PowerCut {
        device_ms: u64,
        #[serde(with = "index")]
        phase: StoragePhase,
        mutations: u32,
    },
    JournalIo {
        device_ms: u64,
        operation: IoOperation,
        stage: IoStage,
        slot: u32,
        offset: u32,
        bytes: usize,
        ok: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip_by_index() {
        assert_eq!(Reason::from_index(12), Some(Reason::Output));
        assert_eq!(Reason::from_index(13), None);
        assert_eq!(Submission::RejectStopped.index(), 5);
        assert_eq!(
            serde_json::to_value(State::ALL).unwrap()[6],
            serde_json::json!("terminal")
        );
    }

    #[test]
    fn commands_check_their_arguments() {
        let parse = |text: &str| serde_json::from_str::<Command>(text);

        assert_eq!(
            parse(r#"{"op":"current","args":[4095]}"#).unwrap(),
            Command::Current { args: [Adc(4095)] }
        );
        assert_eq!(
            parse(r#"{"op":"status"}"#).unwrap(),
            Command::Status { args: [] }
        );

        for bad in [
            r#"{"op":"current","args":[4096]}"#,
            r#"{"op":"cut","args":[3,0]}"#,
            r#"{"op":"edge","args":[2]}"#,
            r#"{"op":"submit","args":[1,65536]}"#,
            r#"{"op":"submit","args":[1]}"#,
            r#"{"op":"status","args":[1]}"#,
            r#"{"op":"auto","args":[true]}"#,
            r#"{"op":"auto","args":[1.0]}"#,
            r#"{"op":"auto"}"#,
            r#"{"op":"stop","values":{}}"#,
            r#"{"op":"shell","args":[]}"#,
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn commands_serialize_in_scenario_shape() {
        let text = serde_json::to_string(&Command::Submit { args: (1, 3) }).unwrap();
        assert_eq!(text, r#"{"op":"submit","args":[1,3]}"#);
    }
}
