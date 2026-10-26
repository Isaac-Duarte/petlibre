//! A frozen image review's `review.json`, and the factory-loader runs it records.
//!
//! `wellfed-build review` writes it beside the exact `image.bin` (and, for a
//! build, the `build-manifest.json`) it describes; `wellfed-flash` reads it
//! before any plan binds that image. Plans bind the SHA-256 of the exact
//! file bytes, so the writer emits [`crate::canonical`] form. Reviews frozen
//! by the former Python reviewer carry the same fields and still read.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::audit::{LayoutAudit, Main, PinmapAudit};

pub const SCHEMA_VERSION: u32 = 1;
/// What a `review.json` is. One kind today; any other reads as
/// [`ReviewKind::Unsupported`], which the reader refuses by name rather than
/// failing to parse the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewKind {
    #[serde(rename = "plaf103_development_image_review")]
    Plaf103DevelopmentImage,
    /// Never written.
    #[serde(other, skip_serializing)]
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageReview {
    pub schema_version: u32,
    pub kind: ReviewKind,
    pub image_kind: ImageKind,
    pub ota_sha256: String,
    pub payload_sha256: String,
    pub payload_bytes: u64,
    /// `None` only for the original stock image, which has no build.
    pub manifest_sha256: Option<String>,
    pub review: Details,
    /// The reviewer that froze it: the `wellfed-build` binary (formerly the
    /// Python script), so a rebuilt reviewer is distinguishable in evidence.
    pub reviewer_sha256: String,
    /// A review covers packaging, linked code and the modeled loader only.
    pub application_boot_verified: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageKind {
    /// The pinned original stock OTA.
    Stock,
    /// A first-boot build.
    Build,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Details {
    Build(Box<BuildDetails>),
    Stock { stock_sha256: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BuildDetails {
    /// The build's first-boot mains audit `mains`, compared whole with the
    /// manifest's.
    pub startup_mains: BTreeMap<String, Main>,
    /// The OTA layout audit; readers check `sha256`.
    pub layout: LayoutAudit,
    /// The KM0 pad-table audit, compared whole with the manifest's.
    pub pinmap: PinmapAudit,
    /// One case per bank, bank 0 first.
    pub factory_mapping: Vec<MappingCase>,
    pub scope: String,
}

/// The actual factory loader run on a baseline whose only valid bank holds
/// the reviewed payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MappingCase {
    pub bank: u8,
    /// The loader's bank check for this bank.
    pub bank_check: FactoryRun,
    /// The loader's ordinary bank selection.
    pub selection: FactoryRun,
}

/// What one run of the factory loader's own instructions did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FactoryRun {
    pub stop: Stop,
    /// R0 at the stop: 1 for a valid bank check; the selected bank plus one.
    pub result: u32,
    /// Each RSIP mapping request, in order.
    pub configs: Vec<MappingConfig>,
    /// The `(index, enable)` arguments of each mapping enable call.
    pub commands: Vec<[u32; 2]>,
    /// The two RSIP mapping register groups after the run.
    pub final_registers: [[u32; 4]; 2],
}

/// The reviewed boundaries a factory-loader run may stop at. The names
/// serialize as the addresses the Python reviewer wrote (`hex()` form).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stop {
    /// Returned from the bank check to the modeled caller.
    #[serde(rename = "0x8f000")]
    Returned,
    /// The selector chose a bank and continues to boot it.
    #[serde(rename = "0x80004a0")]
    Selected,
    /// Neither bank is valid: the loader's no-image wait loop.
    #[serde(rename = "0x8000482")]
    NoImage,
    /// A bank's KM4 signature mismatched: the warning loop.
    #[serde(rename = "0x8000182")]
    Warning,
}

/// One mapping request to the ROM RSIP routine and the registers it set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MappingConfig {
    pub index: u32,
    pub start: u32,
    pub end: u32,
    /// Nonzero: virtual = physical + offset is subtracted, not added.
    pub minus: u32,
    pub offset: u32,
    pub registers: [u32; 4],
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reviews are hashed by their bytes, so the kind writes exactly the
    /// word the Python reviewer wrote; any other reads as unsupported.
    #[test]
    fn the_review_kind_keeps_its_word() {
        assert_eq!(
            serde_json::to_value(ReviewKind::Plaf103DevelopmentImage).unwrap(),
            "plaf103_development_image_review"
        );
        let other: ReviewKind = serde_json::from_value("plaf104_review".into()).unwrap();
        assert_eq!(other, ReviewKind::Unsupported);
    }
}
