//! A device API session for the CLI: `wellfed-connect`'s, with this
//! computer's key and the system's random source.

use std::net::SocketAddr;

use anyhow::{Context, Result};
use wellfed_connect::{Cursor, DISCOVERY};
use wellfed_formats::private;
use wellfed_model::{Answer, Capabilities, ClientRole, Op, Pushed};
use wellfed_proto::api::RequestId;

use crate::keys::OwnerKey;

/// The feeder's address: given, or found over mDNS by its identity.
pub(crate) fn locate(identity: &[u8; 32], address: Option<&str>) -> Result<SocketAddr> {
    Ok(wellfed_connect::locate(identity, address, DISCOVERY)?)
}

/// Fresh bytes from the system's random source.
pub(crate) fn random<const N: usize>() -> Result<[u8; N]> {
    private::random(OwnerKey::system()).context("read the system random source")
}

/// An open session.
pub(crate) struct Connection(wellfed_connect::Connection);

impl Connection {
    pub(crate) async fn open(
        key: &OwnerKey,
        identity: [u8; 32],
        address: SocketAddr,
    ) -> Result<Self> {
        let connection =
            wellfed_connect::Connection::open(key.keypair(), identity, address, random()?)
                .await
                .context("open a session")?;
        Ok(Self(connection))
    }

    pub(crate) fn build(&self) -> &str {
        self.0.build()
    }

    pub(crate) fn role(&self) -> ClientRole {
        self.0.role()
    }

    pub(crate) fn capabilities(&self) -> Capabilities {
        self.0.capabilities()
    }

    /// Events after `after`, counted in the feeder's current epoch.
    pub(crate) async fn subscribe(&mut self, after: Option<u64>, samples: bool) -> Result<()> {
        let epoch = self.0.epoch();
        let from = after.map(|seq| Cursor { epoch, seq });
        self.0.subscribe(from, samples).await?;
        Ok(())
    }

    /// The next message within `wait`; `None` when none came in time.
    pub(crate) async fn next(&mut self, wait: std::time::Duration) -> Result<Option<Pushed>> {
        match self.0.next(wait).await {
            Ok(message) => Ok(Some(message)),
            Err(wellfed_connect::Error::Timeout) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// One request under a fresh id, and its answer.
    pub(crate) async fn ask(&mut self, op: &Op) -> Result<Answer> {
        let id: RequestId = random()?;
        Ok(self.0.ask(id, op).await?)
    }
}
