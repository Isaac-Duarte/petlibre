//! The household's accounts, which owners manage.

use wellfed_api::{NewUser, User, UserId, UserPatch};
use reqwest::{Method, StatusCode};

use crate::{Client, Error, decode, expect_status};

impl Client {
    /// Every account, by name. Owners only.
    pub async fn users(&self) -> Result<Vec<User>, Error> {
        let response = self.api(Method::GET, "/users").send().await?;
        decode(response, StatusCode::OK).await
    }

    /// Add an account. Owners only.
    pub async fn create_user(&self, user: &NewUser) -> Result<User, Error> {
        let response = self.api(Method::POST, "/users").json(user).send().await?;
        decode(response, StatusCode::CREATED).await
    }

    /// Change an account's role, expiry or password. Owners only.
    pub async fn update_user(&self, id: UserId, patch: &UserPatch) -> Result<User, Error> {
        let response = self
            .api(Method::PATCH, &format!("/users/{id}"))
            .json(patch)
            .send()
            .await?;
        decode(response, StatusCode::OK).await
    }

    /// Delete an account and end its sessions. Owners only.
    pub async fn delete_user(&self, id: UserId) -> Result<(), Error> {
        let response = self
            .api(Method::DELETE, &format!("/users/{id}"))
            .send()
            .await?;
        expect_status(response, StatusCode::NO_CONTENT).await?;
        Ok(())
    }
}
