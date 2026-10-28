//! The server's archive: an owner's export, and an import onto a fresh
//! server.

use futures_util::{Stream, TryStreamExt};
use reqwest::{Body, Method, StatusCode};
use wellfed_api::{ExportFormat, ExportQuery, Imported};

use crate::{Client, Error, decode, expect_status};

impl Client {
    /// The archive, as its bytes arrive. A stream that ends in an error
    /// was cut: the archive is incomplete.
    pub async fn export(
        &self,
        format: ExportFormat,
    ) -> Result<impl Stream<Item = Result<Vec<u8>, Error>> + use<>, Error> {
        let response = self
            .api(Method::GET, "/export")
            .query(&ExportQuery { format })
            .send()
            .await?;
        let response = expect_status(response, StatusCode::OK).await?;
        Ok(response
            .bytes_stream()
            .map_ok(Vec::from)
            .map_err(Error::from))
    }

    /// Restore an archive onto this fresh server. `archive` may be bytes
    /// or a stream ([`Body::wrap_stream`]), so a large one is never held.
    pub async fn import(&self, archive: impl Into<Body>) -> Result<Imported, Error> {
        let response = self
            .api(Method::POST, "/import")
            .header(reqwest::header::CONTENT_TYPE, "application/x-tar")
            .body(archive)
            .send()
            .await?;
        decode(response, StatusCode::CREATED).await
    }
}
