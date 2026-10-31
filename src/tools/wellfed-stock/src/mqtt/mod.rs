//! The server's MQTT client, a single actor that owns the `Controller`.
//!
//! It answers the stock device on its namespace and serves API requests
//! arriving over a channel. Commands go out on QoS 0 without retain, and
//! every reconnect builds a new client, so nothing is replayed into a later
//! session; while disconnected, requests are still answered but no command
//! is queued.
//!
//! The broker this client and the feeder connect to is `broker`, with
//! `compat` normalizing the stock firmware's CONNECT in front of it.

pub mod broker;
pub(crate) mod compat;

use std::{sync::Arc, time::Duration};

use anyhow::{Result, ensure};
use chrono::Utc;
use rumqttc::{AsyncClient, ConnectionError, Event, MqttOptions, Packet, Publish, QoS};
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{info, warn};

use crate::{
    config::Config,
    controller::{Controller, Rejection, Status},
    ota,
    protocol::{self, Reply},
};

pub type TrialResult = std::result::Result<ota::trial::Record, Rejection>;

/// An API request.
pub enum Request {
    OtaTrial(
        ota::trial::Input,
        Option<ota::image::Lease>,
        oneshot::Sender<TrialResult>,
    ),
    ResolveTrial(ota::trial::ResolveInput, oneshot::Sender<TrialResult>),
    /// The operator's client saw the replacement answer the device API
    /// ([`Controller::replacement_answered`]).
    TrialAnswered(ota::trial::AnsweredInput, oneshot::Sender<TrialResult>),
}

/// QoS 0, no retain, no offline session, and a new client on reconnect: a
/// command published here is never replayed.
fn publish(client: &AsyncClient, reply: Reply) -> Result<()> {
    client.try_publish(
        reply.topic,
        QoS::AtMostOnce,
        false,
        serde_json::to_vec(&reply.payload)?,
    )?;
    Ok(())
}

/// The actor's state that outlives one broker connection.
struct Actor {
    config: Arc<Config>,
    controller: Controller,
    status: watch::Sender<Status>,
}

impl Actor {
    fn publish_status(&self) {
        self.status.send_replace(self.controller.status.clone());
    }

    async fn subscribe(&self, client: &AsyncClient) -> Result<()> {
        client
            .subscribe(self.config.topic("+/post"), QoS::AtLeastOnce)
            .await?;
        info!("local MQTT server connected");
        Ok(())
    }

    fn query_attributes(&self, client: &AsyncClient) -> Result<()> {
        let query = protocol::service(
            &self.config,
            protocol::Sent::Attributes,
            uuid::Uuid::new_v4(),
            Utc::now(),
        );
        publish(client, query)
    }

    fn session_started(&mut self, client: &AsyncClient, ack: &rumqttc::SubAck) -> Result<()> {
        ensure!(
            ack.return_codes
                .iter()
                .all(|code| matches!(code, rumqttc::SubscribeReasonCode::Success(_))),
            "broker refused server subscription"
        );
        self.controller.connected(Utc::now());
        self.query_attributes(client)
    }

    fn incoming(&mut self, client: &AsyncClient, publish: &Publish) -> Result<()> {
        if publish.retain {
            warn!("ignored retained device message");
            return Ok(());
        }

        self.device_message(client, publish)
    }

    fn device_message(&mut self, client: &AsyncClient, incoming: &Publish) -> Result<()> {
        let Ok((channel, message)) =
            protocol::parse(&self.config, &incoming.topic, &incoming.payload)
        else {
            warn!("ignored invalid device message");
            return Ok(());
        };

        let now = Utc::now();
        let first_seen = self.controller.status.last_seen.is_none();
        self.controller.observe(channel, &message, now)?;
        if let Some(reply) = protocol::reply(&self.config, channel, &message, now) {
            publish(client, reply)?;
        }
        if first_seen || message.cmd == "DEVICE_START_EVENT" {
            self.query_attributes(client)?;
        }

        info!(command = %message.cmd, "device message");
        Ok(())
    }

    /// Hand a newly authorized command to MQTT. If it cannot be queued the
    /// connection is treated as lost, which marks the operation uncertain.
    fn dispatch<T>(
        &mut self,
        client: &AsyncClient,
        outcome: std::result::Result<(T, Option<Reply>), Rejection>,
    ) -> Result<std::result::Result<T, Rejection>> {
        let (record, command) = match outcome {
            Ok(accepted) => accepted,
            Err(rejection) => return Ok(Err(rejection)),
        };

        if let Some(command) = command
            && publish(client, command).is_err()
        {
            self.controller.disconnected(Utc::now())?;
            return Ok(Err(Rejection::Offline));
        }
        Ok(Ok(record))
    }

    fn request(&mut self, client: &AsyncClient, request: Request) -> Result<()> {
        let now = Utc::now();
        match request {
            Request::OtaTrial(input, lease, response) => {
                let outcome = self.controller.ota_trial(input, lease.as_ref(), now);
                let result = self.dispatch(client, outcome)?;
                let _ = response.send(result);
            }

            Request::ResolveTrial(input, response) => {
                let result = self.controller.resolve_trial(input, now);
                // The evidence includes a version report from the last five
                // minutes, which stock sends only around a connect. A
                // read-only attribute query refreshes it for the caller's
                // next attempt; its answer is not an OTA report, so the quiet
                // interval runs on.
                if result.as_ref().err() == Some(&Rejection::ResolutionEvidence)
                    && self.controller.status.device_online
                    && !self.controller.version_fresh(now)
                {
                    self.query_attributes(client)?;
                }
                let _ = response.send(result);
            }
            other => self.request_without_device(other),
        }
        Ok(())
    }

    /// Requests that send nothing, or (while disconnected) are refused or
    /// recorded without a command: nothing accumulates for a later session.
    fn request_without_device(&mut self, request: Request) {
        let now = Utc::now();
        match request {
            Request::OtaTrial(input, lease, response) => {
                let result = self
                    .controller
                    .ota_trial(input, lease.as_ref(), now)
                    .map(|(record, _)| record);
                let _ = response.send(result);
            }

            Request::ResolveTrial(input, response) => {
                let _ = response.send(self.controller.resolve_trial(input, now));
            }

            Request::TrialAnswered(input, response) => {
                let _ = response.send(self.controller.replacement_answered(input, now));
            }
        }
    }

    /// One broker connection, until it drops or the request channel closes.
    async fn session(&mut self, requests: &mut mpsc::Receiver<Request>) -> Result<Session> {
        let config = &self.config;
        let mut options = MqttOptions::new(
            config.server_client_id(),
            config.mqtt_listen.ip().to_string(),
            config.mqtt_listen.port(),
        );
        options.set_credentials("local-server", &config.server_password);
        options.set_clean_session(true);
        options.set_keep_alive(Duration::from_secs(15));
        options.set_max_packet_size(65_536, 65_536);
        let (client, mut eventloop) = AsyncClient::new(options, 32);
        let mut tick = tokio::time::interval(Duration::from_secs(1));

        // The connect is not cancel-safe: a poll that another branch (the
        // tick, a request) drops mid-connect closes its socket, and the next
        // poll starts the connect over. One whose CONNECT had already reached
        // the broker leaves it registering that client while the new one
        // arrives under the same ID, and the broker's takeover ends both. So
        // one poll runs to the end of the connect, with requests served
        // meanwhile as between sessions.
        let connected = {
            let connecting = eventloop.poll();
            tokio::pin!(connecting);
            loop {
                tokio::select! {
                    event = &mut connecting => break event,

                    request = requests.recv() => match request {
                        Some(request) => self.request_without_device(request),
                        None => return Ok(Session::Closed),
                    },

                    _ = tick.tick() => self.controller.expire(Utc::now())?,
                }
                self.publish_status();
            }
        };
        if let Some(ended) = self.event(&client, connected).await? {
            return Ok(ended);
        }
        self.publish_status();

        loop {
            tokio::select! {
                event = eventloop.poll() => if let Some(ended) = self.event(&client, event).await? {
                    return Ok(ended);
                },

                request = requests.recv() => match request {
                    Some(request) => self.request(&client, request)?,
                    None => return Ok(Session::Closed),
                },

                _ = tick.tick() => self.controller.expire(Utc::now())?,
            }
            self.publish_status();
        }
    }

    /// Act on one event of the connection; the session's end when it
    /// dropped.
    async fn event(
        &mut self,
        client: &AsyncClient,
        event: std::result::Result<Event, ConnectionError>,
    ) -> Result<Option<Session>> {
        match event {
            Ok(Event::Incoming(Packet::ConnAck(_))) => self.subscribe(client).await?,
            Ok(Event::Incoming(Packet::SubAck(ack))) => self.session_started(client, &ack)?,
            Ok(Event::Incoming(Packet::Publish(message))) => self.incoming(client, &message)?,
            Ok(_) => {}
            Err(_) => {
                warn!("MQTT disconnected; discarding pending outgoing messages");
                self.controller.disconnected(Utc::now())?;
                self.publish_status();
                return Ok(Some(Session::Dropped));
            }
        }
        Ok(None)
    }

    /// Serve API requests between connections without accumulating commands
    /// for a future session; the controller refuses a new trial while offline.
    async fn between_sessions(&mut self, requests: &mut mpsc::Receiver<Request>) -> Session {
        let delay = tokio::time::sleep(Duration::from_secs(2));
        tokio::pin!(delay);
        loop {
            tokio::select! {
                _ = &mut delay => return Session::Dropped,
                request = requests.recv() => match request {
                    Some(request) => self.request_without_device(request),
                    None => return Session::Closed,
                },
            }
        }
    }
}

/// How a connection (or the pause after one) ended.
enum Session {
    Dropped,
    /// The API side has gone: the actor stops.
    Closed,
}

pub async fn run(
    config: Arc<Config>,
    controller: Controller,
    mut requests: mpsc::Receiver<Request>,
    status: watch::Sender<Status>,
) -> Result<()> {
    let mut actor = Actor {
        config,
        controller,
        status,
    };

    loop {
        if let Session::Closed = actor.session(&mut requests).await? {
            return Ok(());
        }
        if let Session::Closed = actor.between_sessions(&mut requests).await {
            return Ok(());
        }
    }
}
