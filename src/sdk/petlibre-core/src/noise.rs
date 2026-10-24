//! The Noise Protocol Framework (revision 34) for the device API: the two
//! handshakes the product uses, over 25519, ChaChaPoly and SHA-256.
//!
//! - **XX** pairs a client the feeder has never seen: both static keys travel
//!   encrypted, and each side learns the other's.
//! - **IK** opens a paired client's session on the device API (FW47): the
//!   client already holds the feeder's static key and sends its own encrypted
//!   in the first message, so the feeder learns which client is connecting
//!   without trying each enrolled key, in one round trip with no identity in
//!   clear.
//! - **KK** reconnects when both sides already hold the other's static key
//!   (FW44's budget measured it).
//! - **XXpsk3** is XX bound to an invite's secret (FW45): the third message
//!   fails unless both sides hold the same pre-shared key. The responder
//!   learns which invite from the first message's payload, so it sets the
//!   key after the handshake starts.
//!
//! This is sans-IO and allocates nothing: the caller passes message buffers
//! in and gets lengths back, so the feeder's loop task and the host client
//! (`petlibre-client`) run the same code. It draws no randomness either; the
//! caller supplies the one ephemeral secret a handshake needs, from the
//! feeder's vetted generator (FW45) or the host's OS. A secret must never be
//! reused, or the session keys repeat.
//!
//! Host tests check every handshake against `snow` in both roles, so the two
//! implementations agree byte for byte.

use chacha20poly1305::aead::{AeadInPlace, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce, Tag};
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret, X25519_BASEPOINT_BYTES, x25519};

use crate::mac::hmac_sha256;

pub const KEY_LEN: usize = 32;
pub const TAG_LEN: usize = 16;
/// The largest Noise message; the caller's buffers may be smaller.
pub const MAX_MESSAGE: usize = 65_535;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pattern {
    Xx,
    Ik,
    Kk,
    XxPsk3,
}

impl Pattern {
    /// The full protocol name, which seeds the handshake hash.
    pub const fn protocol_name(self) -> &'static [u8] {
        match self {
            Self::Xx => b"Noise_XX_25519_ChaChaPoly_SHA256",
            Self::Ik => b"Noise_IK_25519_ChaChaPoly_SHA256",
            Self::Kk => b"Noise_KK_25519_ChaChaPoly_SHA256",
            Self::XxPsk3 => b"Noise_XXpsk3_25519_ChaChaPoly_SHA256",
        }
    }

    const fn messages(self) -> &'static [&'static [Token]] {
        use Token::{E, Ee, Es, Psk, S, Se, Ss};
        match self {
            Self::Xx => &[&[E], &[E, Ee, S, Es], &[S, Se]],
            Self::Ik => &[&[E, Es, S, Ss], &[E, Ee, Se]],
            Self::Kk => &[&[E, Es, Ss], &[E, Ee, Se]],
            Self::XxPsk3 => &[&[E], &[E, Ee, S, Es], &[S, Se, Psk]],
        }
    }

    /// A PSK pattern also mixes each ephemeral key into the cipher key.
    const fn has_psk(self) -> bool {
        matches!(self, Self::XxPsk3)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Initiator,
    Responder,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Token {
    E,
    S,
    Ee,
    Es,
    Se,
    Ss,
    Psk,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The output buffer cannot hold the message.
    BufferTooSmall,
    /// The message is shorter than its pattern requires, or longer than
    /// [`MAX_MESSAGE`].
    BadLength,
    /// Authentication failed: a tampered, misordered or foreign message.
    Decrypt,
    /// A Diffie-Hellman result was all zeros: the peer sent a low-order key.
    WeakKey,
    /// KK, and IK's initiator, need the remote static key before it starts.
    MissingRemoteKey,
    /// A PSK pattern reached its `psk` token with no key set.
    MissingPsk,
    /// Called out of turn, or after the handshake finished.
    WrongTurn,
    /// The handshake has not finished, so there are no transport keys.
    Unfinished,
    /// 2^64 - 1 messages in one direction; the session must end.
    NonceExhausted,
}

/// A key and its message counter, for one direction.
#[derive(Clone)]
struct CipherState {
    key: Option<[u8; KEY_LEN]>,
    nonce: u64,
}

impl CipherState {
    const fn empty() -> Self {
        Self {
            key: None,
            nonce: 0,
        }
    }

    fn keyed(key: [u8; KEY_LEN]) -> Self {
        Self {
            key: Some(key),
            nonce: 0,
        }
    }

    fn nonce(&self) -> Result<Nonce, Error> {
        // Noise reserves the last value.
        if self.nonce == u64::MAX {
            return Err(Error::NonceExhausted);
        }
        let mut bytes = [0; 12];
        bytes[4..].copy_from_slice(&self.nonce.to_le_bytes());
        Ok(bytes.into())
    }

    /// `out[..plain.len()]` becomes the ciphertext and the tag follows; with
    /// no key yet the plaintext passes through. Returns the length written.
    fn encrypt(&mut self, ad: &[u8], plain: &[u8], out: &mut [u8]) -> Result<usize, Error> {
        let Some(key) = self.key else {
            let out = out.get_mut(..plain.len()).ok_or(Error::BufferTooSmall)?;
            out.copy_from_slice(plain);
            return Ok(plain.len());
        };

        let len = plain.len() + TAG_LEN;
        let out = out.get_mut(..len).ok_or(Error::BufferTooSmall)?;
        let (body, tag_out) = out.split_at_mut(plain.len());
        body.copy_from_slice(plain);
        let tag = ChaCha20Poly1305::new(Key::from_slice(&key))
            .encrypt_in_place_detached(&self.nonce()?, ad, body)
            .map_err(|_| Error::BufferTooSmall)?;
        tag_out.copy_from_slice(&tag);

        self.nonce += 1;
        Ok(len)
    }

    /// The reverse of [`Self::encrypt`]; `out` receives the plaintext.
    fn decrypt(&mut self, ad: &[u8], message: &[u8], out: &mut [u8]) -> Result<usize, Error> {
        let Some(key) = self.key else {
            let out = out.get_mut(..message.len()).ok_or(Error::BufferTooSmall)?;
            out.copy_from_slice(message);
            return Ok(message.len());
        };

        let len = message.len().checked_sub(TAG_LEN).ok_or(Error::BadLength)?;
        let (body, tag) = message.split_at(len);
        let out = out.get_mut(..len).ok_or(Error::BufferTooSmall)?;
        out.copy_from_slice(body);
        ChaCha20Poly1305::new(Key::from_slice(&key))
            .decrypt_in_place_detached(&self.nonce()?, ad, out, Tag::from_slice(tag))
            .map_err(|_| Error::Decrypt)?;

        self.nonce += 1;
        Ok(len)
    }
}

/// The chaining key, the handshake hash and the handshake's cipher.
#[derive(Clone)]
struct SymmetricState {
    ck: [u8; 32],
    h: [u8; 32],
    cipher: CipherState,
}

impl SymmetricState {
    fn new(pattern: Pattern) -> Self {
        // A name of up to 32 bytes is the hash, zero-padded; a longer one
        // is hashed.
        let name = pattern.protocol_name();
        let mut h = [0; 32];
        if let Some(prefix) = h.get_mut(..name.len()) {
            prefix.copy_from_slice(name);
        } else {
            h = Sha256::digest(name).into();
        }
        Self {
            ck: h,
            h,
            cipher: CipherState::empty(),
        }
    }

    fn mix_hash(&mut self, data: &[u8]) {
        let mut hash = Sha256::new();
        hash.update(self.h);
        hash.update(data);
        self.h = hash.finalize().into();
    }

    fn mix_key(&mut self, input: &[u8; 32]) {
        let (ck, key) = hkdf(&self.ck, input);
        self.ck = ck;
        self.cipher = CipherState::keyed(key);
    }

    /// Noise's `MixKeyAndHash`, for the `psk` token.
    fn mix_key_and_hash(&mut self, input: &[u8; 32]) {
        let temp = hmac_sha256(&self.ck, &[input]);
        let ck = hmac_sha256(&temp, &[&[1]]);
        let hash = hmac_sha256(&temp, &[&ck, &[2]]);
        let key = hmac_sha256(&temp, &[&hash, &[3]]);
        self.ck = ck;
        self.mix_hash(&hash);
        self.cipher = CipherState::keyed(key);
    }

    fn encrypt_and_hash(&mut self, plain: &[u8], out: &mut [u8]) -> Result<usize, Error> {
        let h = self.h;
        let len = self.cipher.encrypt(&h, plain, out)?;
        self.mix_hash(&out[..len]);
        Ok(len)
    }

    fn decrypt_and_hash(&mut self, message: &[u8], out: &mut [u8]) -> Result<usize, Error> {
        let h = self.h;
        let len = self.cipher.decrypt(&h, message, out)?;
        self.mix_hash(message);
        Ok(len)
    }

    /// Noise's two-output HKDF over an empty input: the send and receive
    /// keys of the initiator.
    fn split(&self) -> (CipherState, CipherState) {
        let (first, second) = hkdf(&self.ck, &[]);
        (CipherState::keyed(first), CipherState::keyed(second))
    }

    /// How long `plain` becomes once encrypted under the current state.
    fn sealed_len(&self, plain: usize) -> usize {
        plain
            + if self.cipher.key.is_some() {
                TAG_LEN
            } else {
                0
            }
    }
}

/// Noise's HKDF with two outputs, on HMAC-SHA256.
fn hkdf(chaining_key: &[u8; 32], input: &[u8]) -> ([u8; 32], [u8; 32]) {
    let temp = hmac_sha256(chaining_key, &[input]);
    let first = hmac_sha256(&temp, &[&[1]]);
    let second = hmac_sha256(&temp, &[&first, &[2]]);
    (first, second)
}

/// A 25519 key pair.
#[derive(Clone)]
pub struct Keypair {
    secret: StaticSecret,
    public: [u8; KEY_LEN],
}

impl Keypair {
    pub fn from_secret(secret: [u8; KEY_LEN]) -> Self {
        // The Montgomery ladder on the base point, not `PublicKey::from`:
        // without precomputed tables that path runs a general Edwards
        // multiplication whose 1.9 KiB frame overflowed the feeder's loop
        // task (FW44). The ladder is the one every DH already uses.
        let public = x25519(secret, X25519_BASEPOINT_BYTES);
        Self {
            secret: StaticSecret::from(secret),
            public,
        }
    }

    pub const fn public(&self) -> &[u8; KEY_LEN] {
        &self.public
    }

    fn dh(&self, remote: &[u8; KEY_LEN]) -> Result<[u8; 32], Error> {
        let shared = self.secret.diffie_hellman(&PublicKey::from(*remote));
        if !shared.was_contributory() {
            return Err(Error::WeakKey);
        }
        Ok(shared.to_bytes())
    }
}

/// One side of a handshake.
pub struct Handshake {
    pattern: Pattern,
    role: Role,
    state: SymmetricState,
    s: Keypair,
    e: Keypair,
    rs: Option<[u8; KEY_LEN]>,
    re: Option<[u8; KEY_LEN]>,
    psk: Option<[u8; KEY_LEN]>,
    /// The next message of the pattern.
    step: usize,
}

impl Handshake {
    /// `ephemeral` is a fresh random secret, never used before. KK, and IK's
    /// initiator, need the peer's static key in `remote_static`; XX and IK's
    /// responder learn it and ignore one given here.
    pub fn new(
        pattern: Pattern,
        role: Role,
        prologue: &[u8],
        local_static: &Keypair,
        remote_static: Option<[u8; KEY_LEN]>,
        ephemeral: [u8; KEY_LEN],
    ) -> Result<Self, Error> {
        let mut state = SymmetricState::new(pattern);
        state.mix_hash(prologue);

        let rs = match pattern {
            Pattern::Xx | Pattern::XxPsk3 => None,
            Pattern::Kk => {
                // KK's pre-messages: the initiator's static, then the
                // responder's, the same order on both sides.
                let remote = remote_static.ok_or(Error::MissingRemoteKey)?;
                let (first, second) = match role {
                    Role::Initiator => (*local_static.public(), remote),
                    Role::Responder => (remote, *local_static.public()),
                };
                state.mix_hash(&first);
                state.mix_hash(&second);
                Some(remote)
            }
            // IK's one pre-message is the responder's static; the
            // responder learns the initiator's from the first message.
            Pattern::Ik => match role {
                Role::Initiator => {
                    let remote = remote_static.ok_or(Error::MissingRemoteKey)?;
                    state.mix_hash(&remote);
                    Some(remote)
                }
                Role::Responder => {
                    state.mix_hash(local_static.public());
                    None
                }
            },
        };

        Ok(Self {
            pattern,
            role,
            state,
            s: local_static.clone(),
            e: Keypair::from_secret(ephemeral),
            rs,
            re: None,
            psk: None,
            step: 0,
        })
    }

    /// The pre-shared key of a PSK pattern; any time before its token.
    pub fn set_psk(&mut self, psk: [u8; KEY_LEN]) {
        self.psk = Some(psk);
    }

    /// Whether the next call is ours to write (false once finished).
    pub fn is_my_turn(&self) -> bool {
        let initiator_writes = self.step.is_multiple_of(2);
        !self.is_finished() && initiator_writes == (self.role == Role::Initiator)
    }

    pub fn is_finished(&self) -> bool {
        self.step >= self.pattern.messages().len()
    }

    /// The peer's static key, once known.
    pub const fn remote_static(&self) -> Option<&[u8; KEY_LEN]> {
        self.rs.as_ref()
    }

    /// Binds a later message to this handshake (channel binding).
    pub const fn handshake_hash(&self) -> &[u8; 32] {
        &self.state.h
    }

    /// Writes our next message, carrying `payload`, into `out`.
    pub fn write_message(&mut self, payload: &[u8], out: &mut [u8]) -> Result<usize, Error> {
        if !self.is_my_turn() {
            return Err(Error::WrongTurn);
        }

        let mut len = 0;
        for token in self.pattern.messages()[self.step] {
            let rest = out.get_mut(len..).ok_or(Error::BufferTooSmall)?;
            len += match token {
                Token::E => {
                    let public = *self.e.public();
                    rest.get_mut(..KEY_LEN)
                        .ok_or(Error::BufferTooSmall)?
                        .copy_from_slice(&public);
                    self.mix_ephemeral(&public);
                    KEY_LEN
                }
                Token::Psk => {
                    self.mix_psk()?;
                    0
                }
                Token::S => {
                    let public = *self.s.public();
                    self.state.encrypt_and_hash(&public, rest)?
                }
                dh => {
                    let shared = self.dh(*dh)?;
                    self.state.mix_key(&shared);
                    0
                }
            };
        }
        let rest = out.get_mut(len..).ok_or(Error::BufferTooSmall)?;
        len += self.state.encrypt_and_hash(payload, rest)?;
        if len > MAX_MESSAGE {
            return Err(Error::BadLength);
        }

        self.step += 1;
        Ok(len)
    }

    /// Reads the peer's next message into `payload`, returning its length.
    /// A failed read leaves the handshake unusable: start a new one.
    pub fn read_message(&mut self, message: &[u8], payload: &mut [u8]) -> Result<usize, Error> {
        if self.is_finished() || self.is_my_turn() {
            return Err(Error::WrongTurn);
        }
        if message.len() > MAX_MESSAGE {
            return Err(Error::BadLength);
        }

        let mut at = 0;
        for token in self.pattern.messages()[self.step] {
            match token {
                Token::E => {
                    let public = take_key(message, at)?;
                    self.mix_ephemeral(&public);
                    self.re = Some(public);
                    at += KEY_LEN;
                }
                Token::Psk => self.mix_psk()?,
                Token::S => {
                    let len = self.state.sealed_len(KEY_LEN);
                    let sealed = message.get(at..at + len).ok_or(Error::BadLength)?;
                    let mut public = [0; KEY_LEN];
                    self.state.decrypt_and_hash(sealed, &mut public)?;
                    self.rs = Some(public);
                    at += len;
                }
                dh => {
                    let shared = self.dh(*dh)?;
                    self.state.mix_key(&shared);
                }
            }
        }
        let rest = message.get(at..).ok_or(Error::BadLength)?;
        let len = self.state.decrypt_and_hash(rest, payload)?;

        self.step += 1;
        Ok(len)
    }

    /// The transport keys, once the last message is written or read.
    pub fn into_transport(self) -> Result<Transport, Error> {
        if !self.is_finished() {
            return Err(Error::Unfinished);
        }
        let (initiator_send, responder_send) = self.state.split();
        let (send, receive) = match self.role {
            Role::Initiator => (initiator_send, responder_send),
            Role::Responder => (responder_send, initiator_send),
        };
        Ok(Transport {
            send,
            receive,
            remote_static: self.rs,
            handshake_hash: self.state.h,
        })
    }

    fn mix_ephemeral(&mut self, public: &[u8; KEY_LEN]) {
        self.state.mix_hash(public);
        if self.pattern.has_psk() {
            self.state.mix_key(public);
        }
    }

    fn mix_psk(&mut self) -> Result<(), Error> {
        let psk = self.psk.ok_or(Error::MissingPsk)?;
        self.state.mix_key_and_hash(&psk);
        Ok(())
    }

    fn dh(&self, token: Token) -> Result<[u8; 32], Error> {
        let missing = Error::WrongTurn;
        let initiator = self.role == Role::Initiator;
        let re = self.re.as_ref().ok_or(missing);
        let rs = self.rs.as_ref().ok_or(missing);
        match token {
            Token::Ee => self.e.dh(re?),
            Token::Ss => self.s.dh(rs?),
            Token::Es if initiator => self.e.dh(rs?),
            Token::Es => self.s.dh(re?),
            Token::Se if initiator => self.s.dh(re?),
            Token::Se => self.e.dh(rs?),
            Token::E | Token::S | Token::Psk => Err(missing),
        }
    }
}

fn take_key(message: &[u8], at: usize) -> Result<[u8; KEY_LEN], Error> {
    let bytes = message.get(at..at + KEY_LEN).ok_or(Error::BadLength)?;
    let mut key = [0; KEY_LEN];
    key.copy_from_slice(bytes);
    Ok(key)
}

/// The finished session: one key and counter in each direction.
pub struct Transport {
    send: CipherState,
    receive: CipherState,
    remote_static: Option<[u8; KEY_LEN]>,
    handshake_hash: [u8; 32],
}

impl Transport {
    pub const fn remote_static(&self) -> Option<&[u8; KEY_LEN]> {
        self.remote_static.as_ref()
    }

    pub const fn handshake_hash(&self) -> &[u8; 32] {
        &self.handshake_hash
    }

    /// Encrypts `payload` into `out` (`payload.len() + TAG_LEN` bytes).
    pub fn seal(&mut self, payload: &[u8], out: &mut [u8]) -> Result<usize, Error> {
        if payload.len() + TAG_LEN > MAX_MESSAGE {
            return Err(Error::BadLength);
        }
        self.send.encrypt(&[], payload, out)
    }

    /// Decrypts `message` into `out`. Messages must arrive in order; a
    /// failure means the session is broken and must end.
    pub fn open(&mut self, message: &[u8], out: &mut [u8]) -> Result<usize, Error> {
        if message.len() > MAX_MESSAGE {
            return Err(Error::BadLength);
        }
        self.receive.decrypt(&[], message, out)
    }
}
