use std::{sync::Arc, time::Duration};

use anyhow::Result;
use chrono::Utc;
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{info, warn};

use crate::{
    config::Config,
    controller::{Controller, FeedInput, FeedRecord, Rejection, ResolveInput, Status},
    ota_image, ota_trial, probe, protocol,
};

pub type OperationResult = std::result::Result<FeedRecord, Rejection>;
pub type TrialResult = std::result::Result<ota_trial::Record, Rejection>;
pub type ProbeResult = std::result::Result<probe::Record, Rejection>;
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
}

pub async fn run(
    config: Arc<Config>,
    mut controller: Controller,
    mut requests: mpsc::Receiver<Request>,
    status: watch::Sender<Status>,
) -> Result<()> {
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
                            client.subscribe(config.topic("+/post"),QoS::AtLeastOnce).await?;
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
                            status.send_replace(controller.status.clone());
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
            status.send_replace(controller.status.clone());
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
                        None => return Ok(()),
                    }
                }
            }
        }
    }
}
