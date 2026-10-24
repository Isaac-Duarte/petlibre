//! FW45 BLE setup, the client's side ([`petlibre_proto::setup`]).
//!
//! One [`Session`] per connection, as the Noise XX initiator:
//! 1. [`Session::start`] gives the first handshake message.
//! 2. The feeder answers with its static key. [`Session::receive`] then
//!    gives the third message, which carries the client's label.
//! 3. The feeder replies `EnterCode`: show the owner [`Session::code`] to
//!    key in on the feeder's buttons ([`panel_position`]).
//! 4. Once the code is in, the feeder and the session prove it to each
//!    other one press at a time (`petlibre_core::setup_code`); the session
//!    answers each step itself.
//! 5. Only after the feeder has proven the code does the session send the
//!    Wi-Fi and accept `Done`, and only then is the feeder's key known
//!    ([`Session::feeder_key`]). `Refused` ends the session at any point.
//!
//! The proof is what makes the feeder's key trustworthy: XX alone would
//! accept whichever device answered, including a relay in radio range.
//!
//! Every message travels in one length-prefixed frame on the setup
//! characteristic ([`frame`], [`Deframer`]); notifications may split a frame.

use petlibre_core::ble_setup::{self, PREFIX};
use petlibre_core::buttons::Button;
use petlibre_core::noise::{self, Handshake, Keypair, Pattern, Role, Transport};
use petlibre_core::setup_code::{self, Code, Exchange, Round, Side};
use petlibre_proto::setup::{Answer, LABEL_MAX, PROLOGUE, Reply, Request, Wifi};

pub use petlibre_core::ble_setup::{CONTROL, MAX_MESSAGE, SERVICE};

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
    #[error(
        "the feeder did not prove the code: the code was mistyped, or another device is answering or relaying"
    )]
    WrongCode,
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

impl From<setup_code::Error> for Error {
    fn from(error: setup_code::Error) -> Self {
        match error {
            setup_code::Error::Mismatch => Self::WrongCode,
            setup_code::Error::Malformed | setup_code::Error::WrongTurn => Self::Unexpected,
        }
    }
}

/// Where a button sits on the PLAF103's panel, counting from 1 at the left
/// as the owner faces it (Lock, Dispense, Mic, Reset).
pub fn panel_position(button: Button) -> u8 {
    match button {
        Button::Lock => 1,
        Button::Dispense => 2,
        Button::Mic => 3,
        Button::Reset => 4,
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
    /// The feeder has the label; the code proof runs.
    Proving(Box<Transport>, Box<Exchange>),
    /// The feeder proved the code and has the Wi-Fi; its verdict is next.
    Proven(Box<Transport>),
    Ended,
}

pub struct Session {
    state: State,
    ask: Ask,
    code: Code,
    /// Derives the proof's nonces; consumed when the handshake ends.
    seed: [u8; 32],
    /// The key the feeder's handshake message carried, until it proves the
    /// code.
    pending_key: Option<[u8; 32]>,
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
    /// `client` is the client's long-term key. `ephemeral` and `seed` are
    /// fresh secrets from the OS, never used before, and `code` comes from
    /// uniformly random bits ([`Code::from_bits`]).
    pub fn new(
        client: &Keypair,
        ephemeral: [u8; 32],
        ask: Ask,
        code: Code,
        seed: [u8; 32],
    ) -> Result<Self, Error> {
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
            code,
            seed,
            pending_key: None,
            feeder_key: None,
        })
    }

    /// The code the owner keys in on the feeder.
    pub fn code(&self) -> Code {
        self.code
    }

    /// The feeder's static key, once the feeder has proven the code. Until
    /// then the key could be a relay's.
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
            State::Proving(mut transport, mut exchange) => {
                let reply = open(&mut transport, message)?;
                match reply {
                    Reply::EnterCode { .. } => {
                        self.state = State::Proving(transport, exchange);
                        Ok(Step::Reply(reply))
                    }
                    Reply::Round { open, commit } => {
                        exchange.read(Round { open, commit })?;
                        self.answer(transport, exchange)
                    }
                    Reply::Refused(_) => Ok(Step::Reply(reply)),
                    _ => Err(Error::Unexpected),
                }
            }
            State::Proven(mut transport) => match open(&mut transport, message)? {
                reply @ (Reply::Done { .. } | Reply::Refused(_)) => Ok(Step::Reply(reply)),
                _ => Err(Error::Unexpected),
            },
            State::Ended => Err(Error::Unexpected),
        }
    }

    /// The feeder's handshake message: send the label, start the proof.
    fn second(&mut self, mut handshake: Handshake, message: &[u8]) -> Result<Step, Error> {
        let mut ignored = [0u8; MAX_MESSAGE];
        handshake.read_message(message, &mut ignored)?;
        let key = handshake.remote_static().copied();

        let request = Request {
            label: &self.ask.label,
        };
        let mut payload = [0u8; MAX_MESSAGE];
        let len = petlibre_proto::encode(&request, &mut payload).map_err(|_| Error::Malformed)?;
        let mut out = [0u8; MAX_MESSAGE];
        let sent = handshake.write_message(&payload[..len], &mut out)?;

        let transport = handshake.into_transport()?;
        let hash = *transport.handshake_hash();
        let seed = std::mem::take(&mut self.seed);
        let exchange = Exchange::new(Side::Client, hash, self.code, seed);
        self.pending_key = key;
        self.state = State::Proving(Box::new(transport), Box::new(exchange));
        Ok(Step::Send(out[..sent].to_vec()))
    }

    /// Our next step of the proof; the last one carries the Wi-Fi, now
    /// that the feeder has proven the code.
    fn answer(
        &mut self,
        mut transport: Box<Transport>,
        mut exchange: Box<Exchange>,
    ) -> Result<Step, Error> {
        let Round { open, commit } = exchange.write()?;
        let answer = match (exchange.peer_proven(), open) {
            (true, Some(open)) => Answer::Last {
                open,
                wifi: self.ask.wifi.as_ref().map(|wifi| Wifi {
                    ssid: &wifi.ssid,
                    psk: &wifi.psk,
                }),
            },
            _ => Answer::Round { open, commit },
        };
        let mut plain = [0u8; MAX_MESSAGE];
        let len = petlibre_proto::encode(&answer, &mut plain).map_err(|_| Error::Malformed)?;
        let mut out = [0u8; MAX_MESSAGE];
        let sealed = transport.seal(&plain[..len], &mut out)?;
        plain.fill(0);

        if exchange.peer_proven() {
            self.feeder_key = self.pending_key;
            self.state = State::Proven(transport);
        } else {
            self.state = State::Proving(transport, exchange);
        }
        Ok(Step::Send(out[..sealed].to_vec()))
    }
}

fn open(transport: &mut Transport, message: &[u8]) -> Result<Reply, Error> {
    let mut plain = [0u8; MAX_MESSAGE];
    let len = transport.open(message, &mut plain)?;
    petlibre_proto::decode(&plain[..len]).map_err(|_| Error::Malformed)
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
