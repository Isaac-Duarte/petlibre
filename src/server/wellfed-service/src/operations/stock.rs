//! The feeder's own stock firmware: the verified backup the server keeps
//! for it, and "Restore stock", which installs that backup into bank 1.
//!
//! The backup comes from the feeder itself while the replacement runs from
//! bank 0 with stock still in bank 1 (before promotion overwrites it), or
//! from the owner, who uploads the copy their installer kept. Either is
//! checked against the pinned stock image before it is kept, as a file in
//! the data directory (`wellfed-releases`), never in the database.
//!
//! A restore is an install like a release's, sharing its one slot, its
//! history and its progress on the stream, pushed by the same driver on
//! the feeder's long lane. The server holds no signing key: the owner signs
//! the kept backup as the stock image, with a downgrade statement for the
//! feeder at its floor, and the server sends it only after checking that
//! the manifest offers exactly the copy it keeps. Nothing is offered again
//! on the owner's behalf; once activated, the feeder runs stock, which
//! serves no device API.

use wellfed_api::{
    ClientRole, Connection as Link, FeederId, FeederRefusal, Install, InstallTarget,
    StockBackup as StockView, StockFeeder, StockRestore, StockState,
};
use wellfed_mirror::{BankReadError, ReadError};
use wellfed_model::{Answer, Op, UpdatePhase};
use wellfed_releases::{DOWNGRADE_LEN, MANIFEST_LEN, Taken};
use tracing::{info, warn};

use crate::operations::installs::Offer;
use crate::{
    Caller, CommandError, Error, InstallError, InternalError, Permission, ReleaseAdded, Result,
    Service, can,
};

// The API spells the restore's parts at the lengths the feeder reads.
const _: () = assert!(wellfed_api::MANIFEST_LEN == MANIFEST_LEN);
const _: () = assert!(wellfed_api::STATEMENT_LEN == DOWNGRADE_LEN);

impl Service {
    /// The feeder's kept backup, what it says of its banks now, and its
    /// running or last restore.
    ///
    /// The feeder is asked only for an owner, who restores and reads it, so
    /// a page that members and viewers poll does not reach the feeder.
    pub async fn stock(&self, caller: &Caller, id: FeederId) -> Result<StockState> {
        let row = self.row(id).await?;
        let backup = self.releases.stock().view(row.identity).await?;
        let asks = can(caller.user.role, Permission::ServerData);
        let feeder_state = if asks && row.role == ClientRole::Owner {
            self.stock_feeder(id).await
        } else {
            None
        };
        let restore = match self.running_on(id) {
            Some(running) if matches!(running.target, InstallTarget::Stock { .. }) => Some(running),
            _ => {
                let last = self.shared.store.latest_restore(id).await?;
                last.map(|last| self.unsaved(last))
            }
        };

        Ok(StockState {
            feeder: id,
            backup,
            feeder_state,
            restore,
        })
    }

    /// The kept backup's file, checked again: what an owner exports.
    pub async fn stock_file(&self, id: FeederId) -> Result<(StockView, Vec<u8>)> {
        let row = self.row(id).await?;
        let (view, backup) = self.releases.stock().backup(row.identity).await?;
        Ok((view, backup.bytes().to_vec()))
    }

    /// Check an uploaded copy of the feeder's stock and keep it.
    pub async fn upload_stock(
        &self,
        id: FeederId,
        file: Vec<u8>,
    ) -> Result<(StockView, ReleaseAdded)> {
        let row = self.row(id).await?;
        let now = self.shared.clock.now();
        Ok(self
            .releases
            .stock()
            .keep(row.identity, Taken::Upload(file), now)
            .await?)
    }

    /// Read bank 1 from the feeder's bank-0 role, check it and keep it,
    /// unless a verified copy is kept already. Only an owner's key reads
    /// it, and only the replacement running from bank 0 serves it, so both
    /// are refused here first, as the feeder would.
    pub async fn read_stock(&self, id: FeederId) -> Result<(StockView, ReleaseAdded)> {
        let row = self.row(id).await?;
        if let Ok((kept, _)) = self.releases.stock().backup(row.identity).await {
            return Ok((kept, ReleaseAdded::Known));
        }
        if row.role != ClientRole::Owner {
            return Err(Error::Refused(FeederRefusal::Forbidden));
        }
        if self.mirror.link(id) != Some(Link::Online) {
            return Err(CommandError::Offline.into());
        }
        // A feeder that predates the op would end the session on it.
        if !self.mirror.capabilities(id).is_some_and(|c| c.backup) {
            return Err(Error::Refused(FeederRefusal::Unavailable));
        }
        // The feeder answers `busy` while an install holds its flash.
        if self.running_on(id).is_some() {
            return Err(InstallError::Running.into());
        }

        info!(feeder = %id, "reading the stock backup from bank 1");
        let mut operation = self.mirror.operation(id).await?;
        // Another read may have kept it while this one waited its turn.
        if let Ok((kept, _)) = self.releases.stock().backup(row.identity).await {
            return Ok((kept, ReleaseAdded::Known));
        }
        let bank = operation.read_bank1(|_| {}).await.map_err(read_failed)?;
        drop(operation);

        let now = self.shared.clock.now();
        let kept = self
            .releases
            .stock()
            .keep(row.identity, Taken::Bank(bank), now)
            .await?;
        info!(feeder = %id, "kept the verified stock backup");
        Ok(kept)
    }

    /// Restore the feeder's stock from the kept backup, signed by the owner
    /// in `request`, and return the restore as it starts. It runs as an
    /// install does: one at a time per server, on the long lane, with its
    /// progress and its end on the stream.
    ///
    /// Nothing is sent unless the backup still verifies and the manifest
    /// offers exactly it as the stock image; the feeder then checks the
    /// signature and the statement itself.
    pub async fn restore_stock(
        &self,
        caller: &Caller,
        id: FeederId,
        request: StockRestore,
    ) -> Result<Install> {
        let row = self.row(id).await?;
        if row.role != ClientRole::Owner {
            return Err(Error::Refused(FeederRefusal::Forbidden));
        }
        let (view, backup) = self.releases.stock().backup(row.identity).await?;
        let offer = backup
            .restore_offer(&request.manifest.0)
            .map_err(InstallError::RestoreMismatch)?;
        if self.mirror.link(id) != Some(Link::Online) {
            return Err(CommandError::Offline.into());
        }
        if !self.mirror.capabilities(id).is_some_and(|c| c.update) {
            return Err(Error::Refused(FeederRefusal::Unavailable));
        }

        let image_len =
            u32::try_from(backup.payload().len()).map_err(|_| InternalError::StockSize)?;
        let install = self.new_install(
            caller,
            id,
            InstallTarget::Stock {
                sha256: view.sha256,
            },
            offer.version,
            image_len,
        )?;
        let offer = Offer::Stock {
            manifest: request.manifest.0.to_vec(),
            downgrade: request.downgrade.0.to_vec(),
            backup,
        };
        self.begin(install, offer).await
    }

    /// Refuse an install that would promote over stock without a kept
    /// backup, as the installer refuses it: the feeder runs from bank 0
    /// with no install under way, so bank 1 may hold stock, its only copy.
    /// An install already writing bank 1, or one that stopped after writing
    /// some of it, has overwritten it already, so it goes on; one that
    /// stopped before writing anything left it as it was.
    ///
    /// The kept copy counts once it verifies against the server's pins,
    /// which every real server fixes to stock's own: there it is a
    /// `VerifiedBackup`, as the installer's promotion requires.
    pub(crate) async fn keeps_stock(&self, id: FeederId, identity: [u8; 32]) -> Result<()> {
        // A feeder that predates the op would end the session on it.
        if !self.mirror.capabilities(id).is_some_and(|c| c.update) {
            return Err(Error::Refused(FeederRefusal::Unavailable));
        }
        let request = self.request_id()?;
        let view = match self.mirror.ask(id, request, Op::UpdateState).await? {
            Answer::Update(view) => view,
            Answer::Refused(refusal) => return Err(Error::Refused(refusal.into())),
            _ => return Err(InternalError::Unexpected.into()),
        };

        let over_stock = view.bank == 0
            && view.phase == UpdatePhase::Idle
            && (view.stopped.is_none() || view.written == 0);
        if over_stock && self.releases.stock().backup(identity).await.is_err() {
            warn!(feeder = %id, "install refused: bank 1 may hold stock, and no backup is kept");
            return Err(InstallError::StockBackupNeeded.into());
        }
        Ok(())
    }

    /// What the feeder says of its banks, read now; `None` when it does
    /// not answer.
    async fn stock_feeder(&self, id: FeederId) -> Option<StockFeeder> {
        if self.mirror.link(id) != Some(Link::Online) {
            return None;
        }
        let capabilities = self.mirror.capabilities(id)?;
        if !capabilities.update {
            return None;
        }
        let request = self.request_id().ok()?;
        match self.mirror.ask(id, request, Op::UpdateState).await {
            Ok(Answer::Update(view)) => Some(StockFeeder {
                bank: view.bank,
                floor: view.min_version,
                readable: view.bank == 0 && capabilities.backup,
            }),
            Ok(other) => {
                warn!(feeder = %id, "the update state was answered {other:?}");
                None
            }
            Err(error) => {
                warn!(feeder = %id, "the update state was not read: {error}");
                None
            }
        }
    }
}

/// A read that stopped, in the service's domains.
fn read_failed(error: BankReadError) -> crate::Error {
    match error {
        BankReadError::Bank(ReadError::Refused(refusal)) => Error::Refused(refusal.into()),
        BankReadError::Bank(other) => {
            warn!("the stock read stopped: {other}");
            InternalError::Unexpected.into()
        }
        BankReadError::Lost(error) => error.into(),
        BankReadError::Random(failed) => failed.into(),
    }
}
