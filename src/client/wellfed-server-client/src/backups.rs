//! Backups of the server's database. Owners only.

use reqwest::{Method, StatusCode};
use wellfed_api::{Backup, BackupPage};

use crate::{Client, Error, decode};

impl Client {
    /// The backups the server keeps, newest first.
    pub async fn backups(&self) -> Result<BackupPage, Error> {
        let response = self.api(Method::GET, "/backups").send().await?;
        decode(response, StatusCode::OK).await
    }

    /// Back the database up now. Taking one twice is harmless, so it
    /// needs no idempotency key.
    pub async fn backup(&self) -> Result<Backup, Error> {
        let response = self.api(Method::POST, "/backups").send().await?;
        decode(response, StatusCode::CREATED).await
    }
}
