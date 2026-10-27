//! Reading a JSON archive as it arrives, without holding it: the caller
//! pushes bytes in as they come and takes out the manifest, then each row,
//! then each table's end once its count and hash match the manifest's.
//! At most one line and the input not yet read are held.

use serde::Deserialize;

use super::tar::{self, Header};
use super::{
    ArchiveError, BLOCK, END, Encoding, FORMAT_VERSION, MANIFEST, MAX_LINE, MAX_MANIFEST, Manifest,
    Row, Table, TableFile, TableSum, file_name,
};

/// What the archive yields, in order.
#[derive(Debug)]
pub enum Item {
    /// The manifest, checked for this format and its tables.
    Manifest(Manifest),
    Row(Box<Row>),
    /// A table's file ended and matched the manifest.
    Table(TableFile),
    /// The archive's end.
    End,
}

/// An archive being read.
#[derive(Debug, Default)]
pub struct Reader {
    input: Vec<u8>,
    /// Where the unread input starts.
    at: usize,
    /// Padding still to pass over before the next header.
    skip: u64,
    state: State,
    /// The manifest's tables, once read; `next` indexes them.
    tables: Vec<TableFile>,
    next: usize,
    /// Every byte pushed so far.
    pushed: u64,
    /// The whole archive's length, once the manifest gives it.
    length: Option<u64>,
}

#[derive(Debug, Default)]
enum State {
    #[default]
    ManifestHeader,
    Manifest {
        size: usize,
    },
    TableHeader,
    Table {
        expected: TableFile,
        remaining: u64,
        sum: TableSum,
    },
    Trailer,
    End,
}

impl Reader {
    pub fn new() -> Self {
        Self::default()
    }

    /// More of the archive, refused once it runs past the length the
    /// manifest gives the whole archive, so an upload is never taken past
    /// what it declared.
    pub fn push(&mut self, data: &[u8]) -> Result<(), ArchiveError> {
        self.pushed = self.pushed.saturating_add(data.len() as u64);
        if self.length.is_some_and(|length| self.pushed > length) {
            return Err(ArchiveError::TooLong);
        }

        if self.at > 0 {
            self.input.drain(..self.at);
            self.at = 0;
        }
        self.input.extend_from_slice(data);
        Ok(())
    }

    /// The next item, or `None` until more input is pushed.
    pub fn next_item(&mut self) -> Result<Option<Item>, ArchiveError> {
        loop {
            if self.skip > 0 {
                let available = (self.input.len() - self.at) as u64;
                let passed = self.skip.min(available);
                self.at += usize::try_from(passed).map_err(|_| ArchiveError::Truncated)?;
                self.skip -= passed;
                if self.skip > 0 {
                    return Ok(None);
                }
            }

            match &mut self.state {
                State::ManifestHeader => {
                    let Some(header) = self.header()? else {
                        return Ok(None);
                    };
                    let size = match header {
                        Header::File { name, size } if name == MANIFEST && size <= MAX_MANIFEST => {
                            size
                        }
                        Header::File { name, .. } => return Err(ArchiveError::NoManifest(name)),
                        Header::End => return Err(ArchiveError::Truncated),
                    };
                    let size = usize::try_from(size).map_err(|_| ArchiveError::Truncated)?;
                    self.state = State::Manifest { size };
                }
                State::Manifest { size } => {
                    let size = *size;
                    let Some(bytes) = self.take(size) else {
                        return Ok(None);
                    };
                    let manifest = manifest(bytes)?;
                    self.skip = tar::padding(size as u64).len() as u64;
                    let length = archive_length(size as u64, &manifest.tables)?;
                    if self.pushed > length {
                        return Err(ArchiveError::TooLong);
                    }
                    self.length = Some(length);
                    self.tables.clone_from(&manifest.tables);
                    self.state = State::TableHeader;
                    return Ok(Some(Item::Manifest(manifest)));
                }
                State::TableHeader => {
                    let Some(header) = self.header()? else {
                        return Ok(None);
                    };
                    let Some(expected) = self.tables.get(self.next).cloned() else {
                        return Err(ArchiveError::Tables);
                    };
                    let name = file_name(expected.table, Encoding::Json);
                    let found = match header {
                        Header::File { name: found, size } if found == name => size,
                        Header::File { name: found, .. } => {
                            return Err(ArchiveError::Entry {
                                expected: name,
                                found,
                            });
                        }
                        Header::End => return Err(ArchiveError::Truncated),
                    };
                    if found != expected.bytes {
                        return Err(ArchiveError::Size {
                            table: expected.table,
                            manifest: expected.bytes,
                            found,
                        });
                    }
                    self.state = State::Table {
                        sum: TableSum::new(expected.table),
                        remaining: expected.bytes,
                        expected,
                    };
                }
                State::Table {
                    expected,
                    remaining,
                    sum,
                } => {
                    if *remaining == 0 {
                        let file = std::mem::replace(sum, TableSum::new(expected.table)).finish();
                        check(expected, &file)?;
                        self.skip = tar::padding(expected.bytes).len() as u64;
                        self.next += 1;
                        self.state = if self.next == self.tables.len() {
                            State::Trailer
                        } else {
                            State::TableHeader
                        };
                        return Ok(Some(Item::Table(file)));
                    }

                    let table = expected.table;
                    let unread = &self.input[self.at..];
                    let window = unread
                        .len()
                        .min(usize::try_from(*remaining).unwrap_or(usize::MAX));
                    let Some(end) = unread[..window].iter().position(|&byte| byte == b'\n') else {
                        if window > MAX_LINE {
                            return Err(ArchiveError::LineTooLong(table));
                        }
                        if window as u64 == *remaining {
                            // The file's last bytes are here and end mid-line.
                            return Err(ArchiveError::Tar(
                                "a table's file does not end its last line",
                            ));
                        }
                        return Ok(None);
                    };
                    if end > MAX_LINE {
                        return Err(ArchiveError::LineTooLong(table));
                    }

                    let line = &unread[..=end];
                    sum.add_row(line);
                    *remaining -= line.len() as u64;
                    let row = Row::parse(table, &line[..end])?;
                    self.at += end + 1;
                    return Ok(Some(Item::Row(Box::new(row))));
                }
                State::Trailer => {
                    // Both empty blocks: an archive cut short can end in one.
                    let Some(end) = self.take(END.len()) else {
                        return Ok(None);
                    };
                    if end.iter().any(|&byte| byte != 0) {
                        return Err(ArchiveError::Tables);
                    }
                    self.state = State::End;
                    return Ok(Some(Item::End));
                }
                State::End => return Ok(None),
            }
        }
    }

    /// Whether the whole archive has been read: call once the input ends.
    pub fn finish(&self) -> Result<(), ArchiveError> {
        match self.state {
            State::End => Ok(()),
            _ => Err(ArchiveError::Truncated),
        }
    }

    /// The next header block, once it is all here.
    fn header(&mut self) -> Result<Option<Header>, ArchiveError> {
        let Some(bytes) = self.take(BLOCK) else {
            return Ok(None);
        };
        let block: &[u8; BLOCK] = bytes.try_into().map_err(|_| ArchiveError::Truncated)?;
        tar::parse(block).map(Some)
    }

    /// The next `len` bytes, once they are all here.
    fn take(&mut self, len: usize) -> Option<&[u8]> {
        let start = self.at;
        if self.input.len() - start < len {
            return None;
        }
        self.at += len;
        Some(&self.input[start..start + len])
    }
}

/// The manifest, refused unless it is this format's, as JSON, listing
/// every table in order.
fn manifest(bytes: &[u8]) -> Result<Manifest, ArchiveError> {
    #[derive(Deserialize)]
    struct Version {
        format: u32,
    }

    let version: Version = serde_json::from_slice(bytes).map_err(ArchiveError::Manifest)?;
    if version.format != FORMAT_VERSION {
        return Err(ArchiveError::Version {
            found: version.format,
        });
    }
    let manifest: Manifest = serde_json::from_slice(bytes).map_err(ArchiveError::Manifest)?;
    if manifest.encoding != Encoding::Json {
        return Err(ArchiveError::Csv);
    }
    let tables: Vec<Table> = manifest.tables.iter().map(|file| file.table).collect();
    if tables != Table::ALL {
        return Err(ArchiveError::Tables);
    }
    Ok(manifest)
}

fn check(expected: &TableFile, found: &TableFile) -> Result<(), ArchiveError> {
    if found.rows != expected.rows {
        return Err(ArchiveError::Rows {
            table: expected.table,
            manifest: expected.rows,
            found: found.rows,
        });
    }
    if found.sha256 != expected.sha256 {
        return Err(ArchiveError::Hash(expected.table));
    }
    Ok(())
}

/// The length of an archive whose manifest is `manifest` bytes and lists
/// `tables`: each entry's header and padded data, then the end.
fn archive_length(manifest: u64, tables: &[TableFile]) -> Result<u64, ArchiveError> {
    let entry = |size: u64| {
        let padded = size.checked_add(tar::padding(size).len() as u64)?;
        padded.checked_add(BLOCK as u64)
    };

    let mut length = entry(manifest).ok_or(ArchiveError::TooLong)?;
    for file in tables {
        let file = entry(file.bytes).ok_or(ArchiveError::TooLong)?;
        length = length.checked_add(file).ok_or(ArchiveError::TooLong)?;
    }
    length
        .checked_add(END.len() as u64)
        .ok_or(ArchiveError::TooLong)
}
