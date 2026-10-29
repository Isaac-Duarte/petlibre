//! An installer's server end to end: the real `serve --enroll-minutes`
//! with no device in its config, and a simulated stock feeder whose
//! credentials it has never seen. The first feeder enrolls on first use,
//! the server then talks to it as to a captured one (its version report
//! reaches the status document), and a second identity is refused. A later
//! `serve` without the window answers the enrolled feeder alone.

mod common;

use std::{path::Path, time::Duration};

use chrono::Utc;
use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, QoS};
use serde_json::json;
use wellfed_stock::{config, enroll::Enrollment};

/// A stock-shaped identity no file mentions.
const DEVICE: &str = "AF_FRESH_FEEDER";

fn stock(
    listen: std::net::SocketAddr,
    client_id: &str,
    password: &str,
) -> (AsyncClient, EventLoop) {
    let mut options = MqttOptions::new(client_id, listen.ip().to_string(), listen.port());
    options.set_credentials("stock-user", password);
    options.set_clean_session(true);
    AsyncClient::new(options, 32)
}

/// Whether the broker admits `client_id` within 5 s.
async fn admitted(events: &mut EventLoop) -> bool {
    let poll = async {
        loop {
            match events.poll().await {
                Ok(Event::Incoming(Packet::ConnAck(_))) => return true,
                Ok(_) => {}
                Err(_) => return false,
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(5), poll)
        .await
        .unwrap_or(false)
}

fn start(path: &Path, minutes: Option<u64>) -> common::Service {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_wellfed-stock"));
    command.args(["serve", "--config"]).arg(path);
    if let Some(minutes) = minutes {
        command.args(["--enroll-minutes", &minutes.to_string()]);
    }
    common::Service::spawn(command)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_first_stock_feeder_enrolls_and_is_then_the_only_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.json");
    let [listen, http] = common::addresses().unwrap();
    let init = std::process::Command::new(env!("CARGO_BIN_EXE_wellfed-stock"))
        .args([
            "init-installer",
            "--mqtt-listen",
            &listen.to_string(),
            "--output",
        ])
        .arg(&path)
        .output()
        .unwrap();
    assert!(init.status.success(), "{:?}", init.stderr);
    assert!(config::load(&path).is_err(), "no device before enrollment");

    // The installer's config names its HTTP API on the default port; move it.
    let mut file: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    file["http_listen"] = json!(http.to_string());
    wellfed_stock::storage::write_json(&path, &file).unwrap();

    // Without a window, an installer's server refuses to start.
    let refused = std::process::Command::new(env!("CARGO_BIN_EXE_wellfed-stock"))
        .args(["serve", "--config"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(!refused.status.success());

    let mut service = start(&path, Some(1));
    // The broker comes up first.
    service.listening(listen).await;
    let (client, mut events) = stock(listen, DEVICE, "its-own-password");
    assert!(admitted(&mut events).await, "the first feeder is trusted");

    let record = Enrollment::load(&Enrollment::path(&path)).unwrap().unwrap();
    assert_eq!(record.client_id, DEVICE);
    assert_eq!(record.password, "its-own-password");
    assert!(record.address.is_loopback());

    // The server then serves it as its device: a version report arrives.
    let config = config::load(&path).unwrap();
    assert_eq!(config.device_id, DEVICE);
    // The server's own session starts once it has seen the enrollment, so
    // the feeder reports until the server has heard it.
    let session = tokio::spawn(async move { while events.poll().await.is_ok() {} });
    let reporter = tokio::spawn(async move {
        loop {
            let _ = client
                .publish(
                    format!("dl/PLAF103/{DEVICE}/device/event/post"),
                    QoS::AtMostOnce,
                    false,
                    json!({"cmd":"DEVICE_START_EVENT","ts":Utc::now().timestamp_millis(),
                           "softwareVersion":"1.1.15","powerType":1})
                    .to_string(),
                )
                .await;
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    });
    let http_client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    let mut online = false;
    for _ in 0..200 {
        if let Ok(response) = http_client
            .get(format!("http://{http}/api/status"))
            .bearer_auth(&config.api_token)
            .send()
            .await
            && let Ok(status) = response.json::<serde_json::Value>().await
            && status["attributes"]["softwareVersion"] == "1.1.15"
        {
            online = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(online, "the enrolled feeder's report reached the server");
    let status = |http_client: reqwest::Client, token: String| async move {
        http_client
            .get(format!("http://{http}/api/status"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .json::<serde_json::Value>()
            .await
            .unwrap()
    };
    let shown = status(http_client.clone(), config.api_token.clone()).await;
    assert_eq!(shown["enrollment"]["device"], DEVICE);
    assert_eq!(shown["enrollment"]["contested"], false);
    assert!(shown["enrollment"].get("password").is_none());

    // A second feeder, or a guess at the first's password, is refused, and
    // inside the window it contests the enrollment: no install is sent.
    let (_other, mut other_events) = stock(listen, "AF_SECOND_FEEDER", "its-own-password");
    assert!(!admitted(&mut other_events).await);
    let (_guess, mut guess_events) = stock(listen, DEVICE, "guess");
    assert!(!admitted(&mut guess_events).await);
    let shown = status(http_client.clone(), config.api_token.clone()).await;
    assert_eq!(shown["enrollment"]["contested"], true);
    let refused = http_client
        .post(format!("http://{http}/api/ota/trial"))
        .bearer_auth(&config.api_token)
        .json(&json!({"request_id": uuid::Uuid::new_v4(), "server_token": uuid::Uuid::new_v4()}))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), reqwest::StatusCode::CONFLICT);

    reporter.abort();
    session.abort();
    drop(service);

    // Restarted with no window, the enrolled feeder is still the device.
    let mut restarted = start(&path, None);
    restarted.listening(listen).await;
    let (_again, mut again_events) = stock(listen, DEVICE, "its-own-password");
    assert!(admitted(&mut again_events).await);
    let (_late, mut late_events) = stock(listen, "AF_LATE_FEEDER", "x");
    assert!(!admitted(&mut late_events).await);

    // A record that no longer parses fails the status with a fixed answer:
    // a parse error could quote the password it holds.
    std::fs::write(
        Enrollment::path(&path),
        r#"{"client_id":"AF_FRESH_FEEDER","username":"stock-user","password":["its-own-password"]}"#,
    )
    .unwrap();
    let response = http_client
        .get(format!("http://{http}/api/status"))
        .bearer_auth(&config.api_token)
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        reqwest::StatusCode::INTERNAL_SERVER_ERROR
    );
    let body = response.text().await.unwrap();
    assert!(!body.contains("its-own-password"));
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        body,
        json!({"error": wellfed_stock::api::ENROLLMENT_UNREADABLE})
    );
}
