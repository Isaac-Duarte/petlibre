//! FW45 BLE setup, the feeder's side of one session ([`wellfed_proto::setup`]).
//!
//! The firmware hands each message from the setup characteristic to a
//! [`Responder`] and sends what it returns. The responder runs Noise XX
//! under the feeder's identity key and, after the third message, yields the
//! client's [`Offer`]: its static key, its label and any Wi-Fi credentials,
//! already checked against the stores' own rules so the pairing policy and
//! the settings writer see only values they accept. Whether to admit the
//! offer is `pairing`'s decision, and the firmware seals the outcome with
//! [`Responder::reply`].
//!
//! Sans-IO, with no allocation: the host client's tests drive this same code
//! against `wellfed-client`'s initiator.

use wellfed_core::noise::{self, Handshake, Keypair, Pattern, Role, Transport};
use wellfed_core::store::clients::Label;
use wellfed_core::store::net::Credentials;
use wellfed_proto::setup::{PROLOGUE, Reply, Request};

/// The largest message either way, as the setup characteristic frames it.
pub const MAX_MESSAGE: usize = wellfed_core::ble_setup::MAX_MESSAGE;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The handshake or a sealed message failed: a foreign, tampered or
    /// misordered message, or a client on another protocol version.
    Noise(noise::Error),
    /// A message arrived after the offer: the client speaks only once.
    Unexpected,
    /// The request does not decode.
    Malformed,
    /// The label or credentials fail the stores' checks.
    Invalid,
}

impl From<noise::Error> for Error {
    fn from(error: noise::Error) -> Self {
        Self::Noise(error)
    }
}

/// What a new client asks for, checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Offer {
    pub key: [u8; 32],
    pub label: Label,
    pub wifi: Option<Credentials>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Received {
    /// Send the first `n` bytes of the output buffer.
    Send(usize),
    /// The handshake is complete and the client has asked.
    Offer(Offer),
}

enum State {
    Handshake(Handshake),
    Open(Transport),
    /// A failed step leaves nothing to resume; the link must close.
    Failed,
}

pub struct Responder {
    state: State,
    /// The third message's plaintext. Here rather than on the caller's
    /// stack, which the handshake's X25519 already fills on the feeder.
    payload: [u8; MAX_MESSAGE],
}

impl core::fmt::Debug for Responder {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let state = match self.state {
            State::Handshake(_) => "handshake",
            State::Open(_) => "open",
            State::Failed => "failed",
        };
        f.debug_struct("Responder").field("state", &state).finish()
    }
}

impl Responder {
    /// No session yet: every message is unexpected until [`Self::restart`].
    /// The feeder keeps one responder and restarts it in place for each
    /// client, since moving one costs the loop's stack a kilobyte.
    pub const fn idle() -> Self {
        Self {
            state: State::Failed,
            payload: [0; MAX_MESSAGE],
        }
    }

    /// `ephemeral` comes from the feeder's generator, fresh for this session.
    pub fn new(identity: &Keypair, ephemeral: [u8; 32]) -> Result<Self, Error> {
        let mut responder = Self::idle();
        responder.restart(identity, ephemeral)?;
        Ok(responder)
    }

    /// Start a new session, forgetting any earlier one.
    pub fn restart(&mut self, identity: &Keypair, ephemeral: [u8; 32]) -> Result<(), Error> {
        self.state = State::Failed;
        let handshake = Handshake::new(
            Pattern::Xx,
            Role::Responder,
            PROLOGUE,
            identity,
            None,
            ephemeral,
        )?;
        self.state = State::Handshake(handshake);
        Ok(())
    }

    /// End the session; later messages are unexpected.
    pub fn stop(&mut self) {
        self.state = State::Failed;
    }

    /// One message from the client; any error ends the session.
    pub fn receive(&mut self, message: &[u8], out: &mut [u8]) -> Result<Received, Error> {
        let result = self.step(message, out);
        // The plaintext held the passphrase; keep it no longer than needed.
        self.payload.fill(0);
        if result.is_err() {
            self.state = State::Failed;
        }
        result
    }

    fn step(&mut self, message: &[u8], out: &mut [u8]) -> Result<Received, Error> {
        let State::Handshake(handshake) = &mut self.state else {
            return Err(Error::Unexpected);
        };

        let read = handshake.read_message(message, &mut self.payload)?;
        if !handshake.is_finished() {
            // The first message: answer with the second, which carries the
            // feeder's static key and no payload.
            return Ok(Received::Send(handshake.write_message(&[], out)?));
        }

        let key = *handshake.remote_static().ok_or(Error::Unexpected)?;
        let offer = offer(key, &self.payload[..read])?;
        let State::Handshake(handshake) = core::mem::replace(&mut self.state, State::Failed) else {
            return Err(Error::Unexpected);
        };
        self.state = State::Open(handshake.into_transport()?);
        Ok(Received::Offer(offer))
    }

    /// Seal one reply into `out`, after the offer.
    pub fn reply(&mut self, reply: &Reply, out: &mut [u8]) -> Result<usize, Error> {
        let State::Open(transport) = &mut self.state else {
            return Err(Error::Unexpected);
        };
        let mut plain = [0u8; 16];
        let len = wellfed_proto::encode(reply, &mut plain).map_err(|_| Error::Malformed)?;
        Ok(transport.seal(&plain[..len], out)?)
    }
}

fn offer(key: [u8; 32], payload: &[u8]) -> Result<Offer, Error> {
    let request: Request<'_> = wellfed_proto::decode(payload).map_err(|_| Error::Malformed)?;
    let label = Label::new(request.label.as_bytes()).ok_or(Error::Invalid)?;
    let wifi = match request.wifi {
        None => None,
        Some(wifi) => Some(Credentials::new(wifi.ssid, wifi.psk.as_bytes()).ok_or(Error::Invalid)?),
    };
    Ok(Offer { key, label, wifi })
}
