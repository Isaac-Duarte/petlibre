use std::{sync::Arc, time::Duration};

use anyhow::Result;
use chrono::Utc;
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS, SubscribeFilter};
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{info, warn};

use crate::{
    config::Config,
    controller::{Controller, FeedInput, FeedRecord, Rejection, ResolveInput, Status},
    firmware_ota, ota_image, ota_trial, probe, protocol,
    replacement::{self, RequestError},
};

pub type OperationResult = std::result::Result<FeedRecord, Rejection>;
pub type TrialResult = std::result::Result<ota_trial::Record, Rejection>;
pub type ProbeResult = std::result::Result<probe::Record, Rejection>;
pub type ReplacementResult = std::result::Result<uuid::Uuid, RequestError>;
pub enum Request {
    Feed(FeedInput, oneshot::Sender<OperationResult>),
    Resolve(ResolveInput, oneshot::Sender<OperationResult>),
    OtaTrial(
        ota_trial::Input,
        Option<ota_image::Lease>,
        oneshot::Sender<TrialResult>,
    ),
    ResolveTrial(ota_trial::ResolveInput, oneshot::Sender<TrialResult>),
    ProbeQuery(probe::Input, oneshot::Sender<ProbeResult>),
    Replacement(replacement::Op, oneshot::Sender<ReplacementResult>),
    /// Load (or with `None`, clear) the update the server answers `ota/get` from.
    OtaOffer(Option<firmware_ota::Offer>, oneshot::Sender<bool>),
}

/// The controller's status plus the replacement-firmware snapshot at `now`.
fn publish_status(
    status: &watch::Sender<Status>,
    controller: &Controller,
    tracker: &replacement::Tracker,
) {
    let mut value = controller.status.clone();
    if controller.config.firmware.is_some() {
        value.replacement = Some(tracker.snapshot(Utc::now()));
    }
    status.send_replace(value);
}

/// Send one allowlisted request on QoS 0 only to fresh, online firmware; never
/// queued for a later connection.
fn replacement_request(
    config: &Config,
    client: &AsyncClient,
    tracker: &replacement::Tracker,
    op: replacement::Op,
) -> ReplacementResult {
    if config.firmware.is_none() {
        return Err(RequestError::Unconfigured);
    }
    if !tracker.snapshot(Utc::now()).online {
        return Err(RequestError::Offline);
    }
    let id = uuid::Uuid::new_v4();
    let payload = serde_json::to_vec(&serde_json::json!({"id": id, "op": op}))
        .map_err(|_| RequestError::Offline)?;
    client
        .try_publish(
            config.firmware_topic("request"),
            QoS::AtMostOnce,
            false,
            payload,
        )
        .map_err(|_| RequestError::Offline)?;
    Ok(id)
}

pub async fn run(
    config: Arc<Config>,
    mut controller: Controller,
    mut requests: mpsc::Receiver<Request>,
    status: watch::Sender<Status>,
) -> Result<()> {
    // Survives reconnects: health ages out rather than vanishing.
    let mut tracker = replacement::Tracker::default();
    // Owner-loaded only; never persisted, never offered unasked.
    let mut offer: Option<firmware_ota::Offer> = None;
    loop {
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
        loop {
            tokio::select! {
                event = eventloop.poll() => {
                    match event {
                        Ok(Event::Incoming(Packet::ConnAck(_))) => {
                            // One SUBSCRIBE, so one SUBACK starts the session below.
                            let mut filters = vec![SubscribeFilter::new(config.topic("+/post"),QoS::AtLeastOnce)];
                            if config.firmware.is_some() {
                                filters.push(SubscribeFilter::new(config.firmware_topic("health"),QoS::AtMostOnce));
                                filters.push(SubscribeFilter::new(config.firmware_topic("reply"),QoS::AtMostOnce));
                                filters.push(SubscribeFilter::new(config.firmware_topic("ota/get"),QoS::AtMostOnce));
                            }
                            client.subscribe_many(filters).await?;
                            info!("local MQTT server connected");
                        }
                        Ok(Event::Incoming(Packet::SubAck(ack))) => {
                            anyhow::ensure!(ack.return_codes.iter().all(|code|matches!(code,rumqttc::SubscribeReasonCode::Success(_))),"broker refused server subscription");
                            controller.connected(Utc::now());
                            let query = protocol::service(&config,"ATTR_GET_SERVICE",uuid::Uuid::new_v4(),Utc::now());
                            client.try_publish(query.topic,QoS::AtMostOnce,false,serde_json::to_vec(&query.payload)?)?;
                        }
                        Ok(Event::Incoming(Packet::Publish(publish))) => {
                            if publish.retain { warn!("ignored retained device message"); continue; }
                            if config.firmware.is_some() && publish.topic == config.firmware_topic("health") {
                                if !tracker.observe_health(&publish.payload,Utc::now()) { warn!("ignored invalid replacement health"); }
                                publish_status(&status,&controller,&tracker);
                                continue;
                            }
                            if config.firmware.is_some() && publish.topic == config.firmware_topic("ota/get") {
                                match firmware_ota::Offer::answer(offer.as_ref(),&publish.payload) {
                                    Some(answer) => { let _ = client.try_publish(config.firmware_topic("ota/data"),QoS::AtMostOnce,false,answer); }
                                    None => warn!("ignored malformed replacement update request"),
                                }
                                continue;
                            }
                            if config.firmware.is_some() && publish.topic == config.firmware_topic("reply") {
                                if !tracker.observe_reply(&publish.payload,Utc::now()) { warn!("ignored invalid replacement reply"); }
                                publish_status(&status,&controller,&tracker);
                                continue;
                            }
                            match protocol::parse(&config,&publish.topic,&publish.payload) {
                                Ok((channel,message)) => {
                                    let first_seen=controller.status.last_seen.is_none();
                                    controller.observe(&channel,&message,Utc::now())?;
                                    if let Some(reply) = protocol::reply(&config,&channel,&message,Utc::now()) {
                                        client.try_publish(reply.topic,QoS::AtMostOnce,false,serde_json::to_vec(&reply.payload)?)?;
                                    }
                                    if first_seen || message.cmd=="DEVICE_START_EVENT" {
                                        let query=protocol::service(&config,"ATTR_GET_SERVICE",uuid::Uuid::new_v4(),Utc::now());
                                        client.try_publish(query.topic,QoS::AtMostOnce,false,serde_json::to_vec(&query.payload)?)?;
                                    }
                                    info!(command=%message.cmd,"device message");
                                }
                                Err(_) => warn!("ignored invalid device message"),
                            }
                        }
                        Ok(_) => {}
                        Err(_) => {
                            warn!("MQTT disconnected; discarding pending outgoing messages");
                            controller.disconnected(Utc::now())?;
                            publish_status(&status,&controller,&tracker);
                            break;
                        }
                    }
                }
                request = requests.recv() => {
                    let Some(request) = request else { return Ok(()); };
                    match request {
                        Request::Feed(input,response) => {
                            let outcome = controller.feed(input,Utc::now());
                            let result = match outcome {
                                Ok((record,Some(command))) => {
                                    // QoS 0, no retain, no offline session, and a new
                                    // client/eventloop on reconnect: never replay a feed.
                                    if client.try_publish(command.topic,QoS::AtMostOnce,false,serde_json::to_vec(&command.payload)?).is_err() {
                                        controller.disconnected(Utc::now())?;
                                        Err(Rejection::Offline)
                                    } else { Ok(record) }
                                }
                                Ok((record,None)) => Ok(record),
                                Err(error) => Err(error),
                            };
                            let _ = response.send(result);
                        }
                        Request::Resolve(input,response) => { let _ = response.send(controller.resolve(input,Utc::now())); }
                        Request::ResolveTrial(input,response) => { let _ = response.send(controller.resolve_trial(input,Utc::now())); }
                        Request::Replacement(op,response) => { let _ = response.send(replacement_request(&config,&client,&tracker,op)); }
                        Request::OtaOffer(next,response) => {
                            if let Some(o) = &next { info!(version=o.version,bytes=o.payload.len(),"replacement update offered"); }
                            else { info!("replacement update offer cleared"); }
                            offer = next;
                            let _ = response.send(config.firmware.is_some());
                        }
                        Request::ProbeQuery(input,response) => {
                            let result = match controller.probe_query(input,Utc::now()) {
                                Ok((record,Some(command))) => {
                                    if client.try_publish(command.topic,QoS::AtMostOnce,false,serde_json::to_vec(&command.payload)?).is_err() {
                                        controller.disconnected(Utc::now())?;
                                        Err(Rejection::Offline)
                                    } else { Ok(record) }
                                }
                                Ok((record,None)) => Ok(record),
                                Err(error) => Err(error),
                            };
                            let _ = response.send(result);
                        }
                        Request::OtaTrial(input,lease,response) => {
                            let result = match controller.ota_trial(input,lease.as_ref(),Utc::now()) {
                                Ok((record,Some(command))) => {
                                    if client.try_publish(command.topic,QoS::AtMostOnce,false,serde_json::to_vec(&command.payload)?).is_err() {
                                        controller.disconnected(Utc::now())?;
                                        Err(Rejection::Offline)
                                    } else { Ok(record) }
                                }
                                Ok((record,None)) => Ok(record),
                                Err(error) => Err(error),
                            };
                            let _ = response.send(result);
                        }
                    }
                }
                _ = tick.tick() => { controller.expire(Utc::now())?; }
            }
            publish_status(&status, &controller, &tracker);
        }
        // Serve API requests while offline without accumulating commands for a
        // future session. All newly submitted feeds are rejected by the controller.
        let delay = tokio::time::sleep(Duration::from_secs(2));
        tokio::pin!(delay);
        loop {
            tokio::select! {
                _ = &mut delay => break,
                request = requests.recv() => {
                    match request {
                        Some(Request::Feed(input,response)) => {
                            let result = controller.feed(input,Utc::now()).map(|(record,_)|record);
                            let _ = response.send(result);
                        }
                        Some(Request::Resolve(input,response)) => { let _ = response.send(controller.resolve(input,Utc::now())); }
                        Some(Request::OtaTrial(input,lease,response)) => {
                            let result = controller.ota_trial(input,lease.as_ref(),Utc::now()).map(|(record,_)|record);
                            let _ = response.send(result);
                        }
                        Some(Request::ResolveTrial(input,response)) => { let _ = response.send(controller.resolve_trial(input,Utc::now())); }
                        Some(Request::ProbeQuery(input,response)) => {
                            let result = controller.probe_query(input,Utc::now()).map(|(record,_)|record);
                            let _ = response.send(result);
                        }
                        Some(Request::Replacement(_,response)) => {
                            let error = if config.firmware.is_some() { RequestError::Offline } else { RequestError::Unconfigured };
                            let _ = response.send(Err(error));
                        }
                        // An offer is server state, not a device command: keep it across reconnects.
                        Some(Request::OtaOffer(next,response)) => { offer = next; let _ = response.send(config.firmware.is_some()); }
                        None => return Ok(()),
                    }
                }
            }
        }
    }
}
