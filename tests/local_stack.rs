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
use uuid::Uuid;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_broker_api_and_simulated_feeder_complete_once_and_do_not_replay_after_restart() {
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
                    } else if message["cmd"] == "MANUAL_FEEDING_SERVICE" {
                        assert!(!publish.retain);
                        assert_eq!(publish.qos, QoS::AtMostOnce);
                        worker_count.fetch_add(1, Ordering::SeqCst);
                        let id = message["msgId"].clone();
                        for (suffix, message) in [
                            (
                                "service/post",
                                json!({"cmd":"MANUAL_FEEDING_SERVICE","msgId":id,"code":0}),
                            ),
                            (
                                "event/post",
                                json!({"cmd":"GRAIN_OUTPUT_EVENT","msgId":id,"type":2,"execStep":"GRAIN_START","expectGrainNum":1,"actualGrainNum":0,"finished":false}),
                            ),
                            (
                                "event/post",
                                json!({"cmd":"GRAIN_OUTPUT_EVENT","msgId":id,"type":2,"execStep":"GRAIN_END","expectGrainNum":1,"actualGrainNum":1,"finished":true}),
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
    let id = Uuid::new_v4();
    let input = json!({"request_id":id,"portions":1});
    let url = format!("http://{}/api/feed", config.http_listen);
    let response = http
        .post(&url)
        .bearer_auth(&config.api_token)
        .json(&input)
        .send()
        .await
        .unwrap();
    assert!(
        response.status().is_success(),
        "{}",
        response.text().await.unwrap()
    );
    let mut completed = false;
    for _ in 0..100 {
        let state = status(&config, &http).await;
        if state["feeds"][0]["phase"] == "completed" {
            completed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(completed, "simulated feed was not completed");
    let duplicate: Value = http
        .post(&url)
        .bearer_auth(&config.api_token)
        .json(&input)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(duplicate["phase"], "completed");
    assert_eq!(count.load(Ordering::SeqCst), 1);
    worker.abort();
    drop(device);
    drop(service);

    // Restart both actual Rust processes; repeating the old ID is read-only even
    // before any device reconnects, while a fresh ID is refused as offline.
    let _restarted = start(&path);
    ready(&config, &http).await;
    let old: Value = http
        .post(&url)
        .bearer_auth(&config.api_token)
        .json(&input)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(old["phase"], "completed");
    let fresh = http
        .post(&url)
        .bearer_auth(&config.api_token)
        .json(&json!({"request_id":Uuid::new_v4(),"portions":1}))
        .send()
        .await
        .unwrap();
    assert_eq!(fresh.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
}
