//! The HTTP server in process, on a localhost port, in front of a
//! [`TestService`].

use std::net::{Ipv4Addr, SocketAddr};

use wellfed_server::Hosts;
use wellfed_server_client::{Client, Url};
use wellfed_service::{Config, Service};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::service::{TempStore, TestService, config};

/// The server, serving until [`Self::stop`].
#[derive(Debug)]
pub struct TestServer {
    service: TestService,
    address: SocketAddr,
    shutdown: CancellationToken,
    served: JoinHandle<()>,
}

impl TestServer {
    /// The server with [`config`] on a fresh store.
    pub async fn start() -> Self {
        Self::start_with(config()).await
    }

    pub async fn start_with(config: Config) -> Self {
        Self::open(TempStore::new(), config).await
    }

    /// The server with `config` on `store`, which a test may have filled
    /// through the store first.
    pub async fn open(store: TempStore, config: Config) -> Self {
        let service = TestService::open(store, config).await;
        let shutdown = CancellationToken::new();
        let app = wellfed_server::app(
            service.service().clone(),
            shutdown.clone(),
            Hosts::default(),
        );

        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind the server's port");
        let address = listener
            .local_addr()
            .expect("a bound listener has an address");
        let stop = shutdown.clone();
        let served = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move { stop.cancelled().await })
                .await
                .expect("the server serves until shut down");
        });

        Self {
            service,
            address,
            shutdown,
            served,
        }
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// A client of this server with no credentials.
    pub fn client(&self) -> Client {
        let base =
            Url::parse(&format!("http://{}", self.address)).expect("a socket address makes a URL");
        Client::new(&base).expect("an http URL makes a client")
    }

    pub fn service(&self) -> &Service {
        self.service.service()
    }

    /// End the open streams, stop serving, and stop the service.
    pub async fn stop(self) {
        self.shutdown.cancel();
        self.served.await.expect("the server's task ends cleanly");
        self.service.shutdown().await;
    }
}
