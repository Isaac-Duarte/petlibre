//! `wellfed-bench stock-ota`: have stock 1.1.15 install a replacement build
//! over its own MQTT-triggered HTTP updater (the stock-to-replacement POC).
//!
//! The feeder is assumed to run stock, and the run proves it before asking
//! anything: the server must see stock report 1.1.15 on its adapter, which is
//! also the server's own gate for the request. Stock then fetches the build
//! from `ota-images`, checks only its byte-sum, writes it to bank 0 (the bank
//! it is not running from) and invalidates bank 1 ([stock OTA requirements]).
//! The factory loader then boots the build from bank 0, where its recovery
//! role reports health over MQTT while it waits for an update; the promoted
//! build on bank 1 is confirmed over the device API.
//!
//! The request goes out once and is never resent: a rejected stock precheck
//! can erase the stock OTA status sector, and an uncertain one may still be
//! downloading. Then the same build is offered as a signed update, so its
//! bank-0 recovery role installs it into bank 1 and the feeder ends in the
//! normal two-bank state (`--no-promote` stops at bank 0).
//!
//! Stock reaches this server only through `wellfed-local network apply`, a
//! host firewall change the operator makes and removes by hand; the run never
//! touches routing. The way back to stock is the UART stock recovery.
//!
//! [stock OTA requirements]: ../../../../docs/devices/plaf103/stock/ota-requirements.md

use std::path::PathBuf;

use wellfed_bench::commands::Bench;
use wellfed_bench::commands::ota::{self, OtaArgs, Outcome, Staged, Target, Timing};
use wellfed_bench::server::{STOCK_VERSION, TrialRequest};
use wellfed_bench::suite::{ItestArgs, StockOtaArgs};
use wellfed_bench::{Error, Result, say};
use wellfed_core::record::tag;
use wellfed_formats::bench::progress::StepState;
use wellfed_formats::bench::run::SUFFIX;
use wellfed_formats::bench::{VERIFICATION, catalog::Tier};
use wellfed_formats::local::{OtaReport, TrialPhase};

use super::log::Record;
use super::{
    Ctx, HEALTH_SECONDS, INSTALL_SECONDS, Report, STATUS_POLL_SECONDS, STATUS_SECONDS, Server,
    Silent, Verdict, live,
};

/// Stock joins the hotspot, connects and reports its attributes within a
/// minute or two of power; allow a slow join.
const STOCK_SECONDS: f64 = 5.0 * 60.0;
/// The server records the install's health as soon as it arrives.
const CLOSE_SECONDS: f64 = 30.0;
/// The server's resolution window (20 min after the request, 10 quiet
/// connected minutes) plus margin.
const SETTLE_SECONDS: f64 = 25.0 * 60.0;
const SETTLE_POLL_SECONDS: f64 = 30.0;

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
        minutes: INSTALL_SECONDS / 60.0,
        attended_motion: false,
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
/// and the bank-0 recovery role's promotion. It ends where it began:
/// the baseline confirmed on bank 1, `serve` answering. Bank 0 then holds
/// the baseline too, written by stock.
pub(super) fn roundtrip(ctx: &mut Ctx) -> Result<()> {
    let baseline = ctx.baseline()?.clone();
    // `unmet` skips the scenario without it.
    let review = ctx
        .args
        .stock
        .clone()
        .ok_or_else(|| Error::Bench("stock-roundtrip needs --stock STOCK_REVIEW".into()))?;

    // To stock: our updater writes it into bank 1, then its banner shows.
    let to_stock = ctx.ota_args("stock", Some(review.clone()));
    let stock = ctx
        .rig
        .stage(&to_stock)?
        .ok_or_else(|| Error::Bench("Signing the stock review failed".into()))?;
    if stock.target != Target::Stock {
        return Err(Error::Bench(format!(
            "{} is not a stock review",
            review.display()
        )));
    }
    let since = ctx.mark()?;
    ctx.rig.offer(Some(&stock))?;
    ctx.note("  Offered stock; the running build installs it into bank 1");
    let outcome =
        ctx.offline(|ctx| ctx.watch_as(&stock.build, Target::Stock, since, INSTALL_SECONDS))?;
    if outcome != Outcome::Passed {
        return Err(Error::Bench(format!("Stock did not start: {outcome:?}")));
    }
    ctx.rig.offer(None)?;

    // On stock: it reports to a trial-enabled server, routed by the operator.
    ctx.rig.server(Server::StockTrial)?;
    ctx.note(
        "  Stock reaches this server only through the feeder routing: \
         `sudo wellfed-local network apply` while this waits",
    );
    ready(ctx)?;
    settle(ctx)?;

    // Back: stock installs the baseline into bank 0, which then promotes it.
    let back = ctx.ota_args("back", Some(baseline.review.clone()));
    let staged = ctx
        .rig
        .stage(&back)?
        .ok_or_else(|| Error::Bench("Signing the baseline again failed".into()))?;
    let request_id = uuid::Uuid::new_v4().to_string();
    request(ctx, &staged, &request_id)?;
    install(ctx, &staged.build, &request_id)?;
    bank0(ctx, &staged.build, &request_id)?;
    promote(ctx, &staged)?;
    ctx.rig.offer(None)?;

    ctx.rig.server(Server::Serve)?;
    ctx.reachable(STATUS_SECONDS).map(|_| ())
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
                seconds: 0.0,
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
        let seconds = ctx.rig.clock() - begun;

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
            seconds,
        });
    }

    (reports, ctx.went_silent.take())
}

/// Stock 1.1.15 is online on its adapter and the server may ask it. Stock
/// prints no `PL…` records, so the UART's silence means nothing yet.
fn ready(ctx: &mut Ctx) -> Result<()> {
    ctx.note("- Waiting for stock to report to this server");
    let report = ctx.offline(|ctx| {
        ctx.until_status(&format!("stock {STOCK_VERSION}"), STOCK_SECONDS, |status| {
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
        &staged.build[..12]
    ));
    ctx.rig.stock_trial(&trial)
}

/// Stock downloads, checks and activates; the build then starts on bank 0.
/// Stock's own reports and the server's ledger say how far it got.
fn install(ctx: &mut Ctx, build: &str, request_id: &str) -> Result<()> {
    let since = ctx.mark()?;
    let deadline = ctx.rig.clock() + INSTALL_SECONDS;
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
                        None => {
                            return Err(Error::Bench(
                                "The server has no record of the request".into(),
                            ));
                        }
                        Some(TrialPhase::ReportedError) => {
                            let last = status.ota_reports.last().map(ToString::to_string);
                            return Err(Error::Bench(format!(
                                "Stock refused or failed the install ({})",
                                last.unwrap_or_else(|| "no report".into())
                            )));
                        }
                        Some(_) => {}
                    }
                }
                Err(error) => ctx.note(&format!("  Status unavailable: {error}")),
            }

            if ctx.rig.clock() >= deadline {
                return Err(Error::Bench(format!(
                    "The build did not start within {:.0} min",
                    INSTALL_SECONDS / 60.0
                )));
            }
            ctx.rig.wait(STATUS_POLL_SECONDS)?;
        }
    })
}

fn is_start(record: &Record) -> bool {
    record.tag == tag::START && record.first == "start"
}

/// The first startup after the request must be the build, from bank 0.
fn started(line: &str, build: &str) -> Result<()> {
    let record = Record::all(line)
        .find(is_start)
        .ok_or_else(|| Error::Bench("Unreadable startup record".into()))?;
    let reported = record.field("build").unwrap_or("").to_ascii_lowercase();
    let bank: Option<u32> = record.number("bank");

    if reported != build {
        return Err(Error::Bench(format!(
            "Build {} started, not {}",
            reported.get(..12).unwrap_or(&reported),
            &build[..12]
        )));
    }
    if bank != Some(0) {
        return Err(Error::Bench(format!(
            "The build started on bank {bank:?}, not 0"
        )));
    }
    Ok(())
}

/// The build's recovery role reports from bank 0 while it waits for an
/// update, and that health closes the server's install record.
fn bank0(ctx: &mut Ctx, build: &str, request_id: &str) -> Result<()> {
    let report = ctx.until_status(
        "recovery-role health from the build on bank 0",
        HEALTH_SECONDS,
        |status| Ok(status.recovery_health(build)?),
    )?;
    ctx.note(&format!("  {report}"));
    closed(ctx, request_id)
}

/// Offer the same build signed: the bank-0 recovery role asks every 10 s,
/// installs it into bank 1, and the trial boot confirms.
fn promote(ctx: &mut Ctx, staged: &Staged) -> Result<()> {
    let since = ctx.mark()?;
    ctx.rig.offer(Some(staged))?;
    ctx.note(&format!(
        "  Offered version {} for the promotion",
        staged.version
    ));

    let build = &staged.build;
    match ctx.watch(build, since, INSTALL_SECONDS)? {
        Outcome::Passed => {}
        Outcome::FellBack { boot } => {
            return Err(Error::Bench(format!(
                "Bank 1 fell back to bank 0 after trial boot {boot:?}"
            )));
        }
        Outcome::Failed { stage } => {
            return Err(Error::Bench(format!("The promotion failed at {stage}")));
        }
        Outcome::Skipped => {
            return Err(Error::Bench(
                "The promotion's attempt reset; bank 0 will not retry until a cold boot".into(),
            ));
        }
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
    let deadline = ctx.rig.clock() + SETTLE_SECONDS;
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
                return Err(Error::Bench(format!(
                    "The server did not resolve {id} within {:.0} min",
                    SETTLE_SECONDS / 60.0
                )));
            }
            ctx.rig.wait(SETTLE_POLL_SECONDS)?;
        }
    })
}

/// The server records the install as healthy.
fn closed(ctx: &mut Ctx, request_id: &str) -> Result<()> {
    ctx.until_status(
        "the server closing the install",
        CLOSE_SECONDS,
        |status| match status.trial(request_id).map(|trial| trial.phase) {
            Some(TrialPhase::ReplacementHealthy) => Ok(String::new()),
            phase => Err(Error::Bench(format!("trial phase {phase:?}"))),
        },
    )
    .map(|_| ())
}

/// Build the payload, bring the bench up with a trial-enabled server,
/// run the stages, restore the bench and record the verdicts.
pub(super) fn stock_ota(bench: &mut Bench, args: &StockOtaArgs, local_bin: PathBuf) -> Result<u8> {
    let record = bench
        .root
        .join(VERIFICATION)
        .join(format!("{}{SUFFIX}", args.name));
    bench.fresh(&record)?;
    std::fs::create_dir_all(bench.root.join(VERIFICATION))?;

    say!(
        "Stock reaches this server only through the feeder routing: run \
             `sudo wellfed-local network apply --config {}` first, and `network remove` afterwards.",
        args.config.display()
    );
    let Some(staged) = bench.ota_stage(&ota_args(args), &mut Timing::default())? else {
        return Err(Error::Bench(
            "Building or signing the payload failed".into(),
        ));
    };

    let rig_args = rig_args(args);
    let mut rig = live::LiveRig::open(bench, &rig_args, local_bin)?;
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
            ota::duration(report.seconds)
        );
    }
    if let Some(line) = recovery(&reports) {
        say!("{line}");
    }
    say!("Remember `sudo wellfed-local network remove`.");

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
