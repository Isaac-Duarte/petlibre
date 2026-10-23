//! GD25Q64E status decoding and reads over an existing helper session.
//!
//! Port of `src/tools/flash/flash-status.py`. Only the 05/35/15 one-byte status
//! reads exist; there is no status write, WREN, erase, resume or reset.

use serde::Serialize;

use crate::error::{Result, refuse, require};
use crate::journal::{Event, Record};
use crate::protocol::{self, CAPACITY, Port};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Range {
    pub start: usize,
    pub end_exclusive: usize,
}

/// The decoded registers, field for field as `flash-status.py` names them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Decoded {
    pub part: &'static str,
    pub sr1: u8,
    pub sr2: u8,
    pub sr3: u8,
    pub busy: bool,
    pub write_enable_latch: bool,
    pub bp4_to_bp0: u8,
    pub complement: bool,
    pub erase_suspended: bool,
    pub program_suspended: bool,
    pub quad_enable: bool,
    pub srp1_srp0: u8,
    pub security_register_locks: [bool; 3],
    pub driver_strength_field: u8,
    pub dummy_configuration: bool,
    pub sr3_reserved_bits_raw: u8,
    pub protected_ranges: Vec<Range>,
    pub scope: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Blocking {
    FlashBusy,
    EraseOrProgramSuspended,
    SoftwareProtectedRange,
}

impl Blocking {
    fn name(self) -> &'static str {
        match self {
            Self::FlashBusy => "flash_busy",
            Self::EraseOrProgramSuspended => "erase_or_program_suspended",
            Self::SoftwareProtectedRange => "software_protected_range",
        }
    }
}

/// Whether the software status permits a write range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Assessment {
    pub range: Range,
    pub protected_overlap: Vec<Range>,
    pub software_preconditions_met: bool,
    pub blocking_reasons: Vec<Blocking>,
    pub physical_write_permission_established: bool,
    pub decoded: Decoded,
}

fn protected(sr1: u8, sr2: u8) -> Option<(usize, usize)> {
    let bp = (sr1 >> 2) & 31;
    let level = bp & 7;
    let (small, lower, complement) = (bp & 16 != 0, bp & 8 != 0, sr2 & 64 != 0);

    let range = match level {
        0 => None,
        7 => Some((0, CAPACITY)),
        _ => {
            let length = if small {
                4096 << (level - 1).min(3)
            } else {
                1 << (16 + level)
            };
            Some(if lower {
                (0, length)
            } else {
                (CAPACITY - length, CAPACITY)
            })
        }
    };

    if !complement {
        return range;
    }
    match range {
        None => Some((0, CAPACITY)),
        Some((0, CAPACITY)) => None,
        Some((0, end)) => Some((end, CAPACITY)),
        Some((start, _)) => Some((0, start)),
    }
}

pub fn decode(sr1: u8, sr2: u8, sr3: u8) -> Decoded {
    let ranges = protected(sr1, sr2)
        .map(|(start, end_exclusive)| Range {
            start,
            end_exclusive,
        })
        .into_iter()
        .collect();

    Decoded {
        part: "GD25Q64E",
        sr1,
        sr2,
        sr3,
        busy: sr1 & 1 != 0,
        write_enable_latch: sr1 & 2 != 0,
        bp4_to_bp0: (sr1 >> 2) & 31,
        complement: sr2 & 64 != 0,
        erase_suspended: sr2 & 128 != 0,
        program_suspended: sr2 & 4 != 0,
        quad_enable: sr2 & 2 != 0,
        srp1_srp0: ((sr2 & 1) << 1) | (sr1 >> 7),
        security_register_locks: [3, 4, 5].map(|bit| sr2 & (1 << bit) != 0),
        driver_strength_field: (sr3 >> 5) & 3,
        dummy_configuration: sr3 & 1 != 0,
        sr3_reserved_bits_raw: sr3 & 0x9e,
        protected_ranges: ranges,
        scope: "Manufacturer BP/CMP and status fields only; physical write permission not established",
    }
}

/// Whether the software status permits writing `start..end`; physical
/// write permission is never established here.
pub fn assess_range(registers: [u8; 3], start: usize, end: usize) -> Result<Assessment> {
    require(
        start < end && end <= CAPACITY,
        "Range must be within the 8 MiB main array",
    )?;

    let [sr1, sr2, sr3] = registers;
    let decoded = decode(sr1, sr2, sr3);
    let overlaps: Vec<Range> = protected(sr1, sr2)
        .filter(|(a, b)| start < *b && *a < end)
        .map(|(a, b)| Range {
            start: start.max(a),
            end_exclusive: end.min(b),
        })
        .into_iter()
        .collect();

    let mut reasons = Vec::new();
    if decoded.busy {
        reasons.push(Blocking::FlashBusy);
    }
    if decoded.erase_suspended || decoded.program_suspended {
        reasons.push(Blocking::EraseOrProgramSuspended);
    }
    if !overlaps.is_empty() {
        reasons.push(Blocking::SoftwareProtectedRange);
    }

    Ok(Assessment {
        range: Range {
            start,
            end_exclusive: end,
        },
        protected_overlap: overlaps,
        software_preconditions_met: reasons.is_empty(),
        blocking_reasons: reasons,
        physical_write_permission_established: false,
        decoded,
    })
}

/// Three whitelisted one-byte status reads from an existing helper session.
pub fn read_status_registers(port: &mut dyn Port) -> Result<[u8; 3]> {
    let mut values = [0u8; 3];
    for (value, opcode) in values.iter_mut().zip([0x05u8, 0x35, 0x15]) {
        protocol::command_ready(port)?;
        protocol::send(port, &[0x21, opcode, 1])?;
        protocol::expect(port, 0x21)?;
        *value = protocol::exact_read(port, 1, 3.0)?[0];
    }
    Ok(values)
}

/// Two matching sequential reads guard a mutation boundary; no poll, retry
/// or status write.
pub fn check_status(
    port: &mut dyn Port,
    start: usize,
    end: usize,
    boundary: &'static str,
    record: &mut Record,
) -> Result<Assessment> {
    let first = read_status_registers(port)?;
    let second = read_status_registers(port)?;

    if first != second {
        record(Event::FlashStatusUnstable {
            boundary,
            start,
            end_exclusive: end,
            first,
            second,
        })?;
        return refuse("Flash status changed between observations; mutation refused");
    }

    let assessed = assess_range(second, start, end)?;
    record(Event::FlashStatusChecked {
        boundary,
        registers: second,
        assessment: assessed.clone(),
    })?;

    let reasons: Vec<&str> = assessed
        .blocking_reasons
        .iter()
        .map(|reason| reason.name())
        .collect();
    require(
        assessed.software_preconditions_met,
        format!("Flash status blocks {boundary}: {}", reasons.join(", ")),
    )?;

    Ok(assessed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ranges(sr1: u8, sr2: u8) -> Option<(usize, usize)> {
        protected(sr1, sr2)
    }

    #[test]
    fn block_protect_levels_and_complement() {
        assert_eq!(ranges(0, 2), None);
        // BP0 alone: the top 128 KiB.
        assert_eq!(ranges(1 << 2, 0), Some((CAPACITY - 0x20000, CAPACITY)));
        // TB (BP3): the bottom instead.
        assert_eq!(ranges((1 | 8) << 2, 0), Some((0, 0x20000)));
        // SEC (BP4) with level 2: 8 KiB sectors.
        assert_eq!(
            ranges((2 | 16) << 2, 0),
            Some((CAPACITY - 0x2000, CAPACITY))
        );
        assert_eq!(ranges(7 << 2, 0), Some((0, CAPACITY)));
        assert_eq!(ranges(0, 64), Some((0, CAPACITY)));
        assert_eq!(ranges(7 << 2, 64), None);
        assert_eq!(ranges(1 << 2, 64), Some((0, CAPACITY - 0x20000)));
        assert_eq!(ranges((1 | 8) << 2, 64), Some((0x20000, CAPACITY)));
    }

    #[test]
    fn assessment_names_each_blocking_reason() {
        let clear = assess_range([0, 2, 0], 0x186000, 0x2d6000).unwrap();
        assert!(clear.software_preconditions_met);
        assert!(!clear.physical_write_permission_established);

        let busy = assess_range([1 | (7 << 2), 4, 0], 0x186000, 0x2d6000).unwrap();
        let busy = serde_json::to_value(&busy).unwrap();
        assert_eq!(
            busy["blocking_reasons"],
            json!([
                "flash_busy",
                "erase_or_program_suspended",
                "software_protected_range"
            ])
        );
        assert_eq!(
            busy["protected_overlap"],
            json!([{"start": 0x186000, "end_exclusive": 0x2d6000}])
        );
        assert!(assess_range([0, 0, 0], 5, 5).is_err());
    }

    #[test]
    fn decoding_keeps_the_python_fields() {
        let decoded = serde_json::to_value(decode(0x9d, 0x7f, 0xff)).unwrap();
        assert_eq!(
            decoded,
            json!({
                "part": "GD25Q64E", "sr1": 0x9d, "sr2": 0x7f, "sr3": 0xff,
                "busy": true, "write_enable_latch": false, "bp4_to_bp0": 7, "complement": true,
                "erase_suspended": false, "program_suspended": true, "quad_enable": true,
                "srp1_srp0": 3, "security_register_locks": [true, true, true],
                "driver_strength_field": 3, "dummy_configuration": true,
                "sr3_reserved_bits_raw": 0x9e, "protected_ranges": [],
                "scope": "Manufacturer BP/CMP and status fields only; physical write permission not established",
            })
        );
    }
}
