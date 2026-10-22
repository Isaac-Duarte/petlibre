//! FW08: the replacement firmware's own MQTT codec and session (petlibre-core)
//! against the server's real broker and API, over loopback TCP.

mod common;

use std::time::Duration;

use common::{address, ready, start, status};
use petlibre_core::transport::{self as mqtt, Buffer, Event, Health, Loss, Session, State};
use petlibre_local::{config::FirmwareConfig, storage};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

struct Firmware {
    stream: TcpStream,
    session: Session,
    rx: Vec<u8>,
    request_topic: String,
}

fn now_ms() -> u32 {
    use std::sync::OnceLock;
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis() as u32
}

impl Firmware {
    async fn send(&mut self, bytes: &[u8]) {
        self.stream.write_all(bytes).await.unwrap();
        self.session.sent(now_ms());
    }

    /// Read until the session yields a non-pong event.
    async fn next(&mut self) -> Event<'static> {
        loop {
            if let Some((_, used)) = mqtt::decode(&self.rx).unwrap() {
                // Leak the frame so the event can outlive this buffer (test only).
                let frame: &'static [u8] = Box::leak(self.rx[..used].to_vec().into_boxed_slice());
                self.rx.drain(..used);
                let (packet, _) = mqtt::decode(frame).unwrap().unwrap();
                let topic = self.request_topic.clone();
                match self.session.on_packet(now_ms(), packet, &topic) {
                    Event::Pong => continue,
                    event => return event,
                }
            }
            let mut buf = [0; 1024];
            let n = tokio::time::timeout(Duration::from_secs(5), self.stream.read(&mut buf))
                .await
                .expect("broker silent")
                .unwrap();
            assert!(n > 0, "broker closed the connection");
            self.rx.extend_from_slice(&buf[..n]);
        }
    }
}

fn health(online: bool, seq: u32) -> Vec<u8> {
    let snapshot = Health {
        online,
        seq,
        uptime_ms: 1000,
        build: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        bank: 1,
        reset_flags: 0,
        boot_count: 1,
        outputs: mqtt::Outputs::Stopped,
        faults: 0,
        rssi: -20,
        wifi_attempt: 1,
        link_losses: 0,
        dns_losses: 0,
        last_loss: Loss::None,
        connects: 1,
        last_error: None,
    };
    let mut buf = [0; mqtt::MAX_HEALTH];
    let mut out = Buffer::new(&mut buf);
    snapshot.write(&mut out).unwrap();
    out.as_bytes().to_vec()
}

async fn connect(
    config: &petlibre_local::config::Config,
    password: &str,
) -> (Firmware, Option<u8>) {
    let firmware = config.firmware.as_ref().unwrap();
    let mut stream = TcpStream::connect(config.mqtt_listen).await.unwrap();
    let will = health(false, 0);
    let health_topic = config.firmware_topic("health");
    let mut out = [0; mqtt::MAX_PACKET];
    let n = mqtt::encode_connect(
        &mut out,
        &mqtt::Connect {
            client_id: &firmware.client_id,
            username: &firmware.username,
            password: password.as_bytes(),
            will: Some(mqtt::Will {
                topic: &health_topic,
                payload: &will,
            }),
        },
    )
    .unwrap();
    stream.write_all(&out[..n]).await.unwrap();
    let mut fw = Firmware {
        stream,
        session: Session::connecting(now_ms()),
        rx: Vec::new(),
        request_topic: config.firmware_topic("request"),
    };
    let refused = match tokio::time::timeout(Duration::from_secs(3), fw.next()).await {
        Ok(Event::Connected) => None,
        Ok(Event::Closed(mqtt::Fault::Refused(code))) => Some(code),
        other => panic!("unexpected CONNACK outcome {other:?}"),
    };
    (fw, refused)
}

async fn wait_for(
    config: &petlibre_local::config::Config,
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn firmware_codec_reports_health_and_answers_through_the_real_broker() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.json");
    let mut config = common::config();
    config.mqtt_listen = address();
    config.http_listen = address();
    config.firmware = Some(FirmwareConfig {
        client_id: config.firmware_client_id(),
        username: "petlibre-fw".to_owned(),
        password: "0123456789abcdef".repeat(4),
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

    // The broker refuses a wrong password with a CONNACK code the session reports.
    let (_, refused) = connect(&config, "wrong").await;
    assert!(refused.is_some_and(|code| code != 0));

    let (mut fw, refused) = connect(&config, &password).await;
    assert_eq!(refused, None);
    let mut out = [0; mqtt::MAX_PACKET];
    let request_topic = config.firmware_topic("request");
    let n = fw
        .session
        .subscribe(now_ms(), &mut out, &request_topic)
        .unwrap();
    fw.send(&out[..n]).await;
    assert_eq!(fw.next().await, Event::Ready);
    assert_eq!(fw.session.state(), State::Ready);

    // Health, exactly as the firmware formats it, reaches the API.
    let body = health(true, 1);
    let n = mqtt::encode_publish(&mut out, &config.firmware_topic("health"), &body, false).unwrap();
    fw.send(&out[..n]).await;
    let replacement = wait_for(&config, &http, |r| r["health"]["seq"] == 1).await;
    assert_eq!(replacement["online"], true);
    assert_eq!(replacement["health"]["schema"], "petlibre.health");

    // A request from the API arrives on the request topic and parses.
    let response: Value = http
        .post(format!(
            "http://{}/api/replacement/request",
            config.http_listen
        ))
        .bearer_auth(&config.api_token)
        .header("content-type", "application/json")
        .body(r#"{"op":"ping"}"#)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = response["id"].as_str().unwrap().to_owned();
    let Event::Request(payload) = fw.next().await else {
        panic!("no request")
    };
    let request = mqtt::parse_request(payload).unwrap();
    assert_eq!(request.id, id);
    assert_eq!(request.op, Ok(mqtt::Op::Ping));
    let mut reply = [0; mqtt::MAX_REQUEST];
    let mut writer = Buffer::new(&mut reply);
    mqtt::write_reply(&mut writer, &request, 4321).unwrap();
    let len = writer.len();
    let n = mqtt::encode_publish(
        &mut out,
        &config.firmware_topic("reply"),
        &reply[..len],
        false,
    )
    .unwrap();
    fw.send(&out[..n]).await;
    let replacement = wait_for(&config, &http, |r| {
        r["replies"].as_array().is_some_and(|a| !a.is_empty())
    })
    .await;
    let message = &replacement["replies"][0]["message"];
    assert_eq!(message["id"], id.as_str());
    assert_eq!(message["ok"], true);
    assert_eq!(message["uptime_ms"], 4321);

    // Keepalive: PINGREQ gets PINGRESP.
    let n = mqtt::encode_pingreq(&mut out).unwrap();
    fw.send(&out[..n]).await;

    // An abrupt drop publishes the firmware's own will: the server sees offline.
    drop(fw);
    let replacement = wait_for(&config, &http, |r| {
        r["online"] == false && r["health"]["online"] == false
    })
    .await;
    assert_eq!(replacement["health"]["seq"], 0);
}
