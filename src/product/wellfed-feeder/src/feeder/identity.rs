//! The feeder's identity key pair, held by the loop.
//!
//! The settings writer keeps the identity secret in the client store and
//! publishes it ([`Platform::identity_since`]); the loop derives the public
//! key here, where the handshakes that need both run. The derivation is an
//! X25519 multiplication, which the writer's task cannot afford and the
//! loop's already does for every handshake.
//!
//! A boot's first publish and each factory reset bring a new generation. The
//! record carries only the public key's first bytes: enough to tell
//! identities apart across boots and resets, and no secret.

use core::fmt;

use wellfed_session::hex;
use wellfed_session::noise::Keypair;

use super::Platform;

/// Public key bytes in the record.
pub const FINGERPRINT: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    pub generation: u32,
    pub fingerprint: [u8; FINGERPRINT],
}

/// The body of a `PLID1` line.
pub fn describe(record: Record, f: &mut impl fmt::Write) -> fmt::Result {
    write!(
        f,
        "identity generation={} fingerprint={}",
        record.generation,
        hex::Lower(&record.fingerprint)
    )
}

#[derive(Default)]
pub struct Identity {
    generation: u32,
    keypair: Option<Keypair>,
}

impl Identity {
    pub const fn new() -> Self {
        Self {
            generation: 0,
            keypair: None,
        }
    }

    /// The key pair, once the writer has published one.
    pub fn keypair(&self) -> Option<&Keypair> {
        self.keypair.as_ref()
    }

    /// Take a newly published identity, if any.
    pub fn step(&mut self, platform: &mut impl Platform) -> Option<Record> {
        let (generation, secret) = platform.identity_since(self.generation)?;
        let keypair = Keypair::from_secret(secret);

        let mut fingerprint = [0; FINGERPRINT];
        fingerprint.copy_from_slice(&keypair.public()[..FINGERPRINT]);
        self.generation = generation;
        self.keypair = Some(keypair);
        Some(Record {
            generation,
            fingerprint,
        })
    }
}
