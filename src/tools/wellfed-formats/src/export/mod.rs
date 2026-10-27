//! The owner's server archive: what `GET /export` writes and `POST /import`
//! reads back onto a fresh server.
//!
//! An archive is a POSIX tar. Its first entry is `manifest.json`, the
//! [`Manifest`]: the format version, the server version that wrote it, and
//! per table its file's row count, size and SHA-256. A file per table
//! follows in [`Table::ALL`]'s order, which is the order an import inserts
//! them in, so every foreign key names a row already there. A JSON archive
//! holds `<table>.jsonl`, each row one line of canonical JSON
//! ([`crate::canonical`]), so the manifest's hashes are over canonical bytes
//! and a quiet server's database exports the same bytes each time. A CSV archive holds
//! `<table>.csv` with a header row, for a spreadsheet; only a JSON archive
//! imports.
//!
//! Rows are the database's own columns, in its units (Unix milliseconds,
//! the stored spelling of each closed set), with ids as UUIDs and other
//! bytes as lowercase hex. An archive carries no secrets, so a stolen one
//! opens nothing:
//!
//! - **users** without their password hashes: an imported account has no
//!   password until an owner gives it one (`wellfed-server reset-owner`
//!   for an owner, then a temporary password for each other account).
//! - **feeders** without the server's sealed key, so an imported feeder is
//!   archived and must be adopted again, which continues its history.
//! - **sessions** and **tokens** are not exported at all: each is a
//!   credential, or the hash of one.
//! - **job runs** are not exported: they are this server's schedule, and an
//!   imported server runs each job when it is next due.
//!
//! Everything else is: the household, each feeder's cached state, raw
//! events, gaps, samples and their rollups, the projections, the command
//! journal and the audit log, installs, pets, foods and refills.

mod csv;
mod reader;
mod rows;
mod tar;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub use reader::{Item, Reader};
pub use rows::*;
pub use tar::{BLOCK, END, header, padding};

/// The archive layout this build writes and reads. A change to a row's
/// columns or to the layout is a new version.
pub const FORMAT_VERSION: u32 = 1;

/// The manifest's path in the archive.
pub const MANIFEST: &str = "manifest.json";

/// The longest line an import reads: far past any row this server writes,
/// and a bound on what one row may hold in memory.
pub const MAX_LINE: usize = 1 << 20;

/// The longest manifest an import reads.
pub const MAX_MANIFEST: u64 = 64 * 1024;

/// How the tables are written.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Encoding {
    /// One canonical JSON object per line: the form an import reads.
    #[default]
    Json,
    /// A header row, then one row per line, for a spreadsheet.
    Csv,
}

impl Encoding {
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Json => "jsonl",
            Self::Csv => "csv",
        }
    }
}

/// The archive's first entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// [`FORMAT_VERSION`] when written.
    pub format: u32,
    pub encoding: Encoding,
    /// The version of the server that wrote it.
    pub server: String,
    /// Every table, in [`Table::ALL`]'s order.
    pub tables: Vec<TableFile>,
}

impl Manifest {
    /// The manifest's bytes in the archive: canonical JSON and a newline.
    pub fn to_bytes(&self) -> crate::Result<Vec<u8>> {
        let mut bytes = crate::canonical::canonical_of(self)?;
        bytes.push(b'\n');
        Ok(bytes)
    }
}

/// One table's file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableFile {
    pub table: Table,
    pub rows: u64,
    /// The file's size.
    pub bytes: u64,
    /// The SHA-256 of the file's bytes, lowercase hex.
    pub sha256: String,
}

/// A table's file as it is written: its rows, size and running hash.
#[derive(Clone, Debug)]
pub struct TableSum {
    table: Table,
    rows: u64,
    bytes: u64,
    hash: Sha256,
}

impl TableSum {
    pub fn new(table: Table) -> Self {
        Self {
            table,
            rows: 0,
            bytes: 0,
            hash: Sha256::new(),
        }
    }

    /// Count bytes of the file that are not a row (the CSV header).
    pub fn add_bytes(&mut self, bytes: &[u8]) {
        self.bytes += bytes.len() as u64;
        self.hash.update(bytes);
    }

    /// Count one row's bytes.
    pub fn add_row(&mut self, bytes: &[u8]) {
        self.rows += 1;
        self.add_bytes(bytes);
    }

    pub fn finish(self) -> TableFile {
        TableFile {
            table: self.table,
            rows: self.rows,
            bytes: self.bytes,
            sha256: crate::hex::encode(&self.hash.finalize()),
        }
    }
}

/// The path of `table`'s file in an archive written as `encoding`.
pub fn file_name(table: Table, encoding: Encoding) -> String {
    format!("{}.{}", table.name(), encoding.extension())
}

/// What a table's file starts with: the CSV header row, or nothing.
pub fn file_header(table: Table, encoding: Encoding) -> Vec<u8> {
    match encoding {
        Encoding::Json => Vec::new(),
        Encoding::Csv => csv::header(table.columns()),
    }
}

impl Row {
    /// Append the row's line in `encoding` to `out`.
    pub fn write(&self, encoding: Encoding, out: &mut Vec<u8>) -> Result<(), ArchiveError> {
        let value = self.value()?;
        match encoding {
            Encoding::Json => {
                let line = crate::canonical::canonical(&value)
                    .map_err(|error| ArchiveError::Encode(self.table(), error))?;
                out.extend_from_slice(&line);
                out.push(b'\n');
            }
            Encoding::Csv => csv::record(self.table().columns(), &value, out),
        }
        Ok(())
    }
}

/// Why an archive could not be written or read.
#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("encode a {0:?} row: {1}")]
    Encode(Table, crate::Error),
    #[error("not a tar archive this server wrote: {0}")]
    Tar(&'static str),
    #[error("the archive ends before its last table")]
    Truncated,
    #[error("the archive's first entry is {0:?}, not {MANIFEST}")]
    NoManifest(String),
    #[error("the manifest does not read: {0}")]
    Manifest(serde_json::Error),
    #[error("archive format {found}; this server reads format {FORMAT_VERSION}")]
    Version { found: u32 },
    #[error("a CSV archive is for spreadsheets; import the JSON one")]
    Csv,
    #[error("the manifest does not list this format's tables in order")]
    Tables,
    #[error("expected {expected:?} next, found {found:?}")]
    Entry { expected: String, found: String },
    #[error("{table:?} is {found} bytes; the manifest says {manifest}")]
    Size {
        table: Table,
        manifest: u64,
        found: u64,
    },
    #[error("a line of {0:?} is longer than {MAX_LINE} bytes")]
    LineTooLong(Table),
    #[error("a row of {0:?} does not read: {1}")]
    Row(Table, serde_json::Error),
    #[error("{table:?} holds {found} rows; the manifest says {manifest}")]
    Rows {
        table: Table,
        manifest: u64,
        found: u64,
    },
    #[error("{0:?} does not match the manifest's SHA-256")]
    Hash(Table),
    #[error("the upload runs past the archive's length its manifest gives")]
    TooLong,
}

#[cfg(test)]
mod tests;
