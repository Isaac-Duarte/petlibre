//! Import: an export restored onto a fresh server, in one transaction, read
//! as it is uploaded.
//!
//! Import is a fresh server's other way in besides setup, so it has setup's
//! rule rather than a login: it runs only while no account exists, and an
//! account appearing meanwhile (a setup racing it) refuses it at the
//! transaction's start. It logs no one in. The archive holds no password
//! hashes, so every imported account starts with none: no password opens
//! it until the host's `wellfed-server reset-owner` gives an owner a
//! temporary one, and that owner gives each other account its own. The
//! archive holds no feeder keys either, so each imported feeder is
//! archived until it is adopted again, which continues its history.
//!
//! The rows are checked against the manifest as they arrive, and nothing
//! is kept unless every table matches: a damaged, cut or foreign archive
//! leaves the server as fresh as it was.

use futures_util::{Stream, StreamExt};
use tracing::info;
use wellfed_api::{Imported, ImportedTable};
use wellfed_formats::export::{Item, Reader};

use crate::operations::export::ExportError;
use crate::{Result, Service};

/// What an imported account's password hash is: none, which no password
/// matches.
const NO_PASSWORD: &str = "";

impl Service {
    /// Restore the archive `upload` carries onto this fresh server.
    pub async fn import<S, B, E>(&self, mut upload: S) -> Result<Imported>
    where
        S: Stream<Item = std::result::Result<B, E>> + Unpin,
        B: AsRef<[u8]>,
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        if !self.setup_open().await? {
            return Err(ExportError::ImportClosed.into());
        }
        let Some(mut import) = self.shared.store.begin_import(NO_PASSWORD).await? else {
            return Err(ExportError::ImportClosed.into());
        };

        let mut reader = Reader::new();
        let mut tables = Vec::new();
        loop {
            // The import holds the one writer: an upload gone quiet gives it
            // back rather than holding it.
            let idle = self.shared.config.timing.upload_idle;
            let next = tokio::time::timeout(idle, upload.next()).await;
            let next = next.map_err(|idle| ExportError::Upload(Box::new(idle)))?;
            let Some(chunk) = next else {
                break;
            };
            let chunk = chunk.map_err(|error| ExportError::Upload(error.into()))?;
            reader.push(chunk.as_ref()).map_err(ExportError::from)?;
            while let Some(item) = reader.next_item().map_err(ExportError::from)? {
                match item {
                    Item::Manifest(_) | Item::End => {}
                    Item::Row(row) => {
                        let inserted = import.insert(&row).await;
                        inserted.map_err(|source| ExportError::Rejected {
                            table: row.table().name(),
                            source,
                        })?;
                    }
                    Item::Table(file) => tables.push(ImportedTable {
                        table: file.table.name().to_owned(),
                        rows: file.rows,
                    }),
                }
            }
        }
        reader.finish().map_err(ExportError::from)?;
        let archived = import.commit(self.shared.clock.now()).await?;

        info!("imported an archive: {archived} feeders to adopt again");
        self.restored().await?;
        Ok(Imported { tables })
    }

    /// Bring what the service holds in memory in line with the imported
    /// database, as a start would.
    async fn restored(&self) -> Result<()> {
        self.load_zone().await?;
        self.mirror.refresh_projections().await?;
        self.mirror.refresh_samples().await?;
        self.interrupt_commands().await?;
        self.interrupt_installs().await
    }
}
