//! The household: its name, units, time format and zone.

use reqwest::{Method, StatusCode};
use wellfed_api::{Household, HouseholdPatch};

use crate::{Client, Error, decode};

impl Client {
    pub async fn household(&self) -> Result<Household, Error> {
        let response = self.api(Method::GET, "/household").send().await?;
        decode(response, StatusCode::OK).await
    }

    /// Change the household; a new zone goes to every feeder. Owners only.
    pub async fn update_household(&self, patch: &HouseholdPatch) -> Result<Household, Error> {
        let response = self
            .api(Method::PATCH, "/household")
            .json(patch)
            .send()
            .await?;
        decode(response, StatusCode::OK).await
    }
}
