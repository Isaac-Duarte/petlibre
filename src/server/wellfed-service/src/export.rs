//! Export: everything the server holds as one archive
//! ([`wellfed_formats::export`]), streamed from one snapshot of the
//! database without holding it in memory.
//!
//! A tar needs each file's size before its bytes, and the manifest goes
//! first, so the snapshot is read twice: once to count, size and hash each
//! table, then again to write it. Both passes read one read transaction,
//! so they see the same rows, and the second checks that it wrote what the
//! first counted. The second pass runs in a task that hands the caller
//! chunks of at most [`CHUNK`] bytes through a queue of [`QUEUE`], so at
//! most those and one row are held however large the database is; a caller
//! that stops reading stops the task.

use std::sync::Arc;

use futures_util::{Stream, TryStreamExt};
use wellfed_api::ExportFormat;
use wellfed_formats::export::{
    ArchiveError, END, Encoding, FORMAT_VERSION, MANIFEST, Manifest, Row, Table, TableFile,
    TableSum, file_header, file_name, header, padding,
};
use wellfed_store::Snapshot;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

use crate::{Result, Service};

/// The most bytes one chunk of the archive holds.
pub const CHUNK: usize = 64 * 1024;

/// How many chunks wait for the caller at most.
const QUEUE: usize = 4;

/// Export and import refusals.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("import runs only on a fresh server: an account exists")]
    ImportClosed,
    #[error(transparent)]
    Archive(#[from] ArchiveError),
    #[error("a row of {table} does not fit the database")]
    Rejected {
        table: &'static str,
        #[source]
        source: wellfed_store::Error,
    },
    #[error("the upload stopped: {0}")]
    Upload(Box<dyn std::error::Error + Send + Sync>),
    #[error("encode the manifest: {0}")]
    Manifest(wellfed_formats::Error),
    #[error("{0} changed between the passes over one snapshot")]
    Changed(&'static str),
    #[error("another export is being written: one runs at a time")]
    Running,
}

impl Service {
    /// The archive, in `format`, as a stream of chunks. `version` is the
    /// server's, for the manifest. The tables are counted before this
    /// returns; their bytes follow as the caller reads.
    ///
    /// One export is written at a time: another while it runs is
    /// [`ExportError::Running`]. A caller that stops reading frees it.
    pub async fn export(
        &self,
        format: ExportFormat,
        version: &str,
    ) -> Result<impl Stream<Item = Result<Vec<u8>>> + Send + 'static> {
        let encoding = match format {
            ExportFormat::Json => Encoding::Json,
            ExportFormat::Csv => Encoding::Csv,
        };
        let Ok(slot) = Arc::clone(&self.exporting).try_lock_owned() else {
            return Err(ExportError::Running.into());
        };
        let mut snapshot = self.shared.store.snapshot().await?;

        let mut tables = Vec::with_capacity(Table::ALL.len());
        for &table in Table::ALL {
            tables.push(count_table(&mut snapshot, table, encoding).await?);
        }
        let manifest = Manifest {
            format: FORMAT_VERSION,
            encoding,
            server: version.to_owned(),
            tables,
        };

        let (sender, receiver) = mpsc::channel(QUEUE);
        let mut out = Out {
            sender,
            buffer: Vec::with_capacity(CHUNK),
            cancel: self.cancel.child_token(),
        };
        self.tasks.spawn(async move {
            let _slot = slot;
            let written = write_archive(&mut snapshot, &manifest, &mut out).await;
            match written {
                Ok(()) | Err(Stop::Gone) => {}
                Err(Stop::Failed(error)) => {
                    // The caller sees the stream end in it; the tar is cut.
                    tracing::error!("export: {error}");
                    let _gone = out.sender.send(Err(error)).await;
                }
            }
        });
        Ok(ReceiverStream::new(receiver))
    }
}

/// Why writing stopped.
enum Stop {
    /// The caller stopped reading, or the server is stopping.
    Gone,
    Failed(crate::Error),
}

impl From<crate::Error> for Stop {
    fn from(error: crate::Error) -> Self {
        Self::Failed(error)
    }
}

impl From<ExportError> for Stop {
    fn from(error: ExportError) -> Self {
        Self::Failed(error.into())
    }
}

impl From<ArchiveError> for Stop {
    fn from(error: ArchiveError) -> Self {
        ExportError::from(error).into()
    }
}

impl From<wellfed_store::Error> for Stop {
    fn from(error: wellfed_store::Error) -> Self {
        Self::Failed(error.into())
    }
}

/// The archive's way to the caller, in chunks.
struct Out {
    sender: mpsc::Sender<Result<Vec<u8>>>,
    buffer: Vec<u8>,
    cancel: CancellationToken,
}

impl Out {
    async fn put(&mut self, bytes: &[u8]) -> Result<(), Stop> {
        self.buffer.extend_from_slice(bytes);
        if self.buffer.len() >= CHUNK {
            self.flush().await?;
        }
        Ok(())
    }

    /// Hand the caller what is buffered, in chunks of at most [`CHUNK`].
    async fn flush(&mut self) -> Result<(), Stop> {
        for chunk in self.buffer.chunks(CHUNK) {
            let sent = self
                .cancel
                .run_until_cancelled(self.sender.send(Ok(chunk.to_vec())));
            if !matches!(sent.await, Some(Ok(()))) {
                return Err(Stop::Gone);
            }
        }
        self.buffer.clear();
        Ok(())
    }
}

/// The second pass: the manifest, then each table as the first pass
/// counted it, then the end.
async fn write_archive(
    snapshot: &mut Snapshot,
    manifest: &Manifest,
    out: &mut Out,
) -> Result<(), Stop> {
    let bytes = manifest.to_bytes().map_err(ExportError::Manifest)?;
    let size = bytes.len() as u64;
    out.put(&header(MANIFEST, size)?).await?;
    out.put(&bytes).await?;
    out.put(padding(size)).await?;

    for expected in &manifest.tables {
        let name = file_name(expected.table, manifest.encoding);
        out.put(&header(&name, expected.bytes)?).await?;
        let written = write_table(snapshot, expected.table, manifest.encoding, out).await?;
        if written != *expected {
            return Err(ExportError::Changed(expected.table.name()).into());
        }
        out.put(padding(expected.bytes)).await?;
    }

    out.put(&END).await?;
    out.flush().await
}

/// The first pass over a table: its file's rows, size and hash.
async fn count_table(
    snapshot: &mut Snapshot,
    table: Table,
    encoding: Encoding,
) -> Result<TableFile> {
    let mut sum = TableSum::new(table);
    sum.add_bytes(&file_header(table, encoding));

    let mut rows = snapshot.rows(table);
    let mut line = Vec::new();
    while let Some(row) = rows.try_next().await? {
        encode(&row, encoding, &mut line)?;
        sum.add_row(&line);
    }
    Ok(sum.finish())
}

/// The second pass over a table: its file, written as its rows are read.
async fn write_table(
    snapshot: &mut Snapshot,
    table: Table,
    encoding: Encoding,
    out: &mut Out,
) -> Result<TableFile, Stop> {
    let mut sum = TableSum::new(table);
    let start = file_header(table, encoding);
    sum.add_bytes(&start);
    out.put(&start).await?;

    let mut rows = snapshot.rows(table);
    let mut line = Vec::new();
    while let Some(row) = rows.try_next().await? {
        encode(&row, encoding, &mut line)?;
        sum.add_row(&line);
        out.put(&line).await?;
    }
    Ok(sum.finish())
}

/// `row`'s line, in place of what `line` held.
fn encode(row: &Row, encoding: Encoding, line: &mut Vec<u8>) -> Result<(), ExportError> {
    line.clear();
    row.write(encoding, line)?;
    Ok(())
}
