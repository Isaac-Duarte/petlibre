//! Foods, the food each feeder serves over time, and refills. Everyone
//! reads them; owners write them.

use wellfed_api::{
    FeederFood, FeederId, Food, FoodChoice, FoodHistoryPage, FoodId, FoodPatch, FoodPeriod,
    NewFood, NewRefill, PageQuery, Refill, RefillId, RefillPage,
};
use reqwest::{Method, StatusCode};

use crate::{Client, Error, decode, expect_status};

impl Client {
    /// Every food, by brand and name.
    pub async fn foods(&self) -> Result<Vec<Food>, Error> {
        let response = self.api(Method::GET, "/foods").send().await?;
        decode(response, StatusCode::OK).await
    }

    pub async fn food(&self, id: FoodId) -> Result<Food, Error> {
        let response = self
            .api(Method::GET, &format!("/foods/{id}"))
            .send()
            .await?;
        decode(response, StatusCode::OK).await
    }

    /// Add a food. Owners only.
    pub async fn create_food(&self, food: &NewFood) -> Result<Food, Error> {
        let response = self.api(Method::POST, "/foods").json(food).send().await?;
        decode(response, StatusCode::CREATED).await
    }

    /// Change a food. Owners only.
    pub async fn update_food(&self, id: FoodId, patch: &FoodPatch) -> Result<Food, Error> {
        let response = self
            .api(Method::PATCH, &format!("/foods/{id}"))
            .json(patch)
            .send()
            .await?;
        decode(response, StatusCode::OK).await
    }

    /// Delete a food no feeder has served. Owners only.
    pub async fn delete_food(&self, id: FoodId) -> Result<(), Error> {
        let response = self
            .api(Method::DELETE, &format!("/foods/{id}"))
            .send()
            .await?;
        expect_status(response, StatusCode::NO_CONTENT).await?;
        Ok(())
    }

    /// The feeder's food now, and its newest refill.
    pub async fn feeder_food(&self, feeder: FeederId) -> Result<FeederFood, Error> {
        let response = self
            .api(Method::GET, &format!("/feeders/{feeder}/food"))
            .send()
            .await?;
        decode(response, StatusCode::OK).await
    }

    /// Serve `food` from now on. Owners only.
    pub async fn set_feeder_food(
        &self,
        feeder: FeederId,
        food: FoodId,
    ) -> Result<FoodPeriod, Error> {
        let response = self
            .api(Method::PUT, &format!("/feeders/{feeder}/food"))
            .json(&FoodChoice { food })
            .send()
            .await?;
        decode(response, StatusCode::OK).await
    }

    /// A page of the feeder's food history, oldest first.
    pub async fn food_history(
        &self,
        feeder: FeederId,
        query: &PageQuery,
    ) -> Result<FoodHistoryPage, Error> {
        let response = self
            .api(Method::GET, &format!("/feeders/{feeder}/food/history"))
            .query(query)
            .send()
            .await?;
        decode(response, StatusCode::OK).await
    }

    /// A page of the feeder's refills, in the order they were recorded.
    pub async fn refills(&self, feeder: FeederId, query: &PageQuery) -> Result<RefillPage, Error> {
        let response = self
            .api(Method::GET, &format!("/feeders/{feeder}/refills"))
            .query(query)
            .send()
            .await?;
        decode(response, StatusCode::OK).await
    }

    /// Record a refill made now. Owners only.
    pub async fn add_refill(&self, feeder: FeederId, refill: &NewRefill) -> Result<Refill, Error> {
        let response = self
            .api(Method::POST, &format!("/feeders/{feeder}/refills"))
            .json(refill)
            .send()
            .await?;
        decode(response, StatusCode::CREATED).await
    }

    /// Remove a refill recorded by mistake. Owners only.
    pub async fn delete_refill(&self, feeder: FeederId, refill: RefillId) -> Result<(), Error> {
        let response = self
            .api(
                Method::DELETE,
                &format!("/feeders/{feeder}/refills/{refill}"),
            )
            .send()
            .await?;
        expect_status(response, StatusCode::NO_CONTENT).await?;
        Ok(())
    }
}
