//! Region-scoped bank development transactions over an injected transport.
//!
//! Port of the former `src/tools/flash/development-bank.py`. Bank 1 (`0x186000`, the
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

use std::path::Path;

use serde::Serialize;

use crate::error::Result;
use crate::evidence::Root;
use crate::evidence::journal::{At, Boundary, Event, Journal, Phase};
use crate::evidence::report::SummaryStatus;
use crate::link::protocol::{Port, SECTOR};
use crate::link::sector::{self, Baseline};
use crate::link::status;
use wellfed_formats::canonical::{canonical_of, sha256};
use wellfed_formats::flash_plan::{DevelopmentPlan, GiveWayPlan, Kind, PlannedSector, Region};
use wellfed_formats::require;
pub(crate) use wellfed_image::bank::{BANK0, Layout, layout, payload};

/// A development update or give-way that does not match its guards, its
/// reviewed plan or its readback. Offsets are flash addresses.
#[derive(Debug, thiserror::Error)]
pub enum DevelopmentError {
    #[error("Only bank 0 may be overwritten without a source review")]
    SourceReviewRequired { bank: u8 },
    #[error("Factory region differs from the pinned baseline")]
    FactoryDiffers,
    #[error("Other-bank guard has the wrong length")]
    OtherGuardLength { bytes: usize, length: usize },
    #[error("Bank 0 is neither the pinned baseline nor a valid image; investigate before update")]
    Bank0Unknown,
    #[error("Bank 1 must be valid while bank 0 is written")]
    Bank1Invalid,
    #[error("Other bank changed since inspection")]
    OtherBankChanged,

    #[error("Pinned original bank-0 region required")]
    OriginalMissing,
    #[error("The original bank 0 is valid; review it as the source instead")]
    OriginalValid,
    #[error("Exact source/target union acquisition required")]
    UnionLength { bytes: usize, length: usize },
    #[error("Restore applies to a reviewed source only")]
    RestoreScope,
    #[error("Bank 1 is not the reviewed source with only its first marker byte cleared")]
    NotGivenWay,
    #[error("Current application is not the reviewed source image")]
    NotSource,
    #[error("Interrupted update requires the staged marker sector")]
    MarkerNotStaged,
    #[error("Sector {offset:#x} is neither source, staged target nor erased")]
    UnknownSector { offset: usize },
    #[error("More than one partially programmed sector")]
    PartialSectors { count: usize },
    #[error("Marker must be staged first")]
    MarkerNotFirst,
    #[error("A full 8 MiB flash read is required")]
    FullReadRequired { bytes: usize },

    #[error("Reviewed development plan differs")]
    PlanDiffers,
    #[error("Planned sector outside the bank")]
    SectorOutsideBank { offset: u64 },
    #[error("{phase} application region differs")]
    RegionDiffers { phase: Phase },
    #[error("Sector preimage drift")]
    PreimageDrift { offset: usize },
    #[error("Erase readback differs")]
    EraseReadback { offset: usize },
    #[error("Program readback differs")]
    ProgramReadback { offset: usize },
    #[error("Marker sector drift")]
    MarkerDrift,
    #[error("Committed marker readback differs")]
    CommitReadback,

    #[error("Bank 0 is not exactly the reviewed fallback image")]
    FallbackDiffers,
    #[error("The fallback image lacks the loader marker")]
    FallbackUnmarked,
    #[error("Bank 1's marker is not valid; nothing to give way")]
    NothingToGiveWay,
    #[error("Reviewed give-way plan differs")]
    GiveWayPlanDiffers,
    #[error("{phase}: factory region differs")]
    GuardFactory { phase: Phase },
    #[error("{phase}: bank 0 differs from the reviewed fallback")]
    GuardBank0 { phase: Phase },
    #[error("{phase}: bank 1's first sector differs")]
    GuardBank1 { phase: Phase },
    #[error("Bank 1 first-sector readback differs")]
    GiveWayReadback,
}

/// One program packet's payload; a sector takes four.
pub const FRAME: usize = 1024;
pub const FACTORY: (usize, usize) = (0, 0x6000);

/// The first eight bytes of a valid bank (with valid KM0/KM4 signatures).
/// Bank 1 has priority, so clearing its marker is how bank 0 gets to boot;
/// see docs/firmware/bank-policy.md.
pub const MARKER: &[u8; 8] = b"81958711";

/// The acquired application extent. Without a source (bank 0 only) it covers
/// the target alone.
pub fn extent(source: Option<&[u8]>, target: &[u8], layout: &Layout) -> Result<usize> {
    let new = payload(target, layout)?.len();
    match source {
        None => {
            require(layout.bank == 0, || {
                DevelopmentError::SourceReviewRequired { bank: layout.bank }
            })?;
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
        || DevelopmentError::FactoryDiffers,
    )?;

    let (at, length) = layout.other;
    require(other.len() == length, || {
        DevelopmentError::OtherGuardLength {
            bytes: other.len(),
            length,
        }
    })?;

    if layout.bank == 1 {
        require(
            other == &baseline.bytes()[at..at + length] || &other[..8] == MARKER,
            || DevelopmentError::Bank0Unknown,
        )?;
    } else {
        require(&other[..8] == MARKER, || DevelopmentError::Bank1Invalid)?;
    }

    require(
        other_sha256.is_none_or(|expected| sha256(other) == expected),
        || DevelopmentError::OtherBankChanged.into(),
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
            require(layout.bank == 0, || {
                DevelopmentError::SourceReviewRequired { bank: layout.bank }
            })?;
            let Some(original) = update.original.filter(|o| o.len() == new.len()) else {
                return Err(DevelopmentError::OriginalMissing.into());
            };
            require(&original[..8] != MARKER, || DevelopmentError::OriginalValid)?;
            original
        }
        Some(source) => payload(source, layout)?,
    };

    let length = old.len().max(new.len());
    require(current.len() == length, || DevelopmentError::UnionLength {
        bytes: current.len(),
        length,
    })?;

    let with_tail = |base: &[u8]| {
        let mut proposed = new.to_vec();
        proposed.extend_from_slice(&base[new.len().min(base.len())..]);
        proposed
    };

    if update.restore {
        require(!update.interrupted && update.source.is_some(), || {
            DevelopmentError::RestoreScope
        })?;
        require(current[0] == 0 && current[1..old.len()] == old[1..], || {
            DevelopmentError::NotGivenWay
        })?;
        let proposed = with_tail(current);
        let staged = staged_of(&proposed);
        return Ok((proposed, staged));
    }

    if !update.interrupted {
        require(&current[..old.len()] == old, || DevelopmentError::NotSource)?;
        let proposed = with_tail(current);
        let staged = staged_of(&proposed);
        return Ok((proposed, staged));
    }

    // The tail beyond the target is never written, so it must still be the source's.
    let mut union = old.to_vec();
    union.extend_from_slice(&current[old.len()..]);
    let proposed = with_tail(&union);
    let staged = staged_of(&proposed);
    require(current[..SECTOR] == staged[..SECTOR], || {
        DevelopmentError::MarkerNotStaged
    })?;

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
        require(frames, || DevelopmentError::UnknownSector {
            offset: layout.start + at,
        })?;
        partial += 1;
    }

    require(partial <= 1, || DevelopmentError::PartialSectors {
        count: partial,
    })?;
    Ok((proposed, staged))
}

/// The reviewed plan; `other` is the inspected other-bank sector, whose hash
/// it pins.
pub fn plan(update: &Update, current: &[u8], other: Option<&[u8]>) -> Result<DevelopmentPlan> {
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
        || DevelopmentError::MarkerNotFirst,
    )?;

    let target = payload(update.target, layout)?;
    let source_payload_bytes = match update.source {
        None => 0,
        Some(source) => payload(source, layout)?.len(),
    };
    let ordered_sectors = offsets
        .iter()
        .map(|i| PlannedSector {
            offset: wide(layout.start + i),
            bytes: wide(SECTOR),
            before_sha256: sha256(&current[*i..i + SECTOR]),
            staged_sha256: sha256(&staged[*i..i + SECTOR]),
        })
        .collect();

    Ok(DevelopmentPlan {
        schema_version: 1,
        kind: layout.kind,
        bank: layout.bank,
        interrupted: update.interrupted,
        source_ota_sha256: update.source.map(sha256),
        target_ota_sha256: sha256(update.target),
        source_payload_bytes: wide(source_payload_bytes),
        target_payload_bytes: wide(target.len()),
        region: Region {
            offset: wide(layout.start),
            bytes: wide(current.len()),
        },
        current_region_sha256: sha256(current),
        staged_region_sha256: sha256(&staged),
        proposed_region_sha256: sha256(&proposed),
        ordered_sectors,
        erase_sectors: wide(offsets.len()),
        staging_packets: wide(offsets.len() * 4),
        commit_packets: u64::from(changed),
        additional_commit_erases: 0,
        retained_tail_sha256: sha256(&current[target.len().min(current.len())..]),
        scope:
            "Application union and factory/other-bank guards read back; other flash is not scanned"
                .into(),
        other_bank_sha256: other.map(sha256),
        restore_marker: update.restore,
    })
}

/// Plans count in `u64`, which every host `usize` fits.
fn wide(value: usize) -> u64 {
    value as u64
}

/// A pinned full-flash read's bytes over the target's extent in this bank.
pub fn original_region(full_read: &[u8], target: &[u8], layout: &Layout) -> Result<Vec<u8>> {
    require(full_read.len() == crate::link::protocol::CAPACITY, || {
        DevelopmentError::FullReadRequired {
            bytes: full_read.len(),
        }
    })?;
    let length = payload(target, layout)?.len();
    Ok(full_read[layout.start..layout.start + length].to_vec())
}

/// A program packet bounded to this layout's bank; never the factory region.
pub fn frame(layout: &Layout, number: u8, offset: usize, data: &[u8]) -> Result<Vec<u8>> {
    let bank = sector::Bank::Development(layout.bank);
    require(offset >= FACTORY.1, || sector::SectorError::PacketOutside {
        bank,
        offset,
    })?;
    sector::bounded_frame(number, offset, data, layout.start, layout.end, bank)
}

/// One acquisition: the application union, the factory region, and the
/// other bank's first sector.
pub struct Acquired {
    pub application: Vec<u8>,
    pub factory: Vec<u8>,
    pub other: Vec<u8>,
}

pub type Acquire<'a> = dyn FnMut(&mut dyn Port) -> Result<Acquired> + 'a;

fn same(a: &impl Serialize, b: &impl Serialize) -> Result<bool> {
    Ok(canonical_of(a)? == canonical_of(b)?)
}

/// The report a verified transaction saves beside its journal.
#[derive(Debug, Clone, Serialize)]
pub struct Verified {
    pub status: SummaryStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub erase_sectors: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_sha256: Option<String>,
    pub application_boot_verified: bool,
    pub scope: String,
}

/// A reviewed development transaction and everything its plan derives from.
pub struct Transaction<'a> {
    pub baseline: &'a Baseline,
    pub update: &'a Update<'a>,
    /// The inspected application region.
    pub current: &'a [u8],
    /// The inspected other-bank guard sector.
    pub other: Option<&'a [u8]>,
    pub reviewed: &'a DevelopmentPlan,
}

/// Execute a reviewed development plan; the journal goes to `output`.
pub fn run(
    port: &mut dyn Port,
    root: &Root,
    transaction: &Transaction,
    acquire: &mut Acquire,
    output: &Path,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<Verified> {
    let Transaction {
        baseline,
        update,
        current,
        other,
        reviewed,
    } = *transaction;
    let layout = update.layout;
    let start = layout.start;
    let expected = plan(update, current, other)?;

    require(same(reviewed, &expected)?, || DevelopmentError::PlanDiffers)?;
    let (proposed, staged) = images(update, current)?;

    // Freeze all packets before creating durable intent or touching the transport.
    let mut sectors = Vec::new();
    for entry in &expected.ordered_sectors {
        let Some((offset, at)) = usize::try_from(entry.offset)
            .ok()
            .and_then(|offset| Some((offset, offset.checked_sub(start)?)))
        else {
            return Err(DevelopmentError::SectorOutsideBank {
                offset: entry.offset,
            }
            .into());
        };

        let data = staged[at..at + SECTOR].to_vec();
        let frames = sector::sector_frames(offset, &data, |number, at, chunk| {
            frame(&layout, number, at, chunk)
        })?;
        sectors.push((offset, data, frames));
    }

    let commit = frame(&layout, 1, start, &proposed[..1024])?;
    let mut journal = Journal::create(root, output)?;

    let result = (|| -> Result<Verified> {
        journal.append(&Event::DevelopmentStart {
            plan: expected.clone(),
        })?;

        // Every acquisition re-checks the factory region and the other bank's
        // guard, not only the application region being written.
        let mut check = |port: &mut dyn Port, journal: &mut Journal, want: &[u8], phase: Phase| {
            let acquired = acquire(port)?;
            guards(
                baseline,
                &acquired.factory,
                &acquired.other,
                &layout,
                expected.other_bank_sha256.as_deref(),
            )?;
            require(acquired.application == want, || {
                DevelopmentError::RegionDiffers { phase }
            })?;
            journal.append(&Event::RegionVerified {
                phase,
                sha256: sha256(want),
                bytes: want.len(),
            })
        };

        check(port, &mut journal, current, Phase::Preimage)?;

        for (number, (offset, data, frames)) in sectors.iter().enumerate() {
            let offset = *offset;
            let at = offset - start;
            require(
                sector::read_sector(port, offset)? == current[at..at + SECTOR],
                || DevelopmentError::PreimageDrift { offset },
            )?;

            let mut record = |event: Event| {
                journal.append(&At {
                    offset,
                    phase: None,
                    event,
                })
            };

            status::check_status(
                port,
                offset,
                offset + SECTOR,
                Boundary::BeforeErase,
                &mut record,
            )?;
            record(Event::EraseIntent {
                bytes: SECTOR,
                after_sha256: None,
            })?;
            // After the journal fsync: no stale idle-ready byte.
            sector::erase_sector(port, offset)?;
            require(sector::read_sector(port, offset)? == [0xff; SECTOR], || {
                DevelopmentError::EraseReadback { offset }
            })?;
            record(Event::EraseVerified {})?;

            status::check_status(
                port,
                offset,
                offset + SECTOR,
                Boundary::BeforeProgram,
                &mut record,
            )?;
            sector::program_packets(port, frames, &mut record)?;
            require(sector::read_sector(port, offset)? == *data, || {
                DevelopmentError::ProgramReadback { offset }
            })?;
            record(Event::SectorStaged {
                sha256: sha256(data),
            })?;

            progress(number + 1, sectors.len());
        }

        // The marker goes last, after every sector reads back staged. Interrupted
        // runs commit even when no sector is left to stage.
        if !sectors.is_empty() || update.interrupted {
            check(port, &mut journal, &staged, Phase::Staged)?;
            require(
                sector::read_sector(port, start)? == staged[..SECTOR],
                || DevelopmentError::MarkerDrift,
            )?;

            let mut record = |event: Event| {
                journal.append(&At {
                    offset: start,
                    phase: Some(Phase::MarkerCommit),
                    event,
                })
            };

            status::check_status(
                port,
                start,
                start + 1024,
                Boundary::BeforeMarkerCommit,
                &mut record,
            )?;
            record(Event::DevelopmentCommitIntent {
                target_region_sha256: sha256(&proposed),
            })?;
            sector::program_packets(port, std::slice::from_ref(&commit), &mut record)?;
            require(
                sector::read_sector(port, start)? == proposed[..SECTOR],
                || DevelopmentError::CommitReadback,
            )?;

            check(port, &mut journal, &proposed, Phase::Final)?;
            journal.append(&Event::DevelopmentCommitVerified {
                sha256: sha256(&proposed),
            })?;
        }

        journal.append(&Event::DevelopmentVerified {
            snapshot_sha256: sha256(&proposed),
            erase_sectors: sectors.len(),
        })?;

        let report = Verified {
            status: SummaryStatus::VerifiedViaSuppliedTransports,
            region_sha256: Some(sha256(&proposed)),
            erase_sectors: Some(sectors.len()),
            after_sha256: None,
            application_boot_verified: false,
            scope: expected.scope.clone(),
        };
        journal.save_report(&report)?;
        Ok(report)
    })();

    // A stop is terminal and never retried; an interrupted transaction is
    // finished by a fresh acquisition in interrupted mode.
    if let Err(error) = &result {
        journal.stopped(error, None);
    }
    result
}

/// Clear bank 1's first marker byte so the factory loader falls back to
/// bank 0, which must hold exactly the reviewed `fallback` image: a valid
/// marker over a damaged body would leave nothing to boot. NOR programming
/// only clears bits, so the other 1023 bytes are rewritten unchanged.
pub fn give_way_plan(fallback: &[u8], bank0: &[u8], bank1_sector: &[u8]) -> Result<GiveWayPlan> {
    let want = payload(fallback, &BANK0)?;
    require(bank0 == want, || DevelopmentError::FallbackDiffers)?;
    require(&want[..8] == MARKER, || DevelopmentError::FallbackUnmarked)?;
    require(
        bank1_sector.len() == SECTOR && &bank1_sector[..8] == MARKER,
        || DevelopmentError::NothingToGiveWay,
    )?;

    let after = given_way(bank1_sector);
    Ok(GiveWayPlan {
        schema_version: 1,
        kind: Kind::GiveWay,
        bank: 1,
        offset: wide(sector::START),
        fallback_ota_sha256: sha256(fallback),
        bank0_region_sha256: sha256(bank0),
        before_sha256: sha256(bank1_sector),
        after_sha256: sha256(&after),
        program_frame_sha256: sha256(&sector::program_frame(1, sector::START, &after[..1024])?),
        scope: "One 1 KiB program frame at bank 1's start; no erase".into(),
    })
}

pub fn given_way(bank1_sector: &[u8]) -> Vec<u8> {
    let mut after = bank1_sector.to_vec();
    after[0] = 0;
    after
}

/// One guard acquisition for a give-way: factory, bank 0 region, bank 1's
/// first sector.
pub type AcquireGuards<'a> = dyn FnMut(&mut dyn Port) -> Result<[Vec<u8>; 3]> + 'a;

/// A reviewed give-way and the inspected bytes its plan derives from.
pub struct GiveWay<'a> {
    pub baseline: &'a Baseline,
    pub fallback: &'a [u8],
    pub bank0: &'a [u8],
    pub bank1_sector: &'a [u8],
    pub reviewed: &'a GiveWayPlan,
}

pub fn run_give_way(
    port: &mut dyn Port,
    root: &Root,
    give_way: &GiveWay,
    acquire: &mut AcquireGuards,
    output: &Path,
) -> Result<Verified> {
    let GiveWay {
        baseline,
        fallback,
        bank0,
        bank1_sector,
        reviewed,
    } = *give_way;
    let expected = give_way_plan(fallback, bank0, bank1_sector)?;
    require(same(reviewed, &expected)?, || {
        DevelopmentError::GiveWayPlanDiffers
    })?;

    let after = given_way(bank1_sector);
    let packet = sector::program_frame(1, sector::START, &after[..1024])?;
    let mut journal = Journal::create(root, output)?;

    let result = (|| -> Result<Verified> {
        journal.append(&Event::GiveWayStart {
            plan: expected.clone(),
        })?;
        let mut check = |port: &mut dyn Port, journal: &mut Journal, want: &[u8], phase: Phase| {
            let [factory, region, first] = acquire(port)?;
            require(factory == baseline.bytes()[..FACTORY.1], || {
                DevelopmentError::GuardFactory { phase }
            })?;
            require(region == bank0, || DevelopmentError::GuardBank0 { phase })?;
            require(first == want, || DevelopmentError::GuardBank1 { phase })?;
            journal.append(&Event::GuardsVerified { phase })
        };

        check(port, &mut journal, bank1_sector, Phase::Preimage)?;

        let mut record = |event: Event| journal.append(&event);
        status::check_status(
            port,
            sector::START,
            sector::START + 1024,
            Boundary::BeforeGiveWay,
            &mut record,
        )?;
        record(Event::GiveWayIntent {
            frame_sha256: sha256(&packet),
        })?;

        sector::program_packets(port, std::slice::from_ref(&packet), &mut record)?;
        require(sector::read_sector(port, sector::START)? == after, || {
            DevelopmentError::GiveWayReadback
        })?;

        check(port, &mut journal, &after, Phase::Final)?;
        journal.append(&Event::GiveWayVerified {
            after_sha256: sha256(&after),
        })?;

        let report = Verified {
            status: SummaryStatus::VerifiedViaSuppliedTransports,
            region_sha256: None,
            erase_sectors: None,
            after_sha256: Some(sha256(&after)),
            application_boot_verified: false,
            scope: expected.scope.clone(),
        };
        journal.save_report(&report)?;
        Ok(report)
    })();

    if let Err(error) = &result {
        journal.stopped(error, None);
    }
    result
}
