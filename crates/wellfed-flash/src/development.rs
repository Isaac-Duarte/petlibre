//! Region-scoped bank development transactions over an injected transport.
//!
//! Port of the former `tools/flash/development-bank.py`. Bank 1 (`0x186000`, the
//! factory loader's preferred bank) is the default; bank 0 (`0x6000`, the
//! fallback) has its own transaction kind and layout. Neither ever writes the
//! factory region `0x0-0x6000`, which is only read and compared with the
//! pinned baseline.
//!
//! An interrupted transaction is finished from a fresh acquisition, never
//! from the stopped journal: the marker sector must already be staged, and
//! every sector must equal its source, its staged target or erased flash.
//! One sector may instead hold the staged target's first one to three program
//! frames followed by erased flash, the only state a stopped program of an
//! erased sector leaves.

use serde_json::{Map, Value, json};

use crate::canonical::{canonical, sha256};
use crate::error::{Error, Result, require};
use crate::journal::Journal;
use crate::protocol::{Port, SECTOR};
use crate::sector::{self, Baseline};
use crate::status::{self, fields};

/// One program packet's payload; a sector takes four.
pub const FRAME: usize = 1024;
pub const FACTORY: (usize, usize) = (0, 0x6000);

/// The first eight bytes of a valid bank (with valid KM0/KM4 signatures).
/// Bank 1 has priority, so clearing its marker is how bank 0 gets to boot;
/// see docs/firmware/bank-policy.md.
pub const MARKER: &[u8; 8] = b"81958711";
pub const GIVE_WAY_KIND: &str = "plaf103_bank1_give_way";

/// One bank's region, and the other bank's first sector read as a guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub kind: &'static str,
    pub bank: u8,
    pub start: usize,
    pub end: usize,
    pub other: (usize, usize),
}

pub const BANK1: Layout = Layout {
    kind: "plaf103_bank1_development_update",
    bank: 1,
    start: sector::START,
    end: sector::END,
    other: (0x6000, SECTOR),
};

/// Bank 0 keeps the reviewed stock footprint (0x150000), like bank 1.
pub const BANK0: Layout = Layout {
    kind: "plaf103_bank0_development_update",
    bank: 0,
    start: 0x6000,
    end: 0x156000,
    other: (sector::START, SECTOR),
};

pub fn layout(bank: u8) -> Result<Layout> {
    match bank {
        0 => Ok(BANK0),
        1 => Ok(BANK1),
        _ => Err(Error::Refused("Unknown bank".into())),
    }
}

/// The aligned application payload of a complete OTA image for this bank.
pub fn payload<'a>(image: &'a [u8], layout: &Layout) -> Result<&'a [u8]> {
    let data = crate::ota::container(image)?;
    require(
        !data.is_empty() && data.len() <= layout.end - layout.start && data.len() % SECTOR == 0,
        format!(
            "Development image exceeds aligned bank-{} footprint",
            layout.bank
        ),
    )?;
    crate::ota::regions(data)?;
    Ok(data)
}

/// The acquired application extent. Without a source (bank 0 only) it covers
/// the target alone.
pub fn extent(source: Option<&[u8]>, target: &[u8], layout: &Layout) -> Result<usize> {
    let new = payload(target, layout)?.len();
    match source {
        None => {
            require(
                layout.bank == 0,
                "Only bank 0 may be overwritten without a source review",
            )?;
            Ok(new)
        }
        Some(source) => Ok(payload(source, layout)?.len().max(new)),
    }
}

/// The factory region must equal the pinned baseline, and the other bank's
/// first sector must be in a known state; once a plan pins its hash it must
/// stay byte-identical.
///
/// Bank 1: bank 0 is either the pinned baseline (an invalidated marker) or a
/// valid image, so an interrupted bank-1 update falls back to it. Bank 0:
/// bank 1 must be valid, so the unit always has a bootable bank.
pub fn guards(
    baseline: &Baseline,
    factory: &[u8],
    other: &[u8],
    layout: &Layout,
    other_sha256: Option<&str>,
) -> Result<()> {
    require(
        factory == &baseline.bytes()[FACTORY.0..FACTORY.0 + FACTORY.1],
        "Factory region differs from the pinned baseline",
    )?;

    let (at, length) = layout.other;
    require(
        other.len() == length,
        "Other-bank guard has the wrong length",
    )?;

    if layout.bank == 1 {
        require(
            other == &baseline.bytes()[at..at + length] || &other[..8] == MARKER,
            "Bank 0 is neither the pinned baseline nor a valid image; investigate before update",
        )?;
    } else {
        require(
            &other[..8] == MARKER,
            "Bank 1 must be valid while bank 0 is written",
        )?;
    }

    require(
        other_sha256.is_none_or(|expected| sha256(other) == expected),
        "Other bank changed since inspection",
    )
}

/// What one transaction acts on.
#[derive(Clone, Copy)]
pub struct Update<'a> {
    pub source: Option<&'a [u8]>,
    pub target: &'a [u8],
    pub interrupted: bool,
    pub layout: Layout,
    /// Bank 0 without a source: its pinned bytes over the target's extent.
    pub original: Option<&'a [u8]>,
    /// Bank 1 after a give-way: the source with its first marker byte cleared.
    pub restore: bool,
}

fn staged_of(proposed: &[u8]) -> Vec<u8> {
    let mut staged = proposed.to_vec();
    staged[..8].fill(0xff);
    staged
}

/// The proposed and staged (marker withheld) application region.
pub fn images(update: &Update, current: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
    let layout = &update.layout;
    let new = payload(update.target, layout)?;
    let old: &[u8] = match update.source {
        None => {
            require(
                layout.bank == 0,
                "Only bank 0 may be overwritten without a source review",
            )?;
            let original = update.original.filter(|o| o.len() == new.len());
            let original = original
                .ok_or_else(|| Error::Refused("Pinned original bank-0 region required".into()))?;
            require(
                &original[..8] != MARKER,
                "The original bank 0 is valid; review it as the source instead",
            )?;
            original
        }
        Some(source) => payload(source, layout)?,
    };

    require(
        current.len() == old.len().max(new.len()),
        "Exact source/target union acquisition required",
    )?;

    let with_tail = |base: &[u8]| {
        let mut proposed = new.to_vec();
        proposed.extend_from_slice(&base[new.len().min(base.len())..]);
        proposed
    };

    if update.restore {
        require(
            !update.interrupted && update.source.is_some(),
            "Restore applies to a reviewed source only",
        )?;
        require(
            current[0] == 0 && current[1..old.len()] == old[1..],
            "Bank 1 is not the reviewed source with only its first marker byte cleared",
        )?;
        let proposed = with_tail(current);
        let staged = staged_of(&proposed);
        return Ok((proposed, staged));
    }

    if !update.interrupted {
        require(
            &current[..old.len()] == old,
            "Current application is not the reviewed source image",
        )?;
        let proposed = with_tail(current);
        let staged = staged_of(&proposed);
        return Ok((proposed, staged));
    }

    // The tail beyond the target is never written, so it must still be the source's.
    let mut union = old.to_vec();
    union.extend_from_slice(&current[old.len()..]);
    let proposed = with_tail(&union);
    let staged = staged_of(&proposed);
    require(
        current[..SECTOR] == staged[..SECTOR],
        "Interrupted update requires the staged marker sector",
    )?;

    let erased = [0xffu8; SECTOR];
    let mut partial = 0;
    for at in (0..current.len()).step_by(SECTOR) {
        let (sector, want) = (&current[at..at + SECTOR], &staged[at..at + SECTOR]);
        if sector == &union[at..at + SECTOR] || sector == want || sector == erased {
            continue;
        }
        let frames = (FRAME..SECTOR)
            .step_by(FRAME)
            .any(|n| sector[..n] == want[..n] && sector[n..].iter().all(|b| *b == 0xff));
        require(
            frames,
            format!(
                "Sector {:#x} is neither source, staged target nor erased",
                layout.start + at
            ),
        )?;
        partial += 1;
    }

    require(partial <= 1, "More than one partially programmed sector")?;
    Ok((proposed, staged))
}

/// The reviewed plan; `other` is the inspected other-bank sector, whose hash
/// it pins.
pub fn plan(update: &Update, current: &[u8], other: Option<&[u8]>) -> Result<Value> {
    let layout = &update.layout;
    let (proposed, staged) = images(update, current)?;
    let changed = current != proposed;
    let offsets: Vec<usize> = if changed {
        (0..current.len())
            .step_by(SECTOR)
            .filter(|i| current[*i..i + SECTOR] != staged[*i..i + SECTOR])
            .collect()
    } else {
        Vec::new()
    };

    require(
        update.interrupted || !changed || offsets.first() == Some(&0),
        "Marker must be staged first",
    )?;

    let target = payload(update.target, layout)?;
    let mut out = json!({
        "schema_version": 1, "kind": layout.kind, "bank": layout.bank,
        "interrupted": update.interrupted,
        "source_ota_sha256": update.source.map(sha256),
        "target_ota_sha256": sha256(update.target),
        "source_payload_bytes": match update.source {
            None => 0, Some(source) => payload(source, layout)?.len() },
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

    Ok(out)
}

/// A pinned full-flash read's bytes over the target's extent in this bank.
pub fn original_region(full_read: &[u8], target: &[u8], layout: &Layout) -> Result<Vec<u8>> {
    require(
        full_read.len() == crate::protocol::CAPACITY,
        "A full 8 MiB flash read is required",
    )?;
    let length = payload(target, layout)?.len();
    Ok(full_read[layout.start..layout.start + length].to_vec())
}

/// A program packet bounded to this layout's bank; never the factory region.
pub fn frame(layout: &Layout, number: u8, offset: usize, data: &[u8]) -> Result<Vec<u8>> {
    require(
        offset >= FACTORY.1,
        format!("Packet outside aligned bank {}", layout.bank),
    )?;
    sector::bounded_frame(
        number,
        offset,
        data,
        layout.start,
        layout.end,
        &format!("Packet outside aligned bank {}", layout.bank),
    )
}

/// One acquisition: the application union, the factory region, and the
/// other bank's first sector.
pub struct Acquired {
    pub application: Vec<u8>,
    pub factory: Vec<u8>,
    pub other: Vec<u8>,
}

pub type Acquire<'a> = dyn FnMut(&mut dyn Port) -> Result<Acquired> + 'a;

fn same(a: &Value, b: &Value) -> Result<bool> {
    Ok(canonical(a)? == canonical(b)?)
}

/// Execute a reviewed development plan; the journal goes to `output`.
#[allow(clippy::too_many_arguments)]
pub fn run(
    port: &mut dyn Port,
    root: &crate::evidence::Root,
    baseline: &Baseline,
    update: &Update,
    current: &[u8],
    other: Option<&[u8]>,
    reviewed: &Value,
    acquire: &mut Acquire,
    output: &std::path::Path,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<Value> {
    let layout = update.layout;
    let start = layout.start;
    let expected = plan(update, current, other)?;
    let other_sha256 = expected
        .get("other_bank_sha256")
        .and_then(Value::as_str)
        .map(str::to_string);

    require(
        same(reviewed, &expected)?,
        "Reviewed development plan differs",
    )?;
    let (proposed, staged) = images(update, current)?;

    // Freeze all packets before creating durable intent or touching the transport.
    let mut sectors = Vec::new();
    for entry in expected["ordered_sectors"].as_array().unwrap() {
        let offset = entry["offset"].as_u64().unwrap() as usize;
        let at = offset - start;
        let data = staged[at..at + SECTOR].to_vec();
        let frames = (0..4)
            .map(|n| {
                frame(
                    &layout,
                    n as u8 + 1,
                    offset + n * 1024,
                    &data[n * 1024..(n + 1) * 1024],
                )
            })
            .collect::<Result<Vec<_>>>()?;
        sectors.push((offset, data, frames));
    }

    let commit = frame(&layout, 1, start, &proposed[..1024])?;
    let mut journal = Journal::create(root, output)?;

    let result = (|| -> Result<Value> {
        journal.append("transaction_start", fields(json!({"plan": expected})))?;

        // Every acquisition re-checks the factory region and the other bank's
        // guard, not only the application region being written.
        let mut check = |port: &mut dyn Port, journal: &mut Journal, want: &[u8], phase: &str| {
            let acquired = acquire(port)?;
            guards(
                baseline,
                &acquired.factory,
                &acquired.other,
                &layout,
                other_sha256.as_deref(),
            )?;
            require(
                acquired.application == want,
                format!("{phase} application region differs"),
            )?;
            journal.append(
                "region_verified",
                fields(json!({"phase": phase, "sha256": sha256(want), "bytes": want.len()})),
            )
        };

        check(port, &mut journal, current, "preimage")?;

        for (number, (offset, data, frames)) in sectors.iter().enumerate() {
            let offset = *offset;
            let at = offset - start;
            require(
                sector::read_sector(port, offset)? == current[at..at + SECTOR],
                "Sector preimage drift",
            )?;

            let mut record = |event: &str, mut extra: Map<String, Value>| {
                extra.insert("offset".into(), json!(offset));
                journal.append(event, extra)
            };

            status::check_status(port, offset, offset + SECTOR, "before_erase", &mut record)?;
            record("erase_intent", fields(json!({"bytes": SECTOR})))?;
            // After the journal fsync: no stale idle-ready byte.
            sector::erase_sector(port, offset)?;
            require(
                sector::read_sector(port, offset)? == [0xff; SECTOR],
                "Erase readback differs",
            )?;
            record("erase_verified", Map::new())?;

            status::check_status(port, offset, offset + SECTOR, "before_program", &mut record)?;
            sector::program_packets(port, frames, &mut record)?;
            require(
                sector::read_sector(port, offset)? == *data,
                "Program readback differs",
            )?;
            record("sector_staged", fields(json!({"sha256": sha256(data)})))?;

            progress(number + 1, sectors.len());
        }

        // The marker goes last, after every sector reads back staged. Interrupted
        // runs commit even when no sector is left to stage.
        if !sectors.is_empty() || update.interrupted {
            check(port, &mut journal, &staged, "staged")?;
            require(
                sector::read_sector(port, start)? == staged[..SECTOR],
                "Marker sector drift",
            )?;

            let mut record = |event: &str, mut extra: Map<String, Value>| {
                extra.insert("offset".into(), json!(start));
                extra.insert("phase".into(), json!("marker_commit"));
                journal.append(event, extra)
            };

            status::check_status(
                port,
                start,
                start + 1024,
                "before_marker_commit",
                &mut record,
            )?;
            record(
                "marker_commit_intent",
                fields(json!({"target_region_sha256": sha256(&proposed)})),
            )?;
            sector::program_packets(port, std::slice::from_ref(&commit), &mut record)?;
            require(
                sector::read_sector(port, start)? == proposed[..SECTOR],
                "Committed marker readback differs",
            )?;

            check(port, &mut journal, &proposed, "final")?;
            journal.append(
                "marker_commit_verified",
                fields(json!({"sha256": sha256(&proposed)})),
            )?;
        }

        journal.append(
            "transaction_verified",
            fields(json!({"snapshot_sha256": sha256(&proposed), "erase_sectors": sectors.len()})),
        )?;

        let report = json!({"status": "verified_via_supplied_transports",
                            "region_sha256": sha256(&proposed), "erase_sectors": sectors.len(),
                            "application_boot_verified": false, "scope": expected["scope"]});
        journal.save_report(report.clone())?;
        Ok(report)
    })();

    // A stop is terminal and never retried; an interrupted transaction is
    // finished by a fresh acquisition in interrupted mode.
    if let Err(error) = &result {
        journal.stopped(error, Map::new());
    }
    result
}

/// Clear bank 1's first marker byte so the factory loader falls back to
/// bank 0, which must hold exactly the reviewed `fallback` image: a valid
/// marker over a damaged body would leave nothing to boot. NOR programming
/// only clears bits, so the other 1023 bytes are rewritten unchanged.
pub fn give_way_plan(fallback: &[u8], bank0: &[u8], bank1_sector: &[u8]) -> Result<Value> {
    let want = payload(fallback, &BANK0)?;
    require(
        bank0 == want,
        "Bank 0 is not exactly the reviewed fallback image",
    )?;
    require(
        &want[..8] == MARKER,
        "The fallback image lacks the loader marker",
    )?;
    require(
        bank1_sector.len() == SECTOR && &bank1_sector[..8] == MARKER,
        "Bank 1's marker is not valid; nothing to give way",
    )?;

    let after = given_way(bank1_sector);
    Ok(json!({
        "schema_version": 1, "kind": GIVE_WAY_KIND, "bank": 1, "offset": sector::START,
        "fallback_ota_sha256": sha256(fallback), "bank0_region_sha256": sha256(bank0),
        "before_sha256": sha256(bank1_sector), "after_sha256": sha256(&after),
        "program_frame_sha256": sha256(&sector::program_frame(1, sector::START, &after[..1024])?),
        "scope": "One 1 KiB program frame at bank 1's start; no erase",
    }))
}

pub fn given_way(bank1_sector: &[u8]) -> Vec<u8> {
    let mut after = bank1_sector.to_vec();
    after[0] = 0;
    after
}

/// One guard acquisition for a give-way: factory, bank 0 region, bank 1's
/// first sector.
pub type AcquireGuards<'a> = dyn FnMut(&mut dyn Port) -> Result<[Vec<u8>; 3]> + 'a;

#[allow(clippy::too_many_arguments)]
pub fn run_give_way(
    port: &mut dyn Port,
    root: &crate::evidence::Root,
    baseline: &Baseline,
    fallback: &[u8],
    bank0: &[u8],
    bank1_sector: &[u8],
    reviewed: &Value,
    acquire: &mut AcquireGuards,
    output: &std::path::Path,
) -> Result<Value> {
    let expected = give_way_plan(fallback, bank0, bank1_sector)?;
    require(same(reviewed, &expected)?, "Reviewed give-way plan differs")?;

    let after = given_way(bank1_sector);
    let packet = sector::program_frame(1, sector::START, &after[..1024])?;
    let mut journal = Journal::create(root, output)?;

    let result = (|| -> Result<Value> {
        journal.append("transaction_start", fields(json!({"plan": expected})))?;
        let mut check = |port: &mut dyn Port, journal: &mut Journal, want: &[u8], phase: &str| {
            let [factory, region, first] = acquire(port)?;
            require(
                factory == baseline.bytes()[..FACTORY.1],
                format!("{phase}: factory region differs"),
            )?;
            require(
                region == bank0,
                format!("{phase}: bank 0 differs from the reviewed fallback"),
            )?;
            require(
                first == want,
                format!("{phase}: bank 1's first sector differs"),
            )?;
            journal.append("guards_verified", fields(json!({"phase": phase})))
        };

        check(port, &mut journal, bank1_sector, "preimage")?;

        let mut record = |event: &str, extra: Map<String, Value>| journal.append(event, extra);
        status::check_status(
            port,
            sector::START,
            sector::START + 1024,
            "before_give_way",
            &mut record,
        )?;
        record(
            "give_way_intent",
            fields(json!({"frame_sha256": sha256(&packet)})),
        )?;

        sector::program_packets(port, std::slice::from_ref(&packet), &mut record)?;
        require(
            sector::read_sector(port, sector::START)? == after,
            "Bank 1 first-sector readback differs",
        )?;

        check(port, &mut journal, &after, "final")?;
        journal.append(
            "give_way_verified",
            fields(json!({"after_sha256": sha256(&after)})),
        )?;

        let report = json!({"status": "verified_via_supplied_transports",
                            "after_sha256": sha256(&after), "application_boot_verified": false,
                            "scope": expected["scope"]});
        journal.save_report(report.clone())?;
        Ok(report)
    })();

    if let Err(error) = &result {
        journal.stopped(error, Map::new());
    }
    result
}
