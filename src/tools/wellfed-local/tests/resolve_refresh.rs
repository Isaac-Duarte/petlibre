//! A refused trial resolution refreshes stock's version report. Idle stock
//! 1.1.15 sends only heartbeats, which carry no version, so without a fresh
//! report the resolution evidence could never be complete (physical,
//! fw40-stock-ota-04). The server then asks with the same read-only attribute
//! query it sends on connect; it never sends an OTA command.

mod common;

use chrono::{Duration as Age, Utc};
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use uuid::Uuid;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_resolution_asks_stock_for_its_attributes_and_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.json");
    let mut config = common::config();
    config.mqtt_listen = common::address();
    config.http_listen = common::address();
    wellfed_stock::storage::write_json(&path, &config).unwrap();

    // An earlier request that stock reported failing, long enough ago.
    let request_id = Uuid::new_v4();
    let sent = Utc::now() - Age::minutes(30);
    let ledger = json!({
        "device_id": config.device_id,
        "ota_trials": {
            request_id.to_string(): {
                "sequence": 1,
                "input": {"request_id": request_id, "server_token": Uuid::new_v4()},
                "created_at": sent, "updated_at": sent,
                "phase": "reported_error", "download_reported": true,
                "resolution_note": null, "image_sha256": "ab".repeat(32),
            }
        },
    });
    wellfed_stock::storage::write_json(
        &dir.path().join(wellfed_stock::controller::LEDGER_FILE),
        &ledger,
    )
    .unwrap();

    let _service = common::start_mode(&path, true);
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    common::ready(&config, &http).await;

    // A stock device that heartbeats and never reports its version.
    let mut options = MqttOptions::new(
        &config.device_client_id,
        config.mqtt_listen.ip().to_string(),
        config.mqtt_listen.port(),
    );
    options.set_credentials(&config.device_username, &config.device_password);
    options.set_clean_session(true);
    let (client, mut events) = AsyncClient::new(options, 32);

    let queries = Arc::new(AtomicUsize::new(0));
    let others = Arc::new(AtomicUsize::new(0));
    let (seen_queries, seen_others) = (Arc::clone(&queries), Arc::clone(&others));
    let device = config.clone();
    let worker = tokio::spawn(async move {
        loop {
            match events.poll().await.unwrap() {
                Event::Incoming(Packet::ConnAck(_)) => client
                    .subscribe(device.topic("+/sub"), QoS::AtLeastOnce)
                    .await
                    .unwrap(),
                Event::Incoming(Packet::SubAck(_)) => client
                    .publish(
                        device.topic("heart/post"),
                        QoS::AtMostOnce,
                        false,
                        json!({"cmd":"HEARTBEAT","count":1}).to_string(),
                    )
                    .await
                    .unwrap(),
                Event::Incoming(Packet::Publish(publish)) => {
                    let message: Value = serde_json::from_slice(&publish.payload).unwrap();
                    match message["cmd"].as_str() {
                        Some("ATTR_GET_SERVICE") => seen_queries.fetch_add(1, Ordering::SeqCst),
                        // Replies to the device's own posts are not commands.
                        Some("HEARTBEAT") => 0,
                        _ => seen_others.fetch_add(1, Ordering::SeqCst),
                    };
                }
                _ => {}
            }
        }
    });

    // The connect-time query, prompted by the first message.
    for _ in 0..100 {
        if queries.load(Ordering::SeqCst) == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(queries.load(Ordering::SeqCst), 1);

    let refused = http
        .post(format!(
            "http://{}/api/ota/trial/resolve",
            config.http_listen
        ))
        .bearer_auth(&config.api_token)
        .json(&json!({"request_id": request_id, "note": "stock still reports 1.1.15"}))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);

    for _ in 0..100 {
        if queries.load(Ordering::SeqCst) == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(queries.load(Ordering::SeqCst), 2, "the refusal asks once");
    assert_eq!(others.load(Ordering::SeqCst), 0, "and sends nothing else");

    let status = common::status(&config, &http).await;
    assert_eq!(status["ota_trials"][0]["phase"], "reported_error");
    worker.abort();
}
