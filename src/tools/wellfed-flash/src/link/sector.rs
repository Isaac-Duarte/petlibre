//! Bounded sector transport over an established helper session.
//!
//! Port of `src/tools/flash/recovery-sector-transport.py`. Fixed status reads
//! guard every mutation boundary, each mutation intent is recorded before
//! transmission, and a NAK or timeout after any mutation is terminal.

use std::fmt;
use std::sync::Arc;

use serde::Serialize;

use crate::error::Result;
use crate::evidence::journal::{At, Boundary, Event, Located, Phase, Record};
use crate::link::protocol::{self, ACK, CAPACITY, EOT, Port, SECTOR};
use crate::link::status;
use wellfed_formats::canonical::sha256;
use wellfed_formats::require;
use wellfed_image::bank::BANK1;

/// Sector transport and marker-commit refusals. Those after an erase intent
/// leave flash possibly changed, which the journal's stop record says.
#[derive(Debug, thiserror::Error)]
pub enum SectorError {
    #[error("Pinned full baseline required")]
    UnpinnedBaseline { bytes: usize },
    #[error("Packet number must be 1-4")]
    PacketNumber { number: u8 },
    #[error("Packet outside aligned {bank}")]
    PacketOutside { bank: Bank, offset: usize },
    #[error("Full immutable 1 KiB payload required")]
    PartialPayload { bytes: usize },
    #[error("Full sector payload required")]
    PartialSector { bytes: usize },

    #[error("Sector outside aligned stock bank 1")]
    SectorOutside { offset: usize },
    #[error("Expected before-sector hash required")]
    BeforeHash,
    #[error("Current sector differs from reviewed before hash")]
    BeforeDiffers { offset: usize },
    #[error("Sector already matches baseline; no erase required")]
    AlreadyBaseline { offset: usize },
    #[error("Target marker is still valid; stop for plan review")]
    MarkerStillValid,
    #[error("Erase readback is not all 0xff; no program attempted")]
    EraseReadback { offset: usize },
    #[error("Program readback differs from exact baseline sector")]
    ProgramReadback { offset: usize },

    #[error("Exact current full snapshot required")]
    PartialCurrent { bytes: usize },
    #[error("Snapshot provider must return exactly 8 MiB")]
    SnapshotLength { bytes: usize },
    #[error("Full staged image or preserved ranges differ; marker remains uncommitted")]
    StagedDiffers,
    #[error("Marker sector changed after full verification")]
    MarkerSectorChanged,
    #[error("Committed marker-sector readback differs")]
    CommittedReadback,
    #[error("Final image or preserved ranges differ after marker commit")]
    FinalDiffers,
}

/// The bank a program frame is bounded to, as its refusal names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bank {
    /// Stock recovery and give-ways: bank 1 only.
    Stock,
    /// A development layout's own bank.
    Development(u8),
}

impl fmt::Display for Bank {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stock => f.write_str("stock bank 1"),
            Self::Development(bank) => write!(f, "bank {bank}"),
        }
    }
}

/// The research unit's original full-flash backup.
pub const BASELINE_SHA256: &str =
    "2751e6c870af7312fe1ff0039831ac5799fb215d629d85af8720b6e50a7479bc";

/// Bank 1, the factory loader's preferred bank.
pub const START: usize = BANK1.start;
pub const END: usize = BANK1.end;

/// Flash addresses as the helper's program frames name them.
pub const FLASH_BASE: u32 = 0x0800_0000;

/// The pinned original 8 MiB backup; the only accepted baseline.
#[derive(Clone)]
pub struct Baseline(Arc<[u8]>);

impl Baseline {
    pub fn pinned(data: Vec<u8>) -> Result<Self> {
        require(
            data.len() == CAPACITY && sha256(&data) == BASELINE_SHA256,
            || SectorError::UnpinnedBaseline { bytes: data.len() },
        )?;
        Ok(Self(data.into()))
    }

    /// Host tests only: a synthetic stand-in that is never the pinned unit.
    #[cfg(test)]
    pub fn synthetic(data: Vec<u8>) -> Self {
        assert_eq!(data.len(), CAPACITY);
        Self(data.into())
    }

    pub fn bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn sha256(&self) -> String {
        sha256(&self.0)
    }
}

/// A full 1 KiB program frame inside `start..end`.
pub fn bounded_frame(
    number: u8,
    offset: usize,
    payload: &[u8],
    start: usize,
    end: usize,
    bank: Bank,
) -> Result<Vec<u8>> {
    require((1..=4).contains(&number), || SectorError::PacketNumber {
        number,
    })?;
    require(
        start <= offset && offset + 1024 <= end && offset.is_multiple_of(1024),
        || SectorError::PacketOutside { bank, offset },
    )?;
    let Ok(bytes) = <&[u8; 1024]>::try_from(payload) else {
        return Err(SectorError::PartialPayload {
            bytes: payload.len(),
        }
        .into());
    };
    let Some(address) = u32::try_from(offset)
        .ok()
        .and_then(|offset| FLASH_BASE.checked_add(offset))
    else {
        return Err(SectorError::PacketOutside { bank, offset }.into());
    };

    Ok(protocol::frame(number, address, bytes))
}

pub fn program_frame(number: u8, offset: usize, payload: &[u8]) -> Result<Vec<u8>> {
    bounded_frame(number, offset, payload, START, END, Bank::Stock)
}

pub fn read_sector(port: &mut dyn Port, offset: usize) -> Result<Vec<u8>> {
    protocol::read_region(port, offset, SECTOR)
}

pub fn program_packets(port: &mut dyn Port, frames: &[Vec<u8>], record: &mut Record) -> Result<()> {
    protocol::command_ready(port)?;
    protocol::baudcheck(port)?;

    for (index, frame) in frames.iter().enumerate() {
        let sequence = index + 1;
        record(Event::ProgramPacketIntent {
            sequence,
            frame_sha256: sha256(frame),
        })?;
        protocol::send(port, frame)?;
        protocol::expect(port, ACK)?;
        record(Event::ProgramPacketAck { sequence })?;
    }

    record(Event::ProgramEotIntent {})?;
    protocol::send(port, &[EOT])?;
    protocol::expect(port, ACK)
}

/// Erase one sector after durable intent, and verify the erase.
pub fn erase_sector(port: &mut dyn Port, offset: usize) -> Result<()> {
    let request = protocol::erase_request(offset)?;

    // Durable storage can stall beyond the helper's 50 ms ready interval.
    // Drain idle bytes after that stall, immediately before transmission.
    protocol::command_ready(port)?;
    protocol::send(port, &request)?;
    protocol::expect(port, ACK)
}

/// The baseline's bank-1 sector, with the marker withheld in the first.
pub fn sector_payload(baseline: &Baseline, offset: usize) -> Vec<u8> {
    let mut payload = baseline.bytes()[offset..offset + SECTOR].to_vec();
    if offset == START {
        payload[..8].fill(0xff);
    }
    payload
}

/// A sector's four program frames, numbered 1-4.
pub fn sector_frames(
    offset: usize,
    payload: &[u8],
    frame: impl Fn(u8, usize, &[u8]) -> Result<Vec<u8>>,
) -> Result<Vec<Vec<u8>>> {
    require(payload.len() == SECTOR, || SectorError::PartialSector {
        bytes: payload.len(),
    })?;

    (1..=4u8)
        .zip(payload.chunks(1024))
        .map(|(number, chunk)| {
            let at = offset + usize::from(number - 1) * 1024;
            frame(number, at, chunk)
        })
        .collect()
}

/// One restored sector, as the recovery report lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Restored {
    pub offset: usize,
    pub bytes: usize,
    pub before_sha256: String,
    pub after_sha256: String,
    pub program_packets: usize,
    pub marker_withheld: bool,
}

/// Restore one original bank-1 sector of this pinned unit; never retried.
pub fn restore_sector(
    port: &mut dyn Port,
    baseline: &Baseline,
    offset: usize,
    expected_before: &str,
    journal: &mut Located,
) -> Result<Restored> {
    require(
        (START..=END - SECTOR).contains(&offset) && offset.is_multiple_of(SECTOR),
        || SectorError::SectorOutside { offset },
    )?;
    require(wellfed_formats::hex::is_sha256(expected_before), || {
        SectorError::BeforeHash
    })?;

    let payload = sector_payload(baseline, offset);
    let after = sha256(&payload);
    let frames = sector_frames(offset, &payload, program_frame)?;
    let mut mutation_possible = false;

    let mut record = |event: Event| {
        journal(At {
            offset,
            phase: None,
            event,
        })
    };
    let restored = |program_packets, marker_withheld| Restored {
        offset,
        bytes: SECTOR,
        before_sha256: expected_before.to_string(),
        after_sha256: after.clone(),
        program_packets,
        marker_withheld,
    };

    let result = (|| -> Result<Restored> {
        let before = read_sector(port, offset)?;
        require(sha256(&before) == expected_before, || {
            SectorError::BeforeDiffers { offset }
        })?;

        if offset == START && before == payload {
            record(Event::MarkerAlreadyStaged {
                sha256: after.clone(),
            })?;
            return Ok(restored(0, true));
        }

        require(before != payload, || SectorError::AlreadyBaseline {
            offset,
        })?;

        if offset == START {
            require(before[..8] != baseline.bytes()[START..START + 8], || {
                SectorError::MarkerStillValid
            })?;
        }

        record(Event::BeforeVerified {
            sha256: expected_before.to_string(),
        })?;
        status::check_status(
            port,
            offset,
            offset + SECTOR,
            Boundary::BeforeErase,
            &mut record,
        )?;
        record(Event::EraseIntent {
            bytes: SECTOR,
            after_sha256: Some(after.clone()),
        })?;

        // From here on a stop may have changed flash, and its record says so.
        mutation_possible = true;
        erase_sector(port, offset)?;
        require(read_sector(port, offset)? == [0xff; SECTOR], || {
            SectorError::EraseReadback { offset }
        })?;
        record(Event::EraseReadbackVerified {})?;

        status::check_status(
            port,
            offset,
            offset + SECTOR,
            Boundary::BeforeProgram,
            &mut record,
        )?;
        program_packets(port, &frames, &mut record)?;
        require(read_sector(port, offset)? == payload, || {
            SectorError::ProgramReadback { offset }
        })?;

        record(Event::SectorVerified {
            sha256: after.clone(),
        })?;
        Ok(restored(4, offset == START))
    })();

    if let Err(error) = &result {
        let _ = record(Event::Stopped {
            mutation_possible,
            error_type: error.kind(),
        });
    }
    result
}

/// The marker commit, as the recovery report records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MarkerCommit {
    pub snapshot_sha256: String,
    pub marker_bytes: usize,
    pub program_packets: usize,
    pub additional_erases: usize,
}

/// Commit the eight marker bytes after exact staged-image verification.
///
/// The helper's program wrapper does not erase: the first full 1 KiB chunk
/// is reprogrammed with baseline bytes, and only the withheld marker needs
/// 1-to-0 transitions.
pub fn commit_marker(
    port: &mut dyn Port,
    baseline: &Baseline,
    current: &[u8],
    snapshot: &mut dyn FnMut(&mut dyn Port) -> Result<Vec<u8>>,
    journal: &mut Located,
) -> Result<MarkerCommit> {
    require(current.len() == CAPACITY, || SectorError::PartialCurrent {
        bytes: current.len(),
    })?;

    let mut proposed = current.to_vec();
    proposed[START..END].copy_from_slice(&baseline.bytes()[START..END]);
    let mut staged = proposed.clone();
    staged[START..START + 8].fill(0xff);
    let staged_sector = staged[START..START + SECTOR].to_vec();
    let mut mutation_possible = false;

    let mut record = |event: Event| {
        journal(At {
            offset: START,
            phase: Some(Phase::MarkerCommit),
            event,
        })
    };

    let result = (|| -> Result<MarkerCommit> {
        let mut full = |port: &mut dyn Port| -> Result<Vec<u8>> {
            let data = snapshot(port)?;
            require(data.len() == CAPACITY, || SectorError::SnapshotLength {
                bytes: data.len(),
            })?;
            Ok(data)
        };

        require(full(port)? == staged, || SectorError::StagedDiffers)?;
        require(read_sector(port, START)? == staged_sector, || {
            SectorError::MarkerSectorChanged
        })?;

        record(Event::MarkerPreconditionsVerified {
            staged_snapshot_sha256: sha256(&staged),
        })?;
        status::check_status(
            port,
            START,
            START + 1024,
            Boundary::BeforeMarkerCommit,
            &mut record,
        )?;

        let frame = program_frame(1, START, &baseline.bytes()[START..START + 1024])?;
        record(Event::MarkerCommitIntent {
            proposed_snapshot_sha256: sha256(&proposed),
        })?;

        mutation_possible = true;
        program_packets(port, &[frame], &mut record)?;
        require(
            read_sector(port, START)? == baseline.bytes()[START..START + SECTOR],
            || SectorError::CommittedReadback,
        )?;

        let after = full(port)?;
        require(after == proposed, || SectorError::FinalDiffers)?;
        record(Event::MarkerCommitVerified {
            snapshot_sha256: sha256(&after),
        })?;

        Ok(MarkerCommit {
            snapshot_sha256: sha256(&after),
            marker_bytes: 8,
            program_packets: 1,
            additional_erases: 0,
        })
    })();

    if let Err(error) = &result {
        let _ = record(Event::Stopped {
            mutation_possible,
            error_type: error.kind(),
        });
    }
    result
}
