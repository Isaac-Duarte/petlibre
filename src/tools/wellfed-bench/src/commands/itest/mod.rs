//! Real-device integration suite (`wellfed-bench itest`): named scenarios run
//! against the feeder on the bench, never from `cargo test` or `make check`.
//!
//! Builds reach the feeder over the air, which takes minutes where a UART
//! flash takes a supervised loader session. The UART stays attached,
//! receive-only, for the whole run: scenarios assert on its `PL…` records, it
//! is the evidence, and when the feeder goes silent the suite stops and
//! prints the reviewed UART recovery commands instead of acting on its own.
//!
//! Every physical action goes through [`Rig`]: the live rig owns the
//! hotspot, the `wellfed-local` children, the UART captures and (only with
//! `--power`) the feeder supply relay, and restores what it changed when the
//! run ends. Scenarios see only the trait, so they are tested on scripted
//! feeders without hardware.

use std::path::{Path, PathBuf};

use clap::{Args, ValueEnum};
use serde::Serialize;

use super::ota::{Outcome, Staged, Watch};
use crate::config::VERIFICATION;
use crate::server::{ForgedCase, Health, LocalStatus, Reply, Request};
use crate::status::StepState;
use crate::{Error, Result, require};

mod live;
mod log;
pub(crate) mod record;
mod scenarios;
#[cfg(test)]
mod tests;

use log::{Log, Record};
use record::{Evidence, RunRecord, ScenarioRecord, ScenarioVerdict};
use scenarios::{DeviceState, SCENARIOS, Scenario};

/// How often waits look at the UART and the server again.
const POLL_SECONDS: f64 = 1.0;
const STATUS_POLL_SECONDS: f64 = 5.0;
/// A reply normally arrives within a second; a busy settings writer is slower.
const REPLY_SECONDS: f64 = 20.0;
/// The firmware publishes health every 30 s and the server calls it stale
/// after 75 s; three periods covers a reconnect.
const HEALTH_SECONDS: f64 = 120.0;
/// Download, activation, trial boot and the 30 s confirmation take about
/// four minutes; the slowest seen was under six.
const INSTALL_SECONDS: f64 = 15.0 * 60.0;
/// With no record for this long the feeder is presumed hung or bricked.
const SILENT_SECONDS: f64 = 180.0;

/// How much of the suite a run takes on; each tier includes the cheaper ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    /// Minutes, no cold boots or installs beyond the build under test.
    Smoke,
    /// The whole functional suite (about half an hour).
    Standard,
    /// Standard, then long runs (`--soak-minutes`).
    Soak,
}

/// A session flag a scenario cannot run without.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Need {
    Power,
    Build,
    Destructive,
}

impl Need {
    fn flag(self) -> &'static str {
        match self {
            Self::Power => "--power",
            Self::Build => "--build",
            Self::Destructive => "--destructive",
        }
    }

    fn why(self) -> &'static str {
        match self {
            Self::Power => "it cold-boots the feeder",
            Self::Build => "it builds a variant",
            Self::Destructive => "it cuts power during an update",
        }
    }

    fn met(self, args: &ItestArgs) -> bool {
        match self {
            Self::Power => args.power,
            Self::Build => args.build,
            Self::Destructive => args.destructive,
        }
    }
}

#[derive(Debug, Clone, Args)]
pub struct ItestArgs {
    /// Session name: private/captures/NAME-uart-01, …, private/verification/NAME-*.
    #[arg(required_unless_present = "list")]
    pub name: Option<String>,
    /// Frozen review under test; installed over the air unless already running.
    #[arg(long, conflicts_with = "build")]
    pub review: Option<PathBuf>,
    /// Build the PLAF103 firstboot profile from this checkout and freeze it as NAME-good-review first.
    #[arg(long)]
    pub build: bool,
    /// Only these scenarios, comma-separated, whatever their tier; ota-update
    /// always runs first.
    #[arg(long, value_delimiter = ',')]
    pub only: Vec<String>,
    /// How much of the suite to run.
    #[arg(long, value_enum, default_value_t = Tier::Standard)]
    pub tier: Tier,
    /// How long `soak` runs.
    #[arg(long, default_value_t = 60.0)]
    pub soak_minutes: f64,
    /// List the scenarios and what each needs, then exit.
    #[arg(long)]
    pub list: bool,
    /// Allow relay power cycles (cold boots), which some scenarios need.
    #[arg(long)]
    pub power: bool,
    /// Allow power cuts in the middle of an update. The design recovers
    /// from them over the air; if it does not, recovery is a UART flash.
    #[arg(long, requires = "power")]
    pub destructive: bool,
    /// Run the remaining scenarios after one fails (never after the feeder goes silent).
    #[arg(long)]
    pub keep_going: bool,
    /// With --build: build even when a review of the same sources exists.
    #[arg(long, requires = "build")]
    pub rebuild: bool,
    /// Python for --build's review (default: .venv/bin/python under --root).
    #[arg(long)]
    pub python: Option<PathBuf>,
    /// wellfed-local config the suite serves with (root-relative).
    #[arg(long, default_value = crate::local_api::SERVER_CONFIG)]
    pub config: PathBuf,
    /// Hotspot description: its NetworkManager `connection` (root-relative).
    #[arg(long, default_value = "private/hotspot.json")]
    pub hotspot: PathBuf,
}

impl ItestArgs {
    pub(crate) fn name(&self) -> &str {
        self.name.as_deref().unwrap_or("")
    }
}

/// Which `ota_fault_server` fault answers the recovery role's `ota/get`,
/// with the stream offsets the FW10 drills used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fault {
    None,
    Corrupt,
    Range,
    Stall,
}

impl Fault {
    fn arg(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Corrupt => "corrupt",
            Self::Range => "range",
            Self::Stall => "stall",
        }
    }

    fn at(self) -> u32 {
        match self {
            Self::None | Self::Corrupt => 8192,
            Self::Range => 200_000,
            Self::Stall => 300_000,
        }
    }
}

/// What answers the feeder on the hotspot's broker address.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Server {
    /// `wellfed-local serve`: broker, API, health and signed requests.
    Serve,
    /// `wellfed-local broker` alone, for `request_forger`.
    Broker,
    /// The bare broker plus `ota_fault_server` offering `staged`.
    Faulty { fault: Fault, staged: Staged },
}

/// Every effect the suite has on the bench. The live rig talks to the real
/// tools; the tests script one.
trait Rig {
    /// Build (when `build`), review and sign the next version: [`super::Bench::ota_stage`].
    fn stage(&mut self, args: &super::OtaArgs) -> Result<Option<Staged>>;
    /// A frozen review's startup build ID.
    fn review_build(&mut self, review: &Path) -> Result<String>;
    /// Replace whatever answers the feeder now.
    fn server(&mut self, server: Server) -> Result<()>;
    /// Run `request_forger` beside the bare broker; its per-case lines.
    fn forge(&mut self) -> Result<Vec<ForgedCase>>;
    /// Load an offer into `serve`, or clear it.
    fn offer(&mut self, staged: Option<&Staged>) -> Result<()>;
    /// `POST /api/replacement/request`; the request ID.
    fn request(&mut self, request: &Request) -> Result<String>;
    /// `GET /api/status`.
    fn status(&mut self) -> Result<LocalStatus>;
    fn hotspot(&mut self, up: bool) -> Result<()>;
    /// Cut the feeder's supply, wait, restore it: a cold boot.
    fn power_cycle(&mut self) -> Result<()>;
    /// UART bytes received since the last call.
    fn uart(&mut self) -> Result<Vec<u8>>;
    /// An interruptible wait.
    fn wait(&mut self, seconds: f64) -> Result<()>;
    /// Seconds on a monotonic clock.
    fn clock(&self) -> f64;
    /// One line for the operator and the progress page.
    fn note(&mut self, text: &str);
    /// Show a scenario's state on the bench page.
    fn scenario(&mut self, _name: &str, _state: StepState) {}
}

/// The build the suite installed (or found running) and every later
/// scenario returns the feeder to.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Baseline {
    build: String,
    review: PathBuf,
}

/// A failure the suite cannot continue past: the feeder stopped answering.
#[derive(Debug)]
struct Silent(String);

struct Ctx<'r> {
    rig: &'r mut dyn Rig,
    args: ItestArgs,
    log: Log,
    baseline: Option<Baseline>,
    /// Each variant's review, once this run has built or found it.
    variants: Vec<(super::Variant, PathBuf)>,
    /// Silence counts from here (the run's start, or the end of an expected
    /// offline window) until a later record.
    heard: f64,
    /// Inside [`Ctx::offline`]: the feeder may be quiet on purpose.
    offline: bool,
    /// Set when a wait gave up because the feeder went silent.
    went_silent: Option<Silent>,
}

impl<'r> Ctx<'r> {
    fn new(rig: &'r mut dyn Rig, args: &ItestArgs) -> Self {
        Self {
            args: args.clone(),
            log: Log::default(),
            baseline: None,
            variants: Vec::new(),
            heard: rig.clock(),
            offline: false,
            went_silent: None,
            rig,
        }
    }

    fn note(&mut self, text: &str) {
        self.rig.note(text);
    }

    fn pull(&mut self) -> Result<()> {
        let bytes = self.rig.uart()?;
        let at = self.rig.clock();
        self.log.feed(&bytes, at);
        Ok(())
    }

    /// The UART position now: later expectations search from here.
    fn mark(&mut self) -> Result<usize> {
        self.pull()?;
        Ok(self.log.len())
    }

    /// Wait up to `seconds` for a record at or after `since` that `matches`;
    /// the mark just past it.
    fn expect(
        &mut self,
        what: &str,
        since: usize,
        seconds: f64,
        matches: impl Fn(&Record) -> bool,
    ) -> Result<usize> {
        let deadline = self.rig.clock() + seconds;
        loop {
            self.pull()?;
            if let Some(index) = self.log.find(since, &matches) {
                let line = self.log.line(index).text.clone();
                self.note(&format!("  UART: {line}"));
                return Ok(index + 1);
            }
            if self.rig.clock() >= deadline {
                return Err(Error::Bench(format!("No {what} within {seconds:.0} s")));
            }
            self.alive()?;
            self.rig.wait(POLL_SECONDS)?;
        }
    }

    /// Wait up to `seconds` for `n` records at or after `since` that
    /// `matches`; how many there were. The firmware logs a request after
    /// publishing its reply, so its record can trail the reply by a chunk.
    fn expect_count(
        &mut self,
        since: usize,
        n: usize,
        seconds: f64,
        matches: impl Fn(&Record) -> bool,
    ) -> Result<usize> {
        let deadline = self.rig.clock() + seconds;
        loop {
            self.pull()?;
            let found = self.log.count(since, &matches);
            if found >= n || self.rig.clock() >= deadline {
                return Ok(found);
            }
            self.alive()?;
            self.rig.wait(POLL_SECONDS)?;
        }
    }

    /// Poll the server's status until `check` passes; its report.
    fn until_status(
        &mut self,
        what: &str,
        seconds: f64,
        check: impl Fn(&LocalStatus) -> Result<String>,
    ) -> Result<String> {
        let deadline = self.rig.clock() + seconds;
        let mut last = String::new();
        loop {
            // The UART keeps draining, so its captures stay in step.
            self.pull()?;
            let verdict = self.rig.status().and_then(|status| check(&status));
            match verdict {
                Ok(report) => return Ok(report),
                Err(reason) => {
                    let reason = reason.to_string();
                    if reason != last {
                        self.note(&format!("  Waiting for {what}: {reason}"));
                        last = reason;
                    }
                }
            }

            if self.rig.clock() >= deadline {
                return Err(Error::Bench(format!(
                    "No {what} within {seconds:.0} s ({last})"
                )));
            }
            self.alive()?;
            self.rig.wait(STATUS_POLL_SECONDS)?;
        }
    }

    /// Fresh online health from the feeder.
    fn health(&mut self, seconds: f64) -> Result<Health> {
        self.until_status("fresh health", seconds, |status| {
            status.fresh_health()?;
            Ok(String::new())
        })?;

        let status = self.rig.status()?;
        Ok(status.fresh_health()?.clone())
    }

    /// Send one request through the server and wait for the feeder's reply.
    /// Requests go out at QoS 0, so one sent as the transport reconnects is
    /// lost; with no reply it is sent once more. Every request the suite
    /// makes reads, or sets a value again, so a repeat is harmless.
    fn ask(&mut self, request: &Request) -> Result<Reply> {
        match self.ask_once(request) {
            Err(lost @ Error::NoReply { .. }) => {
                self.note(&format!("  {lost}; asking again"));
                self.ask_once(request)
            }
            other => other,
        }
    }

    fn ask_once(&mut self, request: &Request) -> Result<Reply> {
        let id = self.rig.request(request)?;
        let deadline = self.rig.clock() + REPLY_SECONDS;
        loop {
            self.pull()?;
            let status = self.rig.status()?;
            if let Some(reply) = status.reply(&id) {
                return Ok(reply.clone());
            }

            if self.rig.clock() >= deadline {
                return Err(Error::NoReply {
                    op: request.op(),
                    seconds: REPLY_SECONDS,
                });
            }
            self.alive()?;
            self.rig.wait(POLL_SECONDS)?;
        }
    }

    /// Offer `staged` through `serve` and classify the UART until the update
    /// passes, falls back or fails; the server's health must then agree.
    fn install(&mut self, staged: &Staged) -> Result<Outcome> {
        let since = self.mark()?;
        self.rig.offer(Some(staged))?;
        self.note(&format!(
            "  Offered version {} ({})",
            staged.version,
            &staged.build[..12]
        ));
        let outcome = self.watch(&staged.build, since, INSTALL_SECONDS)?;
        if outcome == Outcome::Passed {
            self.confirm(&staged.build)?;
        }
        Ok(outcome)
    }

    /// Classify records from `since` against an offered `build` with the
    /// `ota` command's [`Watch`]; its outcome, or a timeout.
    fn watch(&mut self, build: &str, since: usize, seconds: f64) -> Result<Outcome> {
        let mut watch = Watch::new(build);
        let mut next = since;
        let deadline = self.rig.clock() + seconds;
        loop {
            self.pull()?;
            while next < self.log.len() {
                let text = format!("{}\n", self.log.line(next).text);
                next += 1;
                for event in watch.feed(text.as_bytes()) {
                    self.note(&format!("  {event}"));
                    // The confirmed build already is the offered one: the
                    // server's health then says so.
                    if let super::ota::Event::Offer { same: true, .. } = event {
                        return Ok(Outcome::Passed);
                    }
                }
                if let Some(outcome) = watch.outcome() {
                    return Ok(outcome.clone());
                }
            }
            if self.rig.clock() >= deadline {
                return Err(Error::Bench(format!(
                    "No update outcome for {} within {:.0} min",
                    &build[..12],
                    seconds / 60.0
                )));
            }
            self.alive()?;
            self.rig.wait(POLL_SECONDS)?;
        }
    }

    /// The server sees `build` on bank 1, online and fresh.
    fn confirm(&mut self, build: &str) -> Result<()> {
        let report = self.until_status("health from the new build", HEALTH_SECONDS, |status| {
            Ok(status.health_matches(build)?)
        })?;
        self.note(&format!("  {report}"));
        Ok(())
    }

    /// Whether health already shows `build` running, without waiting.
    fn running(&mut self, build: &str) -> bool {
        self.rig
            .status()
            .is_ok_and(|status| status.health_matches(build).is_ok())
    }

    fn baseline(&self) -> Result<&Baseline> {
        self.baseline
            .as_ref()
            .ok_or_else(|| Error::Bench("No confirmed baseline build".into()))
    }

    /// Re-sign the baseline review at the next version, for a scenario that
    /// needs a fresh offer of the same build.
    fn resign(&mut self, label: &str) -> Result<Staged> {
        let review = self.baseline()?.review.clone();
        let args = self.ota_args(label, Some(review));
        self.rig
            .stage(&args)?
            .ok_or_else(|| Error::Bench(format!("Signing {label} failed")))
    }

    /// The fw11-fail variant signed at the next version: built once, then
    /// re-signed from its review.
    fn failing(&mut self, label: &str) -> Result<Staged> {
        self.variant(label, super::Variant::Fw11Fail)
    }

    /// `variant` signed at the next version: built (or found by the review
    /// cache) once, then re-signed from its review.
    fn variant(&mut self, label: &str, variant: super::Variant) -> Result<Staged> {
        let known = self
            .variants
            .iter()
            .find(|(v, _)| *v == variant)
            .map(|(_, review)| review.clone());
        let mut args = self.ota_args(label, known);
        if args.build {
            args.variant = Some(variant);
        }
        let staged = self.rig.stage(&args)?.ok_or_else(|| {
            Error::Bench(format!(
                "Building or signing the {variant:?} variant failed"
            ))
        })?;
        if args.build {
            self.variants.push((variant, staged.review.clone()));
        }
        Ok(staged)
    }

    /// `ota` arguments that offer `review`, or with none build this checkout.
    fn ota_args(&self, label: &str, review: Option<PathBuf>) -> super::OtaArgs {
        let build = review.is_none();
        super::OtaArgs {
            name: format!("{}-{label}", self.args.name()),
            review,
            build,
            rebuild: build && self.args.rebuild,
            variant: None,
            python: self.args.python.clone(),
            config: self.args.config.clone(),
            version: None,
            minutes: INSTALL_SECONDS / 60.0,
            attended_motion: false,
        }
    }

    /// Let `seconds` pass with the UART drained and the feeder watched.
    fn idle(&mut self, seconds: f64) -> Result<()> {
        let end = self.rig.clock() + seconds;
        while self.rig.clock() < end {
            self.pull()?;
            self.alive()?;
            self.rig
                .wait(POLL_SECONDS.min(end - self.rig.clock()).max(0.0))?;
        }
        Ok(())
    }

    /// Fail a wait early when the feeder has gone silent, rather than let a
    /// 15-minute install watch run out against an unpowered or hung board.
    fn alive(&mut self) -> Result<()> {
        if self.offline {
            return Ok(());
        }
        match self.silent() {
            Some(found) => {
                let reason = found.0.clone();
                self.went_silent = Some(found);
                Err(Error::Bench(reason))
            }
            None => Ok(()),
        }
    }

    /// Run `wait` while the feeder is expected to be unreachable and possibly
    /// quiet (retrying a join it cannot make), bounded only by its own
    /// deadline; silence counts afresh once it ends.
    fn offline<T>(&mut self, wait: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        self.offline = true;
        let result = wait(self);
        self.offline = false;
        self.heard = self.rig.clock();
        result
    }

    /// The feeder looks hung or bricked: no record for [`SILENT_SECONDS`]
    /// and no fresh health.
    fn silent(&mut self) -> Option<Silent> {
        if let Some(found) = self.went_silent.take() {
            return Some(found);
        }

        let _ = self.pull();
        let now = self.rig.clock();
        let last = self.log.last_record_at().unwrap_or(self.heard);
        if now - last.max(self.heard) < SILENT_SECONDS {
            return None;
        }
        let healthy = self
            .rig
            .status()
            .is_ok_and(|status| status.fresh_health().is_ok());
        (!healthy).then(|| {
            Silent(format!(
                "no UART record or fresh health for {SILENT_SECONDS:.0} s"
            ))
        })
    }
}

/// A reply's `ok`, or why not.
fn reply_ok(reply: &Reply) -> Result<()> {
    require(reply.ok, format!("Reply refused: {}", compact(reply)))
}

/// One line of JSON, for a failure message.
fn compact(value: &impl Serialize) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Verdict {
    Pass,
    Fail(String),
    Skip(String),
}

impl Verdict {
    /// As the run's record writes it.
    fn kind(&self) -> ScenarioVerdict {
        match self {
            Self::Pass => ScenarioVerdict::Pass,
            Self::Fail(_) => ScenarioVerdict::Fail,
            Self::Skip(_) => ScenarioVerdict::Skip,
        }
    }

    fn reason(&self) -> Option<&str> {
        match self {
            Self::Pass => None,
            Self::Fail(reason) | Self::Skip(reason) => Some(reason),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Report {
    scenario: &'static str,
    verdict: Verdict,
    seconds: f64,
}

/// One scenario as the page lists it: what it checks and what it needs.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct CatalogEntry {
    pub name: &'static str,
    pub about: &'static str,
    /// The cheapest tier that runs it.
    pub tier: Tier,
    pub needs: Vec<Requirement>,
    /// Installs the build under test; every other scenario needs it.
    pub baseline: bool,
}

/// A session flag a scenario needs, and why.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct Requirement {
    pub flag: &'static str,
    pub why: &'static str,
}

/// The whole suite, in run order.
pub(crate) fn catalog() -> Vec<CatalogEntry> {
    SCENARIOS
        .iter()
        .map(|s| CatalogEntry {
            name: s.name,
            about: s.about,
            tier: s.tier,
            needs: s
                .needs
                .iter()
                .map(|need| Requirement {
                    flag: need.flag(),
                    why: need.why(),
                })
                .collect(),
            baseline: s.baseline,
        })
        .collect()
}

/// The names of the scenarios a run takes on: its status steps.
pub fn steps(args: &ItestArgs) -> Result<Vec<&'static str>> {
    Ok(select(args)?.iter().map(|scenario| scenario.name).collect())
}

/// The scenarios `--only` names, in suite order; ota-update always first.
fn select(args: &ItestArgs) -> Result<Vec<&'static Scenario>> {
    let only = &args.only;
    for name in only {
        require(
            SCENARIOS.iter().any(|s| s.name == name),
            format!(
                "Unknown scenario {name:?}; --list shows them ({})",
                SCENARIOS
                    .iter()
                    .map(|s| s.name)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )?;
    }
    Ok(SCENARIOS
        .iter()
        .filter(|s| match only.is_empty() {
            true => s.baseline || s.tier <= args.tier,
            false => s.baseline || only.iter().any(|n| n == s.name),
        })
        .collect())
}

/// Why a scenario cannot run in this session, if it cannot.
fn unmet(scenario: &Scenario, ctx: &Ctx) -> Option<String> {
    if let Some(need) = scenario.needs.iter().find(|need| !need.met(&ctx.args)) {
        return Some(format!("needs {} ({})", need.flag(), need.why()));
    }
    if !scenario.baseline && ctx.baseline.is_none() {
        return Some("no confirmed baseline build".into());
    }
    None
}

/// Run `scenarios` in order. A failure stops the rest unless `keep_going`;
/// a silent feeder always stops them, with the recovery commands.
fn run(ctx: &mut Ctx, scenarios: &[&'static Scenario]) -> Result<(Vec<Report>, Option<Silent>)> {
    let mut reports = Vec::new();
    let mut stop: Option<String> = None;
    let mut silent = None;
    // Read once the baseline confirms; every later scenario is held to it.
    let mut expected: Option<DeviceState> = None;

    for scenario in scenarios {
        let skip = stop.clone().or_else(|| unmet(scenario, ctx));
        if let Some(reason) = skip {
            ctx.note(&format!("- {}: skipped, {reason}", scenario.name));
            ctx.rig.scenario(scenario.name, StepState::Skipped);
            reports.push(Report {
                scenario: scenario.name,
                verdict: Verdict::Skip(reason),
                seconds: 0.0,
            });
            continue;
        }

        ctx.note(&format!("- {}: {}", scenario.name, scenario.about));
        ctx.rig.scenario(scenario.name, StepState::Running);
        let started = ctx.rig.clock();
        let result = (scenario.run)(ctx);
        let seconds = ctx.rig.clock() - started;

        let mut verdict = match result {
            Ok(()) => Verdict::Pass,
            Err(Error::Interrupted) => return Err(Error::Interrupted),
            Err(error) => Verdict::Fail(error.to_string()),
        };
        if let Some(found) = ctx.silent() {
            if verdict == Verdict::Pass {
                verdict = Verdict::Fail(found.0.clone());
            }
            silent = Some(found);
        } else if scenario.baseline {
            expected = scenarios::device_state(ctx).ok();
        } else if let Some(want) = &expected {
            let leaked = match scenarios::device_state(ctx) {
                Ok(now) if now == *want => None,
                Ok(now) => Some(format!(
                    "left the feeder changed: {}",
                    scenarios::restore_state(ctx, want, &now).join("; ")
                )),
                Err(Error::Interrupted) => return Err(Error::Interrupted),
                Err(error) => Some(format!(
                    "could not read the feeder's state after it: {error}"
                )),
            };
            verdict = match (verdict, leaked) {
                (verdict, None) => verdict,
                (Verdict::Fail(reason), Some(leaked)) => {
                    Verdict::Fail(format!("{reason}; {leaked}"))
                }
                (_, Some(leaked)) => Verdict::Fail(leaked),
            };
        }

        let shown = if verdict == Verdict::Pass {
            StepState::Done
        } else {
            StepState::Failed
        };
        ctx.rig.scenario(scenario.name, shown);
        match &verdict {
            Verdict::Pass => ctx.note(&format!("  {}: pass", scenario.name)),
            Verdict::Fail(reason) => {
                ctx.note(&format!("  {}: FAIL: {reason}", scenario.name));
                if silent.is_some() {
                    stop = Some(format!("the feeder went silent in {}", scenario.name));
                } else if scenario.baseline {
                    stop = Some("the baseline build did not install".into());
                } else if !ctx.args.keep_going {
                    stop = Some(format!("{} failed", scenario.name));
                }
            }
            Verdict::Skip(_) => {}
        }
        reports.push(Report {
            scenario: scenario.name,
            verdict,
            seconds,
        });
    }
    Ok((reports, silent))
}

/// The run's record: per-scenario verdicts beside the captures.
fn summary(
    name: &str,
    reports: &[Report],
    silent: Option<&Silent>,
    captures: &[String],
) -> RunRecord {
    let scenarios = reports
        .iter()
        .map(|report| ScenarioRecord {
            scenario: report.scenario.into(),
            verdict: report.verdict.kind(),
            reason: report.verdict.reason().map(str::to_string),
            seconds: (report.seconds * 10.0).round() / 10.0,
        })
        .collect();

    RunRecord {
        name: name.into(),
        evidence: Evidence::Physical,
        scenarios,
        silent: silent.map(|s| s.0.clone()),
        captures: captures.to_vec(),
    }
}

/// Exit status: 0 when nothing failed, 1 on a failure, 2 when the feeder
/// went silent and needs UART recovery.
fn exit_code(reports: &[Report], silent: Option<&Silent>) -> u8 {
    if silent.is_some() {
        2
    } else if reports
        .iter()
        .any(|r| matches!(r.verdict, Verdict::Fail(_)))
    {
        1
    } else {
        0
    }
}

/// The operator's way back when the feeder stops answering: the reviewed
/// UART tools, never run by the suite itself.
fn recovery_hint(name: &str, baseline: Option<&Baseline>) -> Vec<String> {
    let target = baseline.map_or_else(
        || "<the last confirmed review>".to_string(),
        |b| b.review.display().to_string(),
    );
    vec![
        "The feeder stopped answering. Recover it over UART with the reviewed tools:".into(),
        format!("  wellfed-bench flash-prepare {name}-recover --target {target}"),
        "  then review the plan and run flash-execute with the --expected-plan-sha256 it printed."
            .into(),
    ]
}

/// `--list`: each scenario with what it needs.
pub fn listing() -> Vec<String> {
    catalog()
        .iter()
        .map(|s| {
            let mut needs: Vec<&str> = s.needs.iter().map(|need| need.flag).collect();
            match s.tier {
                Tier::Smoke => {}
                Tier::Standard => needs.insert(0, "standard"),
                Tier::Soak => needs.insert(0, "soak"),
            }
            let needs = if needs.is_empty() {
                String::new()
            } else {
                format!(" [{}]", needs.join(" "))
            };
            format!("{:<18} {}{needs}", s.name, s.about)
        })
        .collect()
}

impl super::Bench {
    /// Run the selected scenarios on the live bench, restore it, then print
    /// and record the verdicts.
    pub fn itest(&mut self, args: &ItestArgs, local_bin: PathBuf) -> Result<u8> {
        let scenarios = select(args)?;
        let name = args.name().to_string();
        let record = self
            .root
            .join(VERIFICATION)
            .join(format!("{name}{}", record::SUFFIX));
        self.fresh(&record)?;
        std::fs::create_dir_all(self.root.join(VERIFICATION))?;

        let mut rig = live::LiveRig::open(self, args, local_bin)?;
        let (result, baseline) = match rig.start() {
            Ok(()) => {
                let mut ctx = Ctx::new(&mut rig, args);
                let result = run(&mut ctx, &scenarios);
                (result, ctx.baseline.take())
            }
            Err(error) => (Err(error), None),
        };
        rig.restore();
        let captures = rig.captures.clone();
        drop(rig);

        let (reports, silent) = result?;
        say!("Results:");
        for report in &reports {
            let verdict = match report.verdict {
                Verdict::Fail(_) => "FAIL",
                ref other => other.kind().as_str(),
            };
            let reason = report.verdict.reason().unwrap_or("");
            say!(
                "  {:<18} {verdict:<5} {:>9}  {reason}",
                report.scenario,
                super::ota::duration(report.seconds)
            );
        }
        if let Some(silent) = &silent {
            say!("Stopped: {}", silent.0);
            for line in recovery_hint(&name, baseline.as_ref()) {
                say!("{line}");
            }
        }

        let summary = summary(&name, &reports, silent.as_ref(), &captures);
        std::fs::write(&record, serde_json::to_string_pretty(&summary)? + "\n")?;
        say!("Recorded {}", self.rel(&record));
        Ok(exit_code(&reports, silent.as_ref()))
    }
}
