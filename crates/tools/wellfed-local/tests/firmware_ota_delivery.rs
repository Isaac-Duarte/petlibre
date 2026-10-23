//! FW10 delivery: the firmware's own codec pulls a signed update from the
//! real broker and server, and the firmware's own parser and writer accept it.

mod common;

use std::time::Duration;

use common::{address, ready};
use wellfed_core::ota::{self, Part, SECTOR, Status};
use wellfed_core::transport as mqtt;
use wellfed_local::{config::FirmwareConfig, firmware_ota, storage};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

struct Client {
    stream: TcpStream,
    rx: Vec<u8>,
}

impl Client {
    async fn send(&mut self, bytes: &[u8]) {
        self.stream.write_all(bytes).await.unwrap();
    }

    /// The next PUBLISH payload on `topic`, skipping SUBACK and other packets.
    async fn next_publish(&mut self, topic: &str) -> Vec<u8> {
        loop {
            if let Some((packet, used)) = mqtt::decode(&self.rx).unwrap() {
                let found = match packet {
                    mqtt::Packet::Publish {
                        topic: t, payload, ..
                    } if t == topic => Some(payload.to_vec()),
                    _ => None,
                };
                self.rx.drain(..used);
                if let Some(payload) = found {
                    return payload;
                }
                continue;
            }

            let mut buf = [0; 2048];
            let n = tokio::time::timeout(Duration::from_secs(5), self.stream.read(&mut buf))
                .await
                .expect("broker silent")
                .unwrap();
            assert!(n > 0, "broker closed the connection");
            self.rx.extend_from_slice(&buf[..n]);
        }
    }

    async fn get(
        &mut self,
        config: &wellfed_local::config::Config,
        part: Part,
        offset: u32,
        len: u32,
    ) -> Vec<u8> {
        let mut out = [0; mqtt::MAX_PACKET];
        let n = mqtt::encode_publish(
            &mut out,
            &config.firmware_topic("ota/get"),
            &ota::encode_get(part, offset, len),
            false,
        )
        .unwrap();
        self.send(&out[..n]).await;
        self.next_publish(&config.firmware_topic("ota/data")).await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn firmware_pulls_a_signed_update_through_the_real_broker() {
    let dir = tempfile::tempdir().unwrap();
    let private = dir.path().join("private");
    std::fs::create_dir(&private).unwrap();
    let path = private.join("server.json");
    let mut config = common::config();
    config.mqtt_listen = address();
    config.http_listen = address();
    config.firmware = Some(FirmwareConfig {
        client_id: config.firmware_client_id(),
        username: "wellfed-fw".to_owned(),
        password: "0123456789abcdef".repeat(4),
        request_key: None,
    });
    storage::write_json(&path, &config).unwrap();

    // Owner side: key, a two-and-a-bit-sector image, a signed manifest.
    firmware_ota::keygen(&private.join("ota-key.json")).unwrap();
    let key = firmware_ota::load_key(&private.join("ota-key.json")).unwrap();
    let public = key.verifying_key().to_bytes();
    let mut container = vec![0u8; firmware_ota::CONTAINER_HEADER];
    let mut payload: Vec<u8> = (0..2 * SECTOR + 300).map(|i| (i * 7 % 255) as u8).collect();
    payload[..8].copy_from_slice(&ota::MARKER);
    container.extend_from_slice(&payload);
    std::fs::write(private.join("image.bin"), &container).unwrap();
    std::fs::write(
        private.join("update.manifest"),
        firmware_ota::sign(&key, &container, 2, [9; 32]).unwrap(),
    )
    .unwrap();

    let _service = common::start_in(&path, false, Some(dir.path()));
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .no_proxy()
        .build()
        .unwrap();
    ready(&config, &http).await;

    let firmware = config.firmware.as_ref().unwrap();
    let mut client = Client {
        stream: TcpStream::connect(config.mqtt_listen).await.unwrap(),
        rx: Vec::new(),
    };

    let mut out = [0; mqtt::MAX_PACKET];
    let n = mqtt::encode_connect(
        &mut out,
        &mqtt::Connect {
            client_id: &firmware.client_id,
            username: &firmware.username,
            password: firmware.password.as_bytes(),
            will: None,
        },
    )
    .unwrap();
    client.send(&out[..n]).await;

    let n = mqtt::encode_subscribe(&mut out, 1, &config.firmware_topic("ota/data")).unwrap();
    client.send(&out[..n]).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Before any offer the server answers "no offer".
    let answer = client.get(&config, Part::Manifest, 0, 0).await;
    assert_eq!(ota::decode_data(&answer).unwrap().status, Status::NoOffer);

    // Paths outside private/ are refused; the real offer loads.
    let post = |body: serde_json::Value| {
        http.post(format!(
            "http://{}/api/replacement/ota/offer",
            config.http_listen
        ))
        .bearer_auth(&config.api_token)
        .json(&body)
        .send()
    };

    let refused = post(serde_json::json!({"manifest":"../x","image":"private/image.bin"}))
        .await
        .unwrap();
    assert_eq!(refused.status(), 400);

    let loaded =
        post(serde_json::json!({"manifest":"private/update.manifest","image":"private/image.bin"}))
            .await
            .unwrap();
    assert!(loaded.status().is_success());

    // The firmware's side, end to end.
    let answer = client.get(&config, Part::Manifest, 0, 0).await;
    let data = ota::decode_data(&answer).unwrap();
    let manifest = ota::parse_manifest(data.bytes, &public, 1).unwrap();
    let mut writer = ota::Writer::new(manifest, ota::MAX_CHUNK as usize).unwrap();
    let mut bank = vec![0xffu8; payload.len()];

    while let Some((offset, len)) = writer.next_request() {
        let answer = client.get(&config, Part::Chunk, offset, len).await;
        let data = ota::decode_data(&answer).unwrap();
        assert_eq!((data.status, data.offset), (Status::Ok, offset));
        if let Some(program) = writer.accept(offset, data.bytes).unwrap() {
            let at = (program.offset - ota::BANK1_OFFSET) as usize;
            bank[at..at + program.data.len()].copy_from_slice(program.data);
            writer.programmed(true).unwrap();
        }
    }

    assert_eq!(&bank[..8], &[0xff; 8], "marker withheld until commit");
    assert_eq!(&bank[8..], &payload[8..]);
    assert_eq!(writer.commit(), Ok((ota::BANK1_OFFSET, ota::MARKER)));

    // Clearing the offer stops answers.
    assert!(
        post(serde_json::json!({}))
            .await
            .unwrap()
            .status()
            .is_success()
    );
    let answer = client.get(&config, Part::Chunk, 0, 512).await;
    assert_eq!(ota::decode_data(&answer).unwrap().status, Status::NoOffer);
}
