//! The server's liveness and readiness.

use reqwest::{Method, StatusCode};
use wellfed_api::{Health, ServerStatus};

use crate::{Client, Error, decode};

impl Client {
    /// Liveness and the server's version; needs no login.
    pub async fn health(&self) -> Result<Health, Error> {
        let response = self.request(Method::GET, "/health").send().await?;
        decode(response, StatusCode::OK).await
    }

    /// Readiness: the database, each feeder's link and the background
    /// jobs' last runs. Owners only.
    pub async fn status(&self) -> Result<ServerStatus, Error> {
        let response = self.api(Method::GET, "/status").send().await?;
        decode(response, StatusCode::OK).await
    }
}
