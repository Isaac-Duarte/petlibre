//! End-to-end stock session: the real `serve` process (broker, MQTT worker
//! and API) with a simulated stock feeder. The broker refuses a wrong
//! password, the server answers the clock and shows only allowlisted
//! attributes, it never sends a feed, and the retired feed route is gone.

mod common;

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use common::{address, ready, start, status};
use wellfed_local::storage;
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use serde_json::{Value, json};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_broker_api_and_simulated_stock_session_without_commands() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.json");
    let mut config = common::config();
    config.mqtt_listen = address();
    config.http_listen = address();
    storage::write_json(&path, &config).unwrap();

    let service = start(&path);
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .no_proxy()
        .build()
        .unwrap();
    ready(&config, &http).await;

    let unauth = http
        .get(format!("http://{}/api/status", config.http_listen))
        .send()
        .await
        .unwrap();
    assert_eq!(unauth.status(), reqwest::StatusCode::UNAUTHORIZED);

    // The embedded broker must reject the wrong password, not allow anonymous use.
    let mut wrong = MqttOptions::new(
        &config.device_client_id,
        config.mqtt_listen.ip().to_string(),
        config.mqtt_listen.port(),
    );
    wrong.set_credentials("test-device", "wrong");
    let (_wrong_client, mut wrong_loop) = AsyncClient::new(wrong, 4);
    let result = tokio::time::timeout(Duration::from_secs(3), wrong_loop.poll())
        .await
        .unwrap();
    assert!(result.is_err(), "broker accepted unauthorized connection");

    let mut options = MqttOptions::new(
        &config.device_client_id,
        config.mqtt_listen.ip().to_string(),
        config.mqtt_listen.port(),
    );
    options.set_credentials(&config.device_username, &config.device_password);
    options.set_clean_session(true);
    let (device, mut events) = AsyncClient::new(options, 32);

    let worker_device = device.clone();
    let worker_config = config.clone();
    let count = Arc::new(AtomicUsize::new(0));
    let worker_count = count.clone();
    let (ntp_send, ntp_receive) = tokio::sync::oneshot::channel();

    let worker = tokio::spawn(async move {
        let mut ntp_send = Some(ntp_send);
        loop {
            match events.poll().await {
                Ok(Event::Incoming(Packet::ConnAck(_))) => {
                    worker_device
                        .subscribe(worker_config.topic("+/sub"), QoS::AtLeastOnce)
                        .await
                        .unwrap();
                }

                Ok(Event::Incoming(Packet::SubAck(_))) => {
                    for (suffix, message) in [
                        ("ntp/post", json!({"cmd":"NTP","ts":0})),
                        (
                            "event/post",
                            json!({"cmd":"ATTR_PUSH_EVENT","volume":50,"softwareVersion":"1.1.15"}),
                        ),
                    ] {
                        worker_device
                            .publish(
                                worker_config.topic(suffix),
                                QoS::AtLeastOnce,
                                false,
                                serde_json::to_vec(&message).unwrap(),
                            )
                            .await
                            .unwrap();
                    }
                }

                Ok(Event::Incoming(Packet::Publish(publish))) => {
                    let message: Value = serde_json::from_slice(&publish.payload).unwrap();
                    if message["cmd"] == "NTP" {
                        if let Some(send) = ntp_send.take() {
                            let _ = send.send(message);
                        }
                    } else if matches!(
                        message["cmd"].as_str(),
                        Some("MANUAL_FEEDING_SERVICE" | "OTA_UPGRADE" | "GET_CONFIG")
                    ) {
                        // Anything that acts on the feeder; replies and the
                        // read-only attribute query are expected.
                        worker_count.fetch_add(1, Ordering::SeqCst);
                    }
                }

                Ok(_) => {}
                Err(_) => break,
            }
        }
    });

    let ntp = tokio::time::timeout(Duration::from_secs(5), ntp_receive)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ntp["calibrationTag"], true);
    assert!(ntp["ts"].as_i64().unwrap() > 1_700_000_000_000);

    let mut online = false;
    for _ in 0..100 {
        let state = status(&config, &http).await;
        if state["device_online"] == true && state["attributes"]["volume"] == 50 {
            online = true;
            assert_eq!(state["attributes"]["softwareVersion"], "1.1.15");
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(online, "simulated feeder was not seen");

    let feed = http
        .post(format!("http://{}/api/feed", config.http_listen))
        .bearer_auth(&config.api_token)
        .json(&json!({"request_id":"6c8e4f8a-0d43-4c4b-9d5e-1f2a3b4c5d6e","portions":1}))
        .send()
        .await
        .unwrap();
    assert_eq!(feed.status(), reqwest::StatusCode::NOT_FOUND);

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(count.load(Ordering::SeqCst), 0, "the server sent a command");

    worker.abort();
    drop(device);
    drop(service);
}
