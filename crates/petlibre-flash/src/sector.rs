//! Bounded sector transport over an established helper session.
//!
//! Port of `tools/flash/recovery-sector-transport.py`. Fixed status reads
//! guard every mutation boundary, each mutation intent is recorded before
//! transmission, and a NAK or timeout after any mutation is terminal.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::error::{Result, require};
use crate::protocol::{self, ACK, CAPACITY, EOT, Port, SECTOR};
use crate::status::{self, Record, fields};
use petlibre_formats::canonical::sha256;

/// The research unit's original full-flash backup.
pub const BASELINE_SHA256: &str =
    "2751e6c870af7312fe1ff0039831ac5799fb215d629d85af8720b6e50a7479bc";

/// Bank 1, the factory loader's preferred bank.
pub const START: usize = 0x186000;
pub const END: usize = 0x2D6000;

/// Flash addresses as the helper's program frames name them.
pub const FLASH_BASE: u32 = 0x0800_0000;

/// The pinned original 8 MiB backup; the only accepted baseline.
#[derive(Clone)]
pub struct Baseline(Arc<[u8]>);

impl Baseline {
    pub fn pinned(data: Vec<u8>) -> Result<Self> {
        require(
            data.len() == CAPACITY && sha256(&data) == BASELINE_SHA256,
            "Pinned full baseline required",
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
    message: &str,
) -> Result<Vec<u8>> {
    require((1..=4).contains(&number), "Packet number must be 1-4")?;
    require(
        start <= offset && offset + 1024 <= end && offset.is_multiple_of(1024),
        message,
    )?;
    let payload: &[u8; 1024] = payload
        .try_into()
        .map_err(|_| crate::Error::Refused("Full immutable 1 KiB payload required".into()))?;
    Ok(protocol::frame(number, FLASH_BASE + offset as u32, payload))
}

pub fn program_frame(number: u8, offset: usize, payload: &[u8]) -> Result<Vec<u8>> {
    bounded_frame(
        number,
        offset,
        payload,
        START,
        END,
        "Packet outside aligned stock bank 1",
    )
}

pub fn read_sector(port: &mut dyn Port, offset: usize) -> Result<Vec<u8>> {
    protocol::read_region(port, offset, SECTOR)
}

pub fn program_packets(port: &mut dyn Port, frames: &[Vec<u8>], record: &mut Record) -> Result<()> {
    protocol::command_ready(port)?;
    protocol::baudcheck(port)?;

    for (index, frame) in frames.iter().enumerate() {
        let sequence = index + 1;
        record(
            "program_packet_intent",
            fields(json!({"sequence": sequence, "frame_sha256": sha256(frame)})),
        )?;
        protocol::send(port, frame)?;
        protocol::expect(port, ACK)?;
        record("program_packet_ack", fields(json!({"sequence": sequence})))?;
    }

    record("program_eot_intent", Default::default())?;
    protocol::send(port, &[EOT])?;
    protocol::expect(port, ACK)
}

/// Erase one sector after durable intent, and verify the erase.
pub fn erase_sector(port: &mut dyn Port, offset: usize) -> Result<()> {
    // Durable storage can stall beyond the helper's 50 ms ready interval.
    // Drain idle bytes after that stall, immediately before transmission.
    protocol::command_ready(port)?;
    protocol::send(port, &protocol::erase_request(offset))?;
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

fn sector_frames(offset: usize, payload: &[u8]) -> Result<Vec<Vec<u8>>> {
    (0..4)
        .map(|i| {
            program_frame(
                i as u8 + 1,
                offset + i * 1024,
                &payload[i * 1024..(i + 1) * 1024],
            )
        })
        .collect()
}

/// Restore one original bank-1 sector of this pinned unit; never retried.
pub fn restore_sector(
    port: &mut dyn Port,
    baseline: &Baseline,
    offset: usize,
    expected_before: &str,
    journal: &mut Record,
) -> Result<Value> {
    require(
        (START..=END - SECTOR).contains(&offset) && offset.is_multiple_of(SECTOR),
        "Sector outside aligned stock bank 1",
    )?;
    require(
        petlibre_formats::hex::is_sha256(expected_before),
        "Expected before-sector hash required",
    )?;

    let payload = sector_payload(baseline, offset);
    let after = sha256(&payload);
    let frames = sector_frames(offset, &payload)?;
    let mut mutation_possible = false;

    let mut record = |event: &str, mut extra: serde_json::Map<String, Value>| {
        extra.insert("offset".into(), json!(offset));
        journal(event, extra)
    };

    let result = (|| -> Result<Value> {
        let before = read_sector(port, offset)?;
        require(
            sha256(&before) == expected_before,
            "Current sector differs from reviewed before hash",
        )?;

        if offset == START && before == payload {
            record("marker_already_staged", fields(json!({"sha256": after})))?;
            return Ok(
                json!({"offset": offset, "bytes": SECTOR, "before_sha256": expected_before,
                             "after_sha256": after, "program_packets": 0, "marker_withheld": true}),
            );
        }

        require(
            before != payload,
            "Sector already matches baseline; no erase required",
        )?;

        if offset == START {
            require(
                before[..8] != baseline.bytes()[START..START + 8],
                "Target marker is still valid; stop for plan review",
            )?;
        }

        record(
            "before_verified",
            fields(json!({"sha256": expected_before})),
        )?;
        status::check_status(port, offset, offset + SECTOR, "before_erase", &mut record)?;
        record(
            "erase_intent",
            fields(json!({"bytes": SECTOR, "after_sha256": after})),
        )?;

        // From here on a stop may have changed flash, and its record says so.
        mutation_possible = true;
        erase_sector(port, offset)?;
        require(
            read_sector(port, offset)? == [0xff; SECTOR],
            "Erase readback is not all 0xff; no program attempted",
        )?;
        record("erase_readback_verified", Default::default())?;

        status::check_status(port, offset, offset + SECTOR, "before_program", &mut record)?;
        program_packets(port, &frames, &mut record)?;
        require(
            read_sector(port, offset)? == payload,
            "Program readback differs from exact baseline sector",
        )?;

        record("sector_verified", fields(json!({"sha256": after})))?;
        Ok(
            json!({"offset": offset, "bytes": SECTOR, "before_sha256": expected_before,
                  "after_sha256": after, "program_packets": 4, "marker_withheld": offset == START}),
        )
    })();

    if let Err(error) = &result {
        let _ = record(
            "stopped",
            fields(json!({"mutation_possible": mutation_possible, "error_type": error.kind()})),
        );
    }
    result
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
    journal: &mut Record,
) -> Result<Value> {
    require(
        current.len() == CAPACITY,
        "Exact current full snapshot required",
    )?;

    let mut proposed = current.to_vec();
    proposed[START..END].copy_from_slice(&baseline.bytes()[START..END]);
    let mut staged = proposed.clone();
    staged[START..START + 8].fill(0xff);
    let staged_sector = staged[START..START + SECTOR].to_vec();
    let mut mutation_possible = false;

    let mut record = |event: &str, mut extra: serde_json::Map<String, Value>| {
        extra.insert("offset".into(), json!(START));
        extra.insert("phase".into(), json!("marker_commit"));
        journal(event, extra)
    };

    let result = (|| -> Result<(Value, Vec<u8>)> {
        let mut full = |port: &mut dyn Port| -> Result<Vec<u8>> {
            let data = snapshot(port)?;
            require(
                data.len() == CAPACITY,
                "Snapshot provider must return exactly 8 MiB",
            )?;
            Ok(data)
        };

        require(
            full(port)? == staged,
            "Full staged image or preserved ranges differ; marker remains uncommitted",
        )?;
        require(
            read_sector(port, START)? == staged_sector,
            "Marker sector changed after full verification",
        )?;

        record(
            "marker_preconditions_verified",
            fields(json!({"staged_snapshot_sha256": sha256(&staged)})),
        )?;
        status::check_status(
            port,
            START,
            START + 1024,
            "before_marker_commit",
            &mut record,
        )?;

        let frame = program_frame(1, START, &baseline.bytes()[START..START + 1024])?;
        record(
            "marker_commit_intent",
            fields(json!({"proposed_snapshot_sha256": sha256(&proposed)})),
        )?;

        mutation_possible = true;
        program_packets(port, &[frame], &mut record)?;
        require(
            read_sector(port, START)? == baseline.bytes()[START..START + SECTOR],
            "Committed marker-sector readback differs",
        )?;

        let after = full(port)?;
        require(
            after == proposed,
            "Final image or preserved ranges differ after marker commit",
        )?;
        record(
            "marker_commit_verified",
            fields(json!({"snapshot_sha256": sha256(&after)})),
        )?;

        Ok((
            json!({"snapshot_sha256": sha256(&after), "marker_bytes": 8, "program_packets": 1,
                   "additional_erases": 0}),
            after,
        ))
    })();

    match result {
        Ok((report, _)) => Ok(report),
        Err(error) => {
            let _ = record(
                "stopped",
                fields(json!({"mutation_possible": mutation_possible, "error_type": error.kind()})),
            );
            Err(error)
        }
    }
}
