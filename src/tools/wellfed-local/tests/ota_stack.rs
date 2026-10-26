//! End-to-end install-trial test against the real server, a simulated stock
//! device over MQTT and a placeholder image-server lease. The install must be
//! published exactly once, a repeated request ID must not resend it, no
//! second trial may start while it is open, and the replacement's
//! device-API answer closes it without another command.

mod common;

use chrono::Utc;
use wellfed_stock::{ota, storage};
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use serde_json::{Value, json};
use std::net::Ipv4Addr;
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use uuid::Uuid;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_explicit_api_install_publishes_once_and_never_again() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.json");
    let mut config = common::config();

    // A distinct loopback host lets this test own the fixed image-server port.
    let host = Ipv4Addr::new(127, 0, 0, 2 + (std::process::id() % 100) as u8);

    let mqtt = std::net::TcpListener::bind((host, 0)).unwrap();
    config.mqtt_listen = mqtt.local_addr().unwrap();
    drop(mqtt);
    config.http_listen = common::address();
    storage::write_json(&path, &config).unwrap();

    let token = Uuid::new_v4();
    let digest = "ab".repeat(32);
    let image_server = std::net::TcpListener::bind((host, ota::image::PORT)).unwrap();
    storage::write_json(
        &ota::image::Lease::path(&path),
        &ota::image::Lease {
            token,
            peer: Ipv4Addr::new(10, 0, 0, 2),
            expires_at: Utc::now() + chrono::Duration::minutes(15),
            sha256: digest.clone(),
        },
    )
    .unwrap();

    let service = common::start_mode(&path, true);
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    common::ready(&config, &http).await;

    let mut options = MqttOptions::new(
        &config.device_client_id,
        config.mqtt_listen.ip().to_string(),
        config.mqtt_listen.port(),
    );
    options.set_credentials(&config.device_username, &config.device_password);
    options.set_clean_session(true);
    let (client, mut events) = AsyncClient::new(options, 32);

    let commands = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&commands);

    // The simulated stock device reports a completed download for each
    // OTA_UPGRADE, as stock does before it activates the new bank.
    let device = config.clone();
    let served = digest.clone();
    let worker = tokio::spawn(async move {
        loop {
            match events.poll().await.unwrap() {
                Event::Incoming(Packet::ConnAck(_)) => client
                    .subscribe(device.topic("+/sub"), QoS::AtLeastOnce)
                    .await
                    .unwrap(),

                Event::Incoming(Packet::SubAck(_)) => {
                    client.publish(device.topic("event/post"),QoS::AtMostOnce,false,
                        json!({"cmd":"DEVICE_START_EVENT","ts":Utc::now().timestamp_millis(),"softwareVersion":"1.1.15","powerType":1}).to_string()).await.unwrap();
                }

                Event::Incoming(Packet::Publish(publish)) => {
                    let message: Value = serde_json::from_slice(&publish.payload).unwrap();
                    assert_ne!(
                        message["cmd"], "MANUAL_FEEDING_SERVICE",
                        "the server sent a feed"
                    );
                    if message["cmd"] == "OTA_UPGRADE" {
                        assert_eq!(publish.qos, QoS::AtMostOnce);
                        assert!(!publish.retain);
                        assert_eq!(message["upgradeType"], 1);
                        assert_eq!(message["targetPid"], "PLAF103");
                        assert_eq!(message["targetSoftwareVersion"], "2.0.0");
                        assert_eq!(message["sha256"], served.as_str());
                        assert!(
                            message["url"]
                                .as_str()
                                .unwrap()
                                .ends_with("/replacement.bin")
                        );
                        observed.fetch_add(1, Ordering::SeqCst);

                        client.publish(device.topic("ota/post"),QoS::AtMostOnce,false,
                            json!({"cmd":"OTA_PROGRESS","msgId":message["msgId"],"progress":100}).to_string()).await.unwrap();
                    }
                }
                _ => {}
            }
        }
    });

    for _ in 0..100 {
        if common::status(&config, &http).await["device_online"] == true {
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }

    // Refusals send nothing: a token that does not match the lease, a missing
    // bearer token, and an unknown `url` field.
    let endpoint = format!("http://{}/api/ota/trial", config.http_listen);
    let stale = ota::trial::Input {
        request_id: Uuid::new_v4(),
        server_token: Uuid::new_v4(),
    };
    assert_eq!(
        http.post(&endpoint)
            .bearer_auth(&config.api_token)
            .json(&stale)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::PRECONDITION_FAILED
    );

    let request = ota::trial::Input {
        request_id: Uuid::new_v4(),
        server_token: token,
    };
    assert_eq!(
        http.post(&endpoint)
            .json(&request)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::UNAUTHORIZED
    );

    let mut arbitrary = serde_json::to_value(&request).unwrap();
    arbitrary["url"] = json!("http://external.invalid/other.bin");
    assert_eq!(
        http.post(&endpoint)
            .bearer_auth(&config.api_token)
            .json(&arbitrary)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::UNPROCESSABLE_ENTITY
    );

    assert_eq!(commands.load(Ordering::SeqCst), 0);

    let response = http
        .post(&endpoint)
        .bearer_auth(&config.api_token)
        .json(&request)
        .send()
        .await
        .unwrap();
    assert!(
        response.status().is_success(),
        "{}",
        response.text().await.unwrap()
    );

    let mut downloaded = false;
    for _ in 0..100 {
        let status = common::status(&config, &http).await;
        if status["ota_trials"][0]["download_reported"] == true {
            downloaded = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    assert!(downloaded);
    assert_eq!(commands.load(Ordering::SeqCst), 1);

    // The same request ID again returns the stored trial without a new
    // command, and a new request is refused while this one is open.
    assert!(
        http.post(&endpoint)
            .bearer_auth(&config.api_token)
            .json(&request)
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    let second = ota::trial::Input {
        request_id: Uuid::new_v4(),
        server_token: token,
    };
    assert_eq!(
        http.post(&endpoint)
            .bearer_auth(&config.api_token)
            .json(&second)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::CONFLICT
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(commands.load(Ordering::SeqCst), 1);

    let status = common::status(&config, &http).await;
    assert!(
        status["ota_reports"]
            .as_array()
            .unwrap()
            .iter()
            .any(|report| {
                report["message_id"] == request.request_id.to_string()
                    && report["command"] == "OTA_PROGRESS"
                    && report["progress"] == 100
            })
    );
    assert_eq!(status["ota_trials"].as_array().unwrap().len(), 1);

    // The bench saw the build answer the device API: the trial closes, and
    // stock is sent nothing more.
    let answered = http
        .post(format!(
            "http://{}/api/ota/trial/answered",
            config.http_listen
        ))
        .bearer_auth(&config.api_token)
        .json(&ota::trial::ResolveInput {
            request_id: request.request_id,
            note: "build answered the device API from bank 0".into(),
        })
        .send()
        .await
        .unwrap();
    assert!(answered.status().is_success());
    let status = common::status(&config, &http).await;
    assert_eq!(status["ota_trials"][0]["phase"], "replacement_healthy");
    assert_eq!(commands.load(Ordering::SeqCst), 1);

    worker.abort();
    drop(service);
    drop(image_server);
}

#[test]
fn trial_mode_refuses_a_build_without_a_clean_source_commit() {
    if wellfed_local::build_info::BUILD.reviewable() {
        return; // Built from a clean commit: the guard correctly permits it.
    }

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_wellfed-local"))
        .args([
            "serve",
            "--config",
            "/nonexistent/server.json",
            "--enable-ota-trial",
        ])
        .env_remove(wellfed_local::build_info::ALLOW_UNREVIEWED_ENV)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("clean commit"));
}

#[tokio::test]
async fn ordinary_server_refuses_trial_publication_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.json");
    let mut config = common::config();
    config.mqtt_listen = common::address();
    config.http_listen = common::address();
    storage::write_json(&path, &config).unwrap();

    let service = common::start(&path);
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    common::ready(&config, &http).await;

    let response = http
        .post(format!("http://{}/api/ota/trial", config.http_listen))
        .bearer_auth(&config.api_token)
        .json(&ota::trial::Input {
            request_id: Uuid::new_v4(),
            server_token: Uuid::new_v4(),
        })
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
    let status = common::status(&config, &http).await;
    assert_eq!(status["ota_trial_enabled"], false);
    assert!(status["ota_trials"].as_array().unwrap().is_empty());
    drop(service);
}
