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

use crate::plant::PlantConfig;

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
    Sensor,
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
    /// The owner's answer to a `submit`, carried in the status `response`;
    /// `RejectHeld` is the loop's own, when its claim on the outputs was
    /// refused (a store write still ran past the claim's wait) and the owner
    /// never saw the feed.
    Submission {
        Enqueued,
        RejectBusy,
        RejectId,
        RejectRequest,
        RejectPhase,
        RejectStopped,
        RejectHeld,
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

/// The most milliseconds one `run` steps: its events stay inside the
/// transport's bounds.
pub const RUN_MAX_MS: u32 = 1000;

/// What the `wedge` command puts in the rotor: 0 nothing, 1 a wedge the
/// first reversal frees, 2 one that stalls every forward drive however
/// often the rotor backs off it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub enum Wedge {
    #[default]
    Clear,
    Frees,
    Holds,
}

impl TryFrom<u8> for Wedge {
    type Error = String;

    fn try_from(value: u8) -> Result<Self, String> {
        match value {
            0 => Ok(Self::Clear),
            1 => Ok(Self::Frees),
            2 => Ok(Self::Holds),
            _ => Err(format!("wedge {value} is not 0, 1 or 2")),
        }
    }
}

impl From<Wedge> for u8 {
    fn from(value: Wedge) -> Self {
        match value {
            Wedge::Clear => 0,
            Wedge::Frees => 1,
            Wedge::Holds => 2,
        }
    }
}

/// The storage phase a `cut` targets: reserve, finish or resolve (recover
/// writes nothing).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct CutPhase(pub StoragePhase);

impl TryFrom<u32> for CutPhase {
    type Error = String;

    fn try_from(value: u32) -> Result<Self, String> {
        match StoragePhase::from_index(value.into()) {
            Some(
                phase @ (StoragePhase::Reserve | StoragePhase::Finish | StoragePhase::Resolve),
            ) => Ok(Self(phase)),
            _ => Err(format!("{value} is not a phase that writes")),
        }
    }
}

impl From<CutPhase> for u32 {
    fn from(value: CutPhase) -> Self {
        u32::try_from(value.0.index()).unwrap_or(u32::MAX)
    }
}

/// A plant's parameters, refused unless its transitions come in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "PlantConfig", into = "PlantConfig")]
pub struct Plant(pub PlantConfig);

impl TryFrom<PlantConfig> for Plant {
    type Error = &'static str;

    fn try_from(config: PlantConfig) -> Result<Self, &'static str> {
        if config.valid() {
            Ok(Self(config))
        } else {
            Err("the plant's transitions overlap")
        }
    }
}

impl From<Plant> for PlantConfig {
    fn from(value: Plant) -> Self {
        value.0
    }
}

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

    /// Advance the device clock by one millisecond, and what moves with
    /// it: the timer, the rotor, the background writer, the watchdog feeder.
    Tick {
        #[serde(default)]
        args: [u64; 0],
    },
    /// The loop's turn: the owner's step when it is due, then the outputs'
    /// release once it rests.
    Poll {
        #[serde(default)]
        args: [u64; 0],
    },
    /// This many ticks, each followed by a poll; stops early after a tick
    /// that requests a watchdog reset.
    Run {
        args: [Bounded<1, RUN_MAX_MS>; 1],
    },

    /// Cut power after this many flash mutations in storage phase 1
    /// (reserve), 2 (finish) or 4 (resolve).
    Cut {
        args: (CutPhase, Bounded<0, 100_000>),
    },
    PowerCut {
        #[serde(default)]
        args: [u64; 0],
    },

    /// The rotor and its feedback, at a position in ms of drive.
    Plant {
        args: (Plant, i64),
    },
    /// The rotor stops turning (or turns again) while the motor drives: no
    /// mark passes, and the current does not show it.
    Jam {
        args: [Flag; 1],
    },
    /// Something wedged in the rotor: forward drive stalls it and reads the
    /// plant's stall current, while driving back turns it freely.
    Wedge {
        args: [Wedge; 1],
    },
    /// Drive PA18 to this level, interrupting as an edge does.
    Edge {
        args: [Flag; 1],
    },
    /// Queue this many store writes for the background writer, one every
    /// so many ms, the first at once.
    Write {
        args: (Bounded<1, 100>, Bounded<0, 600_000>),
    },
    /// Whether the loop steps the owner.
    Owner {
        args: [Flag; 1],
    },
    Timer {
        args: [Flag; 1],
    },
    AdcStuck {
        args: [Flag; 1],
    },
    /// Every motor write fails to take (drives as well as STOP): the pins keep
    /// what they drive and each write's readback fails. Set while the motor
    /// drives, this matches the `stop-fail` variant's held STOP, since the
    /// faulted gate then writes nothing but STOP; the variant itself holds
    /// only STOP.
    StopFails {
        args: [Flag; 1],
    },
    /// Hold the current channel at this reading, as a jam would, in place
    /// of the plant's.
    Current {
        args: [Adc; 1],
    },
    /// The battery channel at rest; a drive on the pack alone sags it by
    /// the plant's `surge_sag_adc` and `drive_sag_adc`.
    Battery {
        args: [Adc; 1],
    },
    /// Bit 0 sets PA16 (the outlet beam reads clear), bit 1 PA17 (the food
    /// beam), bit 2 PA19 (the adapter is present).
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
    /// The rotor rests at a mark, as far as the owner knows; the next feed
    /// homes it otherwise.
    pub at_mark: bool,
    /// The last feed reached a homing mark, uncredited.
    pub homed: bool,
    pub high_water_id: u64,
    pub pending: bool,
    pub journal_error: u32,
    pub motor: u32,
    pub optics: u32,
    pub watchdog_resets: u32,
    /// The rotor's position, in ms of drive (back on `Drive1`).
    pub rotor_ms: i64,
    /// Store writes the background writer finished, and those still queued
    /// or not yet due.
    pub writes_done: u32,
    pub writes_waiting: u32,
    /// The owner's supply verdict: on the pack, and the pack not judged
    /// above its floor.
    pub supply_low: bool,
}

impl Status {
    /// The same status, but for the clock and what moves with every
    /// millisecond of drive: what a `run` reports a change of.
    pub fn same_as(&self, other: &Self) -> bool {
        let still = |s: &Self| Self {
            device_ms: 0,
            response: 0,
            rotor_ms: 0,
            ..*s
        };
        still(self) == still(other)
    }
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
    /// The status changed during a command that steps time (`run`, or a
    /// claim waiting for a write).
    Changed(Status),
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
    /// PA18 changed: by the plant, or an `edge` command.
    Feedback {
        device_ms: u64,
        high: bool,
        /// Whether its interrupt came.
        captured: bool,
    },
    /// A background store write's flash lease began or ended.
    Lease {
        device_ms: u64,
        active: bool,
    },
    PowerCut {
        device_ms: u64,
        #[serde(with = "index")]
        phase: StoragePhase,
        mutations: u32,
        rotor_ms: i64,
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

impl Event {
    /// The worker's clock when it happened.
    pub fn device_ms(&self) -> u64 {
        match *self {
            Self::Status(status) | Self::Changed(status) => status.device_ms,
            Self::Outputs { device_ms, .. }
            | Self::StorageBegin { device_ms, .. }
            | Self::StorageEnd { device_ms, .. }
            | Self::Feedback { device_ms, .. }
            | Self::Lease { device_ms, .. }
            | Self::PowerCut { device_ms, .. }
            | Self::JournalIo { device_ms, .. } => device_ms,
        }
    }
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
        assert_eq!(
            parse(r#"{"op":"wedge","args":[2]}"#).unwrap(),
            Command::Wedge {
                args: [Wedge::Holds]
            }
        );
        assert_eq!(
            parse(r#"{"op":"plant","args":[{"seed":3},9074]}"#).unwrap(),
            Command::Plant {
                args: (
                    Plant(PlantConfig {
                        seed: 3,
                        ..PlantConfig::default()
                    }),
                    9074
                )
            }
        );
        assert_eq!(
            parse(r#"{"op":"cut","args":[4,0]}"#).unwrap(),
            Command::Cut {
                args: (CutPhase(StoragePhase::Resolve), Bounded(0))
            }
        );

        for bad in [
            r#"{"op":"current","args":[4096]}"#,
            r#"{"op":"cut","args":[3,0]}"#,
            r#"{"op":"cut","args":[5,0]}"#,
            r#"{"op":"run","args":[0]}"#,
            r#"{"op":"run","args":[1001]}"#,
            r#"{"op":"plant","args":[{"low_ms":3,"low_jitter_ms":0},0]}"#,
            r#"{"op":"plant","args":[{"color":1},0]}"#,
            r#"{"op":"edge","args":[2]}"#,
            r#"{"op":"wedge","args":[3]}"#,
            r#"{"op":"submit","args":[1,65536]}"#,
            r#"{"op":"submit","args":[1]}"#,
            r#"{"op":"status","args":[1]}"#,
            r#"{"op":"run","args":[true]}"#,
            r#"{"op":"run","args":[1.0]}"#,
            r#"{"op":"run"}"#,
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
