//! The command journal: the writes sent through the server, by key or as
//! pages.

use reqwest::{Method, StatusCode};
use uuid::Uuid;
use wellfed_api::{Command, CommandPage, CommandQuery};

use crate::{Client, Error, decode};

impl Client {
    /// The write sent under `key`: the caller's own, or anyone's for an
    /// owner. A write whose outcome was unknown is read here rather than
    /// sent again under a new key.
    pub async fn command(&self, key: Uuid) -> Result<Command, Error> {
        let response = self
            .api(Method::GET, &format!("/commands/{key}"))
            .send()
            .await?;
        decode(response, StatusCode::OK).await
    }

    /// A page of the journal, oldest first. The next page passes the
    /// page's `next` as `after`.
    pub async fn commands(&self, query: &CommandQuery) -> Result<CommandPage, Error> {
        let response = self
            .api(Method::GET, "/commands")
            .query(query)
            .send()
            .await?;
        decode(response, StatusCode::OK).await
    }
}
