//! The server's feeder keys at rest: ChaCha20-Poly1305 under the config's
//! key, bound to the feeder they belong to, so a sealed key moved to
//! another row does not open.

use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, KeyInit, Nonce};
use wellfed_api::FeederId;
use wellfed_store::SealedKey;

use crate::{InternalError, Result};

pub(crate) fn seal(
    key: &[u8; 32],
    feeder: FeederId,
    identity: &[u8; 32],
    secret: &[u8; 32],
    nonce: [u8; 12],
) -> Result<SealedKey> {
    let aad = bound(feeder, identity);
    let sealed = ChaCha20Poly1305::new(Key::from_slice(key))
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: secret,
                aad: &aad,
            },
        )
        .map_err(|_| InternalError::Unseal)?;
    Ok(SealedKey { nonce, sealed })
}

pub(crate) fn open(
    key: &[u8; 32],
    feeder: FeederId,
    identity: &[u8; 32],
    sealed: &SealedKey,
) -> Result<[u8; 32]> {
    let aad = bound(feeder, identity);
    let secret = ChaCha20Poly1305::new(Key::from_slice(key))
        .decrypt(
            Nonce::from_slice(&sealed.nonce),
            Payload {
                msg: &sealed.sealed,
                aad: &aad,
            },
        )
        .map_err(|_| InternalError::Unseal)?;
    Ok(secret.try_into().map_err(|_| InternalError::Unseal)?)
}

fn bound(feeder: FeederId, identity: &[u8; 32]) -> [u8; 48] {
    let mut aad = [0; 48];
    aad[..16].copy_from_slice(feeder.0.as_bytes());
    aad[16..].copy_from_slice(identity);
    aad
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    #[test]
    fn a_key_opens_only_under_its_key_and_feeder() {
        let feeder = FeederId(Uuid::from_u128(1));
        let sealed = seal(&[1; 32], feeder, &[2; 32], &[3; 32], [4; 12]).unwrap();
        assert_ne!(sealed.sealed[..32], [3; 32]);
        assert_eq!(open(&[1; 32], feeder, &[2; 32], &sealed).unwrap(), [3; 32]);

        assert!(open(&[9; 32], feeder, &[2; 32], &sealed).is_err());
        assert!(open(&[1; 32], FeederId(Uuid::from_u128(2)), &[2; 32], &sealed).is_err());
        assert!(open(&[1; 32], feeder, &[9; 32], &sealed).is_err());
    }
}
