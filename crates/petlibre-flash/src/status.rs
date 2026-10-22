//! GD25Q64E status decoding and reads over an existing helper session.
//!
//! Port of `tools/flash/flash-status.py`. Only the 05/35/15 one-byte status
//! reads exist; there is no status write, WREN, erase, resume or reset.

use serde_json::{Map, Value, json};

use crate::error::{Result, require};
use crate::protocol::{self, CAPACITY, Port};

/// Journal or report sink for one event and its fields.
pub type Record<'a> = dyn FnMut(&str, Map<String, Value>) -> Result<()> + 'a;

pub fn fields(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => panic!("event fields must be an object"),
    }
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

pub fn decode(sr1: u8, sr2: u8, sr3: u8) -> Value {
    let ranges: Vec<Value> = protected(sr1, sr2)
        .map(|(start, end)| json!({"start": start, "end_exclusive": end}))
        .into_iter()
        .collect();
    let locks = [3, 4, 5].map(|bit| sr2 & (1 << bit) != 0);
    json!({
        "part": "GD25Q64E", "sr1": sr1, "sr2": sr2, "sr3": sr3,
        "busy": sr1 & 1 != 0, "write_enable_latch": sr1 & 2 != 0,
        "bp4_to_bp0": (sr1 >> 2) & 31, "complement": sr2 & 64 != 0,
        "erase_suspended": sr2 & 128 != 0, "program_suspended": sr2 & 4 != 0,
        "quad_enable": sr2 & 2 != 0, "srp1_srp0": ((sr2 & 1) << 1) | (sr1 >> 7),
        "security_register_locks": locks,
        "driver_strength_field": (sr3 >> 5) & 3, "dummy_configuration": sr3 & 1 != 0,
        "sr3_reserved_bits_raw": sr3 & 0x9e,
        "protected_ranges": ranges,
        "scope": "Manufacturer BP/CMP and status fields only; physical write permission not established",
    })
}

/// Whether the software status permits writing `start..end`; physical
/// write permission is never established here.
pub fn assess_range(registers: [u8; 3], start: usize, end: usize) -> Result<Value> {
    require(
        start < end && end <= CAPACITY,
        "Range must be within the 8 MiB main array",
    )?;
    let [sr1, sr2, sr3] = registers;
    let decoded = decode(sr1, sr2, sr3);
    let overlaps: Vec<Value> = protected(sr1, sr2)
        .filter(|(a, b)| start < *b && *a < end)
        .map(|(a, b)| json!({"start": start.max(a), "end_exclusive": end.min(b)}))
        .into_iter()
        .collect();
    let mut reasons = Vec::new();
    if decoded["busy"] == true {
        reasons.push("flash_busy");
    }
    if decoded["erase_suspended"] == true || decoded["program_suspended"] == true {
        reasons.push("erase_or_program_suspended");
    }
    if !overlaps.is_empty() {
        reasons.push("software_protected_range");
    }
    Ok(json!({
        "range": {"start": start, "end_exclusive": end}, "protected_overlap": overlaps,
        "software_preconditions_met": reasons.is_empty(), "blocking_reasons": reasons,
        "physical_write_permission_established": false, "decoded": decoded,
    }))
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
    boundary: &str,
    record: &mut Record,
) -> Result<Value> {
    let first = read_status_registers(port)?;
    let second = read_status_registers(port)?;
    if first != second {
        record(
            "flash_status_unstable",
            fields(
                json!({"boundary": boundary, "start": start, "end_exclusive": end,
                          "first": first, "second": second}),
            ),
        )?;
        return Err(crate::Error::Refused(
            "Flash status changed between observations; mutation refused".into(),
        ));
    }
    let assessed = assess_range(second, start, end)?;
    record(
        "flash_status_checked",
        fields(json!({"boundary": boundary, "registers": second, "assessment": assessed})),
    )?;
    let reasons: Vec<&str> = assessed["blocking_reasons"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    require(
        assessed["software_preconditions_met"] == true,
        format!("Flash status blocks {boundary}: {}", reasons.join(", ")),
    )?;
    Ok(assessed)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(clear["software_preconditions_met"], true);
        assert_eq!(clear["physical_write_permission_established"], false);
        let busy = assess_range([1 | (7 << 2), 4, 0], 0x186000, 0x2d6000).unwrap();
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
}
