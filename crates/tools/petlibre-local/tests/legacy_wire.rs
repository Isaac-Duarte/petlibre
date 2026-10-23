//! Reproduce the feeder's actual MQTT 3.1 transport rather than a modern SDK CONNECT.
mod common;

use std::time::Duration;

use petlibre_local::{config::Config, storage};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

fn string(value: &str) -> Vec<u8> {
    let mut bytes = u16::try_from(value.len()).unwrap().to_be_bytes().to_vec();
    bytes.extend_from_slice(value.as_bytes());
    bytes
}

fn packet(kind: u8, body: Vec<u8>) -> Vec<u8> {
    let mut output = vec![kind];
    let mut size = body.len();
    loop {
        let mut byte = (size % 128) as u8;
        size /= 128;
        if size > 0 {
            byte |= 0x80;
        }
        output.push(byte);
        if size == 0 {
            break;
        }
    }
    output.extend(body);
    output
}

fn connect(config: &Config, password: &str) -> Vec<u8> {
    let mut body = string("MQIsdp");
    body.extend_from_slice(&[3, 0xc0, 0, 90]); // captured version, flags and keepalive
    body.extend(string(&config.device_client_id));
    body.extend(string(&config.device_username));
    body.extend(string(password));
    packet(0x10, body)
}

async fn read(stream: &mut TcpStream) -> (u8, Vec<u8>) {
    tokio::time::timeout(Duration::from_secs(3), async {
        let kind = stream.read_u8().await.unwrap();
        let mut size = 0;
        for index in 0..4 {
            let byte = stream.read_u8().await.unwrap();
            size |= usize::from(byte & 0x7f) << (7 * index);
            assert!(size <= 65_536);
            if byte & 0x80 == 0 {
                break;
            }
        }
        let mut body = vec![0; size];
        stream.read_exact(&mut body).await.unwrap();
        (kind, body)
    })
    .await
    .expect("timed out waiting for a broker packet")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn captured_legacy_connect_and_both_subscriptions_reach_local_clock_service() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.json");
    let mut config = common::config();
    config.mqtt_listen = common::address();
    config.http_listen = common::address();
    storage::write_json(&path, &config).unwrap();

    let _service = common::start(&path);
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    common::ready(&config, &http).await;

    // Normalization must preserve authentication rather than bypassing it.
    let mut rejected = TcpStream::connect(config.mqtt_listen).await.unwrap();
    rejected
        .write_all(&connect(&config, "incorrect"))
        .await
        .unwrap();
    let (kind, body) = read(&mut rejected).await;
    assert_eq!(kind, 0x20);
    assert_ne!(body[1], 0);
    drop(rejected);

    let mut device = TcpStream::connect(config.mqtt_listen).await.unwrap();
    // Exercise a CONNECT spread across multiple TCP writes.
    for chunk in connect(&config, &config.device_password).chunks(3) {
        device.write_all(chunk).await.unwrap();
        tokio::task::yield_now().await;
    }
    assert_eq!(read(&mut device).await, (0x20, vec![0, 0]));

    for (id, suffix) in [(2, "+/sub"), (3, "broadcast/sub")] {
        let mut body = vec![0, id];
        body.extend(string(&config.topic(suffix)));
        body.push(1);
        device.write_all(&packet(0x82, body)).await.unwrap();
        assert_eq!(
            read(&mut device).await,
            (0x90, vec![0, id, 1]),
            "captured subscription was not granted QoS 1"
        );
    }

    let mut body = string(&config.topic("ntp/post"));
    body.extend_from_slice(&[0, 4]);
    body.extend(serde_json::to_vec(&json!({"cmd":"NTP","ts":0})).unwrap());
    device.write_all(&packet(0x32, body)).await.unwrap();

    let mut acknowledged = false;
    let mut calibrated = false;
    for _ in 0..6 {
        let (kind, body) = read(&mut device).await;
        if kind == 0x40 {
            assert_eq!(body, vec![0, 4]);
            acknowledged = true;
        }

        if kind >> 4 == 3 {
            // QoS 1 subscriptions must not upgrade the server's QoS 0 publication.
            assert_eq!(kind & 0x0f, 0);
            let length = usize::from(u16::from_be_bytes([body[0], body[1]]));
            let topic = std::str::from_utf8(&body[2..2 + length]).unwrap();
            let message: Value = serde_json::from_slice(&body[2 + length..]).unwrap();
            if topic == config.topic("ntp/sub") {
                assert_eq!(message["cmd"], "NTP");
                assert_eq!(message["code"], 0);
                assert_eq!(message["calibrationTag"], true);
                calibrated = true;
            } else {
                assert_eq!(message["cmd"], "ATTR_GET_SERVICE");
            }
        }
        if acknowledged && calibrated {
            break;
        }
    }
    assert!(
        acknowledged && calibrated,
        "legacy peer did not receive PUBACK and clock response"
    );

    let status = common::status(&config, &http).await;
    assert_eq!(status["device_online"], true);
    assert_eq!(status["feeds"], json!([]), "handshake initiated a feed");
    drop(device);

    // Although the legacy peer requested persistence, the normalized connection
    // never restores a previous subscription or an offline command session.
    let mut reconnected = TcpStream::connect(config.mqtt_listen).await.unwrap();
    reconnected
        .write_all(&connect(&config, &config.device_password))
        .await
        .unwrap();
    assert_eq!(read(&mut reconnected).await, (0x20, vec![0, 0]));
}
