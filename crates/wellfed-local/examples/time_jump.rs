//! FW21 bench test aid: send the replacement firmware one `time` request
//! shifted by `--shift-ms` from the host clock, as the server role, and print
//! the firmware's reply. Run beside `wellfed-local broker` only, never beside
//! `serve` (both would hold the server role). The next `serve` resyncs the
//! firmware to the true time, which is itself a jump back.
//!
//! With a request key in the config, it waits for the firmware's next health
//! (up to 45 s) and signs under its nonce starting at `seq` 1; a firmware that
//! already accepted requests in this session answers `"error":"auth"`.
//!
//! ```text
//! cargo run --example time_jump -- --config private/server.json --shift-ms 3600000
//! ```

use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{TimeDelta, Utc};
use wellfed_local::{config::Config, replacement};
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};

#[tokio::main(flavor = "current_thread")]
#[allow(clippy::print_stdout)]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let (mut config, mut shift_ms) = (None, 0i64);
    while let Some(arg) = args.next() {
        let mut value = || args.next().context("missing value");
        match arg.as_str() {
            "--config" => config = Some(value()?),
            "--shift-ms" => shift_ms = value()?.parse().context("--shift-ms")?,
            other => anyhow::bail!("unknown argument {other}"),
        }
    }
    let config: Config = serde_json::from_slice(&std::fs::read(config.context("--config")?)?)?;
    anyhow::ensure!(config.firmware.is_some(), "the config has no firmware role");

    let mut options = MqttOptions::new(
        config.server_client_id(),
        config.mqtt_listen.ip().to_string(),
        config.mqtt_listen.port(),
    );
    options.set_credentials("local-server", &config.server_password);
    options.set_keep_alive(Duration::from_secs(15));
    let (client, mut eventloop) = AsyncClient::new(options, 8);

    let key = config.firmware.as_ref().and_then(|f| f.request_key());
    let reply = config.firmware_topic("reply");
    let health = config.firmware_topic("health");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    let mut tracker = replacement::Tracker::default();
    let mut sent = None;
    loop {
        let event = tokio::time::timeout_at(deadline, eventloop.poll())
            .await
            .context("no reply within 45 s")?;
        let ready = match &event {
            Ok(Event::Incoming(Packet::ConnAck(_))) => {
                client.subscribe(&reply, QoS::AtMostOnce).await?;
                client.subscribe(&health, QoS::AtMostOnce).await?;
                key.is_none()
            }
            Ok(Event::Incoming(Packet::Publish(publish))) if publish.topic == health => {
                tracker.observe_health(&publish.payload, Utc::now()) && sent.is_none()
            }
            _ => false,
        };
        if ready && sent.is_none() {
            let id = uuid::Uuid::new_v4();
            let at = Utc::now() + TimeDelta::milliseconds(shift_ms);
            let request = replacement::time_request(id, at, config.timezone);
            let payload = tracker
                .finish(request, key.as_ref())
                .map_err(|error| anyhow::anyhow!(error.message()))?;
            client
                .publish(
                    config.firmware_topic("request"),
                    QoS::AtMostOnce,
                    false,
                    payload.clone(),
                )
                .await?;
            eprintln!("sent {payload}");
            sent = Some(id.to_string());
        }
        match event {
            Ok(Event::Incoming(Packet::Publish(publish))) if publish.topic == reply => {
                let message: serde_json::Value = serde_json::from_slice(&publish.payload)?;
                if sent.as_deref() == message["id"].as_str() {
                    println!("{message}");
                    return Ok(());
                }
            }
            Ok(_) => {}
            Err(error) => anyhow::bail!("connection: {error}"),
        }
    }
}
