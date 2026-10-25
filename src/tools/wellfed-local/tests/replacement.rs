//! The replacement-firmware role end to end with a generic MQTT client:
//! health reaches the API and each role stays inside its own topics. Since
//! The server sends the firmware no requests: they go over the device
//! API, and the retired request route is gone.

mod common;

use std::time::Duration;

use common::{address, ready, start, status};
use wellfed_local::config::FirmwareConfig;
use wellfed_stock::storage;
use rumqttc::{
    AsyncClient, Event, EventLoop, LastWill, MqttOptions, Packet, QoS, SubscribeReasonCode,
};
use serde_json::{Value, json};

fn firmware_options(config: &wellfed_local::config::Config, password: &str) -> MqttOptions {
    let firmware = config.firmware.as_ref().unwrap();
    let mut options = MqttOptions::new(
        &firmware.client_id,
        config.mqtt_listen.ip().to_string(),
        config.mqtt_listen.port(),
    );
    options.set_credentials(&firmware.username, password);
    options.set_clean_session(true);
    options.set_keep_alive(Duration::from_secs(30));
    options
}

/// Poll until the next SUBACK and return its return codes.
async fn suback(events: &mut EventLoop) -> Vec<SubscribeReasonCode> {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), events.poll())
            .await
            .unwrap()
            .unwrap()
        {
            Event::Incoming(Packet::SubAck(ack)) => return ack.return_codes,
            _ => continue,
        }
    }
}

async fn connected(events: &mut EventLoop) {
    loop {
        if let Event::Incoming(Packet::ConnAck(_)) =
            tokio::time::timeout(Duration::from_secs(5), events.poll())
                .await
                .unwrap()
                .unwrap()
        {
            return;
        }
    }
}

async fn wait_for(
    config: &wellfed_local::config::Config,
    http: &reqwest::Client,
    check: impl Fn(&Value) -> bool,
) -> Value {
    for _ in 0..100 {
        let value = status(config, http).await;
        if check(&value["replacement"]) {
            return value["replacement"].clone();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("replacement status never matched");
}

fn health(seq: u64, online: bool) -> Vec<u8> {
    serde_json::to_vec(
        &json!({"schema":"wellfed.health","v":1,"online":online,"seq":seq,
        "uptime_ms":1000,"build":"0".repeat(64),"bank":1,"outputs":"stopped","faults":[]}),
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn firmware_role_reports_health_and_stays_in_its_namespace() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.json");
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

    let _service = start(&path);
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .no_proxy()
        .build()
        .unwrap();
    ready(&config, &http).await;

    let password = config.firmware.as_ref().unwrap().password.clone();
    let initial = status(&config, &http).await;
    assert_eq!(initial["replacement"]["health"], Value::Null);
    assert_eq!(initial["replacement"]["stale"], true);

    // A wrong password is refused.
    let (_wrong, mut wrong_loop) = AsyncClient::new(firmware_options(&config, "wrong"), 4);
    let refused = tokio::time::timeout(Duration::from_secs(3), wrong_loop.poll())
        .await
        .unwrap();
    assert!(
        refused.is_err(),
        "broker accepted a wrong firmware password"
    );

    let mut options = firmware_options(&config, &password);
    let will =
        serde_json::to_vec(&json!({"schema":"wellfed.health","v":1,"online":false,"seq":0}))
            .unwrap();
    options.set_last_will(LastWill::new(
        config.firmware_topic("health"),
        will,
        QoS::AtMostOnce,
        false,
    ));

    let (firmware, mut events) = AsyncClient::new(options, 16);
    connected(&mut events).await;
    firmware
        .subscribe(config.firmware_topic("request"), QoS::AtMostOnce)
        .await
        .unwrap();

    assert!(matches!(
        suback(&mut events).await[..],
        [SubscribeReasonCode::Success(_)]
    ));

    // Outside its single subscription, the firmware is refused.
    for topic in [
        config.firmware_topic("health"),
        config.firmware_topic("+"),
        config.topic("+/sub"),
        "wellfed/#".to_owned(),
    ] {
        firmware.subscribe(&topic, QoS::AtMostOnce).await.unwrap();
        assert!(
            matches!(
                suback(&mut events).await[..],
                [SubscribeReasonCode::Failure]
            ),
            "{topic}"
        );
    }

    // The pump keeps the firmware's session polled; nothing is expected on it.
    let (incoming_send, _incoming) = tokio::sync::mpsc::channel::<(String, Vec<u8>)>(8);
    let pump = tokio::spawn(async move {
        loop {
            match events.poll().await {
                Ok(Event::Incoming(Packet::Publish(publish))) => {
                    let _ = incoming_send
                        .send((publish.topic.clone(), publish.payload.to_vec()))
                        .await;
                }
                Ok(_) => {}
                Err(_) => return,
            }
        }
    });

    firmware
        .publish(
            config.firmware_topic("health"),
            QoS::AtMostOnce,
            false,
            health(1, true),
        )
        .await
        .unwrap();
    let seen = wait_for(&config, &http, |r| r["health"]["seq"] == 1).await;
    assert_eq!(seen["online"], true);
    assert_eq!(seen["stale"], false);
    assert_eq!(seen["health"]["outputs"], "stopped");

    // An invalid health message is counted and does not replace the last one.
    firmware
        .publish(
            config.firmware_topic("health"),
            QoS::AtMostOnce,
            false,
            serde_json::to_vec(&json!({"schema":"wellfed.health","v":2,"online":true,"seq":9}))
                .unwrap(),
        )
        .await
        .unwrap();
    let after = wait_for(&config, &http, |r| r["rejected"] == 1).await;
    assert_eq!(after["health"]["seq"], 1);

    // The stock device role cannot subscribe to or publish into the namespace.
    let mut device_options = MqttOptions::new(
        &config.device_client_id,
        config.mqtt_listen.ip().to_string(),
        config.mqtt_listen.port(),
    );
    device_options.set_credentials(&config.device_username, &config.device_password);
    device_options.set_clean_session(true);
    let (device, mut device_events) = AsyncClient::new(device_options, 8);
    connected(&mut device_events).await;

    device
        .subscribe(config.firmware_topic("request"), QoS::AtMostOnce)
        .await
        .unwrap();
    assert!(matches!(
        suback(&mut device_events).await[..],
        [SubscribeReasonCode::Failure]
    ));

    device
        .publish(
            config.firmware_topic("health"),
            QoS::AtMostOnce,
            false,
            health(77, true),
        )
        .await
        .unwrap();

    let _ = tokio::time::timeout(Duration::from_millis(500), device_events.poll()).await;

    // Firmware publishes outside its two topics, then health still shows seq 1:
    // neither the stock-topic event nor the device's health was delivered.
    firmware
        .publish(
            config.topic("event/post"),
            QoS::AtMostOnce,
            false,
            serde_json::to_vec(&json!({"cmd":"ATTR_PUSH_EVENT"})).unwrap(),
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let value = status(&config, &http).await;
    assert_eq!(value["replacement"]["health"]["seq"], 1);
    assert_eq!(
        value["last_seen"],
        Value::Null,
        "stock topic accepted from firmware"
    );

    // A dropped connection (no DISCONNECT) publishes the will: offline.
    pump.abort();
    drop(firmware);
    let offline = wait_for(&config, &http, |r| r["health"]["seq"] == 0).await;
    assert_eq!(offline["online"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_firmware_role_status_has_none_and_the_request_route_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.json");
    let mut config = common::config();
    config.mqtt_listen = address();
    config.http_listen = address();
    storage::write_json(&path, &config).unwrap();

    let _service = start(&path);
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .no_proxy()
        .build()
        .unwrap();
    ready(&config, &http).await;

    assert_eq!(status(&config, &http).await["replacement"], Value::Null);
    let retired = http
        .post(format!(
            "http://{}/api/replacement/request",
            config.http_listen
        ))
        .bearer_auth(&config.api_token)
        .header("content-type", "application/json")
        .body(r#"{"op":"ping"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(retired.status(), reqwest::StatusCode::NOT_FOUND);
}
