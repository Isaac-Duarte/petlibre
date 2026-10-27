//! A feeder's clients and the phones' own keys, and what an owner
//! administers on a feeder: Wi-Fi, a restart, a tune and removing it.

use wellfed_api::{
    FeederClient, FeederId, Generation, Hex, NewPhoneInvite, PhoneInvite, PhoneKey, PlayRequest,
    Removal, TuneName, WifiChange,
};
use reqwest::header;
use reqwest::{Method, StatusCode};
use uuid::Uuid;

use crate::feeders::IDEMPOTENCY_KEY;
use crate::{Client, Error, decode, expect_status};

impl Client {
    /// The feeder's clients, each with whose it is. Owners only.
    pub async fn clients(&self, id: FeederId) -> Result<Vec<FeederClient>, Error> {
        let response = self
            .api(Method::GET, &format!("/feeders/{id}/clients"))
            .send()
            .await?;
        decode(response, StatusCode::OK).await
    }

    /// Revoke one of the feeder's clients. A retry carries the same `key`.
    pub async fn revoke_client(
        &self,
        id: FeederId,
        client: Hex<32>,
        key: Uuid,
    ) -> Result<(), Error> {
        let response = self
            .api(Method::DELETE, &format!("/feeders/{id}/clients/{client}"))
            .header(IDEMPOTENCY_KEY, key.to_string())
            .send()
            .await?;
        expect_status(response, StatusCode::NO_CONTENT).await?;
        Ok(())
    }

    /// Keep a client the server did not invite.
    pub async fn keep_client(&self, id: FeederId, client: Hex<32>) -> Result<(), Error> {
        let response = self
            .api(
                Method::POST,
                &format!("/feeders/{id}/clients/{client}/keep"),
            )
            .send()
            .await?;
        expect_status(response, StatusCode::NO_CONTENT).await?;
        Ok(())
    }

    /// An invite for this token's phone, which joins with `phone`. Never
    /// retried: an invite whose answer was lost is asked for again.
    pub async fn invite_phone(&self, id: FeederId, phone: Hex<32>) -> Result<PhoneInvite, Error> {
        let response = self
            .api(Method::POST, &format!("/feeders/{id}/invites"))
            .json(&NewPhoneInvite { key: phone })
            .send()
            .await?;
        decode(response, StatusCode::CREATED).await
    }

    /// The phone keys the server invited: one's own, or everyone's for an
    /// owner.
    pub async fn phones(&self) -> Result<Vec<PhoneKey>, Error> {
        let response = self.api(Method::GET, "/phones").send().await?;
        decode(response, StatusCode::OK).await
    }

    /// Store Wi-Fi credentials for the feeder's next join, or clear them,
    /// if its Wi-Fi store is still at `base`. A retry carries the same
    /// `key`.
    pub async fn put_wifi(
        &self,
        id: FeederId,
        base: Generation,
        change: &WifiChange,
        key: Uuid,
    ) -> Result<(), Error> {
        let response = self
            .api(Method::PUT, &format!("/feeders/{id}/wifi"))
            .header(header::IF_MATCH, format!("\"{base}\""))
            .header(IDEMPOTENCY_KEY, key.to_string())
            .json(change)
            .send()
            .await?;
        expect_status(response, StatusCode::NO_CONTENT).await?;
        Ok(())
    }

    /// Restart the feeder. An `outcome_unknown` answer is never resent:
    /// the boot count says whether it ran.
    pub async fn reboot(&self, id: FeederId, key: Uuid) -> Result<(), Error> {
        let response = self
            .api(Method::POST, &format!("/feeders/{id}/reboot"))
            .header(IDEMPOTENCY_KEY, key.to_string())
            .send()
            .await?;
        expect_status(response, StatusCode::NO_CONTENT).await?;
        Ok(())
    }

    /// Play a tune now. A retry carries the same `key`.
    pub async fn play(&self, id: FeederId, tune: TuneName, key: Uuid) -> Result<(), Error> {
        let response = self
            .api(Method::POST, &format!("/feeders/{id}/play"))
            .header(IDEMPOTENCY_KEY, key.to_string())
            .json(&PlayRequest { tune })
            .send()
            .await?;
        expect_status(response, StatusCode::NO_CONTENT).await?;
        Ok(())
    }

    /// Remove the feeder from the server, revoking the server's key on it
    /// (the answer says whether that landed) and archiving its history.
    pub async fn remove_feeder(&self, id: FeederId) -> Result<Removal, Error> {
        let response = self
            .api(Method::DELETE, &format!("/feeders/{id}"))
            .send()
            .await?;
        decode(response, StatusCode::OK).await
    }
}
