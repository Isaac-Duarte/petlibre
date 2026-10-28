//! Bearer tokens: the caller's own, or anyone's for an owner.

use reqwest::{Method, StatusCode};
use wellfed_api::{CreatedToken, NewToken, Token, TokenId};

use crate::{Client, Error, decode, expect_status};

impl Client {
    /// The caller's tokens, or everyone's for an owner. No secret is
    /// listed.
    pub async fn tokens(&self) -> Result<Vec<Token>, Error> {
        let response = self.api(Method::GET, "/tokens").send().await?;
        decode(response, StatusCode::OK).await
    }

    /// Make a token for the logged-in user; only a session may. Its
    /// secret is in the answer and nowhere else: [`crate::Auth::Token`]
    /// takes it.
    pub async fn create_token(&self, token: &NewToken) -> Result<CreatedToken, Error> {
        let response = self.api(Method::POST, "/tokens").json(token).send().await?;
        decode(response, StatusCode::CREATED).await
    }

    /// Revoke a token: the caller's own, or anyone's for an owner.
    pub async fn revoke_token(&self, id: TokenId) -> Result<(), Error> {
        let response = self
            .api(Method::DELETE, &format!("/tokens/{id}"))
            .send()
            .await?;
        expect_status(response, StatusCode::NO_CONTENT).await?;
        Ok(())
    }
}
