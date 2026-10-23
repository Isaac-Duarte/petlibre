//! Reviewed flash plans: what `wellfed-flash update|give-way prepare`
//! writes and `execute` (and the bench's summaries) read back.
//!
//! A reviewed plan is the session bindings, the inspection it came from and
//! the operation, written as canonical JSON; its SHA-256 is what the operator
//! approves. Every field here is therefore part of a hashed document: a
//! field written as `null` stays a plain `Option`, and only the fields the
//! client adds conditionally are skipped when absent.

use serde::{Deserialize, Serialize};

/// The transaction a plan or binding authorizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    #[serde(rename = "plaf103_bank1_development_update")]
    Bank1Update,
    #[serde(rename = "plaf103_bank0_development_update")]
    Bank0Update,
    #[serde(rename = "plaf103_bank1_give_way")]
    GiveWay,
    #[serde(rename = "original_stock_bank1_recovery")]
    StockRecovery,
}

/// The executables a plan was prepared by, by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolHashes {
    #[serde(rename = "wellfed-flash")]
    pub wellfed_flash: String,
}

/// Bindings plus the inspection and operation they authorize.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reviewed<B, P> {
    #[serde(flatten)]
    pub bindings: B,
    pub inspection_report_sha256: String,
    pub operation_plan: P,
}

/// What a development update's inspection and plan are bound to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateBindings {
    pub schema_version: u32,
    pub kind: Kind,
    pub port: String,
    pub interrupted: bool,
    /// Bank 0 without a source review: the full read pinning its contents.
    pub original_read_sha256: Option<String>,
    pub restore_marker: bool,
    pub source_review_sha256: Option<String>,
    pub target_review_sha256: String,
    pub baseline_sha256: String,
    pub rom_report_sha256: String,
    pub bounded_read_report_sha256: String,
    pub tool_sha256: ToolHashes,
}

/// What a give-way's inspection and plan are bound to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GiveWayBindings {
    pub schema_version: u32,
    pub kind: Kind,
    pub port: String,
    pub fallback_review_sha256: String,
    pub baseline_sha256: String,
    pub rom_report_sha256: String,
    pub bounded_read_report_sha256: String,
    pub tool_sha256: ToolHashes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Region {
    pub offset: u64,
    pub bytes: u64,
}

/// One sector to erase and stage, in write order (the marker sector first).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedSector {
    pub offset: u64,
    pub bytes: u64,
    pub before_sha256: String,
    pub staged_sha256: String,
}

/// A region-scoped development update of one bank.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevelopmentPlan {
    pub schema_version: u32,
    pub kind: Kind,
    pub bank: u8,
    pub interrupted: bool,
    pub source_ota_sha256: Option<String>,
    pub target_ota_sha256: String,
    pub source_payload_bytes: u64,
    pub target_payload_bytes: u64,
    pub region: Region,
    pub current_region_sha256: String,
    pub staged_region_sha256: String,
    pub proposed_region_sha256: String,
    pub ordered_sectors: Vec<PlannedSector>,
    pub erase_sectors: u64,
    pub staging_packets: u64,
    pub commit_packets: u64,
    pub additional_commit_erases: u64,
    pub retained_tail_sha256: String,
    pub scope: String,
    /// The inspected other-bank guard sector, once an inspection pins it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub other_bank_sha256: Option<String>,
    /// Present (and true) only for a bank-1 marker restore.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub restore_marker: bool,
}

/// Clearing bank 1's first marker byte so the loader falls back to bank 0.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GiveWayPlan {
    pub schema_version: u32,
    pub kind: Kind,
    pub bank: u8,
    pub offset: u64,
    pub fallback_ota_sha256: String,
    pub bank0_region_sha256: String,
    pub before_sha256: String,
    pub after_sha256: String,
    pub program_frame_sha256: String,
    pub scope: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::canonical_of;

    #[test]
    fn conditional_fields_are_absent_and_nulls_stay() {
        let plan = DevelopmentPlan {
            schema_version: 1,
            kind: Kind::Bank0Update,
            bank: 0,
            interrupted: false,
            source_ota_sha256: None,
            target_ota_sha256: "t".into(),
            source_payload_bytes: 0,
            target_payload_bytes: 4096,
            region: Region {
                offset: 0x6000,
                bytes: 4096,
            },
            current_region_sha256: "c".into(),
            staged_region_sha256: "s".into(),
            proposed_region_sha256: "p".into(),
            ordered_sectors: vec![],
            erase_sectors: 0,
            staging_packets: 0,
            commit_packets: 0,
            additional_commit_erases: 0,
            retained_tail_sha256: "r".into(),
            scope: "x".into(),
            other_bank_sha256: None,
            restore_marker: false,
        };

        let text = String::from_utf8(canonical_of(&plan).unwrap()).unwrap();
        assert!(text.contains("\"kind\":\"plaf103_bank0_development_update\""));
        assert!(text.contains("\"source_ota_sha256\":null"));
        assert!(!text.contains("other_bank_sha256"));
        assert!(!text.contains("restore_marker"));

        let back: DevelopmentPlan = serde_json::from_str(&text).unwrap();
        assert_eq!(back, plan);
    }
}
