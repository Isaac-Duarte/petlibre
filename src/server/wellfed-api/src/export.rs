//! Exporting everything the server holds as one archive, and importing it
//! onto a fresh server.
//!
//! The archive is a tar: `manifest.json` (the format version, the server's
//! version, and each table's row count, size and SHA-256), then a file per
//! table, `.jsonl` or, with `?format=csv`, `.csv`. It carries no secrets: no
//! password, token or session hashes and no feeder keys, so after an import
//! each account needs a password from an owner (the first owner's from
//! `wellfed-server reset-owner`) and each feeder is archived until it is
//! adopted again. Only the JSON archive imports.

use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

/// How the tables are written.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    /// One JSON object per line: the archive an import reads.
    #[default]
    Json,
    /// A header row and a row per line, for a spreadsheet.
    Csv,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ExportQuery {
    #[serde(default)]
    pub format: ExportFormat,
}

/// What an import restored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Imported {
    /// Every table in the archive, in its order, with the rows restored.
    pub tables: Vec<ImportedTable>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ImportedTable {
    /// The table's name in the archive.
    pub table: String,
    pub rows: u64,
}
