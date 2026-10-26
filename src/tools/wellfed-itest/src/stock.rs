//! `wellfed-bench stock-ota`: have stock 1.1.15 install a replacement build
//! over its own MQTT-triggered HTTP updater (the stock-to-replacement POC).
//!
//! The feeder is assumed to run stock, and the run proves it before asking
//! anything: the server must see stock report 1.1.15 on its adapter, which is
//! also the server's own gate for the request. Stock then fetches the build
//! from `ota-images`, checks only its byte-sum, writes it to bank 0 (the bank
//! it is not running from) and invalidates bank 1 ([stock OTA requirements]).
//! The factory loader then boots the build from bank 0, which answers the
//! device API (and feeds) while it waits for an update; the promoted build
//! on bank 1 is confirmed over the device API. Stock reports nothing once it
//! has activated the build, and the build speaks no MQTT, so once its
//! device-API status names the build on bank 0 the run tells the server,
//! which closes the request as `replacement_healthy`.
//!
//! The request goes out once and is never resent: a rejected stock precheck
//! can erase the stock OTA status sector, and an uncertain one may still be
//! downloading. Then the same build is pushed as a signed update, so bank 0
//! installs it into bank 1 and the feeder ends in the normal two-bank state
//! (`--no-promote` stops at bank 0).
//!
//! Stock reaches this server only through `wellfed-stock network apply`, a
//! host firewall change the operator makes and removes by hand; the run never
//! touches routing. The way back to stock is the UART stock recovery.
//!
//! [stock OTA requirements]: ../../../../docs/devices/plaf103/stock/ota-requirements.md

use std::path::PathBuf;
use std::time::Duration;

use crate::args::{ItestArgs, StockOtaArgs};
use wellfed_core::ota::install::StopStage;
use wellfed_core::record::Event;
use wellfed_formats::bench::progress::StepState;
use wellfed_formats::bench::run::SUFFIX;
use wellfed_formats::bench::{VERIFICATION, catalog::Tier};
use wellfed_formats::stock::{OtaReport, TrialPhase};
use wellfed_rig::commands::Bench;
use wellfed_rig::commands::ota::{self, OtaArgs, Outcome, Staged, Target, Timing};
use wellfed_rig::say;
use wellfed_rig::server::{STOCK_VERSION, TrialAnswer, TrialRequest};

use super::log::events;
use super::{
    Ctx, Fault, HEALTH_WAIT, INSTALL_WAIT, Report, Result, STATUS_POLL, STATUS_WAIT, Server,
    Silent, Verdict, live, short,
};

/// Why the trip through stock, or a `stock-ota` stage, stopped. Stock is
/// asked once and never again, so none of these is retried.
#[derive(Debug, thiserror::Error)]
pub enum StockError {
    #[error("stock-roundtrip needs --stock STOCK_REVIEW")]
    NoStockReview,
    #[error("Signing the stock review failed")]
    StockUnsigned,
    #[error("{} is not a stock review", review.display())]
    NotStock { review: PathBuf },
    #[error("Stock did not start: {outcome:?}")]
    StockNotStarted { outcome: Outcome },
    #[error("Signing the baseline again failed")]
    BaselineUnsigned,
    #[error("Building or signing the payload failed")]
    PayloadUnstaged,
    #[error("The server has no record of the request")]
    NoRequest,
    /// `report` is stock's last OTA report, if the server kept one.
    #[error(
        "Stock refused or failed the install ({})",
        report.as_ref().map_or_else(|| "no report".into(), ToString::to_string)
    )]
    InstallRefused { report: Option<OtaReport> },
    #[error("The build did not start within {:.0} min", INSTALL_WAIT.as_secs_f64() / 60.0)]
    NotStarted,
    #[error("Unreadable startup record")]
    UnreadableStart,
    #[error("Build {} started, not {}", short(reported), short(build))]
    WrongBuild { reported: String, build: String },
    #[error("The build started on bank {bank:?}, not 0")]
    WrongBank { bank: Option<u8> },
    /// A wait's reason while the device API reports another build.
    #[error("build {build}")]
    OtherBuild { build: String },
    /// A wait's reason while the device API reports another bank.
    #[error("bank {bank}")]
    OtherBank { bank: u8 },
    #[error("Bank 1 fell back to bank 0 after trial boot {boot:?}")]
    PromotionFellBack { boot: Option<u32> },
    #[error("The promotion failed at {stage}")]
    PromotionFailed { stage: StopStage },
    #[error("The promotion's attempt reset; bank 0 will not retry until a cold boot")]
    PromotionSkipped,
    #[error(
        "The server did not resolve {id} within {:.0} min",
        SETTLE_WAIT.as_secs_f64() / 60.0
    )]
    Unresolved { id: String },
}

/// Stock joins the hotspot, connects and reports its attributes within a
/// minute or two of power; allow a slow join.
const STOCK_WAIT: Duration = Duration::from_secs(5 * 60);
/// The server's resolution window (20 min after the request, 10 quiet
/// connected minutes) plus margin.
const SETTLE_WAIT: Duration = Duration::from_secs(25 * 60);
const SETTLE_POLL: Duration = Duration::from_secs(30);

/// The run's stages, in order: its status steps and record entries.
const READY: &str = "stock-ready";
const SETTLE: &str = "stock-settle";
const REQUEST: &str = "stock-request";
const DOWNLOAD: &str = "stock-install";
const BANK0: &str = "bank0-health";
const PROMOTE: &str = "promote";

/// The stages this run takes on.
pub(super) fn steps(args: &StockOtaArgs) -> Vec<&'static str> {
    let mut steps = vec![READY, SETTLE, REQUEST, DOWNLOAD, BANK0];
    if !args.no_promote {
        steps.push(PROMOTE);
    }
    steps
}

/// The `ota` arguments that build and sign the payload. Stock ignores
/// the signed manifest; the promotion offers it.
fn ota_args(args: &StockOtaArgs) -> OtaArgs {
    OtaArgs {
        name: args.name.clone(),
        review: None,
        build: true,
        rebuild: args.rebuild,
        firmware: args.firmware,
        variant: args.variant,
        config: args.config.clone(),
        keys: args.keys.clone(),
        version: None,
        minutes: INSTALL_WAIT.as_secs_f64() / 60.0,
        attended_motion: false,
        downgrade: false,
        fault: None,
        cancel: false,
    }
}

/// The suite's arguments, for the live rig and the shared waits.
fn rig_args(args: &StockOtaArgs) -> ItestArgs {
    ItestArgs {
        name: Some(args.name.clone()),
        review: None,
        build: false,
        only: Vec::new(),
        tier: Tier::Smoke,
        soak_minutes: 0.0,
        list: false,
        power: args.power,
        destructive: false,
        feeds: false,
        stock: None,
        keep_going: false,
        rebuild: false,
        config: args.config.clone(),
        hotspot: args.hotspot.clone(),
        keys: args.keys.clone(),
    }
}

/// `itest` scenario `stock-roundtrip`: from the confirmed baseline to stock
/// 1.1.15 over our own signed update, then back over stock's updater
/// and bank 0's promotion. It ends where it began:
/// the baseline confirmed on bank 1, `serve` answering. Bank 0 then holds
/// the baseline too, written by stock.
pub(super) fn roundtrip(ctx: &mut Ctx) -> Result<()> {
    let baseline = ctx.baseline()?.clone();
    // `unmet` skips the scenario without it.
    let review = ctx.args.stock.clone().ok_or(StockError::NoStockReview)?;

    // To stock: our updater writes it into bank 1, then its banner shows.
    let to_stock = ctx.ota_args("stock", Some(review.clone()));
    let stock = ctx.rig.stage(&to_stock)?.ok_or(StockError::StockUnsigned)?;
    if stock.target != Target::Stock {
        return Err(StockError::NotStock { review }.into());
    }
    let since = ctx.mark()?;
    ctx.rig.offer(Some(&stock), Fault::None)?;
    ctx.note("  Offered stock with a downgrade statement; bank 0 installs it into bank 1");
    let outcome =
        ctx.offline(|ctx| ctx.watch_as(&stock.build, Target::Stock, since, INSTALL_WAIT))?;
    if outcome != Outcome::Passed {
        return Err(StockError::StockNotStarted { outcome }.into());
    }
    ctx.rig.offer(None, Fault::None)?;

    // On stock: it reports to a trial-enabled server, routed by the operator.
    ctx.rig.server(Server::StockTrial)?;
    ctx.note(
        "  Stock reaches this server only through the feeder routing: \
         `sudo wellfed-stock network apply` while this waits",
    );
    ready(ctx)?;
    settle(ctx)?;

    // Back: stock installs the baseline into bank 0, which then promotes it.
    let back = ctx.ota_args("back", Some(baseline.review.clone()));
    let staged = ctx.rig.stage(&back)?.ok_or(StockError::BaselineUnsigned)?;
    let request_id = uuid::Uuid::new_v4().to_string();
    request(ctx, &staged, &request_id)?;
    install(ctx, &staged.build, &request_id)?;
    bank0(ctx, &staged.build, &request_id)?;
    promote(ctx, &staged)?;
    ctx.rig.offer(None, Fault::None)?;

    ctx.rig.server(Server::Serve)?;
    ctx.reachable(STATUS_WAIT).map(|_| ())
}

/// Run the stages in order; the first failure skips the rest.
fn run(ctx: &mut Ctx, args: &StockOtaArgs, staged: &Staged) -> (Vec<Report>, Option<Silent>) {
    let mut reports = Vec::new();
    let mut stop: Option<String> = None;
    let request_id = uuid::Uuid::new_v4().to_string();

    for name in steps(args) {
        if let Some(reason) = &stop {
            ctx.rig.scenario(name, StepState::Skipped);
            reports.push(Report {
                scenario: name,
                verdict: Verdict::Skip(reason.clone()),
                took: Duration::ZERO,
            });
            continue;
        }

        ctx.rig.scenario(name, StepState::Running);
        let begun = ctx.rig.clock();
        let result = match name {
            READY => ready(ctx),
            SETTLE => settle(ctx),
            REQUEST => request(ctx, staged, &request_id),
            DOWNLOAD => install(ctx, &staged.build, &request_id),
            BANK0 => bank0(ctx, &staged.build, &request_id),
            _ => promote(ctx, staged),
        };
        let took = ctx.rig.clock().saturating_sub(begun);

        let verdict = match result {
            Ok(()) => Verdict::Pass,
            Err(error) => {
                stop = Some(format!("{name} failed"));
                Verdict::Fail(error.to_string())
            }
        };
        let shown = match verdict {
            Verdict::Pass => StepState::Done,
            _ => StepState::Failed,
        };
        ctx.rig.scenario(name, shown);
        match &verdict {
            Verdict::Fail(reason) => ctx.note(&format!("  {name}: FAIL: {reason}")),
            _ => ctx.note(&format!("  {name}: pass")),
        }
        reports.push(Report {
            scenario: name,
            verdict,
            took,
        });
    }

    (reports, ctx.went_silent.take())
}

/// Stock 1.1.15 is online on its adapter and the server may ask it. Stock
/// prints no `PL…` records, so the UART's silence means nothing yet.
fn ready(ctx: &mut Ctx) -> Result<()> {
    ctx.note("- Waiting for stock to report to this server");
    let report = ctx.offline(|ctx| {
        ctx.until_status(&format!("stock {STOCK_VERSION}"), STOCK_WAIT, |status| {
            Ok(status.stock_ready()?)
        })
    })?;
    ctx.note(&format!("  {report}"));
    Ok(())
}

/// Serve the build and ask stock once to install it.
fn request(ctx: &mut Ctx, staged: &Staged, request_id: &str) -> Result<()> {
    let token = ctx.rig.images(&staged.review)?;

    let trial = TrialRequest {
        request_id: request_id.to_string(),
        server_token: token,
    };
    ctx.note(&format!(
        "  Asking stock to install {} (request {request_id}); never resent",
        short(&staged.build)
    ));
    ctx.rig.stock_trial(&trial)
}

/// Stock downloads, checks and activates; the build then starts on bank 0.
/// Stock's own reports and the server's ledger say how far it got.
fn install(ctx: &mut Ctx, build: &str, request_id: &str) -> Result<()> {
    let since = ctx.mark()?;
    let deadline = ctx.rig.clock() + INSTALL_WAIT;
    // The server keeps only the latest reports; each has its receipt time.
    let mut noted: Vec<OtaReport> = Vec::new();

    ctx.offline(|ctx| {
        loop {
            ctx.pull()?;
            if let Some(index) = ctx.log.find(since, is_start) {
                let line = ctx.log.line(index).text.clone();
                ctx.note(&format!("  UART: {line}"));
                return started(&line, build);
            }

            // A status call that fails once is retried at the next poll; the
            // UART and the deadline still decide.
            match ctx.rig.status() {
                Ok(status) => {
                    for report in &status.ota_reports {
                        if !noted.contains(report) {
                            ctx.note(&format!("  {report}"));
                            noted.push(report.clone());
                        }
                    }
                    match status.trial(request_id).map(|trial| trial.phase) {
                        None => return Err(StockError::NoRequest.into()),
                        Some(TrialPhase::ReportedError) => {
                            return Err(StockError::InstallRefused {
                                report: status.ota_reports.last().cloned(),
                            }
                            .into());
                        }
                        Some(_) => {}
                    }
                }
                Err(error) => ctx.note(&format!("  Status unavailable: {error}")),
            }

            if ctx.rig.clock() >= deadline {
                return Err(StockError::NotStarted.into());
            }
            ctx.rig.wait(STATUS_POLL)?;
        }
    })
}

fn is_start(event: &Event<'_>) -> bool {
    matches!(event, Event::Start { .. })
}

/// The first startup after the request must be the build, from bank 0.
fn started(line: &str, build: &str) -> Result<()> {
    let Some(Event::Start {
        build: reported,
        bank,
    }) = events(line).find(is_start)
    else {
        return Err(StockError::UnreadableStart.into());
    };
    let reported = reported.to_ascii_lowercase();

    if reported != build {
        return Err(StockError::WrongBuild {
            reported,
            build: build.into(),
        }
        .into());
    }
    if bank != Some(0) {
        return Err(StockError::WrongBank { bank }.into());
    }
    Ok(())
}

/// How long the bank-0 step asks the server to close the request: past the
/// server's 90 s telemetry window, counted from when the build answered.
const ANSWER_WAIT: Duration = Duration::from_secs(120);
const ANSWER_RETRY: Duration = Duration::from_secs(10);

/// The build answers the device API from bank 0 while it waits for an
/// update, which closes the server's install record: stock never reports
/// after activating it. A record the server would not close is noted, not
/// failed: the feeder is where it should be, and the next request waits for
/// the record's resolution instead.
fn bank0(ctx: &mut Ctx, build: &str, request_id: &str) -> Result<()> {
    let report = ctx.until_device("status from the build on bank 0", HEALTH_WAIT, |status| {
        if status.build != build {
            return Err(StockError::OtherBuild {
                build: status.build.clone(),
            }
            .into());
        }
        if status.bank != 0 {
            return Err(StockError::OtherBank { bank: status.bank }.into());
        }
        Ok(format!(
            "Status: build {} on bank 0",
            &build[..build.len().min(12)]
        ))
    })?;
    ctx.note(&format!("  {report}"));

    // The device API's status named exactly this build and bank 0 (the
    // check above); the server holds it to the build it served.
    let answer = TrialAnswer {
        request_id: request_id.to_string(),
        build: build.to_string(),
        bank: 0,
        note: format!(
            "stock-ota: build {} answered the device API from bank 0",
            &build[..build.len().min(12)]
        ),
    };

    // The server refuses while stock's last telemetry is still current (90 s
    // after its last report, which may be just before it rebooted into the
    // build), so a refusal is asked again until that window has surely passed.
    let deadline = ctx.rig.clock() + ANSWER_WAIT;
    loop {
        match ctx.rig.trial_answered(&answer) {
            Ok(()) => {
                ctx.note("  The server closed the request: replacement healthy");
                return Ok(());
            }
            Err(error) if ctx.rig.clock() + ANSWER_RETRY > deadline => {
                ctx.note(&format!(
                    "  The server did not close the request ({error}); the next one waits for its resolution"
                ));
                return Ok(());
            }
            Err(_) => ctx.rig.wait(ANSWER_RETRY)?,
        }
    }
}

/// Push the same build signed: bank 0 installs it into bank 1, and the trial
/// boot confirms.
fn promote(ctx: &mut Ctx, staged: &Staged) -> Result<()> {
    let since = ctx.mark()?;
    ctx.rig.offer(Some(staged), Fault::None)?;
    ctx.note(&format!(
        "  Offered version {} for the promotion",
        staged.version
    ));

    let build = &staged.build;
    match ctx.watch(build, since, INSTALL_WAIT)? {
        Outcome::Passed => {}
        Outcome::FellBack { boot } => {
            return Err(StockError::PromotionFellBack { boot }.into());
        }
        Outcome::Failed { stage } => {
            return Err(StockError::PromotionFailed { stage }.into());
        }
        Outcome::Skipped => return Err(StockError::PromotionSkipped.into()),
    }
    ctx.confirm(build)
}

/// An earlier request that failed or is uncertain blocks a new one until
/// the server resolves it. The server decides from its own evidence: 20
/// minutes after that request, 10 quiet connected minutes, and stock still
/// reporting its version, which then reads as not installed. This only asks
/// it, every 30 s; stock is sent nothing.
fn settle(ctx: &mut Ctx) -> Result<()> {
    let status = ctx.rig.status()?;
    let Some(open) = status.ota_trials.first().filter(|trial| {
        matches!(
            trial.phase,
            TrialPhase::Submitted | TrialPhase::Uncertain | TrialPhase::ReportedError
        )
    }) else {
        return Ok(());
    };
    let id = open.input.request_id.to_string();
    ctx.note(&format!(
        "  Earlier request {id} is {:?}; waiting for the server to resolve it",
        open.phase
    ));

    let note = format!("stock-ota: stock still reports {STOCK_VERSION} after the request");
    let deadline = ctx.rig.clock() + SETTLE_WAIT;
    let mut last = String::new();
    ctx.offline(|ctx| {
        loop {
            ctx.pull()?;
            match ctx.rig.resolve_trial(&id, &note) {
                Ok(()) => {
                    ctx.note("  Resolved: not installed");
                    return Ok(());
                }
                Err(error) => {
                    let reason = error.to_string();
                    if reason != last {
                        ctx.note(&format!("  Not yet resolvable: {reason}"));
                        last = reason;
                    }
                }
            }
            if ctx.rig.clock() >= deadline {
                return Err(StockError::Unresolved { id: id.clone() }.into());
            }
            ctx.rig.wait(SETTLE_POLL)?;
        }
    })
}

/// Build the payload, bring the bench up with a trial-enabled server,
/// run the stages, restore the bench and record the verdicts.
pub(super) fn stock_ota(bench: &mut Bench, args: &StockOtaArgs, stock_bin: PathBuf) -> Result<u8> {
    let record = bench
        .root
        .join(VERIFICATION)
        .join(format!("{}{SUFFIX}", args.name));
    bench.fresh(&record)?;
    std::fs::create_dir_all(bench.root.join(VERIFICATION))?;

    say!(
        "Stock reaches this server only through the feeder routing: run \
             `sudo wellfed-stock network apply --config {}` first, and `network remove` afterwards.",
        args.config.display()
    );
    let Some(staged) = bench.ota_stage(&ota_args(args), &mut Timing::default())? else {
        return Err(StockError::PayloadUnstaged.into());
    };

    let rig_args = rig_args(args);
    let mut rig = live::LiveRig::open(bench, &rig_args, stock_bin)?;
    let result = match rig.start(Server::StockTrial) {
        Ok(()) => {
            let mut ctx = Ctx::new(&mut rig, &rig_args);
            Ok(run(&mut ctx, args, &staged))
        }
        Err(error) => Err(error),
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
            "  {:<14} {verdict:<5} {:>9}  {reason}",
            report.scenario,
            ota::duration(report.took)
        );
    }
    if let Some(line) = recovery(&reports) {
        say!("{line}");
    }
    say!("Remember `sudo wellfed-stock network remove`.");

    let summary = super::summary(&args.name, &reports, silent.as_ref(), &captures);
    std::fs::write(&record, serde_json::to_string_pretty(&summary)? + "\n")?;
    say!("Recorded {}", bench.rel(&record));
    Ok(super::exit_code(&reports, silent.as_ref()))
}

/// Once stock was asked, a failure may leave bank 1 invalid and bank 0
/// holding whatever stock wrote: the way back is over UART.
fn recovery(reports: &[Report]) -> Option<&'static str> {
    let asked = reports
        .iter()
        .any(|r| r.scenario == REQUEST && r.verdict == Verdict::Pass);
    let failed = reports
        .iter()
        .any(|r| matches!(r.verdict, Verdict::Fail(_)));
    (asked && failed).then_some(
        "Stock was asked to install. If the feeder does not answer, recover it over UART with \
         the reviewed tools (docs/devices/plaf103/recovery.md); bank 1 may be invalid.",
    )
}

#[cfg(test)]
mod tests;
