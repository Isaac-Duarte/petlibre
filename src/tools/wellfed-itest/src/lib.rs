//! Real-device integration suite (`wellfed-bench itest`): named scenarios run
//! against the feeder on the bench, never from `cargo test` or `make check`.
//! `wellfed-bench` parses its command line and hands it to the
//! `wellfed-itest` binary, which runs it through [`Linked`].
//!
//! Builds reach the feeder over the air, which takes minutes where a UART
//! flash takes a supervised loader session. The UART stays attached,
//! receive-only, for the whole run: scenarios assert on its `PL…` records, it
//! is the evidence, and when the feeder goes silent the suite stops and
//! prints the reviewed UART recovery commands instead of acting on its own.
//!
//! Every physical action goes through [`Rig`]: the live rig owns the
//! hotspot, the `wellfed-local` children, the UART captures, the device
//! API sessions (every request, the status, and the updates it pushes on a
//! thread of its own, faults included) and (only with `--power`) the
//! feeder supply relay, and restores what it changed when the run ends.
//! `serve` stays for stock's broker and its updater's trial. Scenarios see
//! only the trait, so they are tested on scripted feeders without hardware.

use std::path::{Path, PathBuf};

use wellfed_bench::commands::ota::{self, Outcome, Staged, Target, Watch};
use wellfed_bench::commands::{Bench, Image, OtaArgs};
use wellfed_bench::device::{Answer, Ask, DeviceStatus, Intruder, Rebuffed};
use wellfed_bench::server::{LocalStatus, TrialRequest};
use wellfed_bench::suite::{ItestArgs, StockOtaArgs, Suite};
use wellfed_bench::{Error, Result, require, say};
use wellfed_formats::bench::VERIFICATION;
use wellfed_formats::bench::catalog::Need;
use wellfed_formats::bench::progress::StepState;
use wellfed_formats::bench::run::{Evidence, RunRecord, SUFFIX, ScenarioRecord, ScenarioVerdict};
use serde::Serialize;

mod feeds;
mod live;
mod log;
mod scenarios;
mod stock;
#[cfg(test)]
mod tests;

use log::{Log, Record};
use scenarios::{DeviceState, SCENARIOS, Scenario};

/// How often waits look at the UART and the server again.
const POLL_SECONDS: f64 = 1.0;
const STATUS_POLL_SECONDS: f64 = 5.0;
/// A build that just started on bank 0 joins the hotspot and answers the
/// device API well within this, a reconnect included.
const HEALTH_SECONDS: f64 = 120.0;
/// A booted feeder joins the hotspot and serves the device API well within
/// this; so does one whose session was dropped by a bounce.
const STATUS_SECONDS: f64 = 120.0;
/// Download, activation, trial boot and the 30 s confirmation take about
/// four minutes; the slowest seen was under six.
const INSTALL_SECONDS: f64 = 15.0 * 60.0;
/// With no record for this long the feeder is presumed hung or bricked.
const SILENT_SECONDS: f64 = 180.0;

/// Whether this run's flags allow a scenario that needs `need`.
fn met(need: Need, args: &ItestArgs) -> bool {
    match need {
        Need::Power => args.power,
        Need::Build => args.build,
        Need::Destructive => args.destructive,
        Need::Stock => args.stock.is_some(),
        Need::Feeds => args.feeds,
    }
}

/// Which fault the bench puts into its own transfer, at the image offsets
/// the drills have always used, and the stage bank 0 stops it at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fault {
    None,
    Corrupt,
    Unwanted,
    Stall,
}

impl Fault {
    /// The bench's own fault, as [`crate::device::Device::install`] takes it.
    fn device(self) -> wellfed_bench::device::Fault {
        use wellfed_bench::device::Fault as D;
        match self {
            Self::None => D::None,
            Self::Corrupt => D::Corrupt { at: 8192 },
            Self::Unwanted => D::Unwanted { at: 200_000 },
            Self::Stall => D::Stall { at: 300_000 },
        }
    }

    /// The `PLOTA1 error stage=` the feeder logs for it.
    fn stage(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Corrupt => Some("hash"),
            Self::Unwanted => Some("order"),
            Self::Stall => Some("timeout"),
        }
    }
}

/// What answers stock on the hotspot's broker address.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Server {
    /// `wellfed-local serve`: broker and API.
    Serve,
    /// `serve --enable-ota-trial`, so it may ask stock to install an image.
    StockTrial,
}

/// Every effect the suite has on the bench. The live rig talks to the real
/// tools; the tests script one.
trait Rig {
    /// Build (when `build`), review and sign the next version: [`Bench::ota_stage`].
    fn stage(&mut self, args: &OtaArgs) -> Result<Option<Staged>>;
    /// A frozen review's startup build ID.
    fn review_build(&mut self, review: &Path) -> Result<String>;
    /// Replace whatever answers the feeder now.
    fn server(&mut self, server: Server) -> Result<()>;
    /// Push `staged` over the device API with `fault` in the transfer, on a
    /// thread of its own, replacing any push still running; `None` stops it.
    fn offer(&mut self, staged: Option<&Staged>, fault: Fault) -> Result<()>;
    /// How the push ended, once it has; `Ok` once bank 1 is activated.
    fn offered(&mut self) -> Option<std::result::Result<(), String>>;
    /// One device API request as the owner's key.
    fn device(&mut self, ask: &Ask) -> Result<Answer>;
    /// Open a second owner session now and hold it: the feeder admits no
    /// handshake during a feed, so only a session opened before one reaches
    /// the feeder while it runs.
    fn hold(&mut self) -> Result<()> {
        Err(Error::Bench("This rig holds no second session".into()))
    }
    /// One request on the held session, never reopened.
    fn held(&mut self, _ask: &Ask) -> Result<Answer> {
        Err(Error::Bench("This rig holds no second session".into()))
    }
    /// Close the held session.
    fn release(&mut self) {}
    /// A connection the feeder must not let in, and how it met it.
    fn intrude(&mut self, intruder: Intruder) -> Result<Rebuffed>;
    /// The host's wall clock, Unix milliseconds: what a clock sync sends.
    fn unix_ms(&self) -> i64;
    /// `GET /api/status`: stock's reports and the install trials.
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
    /// Start `ota-images` serving `review`'s build to the feeder; the URL
    /// token from its lease. Only `stock-ota` uses it.
    fn images(&mut self, _review: &Path) -> Result<String> {
        Err(Error::Bench("This rig serves no images".into()))
    }
    /// `POST /api/ota/trial`: stock gets one `OTA_UPGRADE`, never resent.
    fn stock_trial(&mut self, _trial: &TrialRequest) -> Result<()> {
        Err(Error::Bench("This rig sends no stock trials".into()))
    }
    /// `POST /api/ota/trial/resolve`; refused until the server's evidence allows it.
    fn resolve_trial(&mut self, _request_id: &str, _note: &str) -> Result<()> {
        Err(Error::Bench("This rig resolves no stock trials".into()))
    }
    /// `POST /api/ota/trial/answered`: the replacement answered the device
    /// API after the trial, as `note` says; sends the device nothing.
    fn trial_answered(&mut self, _request_id: &str, _note: &str) -> Result<()> {
        Err(Error::Bench("This rig closes no stock trials".into()))
    }
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
    /// Each image's review, once this run has built or found it.
    images: Vec<(Image, PathBuf)>,
    /// Silence counts from here (the run's start, or the end of an expected
    /// offline window) until a later record.
    heard: f64,
    /// Inside [`Ctx::offline`]: the feeder may be quiet on purpose.
    offline: bool,
    /// Set when a wait gave up because the feeder went silent.
    went_silent: Option<Silent>,
    /// Set by a scenario that left the feeder in a state the suite must not
    /// act on (a feed that may still run): the run stops after it, whatever
    /// `--keep-going` says, and nothing is restored.
    halted: Option<String>,
}

impl<'r> Ctx<'r> {
    fn new(rig: &'r mut dyn Rig, args: &ItestArgs) -> Self {
        Self {
            args: args.clone(),
            log: Log::default(),
            baseline: None,
            images: Vec::new(),
            heard: rig.clock(),
            offline: false,
            went_silent: None,
            halted: None,
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

    /// One device API request, with the UART drained first.
    fn ask(&mut self, ask: &Ask) -> Result<Answer> {
        self.pull()?;
        self.rig.device(ask)
    }

    /// One request on the held session, with the UART drained first.
    fn ask_held(&mut self, ask: &Ask) -> Result<Answer> {
        self.pull()?;
        self.rig.held(ask)
    }

    /// The feeder's status over the device API, now.
    fn device_status(&mut self) -> Result<DeviceStatus> {
        match self.ask(&Ask::Status)? {
            Answer::Status(status) => Ok(status),
            other => Err(Error::Bench(format!("Status answered {}", compact(&other)))),
        }
    }

    /// Poll the device API's status until `check` passes; its report.
    fn until_device(
        &mut self,
        what: &str,
        seconds: f64,
        check: impl Fn(&DeviceStatus) -> Result<String>,
    ) -> Result<String> {
        let deadline = self.rig.clock() + seconds;
        let mut last = String::new();
        loop {
            let verdict = self.device_status().and_then(|status| check(&status));
            match verdict {
                Ok(report) => return Ok(report),
                Err(Error::Interrupted) => return Err(Error::Interrupted),
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

    /// The feeder answering the device API, within `seconds`.
    fn reachable(&mut self, seconds: f64) -> Result<DeviceStatus> {
        self.until_device("the device API", seconds, |_| Ok(String::new()))?;
        self.device_status()
    }

    /// Set the feeder's clock from the host's: nothing else does since the
    /// server stopped sending the sync.
    fn sync_clock(&mut self) -> Result<()> {
        match self.ask(&Ask::SyncClock)? {
            Answer::Done { .. } => Ok(()),
            other => Err(Error::Bench(format!(
                "The clock sync answered {}",
                compact(&other)
            ))),
        }
    }

    /// Push `staged` over the device API and classify the UART until the
    /// update passes, falls back or fails; the device API must then agree.
    fn install(&mut self, staged: &Staged) -> Result<Outcome> {
        // The suite's scenarios need health and a trial, which stock has not.
        require(
            staged.target == Target::Replacement,
            "The integration suite installs replacement builds only; offer stock with `wellfed-bench ota`",
        )?;
        let since = self.mark()?;
        self.rig.offer(Some(staged), Fault::None)?;
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
        self.watch_as(build, Target::Replacement, since, seconds)
    }

    /// As [`Self::watch`] for either target: stock passes on its banner.
    fn watch_as(
        &mut self,
        build: &str,
        target: Target,
        since: usize,
        seconds: f64,
    ) -> Result<Outcome> {
        let mut watch = Watch::of(build, target);
        let mut next = since;
        let deadline = self.rig.clock() + seconds;
        loop {
            self.pull()?;
            while next < self.log.len() {
                let text = format!("{}\n", self.log.line(next).text);
                next += 1;
                for event in watch.feed(text.as_bytes()) {
                    self.note(&format!("  {event}"));
                    // The confirmed build already is the offered one: its
                    // status then says so.
                    if let ota::Event::Offer { same: true, .. } = event {
                        return Ok(Outcome::Passed);
                    }
                }
                if let Some(outcome) = watch.outcome() {
                    return Ok(outcome.clone());
                }
            }
            // A push that ended before any UART outcome: the feeder refused
            // the offer, or reported a stop over the API.
            if let Some(Err(error)) = self.rig.offered() {
                return Ok(Outcome::Failed {
                    stage: format!("device API: {error}"),
                });
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

    /// The device API reports `build` on bank 1.
    fn confirm(&mut self, build: &str) -> Result<()> {
        let report = self.until_device("status from the new build", STATUS_SECONDS, |status| {
            Ok(status.matches(build)?)
        })?;
        self.note(&format!("  {report}"));
        Ok(())
    }

    /// Whether the device API already reports `build` running, without
    /// waiting.
    fn running(&mut self, build: &str) -> bool {
        self.rig.device(&Ask::Status).is_ok_and(
            |answer| matches!(answer, Answer::Status(status) if status.matches(build).is_ok()),
        )
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

    /// The trial firmware's fail variant signed at the next version:
    /// built once, then re-signed from its review.
    fn failing(&mut self, label: &str) -> Result<Staged> {
        self.image(label, Image::trial(wellfed_bench::commands::Variant::Fail))
    }

    /// `image` signed at the next version: built (or found by the review
    /// cache) once, then re-signed from its review.
    fn image(&mut self, label: &str, image: Image) -> Result<Staged> {
        let known = self
            .images
            .iter()
            .find(|(i, _)| *i == image)
            .map(|(_, review)| review.clone());
        let mut args = self.ota_args(label, known);
        if args.build {
            args.firmware = Some(image.firmware);
            args.variant = image.variant;
        }
        let staged = self
            .rig
            .stage(&args)?
            .ok_or_else(|| Error::Bench(format!("Building or signing {image} failed")))?;
        if args.build {
            self.images.push((image, staged.review.clone()));
        }
        Ok(staged)
    }

    /// `ota` arguments that offer `review`, or with none build this checkout.
    fn ota_args(&self, label: &str, review: Option<PathBuf>) -> OtaArgs {
        let build = review.is_none();
        OtaArgs {
            name: format!("{}-{label}", self.args.name()),
            review,
            build,
            rebuild: build && self.args.rebuild,
            // The suite talks to the device API, which only the
            // product firmware serves.
            firmware: build.then_some(wellfed_bench::commands::Firmware::Feeder),
            variant: None,
            config: self.args.config.clone(),
            keys: self.args.keys.clone(),
            version: None,
            minutes: INSTALL_SECONDS / 60.0,
            attended_motion: false,
            downgrade: false,
            fault: None,
            cancel: false,
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
    /// and no device API answer.
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
        let answers = self.rig.device(&Ask::Status).is_ok();
        (!answers).then(|| {
            Silent(format!(
                "no UART record or device API answer for {SILENT_SECONDS:.0} s"
            ))
        })
    }
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

/// The names of the scenarios a run takes on: its status steps.
fn steps(args: &ItestArgs) -> Result<Vec<&'static str>> {
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
    if let Some(need) = scenario.needs.iter().find(|need| !met(**need, &ctx.args)) {
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
        let halted = ctx.halted.take();
        if let Some(reason) = &halted {
            if verdict == Verdict::Pass {
                verdict = Verdict::Fail(reason.clone());
            }
        } else if let Some(found) = ctx.silent() {
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
                if let Some(reason) = &halted {
                    stop = Some(format!("{} stopped the run: {reason}", scenario.name));
                } else if silent.is_some() {
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
        // stock-roundtrip can stop on stock, which reports nothing here.
        "If it stopped on stock: while stock still reports, wellfed-bench stock-ota brings the \
         replacement back over the air; to put stock itself back, wellfed-bench stock-prepare."
            .into(),
    ]
}

/// The suite, linked into the `wellfed-itest` binary.
pub struct Linked;

impl Suite for Linked {
    fn itest_steps(&self, args: &ItestArgs) -> Result<Vec<&'static str>> {
        steps(args)
    }

    fn stock_ota_steps(&self, args: &StockOtaArgs) -> Vec<&'static str> {
        stock::steps(args)
    }

    fn itest(&self, bench: &mut Bench, args: &ItestArgs, local_bin: PathBuf) -> Result<u8> {
        itest(bench, args, local_bin)
    }

    fn stock_ota(&self, bench: &mut Bench, args: &StockOtaArgs, local_bin: PathBuf) -> Result<u8> {
        stock::stock_ota(bench, args, local_bin)
    }
}

/// Run the selected scenarios on the live bench, restore it, then print
/// and record the verdicts.
fn itest(bench: &mut Bench, args: &ItestArgs, local_bin: PathBuf) -> Result<u8> {
    let scenarios = select(args)?;
    let name = args.name().to_string();
    let record = bench
        .root
        .join(VERIFICATION)
        .join(format!("{name}{SUFFIX}"));
    bench.fresh(&record)?;
    std::fs::create_dir_all(bench.root.join(VERIFICATION))?;

    let mut rig = live::LiveRig::open(bench, args, local_bin)?;
    let (result, baseline) = match rig.start(Server::Serve) {
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
            ota::duration(report.seconds)
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
    say!("Recorded {}", bench.rel(&record));
    Ok(exit_code(&reports, silent.as_ref()))
}
