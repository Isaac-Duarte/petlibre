//! The reviewed stock KM0 pad table, reproduced in the SDK source and
//! audited in the linked image, without device I/O.
//!
//! The pinned SDK's `pmap_func` differs from stock in exactly eight active
//! pulls, which the overlay sets to no-pull. Source text alone cannot detect
//! stale objects, so the linked table's bytes are compared with stock too.

use std::sync::LazyLock;

use petlibre_formats::layout::Hex;
use regex::Regex;
use serde::Serialize;

use super::{refuse, word};
use crate::{Result, require, sha256};

const STOCK_SHA256: &str = "05147476fc70c91cc77ad7da6d6772376b2081d460ac7b1912e2d73868a09498";
const STOCK_BYTES: usize = 1_376_288;
const TABLE_ADDRESS: u64 = 0x0C01_3328;
const XIP_ADDRESS: u64 = 0x0C00_0020;
const TABLE_ROWS: usize = 65;
const ROW_BYTES: usize = 20;
const CHANGED_PINS: [u32; 8] = [12, 13, 14, 15, 18, 30, 33, 34];
const NO_PIN: u32 = 0xffff_ffff;

static TABLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?ms)const\s+PMAP_TypeDef\s+pmap_func\[\]\s*=\s*\{(?P<body>.*?)^\};")
        .expect("table pattern")
});
static ROW: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[ \t]*\{[ \t]*(_P[AB]_\d+|_PNC)[ \t]*,[ \t]*([^{}\r\n]+)\},[^\r\n]*$")
        .expect("row pattern")
});
static ACTIVE_PULL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([^,]+,[ \t]*)GPIO_PuPd_[A-Z]+").expect("pull pattern"));

type Row = [u32; 5];

fn pull(name: &str) -> Option<u32> {
    Some(match name {
        "GPIO_PuPd_NOPULL" => 0,
        "GPIO_PuPd_DOWN" => 1,
        "GPIO_PuPd_UP" => 2,
        "GPIO_PuPd_SHUTDOWN" => 3,
        "GPIO_PuPd_KEEP" => 255,
        _ => return None,
    })
}

fn flag(name: &str) -> Option<u32> {
    match name {
        "FALSE" => Some(0),
        "TRUE" => Some(1),
        _ => None,
    }
}

/// The stock table's bytes and rows: pin, active pull, sleep pull, low-power
/// pull, flag.
fn stock_table(reference: &[u8]) -> Result<(&[u8], Vec<Row>)> {
    require(
        reference.len() == STOCK_BYTES && sha256(reference) == STOCK_SHA256,
        "Pin table requires the exact verified stock OTA",
    )?;
    let start = 64 + (TABLE_ADDRESS - XIP_ADDRESS) as usize;
    let data = &reference[start..start + TABLE_ROWS * ROW_BYTES];
    let rows: Vec<Row> = data
        .as_chunks::<ROW_BYTES>()
        .0
        .iter()
        .map(|row| {
            let (words, _) = row.as_chunks::<4>();
            std::array::from_fn(|i| u32::from_le_bytes(words[i]))
        })
        .collect();

    let pins: Vec<u32> = rows.iter().map(|row| row[0]).collect();
    let expected: Vec<u32> = (0..64).chain([NO_PIN]).collect();
    require(pins == expected, "Unexpected stock pin order or sentinel")?;
    Ok((data, rows))
}

/// Patch the eight active pulls; refuse any other discrepancy with stock.
pub(crate) fn overlay(source: &str, reference: &[u8]) -> Result<String> {
    let (_, expected) = stock_table(reference)?;
    let tables: Vec<_> = TABLE.captures_iter(source).collect();
    let [table] = &tables[..] else {
        return refuse("Expected one pinned SDK pmap_func table");
    };
    let body = table.name("body").expect("TABLE always captures its body");

    let (mut output, mut rows, mut changed) = (String::new(), 0usize, Vec::new());
    for line in body.as_str().split_inclusive('\n') {
        let mut line = line.to_string();
        if let Some(m) = ROW.captures(line.trim_end_matches(['\r', '\n'])) {
            let name = &m[1];
            let pin = if name == "_PNC" {
                NO_PIN
            } else {
                (if &name[2..3] == "B" { 32 } else { 0 }) + name[4..].parse::<u32>().unwrap_or(0)
            };
            let values: Vec<&str> = m[2].split(',').map(str::trim).collect();
            let parsed: Option<Row> = (values.len() == 4)
                .then(|| {
                    Some([
                        pin,
                        pull(values[0])?,
                        pull(values[1])?,
                        pull(values[2])?,
                        flag(values[3])?,
                    ])
                })
                .flatten();
            let Some(row) = parsed else {
                return refuse("Unrecognized SDK pad fields");
            };

            require(
                rows < TABLE_ROWS && row[0] == expected[rows][0],
                "SDK pin order or sentinel differs from stock",
            )?;
            let target = expected[rows];
            require(
                row[2..] == target[2..],
                "SDK sleep/low-power fields differ from stock",
            )?;
            if row[1] != target[1] {
                require(
                    CHANGED_PINS.contains(&pin) && target[1] == 0,
                    "Unexpected active pull discrepancy",
                )?;
                require(
                    ACTIVE_PULL.is_match(&line),
                    "Cannot replace SDK active pull",
                )?;
                line = ACTIVE_PULL
                    .replace(&line, "${1}GPIO_PuPd_NOPULL")
                    .into_owned();
                changed.push(pin);
            }
            rows += 1;
        }
        output.push_str(&line);
    }

    require(
        rows == TABLE_ROWS && changed == CHANGED_PINS,
        "Expected 65 records and the eight reviewed active pull changes",
    )?;
    Ok(format!(
        "{}{output}{}",
        &source[..body.start()],
        &source[body.end()..]
    ))
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PinmapAudit {
    runtime_address: Hex,
    rows: usize,
    bytes: usize,
    sha256: String,
    matches_stock: bool,
    scope: &'static str,
}

/// Check the linked table's bytes in a candidate OTA image against stock.
pub(crate) fn audit_linked_table(
    candidate: &[u8],
    address: u64,
    reference: &[u8],
) -> Result<PinmapAudit> {
    let (expected, _) = stock_table(reference)?;
    let header = [32, 36, 40, 44].map(|at| word(candidate, at));
    let ([Some(magic1), Some(magic2), Some(size), Some(base)], 64..) = (header, candidate.len())
    else {
        return refuse("Candidate KM0 image header is truncated");
    };
    require(
        (magic1, magic2, base) == (0x3539_3138, 0x3131_3738, XIP_ADDRESS),
        "Unexpected candidate KM0 XIP header",
    )?;

    // The table must lie inside the XIP body, which must lie inside the image.
    let table = expected.len() as u64;
    let start = address
        .checked_sub(base)
        .filter(|offset| offset + table <= size && 64 + size <= candidate.len() as u64)
        .and_then(|offset| usize::try_from(64 + offset).ok());
    let Some(start) = start else {
        return refuse("Linked pad table is outside the KM0 XIP body");
    };
    let actual = &candidate[start..start + expected.len()];
    require(
        actual == expected,
        "Linked KM0 pad table differs from verified stock",
    )?;

    Ok(PinmapAudit {
        runtime_address: Hex(address),
        rows: TABLE_ROWS,
        bytes: actual.len(),
        sha256: sha256(actual),
        matches_stock: true,
        scope: "Pad pull table only; reset levels, GPIO direction and physical behavior unverified",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Repo;
    use std::fs;

    /// The ignored stock image and pinned SDK source, when present.
    fn evidence() -> (Vec<u8>, String) {
        let repo = Repo::here().unwrap();
        let source = repo
            .source()
            .join("component/soc/realtek/amebad/fwlib/usrcfg/rtl8721dlp_pinmapcfg.c");
        (
            fs::read(repo.stock()).expect("ignored verified stock OTA"),
            fs::read_to_string(source).expect("pinned SDK checkout"),
        )
    }

    #[test]
    #[ignore = "needs the ignored stock OTA and pinned SDK checkout"]
    fn linked_stock_table_and_record_fields() {
        let (stock, _) = evidence();
        let report = audit_linked_table(&stock, TABLE_ADDRESS, &stock).unwrap();
        assert_eq!(report.rows, 65);
        assert_eq!(report.bytes, 1300);
        assert!(report.matches_stock);
        let (_, rows) = stock_table(&stock).unwrap();
        assert_eq!(rows[13], [13, 0, 255, 255, 1]);
        assert_eq!(rows[33], [33, 0, 255, 255, 0]);
    }

    #[test]
    #[ignore = "needs the ignored stock OTA and pinned SDK checkout"]
    fn overlay_changes_only_eight_active_fields() {
        let (stock, source) = evidence();
        let updated = overlay(&source, &stock).unwrap();
        assert_eq!(source.lines().count(), updated.lines().count());

        let changed: Vec<_> = source
            .lines()
            .zip(updated.lines())
            .filter(|(a, b)| a != b)
            .collect();
        assert_eq!(changed.len(), 8);
        for (before, after) in changed {
            let pull = if before.contains("GPIO_PuPd_DOWN") {
                "GPIO_PuPd_DOWN"
            } else {
                "GPIO_PuPd_UP"
            };
            assert_eq!(after, before.replacen(pull, "GPIO_PuPd_NOPULL", 1));
        }
    }

    #[test]
    #[ignore = "needs the ignored stock OTA and pinned SDK checkout"]
    fn unreviewed_changes_and_damaged_references_are_rejected() {
        let (stock, source) = evidence();

        let sleep = source
            .lines()
            .find(|l| l.trim_start().starts_with("{_PA_13,"))
            .unwrap();
        let damaged = source.replacen(
            sleep,
            &sleep.replacen("GPIO_PuPd_KEEP", "GPIO_PuPd_DOWN", 1),
            1,
        );
        assert!(
            overlay(&damaged, &stock)
                .unwrap_err()
                .to_string()
                .contains("sleep/low-power")
        );
        for replacement in ["_PA_14,", "_PA_99,"] {
            let error = overlay(&source.replacen("_PA_13,", replacement, 1), &stock).unwrap_err();
            assert!(error.to_string().contains("pin order"));
        }

        let mut mutated = stock.clone();
        *mutated.last_mut().unwrap() ^= 1;
        assert!(
            overlay(&source, &mutated)
                .unwrap_err()
                .to_string()
                .contains("exact verified stock")
        );
        assert!(audit_linked_table(&stock, TABLE_ADDRESS, &mutated).is_err());

        // A valid container checksum must not hide stale or incorrect pad data.
        let mut stale = stock.clone();
        let active_pull = 64 + (TABLE_ADDRESS - XIP_ADDRESS) as usize + 13 * 20 + 4;
        stale[active_pull..active_pull + 4].copy_from_slice(&1u32.to_le_bytes());
        let sum = stale[32..]
            .iter()
            .fold(0u32, |s, b| s.wrapping_add(*b as u32));
        stale[16..20].copy_from_slice(&sum.to_le_bytes());
        let error = audit_linked_table(&stale, TABLE_ADDRESS, &stock).unwrap_err();
        assert!(error.to_string().contains("differs from verified stock"));

        let error = audit_linked_table(&stock, XIP_ADDRESS - 4, &stock).unwrap_err();
        assert!(error.to_string().contains("outside the KM0 XIP body"));
        let mut moved = stock.clone();
        moved[44..48].copy_from_slice(&0x0E00_0020u32.to_le_bytes());
        let error = audit_linked_table(&moved, TABLE_ADDRESS, &stock).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Unexpected candidate KM0 XIP header")
        );
    }
}
