//! The device API over tokio: [`wellfed_client::api`]'s sans-IO sessions
//! driven over a real TCP connection.
//!
//! - [`locate`] finds a feeder by its identity over mDNS, or takes the
//!   address it is given.
//! - [`Connection`] is an open session. [`Connection::ask`] sends one
//!   request and waits for its answer; events, gaps and samples that arrive
//!   meanwhile stay queued for [`Connection::next`] rather than being lost.
//! - [`join`] redeems an invite.
//!
//! The feeder drops a session that sends nothing for its idle limit (90 s),
//! so [`Connection::next`] sends a `Status` as a keepalive when the
//! connection has been quiet on this side for [`KEEPALIVE`]; its answer is
//! swallowed. A live feeder answers every keepalive, so a connection that
//! hears nothing for [`SILENCE`] fails with [`Error::Silent`]: a feeder
//! that lost power sends no reset, and the kernel would otherwise keep the
//! socket open for many minutes.
//!
//! Every method is cancel-safe, so a caller can `select!` on
//! [`Connection::next`] beside its own work: reads keep nothing in flight,
//! and a write cut short stays queued and finishes before the next one. A
//! cancelled [`Connection::ask`] may still have sent its request; the
//! answer then arrives through [`Connection::next`].
//!
//! Randomness is the caller's: request ids and ephemeral keys come in as
//! arguments, so a server can draw them from its own generator and tests
//! can fix them. Timeouts are this crate's; a request that times out may
//! still have run, so the caller retries a write with the same id.

#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use wellfed_client::api::{Incoming, Invite, Join, JoinStep, Session};
use wellfed_core::hex::Lower;
use wellfed_core::noise::Keypair;
use wellfed_proto::api::{
    FRAME_MAX, FeederMessage, Joined, Op, OpenRefusal, Outcome, PORT, RequestId, Role, SERVICE,
    Welcome,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{Instant, timeout, timeout_at};

/// mDNS answers on a LAN arrive within a second or two.
pub const DISCOVERY: Duration = Duration::from_secs(6);
pub const CONNECT: Duration = Duration::from_secs(5);
/// A request is answered at once, or after its store write.
pub const ANSWER: Duration = Duration::from_secs(10);
/// Well inside the feeder's 90 s idle limit, with room for a lost segment.
pub const KEEPALIVE: Duration = Duration::from_secs(45);
/// Room for a keepalive (sent 45 s after this side last wrote) and its
/// answer, with margin: hearing nothing for this long, the feeder is gone.
pub const SILENCE: Duration = Duration::from_secs(90);

/// The id of the keepalive's `Status`. A read is never remembered by the
/// feeder, so reusing one id is safe; its answer is dropped here, so a
/// caller may not use it.
pub const KEEPALIVE_ID: RequestId = *b"wellfed-alive\0\0";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}: not an address or address:port")]
    Address(String),
    #[error("mDNS: {0}")]
    Mdns(#[from] mdns_sd::Error),
    #[error("no feeder {0} answered over mDNS; pass its address")]
    NotFound(String),
    #[error("connect to {0}: timed out")]
    ConnectTimeout(SocketAddr),
    #[error("connect to {address}: {source}")]
    Connect {
        address: SocketAddr,
        source: std::io::Error,
    },
    #[error("the feeder did not answer in time")]
    Timeout,
    #[error("the feeder closed the connection")]
    Closed,
    #[error("the feeder stopped answering")]
    Silent,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Session(#[from] wellfed_client::api::Error),
    #[error("the feeder's message does not decode")]
    Malformed,
    #[error("a message does not fit one frame")]
    Encode,
    #[error("the feeder spoke before its welcome")]
    NoWelcome,
    #[error("the feeder sent a second welcome")]
    Unexpected,
    #[error("that request id is the keepalive's")]
    ReservedId,
    #[error("the feeder does not know this key (not enrolled, or revoked)")]
    UnknownKey,
    #[error("the feeder serves protocol {0}; update this tool or the feeder")]
    Version(u16),
    #[error("every session slot is taken")]
    Full,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// The feeder's address: `address` when given (an IP takes the API's port),
/// otherwise the feeder with `identity` found over mDNS within `wait`.
///
/// mDNS browsing blocks the calling thread for up to `wait`.
pub fn locate(identity: &[u8; 32], address: Option<&str>, wait: Duration) -> Result<SocketAddr> {
    if let Some(address) = address {
        if let Ok(ip) = address.parse::<IpAddr>() {
            return Ok(SocketAddr::new(ip, PORT));
        }
        return address
            .parse()
            .map_err(|_| Error::Address(address.to_owned()));
    }

    let daemon = mdns_sd::ServiceDaemon::new()?;
    let receiver = daemon.browse(&format!("{SERVICE}.local."))?;
    let want = Lower(identity).to_string();
    let deadline = std::time::Instant::now() + wait;
    let found = loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            break None;
        }
        let Ok(event) = receiver.recv_timeout(left) else {
            break None;
        };
        let mdns_sd::ServiceEvent::ServiceResolved(info) = event else {
            continue;
        };
        if info.get_property_val_str("id") != Some(want.as_str()) {
            continue;
        }
        if let Some(ip) = info.get_addresses_v4().into_iter().next() {
            break Some(SocketAddr::new(IpAddr::V4(ip), info.get_port()));
        }
    };
    // Shutting down only stops the daemon's thread; a failure leaves
    // nothing behind that matters to the caller.
    let _ = daemon.shutdown();
    found.ok_or_else(|| Error::NotFound(want[..16].to_owned()))
}

/// One message from the feeder, owned: the session's own buffer is reused
/// by the next read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message(Vec<u8>);

impl Message {
    fn encode(message: &FeederMessage<'_>) -> Result<Self> {
        let mut out = vec![0; FRAME_MAX];
        let len = wellfed_proto::encode(message, &mut out).map_err(|_| Error::Encode)?;
        out.truncate(len);
        Ok(Self(out))
    }

    pub fn get(&self) -> FeederMessage<'_> {
        wellfed_proto::decode(&self.0).expect("encoded from a decoded message")
    }
}

/// One request's answer, owned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer(Vec<u8>);

impl Answer {
    fn encode(outcome: &Outcome<'_>) -> Result<Self> {
        let mut out = vec![0; FRAME_MAX];
        let len = wellfed_proto::encode(outcome, &mut out).map_err(|_| Error::Encode)?;
        out.truncate(len);
        Ok(Self(out))
    }

    pub fn outcome(&self) -> Outcome<'_> {
        wellfed_proto::decode(&self.0).expect("encoded from a decoded outcome")
    }
}

/// An open session.
pub struct Connection {
    stream: TcpStream,
    session: Session,
    build: String,
    role: Role,
    /// Messages that arrived while [`Self::ask`] waited for its answer.
    queued: VecDeque<Message>,
    /// Bytes the socket has not yet taken. A write whose future was
    /// dropped leaves its rest here, and it goes out before anything else,
    /// so frames never interleave.
    outbox: Vec<u8>,
    /// When this side last wrote, for the keepalive.
    sent_at: Instant,
    /// When the feeder was last heard, for [`SILENCE`].
    received_at: Instant,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("build", &self.build)
            .field("role", &self.role)
            .finish_non_exhaustive()
    }
}

impl Connection {
    /// Open a session with the feeder `identity` at `address` as `keys`;
    /// `ephemeral` is fresh from the OS generator.
    pub async fn open(
        keys: &Keypair,
        identity: [u8; 32],
        address: SocketAddr,
        ephemeral: [u8; 32],
    ) -> Result<Self> {
        let (session, hello) = Session::connect(keys, identity, ephemeral)?;
        let mut stream = timeout(CONNECT, TcpStream::connect(address))
            .await
            .map_err(|_| Error::ConnectTimeout(address))?
            .map_err(|source| Error::Connect { address, source })?;
        stream.set_nodelay(true)?;
        stream.write_all(&hello).await?;

        let mut connection = Self {
            stream,
            session,
            build: String::new(),
            role: Role::Member,
            queued: VecDeque::new(),
            outbox: Vec::new(),
            sent_at: Instant::now(),
            received_at: Instant::now(),
        };
        let deadline = Instant::now() + ANSWER;
        loop {
            connection.read(deadline).await?;
            let Some(incoming) = connection.session.next_incoming()? else {
                continue;
            };
            let Incoming::Welcome(welcome) = incoming else {
                return Err(Error::NoWelcome);
            };
            return match welcome {
                Welcome::Open { build, role, .. } => {
                    connection.build = build.to_owned();
                    connection.role = role;
                    Ok(connection)
                }
                Welcome::Refused(OpenRefusal::Unknown) => Err(Error::UnknownKey),
                Welcome::Refused(OpenRefusal::Version { supported }) => {
                    Err(Error::Version(supported))
                }
                Welcome::Refused(OpenRefusal::Full) => Err(Error::Full),
            };
        }
    }

    /// The feeder's build, from its welcome.
    pub fn build(&self) -> &str {
        &self.build
    }

    /// The role the feeder gave this key.
    pub fn role(&self) -> Role {
        self.role
    }

    /// Messages that arrived while [`Self::ask`] waited, not yet taken by
    /// [`Self::next`].
    pub fn queued(&self) -> usize {
        self.queued.len()
    }

    /// Read what arrives before `deadline` into the session.
    async fn read(&mut self, deadline: Instant) -> Result<()> {
        let mut buffer = [0u8; 2048];
        let n = timeout_at(deadline, self.stream.read(&mut buffer))
            .await
            .map_err(|_| Error::Timeout)??;
        if n == 0 {
            return Err(Error::Closed);
        }
        self.received_at = Instant::now();
        self.session.receive(&buffer[..n])?;
        Ok(())
    }

    async fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.outbox.extend_from_slice(bytes);
        self.flush().await
    }

    /// Send what the outbox holds. Each `write` either takes bytes or
    /// leaves them, and they leave the outbox in the same poll, so a
    /// dropped future loses or repeats nothing.
    async fn flush(&mut self) -> Result<()> {
        if self.outbox.is_empty() {
            return Ok(());
        }
        while !self.outbox.is_empty() {
            let n = self.stream.write(&self.outbox).await?;
            if n == 0 {
                return Err(Error::Closed);
            }
            self.outbox.drain(..n);
        }
        self.sent_at = Instant::now();
        Ok(())
    }

    /// Send one request under `id`; its answer comes through [`Self::next`].
    /// `id` may not be [`KEEPALIVE_ID`].
    pub async fn request(&mut self, id: RequestId, op: Op<'_>) -> Result<()> {
        if id == KEEPALIVE_ID {
            return Err(Error::ReservedId);
        }
        self.send_request(id, op).await
    }

    async fn send_request(&mut self, id: RequestId, op: Op<'_>) -> Result<()> {
        let bytes = self.session.request(id, op)?;
        self.write(&bytes).await
    }

    /// Ask for every event after `after` (`None`: the oldest kept), then
    /// each new one, and the live samples if `samples`.
    pub async fn subscribe(&mut self, after: Option<u64>, samples: bool) -> Result<()> {
        let bytes = self.session.subscribe(after, samples)?;
        self.write(&bytes).await
    }

    /// The next message within `wait`, keeping the session alive meanwhile.
    pub async fn next(&mut self, wait: Duration) -> Result<Message> {
        if let Some(message) = self.queued.pop_front() {
            return Ok(message);
        }
        let deadline = Instant::now() + wait;
        self.flush().await?;
        loop {
            if let Some(message) = self.decoded()? {
                return Ok(message);
            }
            let now = Instant::now();
            let silent = self.received_at + SILENCE;
            if silent <= now {
                return Err(Error::Silent);
            }
            let keepalive = self.sent_at + KEEPALIVE;
            if keepalive <= now {
                self.send_request(KEEPALIVE_ID, Op::Status).await?;
                continue;
            }
            match self.read(deadline.min(keepalive).min(silent)).await {
                Err(Error::Timeout) if Instant::now() < deadline => {}
                result => result?,
            }
        }
    }

    /// The next message already opened, other than a keepalive's answer.
    fn decoded(&mut self) -> Result<Option<Message>> {
        while let Some(incoming) = self.session.next_incoming()? {
            let Incoming::Message(message) = incoming else {
                return Err(Error::Unexpected);
            };
            if matches!(message, FeederMessage::Response { id, .. } if id == KEEPALIVE_ID) {
                continue;
            }
            return Ok(Some(Message::encode(&message)?));
        }
        Ok(None)
    }

    /// One request under `id` and its answer. Other messages that arrive
    /// meanwhile stay queued for [`Self::next`].
    pub async fn ask(&mut self, id: RequestId, op: Op<'_>) -> Result<Answer> {
        self.request(id, op).await?;
        let deadline = Instant::now() + ANSWER;
        loop {
            while let Some(message) = self.decoded()? {
                if let FeederMessage::Response { id: got, result } = message.get()
                    && got == id
                {
                    return Answer::encode(&result);
                }
                self.queued.push_back(message);
            }
            self.read(deadline).await?;
        }
    }
}

/// Redeem `invite` at `address` as `keys`, listed as `label`: the role the
/// feeder enrolled the key with, or its refusal.
pub async fn join(
    keys: &Keypair,
    invite: Invite,
    label: &str,
    address: SocketAddr,
    ephemeral: [u8; 32],
) -> Result<Joined> {
    let (mut join, hello) = Join::connect(keys, invite, label, ephemeral)?;
    let mut stream = timeout(CONNECT, TcpStream::connect(address))
        .await
        .map_err(|_| Error::ConnectTimeout(address))?
        .map_err(|source| Error::Connect { address, source })?;
    stream.write_all(&hello).await?;

    let mut buffer = [0u8; 2048];
    loop {
        let n = timeout(ANSWER, stream.read(&mut buffer))
            .await
            .map_err(|_| Error::Timeout)??;
        if n == 0 {
            // The feeder drops an unknown or expired invite unanswered.
            return Err(Error::Closed);
        }
        match join.receive(&buffer[..n])? {
            None => {}
            Some(JoinStep::Send(bytes)) => stream.write_all(&bytes).await?,
            Some(JoinStep::Done(joined)) => return Ok(joined),
        }
    }
}
