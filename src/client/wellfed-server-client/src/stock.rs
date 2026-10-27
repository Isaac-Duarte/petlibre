//! The feeder's stock firmware as the server keeps it: its state, the
//! backup's export, an upload or a read from the feeder, and "Restore
//! stock".

use wellfed_api::{FeederId, Install, StockBackup, StockRestore, StockState};
use reqwest::header::{self, HeaderValue};
use reqwest::{Method, Response, StatusCode};

use crate::{Client, Error, decode, expect_status};

/// What keeping a copy of stock did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kept {
    /// Verified and kept.
    New(StockBackup),
    /// The server kept a verified copy already.
    Known(StockBackup),
}

impl Client {
    /// The feeder's kept stock backup, what the feeder says of its banks
    /// now, and its running or last restore.
    pub async fn stock(&self, feeder: FeederId) -> Result<StockState, Error> {
        let response = self
            .api(Method::GET, &format!("/feeders/{feeder}/stock"))
            .send()
            .await?;
        decode(response, StatusCode::OK).await
    }

    /// The kept backup's bytes, as the server checked them again: the
    /// vendor's stock OTA container. Owners only.
    pub async fn stock_backup(&self, feeder: FeederId) -> Result<Vec<u8>, Error> {
        let response = self
            .api(Method::GET, &format!("/feeders/{feeder}/stock/backup"))
            .send()
            .await?;
        let response = expect_status(response, StatusCode::OK).await?;
        Ok(response.bytes().await?.to_vec())
    }

    /// Upload a copy of the feeder's stock. A copy the server refuses comes
    /// back as a `stock_refused` problem. Uploading the same copy again is
    /// harmless, so it needs no idempotency key.
    pub async fn upload_stock(&self, feeder: FeederId, file: Vec<u8>) -> Result<Kept, Error> {
        let response = self
            .api(Method::POST, &format!("/feeders/{feeder}/stock/upload"))
            .header(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/octet-stream"),
            )
            .body(file)
            .send()
            .await?;
        kept(response).await
    }

    /// Have the server read bank 1 from the feeder (the replacement on
    /// bank 0, before promotion) and keep it; this waits for the read. A
    /// read changes nothing on the feeder, so it needs no idempotency key.
    pub async fn read_stock(&self, feeder: FeederId) -> Result<Kept, Error> {
        let response = self
            .api(Method::POST, &format!("/feeders/{feeder}/stock/read"))
            .send()
            .await?;
        kept(response).await
    }

    /// Restore stock from the kept backup, as `restore` signs it. It runs
    /// on the server in the background as an install does: this returns
    /// once it has started, and the stream carries its progress and its
    /// end. Never sent again: another restore is another request.
    pub async fn restore_stock(
        &self,
        feeder: FeederId,
        restore: &StockRestore,
    ) -> Result<Install, Error> {
        let response = self
            .api(Method::POST, &format!("/feeders/{feeder}/stock/restore"))
            .json(restore)
            .send()
            .await?;
        decode(response, StatusCode::ACCEPTED).await
    }
}

async fn kept(response: Response) -> Result<Kept, Error> {
    if response.status() == StatusCode::OK {
        return Ok(Kept::Known(decode(response, StatusCode::OK).await?));
    }
    Ok(Kept::New(decode(response, StatusCode::CREATED).await?))
}
