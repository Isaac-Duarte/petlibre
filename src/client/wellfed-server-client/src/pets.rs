//! The household's pets, their weight logs and the feeders that serve
//! them. Everyone reads them; owners write them.

use reqwest::{Method, StatusCode};
use wellfed_api::{
    FeederId, NewPet, NewWeight, PageQuery, Pet, PetId, PetPatch, Weight, WeightId, WeightPage,
};

use crate::{Client, Error, decode, expect_status};

impl Client {
    /// Every pet, by name.
    pub async fn pets(&self) -> Result<Vec<Pet>, Error> {
        let response = self.api(Method::GET, "/pets").send().await?;
        decode(response, StatusCode::OK).await
    }

    pub async fn pet(&self, id: PetId) -> Result<Pet, Error> {
        let response = self.api(Method::GET, &format!("/pets/{id}")).send().await?;
        decode(response, StatusCode::OK).await
    }

    /// Add a pet. Owners only.
    pub async fn create_pet(&self, pet: &NewPet) -> Result<Pet, Error> {
        let response = self.api(Method::POST, "/pets").json(pet).send().await?;
        decode(response, StatusCode::CREATED).await
    }

    /// Change a pet. Owners only.
    pub async fn update_pet(&self, id: PetId, patch: &PetPatch) -> Result<Pet, Error> {
        let response = self
            .api(Method::PATCH, &format!("/pets/{id}"))
            .json(patch)
            .send()
            .await?;
        decode(response, StatusCode::OK).await
    }

    /// Delete a pet and its weight log. Owners only.
    pub async fn delete_pet(&self, id: PetId) -> Result<(), Error> {
        let response = self
            .api(Method::DELETE, &format!("/pets/{id}"))
            .send()
            .await?;
        expect_status(response, StatusCode::NO_CONTENT).await?;
        Ok(())
    }

    /// Let `feeder` serve the pet. Owners only.
    pub async fn assign_feeder(&self, pet: PetId, feeder: FeederId) -> Result<Pet, Error> {
        let response = self
            .api(Method::PUT, &format!("/pets/{pet}/feeders/{feeder}"))
            .send()
            .await?;
        decode(response, StatusCode::OK).await
    }

    /// Stop `feeder` serving the pet. Owners only.
    pub async fn unassign_feeder(&self, pet: PetId, feeder: FeederId) -> Result<Pet, Error> {
        let response = self
            .api(Method::DELETE, &format!("/pets/{pet}/feeders/{feeder}"))
            .send()
            .await?;
        decode(response, StatusCode::OK).await
    }

    /// A page of the pet's weight log, oldest reading first.
    pub async fn weights(&self, pet: PetId, query: &PageQuery) -> Result<WeightPage, Error> {
        let response = self
            .api(Method::GET, &format!("/pets/{pet}/weights"))
            .query(query)
            .send()
            .await?;
        decode(response, StatusCode::OK).await
    }

    /// Log a weight. Owners only.
    pub async fn add_weight(&self, pet: PetId, weight: &NewWeight) -> Result<Weight, Error> {
        let response = self
            .api(Method::POST, &format!("/pets/{pet}/weights"))
            .json(weight)
            .send()
            .await?;
        decode(response, StatusCode::CREATED).await
    }

    /// Remove an entry from the weight log. Owners only.
    pub async fn delete_weight(&self, pet: PetId, weight: WeightId) -> Result<(), Error> {
        let response = self
            .api(Method::DELETE, &format!("/pets/{pet}/weights/{weight}"))
            .send()
            .await?;
        expect_status(response, StatusCode::NO_CONTENT).await?;
        Ok(())
    }
}
