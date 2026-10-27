//! Adopted feeders: the list, adoption, status, settings, the panel lock
//! and the history.

use wellfed_api::{
    AdoptFeeder, EventPage, EventQuery, Feeder, FeederDetail, FeederId, Generation, LockRequest,
    Settings, SettingsPatch,
};
use reqwest::header;
use reqwest::{Method, StatusCode};
use uuid::Uuid;

use crate::{Client, Error, decode, expect_status, is_problem};

/// The header a write's key travels in.
const IDEMPOTENCY_KEY: &str = "Idempotency-Key";

impl Client {
    /// The adopted feeders, with the server's connection to each.
    pub async fn feeders(&self) -> Result<Vec<Feeder>, Error> {
        let response = self.api(Method::GET, "/feeders").send().await?;
        decode(response, StatusCode::OK).await
    }

    /// Adopt a feeder by its invite for the server. Owners only.
    pub async fn adopt(&self, request: &AdoptFeeder) -> Result<Feeder, Error> {
        let response = self
            .api(Method::POST, "/feeders")
            .json(request)
            .send()
            .await?;
        decode(response, StatusCode::CREATED).await
    }

    /// One feeder and its status.
    pub async fn feeder(&self, id: FeederId) -> Result<FeederDetail, Error> {
        let response = self
            .api(Method::GET, &format!("/feeders/{id}"))
            .send()
            .await?;
        decode(response, StatusCode::OK).await
    }

    /// The feeder's settings, read from it. Their `generation` is the base
    /// an edit names.
    pub async fn settings(&self, id: FeederId) -> Result<Settings, Error> {
        let response = self
            .api(Method::GET, &format!("/feeders/{id}/settings"))
            .send()
            .await?;
        decode(response, StatusCode::OK).await
    }

    /// Change settings, if they are still at `base`. A retry carries the
    /// same `key`; settings that moved on come back as [`Error::Stale`].
    pub async fn put_settings(
        &self,
        id: FeederId,
        base: Generation,
        patch: &SettingsPatch,
        key: Uuid,
    ) -> Result<Settings, Error> {
        let response = self
            .api(Method::PATCH, &format!("/feeders/{id}/settings"))
            .header(header::IF_MATCH, format!("\"{base}\""))
            .header(IDEMPOTENCY_KEY, key.to_string())
            .json(patch)
            .send()
            .await?;

        // A stale edit is answered with the current settings, not a problem.
        if response.status() == StatusCode::PRECONDITION_FAILED && !is_problem(&response) {
            let current = decode(response, StatusCode::PRECONDITION_FAILED).await?;
            return Err(Error::Stale(Box::new(current)));
        }
        decode(response, StatusCode::OK).await
    }

    /// Lock or unlock the feeder's panel. A retry carries the same `key`.
    pub async fn set_lock(&self, id: FeederId, locked: bool, key: Uuid) -> Result<(), Error> {
        let response = self
            .api(Method::POST, &format!("/feeders/{id}/lock"))
            .header(IDEMPOTENCY_KEY, key.to_string())
            .json(&LockRequest { locked })
            .send()
            .await?;
        expect_status(response, StatusCode::NO_CONTENT).await?;
        Ok(())
    }

    /// A page of the feeder's history as the server holds it.
    pub async fn events(&self, id: FeederId, query: &EventQuery) -> Result<EventPage, Error> {
        let response = self
            .api(Method::GET, &format!("/feeders/{id}/events"))
            .query(query)
            .send()
            .await?;
        decode(response, StatusCode::OK).await
    }
}
