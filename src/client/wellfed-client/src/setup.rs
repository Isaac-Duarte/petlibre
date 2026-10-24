//! FW45 BLE setup, the client's side ([`wellfed_proto::setup`]).
//!
//! One [`Session`] per connection, as the Noise XX initiator:
//! 1. [`Session::start`] gives the first handshake message.
//! 2. The feeder answers with its static key. [`Session::receive`] then
//!    gives the third message, which carries the [`Ask`] encrypted, and the
//!    feeder's key is known ([`Session::feeder_key`]).
//! 3. Each later message is a [`Reply`]: `Confirming` while the feeder waits
//!    for a Dispense press, then `Done` or `Refused`, which end the session.
//!
//! Every message travels in one length-prefixed frame on the setup
//! characteristic ([`frame`], [`Deframer`]); notifications may split a frame.

use wellfed_core::ble_setup::{self, PREFIX};
use wellfed_core::noise::{self, Handshake, Keypair, Pattern, Role, Transport};
use wellfed_proto::setup::{LABEL_MAX, PROLOGUE, Reply, Request, Wifi};

pub use wellfed_core::ble_setup::{CONTROL, MAX_MESSAGE, SERVICE};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the label must be 1-{LABEL_MAX} bytes with no NUL")]
    Label,
    #[error("the SSID must be 1-32 bytes and the passphrase 8-63 printable ASCII characters")]
    Credentials,
    #[error(
        "the handshake failed ({0:?}): not a feeder in setup mode, or a different protocol version"
    )]
    Noise(noise::Error),
    #[error("the feeder sent a message the session did not expect")]
    Unexpected,
    #[error("the feeder's message does not decode")]
    Malformed,
    #[error("a frame announces {0} bytes, more than the setup characteristic carries")]
    TooLong(usize),
}

impl From<noise::Error> for Error {
    fn from(error: noise::Error) -> Self {
        Self::Noise(error)
    }
}

/// What the client asks of the feeder.
#[derive(Clone, PartialEq, Eq)]
pub struct Ask {
    pub label: String,
    pub wifi: Option<Credentials>,
}

/// Wi-Fi for the feeder's next join. The passphrase never appears in
/// `Debug` output.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    ssid: Vec<u8>,
    psk: String,
}

impl Credentials {
    /// The feeder's own rules: a 1-32 byte SSID and an 8-63 character
    /// printable-ASCII passphrase.
    pub fn new(ssid: Vec<u8>, psk: String) -> Result<Self, Error> {
        let ssid_ok = (1..=32).contains(&ssid.len());
        let psk_ok =
            (8..=63).contains(&psk.len()) && psk.bytes().all(|b| (0x20..0x7f).contains(&b));
        if !(ssid_ok && psk_ok) {
            return Err(Error::Credentials);
        }
        Ok(Self { ssid, psk })
    }

    pub fn ssid(&self) -> &[u8] {
        &self.ssid
    }
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("ssid", &String::from_utf8_lossy(&self.ssid))
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for Ask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ask")
            .field("label", &self.label)
            .field("wifi", &self.wifi)
            .finish()
    }
}

/// What to do after a message from the feeder.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// Send these bytes (unframed) to the feeder.
    Send(Vec<u8>),
    /// The feeder replied; `Done` and `Refused` end the session.
    Reply(Reply),
}

enum State {
    Handshake(Box<Handshake>),
    Open(Box<Transport>),
    Ended,
}

pub struct Session {
    state: State,
    ask: Ask,
    feeder_key: Option<[u8; 32]>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("ask", &self.ask)
            .finish_non_exhaustive()
    }
}

impl Session {
    /// `client` is the client's long-term key; `ephemeral` a fresh secret
    /// from the OS, never used before.
    pub fn new(client: &Keypair, ephemeral: [u8; 32], ask: Ask) -> Result<Self, Error> {
        let label = ask.label.as_bytes();
        if label.is_empty() || label.len() > LABEL_MAX || label.contains(&0) {
            return Err(Error::Label);
        }
        let handshake = Handshake::new(
            Pattern::Xx,
            Role::Initiator,
            PROLOGUE,
            client,
            None,
            ephemeral,
        )?;
        Ok(Self {
            state: State::Handshake(Box::new(handshake)),
            ask,
            feeder_key: None,
        })
    }

    /// The feeder's static key, once its handshake message has arrived.
    pub fn feeder_key(&self) -> Option<[u8; 32]> {
        self.feeder_key
    }

    /// The first handshake message.
    pub fn start(&mut self) -> Result<Vec<u8>, Error> {
        let State::Handshake(handshake) = &mut self.state else {
            return Err(Error::Unexpected);
        };
        let mut out = [0u8; MAX_MESSAGE];
        let len = handshake.write_message(&[], &mut out)?;
        Ok(out[..len].to_vec())
    }

    /// One message from the feeder.
    pub fn receive(&mut self, message: &[u8]) -> Result<Step, Error> {
        match std::mem::replace(&mut self.state, State::Ended) {
            State::Handshake(handshake) => self.second(*handshake, message),
            State::Open(mut transport) => {
                let mut plain = [0u8; MAX_MESSAGE];
                let len = transport.open(message, &mut plain)?;
                let reply: Reply =
                    wellfed_proto::decode(&plain[..len]).map_err(|_| Error::Malformed)?;
                if matches!(reply, Reply::Confirming { .. }) {
                    self.state = State::Open(transport);
                }
                Ok(Step::Reply(reply))
            }
            State::Ended => Err(Error::Unexpected),
        }
    }

    /// The feeder's handshake message: learn its key, send the ask.
    fn second(&mut self, mut handshake: Handshake, message: &[u8]) -> Result<Step, Error> {
        let mut ignored = [0u8; MAX_MESSAGE];
        handshake.read_message(message, &mut ignored)?;
        self.feeder_key = handshake.remote_static().copied();

        let request = Request {
            label: &self.ask.label,
            wifi: self.ask.wifi.as_ref().map(|wifi| Wifi {
                ssid: &wifi.ssid,
                psk: &wifi.psk,
            }),
        };
        let mut payload = [0u8; MAX_MESSAGE];
        let len = wellfed_proto::encode(&request, &mut payload).map_err(|_| Error::Malformed)?;
        let mut out = [0u8; MAX_MESSAGE];
        let sent = handshake.write_message(&payload[..len], &mut out)?;

        self.state = State::Open(Box::new(handshake.into_transport()?));
        Ok(Step::Send(out[..sent].to_vec()))
    }
}

/// One message as a frame: its 2-byte little-endian length, then the bytes.
pub fn frame(message: &[u8]) -> Result<Vec<u8>, Error> {
    let prefix = ble_setup::prefix(message.len()).ok_or(Error::TooLong(message.len()))?;
    let mut out = prefix.to_vec();
    out.extend_from_slice(message);
    Ok(out)
}

/// Reassembles frames from notifications, which may split or join them.
#[derive(Debug, Default)]
pub struct Deframer {
    buffer: Vec<u8>,
}

impl Deframer {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// The next whole message, if one has arrived.
    pub fn next_message(&mut self) -> Result<Option<Vec<u8>>, Error> {
        let Some(prefix) = self.buffer.first_chunk::<PREFIX>() else {
            return Ok(None);
        };
        let announced = usize::from(u16::from_le_bytes(*prefix));
        let len = ble_setup::parse_prefix(*prefix).ok_or(Error::TooLong(announced))?;
        if self.buffer.len() < PREFIX + len {
            return Ok(None);
        }
        let message = self.buffer[PREFIX..PREFIX + len].to_vec();
        self.buffer.drain(..PREFIX + len);
        Ok(Some(message))
    }
}
