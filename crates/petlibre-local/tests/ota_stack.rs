//! End-to-end OTA trial tests against the real server, a simulated stock
//! device over MQTT and a placeholder image-server lease. Each trial or probe
//! query must be published exactly once, a repeated request ID must not resend
//! it, and the trial must hold manual feeding until the original version runs.

mod common;

use chrono::Utc;
use petlibre_local::{
    ota_image,
    ota_trial::{self, Image},
    storage,
};
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
async fn explicit_api_trial_and_restore_publish_once_and_hold_manual_feeding() {
    run_cycle(ota_image::Profile::StockVersion).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn c_probe_requires_explicit_queries_and_restores_without_replaying_any_command() {
    run_cycle(ota_image::Profile::CExecution).await;
}

async fn run_cycle(profile: ota_image::Profile) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.json");
    let mut config = common::config();

    // A distinct loopback host lets this test own the fixed image-server port.
    let host = Ipv4Addr::new(
        127,
        0,
        0,
        2 + (std::process::id() % 100) as u8
            + if profile == ota_image::Profile::CExecution {
                100
            } else {
                0
            },
    );

    let mqtt = std::net::TcpListener::bind((host, 0)).unwrap();
    config.mqtt_listen = mqtt.local_addr().unwrap();
    drop(mqtt);
    config.http_listen = common::address();
    storage::write_json(&path, &config).unwrap();

    let token = Uuid::new_v4();
    let image_server = std::net::TcpListener::bind((host, ota_image::PORT)).unwrap();
    storage::write_json(
        &ota_image::Lease::path(&path),
        &ota_image::Lease {
            token,
            peer: Ipv4Addr::new(10, 0, 0, 2),
            expires_at: Utc::now() + chrono::Duration::minutes(15),
            profile,
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
    let queries = Arc::new(AtomicUsize::new(0));
    let observed_queries = Arc::clone(&queries);

    // The simulated stock device acknowledges each OTA_UPGRADE as a completed
    // download and restarts into the target version; it answers GET_CONFIG with a
    // fresh reply ID, as the stock firmware does.
    let device = config.clone();
    let worker = tokio::spawn(async move {
        let mut version = "1.1.15".to_owned();
        loop {
            match events.poll().await.unwrap() {
                Event::Incoming(Packet::ConnAck(_)) => client
                    .subscribe(device.topic("+/sub"), QoS::AtLeastOnce)
                    .await
                    .unwrap(),

                Event::Incoming(Packet::SubAck(_)) => {
                    client.publish(device.topic("event/post"),QoS::AtMostOnce,false,
                        json!({"cmd":"DEVICE_START_EVENT","ts":Utc::now().timestamp_millis(),"softwareVersion":version,"powerType":1}).to_string()).await.unwrap();
                }

                Event::Incoming(Packet::Publish(publish)) => {
                    let message: Value = serde_json::from_slice(&publish.payload).unwrap();
                    if message["cmd"] == "OTA_UPGRADE" {
                        assert_eq!(publish.qos, QoS::AtMostOnce);
                        assert!(!publish.retain);
                        assert_eq!(message["upgradeType"], 1);
                        assert_eq!(message["targetPid"], "PLAF103");
                        observed.fetch_add(1, Ordering::SeqCst);

                        let target = message["targetSoftwareVersion"].as_str().unwrap();
                        let (file, digest) = if target == "1.1.16" {
                            ("test.bin", petlibre_local::ota_image::TEST_SHA256)
                        } else if target == "1.1.17" {
                            ("probe.bin", petlibre_local::ota_image::C_PROBE_SHA256)
                        } else {
                            assert_eq!(target, "1.1.15");
                            ("original.bin", petlibre_local::ota_image::ORIGINAL_SHA256)
                        };

                        assert_eq!(message["sha256"], digest);
                        assert!(message["url"].as_str().unwrap().ends_with(file));

                        client.publish(device.topic("ota/post"),QoS::AtMostOnce,false,
                            json!({"cmd":"OTA_PROGRESS","msgId":message["msgId"],"progress":100}).to_string()).await.unwrap();
                        version = target.to_owned();
                        client.publish(device.topic("event/post"),QoS::AtMostOnce,false,
                            json!({"cmd":"DEVICE_START_EVENT","ts":Utc::now().timestamp_millis(),"softwareVersion":version,"powerType":1}).to_string()).await.unwrap();
                    } else if message["cmd"] == "GET_CONFIG" {
                        assert_eq!(publish.topic, device.topic("config/sub"));
                        observed_queries.fetch_add(1, Ordering::SeqCst);
                        assert_eq!(publish.qos, QoS::AtMostOnce);
                        assert!(!publish.retain);

                        let id = Uuid::new_v4().simple().to_string();
                        assert_ne!(message["msgId"], id);
                        let mut reply = json!({"cmd":"GET_CONFIG","msgId":id,
                            "ts":Utc::now().timestamp_millis(), "softwareVersion":version,"powerType":1});
                        if version == "1.1.17" {
                            reply["localReProbe"] =
                                json!(petlibre_local::probe::expected(&id).unwrap());
                        }

                        client
                            .publish(
                                device.topic("config/post"),
                                QoS::AtMostOnce,
                                false,
                                reply.to_string(),
                            )
                            .await
                            .unwrap();
                    } else {
                        assert_ne!(
                            message["cmd"], "MANUAL_FEEDING_SERVICE",
                            "OTA experiment emitted a motor command"
                        );
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
    let stale = ota_trial::Input {
        request_id: Uuid::new_v4(),
        image: Image::Test,
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

    let denied = ota_trial::Input {
        request_id: Uuid::new_v4(),
        image: Image::Test,
        server_token: token,
    };
    assert_eq!(
        http.post(&endpoint)
            .json(&denied)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::UNAUTHORIZED
    );

    let mut arbitrary = serde_json::to_value(&denied).unwrap();
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

    let pair = match profile {
        ota_image::Profile::StockVersion => [Image::Test, Image::Original],
        ota_image::Profile::CExecution => [Image::CProbe, Image::CProbeOriginal],
    };
    for (index, image) in pair.into_iter().enumerate() {
        let request = ota_trial::Input {
            request_id: Uuid::new_v4(),
            image,
            server_token: token,
        };

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

        let mut completed = false;
        for _ in 0..100 {
            let status = common::status(&config, &http).await;
            if status["ota_trials"][0]["phase"] == "version_reported" {
                completed = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }

        assert!(completed);
        assert_eq!(commands.load(Ordering::SeqCst), index + 1);

        // The same request ID again returns the stored trial without a new command.
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
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(commands.load(Ordering::SeqCst), index + 1);

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
        assert_eq!(status["ota_hold"], index == 0);
        assert_eq!(status["ota_trials"].as_array().unwrap().len(), index + 1);

        if index == 0 {
            let feed = http
                .post(format!("http://{}/api/feed", config.http_listen))
                .bearer_auth(&config.api_token)
                .json(&json!({"request_id":Uuid::new_v4(),"portions":1}))
                .send()
                .await
                .unwrap();
            assert_eq!(feed.status(), reqwest::StatusCode::CONFLICT);
        }

        if image == Image::CProbe {
            assert_eq!(
                queries.load(Ordering::SeqCst),
                0,
                "configuration hook ran automatically"
            );

            let query_endpoint = format!("http://{}/api/probe/query", config.http_listen);
            for count in 1..=2 {
                let query = json!({"request_id":Uuid::new_v4()});
                assert_eq!(
                    http.post(&query_endpoint)
                        .json(&query)
                        .send()
                        .await
                        .unwrap()
                        .status(),
                    reqwest::StatusCode::UNAUTHORIZED
                );

                let response = http
                    .post(&query_endpoint)
                    .bearer_auth(&config.api_token)
                    .json(&query)
                    .send()
                    .await
                    .unwrap();
                assert!(
                    response.status().is_success(),
                    "{}",
                    response.text().await.unwrap()
                );

                let mut confirmed = false;
                for _ in 0..100 {
                    let status = common::status(&config, &http).await;
                    let reports = status["probe_queries"].as_array().unwrap();
                    if reports.len() == count
                        && reports.iter().all(|q| q["phase"] == "proof_reported")
                    {
                        confirmed = true;
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }

                assert!(confirmed, "computed diagnostic was not observed");

                // The same query ID again sends no second query.
                assert!(
                    http.post(&query_endpoint)
                        .bearer_auth(&config.api_token)
                        .json(&query)
                        .send()
                        .await
                        .unwrap()
                        .status()
                        .is_success()
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
                assert_eq!(queries.load(Ordering::SeqCst), count);
            }
        }
    }

    let status = common::status(&config, &http).await;
    assert_eq!(status["ota_trials"].as_array().unwrap().len(), 2);
    assert_eq!(
        queries.load(Ordering::SeqCst),
        if profile == ota_image::Profile::CExecution {
            2
        } else {
            0
        }
    );

    worker.abort();
    drop(service);
    drop(image_server);
}

#[test]
fn trial_mode_refuses_a_build_without_a_clean_source_commit() {
    if petlibre_local::build_info::BUILD.reviewable() {
        return; // Built from a clean commit: the guard correctly permits it.
    }

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_petlibre-local"))
        .args([
            "serve",
            "--config",
            "/nonexistent/server.json",
            "--enable-ota-trial",
        ])
        .env_remove(petlibre_local::build_info::ALLOW_UNREVIEWED_ENV)
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
        .json(&ota_trial::Input {
            request_id: Uuid::new_v4(),
            image: Image::Test,
            server_token: Uuid::new_v4(),
        })
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
    let status = common::status(&config, &http).await;
    assert_eq!(status["ota_trial_enabled"], false);
    assert_eq!(status["ota_hold"], false);
    assert!(status["ota_trials"].as_array().unwrap().is_empty());
    drop(service);
}
