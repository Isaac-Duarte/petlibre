//! Golden shapes: the typed plans, bindings, reports and journal events
//! against the `json!` documents the client wrote before they were typed.
//!
//! Plan digests are SHA-256 over canonical JSON, and the Python tools in
//! `src/tools/flash/` still read these files, so field names, `null` versus
//! absent and integer-ness must not drift. Each `legacy_*` builder is the
//! former `json!` verbatim, kept only as the reference.

use serde_json::{Value, json};

use super::*;
use crate::development::{given_way, images, payload};
use petlibre_formats::canonical::{canonical, canonical_of};

fn same(typed: &impl serde::Serialize, legacy: &Value) {
    assert_eq!(
        String::from_utf8(canonical_of(typed).unwrap()).unwrap(),
        String::from_utf8(canonical(legacy).unwrap()).unwrap()
    );
}

fn legacy_development_plan(update: &Update, current: &[u8], other: Option<&[u8]>) -> Value {
    let layout = &update.layout;
    let (proposed, staged) = images(update, current).unwrap();
    let changed = current != proposed;
    let offsets: Vec<usize> = if changed {
        (0..current.len())
            .step_by(SECTOR)
            .filter(|i| current[*i..i + SECTOR] != staged[*i..i + SECTOR])
            .collect()
    } else {
        Vec::new()
    };

    let target = payload(update.target, layout).unwrap();
    let mut out = json!({
        "schema_version": 1, "kind": layout.kind, "bank": layout.bank,
        "interrupted": update.interrupted,
        "source_ota_sha256": update.source.map(sha256),
        "target_ota_sha256": sha256(update.target),
        "source_payload_bytes": match update.source {
            None => 0, Some(source) => payload(source, layout).unwrap().len() },
        "target_payload_bytes": target.len(),
        "region": {"offset": layout.start, "bytes": current.len()},
        "current_region_sha256": sha256(current), "staged_region_sha256": sha256(&staged),
        "proposed_region_sha256": sha256(&proposed),
        "ordered_sectors": offsets.iter().map(|i| json!({
            "offset": layout.start + i, "bytes": SECTOR,
            "before_sha256": sha256(&current[*i..i + SECTOR]),
            "staged_sha256": sha256(&staged[*i..i + SECTOR])})).collect::<Vec<_>>(),
        "erase_sectors": offsets.len(), "staging_packets": offsets.len() * 4,
        "commit_packets": u8::from(changed), "additional_commit_erases": 0,
        "retained_tail_sha256": sha256(&current[target.len().min(current.len())..]),
        "scope": "Application union and factory/other-bank guards read back; other flash is not scanned",
    });

    if let Some(other) = other {
        out["other_bank_sha256"] = json!(sha256(other));
    }
    if update.restore {
        out["restore_marker"] = json!(true);
    }
    out
}

fn legacy_give_way_plan(fallback: &[u8], bank0: &[u8], bank1_sector: &[u8]) -> Value {
    let after = given_way(bank1_sector);
    json!({
        "schema_version": 1, "kind": "plaf103_bank1_give_way", "bank": 1, "offset": sector::START,
        "fallback_ota_sha256": sha256(fallback), "bank0_region_sha256": sha256(bank0),
        "before_sha256": sha256(bank1_sector), "after_sha256": sha256(&after),
        "program_frame_sha256": sha256(&sector::program_frame(1, sector::START, &after[..1024]).unwrap()),
        "scope": "One 1 KiB program frame at bank 1's start; no erase",
    })
}

fn legacy_recovery_plan(baseline: &Baseline, current: &[u8], created_utc: &str) -> Value {
    use crate::sector::END;
    let baseline = baseline.bytes();
    let mut sectors = Vec::new();
    let mut proposed = current.to_vec();

    for offset in (START..END).step_by(SECTOR) {
        let (before, after) = (
            &current[offset..offset + SECTOR],
            &baseline[offset..offset + SECTOR],
        );
        if before == after {
            continue;
        }

        let mut staged = after.to_vec();
        if offset == START {
            staged[..8].fill(0xff);
        }

        sectors.push(json!({
            "offset": offset, "bytes": SECTOR,
            "before_sha256": sha256(before), "after_sha256": sha256(after),
            "stage_sha256": sha256(&staged), "stage_required": before != staged,
            "marker_withheld": offset == START,
            "payload": format!("sector-{offset:06x}.bin"),
        }));
        proposed[offset..offset + SECTOR].copy_from_slice(after);
    }

    sectors.sort_by_key(|entry| {
        let offset = entry["offset"].as_u64().unwrap() as usize;
        (offset == START, offset)
    });

    let preserved: Vec<Value> = [(0, START), (END, CAPACITY)]
        .iter()
        .map(|(a, b)| json!({"start": a, "end_exclusive": b, "sha256": sha256(&current[*a..*b])}))
        .collect();

    let activation = if sectors.is_empty() {
        Value::Null
    } else {
        let mut staged = proposed.clone();
        staged[START..START + 8].fill(0xff);
        json!({"marker_offset": START, "marker_bytes": 8, "additional_erase": false,
               "staged_snapshot_sha256": sha256(&staged),
               "payload": "marker-commit-1k.bin", "payload_bytes": 1024,
               "payload_sha256": sha256(&baseline[START..START + 1024])})
    };

    json!({
        "schema_version": 2, "created_utc": created_utc,
        "baseline_sha256": sha256(baseline), "current_snapshot_sha256": sha256(current),
        "proposed_snapshot_sha256": sha256(&proposed), "target_bank": 1,
        "target_range": {"start": START, "end_exclusive": END}, "sector_bytes": SECTOR,
        "changed_sector_count": sectors.len(), "ordered_sector_payloads": sectors,
        "activation": activation,
        "preserve_current_ranges": preserved,
        "scope": "Offline plan only; no physical operation authorized or executed by this tool",
        "required_before_any_write": [
            "Operator must establish current snapshot is from the research feeder; hashes do not identify hardware",
            "Prove independent hardware loader entry and obtain stable current readback before stock can run",
            "Review pinned helper write/erase protocol and accepted protection/encryption behavior",
            "Use only sector erase; never chip erase or blanket protection/status-register clearing",
            "Check each sector against before_sha256 immediately before erasing; stop on mismatch",
            "Read back staged sectors against stage_sha256, withholding the first eight marker bytes",
            "Verify staged full image and current preserved ranges before a separate no-erase marker commit",
            "Verify full target bank and all preserved current ranges before attempting stock boot",
            "No blind retries, automatic fallback writes or writes to factory/OTP/eFuse/settings"],
    })
}

fn created(plan: &impl serde::Serialize) -> String {
    serde_json::to_value(plan).unwrap()["created_utc"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn development_plans_keep_their_canonical_bytes() {
    let source = image(30000, 3);
    let target = variant(&source, true);
    let old = development::payload(&source, &BANK1).unwrap().to_vec();
    let length = development::extent(Some(&source), &target, &BANK1).unwrap();
    let unit = unit(&old);
    let current = &unit.flash[START..START + length];
    let other = &unit.flash[0x6000..0x6000 + SECTOR];

    // A bank-1 update with its other-bank pin.
    let plain = update(Some(&source), &target, BANK1);
    same(
        &development::plan(&plain, current, Some(other)).unwrap(),
        &legacy_development_plan(&plain, current, Some(other)),
    );

    // A no-op without the pin.
    let noop = update(Some(&source), &source, BANK1);
    let unchanged = &unit.flash[START..START + old.len()];
    same(
        &development::plan(&noop, unchanged, None).unwrap(),
        &legacy_development_plan(&noop, unchanged, None),
    );

    // A marker restore after a give-way.
    let mut restore = update(Some(&source), &source, BANK1);
    restore.restore = true;
    let mut given = unchanged.to_vec();
    given[0] = 0;
    same(
        &development::plan(&restore, &given, Some(other)).unwrap(),
        &legacy_development_plan(&restore, &given, Some(other)),
    );

    // Bank 0 without a source review: its source fields are null.
    let fallback = image(20000, 9);
    let new = development::payload(&fallback, &BANK0).unwrap();
    let original = unit.flash[0x6000..0x6000 + new.len()].to_vec();
    let mut bank0 = update(None, &fallback, BANK0);
    bank0.original = Some(&original);
    let bank1_first = &unit.flash[START..START + SECTOR];
    same(
        &development::plan(&bank0, &original, Some(bank1_first)).unwrap(),
        &legacy_development_plan(&bank0, &original, Some(bank1_first)),
    );

    // An interrupted finish from a staged marker sector.
    let (_, staged) = images(&plain, current).unwrap();
    let mut stopped = current.to_vec();
    stopped[..SECTOR].copy_from_slice(&staged[..SECTOR]);
    let mut finish = plain;
    finish.interrupted = true;
    same(
        &development::plan(&finish, &stopped, Some(other)).unwrap(),
        &legacy_development_plan(&finish, &stopped, Some(other)),
    );
}

#[test]
fn give_way_plans_keep_their_canonical_bytes() {
    let installed = image(30000, 3);
    let fallback = image(20000, 9);
    let bank0 = development::payload(&fallback, &BANK0).unwrap().to_vec();
    let unit = unit(development::payload(&installed, &BANK1).unwrap());
    let first = &unit.flash[START..START + SECTOR];

    same(
        &development::give_way_plan(&fallback, &bank0, first).unwrap(),
        &legacy_give_way_plan(&fallback, &bank0, first),
    );
}

#[test]
fn recovery_plans_keep_their_canonical_bytes() {
    let installed = image(30000, 3);
    let unit = unit(development::payload(&installed, &BANK1).unwrap());
    let mut current = unit.flash.clone();
    current[START] = 0;

    let plan = crate::recovery::plan(&unit.baseline, &current).unwrap();
    same(
        &plan,
        &legacy_recovery_plan(&unit.baseline, &current, &created(&plan)),
    );

    // Nothing left to restore: no sectors and a null activation.
    let mut restored = current.clone();
    restored[START..crate::sector::END]
        .copy_from_slice(&unit.baseline.bytes()[START..crate::sector::END]);
    let empty = crate::recovery::plan(&unit.baseline, &restored).unwrap();
    same(
        &empty,
        &legacy_recovery_plan(&unit.baseline, &restored, &created(&empty)),
    );
}

/// A port that never answers: an observation of a silent line.
struct Silent;

impl Port for Silent {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        std::thread::sleep(std::time::Duration::from_millis(10));
        Ok(0)
    }

    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }

    fn in_waiting(&mut self) -> std::io::Result<usize> {
        Ok(0)
    }
}

#[test]
fn a_read_session_keeps_its_report_and_summary_fields() {
    let (checkout, root) = private_root();
    let context = Context {
        root,
        tool_sha256: hash("tool"),
        open: Box::new(|_| Ok(Box::new(Silent))),
        pin: Pin::Synthetic,
    };
    let output = checkout.path().join("private/observe");

    let outcome = crate::read::run(
        &context,
        &crate::read::Args {
            mode: crate::read::Mode::Observe,
            port: "/dev/ttyUSB0".into(),
            output: output.clone(),
            helper: None,
            candidate_rom: vec![],
            rom_report: None,
            small_report: None,
            seconds: 1.0,
            start_core: crate::protocol::Core::Km4,
        },
    )
    .unwrap();

    let text = fs::read_to_string(output.join("report.json")).unwrap();
    let report: Value = serde_json::from_str(&text).unwrap();
    let result = json!({
        "bytes": 0, "transmitted_bytes": 0, "nak_bytes": 0, "longest_nak_run": 0,
        "download_banner_seen": false, "stock_startup_markers": 0, "fault_marker_seen": false,
        "limit": "Serial output only; operator must establish power/strap state and loader identity",
    });
    let legacy = json!({
        "mode": "observe", "port": "/dev/ttyUSB0", "baud": 115200, "format": "8N1",
        "start_utc": report["start_utc"], "status": "complete",
        "helper_sha256": null,
        "flash_write_or_erase_commands": false, "capacity_bytes": CAPACITY,
        "tool": "petlibre-flash", "transport": "serial", "tool_sha256": hash("tool"),
        "limitations": ["No failed-boot recovery established",
                        "Actual ROM side effects not exhaustively verified",
                        "Helper disables OTF; a raw dump may contain ciphertext",
                        "External flash excludes separate OTP/eFuse"],
        "result": result,
        "end_utc": report["end_utc"],
        "serial-rx.bin_sha256": sha256(b""), "serial-tx.bin_sha256": sha256(b""),
    });

    // Saved pretty with sorted keys, as the untyped report was.
    assert_eq!(text, serde_json::to_string_pretty(&legacy).unwrap() + "\n");
    assert_eq!(outcome.completion, crate::session::Completion::Complete);
    assert_eq!(
        outcome.summary,
        json!({"status": "complete", "mode": "observe", "output": output,
               "result": result, "error": null})
    );
}

fn hash(name: &str) -> String {
    sha256(name.as_bytes())
}

fn tools() -> petlibre_formats::flash_plan::ToolHashes {
    petlibre_formats::flash_plan::ToolHashes {
        petlibre_flash: hash("tool"),
    }
}

#[test]
fn reviewed_update_and_give_way_bindings_keep_their_canonical_bytes() {
    use petlibre_formats::flash_plan::{GiveWayBindings, Kind, Reviewed, UpdateBindings};

    let source = image(30000, 3);
    let old = development::payload(&source, &BANK1).unwrap().to_vec();
    let unit = unit(&old);
    let current = &unit.flash[START..START + old.len()];
    let plan = development::plan(&update(Some(&source), &source, BANK1), current, None).unwrap();

    for (original, source_review) in [(None, Some(hash("source"))), (Some(hash("read")), None)] {
        let bindings = UpdateBindings {
            schema_version: 1,
            kind: Kind::Bank1Update,
            port: "/dev/ttyUSB0".into(),
            interrupted: true,
            original_read_sha256: original.clone(),
            restore_marker: false,
            source_review_sha256: source_review.clone(),
            target_review_sha256: hash("target"),
            baseline_sha256: hash("baseline"),
            rom_report_sha256: hash("rom"),
            bounded_read_report_sha256: hash("small"),
            tool_sha256: tools(),
        };
        let legacy = json!({
            "schema_version": 1, "kind": "plaf103_bank1_development_update", "port": "/dev/ttyUSB0",
            "interrupted": true,
            "original_read_sha256": original,
            "restore_marker": false,
            "source_review_sha256": source_review,
            "target_review_sha256": hash("target"),
            "baseline_sha256": hash("baseline"),
            "rom_report_sha256": hash("rom"), "bounded_read_report_sha256": hash("small"),
            "tool_sha256": {"petlibre-flash": hash("tool")},
        });
        same(&bindings, &legacy);

        let mut reviewed_legacy = legacy.clone();
        reviewed_legacy["inspection_report_sha256"] = json!(hash("inspection"));
        reviewed_legacy["operation_plan"] = serde_json::to_value(&plan).unwrap();
        same(
            &Reviewed {
                bindings,
                inspection_report_sha256: hash("inspection"),
                operation_plan: plan.clone(),
            },
            &reviewed_legacy,
        );
    }

    let give_way = GiveWayBindings {
        schema_version: 1,
        kind: Kind::GiveWay,
        port: "/dev/ttyUSB0".into(),
        fallback_review_sha256: hash("fallback"),
        baseline_sha256: hash("baseline"),
        rom_report_sha256: hash("rom"),
        bounded_read_report_sha256: hash("small"),
        tool_sha256: tools(),
    };
    same(
        &give_way,
        &json!({
            "schema_version": 1, "kind": "plaf103_bank1_give_way", "port": "/dev/ttyUSB0",
            "fallback_review_sha256": hash("fallback"), "baseline_sha256": hash("baseline"),
            "rom_report_sha256": hash("rom"), "bounded_read_report_sha256": hash("small"),
            "tool_sha256": {"petlibre-flash": hash("tool")},
        }),
    );
}

#[test]
fn reviewed_stock_recovery_keeps_its_canonical_bytes() {
    let installed = image(30000, 3);
    let unit = unit(development::payload(&installed, &BANK1).unwrap());
    let mut current = unit.flash.clone();
    current[START] = 0;
    let plan = crate::recovery::plan(&unit.baseline, &current).unwrap();

    same(
        &crate::stock::Reviewed {
            schema_version: 1,
            kind: petlibre_formats::flash_plan::Kind::StockRecovery,
            port: "/dev/ttyUSB0".into(),
            recovery_plan: plan.clone(),
            current_report_sha256: hash("current"),
            bounded_read_report_sha256: hash("small"),
            rom_report_sha256: hash("rom"),
            tool_sha256: tools(),
        },
        &json!({
            "schema_version": 1, "kind": "original_stock_bank1_recovery", "port": "/dev/ttyUSB0",
            "recovery_plan": serde_json::to_value(&plan).unwrap(),
            "current_report_sha256": hash("current"),
            "bounded_read_report_sha256": hash("small"), "rom_report_sha256": hash("rom"),
            "tool_sha256": {"petlibre-flash": hash("tool")},
        }),
    );
}

#[test]
fn journal_events_keep_their_fields() {
    use crate::journal::{At, Event};

    let at = |offset, phase, event| {
        serde_json::to_value(At {
            offset,
            phase,
            event,
        })
        .unwrap()
    };
    let plain = |event: Event| serde_json::to_value(event).unwrap();
    let assessment = crate::status::assess_range([0, 2, 0], START, START + SECTOR).unwrap();

    let cases = [
        (
            at(
                START,
                Some("marker_commit"),
                Event::FlashStatusChecked {
                    boundary: "before_marker_commit",
                    registers: [0, 2, 0],
                    assessment: assessment.clone(),
                },
            ),
            json!({"event": "flash_status_checked", "offset": START, "phase": "marker_commit",
                   "boundary": "before_marker_commit", "registers": [0, 2, 0],
                   "assessment": serde_json::to_value(&assessment).unwrap()}),
        ),
        (
            at(
                START,
                None,
                Event::FlashStatusUnstable {
                    boundary: "before_erase",
                    start: START,
                    end_exclusive: START + SECTOR,
                    first: [1, 2, 3],
                    second: [1, 2, 4],
                },
            ),
            json!({"event": "flash_status_unstable", "offset": START, "boundary": "before_erase",
                   "start": START, "end_exclusive": START + SECTOR,
                   "first": [1, 2, 3], "second": [1, 2, 4]}),
        ),
        (
            at(
                START,
                None,
                Event::EraseIntent {
                    bytes: SECTOR,
                    after_sha256: None,
                },
            ),
            json!({"event": "erase_intent", "offset": START, "bytes": SECTOR}),
        ),
        (
            at(
                START,
                None,
                Event::EraseIntent {
                    bytes: SECTOR,
                    after_sha256: Some(hash("after")),
                },
            ),
            json!({"event": "erase_intent", "offset": START, "bytes": SECTOR,
                   "after_sha256": hash("after")}),
        ),
        (
            at(START, None, Event::EraseVerified {}),
            json!({"event": "erase_verified", "offset": START}),
        ),
        (
            at(
                START,
                Some("marker_commit"),
                Event::ProgramPacketIntent {
                    sequence: 1,
                    frame_sha256: hash("frame"),
                },
            ),
            json!({"event": "program_packet_intent", "offset": START, "phase": "marker_commit",
                   "sequence": 1, "frame_sha256": hash("frame")}),
        ),
        (
            plain(Event::ProgramEotIntent {}),
            json!({"event": "program_eot_intent"}),
        ),
        (
            at(
                START,
                Some("marker_commit"),
                Event::DevelopmentCommitIntent {
                    target_region_sha256: hash("target"),
                },
            ),
            json!({"event": "marker_commit_intent", "offset": START, "phase": "marker_commit",
                   "target_region_sha256": hash("target")}),
        ),
        (
            plain(Event::DevelopmentCommitVerified {
                sha256: hash("proposed"),
            }),
            json!({"event": "marker_commit_verified", "sha256": hash("proposed")}),
        ),
        (
            plain(Event::DevelopmentVerified {
                snapshot_sha256: hash("proposed"),
                erase_sectors: 3,
            }),
            json!({"event": "transaction_verified", "snapshot_sha256": hash("proposed"),
                   "erase_sectors": 3}),
        ),
        (
            plain(Event::RecoveryVerified {
                snapshot_sha256: hash("after"),
                sector_count: 2,
                marker_commit: true,
            }),
            json!({"event": "transaction_verified", "snapshot_sha256": hash("after"),
                   "sector_count": 2, "marker_commit": true}),
        ),
        (
            plain(Event::RecoveryStart {
                transaction_id: "t".into(),
                baseline_sha256: hash("baseline"),
                current_sha256: hash("current"),
                plan_sha256: hash("plan"),
            }),
            json!({"event": "transaction_start", "transaction_id": "t",
                   "baseline_sha256": hash("baseline"), "current_sha256": hash("current"),
                   "plan_sha256": hash("plan")}),
        ),
        (
            at(
                START,
                None,
                Event::Stopped {
                    mutation_possible: true,
                    error_type: "io",
                },
            ),
            json!({"event": "stopped", "offset": START, "mutation_possible": true,
                   "error_type": "io"}),
        ),
        (
            plain(Event::TransactionStopped {
                error_type: "refused",
                mutation_possible: false,
            }),
            json!({"event": "transaction_stopped", "error_type": "refused",
                   "mutation_possible": false}),
        ),
        (
            plain(Event::GuardsVerified { phase: "final" }),
            json!({"event": "guards_verified", "phase": "final"}),
        ),
        (
            plain(Event::RegionVerified {
                phase: "preimage",
                sha256: hash("region"),
                bytes: SECTOR,
            }),
            json!({"event": "region_verified", "phase": "preimage", "sha256": hash("region"),
                   "bytes": SECTOR}),
        ),
    ];

    for (typed, legacy) in cases {
        same(&typed, &legacy);
    }
}
