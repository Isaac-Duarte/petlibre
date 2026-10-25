//! The replacement firmware's side of the server's MQTT session, beside
//! stock's (`wellfed_stock::mqtt`): when a firmware role is configured, it
//! carries the firmware's health and update topics. It sends the firmware no
//! requests: those go over the Noise device API, and only bank 0's recovery
//! role still publishes health here.

use std::sync::Arc;

use chrono::Utc;
use wellfed_stock::mqtt::{Companion, Evidence};
use rumqttc::{AsyncClient, Publish, QoS, SubscribeFilter};
use tokio::sync::oneshot;
use tracing::{info, warn};

use crate::{
    config::Config,
    ota,
    replacement::{self, Health, Status},
};

/// Load (or with `None`, clear) the update the server answers `ota/get`
/// from; answered with whether a firmware role is configured.
pub struct SetOffer(pub Option<ota::firmware::Offer>, pub oneshot::Sender<bool>);

pub type Request = wellfed_stock::mqtt::Request<SetOffer>;

pub struct Firmware {
    config: Arc<Config>,
    /// Survives reconnects: health ages out rather than vanishing.
    tracker: replacement::Tracker,
    /// Owner-loaded only; never persisted, never offered unasked.
    offer: Option<ota::firmware::Offer>,
}

impl Firmware {
    pub fn new(config: Arc<Config>) -> Self {
        Self {
            config,
            tracker: replacement::Tracker::default(),
            offer: None,
        }
    }

    fn health(&mut self, payload: &[u8], evidence: Evidence<'_, Health>) {
        let now = Utc::now();
        if !self.tracker.observe_health(payload, now) {
            warn!("ignored invalid replacement health");
        } else if self.tracker.snapshot(now).online
            && let Err(error) = evidence.replacement_healthy(now)
        {
            warn!(%error, "replacement install not recorded as healthy");
        }
    }

    fn update_request(&mut self, client: &AsyncClient, payload: &[u8]) {
        let (answer, dropped) = ota::firmware::Offer::serve(&mut self.offer, payload);
        if dropped {
            warn!(
                "replacement update offer dropped: delivered once, then asked for again after a fallback; offer again to retry"
            );
        }

        match answer {
            Some(answer) => {
                let _ = client.try_publish(
                    self.config.firmware_topic("ota/data"),
                    QoS::AtMostOnce,
                    false,
                    answer,
                );
            }
            None => warn!("ignored malformed replacement update request"),
        }
    }
}

impl Companion for Firmware {
    type Health = Health;
    type Request = SetOffer;

    fn filters(&self) -> Vec<SubscribeFilter> {
        if self.config.firmware.is_none() {
            return Vec::new();
        }
        ["health", "ota/get", "ota/check"]
            .into_iter()
            .map(|topic| SubscribeFilter::new(self.config.firmware_topic(topic), QoS::AtMostOnce))
            .collect()
    }

    fn incoming(
        &mut self,
        client: &AsyncClient,
        publish: &Publish,
        evidence: Evidence<'_, Health>,
    ) -> bool {
        if self.config.firmware.is_none() {
            return false;
        }

        let topic = |suffix| self.config.firmware_topic(suffix);
        if publish.topic == topic("health") {
            self.health(&publish.payload, evidence);
            return true;
        }
        if publish.topic == topic("ota/get") {
            self.update_request(client, &publish.payload);
            return true;
        }
        if publish.topic == topic("ota/check") {
            // The normal role's offer check: the manifest only, and never
            // counted as a delivery.
            let offered = ota::firmware::Offer::check(self.offer.as_ref());
            let _ = client.try_publish(
                self.config.firmware_topic("ota/offered"),
                QoS::AtMostOnce,
                false,
                offered,
            );
            return true;
        }
        false
    }

    fn status(&self, status: &mut Status) {
        if self.config.firmware.is_some() {
            status.replacement = Some(self.tracker.snapshot(Utc::now()));
        }
    }

    fn request(&mut self, SetOffer(next, response): SetOffer) {
        match &next {
            Some(offer) => info!(
                version = offer.version,
                bytes = offer.payload.len(),
                "replacement update offered"
            ),
            None => info!("replacement update offer cleared"),
        }
        // An offer is server state, not a device command: kept across reconnects.
        self.offer = next;
        let _ = response.send(self.config.firmware.is_some());
    }
}
