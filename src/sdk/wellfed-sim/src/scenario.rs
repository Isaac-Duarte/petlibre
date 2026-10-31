//! Scenario files: the plant, timed worker commands, reboots and status
//! expectations.
//!
//! A scenario is checked fully before any worker starts. Each step keeps
//! its raw JSON because the report quotes actions exactly as written. The
//! plant defaults to the measured mechanism; a scenario changes only what
//! it tests (the seed, misses, bounces, the period with food).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::plant::PlantConfig;
use crate::protocol::{Command, Fault, Outcome, Plant, Profile, Reason, State, Submission};
use crate::{HarnessError, Result, require};

pub const VERSION: u64 = 2;
const MAX_ACTIONS: usize = 10_000;
/// Ten minutes: the longest feed the limits allow, and then some.
pub const MAX_END_MS: u64 = 900_000;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fields {
    version: u64,
    name: String,
    #[serde(default)]
    profile: Profile,
    #[serde(default = "default_plant")]
    plant: Plant,
    end_ms: u64,
    actions: Vec<Step>,
}

fn default_plant() -> Plant {
    Plant(PlantConfig::default())
}

/// A validated scenario, with its defaults filled in.
#[derive(Debug)]
pub struct Scenario {
    pub name: String,
    pub profile: Profile,
    pub plant: PlantConfig,
    pub end_ms: u64,
    pub steps: Vec<Step>,
}

#[derive(Debug, Deserialize)]
#[serde(try_from = "Value")]
pub struct Step {
    pub at_ms: u64,
    pub action: Action,
    /// The action as the scenario wrote it.
    pub raw: Value,
}

impl TryFrom<Value> for Step {
    type Error = serde_json::Error;

    fn try_from(raw: Value) -> std::result::Result<Self, Self::Error> {
        #[derive(Deserialize)]
        struct At {
            at_ms: u64,
        }

        let At { at_ms } = At::deserialize(&raw)?;
        let mut rest = raw.clone();
        if let Some(fields) = rest.as_object_mut() {
            fields.remove("at_ms");
        }

        Ok(Self {
            at_ms,
            action: Action::deserialize(rest)?,
            raw,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Expect {
        values: Expectation,
    },
    /// Restart a powered-off worker on the same journal.
    Boot {
        #[serde(default)]
        args: [u64; 0],
    },
    #[serde(untagged)]
    Command(Command),
}

/// Status values to assert: the worker's [`crate::protocol::Status`] fields
/// plus the harness's `online`, `boots` and last `submission`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expectation {
    pub boots: Option<u64>,
    pub completed: Option<u16>,
    pub device_ms: Option<u64>,
    pub durable: Option<bool>,
    pub fault: Option<Fault>,
    pub food: Option<Option<bool>>,
    pub food_reading: Option<bool>,
    pub food_reads: Option<u32>,
    pub high_water_id: Option<u64>,
    pub id: Option<u64>,
    pub journal_error: Option<u32>,
    pub motor: Option<u32>,
    pub online: Option<bool>,
    pub optics: Option<u32>,
    pub optics_off: Option<bool>,
    pub outcome: Option<Outcome>,
    pub pending: Option<bool>,
    pub phase_known: Option<bool>,
    pub at_mark: Option<bool>,
    pub homed: Option<bool>,
    pub reason: Option<Reason>,
    pub response: Option<u64>,
    pub sensing: Option<bool>,
    pub state: Option<State>,
    pub stopped: Option<bool>,
    pub submission: Option<Submission>,
    pub supervisor_fault: Option<u32>,
    pub supply_low: Option<bool>,
    pub watchdog_fault: Option<u32>,
    pub watchdog_resets: Option<u32>,
    pub writes_done: Option<u32>,
    pub writes_waiting: Option<u32>,
}

impl Expectation {
    /// Whether it names only what is known while the power is off.
    pub fn power_only(&self) -> bool {
        Self {
            online: None,
            boots: None,
            ..*self
        } == Self::default()
    }
}

impl Scenario {
    pub fn from_value(raw: &Value) -> Result<Self> {
        let fields = Fields::deserialize(raw).map_err(HarnessError::Scenario)?;

        require(fields.version == VERSION, "Scenario version must be 2")?;
        require(
            (1..=100).contains(&fields.name.chars().count()),
            "Scenario needs a name of 1..100 characters",
        )?;
        require(fields.end_ms <= MAX_END_MS, "end_ms must be 0..900000")?;
        require(
            fields.actions.len() <= MAX_ACTIONS,
            "actions must be a list with at most 10000 entries",
        )?;

        let mut previous = 0;
        for step in &fields.actions {
            require(
                (previous..=fields.end_ms).contains(&step.at_ms),
                "Actions must have nondecreasing timestamps within end_ms",
            )?;
            previous = step.at_ms;

            // The harness steps the worker and places its rotor itself.
            require(
                !matches!(
                    step.action,
                    Action::Command(
                        Command::Tick { .. }
                            | Command::Poll { .. }
                            | Command::Run { .. }
                            | Command::Plant { .. }
                    )
                ),
                "Unknown action",
            )?;
            if let Action::Expect { values } = step.action {
                require(
                    values != Expectation::default(),
                    "expect requires known, nonempty status values",
                )?;
            }
        }

        require(
            fields
                .actions
                .iter()
                .any(|step| matches!(step.action, Action::Expect { .. })),
            "Scenario needs at least one expectation",
        )?;

        Ok(Self {
            name: fields.name,
            profile: fields.profile,
            plant: fields.plant.0,
            end_ms: fields.end_ms,
            steps: fields.actions,
        })
    }
}
