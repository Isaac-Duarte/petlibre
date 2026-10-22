//! FW10 bench test aid: answer the replacement firmware's `ota/get` as the
//! server role, optionally with one injected fault, so the updater's failure
//! paths can be exercised on hardware. Run beside `wellfed-local broker`
//! only, never beside `serve` (two servers would both answer). The real
//! server refuses a mismatched offer, so faults cannot come from it.
//!
//! ```text
//! cargo run --example ota_fault_server -- --config private/server.json \
//!     --manifest M --image I [--fault none|corrupt|range|stall] [--at OFFSET]
//! ```
//!
//! - `corrupt`: flip one byte of the chunk at `--at` (the hash must fail).
//! - `range`: answer `Range` from `--at` on (a truncated offer).
//! - `stall`: stop answering from `--at` on (the updater must give up).
//!
//! Every answer is logged with its time, for transfer timing.

use std::{path::PathBuf, time::Duration, time::Instant};

use anyhow::{Context, Result, bail};
use wellfed_core::ota;
use wellfed_local::{config::Config, firmware_ota::Offer};
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Fault {
    None,
    Corrupt,
    Range,
    Stall,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let (mut config, mut manifest, mut image) = (None, None, None);
    let (mut fault, mut at) = (Fault::None, 8192u32);

    while let Some(arg) = args.next() {
        let mut value = || args.next().context("missing value");
        match arg.as_str() {
            "--config" => config = Some(PathBuf::from(value()?)),
            "--manifest" => manifest = Some(PathBuf::from(value()?)),
            "--image" => image = Some(PathBuf::from(value()?)),
            "--at" => at = value()?.parse()?,
            "--fault" => {
                fault = match value()?.as_str() {
                    "none" => Fault::None,
                    "corrupt" => Fault::Corrupt,
                    "range" => Fault::Range,
                    "stall" => Fault::Stall,
                    other => bail!("unknown fault {other}"),
                }
            }
            other => bail!("unknown argument {other}"),
        }
    }

    let config = Config::load(&config.context("--config")?)?;
    anyhow::ensure!(config.firmware.is_some(), "the config has no firmware role");
    let offer = Offer::new(
        &std::fs::read(manifest.context("--manifest")?)?,
        &std::fs::read(image.context("--image")?)?,
    )?;

    eprintln!(
        "offering version {} ({} bytes), fault {fault:?} at {at}",
        offer.version,
        offer.payload.len()
    );

    let mut options = MqttOptions::new(
        config.server_client_id(),
        config.mqtt_listen.ip().to_string(),
        config.mqtt_listen.port(),
    );
    options.set_credentials("local-server", &config.server_password);
    options.set_keep_alive(Duration::from_secs(15));
    options.set_max_packet_size(65_536, 65_536);
    let (client, mut eventloop) = AsyncClient::new(options, 32);

    let get = config.firmware_topic("ota/get");
    let data = config.firmware_topic("ota/data");
    let started = Instant::now();

    loop {
        match eventloop.poll().await {
            Ok(Event::Incoming(Packet::ConnAck(_))) => {
                client.subscribe(&get, QoS::AtMostOnce).await?;
                eprintln!("connected");
            }
            Ok(Event::Incoming(Packet::Publish(publish))) if publish.topic == get => {
                let Some((part, offset, len)) = ota::decode_get(&publish.payload) else {
                    continue;
                };

                let t = started.elapsed().as_secs_f32();
                let faulted = part == ota::Part::Chunk && offset >= at;
                let answer = match (fault, faulted) {
                    (Fault::Stall, true) => {
                        eprintln!("{t:8.2} get {part:?} {offset} {len}: stalled");
                        continue;
                    }
                    (Fault::Range, true) => {
                        ota::encode_data_header(part, ota::Status::Range, offset, 0).to_vec()
                    }
                    _ => {
                        let mut answer =
                            Offer::answer(Some(&offer), &publish.payload).context("answer")?;
                        if fault == Fault::Corrupt && part == ota::Part::Chunk && offset == at {
                            let last = answer.len() - 1;
                            answer[last] ^= 0x01;
                        }
                        answer
                    }
                };

                eprintln!("{t:8.2} get {part:?} {offset} {len}");
                client
                    .publish(&data, QoS::AtMostOnce, false, answer)
                    .await?;
            }
            Ok(_) => {}
            Err(error) => {
                eprintln!("connection: {error}; retrying");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}
