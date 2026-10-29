//! The phone's client key: one X25519 static key that is its identity to
//! every feeder it pairs with. Dart keeps the secret in the Android
//! Keystore-backed secure storage and hands it back at each start.

use std::path::Path;

use flutter_rust_bridge::frb;
use wellfed_formats::private::{self, SYSTEM_RANDOM};
use wellfed_session::noise::Keypair;

use crate::api::error::{BridgeError, ErrorKind};

/// `N` fresh bytes from the OS generator: secrets, ephemeral keys, request
/// ids and code bits.
pub(crate) fn random<const N: usize>() -> Result<[u8; N], BridgeError> {
    private::random(Path::new(SYSTEM_RANDOM))
        .map_err(|error| BridgeError::new(ErrorKind::Random, error))
}

#[frb(opaque)]
pub struct ClientKey {
    secret: [u8; 32],
    keypair: Keypair,
}

impl ClientKey {
    /// A new key, for a phone that has none yet.
    #[frb(sync)]
    pub fn generate() -> Result<Self, BridgeError> {
        Ok(Self::from_secret(random()?))
    }

    /// The key whose secret secure storage kept.
    #[frb(sync)]
    pub fn restore(secret: Vec<u8>) -> Result<Self, BridgeError> {
        let secret: [u8; 32] = secret.try_into().map_err(|bytes: Vec<u8>| {
            BridgeError::new(ErrorKind::Key, format!("{} bytes, not 32", bytes.len()))
        })?;
        Ok(Self::from_secret(secret))
    }

    /// The secret, for secure storage only.
    #[frb(sync)]
    pub fn secret(&self) -> Vec<u8> {
        self.secret.to_vec()
    }

    /// The public key, as feeders list this phone.
    #[frb(sync)]
    pub fn public(&self) -> Vec<u8> {
        self.keypair.public().to_vec()
    }

    pub(crate) fn from_secret(secret: [u8; 32]) -> Self {
        Self {
            secret,
            keypair: Keypair::from_secret(secret),
        }
    }

    pub(crate) fn keypair(&self) -> &Keypair {
        &self.keypair
    }
}
