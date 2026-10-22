//! Inspect, prepare and execute reviewed application-region updates of bank 1
//! (default) or bank 0, and bank-1 give-ways. The factory region is never
//! written.
//!
//! Port of the former `tools/flash/uart-development.py` and `uart-give-way.py`. Each
//! continues an established helper session: none uploads a helper, enters a
//! loader, changes baud or status, boots an application or replays a stopped
//! transaction. An interrupted update is planned afresh from a new
//! inspection; its stopped journal is never read.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::canonical::{canonical, sha256};
use crate::development::{self, Acquired, BANK0, FACTORY, Layout, Update};
use crate::error::{Error, Result, require};
use crate::evidence::{self, utc};
use crate::protocol::{CAPACITY, Port, SECTOR};
use crate::review::{Frozen, frozen};
use crate::sector::{self, Baseline};
use crate::session::{self, Context, Outcome, progress};
use crate::status;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Inspect,
    Prepare,
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
    bindings: Value,
}

impl Loaded {
    fn update(&self) -> Update<'_> {
        Update {
            source: self.source.as_ref().map(|f| f.image.as_slice()),
            target: &self.target.image,
            interrupted: self.bindings["interrupted"] == true,
            layout: self.layout,
            original: self.original.as_deref(),
            restore: self.bindings["restore_marker"] == true,
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
            let path = args.original_read.as_deref().ok_or_else(|| {
                Error::Refused(
                    "Overwriting bank 0 needs --original-read: a full flash read pinning its current contents"
                        .into(),
                )
            })?;
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
    let bindings = json!({
        "schema_version": 1, "kind": layout.kind, "port": args.evidence.port,
        "interrupted": args.interrupted,
        "original_read_sha256": original.as_deref().map(sha256),
        "restore_marker": args.restore_marker,
        "source_review_sha256": source.as_ref().map(|f| &f.review_sha256),
        "target_review_sha256": target.review_sha256,
        "baseline_sha256": baseline.sha256(),
        "rom_report_sha256": sha256(&rom), "bounded_read_report_sha256": sha256(&small),
        "tool_sha256": context.tools(),
    });
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
    let values = session::inspected_transfers(
        &context.root,
        &path,
        &report,
        &loaded.extents()?,
        "Complete region/guards required",
    )?;
    development::guards(
        &loaded.baseline,
        &values[0],
        &values[1],
        &loaded.layout,
        None,
    )?;
    development::images(&loaded.update(), &values[2])?;
    let checks = report["status_checks"].as_array();
    require(
        checks.is_some_and(|c| {
            c.len() == 1
                && c[0]["event"] == "flash_status_checked"
                && c[0]["assessment"]["software_preconditions_met"] == true
        }),
        "Permitted inspection status pair missing",
    )?;
    let [_, other, application] = <[Vec<u8>; 3]>::try_from(values).unwrap();
    Ok((application, other, sha))
}

fn without_review(reviewed: &Value) -> Value {
    let mut bindings = reviewed.clone();
    let map = bindings.as_object_mut().unwrap();
    map.remove("inspection_report_sha256");
    map.remove("operation_plan");
    bindings
}

pub fn run(context: &Context, args: &Args) -> Result<Outcome> {
    let loaded = load(context, args)?;
    if args.mode == Mode::Inspect {
        return session_run(context, args, &loaded, None);
    }
    let (current, other, report_sha) = inspected(context, args, &loaded)?;
    let mut reviewed = loaded.bindings.clone();
    reviewed["inspection_report_sha256"] = json!(report_sha);
    reviewed["operation_plan"] = development::plan(&loaded.update(), &current, Some(&other))?;
    if args.mode == Mode::Prepare {
        let sha = session::write_plan(&context.root, &args.evidence.output, &reviewed)?;
        progress(&format!("Prepared development plan SHA256: {sha}"));
        return Ok(Outcome {
            complete: true,
            summary: json!({"status": "prepared", "plan": args.evidence.output, "plan_sha256": sha,
                            "erase_sectors": reviewed["operation_plan"]["erase_sectors"]}),
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
        canonical(&reviewed["operation_plan"])?
            == canonical(&development::plan(
                &loaded.update(),
                &current,
                Some(&other),
            )?)?,
        "Operation differs before serial access",
    )?;
    require(
        canonical(&without_review(&reviewed))? == canonical(&loaded.bindings)?,
        "Execution bindings differ before serial access",
    )?;
    session_run(
        context,
        args,
        &loaded,
        Some((current, other, reviewed, raw)),
    )
}

fn session_run(
    context: &Context,
    args: &Args,
    loaded: &Loaded,
    execution: Option<Execution>,
) -> Result<Outcome> {
    let directory = context.root.output_directory(&args.evidence.output)?;
    if let Some((_, _, _, raw)) = &execution {
        session::keep_plan(&directory, raw)?;
    }
    let report_path = directory.join("report.json");
    let report = RefCell::new(json!({
        "mode": args.mode.name(), "status": "prepared", "start_utc": utc(),
        "bindings": loaded.bindings, "helper_upload_attempted": false, "baud": 115200,
        "observations": [], "status_checks": [], "application_boot_verified": false,
        "tool": "petlibre-flash", "transport": context.transport(),
        "scope": "Application union and factory/other-bank guards only; unscanned flash preserved by write scope",
    }));
    let save = || evidence::save_report(&report_path, &report.borrow());
    save()?;
    let extents = loaded.extents()?;
    let update = loaded.update();
    let result = (|| -> Result<Option<Value>> {
        let mut port = context.record(&args.evidence.port, &directory)?;
        report.borrow_mut()["status"] = json!("running");
        save()?;
        progress(&format!(
            "Development {} of bank {}; region {} bytes; no full-flash scans",
            args.mode.name(),
            loaded.layout.bank,
            extents[2].1
        ));
        let mut acquire = |port: &mut dyn Port| -> Result<Acquired> {
            let number = report.borrow()["observations"].as_array().unwrap().len() + 1;
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
            report.borrow_mut()["observations"]
                .as_array_mut()
                .unwrap()
                .push(Value::Array(observation));
            save()?;
            progress(&format!("Region acquisition {number} complete"));
            let [factory, other, application] = <[Vec<u8>; 3]>::try_from(values).unwrap();
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
                let mut record = |event: &str, extra: Map<String, Value>| {
                    let mut entry = extra;
                    entry.insert("event".into(), json!(event));
                    report.borrow_mut()["status_checks"]
                        .as_array_mut()
                        .unwrap()
                        .push(Value::Object(entry));
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
            Some((current, other, reviewed, _)) => {
                let mut counter = |n: usize, total: usize| {
                    if n == 1 || n.is_multiple_of(8) || n == total {
                        progress(&format!("Verified changed sectors: {n}/{total}"));
                    }
                };
                development::run(
                    &mut port,
                    &context.root,
                    &loaded.baseline,
                    &update,
                    current,
                    Some(other),
                    &reviewed["operation_plan"],
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
        &report,
        result,
        execution.as_ref().map(|e| &e.3),
    )
}

/// Close a session's report the way the Python clients do.
fn finish(
    directory: &Path,
    report: &RefCell<Value>,
    result: Result<Option<Value>>,
    raw_plan: Option<&Vec<u8>>,
) -> Result<Outcome> {
    let complete = result.is_ok();
    {
        let mut report = report.borrow_mut();
        match result {
            Ok(None) => report["status"] = json!("complete"),
            Ok(Some(result)) => {
                report["status"] = json!("verified");
                report["result"] = result;
                report["reviewed_plan_sha256"] = json!(sha256(raw_plan.unwrap()));
            }
            Err(error) => {
                report["status"] = json!("stopped");
                report["error"] = json!(format!("{}: {error}", error.kind()));
            }
        }
        progress(if complete {
            "Completed; application boot remains a separate bench check"
        } else {
            "Stopped without replay; keep application boot excluded and inspect evidence"
        });
        report["end_utc"] = json!(utc());
        evidence::stream_hashes(directory, &mut report);
    }
    let saved = evidence::save_report(&directory.join("report.json"), &report.borrow());
    let report = report.borrow();
    Ok(session::closed(
        complete,
        json!({"status": report["status"], "mode": report["mode"], "output": directory,
               "result": report.get("result"), "error": report.get("error")}),
        saved,
    ))
}

/// What an execution acts on, all checked before serial access: two
/// inspected regions, the reviewed plan and its exact bytes.
type Execution = (Vec<u8>, Vec<u8>, Value, Vec<u8>);

pub struct GiveWayArgs {
    pub mode: Mode,
    pub evidence: Evidence,
    pub fallback: PathBuf,
}

struct GiveWay {
    baseline: Baseline,
    fallback: Vec<u8>,
    bindings: Value,
}

impl GiveWay {
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

fn give_way_load(context: &Context, args: &GiveWayArgs) -> Result<GiveWay> {
    let fallback = frozen(&context.root, &args.fallback, &BANK0)?;
    let (baseline, rom, small) = continued(context, &args.evidence)?;
    let bindings = json!({
        "schema_version": 1, "kind": development::GIVE_WAY_KIND, "port": args.evidence.port,
        "fallback_review_sha256": fallback.review_sha256, "baseline_sha256": baseline.sha256(),
        "rom_report_sha256": sha256(&rom), "bounded_read_report_sha256": sha256(&small),
        "tool_sha256": context.tools(),
    });
    Ok(GiveWay {
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
    let extents: Vec<(usize, usize)> = loaded.extents()?.iter().map(|(_, o, l)| (*o, *l)).collect();
    let values = session::inspected_transfers(
        &context.root,
        &path,
        &report,
        &extents,
        "Complete guard acquisition required",
    )?;
    require(
        values[0] == loaded.baseline.bytes()[..0x6000],
        "Factory region differs from the pinned baseline",
    )?;
    let operation = development::give_way_plan(&loaded.fallback, &values[1], &values[2])?;
    let mut reviewed = loaded.bindings.clone();
    reviewed["inspection_report_sha256"] = json!(report_sha);
    reviewed["operation_plan"] = operation;
    if args.mode == Mode::Prepare {
        let sha = session::write_plan(&context.root, &args.evidence.output, &reviewed)?;
        progress(&format!("Prepared give-way plan SHA256: {sha}"));
        return Ok(Outcome {
            complete: true,
            summary: json!({"status": "prepared", "plan": args.evidence.output, "plan_sha256": sha}),
        });
    }
    let raw = session::reviewed_plan(
        &context.root,
        args.evidence.plan.as_deref(),
        args.evidence.expected_plan_sha256.as_deref(),
        &reviewed,
    )?;
    let [_, bank0, bank1] = <[Vec<u8>; 3]>::try_from(values).unwrap();
    give_way_session(context, args, &loaded, Some((bank0, bank1, reviewed, raw)))
}

fn give_way_session(
    context: &Context,
    args: &GiveWayArgs,
    loaded: &GiveWay,
    execution: Option<Execution>,
) -> Result<Outcome> {
    let directory = context.root.output_directory(&args.evidence.output)?;
    if let Some((_, _, _, raw)) = &execution {
        session::keep_plan(&directory, raw)?;
    }
    let report_path = directory.join("report.json");
    let report = RefCell::new(json!({
        "mode": args.mode.name(), "status": "prepared", "start_utc": utc(),
        "bindings": loaded.bindings, "helper_upload_attempted": false, "observations": [],
        "status_checks": [], "application_boot_verified": false, "tool": "petlibre-flash", "transport": context.transport(),
    }));
    let save = || evidence::save_report(&report_path, &report.borrow());
    save()?;
    let extents = loaded.extents()?;
    let result = (|| -> Result<Option<Value>> {
        let mut port = context.record(&args.evidence.port, &directory)?;
        report.borrow_mut()["status"] = json!("running");
        save()?;
        let mut acquire = |port: &mut dyn Port| -> Result<[Vec<u8>; 3]> {
            let number = report.borrow()["observations"].as_array().unwrap().len() + 1;
            let mut observation = Vec::new();
            let mut values = Vec::new();
            for (label, offset, length) in extents {
                let (entry, data) =
                    session::acquire_region(port, &directory, label, number, offset, length)?;
                observation.push(entry);
                values.push(data);
            }
            report.borrow_mut()["observations"]
                .as_array_mut()
                .unwrap()
                .push(Value::Array(observation));
            save()?;
            progress(&format!("Guard acquisition {number} complete"));
            Ok(<[Vec<u8>; 3]>::try_from(values).unwrap())
        };
        match &execution {
            None => {
                let [factory, region, first] = acquire(&mut port)?;
                require(
                    factory == loaded.baseline.bytes()[..0x6000],
                    "Factory region differs from the pinned baseline",
                )?;
                development::give_way_plan(&loaded.fallback, &region, &first)?;
                let mut record = |event: &str, extra: Map<String, Value>| {
                    let mut entry = extra;
                    entry.insert("event".into(), json!(event));
                    report.borrow_mut()["status_checks"]
                        .as_array_mut()
                        .unwrap()
                        .push(Value::Object(entry));
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
            Some((bank0, bank1, reviewed, _)) => development::run_give_way(
                &mut port,
                &context.root,
                &loaded.baseline,
                &loaded.fallback,
                bank0,
                bank1,
                &reviewed["operation_plan"],
                &mut acquire,
                &directory.join("transaction"),
            )
            .map(Some),
        }
    })();
    finish(
        &directory,
        &report,
        result,
        execution.as_ref().map(|e| &e.3),
    )
}
