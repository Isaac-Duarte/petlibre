//! Reaching a feeder: at its stored address, or wherever mDNS finds it now,
//! for a task's session or to redeem an invite.

use std::net::SocketAddr;
use std::time::Duration;

use tracing::{debug, warn};
use wellfed_api::FeederId;
use wellfed_connect::Connection;
use wellfed_model::{ClientRole, Joined};
use wellfed_session::noise::Keypair;

use crate::{ConnectError, Discovery, Invite, Mirror, Shared, Unreachable};

/// A key the feeder enrolled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Enrolled {
    pub role: ClientRole,
    /// The enrolled key's public half, as the feeder lists it.
    pub public: [u8; 32],
}

impl Mirror {
    /// Where to reach a feeder: `address` when one is given, or the feeder
    /// found over mDNS.
    pub async fn locate(
        &self,
        identity: [u8; 32],
        address: Option<&str>,
    ) -> Result<SocketAddr, ConnectError> {
        match address.map(str::trim).filter(|address| !address.is_empty()) {
            Some(address) => wellfed_connect::locate(&identity, Some(address), Duration::ZERO)
                .map_err(|_| ConnectError::Address),
            None if self.shared.config.discovery == Discovery::Off => {
                Err(ConnectError::AddressRequired)
            }
            None => discover(identity).await,
        }
    }

    /// Redeem `invite` at `address` with the key `secret`, listed on the
    /// feeder as `label`.
    pub async fn enroll(
        &self,
        invite: Invite,
        label: &str,
        address: SocketAddr,
        secret: [u8; 32],
    ) -> Result<Enrolled, ConnectError> {
        let keys = Keypair::from_secret(secret);
        let ephemeral = self.shared.random()?;
        let joined = wellfed_connect::join(&keys, invite, label, address, ephemeral)
            .await
            .map_err(|error| match error {
                // The feeder drops an unknown or expired invite unanswered.
                wellfed_connect::Error::Closed => ConnectError::InviteExpired,
                other => Unreachable(other).into(),
            })?;
        match joined {
            Joined::Enrolled { role } => Ok(Enrolled {
                role,
                public: *keys.public(),
            }),
            Joined::Refused(refusal) => Err(ConnectError::Refused(refusal)),
        }
    }
}

/// Open a session at the stored address, or, when that fails and discovery
/// is on, wherever mDNS finds the feeder now.
pub(crate) async fn open(
    shared: &Shared,
    id: FeederId,
    identity: [u8; 32],
    keys: &Keypair,
    address: Option<&str>,
) -> Result<(Connection, SocketAddr), ConnectError> {
    let stored = address
        .and_then(|address| wellfed_connect::locate(&identity, Some(address), Duration::ZERO).ok());
    if let Some(address) = stored {
        let ephemeral = shared.random()?;
        match Connection::open(keys, identity, address, ephemeral).await {
            Ok(connection) => return Ok((connection, address)),
            // The feeder answered: looking elsewhere would not help.
            Err(
                error @ (wellfed_connect::Error::UnknownKey
                | wellfed_connect::Error::Version(_)
                | wellfed_connect::Error::Full),
            ) => return Err(Unreachable(error).into()),
            Err(error) if shared.config.discovery == Discovery::Off => {
                return Err(Unreachable(error).into());
            }
            Err(error) => debug!("{address}: {error}; looking over mDNS"),
        }
    } else if shared.config.discovery == Discovery::Off {
        let address = address.unwrap_or_default().to_owned();
        return Err(Unreachable(wellfed_connect::Error::Address(address)).into());
    }

    let found = discover(identity).await?;
    if let Err(error) = shared.store.set_address(id, &found.to_string()).await {
        warn!("remember the address {found}: {error}");
    }
    let ephemeral = shared.random()?;
    let connection = Connection::open(keys, identity, found, ephemeral)
        .await
        .map_err(Unreachable)?;
    Ok((connection, found))
}

/// The feeder's address over mDNS, which blocks a thread while it browses.
async fn discover(identity: [u8; 32]) -> Result<SocketAddr, ConnectError> {
    tokio::task::spawn_blocking(move || {
        wellfed_connect::locate(&identity, None, wellfed_connect::DISCOVERY)
    })
    .await
    .map_err(|_| ConnectError::Lookup)?
    .map_err(|error| Unreachable(error).into())
}
