//! Inspect, prepare and execute reviewed application-region updates of bank 1
//! (default) or bank 0, and bank-1 give-ways. The factory region is never
//! written.
//!
//! Port of the former `src/tools/flash/uart-development.py` and `uart-give-way.py`. Each
//! continues an established helper session: none uploads a helper, enters a
//! loader, changes baud or status, boots an application or replays a stopped
//! transaction. An interrupted update is planned afresh from a new
//! inspection; its stopped journal is never read.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::IgnoredAny;

use crate::error::{Result, refuse, require};
use crate::evidence::journal::Event;
use crate::evidence::report::{Session, Status, Transport};
use crate::evidence::review::{Frozen, frozen};
use crate::evidence::session::{self, Completion, Context, Outcome, Region, Saved, progress};
use crate::link::protocol::{CAPACITY, Port, SECTOR};
use crate::link::sector::{self, Baseline};
use crate::link::status;
use crate::writers::development::{
    self, Acquired, BANK0, FACTORY, GiveWay, Layout, Transaction, Update,
};
use wellfed_formats::canonical::{canonical_of, sha256};
use wellfed_formats::flash_plan::{
    DevelopmentPlan, GiveWayBindings, GiveWayPlan, Kind, Reviewed, UpdateBindings,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Mode {
    /// Read the region and guards; write nothing.
    Inspect,
    /// Write a reviewed plan from a completed inspection; no serial access.
    Prepare,
    /// Execute exactly the reviewed plan.
    Execute,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Self::Inspect => "inspect",
            Self::Prepare => "prepare",
            Self::Execute => "execute",
        }
    }
}

/// The evidence a session continues from, common to updates and give-ways.
pub struct Evidence {
    pub port: String,
    pub baseline: PathBuf,
    pub rom_report: PathBuf,
    pub small_report: PathBuf,
    pub helper: Option<PathBuf>,
    pub output: PathBuf,
    pub inspection: Option<PathBuf>,
    pub plan: Option<PathBuf>,
    pub expected_plan_sha256: Option<String>,
}

pub struct Args {
    pub mode: Mode,
    pub evidence: Evidence,
    pub target: PathBuf,
    pub source: Option<PathBuf>,
    pub original_read: Option<PathBuf>,
    pub bank: u8,
    pub restore_marker: bool,
    pub interrupted: bool,
}

struct Loaded {
    layout: Layout,
    baseline: Baseline,
    source: Option<Frozen>,
    target: Frozen,
    original: Option<Vec<u8>>,
    bindings: UpdateBindings,
}

impl Loaded {
    fn update(&self) -> Update<'_> {
        Update {
            source: self.source.as_ref().map(|f| f.image.as_slice()),
            target: &self.target.image,
            interrupted: self.bindings.interrupted,
            layout: self.layout,
            original: self.original.as_deref(),
            restore: self.bindings.restore_marker,
        }
    }

    fn extents(&self) -> Result<[(usize, usize); 3]> {
        let length = development::extent(
            self.source.as_ref().map(|f| f.image.as_slice()),
            &self.target.image,
            &self.layout,
        )?;
        Ok([FACTORY, self.layout.other, (self.layout.start, length)])
    }
}

/// What an execution acts on, all checked before serial access.
struct Execution<B, P> {
    /// The two inspected regions the plan derives from: the application
    /// region and other-bank sector of an update, or bank 0 and bank 1's
    /// first sector of a give-way.
    inspected: [Vec<u8>; 2],
    reviewed: Reviewed<B, P>,
    /// The reviewed plan file's exact bytes.
    raw: Vec<u8>,
}

/// An update or give-way session report's fields beyond the lifecycle.
#[derive(Serialize)]
struct Details<'a, B> {
    mode: &'static str,
    bindings: &'a B,
    helper_upload_attempted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    baud: Option<u32>,
    observations: Vec<Vec<Region>>,
    status_checks: Vec<Event>,
    application_boot_verified: bool,
    tool: &'static str,
    transport: Transport,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<development::Verified>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reviewed_plan_sha256: Option<String>,
}

impl<'a, B> Details<'a, B> {
    fn new(context: &Context, mode: Mode, bindings: &'a B) -> Self {
        Self {
            mode: mode.name(),
            bindings,
            helper_upload_attempted: false,
            baud: None,
            observations: Vec::new(),
            status_checks: Vec::new(),
            application_boot_verified: false,
            tool: "wellfed-flash",
            transport: context.transport(),
            scope: None,
            result: None,
            reviewed_plan_sha256: None,
        }
    }
}

#[derive(Serialize)]
struct Summary<'a> {
    status: Status,
    mode: &'static str,
    output: &'a Path,
    result: Option<development::Verified>,
    error: Option<String>,
}

#[derive(Serialize)]
struct Prepared<'a> {
    status: &'static str,
    plan: &'a Path,
    plan_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    erase_sectors: Option<u64>,
}

/// Session evidence both commands check before any serial access.
fn continued(context: &Context, evidence: &Evidence) -> Result<(Baseline, Vec<u8>, Vec<u8>)> {
    let baseline = context.baseline(&evidence.baseline)?;
    context.helper(evidence.helper.as_deref())?;
    let rom = session::validate_rom_report(context, &evidence.rom_report)?;
    let small =
        session::validate_status_session(context, Some(&evidence.small_report), &evidence.port)?;
    Ok((baseline, rom, small))
}

fn load(context: &Context, args: &Args) -> Result<Loaded> {
    let root = &context.root;
    let layout = development::layout(args.bank)?;

    // Bank 0 may be overwritten from its pinned contents instead of a source review.
    let source = args
        .source
        .as_deref()
        .map(|path| frozen(root, path, &layout))
        .transpose()?;
    require(
        source.is_some() || layout.bank == 0,
        "A source review is required for bank 1",
    )?;

    let target = frozen(root, &args.target, &layout)?;
    let (baseline, rom, small) = continued(context, &args.evidence)?;

    let original = match &source {
        Some(_) => None,
        None => {
            let Some(path) = args.original_read.as_deref() else {
                return refuse(
                    "Overwriting bank 0 needs --original-read: a full flash read pinning its current contents",
                );
            };
            let read = root.private_file(path)?;
            require(
                read.len() == CAPACITY,
                "The original read must be a full 8 MiB read",
            )?;
            require(
                read[..0x6000] == baseline.bytes()[..0x6000],
                "The original read's factory region differs",
            )?;
            Some(read)
        }
    };

    let bindings = UpdateBindings {
        schema_version: 1,
        kind: layout.kind,
        port: args.evidence.port.clone(),
        interrupted: args.interrupted,
        original_read_sha256: original.as_deref().map(sha256),
        restore_marker: args.restore_marker,
        source_review_sha256: source.as_ref().map(|f| f.review_sha256.clone()),
        target_review_sha256: target.review_sha256.clone(),
        baseline_sha256: baseline.sha256(),
        rom_report_sha256: sha256(&rom),
        bounded_read_report_sha256: sha256(&small),
        tool_sha256: context.tools(),
    };

    let original = original
        .map(|read| development::original_region(&read, &target.image, &layout))
        .transpose()?;

    Ok(Loaded {
        layout,
        baseline,
        source,
        target,
        original,
        bindings,
    })
}

/// The one permitted status pair an inspection must have recorded.
fn status_permitted(report: &Saved<IgnoredAny>) -> Result<()> {
    let checks = report.status_checks.as_deref();
    require(
        checks.is_some_and(|c| {
            c.len() == 1
                && c[0].event.as_deref() == Some("flash_status_checked")
                && c[0]
                    .assessment
                    .as_ref()
                    .and_then(|a| a.software_preconditions_met)
                    == Some(true)
        }),
        "Permitted inspection status pair missing",
    )
}

/// The inspected application region and the other bank's first sector.
fn inspected(
    context: &Context,
    args: &Args,
    loaded: &Loaded,
) -> Result<(Vec<u8>, Vec<u8>, String)> {
    let (path, report, sha) = session::inspection(
        context,
        args.evidence.inspection.as_deref(),
        &loaded.bindings,
    )?;

    let [factory, other, application] = session::inspected_transfers(
        &context.root,
        &path,
        &report,
        &loaded.extents()?,
        "Complete region/guards required",
    )?;

    development::guards(&loaded.baseline, &factory, &other, &loaded.layout, None)?;
    development::images(&loaded.update(), &application)?;
    status_permitted(&report)?;

    Ok((application, other, sha))
}

pub fn run(context: &Context, args: &Args) -> Result<Outcome> {
    let loaded = load(context, args)?;
    if args.mode == Mode::Inspect {
        return session_run(context, args, &loaded, None);
    }

    let (current, other, report_sha) = inspected(context, args, &loaded)?;
    let reviewed = Reviewed {
        bindings: loaded.bindings.clone(),
        inspection_report_sha256: report_sha,
        operation_plan: development::plan(&loaded.update(), &current, Some(&other))?,
    };

    if args.mode == Mode::Prepare {
        let sha = session::write_plan(&context.root, &args.evidence.output, &reviewed)?;
        progress(&format!("Prepared development plan SHA256: {sha}"));
        return Outcome::done(&Prepared {
            status: "prepared",
            plan: &args.evidence.output,
            plan_sha256: sha,
            erase_sectors: Some(reviewed.operation_plan.erase_sectors),
        });
    }

    let raw = session::reviewed_plan(
        &context.root,
        args.evidence.plan.as_deref(),
        args.evidence.expected_plan_sha256.as_deref(),
        &reviewed,
    )?;

    // Recheck the operation and bindings themselves before serial access.
    require(
        canonical_of(&reviewed.operation_plan)?
            == canonical_of(&development::plan(
                &loaded.update(),
                &current,
                Some(&other),
            )?)?,
        "Operation differs before serial access",
    )?;
    require(
        canonical_of(&reviewed.bindings)? == canonical_of(&loaded.bindings)?,
        "Execution bindings differ before serial access",
    )?;

    let execution = Execution {
        inspected: [current, other],
        reviewed,
        raw,
    };
    session_run(context, args, &loaded, Some(&execution))
}

fn session_run(
    context: &Context,
    args: &Args,
    loaded: &Loaded,
    execution: Option<&Execution<UpdateBindings, DevelopmentPlan>>,
) -> Result<Outcome> {
    let directory = context.root.output_directory(&args.evidence.output)?;
    if let Some(execution) = &execution {
        session::keep_plan(&directory, &execution.raw)?;
    }

    let report = Session::open(
        directory.join("report.json"),
        Details {
            baud: Some(115200),
            scope: Some(
                "Application union and factory/other-bank guards only; unscanned flash preserved by write scope",
            ),
            ..Details::new(context, args.mode, &loaded.bindings)
        },
    )?;

    let extents = loaded.extents()?;
    let update = loaded.update();

    let result = (|| -> Result<Option<development::Verified>> {
        let mut port = context.record(&args.evidence.port, &directory)?;
        report.update(|r| r.status = Status::Running)?;

        progress(&format!(
            "Development {} of bank {}; region {} bytes; no full-flash scans",
            args.mode.name(),
            loaded.layout.bank,
            extents[2].1
        ));

        let mut acquire = |port: &mut dyn Port| -> Result<Acquired> {
            let number = report.read(|r| r.details.observations.len()) + 1;
            let mut observation = Vec::new();
            let mut values = Vec::new();

            for (label, (offset, length)) in
                ["factory", "other-bank", "application"].iter().zip(extents)
            {
                let (entry, data) =
                    session::acquire_region(port, &directory, label, number, offset, length)?;
                observation.push(entry);
                values.push(data);
            }

            report.update(|r| r.details.observations.push(observation))?;
            progress(&format!("Region acquisition {number} complete"));

            let Ok([factory, other, application]) = <[Vec<u8>; 3]>::try_from(values) else {
                return refuse("Incomplete region acquisition");
            };
            Ok(Acquired {
                application,
                factory,
                other,
            })
        };

        match &execution {
            None => {
                let acquired = acquire(&mut port)?;
                development::guards(
                    &loaded.baseline,
                    &acquired.factory,
                    &acquired.other,
                    &loaded.layout,
                    None,
                )?;
                development::images(&update, &acquired.application)?;

                // Written with the report at its next save.
                let mut record = |event: Event| {
                    report.edit(|r| r.details.status_checks.push(event));
                    Ok(())
                };

                status::check_status(
                    &mut port,
                    loaded.layout.start,
                    loaded.layout.start + acquired.application.len(),
                    "inspection",
                    &mut record,
                )?;
                Ok(None)
            }

            Some(execution) => {
                let [current, other] = &execution.inspected;
                let mut counter = |n: usize, total: usize| {
                    if n == 1 || n.is_multiple_of(8) || n == total {
                        progress(&format!("Verified changed sectors: {n}/{total}"));
                    }
                };

                let transaction = Transaction {
                    baseline: &loaded.baseline,
                    update: &update,
                    current,
                    other: Some(other),
                    reviewed: &execution.reviewed.operation_plan,
                };
                development::run(
                    &mut port,
                    &context.root,
                    &transaction,
                    &mut acquire,
                    &directory.join("transaction"),
                    &mut counter,
                )
                .map(Some)
            }
        }
    })();

    finish(
        &directory,
        report,
        result,
        execution.as_ref().map(|e| e.raw.as_slice()),
    )
}

/// Close a session's report the way the Python clients do.
fn finish<B: Serialize>(
    directory: &Path,
    report: Session<Details<'_, B>>,
    result: Result<Option<development::Verified>>,
    raw_plan: Option<&[u8]>,
) -> Result<Outcome> {
    let completion = match &result {
        Ok(_) => Completion::Complete,
        Err(_) => Completion::Stopped,
    };

    let (report, saved) = report.close(directory, |r| {
        match result {
            Ok(None) => r.status = Status::Complete,
            Ok(Some(result)) => {
                r.status = Status::Verified;
                r.details.result = Some(result);
                r.details.reviewed_plan_sha256 = raw_plan.map(sha256);
            }
            Err(error) => r.stop(Status::Stopped, &error),
        }

        progress(match completion {
            Completion::Complete => "Completed; application boot remains a separate bench check",
            Completion::Stopped => {
                "Stopped without replay; keep application boot excluded and inspect evidence"
            }
        });
    });

    let summary = Summary {
        status: report.status,
        mode: report.details.mode,
        output: directory,
        result: report.details.result,
        error: report.error,
    };
    Ok(session::closed(completion, &summary, saved))
}

pub struct GiveWayArgs {
    pub mode: Mode,
    pub evidence: Evidence,
    pub fallback: PathBuf,
}

struct GiveWayLoaded {
    baseline: Baseline,
    fallback: Vec<u8>,
    bindings: GiveWayBindings,
}

impl GiveWayLoaded {
    fn extents(&self) -> Result<[(&'static str, usize, usize); 3]> {
        Ok([
            ("factory", 0, 0x6000),
            (
                "bank0",
                BANK0.start,
                development::payload(&self.fallback, &BANK0)?.len(),
            ),
            ("bank1-sector", sector::START, SECTOR),
        ])
    }
}

fn give_way_load(context: &Context, args: &GiveWayArgs) -> Result<GiveWayLoaded> {
    let fallback = frozen(&context.root, &args.fallback, &BANK0)?;
    let (baseline, rom, small) = continued(context, &args.evidence)?;
    let bindings = GiveWayBindings {
        schema_version: 1,
        kind: Kind::GiveWay,
        port: args.evidence.port.clone(),
        fallback_review_sha256: fallback.review_sha256,
        baseline_sha256: baseline.sha256(),
        rom_report_sha256: sha256(&rom),
        bounded_read_report_sha256: sha256(&small),
        tool_sha256: context.tools(),
    };
    Ok(GiveWayLoaded {
        baseline,
        fallback: fallback.image,
        bindings,
    })
}

pub fn give_way(context: &Context, args: &GiveWayArgs) -> Result<Outcome> {
    let loaded = give_way_load(context, args)?;
    if args.mode == Mode::Inspect {
        return give_way_session(context, args, &loaded, None);
    }

    let (path, report, report_sha) = session::inspection(
        context,
        args.evidence.inspection.as_deref(),
        &loaded.bindings,
    )?;
    let extents = loaded
        .extents()?
        .map(|(_, offset, length)| (offset, length));
    let [factory, bank0, bank1] = session::inspected_transfers(
        &context.root,
        &path,
        &report,
        &extents,
        "Complete guard acquisition required",
    )?;

    require(
        factory == loaded.baseline.bytes()[..0x6000],
        "Factory region differs from the pinned baseline",
    )?;

    let reviewed = Reviewed {
        bindings: loaded.bindings.clone(),
        inspection_report_sha256: report_sha,
        operation_plan: development::give_way_plan(&loaded.fallback, &bank0, &bank1)?,
    };

    if args.mode == Mode::Prepare {
        let sha = session::write_plan(&context.root, &args.evidence.output, &reviewed)?;
        progress(&format!("Prepared give-way plan SHA256: {sha}"));
        return Outcome::done(&Prepared {
            status: "prepared",
            plan: &args.evidence.output,
            plan_sha256: sha,
            erase_sectors: None,
        });
    }

    let raw = session::reviewed_plan(
        &context.root,
        args.evidence.plan.as_deref(),
        args.evidence.expected_plan_sha256.as_deref(),
        &reviewed,
    )?;

    let execution = Execution {
        inspected: [bank0, bank1],
        reviewed,
        raw,
    };
    give_way_session(context, args, &loaded, Some(&execution))
}

fn give_way_session(
    context: &Context,
    args: &GiveWayArgs,
    loaded: &GiveWayLoaded,
    execution: Option<&Execution<GiveWayBindings, GiveWayPlan>>,
) -> Result<Outcome> {
    let directory = context.root.output_directory(&args.evidence.output)?;
    if let Some(execution) = &execution {
        session::keep_plan(&directory, &execution.raw)?;
    }

    let report = Session::open(
        directory.join("report.json"),
        Details::new(context, args.mode, &loaded.bindings),
    )?;

    let extents = loaded.extents()?;

    let result = (|| -> Result<Option<development::Verified>> {
        let mut port = context.record(&args.evidence.port, &directory)?;
        report.update(|r| r.status = Status::Running)?;

        let mut acquire = |port: &mut dyn Port| -> Result<[Vec<u8>; 3]> {
            let number = report.read(|r| r.details.observations.len()) + 1;
            let mut observation = Vec::new();
            let mut values = Vec::new();

            for (label, offset, length) in extents {
                let (entry, data) =
                    session::acquire_region(port, &directory, label, number, offset, length)?;
                observation.push(entry);
                values.push(data);
            }

            report.update(|r| r.details.observations.push(observation))?;
            progress(&format!("Guard acquisition {number} complete"));

            let Ok(values) = <[Vec<u8>; 3]>::try_from(values) else {
                return refuse("Incomplete guard acquisition");
            };
            Ok(values)
        };

        match &execution {
            None => {
                let [factory, region, first] = acquire(&mut port)?;
                require(
                    factory == loaded.baseline.bytes()[..0x6000],
                    "Factory region differs from the pinned baseline",
                )?;
                development::give_way_plan(&loaded.fallback, &region, &first)?;

                // Written with the report at its next save.
                let mut record = |event: Event| {
                    report.edit(|r| r.details.status_checks.push(event));
                    Ok(())
                };

                status::check_status(
                    &mut port,
                    sector::START,
                    sector::START + 1024,
                    "inspection",
                    &mut record,
                )?;
                Ok(None)
            }

            Some(execution) => {
                let [bank0, bank1] = &execution.inspected;
                let give_way = GiveWay {
                    baseline: &loaded.baseline,
                    fallback: &loaded.fallback,
                    bank0,
                    bank1_sector: bank1,
                    reviewed: &execution.reviewed.operation_plan,
                };
                development::run_give_way(
                    &mut port,
                    &context.root,
                    &give_way,
                    &mut acquire,
                    &directory.join("transaction"),
                )
                .map(Some)
            }
        }
    })();

    finish(
        &directory,
        report,
        result,
        execution.as_ref().map(|e| e.raw.as_slice()),
    )
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::evidence::Root;
    use crate::evidence::session::Pin;

    fn context(checkout: &Path) -> Context {
        Context {
            root: Root::new(checkout).unwrap(),
            tool_sha256: "t".repeat(64),
            open: Box::new(|_| refuse("No port in this test")),
            pin: Pin::Research,
        }
    }

    fn saved(path: &Path) -> Value {
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn update_reports_keep_their_fields() {
        let checkout = tempfile::tempdir().unwrap();
        std::fs::create_dir(checkout.path().join("private")).unwrap();
        let context = context(checkout.path());
        let directory = checkout.path().join("private");
        let bindings = json!({"kind": "plaf103_bank1_development_update"});

        let report = Session::open(
            directory.join("report.json"),
            Details {
                baud: Some(115200),
                scope: Some("scope"),
                ..Details::new(&context, Mode::Execute, &bindings)
            },
        )
        .unwrap();
        let prepared = saved(&directory.join("report.json"));
        assert_eq!(
            prepared,
            json!({
                "mode": "execute", "status": "prepared", "start_utc": prepared["start_utc"],
                "bindings": bindings, "helper_upload_attempted": false, "baud": 115200,
                "observations": [], "status_checks": [], "application_boot_verified": false,
                "tool": "wellfed-flash", "transport": "serial", "scope": "scope",
            })
        );

        let verified = development::Verified {
            status: "verified_via_supplied_transports",
            region_sha256: Some("r".into()),
            erase_sectors: Some(2),
            after_sha256: None,
            application_boot_verified: false,
            scope: "scope".into(),
        };
        let outcome = finish(&directory, report, Ok(Some(verified)), Some(b"plan")).unwrap();
        let result = json!({"status": "verified_via_supplied_transports", "region_sha256": "r",
                            "erase_sectors": 2, "application_boot_verified": false,
                            "scope": "scope"});

        let closed = saved(&directory.join("report.json"));
        assert_eq!(
            closed,
            json!({
                "mode": "execute", "status": "verified", "start_utc": prepared["start_utc"],
                "bindings": bindings, "helper_upload_attempted": false, "baud": 115200,
                "observations": [], "status_checks": [], "application_boot_verified": false,
                "tool": "wellfed-flash", "transport": "serial", "scope": "scope",
                "result": result, "reviewed_plan_sha256": sha256(b"plan"),
                "end_utc": closed["end_utc"],
            })
        );
        assert_eq!(outcome.completion, Completion::Complete);
        assert_eq!(
            outcome.summary,
            json!({"status": "verified", "mode": "execute", "output": directory,
                   "result": result, "error": null})
        );
    }

    #[test]
    fn a_stopped_give_way_report_keeps_its_fields() {
        let checkout = tempfile::tempdir().unwrap();
        std::fs::create_dir(checkout.path().join("private")).unwrap();
        let context = context(checkout.path());
        let directory = checkout.path().join("private");
        let bindings = json!({"kind": "plaf103_bank1_give_way"});

        let report = Session::open(
            directory.join("report.json"),
            Details::new(&context, Mode::Inspect, &bindings),
        )
        .unwrap();
        report.edit(|r| {
            r.details.status_checks.push(Event::FlashStatusUnstable {
                boundary: "inspection",
                start: 1,
                end_exclusive: 2,
                first: [0, 0, 0],
                second: [1, 0, 0],
            })
        });
        let outcome = finish(&directory, report, refuse("No port"), None).unwrap();

        let closed = saved(&directory.join("report.json"));
        assert_eq!(
            closed,
            json!({
                "mode": "inspect", "status": "stopped", "start_utc": closed["start_utc"],
                "bindings": bindings, "helper_upload_attempted": false,
                "observations": [],
                "status_checks": [{"event": "flash_status_unstable", "boundary": "inspection",
                                   "start": 1, "end_exclusive": 2,
                                   "first": [0, 0, 0], "second": [1, 0, 0]}],
                "application_boot_verified": false,
                "tool": "wellfed-flash", "transport": "serial",
                "error": "refused: No port", "end_utc": closed["end_utc"],
            })
        );
        assert_eq!(outcome.completion, Completion::Stopped);
        assert_eq!(
            outcome.summary,
            json!({"status": "stopped", "mode": "inspect", "output": directory,
                   "result": null, "error": "refused: No port"})
        );
    }
}
