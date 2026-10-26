//! The embedded MQTT broker, which `serve` runs as its `broker` child process.
//!
//! The broker itself listens only on an ephemeral loopback port. The
//! configured feeder-facing address is served by `compat`, which normalizes
//! the stock firmware's first CONNECT and relays it. `DeviceAuth` admits
//! only stock's two fixed identities (the stock device and the server),
//! each to only an allowlist of stock topics. Retained messages, persistent
//! sessions and offline queues are disabled, so no command can wait for a
//! later connection.

use std::{future::Future, net::SocketAddr, pin::Pin, sync::Arc, time::Duration};

use anyhow::{Context, Result};
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

use crate::{config::Config, mqtt::compat};

struct DeviceAuth {
    config: Config,
}

impl DeviceAuth {
    /// The user a login is admitted as: one of stock's two identities.
    fn login(&self, client_id: &str, username: &str, password: &[u8]) -> Option<&'static str> {
        let config = &self.config;
        let device = client_id == config.device_client_id
            && username == config.device_username
            && bool::from(password.ct_eq(config.device_password.as_bytes()));
        let server = client_id == config.server_client_id()
            && username == "local-server"
            && bool::from(password.ct_eq(config.server_password.as_bytes()));

        if device {
            Some("device")
        } else if server {
            Some("server")
        } else {
            None
        }
    }
}

type AuthFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

impl AuthProvider for DeviceAuth {
    fn authenticate<'a>(
        &'a self,
        connect: &'a ConnectPacket,
        _addr: SocketAddr,
    ) -> AuthFuture<'a, mqtt5::error::Result<AuthResult>> {
        let username = connect.username.as_deref().unwrap_or("");
        let password = connect.password.as_deref().unwrap_or(&[]);

        let result = match self.login(&connect.client_id, username, password) {
            Some(user) => AuthResult::success_with_user(user),
            None => AuthResult::fail(ReasonCode::NotAuthorized),
        };
        Box::pin(async move { Ok(result) })
    }

    fn authorize_publish<'a>(
        &'a self,
        _id: &str,
        user: Option<&'a str>,
        topic: &'a str,
    ) -> AuthFuture<'a, bool> {
        let allowed = publish_allowed(&self.config, user, topic);
        Box::pin(async move { allowed })
    }

    fn authorize_subscribe<'a>(
        &'a self,
        _id: &str,
        user: Option<&'a str>,
        topic: &'a str,
    ) -> AuthFuture<'a, bool> {
        let allowed = subscribe_allowed(&self.config, user, topic);
        Box::pin(async move { allowed })
    }
}

/// What stock's two identities may publish: each only its side of stock's
/// own topics.
pub fn publish_allowed(config: &Config, user: Option<&str>, topic: &str) -> bool {
    let prefix = config.topic("");
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
pub fn subscribe_allowed(config: &Config, user: Option<&str>, topic: &str) -> bool {
    let prefix = config.topic("");
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

pub async fn run(config: Config) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(config.mqtt_listen).await?;

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
        .with_auth_provider(Arc::new(DeviceAuth { config }));

    let broker_address = broker
        .local_addr()
        .context("broker has no loopback listener")?;

    let shutdown = broker.shutdown_handle();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        shutdown.shutdown();
    });

    tokio::select! {
        result=broker.run()=>{result?;Ok(())},
        result=compat::run(listener,broker_address)=>result,
    }
}

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

    const STOCK: &str = "dl/PLAF103/AF_TEST/device/";

    #[test]
    fn each_stock_identity_reaches_only_its_side_of_stock_topics() {
        let c = config();
        let (device, server) = (Some("device"), Some("server"));

        assert!(publish_allowed(&c, device, &format!("{STOCK}event/post")));
        assert!(!publish_allowed(&c, device, &format!("{STOCK}event/sub")));
        assert!(subscribe_allowed(&c, device, &format!("{STOCK}+/sub")));
        assert!(!subscribe_allowed(&c, device, &format!("{STOCK}+/post")));

        assert!(publish_allowed(&c, server, &format!("{STOCK}service/sub")));
        assert!(!publish_allowed(&c, server, &format!("{STOCK}ota/post")));
        assert!(subscribe_allowed(&c, server, &format!("{STOCK}+/post")));
        assert!(!subscribe_allowed(&c, server, &format!("{STOCK}#")));

        for user in [None, Some("firmware"), Some("other")] {
            assert!(!publish_allowed(&c, user, &format!("{STOCK}event/post")));
            assert!(!subscribe_allowed(&c, user, &format!("{STOCK}+/sub")));
        }
        assert!(!publish_allowed(
            &c,
            device,
            "dl/PLAF103/OTHER/device/event/post"
        ));
        assert!(!subscribe_allowed(&c, server, "#"));
    }

    #[test]
    fn only_stock_identities_log_in() {
        let auth = DeviceAuth { config: config() };

        assert_eq!(auth.login("AF_TEST", "dev", b"pw"), Some("device"));
        assert_eq!(auth.login("AF_TEST", "dev", b"wrong"), None);
        assert_eq!(auth.login("intruder", "intruder", b"anything"), None);
    }
}
