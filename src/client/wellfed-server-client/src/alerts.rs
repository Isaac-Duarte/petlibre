//! Alerts: everyone reads them; an owner acknowledges one. The stream
//! carries each change as an `alert` update.

use wellfed_api::{Alert, AlertId, AlertPage, AlertQuery};
use reqwest::{Method, StatusCode};

use crate::{Client, Error, decode};

impl Client {
    /// A page of alerts, oldest first.
    pub async fn alerts(&self, query: &AlertQuery) -> Result<AlertPage, Error> {
        let response = self.api(Method::GET, "/alerts").query(query).send().await?;
        decode(response, StatusCode::OK).await
    }

    /// Acknowledge an alert. Owners only. Acknowledging one twice is
    /// harmless: the second answers it as it stands.
    pub async fn acknowledge_alert(&self, id: AlertId) -> Result<Alert, Error> {
        let response = self
            .api(Method::POST, &format!("/alerts/{id}/ack"))
            .send()
            .await?;
        decode(response, StatusCode::OK).await
    }
}
