//! BLE setup, the feeder's side of one session ([`petlibre_proto::setup`]).
//!
//! The firmware hands each message from the setup characteristic to a
//! [`Responder`] and sends what it returns:
//! 1. The responder runs Noise XX under the feeder's identity key and, after
//!    the third message, yields the client's [`Offer`]: its static key and a
//!    label the client store accepts.
//! 2. Once pairing has collected the owner's code from the buttons, the
//!    firmware starts the proof with [`Responder::prove`], and the
//!    responder answers each of the client's steps
//!    (`petlibre_core::setup_code`).
//! 3. When the client's last step opens, the client holds the code under
//!    this handshake: [`Received::Proven`] carries its Wi-Fi, checked
//!    against the Wi-Fi store's rules.
//!
//! The firmware seals the outcome with [`Responder::reply`].
//!
//! Sans-IO, with no allocation: the host client's tests drive this same code
//! against `petlibre-client`'s initiator.

use petlibre_core::noise::{self, Handshake, Keypair, Pattern, Role, Transport};
use petlibre_core::setup_code::{self, Code, Exchange, Round, Side};
use petlibre_core::store::clients::Label;
use petlibre_core::store::net::Credentials;
use petlibre_proto::setup::{Answer, PROLOGUE, REPLY_MAX, Reply, Request};

/// The largest message either way, as the setup characteristic frames it.
pub const MAX_MESSAGE: usize = petlibre_core::ble_setup::MAX_MESSAGE;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The handshake or a sealed message failed: a foreign, tampered or
    /// misordered message, or a client on another protocol version.
    Noise(noise::Error),
    /// A message the session's stage does not take.
    Unexpected,
    /// The request does not decode.
    Malformed,
    /// The label or credentials fail the stores' checks.
    Invalid,
    /// The client's proof does not open against the code keyed in here.
    WrongCode,
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
            setup_code::Error::Malformed => Self::Malformed,
            setup_code::Error::WrongTurn => Self::Unexpected,
        }
    }
}

/// Who the new client is, checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Offer {
    pub key: [u8; 32],
    pub label: Label,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Received {
    /// Send the first `n` bytes of the output buffer.
    Send(usize),
    /// The handshake is complete and the client has asked; it waits for
    /// the owner's code.
    Offer(Offer),
    /// The client proved the code: it is the one the owner set up.
    Proven { wifi: Option<Credentials> },
}

enum State {
    Handshake(Handshake),
    /// The offer is made; the code is not in yet.
    Asked(Transport),
    Proving(Transport, Exchange),
    Proven(Transport),
    /// A failed step leaves nothing to resume; the link must close.
    Failed,
}

pub struct Responder {
    state: State,
    /// A message's plaintext. Here rather than on the caller's stack, which
    /// the handshake's X25519 already fills on the feeder.
    payload: [u8; MAX_MESSAGE],
}

impl core::fmt::Debug for Responder {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let state = match self.state {
            State::Handshake(_) => "handshake",
            State::Asked(_) => "asked",
            State::Proving(..) => "proving",
            State::Proven(_) => "proven",
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

    /// The owner keyed `code` in: write the proof's first step into `out`.
    /// `seed` comes from the feeder's generator, fresh for this proof.
    pub fn prove(&mut self, code: Code, seed: [u8; 32], out: &mut [u8]) -> Result<usize, Error> {
        let State::Asked(transport) = core::mem::replace(&mut self.state, State::Failed) else {
            return Err(Error::Unexpected);
        };
        let hash = *transport.handshake_hash();
        self.state = State::Proving(transport, Exchange::new(Side::Feeder, hash, code, seed));
        let result = self.step_out(out);
        if result.is_err() {
            self.state = State::Failed;
        }
        result
    }

    /// One message from the client; any error ends the session.
    pub fn receive(&mut self, message: &[u8], out: &mut [u8]) -> Result<Received, Error> {
        let result = self.step(message, out);
        // The plaintext may hold the passphrase; keep it no longer than
        // needed.
        self.payload.fill(0);
        if result.is_err() {
            self.state = State::Failed;
        }
        result
    }

    fn step(&mut self, message: &[u8], out: &mut [u8]) -> Result<Received, Error> {
        match &mut self.state {
            State::Handshake(_) => self.handshake(message, out),
            State::Proving(transport, exchange) => {
                let len = transport.open(message, &mut self.payload)?;
                let answer: Answer<'_> =
                    petlibre_proto::decode(&self.payload[..len]).map_err(|_| Error::Malformed)?;
                let (round, wifi) = match answer {
                    Answer::Round { open, commit } => (Round { open, commit }, None),
                    Answer::Last { open, wifi } => {
                        let round = Round {
                            open: Some(open),
                            commit: None,
                        };
                        (round, Some(wifi))
                    }
                };
                exchange.read(round)?;

                match (exchange.peer_proven(), wifi) {
                    (false, None) => Ok(Received::Send(self.step_out(out)?)),
                    (true, Some(wifi)) => {
                        let wifi = wifi
                            .map(|w| {
                                Credentials::new(w.ssid, w.psk.as_bytes()).ok_or(Error::Invalid)
                            })
                            .transpose()?;
                        let State::Proving(transport, _) =
                            core::mem::replace(&mut self.state, State::Failed)
                        else {
                            return Err(Error::Unexpected);
                        };
                        self.state = State::Proven(transport);
                        Ok(Received::Proven { wifi })
                    }
                    // The last step must carry the Wi-Fi, and only it may.
                    _ => Err(Error::Malformed),
                }
            }
            State::Asked(_) | State::Proven(_) | State::Failed => Err(Error::Unexpected),
        }
    }

    fn handshake(&mut self, message: &[u8], out: &mut [u8]) -> Result<Received, Error> {
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
        self.state = State::Asked(handshake.into_transport()?);
        Ok(Received::Offer(offer))
    }

    /// Our next step of the proof, sealed into `out`.
    fn step_out(&mut self, out: &mut [u8]) -> Result<usize, Error> {
        let State::Proving(_, exchange) = &mut self.state else {
            return Err(Error::Unexpected);
        };
        let Round { open, commit } = exchange.write()?;
        self.reply(&Reply::Round { open, commit }, out)
    }

    /// Seal one reply into `out`, after the offer.
    pub fn reply(&mut self, reply: &Reply, out: &mut [u8]) -> Result<usize, Error> {
        let (State::Asked(transport) | State::Proving(transport, _) | State::Proven(transport)) =
            &mut self.state
        else {
            return Err(Error::Unexpected);
        };
        let mut plain = [0u8; REPLY_MAX];
        let len = petlibre_proto::encode(reply, &mut plain).map_err(|_| Error::Malformed)?;
        Ok(transport.seal(&plain[..len], out)?)
    }
}

fn offer(key: [u8; 32], payload: &[u8]) -> Result<Offer, Error> {
    let request: Request<'_> = petlibre_proto::decode(payload).map_err(|_| Error::Malformed)?;
    let label = Label::new(request.label.as_bytes()).ok_or(Error::Invalid)?;
    Ok(Offer { key, label })
}
