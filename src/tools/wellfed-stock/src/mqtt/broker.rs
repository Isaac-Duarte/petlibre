//! The embedded MQTT broker, which `serve` runs in its own process beside
//! the server's session ([`crate::server`]); the `broker` subcommand runs it
//! alone.
//!
//! The broker itself listens only on an ephemeral loopback port. The
//! configured feeder-facing address is served by `compat`, which normalizes
//! the stock firmware's first CONNECT and relays it. `DeviceAuth` admits
//! only stock's two fixed identities (the stock device and the server),
//! each to only an allowlist of stock topics. Retained messages, persistent
//! sessions and offline queues are disabled, so no command can wait for a
//! later connection.
//!
//! An installer's broker starts with no device and an enrollment window
//! ([`crate::enroll`]): the first stock-shaped CONNECT inside it is
//! recorded and becomes the device; every other identity is refused as
//! before. The server's own identity is derived from the device's ID, so
//! the server logs in only once a device is known.
//!
//! That ordering loses stock's connect burst unless the broker waits: stock
//! publishes its clock request, its start event and its version report as
//! soon as its CONNACK arrives, and no retained message or queue keeps them
//! for a subscriber that comes later (physical, October 30: the server
//! subscribed 0.7 s after stock connected, and the version report was lost).
//! So a [`Handoff`] holds the enrolling feeder's CONNACK until the server,
//! told of the enrollment at once, has subscribed.

use std::{
    future::Future,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use chrono::Utc;
use mqtt5::{
    broker::{
        BrokerConfig, MqttBroker,
        auth::{AuthProvider, AuthResult},
        config::{StorageBackend, StorageConfig},
    },
    packet::connect::ConnectPacket,
    protocol::v5::reason_codes::ReasonCode,
};
use subtle::ConstantTimeEq;
use tokio::sync::watch;
use tracing::{info, warn};

use crate::{
    config::{self, Config, InstallConfig},
    enroll::{self, Contest, Enrollment},
    mqtt::compat::{self, Peers},
};

/// The one device the broker admits.
struct Device {
    id: String,
    client_id: String,
    username: String,
    password: String,
}

/// Where a first-use identity is recorded, and until when one may be.
struct Window {
    record: PathBuf,
    /// Where a second identity inside the window is noted.
    contested: PathBuf,
    until: Instant,
}

/// How long an enrolling feeder's CONNACK waits for the server to
/// subscribe. On loopback the server takes milliseconds; the bound only
/// keeps a server that never comes from failing the feeder's connect, which
/// the broker gives up on after 10 s. Past it the feeder is admitted
/// anyway, and its burst may be lost as before.
const HANDOFF_WAIT: Duration = Duration::from_secs(3);

/// An enrollment handed to a server in the same process: `enrolled` is set
/// as the feeder is recorded, and its CONNACK waits until `listening` is.
pub struct Handoff {
    pub enrolled: watch::Sender<bool>,
    pub listening: watch::Receiver<bool>,
}

/// Who may log in: the device (once known) and the server.
pub struct DeviceAuth {
    server_password: String,
    device: Mutex<Option<Device>>,
    window: Option<Window>,
    /// The feeder's own address behind each relayed connection.
    peers: Peers,
    handoff: Option<Handoff>,
}

impl DeviceAuth {
    /// A broker for `config`'s device.
    pub fn device(config: &Config) -> Self {
        Self {
            server_password: config.server_password.clone(),
            device: Mutex::new(Some(Device {
                id: config.device_id.clone(),
                client_id: config.device_client_id.clone(),
                username: config.device_username.clone(),
                password: config.device_password.clone(),
            })),
            window: None,
            peers: Peers::default(),
            handoff: None,
        }
    }

    /// An installer's broker: no device until the first stock feeder
    /// connects within `window`, which is then recorded at `record`.
    pub fn enrolling(config: &InstallConfig, config_path: &Path, window: Duration) -> Self {
        Self {
            server_password: config.server_password.clone(),
            device: Mutex::new(None),
            window: Some(Window {
                record: Enrollment::path(config_path),
                contested: Enrollment::contested_path(config_path),
                until: Instant::now() + window,
            }),
            peers: Peers::default(),
            handoff: None,
        }
    }

    /// Hand the enrollment to a server in this process ([`Handoff`]).
    #[must_use]
    pub fn with_handoff(mut self, handoff: Handoff) -> Self {
        self.handoff = Some(handoff);
        self
    }

    /// The enrolled device's ID, once there is one.
    fn device_id(&self) -> Option<String> {
        let device = self.device.lock().unwrap_or_else(PoisonError::into_inner);
        device.as_ref().map(|device| device.id.clone())
    }

    /// The user a login is admitted as: one of stock's two identities.
    /// `peer` is where the feeder connected from, when the relay knows.
    fn login(
        &self,
        client_id: &str,
        username: &str,
        password: &[u8],
        peer: Option<IpAddr>,
    ) -> Option<&'static str> {
        let mut device = self.device.lock().unwrap_or_else(PoisonError::into_inner);

        let Some(known) = device.as_ref() else {
            let enrolled = self.enroll(client_id, username, password, peer)?;
            *device = Some(enrolled);
            return Some("device");
        };
        let is_device = client_id == known.client_id
            && username == known.username
            && bool::from(password.ct_eq(known.password.as_bytes()));
        let is_server = client_id == config::server_client_id(&known.id)
            && username == "local-server"
            && bool::from(password.ct_eq(self.server_password.as_bytes()));

        if is_device {
            Some("device")
        } else if is_server {
            Some("server")
        } else {
            self.contest(client_id, username, password, peer);
            None
        }
    }

    /// A stock-shaped identity other than the enrolled one, inside the
    /// window: the enrollment can no longer be trusted, so it is marked
    /// contested and the server refuses to install.
    fn contest(&self, client_id: &str, username: &str, password: &[u8], peer: Option<IpAddr>) {
        let Some(window) = &self.window else {
            return;
        };
        if Instant::now() >= window.until || !enroll::plausible(client_id, username, password) {
            return;
        }
        warn!(
            device = client_id,
            ?peer,
            "a second feeder tried the broker during enrollment"
        );
        let contest = Contest {
            client_id: client_id.into(),
            address: peer,
            at: Utc::now(),
        };
        if let Err(error) = contest.record(&window.contested) {
            warn!(%error, "contest not recorded");
        }
    }

    /// Record a first-use identity, inside the window and only when the
    /// record can be created (it never replaces one).
    fn enroll(
        &self,
        client_id: &str,
        username: &str,
        password: &[u8],
        peer: Option<IpAddr>,
    ) -> Option<Device> {
        let window = self.window.as_ref()?;
        if Instant::now() >= window.until || !enroll::plausible(client_id, username, password) {
            return None;
        }
        // Stock's credentials are text; anything else is not a feeder.
        let password = std::str::from_utf8(password).ok()?;
        let peer = peer?;

        let record = Enrollment {
            client_id: client_id.into(),
            username: username.into(),
            password: password.into(),
            address: peer,
            enrolled_at: Utc::now(),
        };
        if let Err(error) = enroll::create(&record, &window.record) {
            warn!(%error, "enrollment not recorded; refusing the feeder");
            return None;
        }
        info!(device = client_id, %peer, "feeder enrolled on first use");
        Some(Device {
            id: client_id.into(),
            client_id: client_id.into(),
            username: username.into(),
            password: password.into(),
        })
    }
}

type AuthFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

impl AuthProvider for DeviceAuth {
    fn authenticate<'a>(
        &'a self,
        connect: &'a ConnectPacket,
        addr: SocketAddr,
    ) -> AuthFuture<'a, mqtt5::error::Result<AuthResult>> {
        let username = connect.username.as_deref().unwrap_or("");
        let password = connect.password.as_deref().unwrap_or(&[]);
        let peer = self.peers.feeder(addr);

        let unknown = self.device_id().is_none();
        let user = self.login(&connect.client_id, username, password, peer);
        let listening = match (&self.handoff, user) {
            (Some(handoff), Some("device")) if unknown => {
                handoff.enrolled.send_replace(true);
                Some(handoff.listening.clone())
            }
            _ => None,
        };
        Box::pin(async move {
            if let Some(mut listening) = listening
                && tokio::time::timeout(HANDOFF_WAIT, listening.wait_for(|on| *on))
                    .await
                    .is_err()
            {
                warn!(
                    "the server did not subscribe in time; the feeder's first reports may be lost"
                );
            }
            Ok(match user {
                Some(user) => AuthResult::success_with_user(user),
                None => AuthResult::fail(ReasonCode::NotAuthorized),
            })
        })
    }

    fn authorize_publish<'a>(
        &'a self,
        _id: &str,
        user: Option<&'a str>,
        topic: &'a str,
    ) -> AuthFuture<'a, bool> {
        let allowed = self
            .device_id()
            .is_some_and(|device| publish_allowed(&device, user, topic));
        Box::pin(async move { allowed })
    }

    fn authorize_subscribe<'a>(
        &'a self,
        _id: &str,
        user: Option<&'a str>,
        topic: &'a str,
    ) -> AuthFuture<'a, bool> {
        let allowed = self
            .device_id()
            .is_some_and(|device| subscribe_allowed(&device, user, topic));
        Box::pin(async move { allowed })
    }
}

/// The topic prefix of `device`'s namespace.
fn prefix(device: &str) -> String {
    format!("dl/PLAF103/{device}/device/")
}

/// What stock's two identities may publish: each only its side of stock's
/// own topics.
pub fn publish_allowed(device: &str, user: Option<&str>, topic: &str) -> bool {
    let prefix = prefix(device);
    let suffix = topic.strip_prefix(&prefix);

    match user {
        Some("device") => suffix.is_some_and(|s| {
            matches!(
                s,
                "event/post"
                    | "service/post"
                    | "system/post"
                    | "ntp/post"
                    | "heart/post"
                    | "ota/post"
                    | "config/post"
            )
        }),
        Some("server") => suffix.is_some_and(|s| {
            matches!(
                s,
                "event/sub" | "service/sub" | "system/sub" | "ntp/sub" | "heart/sub" | "config/sub"
            )
        }),
        _ => false,
    }
}

/// What stock's two identities may subscribe to.
pub fn subscribe_allowed(device: &str, user: Option<&str>, topic: &str) -> bool {
    let prefix = prefix(device);
    let suffix = topic.strip_prefix(&prefix);

    match user {
        Some("device") => suffix.is_some_and(|s| {
            matches!(
                s,
                "event/sub"
                    | "service/sub"
                    | "system/sub"
                    | "ntp/sub"
                    | "heart/sub"
                    | "config/sub"
                    | "broadcast/sub"
                    | "+/sub"
            )
        }),
        Some("server") => suffix == Some("+/post"),
        _ => false,
    }
}

/// Serve stock on `listen` with `auth` until `stop` resolves.
pub async fn run(
    listen: SocketAddr,
    auth: DeviceAuth,
    stop: impl Future<Output = ()>,
) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(listen).await?;
    let peers = auth.peers.clone();

    let settings = BrokerConfig {
        bind_addresses: vec!["127.0.0.1:0".parse()?],
        max_clients: 8,
        max_packet_size: 65_536,
        session_expiry_interval: Duration::ZERO,
        maximum_qos: 1,
        retain_available: false,
        shared_subscription_available: false,
        max_subscriptions_per_client: 16,
        client_channel_capacity: 64,
        max_outbound_inflight: 32,
        sys_topics_enabled: false,
        storage_config: StorageConfig {
            backend: StorageBackend::Memory,
            enable_persistence: false,
            max_queued_messages_per_client: 0,
            max_queued_bytes_per_client: 0,
            ..Default::default()
        },
        ..Default::default()
    };

    // The builder validates its file-based auth configuration before accepting
    // a custom provider. Replace that default provider BEFORE run() accepts
    // any clients. DeviceAuth rejects anonymous clients and scopes every topic.
    let mut broker = MqttBroker::with_config(settings)
        .await?
        .with_auth_provider(Arc::new(auth));

    let broker_address = broker
        .local_addr()
        .context("broker has no loopback listener")?;

    // Shut down rather than dropped: the broker's client tasks end with it,
    // so nothing is left holding a session once `run` returns.
    let shutdown = broker.shutdown_handle();
    let running = broker.run();
    tokio::pin!(running);
    tokio::select! {
        result = &mut running => {result?; Ok(())},
        result = compat::run(listener, broker_address, peers) => result,
        () = stop => {
            shutdown.shutdown();
            let _ = tokio::time::timeout(STOP_WAIT, running).await;
            Ok(())
        }
    }
}

/// How long a stopped broker gets to end its client tasks.
const STOP_WAIT: Duration = Duration::from_secs(3);

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        let config: Config = serde_json::from_value(serde_json::json!({
            "version":1,"device_id":"AF_TEST","device_client_id":"AF_TEST",
            "device_username":"dev","device_password":"pw",
            "server_password":"s".repeat(32),"api_token":"t".repeat(32),
            "mqtt_listen":"127.0.0.1:1883","http_listen":"127.0.0.1:8765",
            "timezone":"UTC","plans":[],"binding_response":null,
            "hotspot_interface":"wlan1","target_mac":"02:00:00:00:00:01"
        }))
        .unwrap();
        config.validate().unwrap();
        config
    }

    fn installer() -> InstallConfig {
        serde_json::from_value(serde_json::json!({
            "version":1,"server_password":"s".repeat(32),"api_token":"t".repeat(32),
            "mqtt_listen":"127.0.0.1:1883","http_listen":"127.0.0.1:8765","timezone":"UTC"
        }))
        .unwrap()
    }

    const STOCK: &str = "dl/PLAF103/AF_TEST/device/";
    const PEER: Option<IpAddr> = Some(IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 4, 20)));

    #[test]
    fn each_stock_identity_reaches_only_its_side_of_stock_topics() {
        let c = "AF_TEST";
        let (device, server) = (Some("device"), Some("server"));

        assert!(publish_allowed(c, device, &format!("{STOCK}event/post")));
        assert!(!publish_allowed(c, device, &format!("{STOCK}event/sub")));
        assert!(subscribe_allowed(c, device, &format!("{STOCK}+/sub")));
        assert!(!subscribe_allowed(c, device, &format!("{STOCK}+/post")));

        assert!(publish_allowed(c, server, &format!("{STOCK}service/sub")));
        assert!(!publish_allowed(c, server, &format!("{STOCK}ota/post")));
        assert!(subscribe_allowed(c, server, &format!("{STOCK}+/post")));
        assert!(!subscribe_allowed(c, server, &format!("{STOCK}#")));

        for user in [None, Some("firmware"), Some("other")] {
            assert!(!publish_allowed(c, user, &format!("{STOCK}event/post")));
            assert!(!subscribe_allowed(c, user, &format!("{STOCK}+/sub")));
        }
        assert!(!publish_allowed(
            c,
            device,
            "dl/PLAF103/OTHER/device/event/post"
        ));
        assert!(!subscribe_allowed(c, server, "#"));
    }

    #[test]
    fn only_stock_identities_log_in() {
        let auth = DeviceAuth::device(&config());

        assert_eq!(auth.login("AF_TEST", "dev", b"pw", None), Some("device"));
        assert_eq!(auth.login("AF_TEST", "dev", b"wrong", None), None);
        assert_eq!(auth.login("intruder", "intruder", b"anything", PEER), None);
        let server = "s".repeat(32);
        assert_eq!(
            auth.login(
                "local-server-AF_TEST",
                "local-server",
                server.as_bytes(),
                None
            ),
            Some("server")
        );
    }

    #[test]
    fn the_first_stock_feeder_in_the_window_enrolls_and_is_pinned() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("server.json");
        let record = Enrollment::path(&config);
        let contested = Enrollment::contested_path(&config);
        let auth = DeviceAuth::enrolling(&installer(), &config, Duration::from_secs(60));
        let server = "s".repeat(32);

        // No server login, no topics, before a device is known.
        assert_eq!(
            auth.login(
                "local-server-AF_NEW",
                "local-server",
                server.as_bytes(),
                None
            ),
            None
        );
        assert!(auth.device_id().is_none());
        // Not stock-shaped, or with no known address: nothing is recorded.
        assert_eq!(auth.login("local-server-x", "u", b"p", PEER), None);
        assert_eq!(auth.login("AF_NEW", "stock", b"its-own", None), None);
        assert!(!record.exists());

        assert_eq!(
            auth.login("AF_NEW", "stock", b"its-own", PEER),
            Some("device")
        );
        let enrolled = Enrollment::load(&record).unwrap().unwrap();
        assert_eq!(enrolled.client_id, "AF_NEW");
        assert_eq!(enrolled.password, "its-own");
        assert_eq!(enrolled.address, PEER.unwrap());

        // Pinned: the same identity again, the server for it, nobody else.
        assert_eq!(
            auth.login("AF_NEW", "stock", b"its-own", PEER),
            Some("device")
        );
        assert!(!contested.exists());
        assert_eq!(auth.login("AF_NEW", "stock", b"guess", PEER), None);
        assert_eq!(auth.login("AF_OTHER", "stock", b"its-own", PEER), None);
        // Either is a second identity inside the window: the enrollment is
        // contested, and the status says so.
        assert!(contested.exists());
        assert!(Enrollment::view(&config).unwrap().unwrap().contested);
        assert_eq!(
            auth.login(
                "local-server-AF_NEW",
                "local-server",
                server.as_bytes(),
                None
            ),
            Some("server")
        );
        assert_eq!(auth.device_id().as_deref(), Some("AF_NEW"));
    }

    #[test]
    fn nobody_enrolls_after_the_window_or_over_an_existing_record() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("server.json");
        let record = Enrollment::path(&config);
        let closed = DeviceAuth::enrolling(&installer(), &config, Duration::ZERO);
        assert_eq!(closed.login("AF_NEW", "stock", b"pw", PEER), None);
        assert!(!record.exists());

        std::fs::write(&record, "{}").unwrap();
        let open = DeviceAuth::enrolling(&installer(), &config, Duration::from_secs(60));
        assert_eq!(open.login("AF_NEW", "stock", b"pw", PEER), None);
    }
}
