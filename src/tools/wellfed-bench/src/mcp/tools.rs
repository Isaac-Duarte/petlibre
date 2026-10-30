//! The MCP tools: their names, what each physically does, their arguments,
//! and the command line each hardware tool posts.
//!
//! A hardware tool builds the same words a person types and posts them as a
//! job, so the server's parse and every check behind it apply unchanged.
//! What a tool cannot express is refused here, before any words exist:
//! motion that needs a person at the bench (`--attended-motion`, the
//! suite's `--feeds`), breaking the lease, and a destructive suite run
//! without its explicit confirmation, and an answer to an install's
//! question that only a person at the feeder may give: whether it is the
//! owner's feeder, and that its recovery window is open. No string a
//! hardware tool takes may
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
use wellfed_formats::bench::prompt::{Answer, Prompt};

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
    #[error(
        "{tool}: a dispense press is refused over MCP without confirm_motion: outside a setup or enrollment window it is a feed on an unlocked panel; with the hopper empty, set confirm_motion to true to mean it"
    )]
    UnconfirmedMotion { tool: String },
    /// The job's own presses include Dispense: the update key's
    /// enrollment, and any setup code with Dispense in it.
    #[error(
        "{tool}: virtual_panel is refused over MCP without confirm_motion: the job presses Dispense itself (the update key's enrollment, and any setup code that holds it), which outside a window is a feed on an unlocked panel; with the hopper empty, set confirm_motion to true to mean it"
    )]
    UnconfirmedPanel { tool: String },
    #[error(
        "{tool}: a reset hold over 1000 ms is refused over MCP without confirm_destructive: one of 5 s on an unlocked panel erases the feeder's enrolled keys, the bench's included; set confirm_destructive to true to mean it"
    )]
    UnconfirmedReset { tool: String },
    #[error("{tool}: a press names each button once")]
    RepeatedButton { tool: String },
    #[error("{tool}: a press names at least one button")]
    NoButton { tool: String },
    /// The owner's identity check, and a step at the feeder's panel: an
    /// agent saying yes or done would stand in for a person who may not
    /// be there.
    #[error(
        "job_answer: the {name} question is the owner's to answer, at the feeder: answer it on the bench page or with `wellfed-bench answer`"
    )]
    PersonAnswers { name: &'static str },
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
    /// An answer to a job's question: the running job's unless `id`.
    JobAnswer {
        id: Option<u64>,
        seq: u32,
        answer: Answer,
    },
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
    /// A job asks a question (install), already or once it does.
    Prompt,
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

/// An answer, as `job_answer` takes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum Reply {
    /// The instruction is done.
    Done,
    Yes,
    No,
}

#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AnswerArgs {
    /// The job; the running one if left out.
    id: Option<u64>,
    /// The question's number, as bench_status's prompt gives it.
    seq: u32,
    answer: Reply,
}

#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RestoreStockArgs {
    /// A stock 1.1.15 image to restore instead of the kept backup; it must
    /// match the pinned image exactly.
    backup: Option<String>,
    /// An update-signing key the feeder already trusts, such as the bench's.
    /// Otherwise a fresh key is made and enrolled with a Dispense press.
    key: Option<String>,
    /// The version the stock manifest carries; otherwise one above the
    /// feeder's floor.
    version: Option<u32>,
    /// The job presses Dispense itself on a running virtual-panel build.
    #[serde(default)]
    virtual_panel: bool,
    /// Required with virtual_panel: the hopper is empty.
    #[serde(default)]
    confirm_motion: bool,
    /// This computer's client key; the tool's default otherwise.
    keys: Option<String>,
    /// The feeder's identity, as a hex prefix, when the key file knows more
    /// than one.
    feeder: Option<String>,
    /// The feeder's address; found over mDNS otherwise.
    address: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct InstallArgs {
    /// The replacement's frozen review, root-relative.
    review: String,
    /// The signed update manifest for the same build; without it the
    /// installer signs with update_key (or, for a bench review, the bench
    /// key).
    manifest: Option<String>,
    /// The update-signing key to sign with and enroll by a Dispense press.
    update_key: Option<String>,
    /// A consumer image's network: a private file of its ssid and password
    /// (private/hotspot.json on the bench).
    home_network: Option<String>,
    /// The feeder's Bluetooth address for a consumer image's setup.
    ble_address: Option<String>,
    /// Stock's MQTT client ID, pinned by the owner: the feeder check passes
    /// for it without a question and stops the run for any other.
    expect_device: Option<String>,
    /// The job answers the installer's panel prompts itself on a running
    /// virtual-panel build: the setup code's presses and the Dispense press.
    #[serde(default)]
    virtual_panel: bool,
    /// Required with virtual_panel, whose presses include Dispense: the
    /// hopper is empty.
    #[serde(default)]
    confirm_motion: bool,
    /// The installer's wellfed-stock config (init-installer).
    stock_config: String,
    /// Stock's broker name, which the owner's resolver points here.
    broker_name: Option<String>,
    /// The replacement is already on bank 0: start at pairing.
    #[serde(default)]
    from_bank0: bool,
    enroll_minutes: Option<u64>,
    #[serde(default)]
    calibrate_clock: bool,
    /// The owner key file; the installer's default otherwise.
    keys: Option<String>,
    /// The replacement's address.
    address: Option<String>,
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

/// A panel button, as `wellfed-bench press` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum PanelButton {
    Reset,
    Mic,
    Dispense,
    Lock,
}

impl PanelButton {
    fn word(self) -> &'static str {
        match self {
            Self::Reset => "reset",
            Self::Mic => "mic",
            Self::Dispense => "dispense",
            Self::Lock => "lock",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PressArgs {
    /// The chord: one button, or several held together (lock and dispense
    /// open the recovery window).
    #[schemars(length(min = 1, max = 4))]
    buttons: Vec<PanelButton>,
    /// How long the chord is held, 1 to 15000 ms (default 150: a press).
    #[schemars(range(min = 1, max = 15000))]
    hold_ms: Option<u32>,
    /// Required for a dispense press, which feeds outside a setup or
    /// enrollment window: the hopper is empty.
    #[serde(default)]
    confirm_motion: bool,
    /// Required for a reset hold over 1000 ms: one of 5 s erases the
    /// enrolled keys on an unlocked panel, and a late poll can count a
    /// hold just short of it as reaching it.
    #[serde(default)]
    confirm_destructive: bool,
}

/// The longest Reset hold allowed without `confirm_destructive`: far
/// enough below the panel's 5 s factory restore that no stall in the
/// button task can carry a hold to it.
const RESET_HOLD_MS: u32 = 1_000;

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
        description: "Hold until a condition holds or the timeout passes: a job's end, a UART/device log/job line containing text, a device event kind, the feeder's status naming a build or bank, an operator timer falling due, an idle bench, or a job's question (prompt). Reads only.",
        schema: schema::<WaitArgs>,
    },
    Spec {
        name: "job_answer",
        description: "Answer the question the running install asks (bench_status's prompt, by its seq): done for an instruction such as setting the DNS override, yes or no for a question. Sent once, never ahead of a question. Refused for the owner's own questions (is this your feeder, the recovery window at the panel), which a person answers on the page or with the CLI.",
        schema: schema::<AnswerArgs>,
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
        name: "press",
        description: "Hold panel buttons together on a running virtual-panel build, typed on its UART: the same gesture as fingers, for setup codes, the recovery window (lock and dispense held 5 s, on a panel a 5 s lock hold unlocked first; it relocks after 30 s idle) and update-key enrollment. Refused unless the build echoes the press and reports its release. Its effects are a finger's: a 5 s Lock hold unlocks the panel; any press with Dispense can feed (needs confirm_motion, hopper empty); a Reset hold over 1 s needs confirm_destructive, as one of 5 s on an unlocked panel erases the enrolled keys. Opening the recovery window does not let an agent answer an install's recovery question: that stays a person's.",
        schema: schema::<PressArgs>,
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
    Spec {
        name: "install",
        description: "Stock to Wellfed with `wellfed install` as a job: its own stock server and sessions (the server's session pauses), stock installs the review into bank 0 over its own updater, then promotion writes bank 1. Restarts the feeder itself with relay 1 (off about 10 s, then on). With virtual_panel (needs confirm_motion) it also keys a consumer image's setup code in and presses Dispense for the update key's enrollment on a running virtual-panel build, as press does, and a failed press stops the install. Its other questions wait in bench_status's prompt for job_answer or a person (expect_device answers the feeder check ahead, for the pinned device only); no flash write over UART.",
        schema: schema::<InstallArgs>,
    },
    Spec {
        name: "restore_stock",
        description: "Put stock back from the kept backup with `wellfed restore-stock` as a job: its own sessions (the server's session pauses), a fresh update key enrolled with a Dispense press unless key names one the feeder trusts, then bank 1 gives way and bank 0 writes stock. With virtual_panel (needs confirm_motion) it presses Dispense itself on a running virtual-panel build, and a failed press stops the job; otherwise the press waits in bench_status's prompt for a person. Nothing is retried or offered again: a feeder that stops answering after the offer ends the job uncertain, and stock serves no device API to ask. No flash write over UART.",
        schema: schema::<RestoreStockArgs>,
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
        "job_answer" => {
            let args: AnswerArgs = parse(name, args)?;
            Call::JobAnswer {
                id: args.id,
                seq: args.seq,
                answer: match args.answer {
                    Reply::Done => Answer::Done,
                    Reply::Yes => Answer::Yes,
                    Reply::No => Answer::No,
                },
            }
        }
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

/// Refused for a question only the owner may answer, at the feeder.
pub(crate) fn agent_may_answer(prompt: &Prompt) -> Result<(), ToolError> {
    match prompt {
        Prompt::RecognizeFeeder { .. } | Prompt::OpenRecoveryWindow => {
            Err(ToolError::PersonAnswers {
                name: prompt.name(),
            })
        }
        // The feeder's own check of the code or the press decides, not the
        // answer, which only lets the installer ask its next question.
        Prompt::SetOverride { .. }
        | Prompt::RestartFeeder
        | Prompt::SetupMode
        | Prompt::JoinBack
        | Prompt::KeySetupCode { .. }
        | Prompt::PressDispense { .. } => Ok(()),
    }
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
        "press" => {
            let args: PressArgs = parse(name, args)?;
            let has = |button| args.buttons.contains(&button);
            let mut words: Vec<&str> = args.buttons.iter().map(|b| b.word()).collect();
            words.sort_unstable();
            words.dedup();
            if args.buttons.is_empty() {
                return Err(ToolError::NoButton { tool: name.into() });
            }
            if words.len() != args.buttons.len() {
                return Err(ToolError::RepeatedButton { tool: name.into() });
            }
            if has(PanelButton::Dispense) && !args.confirm_motion {
                return Err(ToolError::UnconfirmedMotion { tool: name.into() });
            }
            if has(PanelButton::Reset)
                && args.hold_ms.is_some_and(|ms| ms > RESET_HOLD_MS)
                && !args.confirm_destructive
            {
                return Err(ToolError::UnconfirmedReset { tool: name.into() });
            }
            let chord = args
                .buttons
                .iter()
                .map(|b| b.word())
                .collect::<Vec<_>>()
                .join("+");
            Line::new(&["press"])
                .value("--hold-ms", args.hold_ms)
                .word(chord)
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
        "install" => {
            let args: InstallArgs = parse(name, args)?;
            if args.virtual_panel && !args.confirm_motion {
                return Err(ToolError::UnconfirmedPanel { tool: name.into() });
            }
            Line::new(&["install"])
                .value("--review", Some(args.review))
                .value("--manifest", args.manifest)
                .value("--update-key", args.update_key)
                .value("--home-network", args.home_network)
                .value("--ble-address", args.ble_address)
                .value("--expect-device", args.expect_device)
                .switch("--virtual-panel", args.virtual_panel)
                .value("--stock-config", Some(args.stock_config))
                .value("--broker-name", args.broker_name)
                .switch("--from-bank0", args.from_bank0)
                .value("--enroll-minutes", args.enroll_minutes)
                .switch("--calibrate-clock", args.calibrate_clock)
                .value("--keys", args.keys)
                .value("--address", args.address)
        }
        "restore_stock" => {
            let args: RestoreStockArgs = parse(name, args)?;
            if args.virtual_panel && !args.confirm_motion {
                return Err(ToolError::UnconfirmedPanel { tool: name.into() });
            }
            Line::new(&["restore-stock"])
                .value("--backup", args.backup)
                .value("--key", args.key)
                .value("--version", args.version)
                .switch("--virtual-panel", args.virtual_panel)
                .value("--keys", args.keys)
                .value("--feeder", args.feeder)
                .value("--address", args.address)
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
            "press",
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
            "install",
            "job_answer",
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
    fn install_builds_its_command_line() {
        let install = call(
            "install",
            args(serde_json::json!({
                "review": "private/verification/r",
                "manifest": "private/m.json",
                "stock_config": "private/installer.json",
                "broker_name": "broker.example.net",
                "calibrate_clock": true,
            })),
        )
        .unwrap();
        assert_eq!(
            install,
            Call::Job(
                [
                    "install",
                    "--review",
                    "private/verification/r",
                    "--manifest",
                    "private/m.json",
                    "--stock-config",
                    "private/installer.json",
                    "--broker-name",
                    "broker.example.net",
                    "--calibrate-clock",
                ]
                .map(String::from)
                .to_vec()
            )
        );
        assert!(matches!(
            call(
                "install",
                args(serde_json::json!({"review": "-r", "manifest": "m", "stock_config": "c"}))
            ),
            Err(ToolError::LeadingDash { .. })
        ));

        let restore = serde_json::json!({"virtual_panel": true});
        assert!(matches!(
            call("restore_stock", args(restore)),
            Err(ToolError::UnconfirmedPanel { .. })
        ));
        assert_eq!(
            call(
                "restore_stock",
                args(serde_json::json!({"virtual_panel": true, "confirm_motion": true}))
            )
            .unwrap(),
            Call::Job(
                ["restore-stock", "--virtual-panel"]
                    .map(String::from)
                    .to_vec()
            )
        );

        // The unattended consumer run: the job presses Dispense itself, so
        // it needs the motion confirmed.
        let consumer = serde_json::json!({
            "review": "private/verification/c",
            "update_key": "private/ota-key.json",
            "home_network": "private/hotspot.json",
            "ble_address": "AA:BB:CC:DD:EE:FF",
            "expect_device": "AF_FEEDER",
            "stock_config": "private/installer.json",
            "virtual_panel": true,
        });
        assert!(matches!(
            call("install", args(consumer.clone())),
            Err(ToolError::UnconfirmedPanel { .. })
        ));
        let mut confirmed = consumer;
        confirmed["confirm_motion"] = true.into();
        assert_eq!(
            call("install", args(confirmed)).unwrap(),
            Call::Job(
                [
                    "install",
                    "--review",
                    "private/verification/c",
                    "--update-key",
                    "private/ota-key.json",
                    "--home-network",
                    "private/hotspot.json",
                    "--ble-address",
                    "AA:BB:CC:DD:EE:FF",
                    "--expect-device",
                    "AF_FEEDER",
                    "--virtual-panel",
                    "--stock-config",
                    "private/installer.json",
                ]
                .map(String::from)
                .to_vec()
            )
        );
        assert_eq!(
            call(
                "job_answer",
                args(serde_json::json!({"seq": 2, "answer": "done"}))
            )
            .unwrap(),
            Call::JobAnswer {
                id: None,
                seq: 2,
                answer: Answer::Done
            }
        );
    }

    /// The owner's identity check and the panel's recovery window need a
    /// person; the instructions an agent can carry out do not.
    #[test]
    fn an_agent_may_not_answer_the_owner_s_own_questions() {
        for prompt in [
            Prompt::RecognizeFeeder {
                device: "AF".into(),
                address: "192.168.1.31".into(),
            },
            Prompt::OpenRecoveryWindow,
        ] {
            assert!(
                matches!(agent_may_answer(&prompt), Err(ToolError::PersonAnswers { name }) if name == prompt.name()),
                "{prompt:?}"
            );
        }
        for prompt in [
            Prompt::SetOverride {
                name: "broker.example.net".into(),
                broker: "192.168.1.20".parse().unwrap(),
            },
            Prompt::JoinBack,
            Prompt::PressDispense {
                purpose: wellfed_formats::bench::prompt::DispensePurpose::EnrollUpdateKey,
            },
        ] {
            assert!(agent_may_answer(&prompt).is_ok(), "{prompt:?}");
        }
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
    fn a_press_that_can_feed_or_erase_keys_needs_its_confirmation() {
        let press = |value| call("press", args(value));
        assert!(matches!(
            press(serde_json::json!({"buttons": ["dispense"]})),
            Err(ToolError::UnconfirmedMotion { .. })
        ));
        assert!(matches!(
            press(serde_json::json!({"buttons": ["lock", "dispense"], "hold_ms": 5200})),
            Err(ToolError::UnconfirmedMotion { .. })
        ));
        assert!(matches!(
            press(serde_json::json!({"buttons": ["lock", "dispense"], "hold_ms": 5200, "confirm_motion": true})),
            Ok(Call::Job(words)) if words == ["press", "--hold-ms", "5200", "--", "lock+dispense"]
        ));
        assert!(matches!(
            press(serde_json::json!({"buttons": ["reset"], "hold_ms": 1001})),
            Err(ToolError::UnconfirmedReset { .. })
        ));
        assert!(matches!(
            press(serde_json::json!({"buttons": ["reset"], "hold_ms": 1000})),
            Ok(Call::Job(_))
        ));
        assert!(matches!(
            press(serde_json::json!({"buttons": []})),
            Err(ToolError::NoButton { .. })
        ));
        assert!(matches!(
            press(serde_json::json!({"buttons": ["mic", "mic"]})),
            Err(ToolError::RepeatedButton { .. })
        ));
        assert!(matches!(
            press(serde_json::json!({"buttons": ["lock"], "hold_ms": 5200})),
            Ok(Call::Job(words)) if words == ["press", "--hold-ms", "5200", "--", "lock"]
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
