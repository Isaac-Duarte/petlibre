//! The MCP tools: their names, what each physically does, their arguments,
//! and the command line each hardware tool posts.
//!
//! A hardware tool builds the same words a person types and posts them as a
//! job, so the server's parse and every check behind it apply unchanged.
//! What a tool cannot express is refused here, before any words exist:
//! motion that needs a person at the bench (`--attended-motion`, the
//! suite's `--feeds`), breaking the lease, and a destructive suite run
//! without its explicit confirmation. No string a hardware tool takes may
//! start with `-`, so none reads as a flag, and the session name comes
//! after `--`. The note tools build their notes directly: their text is
//! free, dashes included, and goes through no command line.

use std::path::PathBuf;
use std::sync::Arc;

use rmcp::model::{JsonObject, Tool};
use schemars::JsonSchema;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use wellfed_bench_server::api::DuringWrite;

use crate::note::{self, NoteArgs, What};

/// Why a tool call was refused before it reached the bench.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ToolError {
    #[error("No tool {name:?}")]
    Unknown { name: String },
    #[error("{tool}: {error}")]
    Arguments {
        tool: String,
        error: serde_json::Error,
    },
    #[error(
        "{tool}: attended_motion is refused over MCP: a target or review with a motion policy moves the motor, which needs a person at the bench; run it from a terminal there"
    )]
    AttendedMotion { tool: String },
    #[error(
        "{tool}: feeds is refused over MCP: the suite's feeds move the motor unattended; run them from a terminal with a person at the bench"
    )]
    Feeds { tool: String },
    #[error(
        "{tool}: break_lease is refused over MCP: only a person who has looked at the bench may free its lease (`wellfed-bench status --break-lease`)"
    )]
    BreakLease { tool: String },
    #[error(
        "{tool}: destructive is refused over MCP without confirm_destructive: power cuts mid-update may leave a feeder only a UART flash recovers; set confirm_destructive to true to mean it"
    )]
    UnconfirmedDestructive { tool: String },
    /// It would read as a flag on the command line the tool builds.
    #[error("{tool}: {field} {value:?} starts with '-', which no hardware tool takes")]
    LeadingDash {
        tool: String,
        field: String,
        value: String,
    },
}

/// What a call asks the bench for.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Call {
    Status,
    JobOutput {
        id: u64,
        after: u64,
    },
    JobCancel {
        id: Option<u64>,
        during_write: DuringWrite,
    },
    WaitFor(WaitArgs),
    LogsTail {
        source: LogSource,
        lines: usize,
    },
    Banks,
    /// A note, as `wellfed-bench note` takes it.
    Note(NoteArgs),
    /// A device API read, or the clock sync, as JSON for the server's
    /// read-only request type.
    Device(Value),
    /// A hardware job's words.
    Job(Vec<String>),
}

/// The refusal for a key no tool takes, whatever its schema says.
fn refused_key(tool: &str, key: &str) -> Option<ToolError> {
    let tool = tool.to_string();
    match key {
        "attended_motion" => Some(ToolError::AttendedMotion { tool }),
        "break_lease" => Some(ToolError::BreakLease { tool }),
        "feeds" => Some(ToolError::Feeds { tool }),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct NoArgs {}

#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct JobOutputArgs {
    /// The job's number.
    id: u64,
    /// The line number to read after (0 for the start); each answer gives
    /// the next.
    #[serde(default)]
    after: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CancelArgs {
    /// The job to cancel; the running one if left out.
    id: Option<u64>,
    /// Set true to cancel a UART write, which then stops terminally and
    /// leaves the bank half-written until another write.
    #[serde(default)]
    during_write: bool,
}

/// What `wait_for` waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Until {
    /// Job `id` ended.
    Job,
    /// A UART, device log or job output line containing `text`.
    Line,
    /// A device event of `kind`, such as boot or fed.
    Event,
    /// The feeder's status names `build` (a prefix) and/or `bank`.
    Status,
    /// The operator's timer `label` is due, seen or missed.
    Timer,
    /// No job runs and the flash client holds no UART.
    Idle,
}

#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct WaitArgs {
    #[serde(rename = "for")]
    pub(crate) until: Until,
    pub(crate) id: Option<u64>,
    pub(crate) text: Option<String>,
    pub(crate) kind: Option<String>,
    pub(crate) build: Option<String>,
    pub(crate) bank: Option<u8>,
    pub(crate) label: Option<String>,
    /// Seconds to wait, at most 600.
    pub(crate) timeout: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum LogSource {
    /// The feeder's debug log, as the server's session follows it.
    Device,
    /// The receive-only capture's UART lines.
    Uart,
    /// The current job's output.
    Job,
}

#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LogsArgs {
    source: LogSource,
    /// How many of the newest lines (default 50, at most 500).
    lines: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct StepArgs {
    text: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NextArgs {
    /// Replaces the list.
    steps: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TimerArgs {
    name: String,
    /// Due at this local time (HH:MM), the next time it comes.
    at: Option<String>,
    /// Due this long from now, such as 90s, 5m or 1h30m.
    #[serde(rename = "in")]
    after: Option<String>,
    /// Drop the timer.
    #[serde(default)]
    clear: bool,
    /// A UART or device log line containing this text, such as "PLSCH1 fire".
    expect: Option<String>,
    /// A device event of this kind, such as boot or fed.
    expect_event: Option<String>,
    /// How long after the due time the expectation may still arrive (default 2m).
    window: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum Verdict {
    Pass,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EvidenceArgs {
    /// The work the evidence is for, in the operator's words.
    card: String,
    /// A file or directory inside private/, from the checkout's root.
    path: String,
    verdict: Verdict,
    #[serde(default)]
    text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum Power {
    On,
    Off,
}

#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PowerArgs {
    state: Power,
}

#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NamedArgs {
    /// A single plain session name, new under private/.
    name: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TimedArgs {
    /// A single plain session name, new under private/captures/.
    name: String,
    seconds: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UpdateArgs {
    name: String,
    /// The frozen review to install, root-relative.
    target: String,
    /// The frozen review of the installed image.
    source: Option<String>,
    /// 1 (default), or 0, the factory loader's fallback.
    bank: Option<u8>,
    original_read: Option<String>,
    #[serde(default)]
    interrupted: bool,
    #[serde(default)]
    restore_marker: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExecuteArgs {
    name: String,
    target: String,
    source: Option<String>,
    bank: Option<u8>,
    original_read: Option<String>,
    #[serde(default)]
    interrupted: bool,
    #[serde(default)]
    restore_marker: bool,
    /// The plan's SHA-256 exactly as flash_prepare printed it, after review.
    plan_sha256: String,
    boot_seconds: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GiveWayArgs {
    name: String,
    /// The frozen review of bank 0's image.
    fallback: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GiveWayExecuteArgs {
    name: String,
    fallback: String,
    plan_sha256: String,
    boot_seconds: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct StockArgs {
    name: String,
    /// Bank 0's frozen review, to give way first while bank 1 holds an image.
    fallback: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct StockExecuteArgs {
    name: String,
    fallback: Option<String>,
    plan_sha256: String,
    boot_seconds: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OtaArgs {
    name: String,
    /// The frozen review to offer; or `build`.
    review: Option<String>,
    #[serde(default)]
    build: bool,
    #[serde(default)]
    rebuild: bool,
    /// For build: qa (default), optics, buttons, adc, speaker, blink, trial or feeder.
    firmware: Option<String>,
    variant: Option<String>,
    version: Option<u32>,
    minutes: Option<f64>,
    #[serde(default)]
    downgrade: bool,
    /// corrupt@N, unwanted@N, stall@N, drop@N or give-way-only.
    fault: Option<String>,
    /// Only cancel a pending install.
    #[serde(default)]
    cancel: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ItestArgs {
    name: String,
    review: Option<String>,
    #[serde(default)]
    build: bool,
    #[serde(default)]
    rebuild: bool,
    /// smoke, standard or full.
    tier: Option<String>,
    /// Only these scenarios.
    only: Option<Vec<String>>,
    /// Allow relay power cycles (cold boots).
    #[serde(default)]
    power: bool,
    /// Allow power cuts mid-update (needs power and confirm_destructive).
    #[serde(default)]
    destructive: bool,
    /// Set true to confirm a destructive run: a failed recovery from a cut
    /// is a UART flash.
    #[serde(default)]
    confirm_destructive: bool,
    #[serde(default)]
    keep_going: bool,
    soak_minutes: Option<f64>,
    sustained_cycles: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TransientArgs {
    name: String,
    cycles: Option<u32>,
    /// boot or loader.
    mode: Option<String>,
    /// driver or motor.
    probes: Option<String>,
    seconds: Option<f64>,
    samplerate: Option<u64>,
}

/// One tool: its name, what it physically does, and its arguments' type.
struct Spec {
    name: &'static str,
    description: &'static str,
    schema: fn() -> Arc<JsonObject>,
}

fn schema<T: JsonSchema + 'static>() -> Arc<JsonObject> {
    rmcp::handler::server::common::schema_for_type::<T>()
}

/// The device read's arguments: an op, and a section for `stats`.
fn device_schema() -> Arc<JsonObject> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "op": {
                "type": "string",
                "enum": ["status", "settings", "time", "plans", "clients", "update_state", "calibration", "call", "stats"],
            },
            "section": {
                "type": "string",
                "enum": ["counters", "memory", "flash", "power", "food", "feeds", "portions"],
                "description": "For stats only.",
            },
        },
        "required": ["op"],
        "additionalProperties": false,
    });
    match schema {
        Value::Object(object) => Arc::new(object),
        _ => Arc::new(JsonObject::new()),
    }
}

const SPECS: &[Spec] = &[
    Spec {
        name: "bench_status",
        description: "Read the bench server's live state, trimmed: the job and its step, children, the UART (idle, capturing, loader, writing with sectors), relays, the feeder session and status, banks, and the operator's notes. Reads only; touches nothing.",
        schema: schema::<NoArgs>,
    },
    Spec {
        name: "job_output",
        description: "Read a page of a job's output after a line number, with its end once that is all. Reads only.",
        schema: schema::<JobOutputArgs>,
    },
    Spec {
        name: "job_cancel",
        description: "Cancel the running job: the server sends SIGINT to wellfed-flash (which stops itself terminally and records it) and SIGTERM to other children, then the job runs its usual cleanup, which may power the feeder off. Refused during a UART write unless during_write is true: the write then stops for good and the bank stays half-written.",
        schema: schema::<CancelArgs>,
    },
    Spec {
        name: "wait_for",
        description: "Hold until a condition holds or the timeout passes: a job's end, a UART/device log/job line containing text, a device event kind, the feeder's status naming a build or bank, an operator timer falling due, or an idle bench. Reads only.",
        schema: schema::<WaitArgs>,
    },
    Spec {
        name: "logs_tail",
        description: "The newest lines of the feeder's debug log, the UART capture, or the current job's output. Reads only.",
        schema: schema::<LogsArgs>,
    },
    Spec {
        name: "banks",
        description: "The bank ledger: which build the server believes each flash bank holds, and whether the feeder confirmed it. Reads only.",
        schema: schema::<NoArgs>,
    },
    Spec {
        name: "note_step",
        description: "Record the step under way on the bench's console and page (notes.jsonl). Touches no hardware.",
        schema: schema::<StepArgs>,
    },
    Spec {
        name: "note_next",
        description: "Replace the list of next steps on the console and page. Touches no hardware.",
        schema: schema::<NextArgs>,
    },
    Spec {
        name: "note_timer",
        description: "Start or clear an operator timer; with expect or expect_event the server marks it seen when that arrives, or missed after its window. Touches no hardware.",
        schema: schema::<TimerArgs>,
    },
    Spec {
        name: "note_evidence",
        description: "Record evidence inside private/ for a card with a pass or fail verdict. Touches no hardware.",
        schema: schema::<EvidenceArgs>,
    },
    Spec {
        name: "device_read",
        description: "One read-only device API request on the server's feeder session (status, settings, time, plans, clients, update state, calibration, meal call, or a stats section). Never a feed, cancel, plan, setting or update.",
        schema: device_schema,
    },
    Spec {
        name: "clock_sync",
        description: "Set the feeder's clock to this host's clock and zone over the server's session, as `wellfed time` does. Changes only the feeder's clock.",
        schema: schema::<NoArgs>,
    },
    Spec {
        name: "power",
        description: "Switch relay 1: on powers the feeder with the loader strap released; off releases the strap and cuts the feeder's USB 5V.",
        schema: schema::<PowerArgs>,
    },
    Spec {
        name: "boot",
        description: "Cold boot: cut power, start a receive-only UART capture, then power the feeder on into its application and record it.",
        schema: schema::<TimedArgs>,
    },
    Spec {
        name: "capture",
        description: "Record the running feeder's UART, receive only. Power is untouched.",
        schema: schema::<TimedArgs>,
    },
    Spec {
        name: "loader",
        description: "Cold boot into the ROM loader: cut power, hold the strap (relay 2) across power-on, check the download banner, and leave the feeder powered in the loader.",
        schema: schema::<NamedArgs>,
    },
    Spec {
        name: "dump",
        description: "Cold boot into the ROM loader, then read flash over UART (a bounded read and a full read) into private/flash-backups/. Reads flash only.",
        schema: schema::<NamedArgs>,
    },
    Spec {
        name: "flash_prepare",
        description: "Cold boot into the ROM loader, read flash, inspect it and plan an update of a bank. Writes nothing; prints the plan and its SHA-256 for review.",
        schema: schema::<UpdateArgs>,
    },
    Spec {
        name: "flash_execute",
        description: "WRITES FLASH over UART: executes the reviewed plan whose SHA-256 is given (wellfed-flash refuses any other), then cold boots. Nothing on the bench may be touched during the write.",
        schema: schema::<ExecuteArgs>,
    },
    Spec {
        name: "give_way_prepare",
        description: "Cold boot into the ROM loader, read flash and plan clearing bank 1's first marker byte. Writes nothing.",
        schema: schema::<GiveWayArgs>,
    },
    Spec {
        name: "give_way_execute",
        description: "WRITES FLASH over UART: clears bank 1's first marker byte per the reviewed plan (bank 1 then holds no valid image), then cold boots into bank 0.",
        schema: schema::<GiveWayExecuteArgs>,
    },
    Spec {
        name: "stock_prepare",
        description: "Plan returning bank 1 to the pinned stock image over UART (with fallback, a give-way first). Writes nothing.",
        schema: schema::<StockArgs>,
    },
    Spec {
        name: "stock_execute",
        description: "WRITES FLASH over UART: executes the reviewed stock plan whose SHA-256 is given, then cold boots into stock.",
        schema: schema::<StockExecuteArgs>,
    },
    Spec {
        name: "ota",
        description: "Install a review on the running feeder over the air through the device API (it reboots into the new build, or falls back), watching the UART; never touches power.",
        schema: schema::<OtaArgs>,
    },
    Spec {
        name: "itest",
        description: "Run the integration suite on the feeder: its own hotspot, server and UART capture, installs over the air; cold boots only with power, power cuts mid-update only with destructive and confirm_destructive. Restores the bench afterwards.",
        schema: schema::<ItestArgs>,
    },
    Spec {
        name: "transients",
        description: "Logic-analyzer captures (sigrok) of repeated relay power-ons or loader entries; each cycle powers the feeder on and ends powered off. No UART, no flash.",
        schema: schema::<TransientArgs>,
    },
];

/// Every tool, as `tools/list` answers.
pub(crate) fn list() -> Vec<Tool> {
    SPECS
        .iter()
        .map(|spec| Tool::new(spec.name, spec.description, (spec.schema)()))
        .collect()
}

fn parse<T: DeserializeOwned>(tool: &str, args: JsonObject) -> Result<T, ToolError> {
    serde_json::from_value(Value::Object(args)).map_err(|error| ToolError::Arguments {
        tool: tool.into(),
        error,
    })
}

/// What `name` with `args` asks of the bench.
pub(crate) fn call(name: &str, args: JsonObject) -> Result<Call, ToolError> {
    if let Some(refused) = args.keys().find_map(|key| refused_key(name, key)) {
        return Err(refused);
    }

    let note = |what| Call::Note(NoteArgs { what });
    Ok(match name {
        "bench_status" => {
            parse::<NoArgs>(name, args)?;
            Call::Status
        }
        "job_output" => {
            let args: JobOutputArgs = parse(name, args)?;
            Call::JobOutput {
                id: args.id,
                after: args.after,
            }
        }
        "job_cancel" => {
            let args: CancelArgs = parse(name, args)?;
            Call::JobCancel {
                id: args.id,
                during_write: if args.during_write {
                    DuringWrite::Cancel
                } else {
                    DuringWrite::Refuse
                },
            }
        }
        "wait_for" => Call::WaitFor(parse(name, args)?),
        "logs_tail" => {
            let args: LogsArgs = parse(name, args)?;
            Call::LogsTail {
                source: args.source,
                lines: args.lines.unwrap_or(50).min(500),
            }
        }
        "banks" => {
            parse::<NoArgs>(name, args)?;
            Call::Banks
        }

        "note_step" => note(What::Step {
            text: parse::<StepArgs>(name, args)?.text,
        }),
        "note_next" => note(What::Next {
            steps: parse::<NextArgs>(name, args)?.steps,
        }),
        "note_timer" => note(What::Timer(timer(name, parse(name, args)?)?)),
        "note_evidence" => {
            let args: EvidenceArgs = parse(name, args)?;
            note(What::Evidence(note::EvidenceArgs {
                card: args.card,
                path: PathBuf::from(args.path),
                pass: args.verdict == Verdict::Pass,
                fail: args.verdict == Verdict::Fail,
                text: args.text,
            }))
        }

        "device_read" => Call::Device(Value::Object(args)),
        "clock_sync" => {
            parse::<NoArgs>(name, args)?;
            Call::Device(serde_json::json!({ "op": "clock_sync" }))
        }

        _ => Call::Job(job_words(name, args)?),
    })
}

/// The timer the tool asks for, with the command line's rules: a time or
/// a clear, and one expectation, none beside a clear.
fn timer(tool: &str, args: TimerArgs) -> Result<note::TimerArgs, ToolError> {
    let when = [args.at.is_some(), args.after.is_some(), args.clear];
    let expectations = [args.expect.is_some(), args.expect_event.is_some()];
    let expects = expectations.iter().filter(|set| **set).count();
    let invalid = |why: &str| ToolError::Arguments {
        tool: tool.into(),
        error: serde::de::Error::custom(why),
    };
    if when.iter().filter(|set| **set).count() != 1 {
        return Err(invalid("exactly one of at, in or clear says when"));
    }
    if expects > 1 {
        return Err(invalid("expect and expect_event: one expectation at most"));
    }
    if args.clear && (expects > 0 || args.window.is_some()) {
        return Err(invalid("a clear takes no expectation or window"));
    }
    Ok(note::TimerArgs {
        name: args.name,
        at: args.at,
        after: args.after,
        clear: args.clear,
        expect: args.expect,
        expect_event: args.expect_event,
        window: args.window,
    })
}

/// A command line under construction: the subcommand and its flags, then
/// `--` and the session name, so nothing after the flags reads as one.
struct Line {
    words: Vec<String>,
    name: Option<String>,
}

impl Line {
    fn new(words: &[&str]) -> Self {
        Self {
            words: words.iter().map(|word| (*word).to_string()).collect(),
            name: None,
        }
    }

    fn word(mut self, word: impl Into<String>) -> Self {
        self.name = Some(word.into());
        self
    }

    fn value(mut self, flag: &str, value: Option<impl ToString>) -> Self {
        if let Some(value) = value {
            self.words.push(flag.into());
            self.words.push(value.to_string());
        }
        self
    }

    fn switch(mut self, flag: &str, on: bool) -> Self {
        if on {
            self.words.push(flag.into());
        }
        self
    }

    fn finish(self) -> Vec<String> {
        let mut words = self.words;
        if let Some(name) = self.name {
            words.push("--".into());
            words.push(name);
        }
        words
    }

    fn update(self, args: UpdateArgs) -> Self {
        self.word(args.name)
            .value("--target", Some(args.target))
            .value("--source", args.source)
            .value("--bank", args.bank)
            .value("--original-read", args.original_read)
            .switch("--interrupted", args.interrupted)
            .switch("--restore-marker", args.restore_marker)
    }
}

/// Refused if any string `args` holds, in a list too, starts with `-`.
fn no_leading_dash(tool: &str, args: &JsonObject) -> Result<(), ToolError> {
    for (field, value) in args {
        let strings: Vec<&str> = match value {
            Value::String(text) => vec![text.as_str()],
            Value::Array(items) => items.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        if let Some(text) = strings.into_iter().find(|text| text.starts_with('-')) {
            return Err(ToolError::LeadingDash {
                tool: tool.into(),
                field: field.clone(),
                value: text.into(),
            });
        }
    }
    Ok(())
}

fn job_words(name: &str, args: JsonObject) -> Result<Vec<String>, ToolError> {
    no_leading_dash(name, &args)?;
    let line = match name {
        "power" => match parse::<PowerArgs>(name, args)?.state {
            Power::On => Line::new(&["on"]),
            Power::Off => Line::new(&["off"]),
        },
        "boot" | "capture" => {
            let args: TimedArgs = parse(name, args)?;
            Line::new(&[name])
                .word(args.name)
                .value("--seconds", args.seconds)
        }
        "loader" | "dump" => Line::new(&[name]).word(parse::<NamedArgs>(name, args)?.name),
        "flash_prepare" => Line::new(&["flash-prepare"]).update(parse(name, args)?),
        "flash_execute" => {
            let args: ExecuteArgs = parse(name, args)?;
            let update = UpdateArgs {
                name: args.name,
                target: args.target,
                source: args.source,
                bank: args.bank,
                original_read: args.original_read,
                interrupted: args.interrupted,
                restore_marker: args.restore_marker,
            };
            Line::new(&["flash-execute"])
                .update(update)
                .value("--expected-plan-sha256", Some(args.plan_sha256))
                .value("--boot-seconds", args.boot_seconds)
        }
        "give_way_prepare" => {
            let args: GiveWayArgs = parse(name, args)?;
            Line::new(&["give-way-prepare"])
                .word(args.name)
                .value("--fallback", Some(args.fallback))
        }
        "give_way_execute" => {
            let args: GiveWayExecuteArgs = parse(name, args)?;
            Line::new(&["give-way-execute"])
                .word(args.name)
                .value("--fallback", Some(args.fallback))
                .value("--expected-plan-sha256", Some(args.plan_sha256))
                .value("--boot-seconds", args.boot_seconds)
        }
        "stock_prepare" => {
            let args: StockArgs = parse(name, args)?;
            Line::new(&["stock-prepare"])
                .word(args.name)
                .value("--fallback", args.fallback)
        }
        "stock_execute" => {
            let args: StockExecuteArgs = parse(name, args)?;
            Line::new(&["stock-execute"])
                .word(args.name)
                .value("--fallback", args.fallback)
                .value("--expected-plan-sha256", Some(args.plan_sha256))
                .value("--boot-seconds", args.boot_seconds)
        }
        "ota" => {
            let args: OtaArgs = parse(name, args)?;
            Line::new(&["ota"])
                .word(args.name)
                .value("--review", args.review)
                .switch("--build", args.build)
                .switch("--rebuild", args.rebuild)
                .value("--firmware", args.firmware)
                .value("--variant", args.variant)
                .value("--version", args.version)
                .value("--minutes", args.minutes)
                .switch("--downgrade", args.downgrade)
                .value("--fault", args.fault)
                .switch("--cancel", args.cancel)
        }
        "itest" => {
            let args: ItestArgs = parse(name, args)?;
            if args.destructive && !args.confirm_destructive {
                return Err(ToolError::UnconfirmedDestructive { tool: name.into() });
            }
            Line::new(&["itest"])
                .word(args.name)
                .value("--review", args.review)
                .switch("--build", args.build)
                .switch("--rebuild", args.rebuild)
                .value("--tier", args.tier)
                .value("--only", args.only.map(|only| only.join(",")))
                .switch("--power", args.power)
                .switch("--destructive", args.destructive)
                .switch("--keep-going", args.keep_going)
                .value("--soak-minutes", args.soak_minutes)
                .value("--sustained-cycles", args.sustained_cycles)
        }
        "transients" => {
            let args: TransientArgs = parse(name, args)?;
            Line::new(&["transients"])
                .word(args.name)
                .value("--cycles", args.cycles)
                .value("--mode", args.mode)
                .value("--probes", args.probes)
                .value("--seconds", args.seconds)
                .value("--samplerate", args.samplerate)
        }
        _ => return Err(ToolError::Unknown { name: name.into() }),
    };
    Ok(line.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(value: Value) -> JsonObject {
        match value {
            Value::Object(object) => object,
            _ => panic!("not an object"),
        }
    }

    #[test]
    fn every_tool_is_listed_with_an_object_schema_and_a_physical_description() {
        let tools = list();

        assert_eq!(tools.len(), SPECS.len());
        for tool in &tools {
            assert_eq!(
                tool.input_schema.get("type"),
                Some(&Value::String("object".into())),
                "{}",
                tool.name
            );
            assert!(
                tool.description
                    .as_ref()
                    .is_some_and(|text| text.len() > 30)
            );
        }
        for name in [
            "power",
            "boot",
            "capture",
            "loader",
            "dump",
            "flash_prepare",
            "flash_execute",
            "give_way_prepare",
            "give_way_execute",
            "stock_prepare",
            "stock_execute",
            "ota",
            "itest",
            "transients",
        ] {
            assert!(tools.iter().any(|tool| tool.name == name), "{name}");
        }
    }

    #[test]
    fn a_hardware_tool_builds_the_command_line_a_person_types() {
        let execute = call(
            "flash_execute",
            args(serde_json::json!({
                "name": "unit-07",
                "target": "private/verification/r-review",
                "source": "private/verification/q-review",
                "plan_sha256": "ab12",
            })),
        )
        .unwrap();

        assert_eq!(
            execute,
            Call::Job(
                [
                    "flash-execute",
                    "--target",
                    "private/verification/r-review",
                    "--source",
                    "private/verification/q-review",
                    "--expected-plan-sha256",
                    "ab12",
                    "--",
                    "unit-07"
                ]
                .map(String::from)
                .to_vec()
            )
        );
        assert_eq!(
            call("power", args(serde_json::json!({"state": "on"}))).unwrap(),
            Call::Job(vec!["on".into()])
        );
    }

    #[test]
    fn motion_the_lease_and_an_unconfirmed_destructive_run_are_refused() {
        let refused = |tool: &str, value: Value| call(tool, args(value)).unwrap_err();

        assert!(matches!(
            refused(
                "flash_execute",
                serde_json::json!({"name": "u", "target": "t", "plan_sha256": "00", "attended_motion": true})
            ),
            ToolError::AttendedMotion { tool } if tool == "flash_execute"
        ));
        assert!(matches!(
            refused(
                "ota",
                serde_json::json!({"name": "o", "build": true, "attended_motion": false})
            ),
            ToolError::AttendedMotion { .. }
        ));
        assert!(matches!(
            refused("itest", serde_json::json!({"name": "i", "feeds": true})),
            ToolError::Feeds { .. }
        ));
        assert!(matches!(
            refused("bench_status", serde_json::json!({"break_lease": true})),
            ToolError::BreakLease { .. }
        ));
        assert!(matches!(
            refused(
                "itest",
                serde_json::json!({"name": "i", "power": true, "destructive": true})
            ),
            ToolError::UnconfirmedDestructive { .. }
        ));

        // A plan hash is not optional, and nothing unknown slips through.
        assert!(matches!(
            call(
                "flash_execute",
                args(serde_json::json!({"name": "u", "target": "t"}))
            ),
            Err(ToolError::Arguments { .. })
        ));
        assert!(matches!(
            call(
                "boot",
                args(serde_json::json!({"name": "b", "off_seconds": 0}))
            ),
            Err(ToolError::Arguments { .. })
        ));
        assert!(matches!(
            call("itest", args(serde_json::json!({"name": "i", "power": true, "destructive": true, "confirm_destructive": true}))),
            Ok(Call::Job(words)) if words.contains(&"--destructive".to_string())
        ));
    }

    #[test]
    fn a_flag_passed_as_a_value_is_refused_before_any_words_exist() {
        for (tool, value, field) in [
            (
                "flash_prepare",
                serde_json::json!({"name": "--attended-motion", "target": "t"}),
                "name",
            ),
            (
                "itest",
                serde_json::json!({"name": "--feeds", "tier": "smoke"}),
                "name",
            ),
            (
                "itest",
                serde_json::json!({"name": "i", "only": ["boot", "--feeds"]}),
                "only",
            ),
            (
                "ota",
                serde_json::json!({"name": "o", "review": "-r"}),
                "review",
            ),
        ] {
            assert!(
                matches!(
                    call(tool, args(value.clone())),
                    Err(ToolError::LeadingDash { field: refused, .. }) if refused == field
                ),
                "{tool} {value}"
            );
        }
    }

    #[test]
    fn a_note_is_built_from_its_arguments_dashes_and_all() {
        assert_eq!(
            call(
                "note_step",
                args(serde_json::json!({"text": "- flash bank 1"}))
            )
            .unwrap(),
            Call::Note(NoteArgs {
                what: What::Step {
                    text: "- flash bank 1".into()
                }
            })
        );
        assert_eq!(
            call(
                "note_next",
                args(serde_json::json!({"steps": ["- boot", "--smoke"]}))
            )
            .unwrap(),
            Call::Note(NoteArgs {
                what: What::Next {
                    steps: vec!["- boot".into(), "--smoke".into()]
                }
            })
        );

        let Call::Note(NoteArgs {
            what: What::Timer(timer),
        }) = call(
            "note_timer",
            args(serde_json::json!({"name": "fire", "in": "5m", "expect": "-- PLSCH1 fire"})),
        )
        .unwrap()
        else {
            panic!("not a timer");
        };
        assert_eq!(
            (
                timer.name.as_str(),
                timer.after.as_deref(),
                timer.expect.as_deref()
            ),
            ("fire", Some("5m"), Some("-- PLSCH1 fire"))
        );

        // The command line's rules still hold.
        for value in [
            serde_json::json!({"name": "t"}),
            serde_json::json!({"name": "t", "in": "1m", "clear": true}),
            serde_json::json!({"name": "t", "in": "1m", "expect": "a", "expect_event": "boot"}),
            serde_json::json!({"name": "t", "clear": true, "expect": "a"}),
        ] {
            assert!(
                matches!(
                    call("note_timer", args(value.clone())),
                    Err(ToolError::Arguments { .. })
                ),
                "{value}"
            );
        }
    }
}
