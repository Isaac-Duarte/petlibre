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

use crate::{compat, config::Config};

struct DeviceAuth(Config);
type AuthFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

impl AuthProvider for DeviceAuth {
    fn authenticate<'a>(
        &'a self,
        connect: &'a ConnectPacket,
        _addr: SocketAddr,
    ) -> AuthFuture<'a, mqtt5::error::Result<AuthResult>> {
        let config = &self.0;
        let username = connect.username.as_deref().unwrap_or("");
        let password = connect.password.as_deref().unwrap_or(&[]);
        let device = connect.client_id == config.device_client_id
            && username == config.device_username
            && bool::from(password.ct_eq(config.device_password.as_bytes()));
        let server = connect.client_id == config.server_client_id()
            && username == "local-server"
            && bool::from(password.ct_eq(config.server_password.as_bytes()));
        let firmware = config.firmware.as_ref().is_some_and(|firmware| {
            connect.client_id == firmware.client_id
                && username == firmware.username
                && bool::from(password.ct_eq(firmware.password.as_bytes()))
        });
        Box::pin(async move {
            Ok(if device {
                AuthResult::success_with_user("device")
            } else if server {
                AuthResult::success_with_user("server")
            } else if firmware {
                AuthResult::success_with_user("firmware")
            } else {
                AuthResult::fail(ReasonCode::NotAuthorized)
            })
        })
    }

    fn authorize_publish<'a>(
        &'a self,
        _id: &str,
        user: Option<&'a str>,
        topic: &'a str,
    ) -> AuthFuture<'a, bool> {
        let allowed = publish_allowed(&self.0, user, topic);
        Box::pin(async move { allowed })
    }

    fn authorize_subscribe<'a>(
        &'a self,
        _id: &str,
        user: Option<&'a str>,
        topic: &'a str,
    ) -> AuthFuture<'a, bool> {
        let allowed = subscribe_allowed(&self.0, user, topic);
        Box::pin(async move { allowed })
    }
}

/// The topic's suffix in the replacement-firmware namespace, only when that
/// role is configured. Wildcards never match: every allowed suffix is literal.
fn firmware_suffix<'a>(config: &Config, topic: &'a str) -> Option<&'a str> {
    config.firmware.as_ref()?;
    topic.strip_prefix(&config.firmware_namespace())
}

fn publish_allowed(config: &Config, user: Option<&str>, topic: &str) -> bool {
    let prefix = config.topic("");
    let suffix = topic.strip_prefix(&prefix);
    let firmware = firmware_suffix(config, topic);
    match user {
        Some("firmware") => matches!(firmware, Some("health" | "reply")),
        Some("server") if firmware.is_some() => firmware == Some("request"),
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

fn subscribe_allowed(config: &Config, user: Option<&str>, topic: &str) -> bool {
    let prefix = config.topic("");
    let suffix = topic.strip_prefix(&prefix);
    let firmware = firmware_suffix(config, topic);
    match user {
        Some("firmware") => firmware == Some("request"),
        Some("server") if firmware.is_some() => matches!(firmware, Some("health" | "reply")),
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
        .with_auth_provider(Arc::new(DeviceAuth(config)));
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

    fn config(firmware: bool) -> Config {
        let mut value = serde_json::json!({
            "version":1,"device_id":"AF_TEST","device_client_id":"AF_TEST",
            "device_username":"dev","device_password":"pw",
            "server_password":"s".repeat(32),"api_token":"t".repeat(32),
            "mqtt_listen":"127.0.0.1:1883","http_listen":"127.0.0.1:8765",
            "timezone":"UTC","max_portions":1,"plans":[],"binding_response":null,
            "hotspot_interface":"wlan1","target_mac":"02:00:00:00:00:01"
        });
        if firmware {
            value["firmware"] = serde_json::json!({
                "client_id":"petlibre-fw-AF_TEST","username":"petlibre-fw","password":"a".repeat(64)
            });
        }
        let config: Config = serde_json::from_value(value).unwrap();
        config.validate().unwrap();
        config
    }

    const FW: &str = "petlibre/1/AF_TEST/";
    const STOCK: &str = "dl/PLAF103/AF_TEST/device/";

    #[test]
    fn firmware_role_reaches_only_its_three_literal_topics() {
        let c = config(true);
        let user = Some("firmware");
        for (topic, publish, subscribe) in [
            ("health", true, false),
            ("reply", true, false),
            ("request", false, true),
            ("+", false, false),
            ("#", false, false),
            ("other", false, false),
        ] {
            let topic = format!("{FW}{topic}");
            assert_eq!(publish_allowed(&c, user, &topic), publish, "{topic}");
            assert_eq!(subscribe_allowed(&c, user, &topic), subscribe, "{topic}");
        }
        for topic in ["event/post", "event/sub", "+/sub", "+/post"] {
            let topic = format!("{STOCK}{topic}");
            assert!(!publish_allowed(&c, user, &topic) && !subscribe_allowed(&c, user, &topic));
        }
        assert!(!subscribe_allowed(&c, user, "petlibre/#"));
        assert!(!subscribe_allowed(&c, user, "petlibre/1/OTHER/request"));
    }

    #[test]
    fn server_gains_the_reverse_topics_and_stock_device_gains_none() {
        let c = config(true);
        let server = Some("server");
        assert!(publish_allowed(&c, server, &format!("{FW}request")));
        assert!(!publish_allowed(&c, server, &format!("{FW}health")));
        assert!(subscribe_allowed(&c, server, &format!("{FW}health")));
        assert!(subscribe_allowed(&c, server, &format!("{FW}reply")));
        assert!(!subscribe_allowed(&c, server, &format!("{FW}request")));
        assert!(!subscribe_allowed(&c, server, &format!("{FW}+")));
        // Existing server access is unchanged.
        assert!(subscribe_allowed(&c, server, &format!("{STOCK}+/post")));
        assert!(publish_allowed(&c, server, &format!("{STOCK}service/sub")));
        let device = Some("device");
        for topic in ["health", "reply", "request", "+", "#"] {
            let topic = format!("{FW}{topic}");
            assert!(!publish_allowed(&c, device, &topic) && !subscribe_allowed(&c, device, &topic));
        }
        assert!(publish_allowed(&c, device, &format!("{STOCK}event/post")));
        assert!(!publish_allowed(&c, None, &format!("{FW}health")));
    }

    #[test]
    fn without_a_firmware_role_the_namespace_is_closed_to_everyone() {
        let c = config(false);
        for user in ["firmware", "server", "device"] {
            for topic in ["health", "reply", "request"] {
                let topic = format!("{FW}{topic}");
                assert!(!publish_allowed(&c, Some(user), &topic));
                assert!(!subscribe_allowed(&c, Some(user), &topic));
            }
        }
    }
}
