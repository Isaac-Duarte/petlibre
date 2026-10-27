//! Feeds and their cancels.

use wellfed_api::{Command, FeedRequest, FeederId};
use reqwest::{Method, StatusCode};
use uuid::Uuid;

use crate::feeders::IDEMPOTENCY_KEY;
use crate::{Client, Error, decode, expect_status};

impl Client {
    /// Feed `portions` now, under `key`: the journal entry once the feeder
    /// has reserved the feed, with its number. The result follows on the
    /// stream and at [`Client::command`]. An `outcome_unknown` answer is
    /// never resent under another key: a retry carries the same `key`, and
    /// the entry is read until the feeder's history settles it.
    pub async fn feed(&self, id: FeederId, portions: u8, key: Uuid) -> Result<Command, Error> {
        let response = self
            .api(Method::POST, &format!("/feeders/{id}/feed"))
            .header(IDEMPOTENCY_KEY, key.to_string())
            .json(&FeedRequest { portions })
            .send()
            .await?;
        decode(response, StatusCode::ACCEPTED).await
    }

    /// Stop the feed numbered `feed` if it still runs. A retry carries the
    /// same `key`.
    pub async fn cancel_feed(&self, id: FeederId, feed: u32, key: Uuid) -> Result<(), Error> {
        let response = self
            .api(Method::POST, &format!("/feeders/{id}/feeds/{feed}/cancel"))
            .header(IDEMPOTENCY_KEY, key.to_string())
            .send()
            .await?;
        expect_status(response, StatusCode::NO_CONTENT).await?;
        Ok(())
    }
}
