//! Device API, the client's side ([`wellfed_proto::api`]).
//!
//! The caller owns the TCP connection and moves bytes; this module frames,
//! seals and checks them:
//! - [`Session::connect`] opens an enrolled client's session (Noise IK
//!   against the feeder's known identity). Write its bytes, then feed what
//!   arrives to [`Session::receive`] and drain [`Session::next_incoming`]: the first
//!   message is the feeder's [`Welcome`], then responses, events and
//!   samples.
//! - [`Session::request`] seals one request under the caller's id. A write
//!   whose answer never arrived is retried with the same id (after a
//!   reconnect too), so the feeder answers it once.
//! - [`Join::connect`] redeems an invite (Noise XXpsk3), checking that the
//!   feeder that answers holds the identity the invite named.
//!
//! [`crate::connection::Connection`] runs a whole session over a
//! [`Session`]: the clients use it, and the bench and tests use
//! [`Session`] directly to send what a well-behaved client never would.

use wellfed_core::frame::{self, PREFIX};
use wellfed_core::hex::{self, Lower};
use wellfed_core::noise::{self, Handshake, Keypair, Pattern, Role, Transport};
use wellfed_proto::api::{
    Capabilities, ClientMessage, FRAME_MAX, FeederMessage, Hello, Join as JoinRequest, Joined, Op,
    Open, OpenRefusal, PROLOGUE, PROTOCOL_MAJOR, RequestId, Welcome,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the handshake failed ({0:?}): the wrong feeder, or a different protocol version")]
    Noise(noise::Error),
    #[error("the feeder sent a message the session did not expect")]
    Unexpected,
    #[error("the feeder's message does not decode")]
    Malformed,
    #[error("a frame announces {0} bytes, more than the device API carries")]
    TooLong(usize),
    #[error("the message does not fit one frame")]
    TooLarge,
    #[error("the feeder that answered is not the one the invite names")]
    WrongFeeder,
    #[error("the label must be 1-16 bytes with no NUL")]
    Label,
    #[error("the feeder refused the session: {0:?}")]
    Refused(OpenRefusal),
    #[error("the feeder did not answer in time")]
    Timeout,
    #[error("the feeder stopped answering")]
    Silent,
    #[error("that request id is the keepalive's")]
    ReservedId,
    #[error("the session is not open yet")]
    NotOpen,
    #[error(transparent)]
    Setup(#[from] crate::setup::Error),
}

impl From<noise::Error> for Error {
    fn from(error: noise::Error) -> Self {
        Self::Noise(error)
    }
}

/// One frame around `message`.
pub fn frame(message: &[u8]) -> Result<Vec<u8>, Error> {
    let prefix = frame::prefix(message.len(), FRAME_MAX).ok_or(Error::TooLarge)?;
    let mut out = prefix.to_vec();
    out.extend_from_slice(message);
    Ok(out)
}

/// Reassembles frames from a byte stream, which may split or join them.
#[derive(Debug, Default)]
struct Deframer {
    buffer: Vec<u8>,
}

impl Deframer {
    fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    fn next_frame(&mut self) -> Result<Option<Vec<u8>>, Error> {
        let Some(prefix) = self.buffer.first_chunk::<PREFIX>() else {
            return Ok(None);
        };
        let announced = usize::from(u16::from_le_bytes(*prefix));
        let len = frame::parse_prefix(*prefix, FRAME_MAX).ok_or(Error::TooLong(announced))?;
        if self.buffer.len() < PREFIX + len {
            return Ok(None);
        }
        let message = self.buffer[PREFIX..PREFIX + len].to_vec();
        self.buffer.drain(..PREFIX + len);
        Ok(Some(message))
    }
}

/// The clear frame that opens a connection.
pub(crate) fn open_frame(open: Open) -> Result<Vec<u8>, Error> {
    let mut out = [0; 16];
    let len = wellfed_proto::encode(&open, &mut out).map_err(|_| Error::TooLarge)?;
    frame(&out[..len])
}

enum State {
    Handshake(Box<Handshake>),
    Open(Box<Transport>),
    Ended,
}

/// What [`Session::next_incoming`] yields.
#[derive(Debug, PartialEq, Eq)]
#[expect(
    clippy::large_enum_variant,
    reason = "borrowed from the session, never stored"
)]
pub enum Incoming<'a> {
    Welcome(Welcome<'a>),
    Message(FeederMessage<'a>),
}

/// An enrolled client's session.
pub struct Session {
    state: State,
    deframer: Deframer,
    /// Opened messages waiting for [`Self::next_incoming`], oldest first.
    inbox: std::collections::VecDeque<Vec<u8>>,
    /// The last message [`Self::next_incoming`] returned; its borrow lives here.
    current: Vec<u8>,
    welcomed: bool,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("open", &matches!(self.state, State::Open(_)))
            .finish_non_exhaustive()
    }
}

impl Session {
    /// Open a session to the feeder whose identity is `feeder`; the bytes to
    /// write first. `ephemeral` comes from the OS generator, fresh.
    pub fn connect(
        keys: &Keypair,
        feeder: [u8; 32],
        ephemeral: [u8; 32],
    ) -> Result<(Self, Vec<u8>), Error> {
        let hello = Hello {
            major: PROTOCOL_MAJOR,
            capabilities: Capabilities::KNOWN,
        };
        Self::connect_with(keys, feeder, ephemeral, hello)
    }

    /// As [`Self::connect`] with another hello: what a client built before
    /// some capability, or for another major version, looks like to the
    /// feeder.
    pub fn connect_with(
        keys: &Keypair,
        feeder: [u8; 32],
        ephemeral: [u8; 32],
        hello: Hello,
    ) -> Result<(Self, Vec<u8>), Error> {
        let mut handshake = Handshake::new(
            Pattern::Ik,
            Role::Initiator,
            PROLOGUE,
            keys,
            Some(feeder),
            ephemeral,
        )?;
        let mut payload = [0; 16];
        let len = wellfed_proto::encode(&hello, &mut payload).map_err(|_| Error::TooLarge)?;
        let mut message = [0; FRAME_MAX];
        let written = handshake.write_message(&payload[..len], &mut message)?;

        let mut out = open_frame(Open::Session)?;
        out.extend(frame(&message[..written])?);
        let session = Self {
            state: State::Handshake(Box::new(handshake)),
            deframer: Deframer::default(),
            inbox: std::collections::VecDeque::new(),
            current: Vec::new(),
            welcomed: false,
        };
        Ok((session, out))
    }

    pub fn is_open(&self) -> bool {
        matches!(self.state, State::Open(_))
    }

    /// Bytes from the connection. Any error ends the session.
    pub fn receive(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let result = self.take(bytes);
        if result.is_err() {
            self.state = State::Ended;
        }
        result
    }

    fn take(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.deframer.push(bytes);
        while let Some(message) = self.deframer.next_frame()? {
            let mut plain = vec![0; FRAME_MAX];
            let len = match &mut self.state {
                State::Handshake(handshake) => {
                    let len = handshake.read_message(&message, &mut plain)?;
                    let State::Handshake(handshake) =
                        std::mem::replace(&mut self.state, State::Ended)
                    else {
                        return Err(Error::Unexpected);
                    };
                    self.state = State::Open(Box::new(handshake.into_transport()?));
                    len
                }
                State::Open(transport) => transport.open(&message, &mut plain)?,
                State::Ended => return Err(Error::Unexpected),
            };
            plain.truncate(len);
            self.inbox.push_back(plain);
        }
        Ok(())
    }

    /// The next message, decoded; it borrows the session until the next
    /// call.
    pub fn next_incoming(&mut self) -> Result<Option<Incoming<'_>>, Error> {
        let Some(plain) = self.inbox.pop_front() else {
            return Ok(None);
        };
        self.current = plain;
        if !self.welcomed {
            self.welcomed = true;
            let welcome: Welcome<'_> =
                wellfed_proto::decode(&self.current).map_err(|_| Error::Malformed)?;
            if matches!(welcome, Welcome::Refused(_)) {
                self.state = State::Ended;
            }
            return Ok(Some(Incoming::Welcome(welcome)));
        }
        let message = wellfed_proto::decode(&self.current).map_err(|_| Error::Malformed)?;
        Ok(Some(Incoming::Message(message)))
    }

    /// One request under `id`, sealed and framed.
    pub fn request(&mut self, id: RequestId, op: Op<'_>) -> Result<Vec<u8>, Error> {
        self.send(&ClientMessage::Request { id, op })
    }

    /// Ask for every event after `after` and then each new one, and the live
    /// samples if `samples`.
    pub fn subscribe(&mut self, after: Option<u64>, samples: bool) -> Result<Vec<u8>, Error> {
        self.send(&ClientMessage::Subscribe { after, samples })
    }

    fn send(&mut self, message: &ClientMessage<'_>) -> Result<Vec<u8>, Error> {
        let State::Open(transport) = &mut self.state else {
            return Err(Error::Unexpected);
        };
        let mut plain = [0; FRAME_MAX];
        let len = wellfed_proto::encode(message, &mut plain).map_err(|_| Error::TooLarge)?;
        let mut sealed = [0; FRAME_MAX];
        let sealed_len = transport.seal(&plain[..len], &mut sealed)?;
        frame(&sealed[..sealed_len])
    }
}

/// An invite as the issuing client shows it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Invite {
    /// The feeder's identity key.
    pub feeder: [u8; 32],
    pub id: u32,
    pub secret: [u8; 32],
}

impl std::fmt::Debug for Invite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Invite")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// The text form's start; the digit is the form's version.
pub const INVITE_PREFIX: &str = "wellfed-invite:1:";

/// Why a text is not an invite.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum InviteText {
    #[error("not a wellfed invite")]
    Prefix,
    #[error("an invite has a feeder, an id and a secret")]
    Parts,
    #[error("the invite's feeder key is not 32 bytes of lowercase hex")]
    Feeder,
    #[error("the invite's id is not a number")]
    Id,
    #[error("the invite's secret is not 32 bytes of lowercase hex")]
    Secret,
}

impl Invite {
    /// The invite as one line of text (`wellfed-invite:1:<feeder>:<id>:<secret>`),
    /// as the issuing client shows it and a redeeming one takes it. It holds
    /// the secret, so it is shown once and never logged.
    pub fn text(&self) -> String {
        format!(
            "{INVITE_PREFIX}{}:{}:{}",
            Lower(&self.feeder),
            self.id,
            Lower(&self.secret)
        )
    }

    pub fn parse(text: &str) -> Result<Self, InviteText> {
        let rest = text
            .trim()
            .strip_prefix(INVITE_PREFIX)
            .ok_or(InviteText::Prefix)?;
        let mut parts = rest.split(':');
        let (Some(feeder), Some(id), Some(secret), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(InviteText::Parts);
        };

        Ok(Self {
            feeder: hex::decode_array(feeder.as_bytes()).ok_or(InviteText::Feeder)?,
            id: id.parse().map_err(|_| InviteText::Id)?,
            secret: hex::decode_array(secret.as_bytes()).ok_or(InviteText::Secret)?,
        })
    }
}

/// What a redemption step asks of the caller.
#[derive(Debug, PartialEq, Eq)]
pub enum JoinStep {
    /// Write these bytes and wait for more.
    Send(Vec<u8>),
    /// The feeder's answer; the connection is done.
    Done(Joined),
}

/// A new client redeeming an invite.
pub struct Join {
    handshake: Option<Box<Handshake>>,
    transport: Option<Box<Transport>>,
    deframer: Deframer,
    invite: Invite,
    label: String,
}

impl std::fmt::Debug for Join {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Join").finish_non_exhaustive()
    }
}

impl Join {
    /// Start redeeming `invite` as `label`; the bytes to write first.
    pub fn connect(
        keys: &Keypair,
        invite: Invite,
        label: &str,
        ephemeral: [u8; 32],
    ) -> Result<(Self, Vec<u8>), Error> {
        if label.is_empty() || label.len() > 16 || label.contains('\0') {
            return Err(Error::Label);
        }
        let mut handshake = Handshake::new(
            Pattern::XxPsk3,
            Role::Initiator,
            PROLOGUE,
            keys,
            None,
            ephemeral,
        )?;
        handshake.set_psk(invite.secret);
        let mut message = [0; 64];
        let written = handshake.write_message(&[], &mut message)?;

        let mut out = open_frame(Open::Invite { id: invite.id })?;
        out.extend(frame(&message[..written])?);
        let join = Self {
            handshake: Some(Box::new(handshake)),
            transport: None,
            deframer: Deframer::default(),
            invite,
            label: label.to_owned(),
        };
        Ok((join, out))
    }

    /// Bytes from the connection: what to write next, or the answer.
    pub fn receive(&mut self, bytes: &[u8]) -> Result<Option<JoinStep>, Error> {
        self.deframer.push(bytes);
        let Some(message) = self.deframer.next_frame()? else {
            return Ok(None);
        };
        let mut plain = [0; FRAME_MAX];

        if let Some(mut handshake) = self.handshake.take() {
            handshake.read_message(&message, &mut plain)?;
            if handshake.remote_static() != Some(&self.invite.feeder) {
                return Err(Error::WrongFeeder);
            }
            let join = JoinRequest { label: &self.label };
            let mut payload = [0; 32];
            let len = wellfed_proto::encode(&join, &mut payload).map_err(|_| Error::TooLarge)?;
            let mut out = [0; 256];
            let written = handshake.write_message(&payload[..len], &mut out)?;
            self.transport = Some(Box::new(handshake.into_transport()?));
            return Ok(Some(JoinStep::Send(frame(&out[..written])?)));
        }

        let transport = self.transport.as_mut().ok_or(Error::Unexpected)?;
        let len = transport.open(&message, &mut plain)?;
        let joined = wellfed_proto::decode(&plain[..len]).map_err(|_| Error::Malformed)?;
        Ok(Some(JoinStep::Done(joined)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_invite_survives_its_text_form() {
        let invite = Invite {
            feeder: [0xab; 32],
            id: 7,
            secret: [0x01; 32],
        };
        let text = invite.text();
        assert!(text.starts_with("wellfed-invite:1:abab"));
        assert_eq!(Invite::parse(&text), Ok(invite));
        assert_eq!(Invite::parse(&format!(" {text}\n")), Ok(invite));
    }

    #[test]
    fn a_malformed_invite_says_which_part() {
        let feeder = "ab".repeat(32);
        let secret = "01".repeat(32);
        let upper = "AB".repeat(32);
        let short = "01".repeat(31);
        for (text, error) in [
            (format!("invite:{feeder}:1:{secret}"), InviteText::Prefix),
            (format!("{INVITE_PREFIX}{feeder}:1"), InviteText::Parts),
            (
                format!("{INVITE_PREFIX}{feeder}:1:{secret}:x"),
                InviteText::Parts,
            ),
            (
                format!("{INVITE_PREFIX}{upper}:1:{secret}"),
                InviteText::Feeder,
            ),
            (
                format!("{INVITE_PREFIX}{feeder}:x:{secret}"),
                InviteText::Id,
            ),
            (
                format!("{INVITE_PREFIX}{feeder}:1:{short}"),
                InviteText::Secret,
            ),
        ] {
            assert_eq!(Invite::parse(&text), Err(error), "{text}");
        }
    }
}
