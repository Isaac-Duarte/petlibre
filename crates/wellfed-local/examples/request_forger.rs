//! Integration-suite aid: send the replacement firmware requests the real
//! server never would, and print each reply, so request authentication can be
//! checked on hardware. Run beside `wellfed-local broker` only, never beside
//! `serve`: it logs in as the server role, which one client at a time holds.
//!
//! ```text
//! cargo run -p wellfed-local --example request_forger -- --config private/server.json [--seconds 90]
//! ```
//!
//! It waits for a health report carrying the firmware's session nonce, then
//! sends four requests in order and prints one JSON line per case on stdout:
//!
//! - `unsigned`: a state-changing `wifi` clear with no MAC (must be refused).
//!   It clears to the build's credentials, so an accepted forgery still
//!   leaves the feeder joinable.
//! - `wrong_key`: a `settings` read signed with a fresh random key (refused).
//! - `valid`: the same read signed with the configured key (accepted).
//! - `replay`: the `valid` payload sent again unchanged (refused).
//!
//! Nothing here prints the configured key or the firmware's credentials.

#![allow(clippy::print_stdout)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use wellfed_local::config::Config;
use wellfed_local::request_auth::{Request, RequestKey};
use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, QoS};
use serde_json::{Value, json};
use uuid::Uuid;

/// How long each case waits for its reply.
const REPLY_SECONDS: u64 = 10;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let (mut config, mut seconds) = (None, 90u64);

    while let Some(arg) = args.next() {
        let mut value = || args.next().context("missing value");
        match arg.as_str() {
            "--config" => config = Some(PathBuf::from(value()?)),
            "--seconds" => seconds = value()?.parse()?,
            other => bail!("unknown argument {other}"),
        }
    }

    let config = Config::load(&config.context("--config")?)?;
    let key = config
        .firmware
        .as_ref()
        .context("the config has no firmware role")?
        .request_key()
        .context("the firmware role has no request key; nothing is signed to forge against")?;

    let mut options = MqttOptions::new(
        config.server_client_id(),
        config.mqtt_listen.ip().to_string(),
        config.mqtt_listen.port(),
    );
    options.set_credentials("local-server", &config.server_password);
    options.set_keep_alive(Duration::from_secs(15));
    let (client, mut eventloop) = AsyncClient::new(options, 32);

    let health = config.firmware_topic("health");
    let reply = config.firmware_topic("reply");
    let request = config.firmware_topic("request");

    let deadline = Instant::now() + Duration::from_secs(seconds);
    let nonce = loop {
        let Some(message) = next(&mut eventloop, deadline).await? else {
            bail!("no health with a nonce within {seconds} s");
        };
        match message {
            Incoming::Connected => {
                client.subscribe(&health, QoS::AtMostOnce).await?;
                client.subscribe(&reply, QoS::AtMostOnce).await?;
                eprintln!("connected; waiting for health");
            }
            Incoming::Publish(topic, payload) if topic == health => {
                if let Some(nonce) = payload["nonce"].as_str() {
                    break nonce.to_owned();
                }
            }
            Incoming::Publish(..) => {}
        }
    };

    let unsigned = Request::new(Uuid::new_v4(), "wifi")
        .field("clear", 1)
        .unsigned();
    let wrong_key =
        Request::new(Uuid::new_v4(), "settings").signed(&RequestKey::generate()?, &nonce, 1);
    let valid = Request::new(Uuid::new_v4(), "settings").signed(&key, &nonce, 1);
    let cases = [
        ("unsigned", unsigned),
        ("wrong_key", wrong_key),
        ("valid", valid.clone()),
        ("replay", valid),
    ];

    for (case, payload) in cases {
        let id = serde_json::from_str::<Value>(&payload)?["id"].clone();
        client
            .publish(&request, QoS::AtMostOnce, false, payload)
            .await?;

        let deadline = Instant::now() + Duration::from_secs(REPLY_SECONDS);
        let answer = loop {
            match next(&mut eventloop, deadline).await? {
                None => break Value::Null,
                Some(Incoming::Publish(topic, message))
                    if topic == reply && message["id"] == id =>
                {
                    break message;
                }
                Some(_) => {}
            }
        };
        println!("{}", json!({"case": case, "reply": answer}));
    }
    Ok(())
}

enum Incoming {
    Connected,
    Publish(String, Value),
}

/// The next connection or publish before `deadline`; `None` once it passes.
/// A publish that is not JSON comes through as `null`.
async fn next(eventloop: &mut EventLoop, deadline: Instant) -> Result<Option<Incoming>> {
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let Ok(event) = tokio::time::timeout(left, eventloop.poll()).await else {
            return Ok(None);
        };
        match event {
            Ok(Event::Incoming(Packet::ConnAck(_))) => return Ok(Some(Incoming::Connected)),
            Ok(Event::Incoming(Packet::Publish(publish))) => {
                let payload = serde_json::from_slice(&publish.payload).unwrap_or(Value::Null);
                return Ok(Some(Incoming::Publish(publish.topic, payload)));
            }
            Ok(_) => {}
            Err(error) => {
                eprintln!("connection: {error}; retrying");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}
