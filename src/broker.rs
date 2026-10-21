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
        Box::pin(async move {
            Ok(if device {
                AuthResult::success_with_user("device")
            } else if server {
                AuthResult::success_with_user("server")
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
        let prefix = self.0.topic("");
        let suffix = topic.strip_prefix(&prefix);
        let allowed = match user {
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
                    "event/sub"
                        | "service/sub"
                        | "system/sub"
                        | "ntp/sub"
                        | "heart/sub"
                        | "config/sub"
                )
            }),
            _ => false,
        };
        Box::pin(async move { allowed })
    }

    fn authorize_subscribe<'a>(
        &'a self,
        _id: &str,
        user: Option<&'a str>,
        topic: &'a str,
    ) -> AuthFuture<'a, bool> {
        let prefix = self.0.topic("");
        let suffix = topic.strip_prefix(&prefix);
        let allowed = match user {
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
        };
        Box::pin(async move { allowed })
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
