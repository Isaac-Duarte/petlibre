//! The device API over tokio: [`wellfed_client::connection`]'s sans-IO
//! session driven over a real TCP connection. Every session rule
//! (keepalive, silence, answers, the event cursor) is the client crate's;
//! this crate only moves bytes and keeps the clock.
//!
//! - [`locate`] finds a feeder by its identity over mDNS, or takes the
//!   address it is given.
//! - [`Connection`] is an open session. [`Connection::ask`] sends one
//!   request and waits for its answer; events, gaps and samples that arrive
//!   meanwhile stay queued for [`Connection::next`] rather than being lost.
//!   A caller that keeps several requests outstanding (the server's feeder
//!   task) queues each with [`Connection::request`] and takes answers and
//!   pushed messages alike from [`Connection::receive`], matching answers
//!   by id: the feeder reads one request per client per pass and answers
//!   in the order they arrive.
//! - [`join`] redeems an invite, and [`Recovery`] runs the recovery window.
//! - [`update::push`] drives a signed update until bank 1 is activated,
//!   over the CLI's own session or the server's feeder task.
//!
//! Every method is cancel-safe, so a caller can `select!` on
//! [`Connection::next`] beside its own work: reads keep nothing in flight,
//! and a write cut short stays queued and finishes before the next one. A
//! cancelled [`Connection::ask`] may still have sent its request; its
//! answer is then dropped.
//!
//! Randomness is the caller's: request ids and ephemeral keys come in as
//! arguments, so a server can draw them from its own generator and tests
//! can fix them. A request that times out may still have run, so the
//! caller retries a write with the same id.

#![forbid(unsafe_code)]

pub mod update;

use std::collections::VecDeque;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{Instant, timeout, timeout_at};
use wellfed_client::api::{Invite, Join, JoinStep};
use wellfed_client::connection::{self, Event, Opened, Recovery as Window, Timing};
pub use wellfed_client::connection::{Cursor, KEEPALIVE_ID, Resume};
use wellfed_model::setup::Reply;
use wellfed_model::{Answer, Capabilities, ClientRole, Joined, Op, OpenRefusal, Pushed};
use wellfed_proto::api::{PORT, RequestId, SERVICE};
use wellfed_session::hex::Lower;
use wellfed_session::noise::Keypair;

/// mDNS answers on a LAN arrive within a second or two.
pub const DISCOVERY: Duration = Duration::from_secs(6);
pub const CONNECT: Duration = Duration::from_secs(5);
/// A request is answered at once, or after its store write.
pub const ANSWER: Duration = Timing::DEFAULT.answer;
/// Hearing nothing for this long, the feeder is gone.
pub const SILENCE: Duration = Timing::DEFAULT.silence;

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
    Session(wellfed_client::api::Error),
    #[error("a message does not fit one frame")]
    Encode,
    #[error("that request id is the keepalive's")]
    ReservedId,
    #[error("the feeder does not know this key (not enrolled, or revoked)")]
    UnknownKey,
    #[error("the feeder serves protocol {0}; update this tool or the feeder")]
    Version(u16),
    #[error("every session slot is taken")]
    Full,
}

impl From<wellfed_client::api::Error> for Error {
    fn from(error: wellfed_client::api::Error) -> Self {
        use wellfed_client::api::Error as Session;
        match error {
            Session::Refused(OpenRefusal::Unknown) => Self::UnknownKey,
            Session::Refused(OpenRefusal::Version { supported }) => Self::Version(supported),
            Session::Refused(OpenRefusal::Full) => Self::Full,
            Session::Timeout => Self::Timeout,
            Session::Silent => Self::Silent,
            Session::ReservedId => Self::ReservedId,
            Session::TooLarge => Self::Encode,
            other => Self::Session(other),
        }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// An address as an owner types it: an IP takes the API's port.
pub fn parse_address(address: &str) -> Result<SocketAddr> {
    if let Ok(ip) = address.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, PORT));
    }
    address
        .parse()
        .map_err(|_| Error::Address(address.to_owned()))
}

/// The feeder's address: `address` when given (an IP takes the API's port),
/// otherwise the feeder with `identity` found over mDNS within `wait`.
///
/// mDNS browsing blocks the calling thread for up to `wait`.
pub fn locate(identity: &[u8; 32], address: Option<&str>, wait: Duration) -> Result<SocketAddr> {
    if let Some(address) = address {
        return parse_address(address);
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

/// The clock the sans-IO session reads: tokio's, so a paused test clock
/// drives it too.
fn now() -> std::time::Instant {
    Instant::now().into_std()
}

async fn dial(address: SocketAddr) -> Result<TcpStream> {
    let stream = timeout(CONNECT, TcpStream::connect(address))
        .await
        .map_err(|_| Error::ConnectTimeout(address))?
        .map_err(|source| Error::Connect { address, source })?;
    stream.set_nodelay(true)?;
    Ok(stream)
}

/// Bytes the socket has not yet taken. A write whose future was dropped
/// leaves its rest here, and it goes out before anything else, so frames
/// never interleave.
#[derive(Debug, Default)]
struct Outbox(Vec<u8>);

impl Outbox {
    /// Send what the outbox holds. Each `write` either takes bytes or
    /// leaves them, and they leave the outbox in the same poll, so a
    /// dropped future loses or repeats nothing.
    async fn flush(&mut self, stream: &mut TcpStream) -> Result<()> {
        while !self.0.is_empty() {
            let n = stream.write(&self.0).await?;
            if n == 0 {
                return Err(Error::Closed);
            }
            self.0.drain(..n);
        }
        Ok(())
    }
}

/// What [`Connection::receive`] yields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Received {
    /// An event, gap, sample or log batch the feeder pushed.
    Pushed(Pushed),
    /// The answer to the request under `id`, or `None` when none came in
    /// time or the session ended first: the request may have run.
    Answer {
        id: RequestId,
        answer: Option<Answer>,
    },
}

/// An open session.
pub struct Connection {
    stream: TcpStream,
    session: connection::Connection,
    outbox: Outbox,
    /// Pushed messages not yet taken by [`Self::next`].
    queued: VecDeque<Pushed>,
    /// Answers, or their absence, not yet taken by [`Self::ask`] or
    /// [`Self::receive`].
    answers: VecDeque<(RequestId, Option<Answer>)>,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("opened", &self.session.opened())
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
        let session = connection::Connection::new(keys, identity, ephemeral, now())?;
        let mut connection = Self {
            stream: dial(address).await?,
            session,
            outbox: Outbox::default(),
            queued: VecDeque::new(),
            answers: VecDeque::new(),
        };
        while connection.session.opened().is_none() {
            connection.turn(None).await?;
        }
        Ok(connection)
    }

    fn opened(&self) -> &Opened {
        self.session
            .opened()
            .expect("a Connection exists only once welcomed")
    }

    /// The feeder's build, from its welcome.
    pub fn build(&self) -> &str {
        &self.opened().build
    }

    /// The role the feeder gave this key.
    pub fn role(&self) -> ClientRole {
        self.opened().role
    }

    /// The features both sides claimed: the session runs on these only, and
    /// an op outside them would end it on a feeder that predates the op.
    pub fn capabilities(&self) -> Capabilities {
        self.opened().capabilities
    }

    /// The epoch the feeder's event numbers count in.
    pub fn epoch(&self) -> u32 {
        self.opened().epoch
    }

    /// The newest event or gap end received since [`Self::subscribe`].
    pub fn cursor(&self) -> Option<Cursor> {
        self.session.cursor()
    }

    /// Messages that arrived while [`Self::ask`] waited, not yet taken by
    /// [`Self::next`].
    pub fn queued(&self) -> usize {
        self.queued.len()
    }

    /// One turn of the loop: write what the session has, then read or
    /// wake for its clock, until `until` at the latest. Cancel-safe.
    async fn turn(&mut self, until: Option<Instant>) -> Result<()> {
        while let Some(bytes) = self.session.poll_transmit() {
            self.outbox.0.extend(bytes);
        }
        self.outbox.flush(&mut self.stream).await?;

        let due = self.session.poll_timeout().map(Instant::from_std);
        let wake = match (due, until) {
            (Some(due), Some(until)) => due.min(until),
            (due, until) => due.or(until).unwrap_or_else(|| Instant::now() + SILENCE),
        };
        // A clock already due acts before any read: a session silent past
        // its limit is gone even when a close is waiting to be read.
        let mut buffer = [0u8; 2048];
        let read = if wake <= Instant::now() {
            Err(())
        } else {
            timeout_at(wake, self.stream.read(&mut buffer))
                .await
                .map_err(drop)
        };
        match read {
            Ok(read) => {
                let n = read?;
                if n == 0 {
                    return Err(Error::Closed);
                }
                self.session.handle_input(now(), &buffer[..n])?;
            }
            Err(_) => self.session.handle_timeout(now())?,
        }

        while let Some(event) = self.session.poll_event() {
            match event {
                Event::Pushed(message) => self.queued.push_back(message),
                Event::Answer { id, answer } => self.answers.push_back((id, Some(answer))),
                Event::Unanswered { id } => self.answers.push_back((id, None)),
            }
        }
        Ok(())
    }

    /// Ask for the history after `from`, the newest event the caller holds
    /// (`None`: the oldest kept), then each new one, and the live samples
    /// if `samples`. A cursor from an epoch the feeder no longer has starts
    /// from the oldest kept: [`Resume::Restarted`].
    pub async fn subscribe(&mut self, from: Option<Cursor>, samples: bool) -> Result<Resume> {
        let resume = self.session.subscribe(now(), from, samples)?;
        while let Some(bytes) = self.session.poll_transmit() {
            self.outbox.0.extend(bytes);
        }
        self.outbox.flush(&mut self.stream).await?;
        Ok(resume)
    }

    /// Queue one request under `id` without waiting for its answer; it is
    /// written by the next [`Self::receive`], which later yields its answer
    /// or its absence. Several may be outstanding. `id` may not be
    /// [`KEEPALIVE_ID`]. Answers go to [`Self::receive`] only: [`Self::ask`]
    /// and [`Self::next`] drop those of other requests.
    pub fn request(&mut self, id: RequestId, op: &Op) -> Result<()> {
        self.session.request(now(), id, op)?;
        while let Some(bytes) = self.session.poll_transmit() {
            self.outbox.0.extend(bytes);
        }
        Ok(())
    }

    /// The next answer or pushed message within `wait`, writing what is
    /// queued and keeping the session alive meanwhile. Cancel-safe. When
    /// the session ends, the error comes back and the requests still
    /// outstanding get no answer here: each may have run.
    pub async fn receive(&mut self, wait: Duration) -> Result<Received> {
        let deadline = Instant::now() + wait;
        loop {
            if let Some((id, answer)) = self.answers.pop_front() {
                return Ok(Received::Answer { id, answer });
            }
            if let Some(message) = self.queued.pop_front() {
                return Ok(Received::Pushed(message));
            }
            if deadline <= Instant::now() {
                return Err(Error::Timeout);
            }
            self.turn(Some(deadline)).await?;
        }
    }

    /// The next pushed message within `wait`, keeping the session alive
    /// meanwhile.
    pub async fn next(&mut self, wait: Duration) -> Result<Pushed> {
        let deadline = Instant::now() + wait;
        loop {
            if let Some(message) = self.queued.pop_front() {
                return Ok(message);
            }
            // Answers belong to `ask` and `receive`; one given up on is dropped.
            self.answers.clear();
            if deadline <= Instant::now() {
                return Err(Error::Timeout);
            }
            self.turn(Some(deadline)).await?;
        }
    }

    /// One request under `id` and its answer. Other messages that arrive
    /// meanwhile stay queued for [`Self::next`]. `id` may not be
    /// [`KEEPALIVE_ID`].
    pub async fn ask(&mut self, id: RequestId, op: &Op) -> Result<Answer> {
        self.session.request(now(), id, op)?;
        loop {
            while let Some((answered, answer)) = self.answers.pop_front() {
                if answered == id {
                    return answer.ok_or(Error::Timeout);
                }
            }
            self.turn(None).await?;
        }
    }
}

/// The feeder's recovery window over the device API, as an owner key that
/// proves the setup code.
pub struct Recovery {
    stream: TcpStream,
    window: Window,
    outbox: Outbox,
}

impl std::fmt::Debug for Recovery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Recovery").finish_non_exhaustive()
    }
}

impl Recovery {
    /// Open the recovery session at `address` with `session`'s setup
    /// exchange.
    pub async fn open(
        address: SocketAddr,
        session: wellfed_client::setup::Session,
    ) -> Result<Self> {
        Ok(Self {
            stream: dial(address).await?,
            window: Window::new(session)?,
            outbox: Outbox::default(),
        })
    }

    /// The feeder's next reply within `wait`; the session answers the
    /// proof's steps itself meanwhile. A closed connection means the
    /// window is not open.
    pub async fn reply(&mut self, wait: Duration) -> Result<Reply> {
        let deadline = Instant::now() + wait;
        loop {
            while let Some(bytes) = self.window.poll_transmit() {
                self.outbox.0.extend(bytes);
            }
            self.outbox.flush(&mut self.stream).await?;
            if let Some(reply) = self.window.poll_reply() {
                return Ok(reply);
            }

            let mut buffer = [0u8; 1024];
            let n = timeout_at(deadline, self.stream.read(&mut buffer))
                .await
                .map_err(|_| Error::Timeout)??;
            if n == 0 {
                return Err(Error::Closed);
            }
            self.window.handle_input(&buffer[..n])?;
        }
    }

    /// The setup session: its code, and the feeder's key once proven.
    pub fn session(&self) -> &wellfed_client::setup::Session {
        self.window.session()
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
    let mut stream = dial(address).await?;
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
