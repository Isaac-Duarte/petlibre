//! The audit log: who did what through the server.

use wellfed_api::{AuditPage, PageQuery};
use reqwest::{Method, StatusCode};

use crate::{Client, Error, decode};

impl Client {
    /// A page of the audit log, oldest first: the caller's own entries, or
    /// everyone's for an owner. The next page passes the page's `next` as
    /// `after`.
    pub async fn audit(&self, query: &PageQuery) -> Result<AuditPage, Error> {
        let response = self.api(Method::GET, "/audit").query(query).send().await?;
        decode(response, StatusCode::OK).await
    }
}
