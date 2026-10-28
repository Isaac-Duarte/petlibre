//! The feeder's device API as the bench speaks it: a Noise session
//! over TCP port 4747 as the owner's enrolled key (`wellfed setup` wrote
//! it to `private/wellfed-owner.json`), carrying the requests the suite and
//! the `ota` command make.
//!
//! Blocking, like the rest of the bench. It mirrors `wellfed-connect`, the
//! tokio driver the `wellfed` CLI uses (find the feeder, open a session,
//! ask): both only move bytes for `wellfed_client::api::Session`, and the
//! bench can move onto that crate once it runs on tokio.
//!
//! The feeder is found by its identity over mDNS (`_wellfed._tcp`, TXT
//! `id=`), and otherwise by its MAC in the hotspot's neighbour table: the
//! bench host's firewall drops inbound UDP 5353, so mDNS answers may never
//! arrive. The address and the session are kept between requests; a request
//! on a kept session that fails is sent once more on a fresh one under the
//! same id, which the feeder answers without running a write twice.
//!
//! A second, held session ([`Device::hold`]) is never reopened: the feeder
//! admits no handshake while a feed holds its outputs, so only a session
//! opened before the feed can reach it during one.
//!
//! [`Device::install`] pushes an update the way every client does
//! (`wellfed_client::update`): the offer, the give-way restart, the offer
//! again to bank 0, the chunks, and a resume after any drop. It can spoil
//! its own transfer ([`Fault`]) for the suite's failure drills; an offer is
//! never resent behind the caller's back.

use std::fmt;
use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use wellfed_client::api::{Error as SessionError, Incoming, Session};
use wellfed_client::update::{Image, InstallError, Installer, Next, Sent};
use wellfed_formats::hex;
use wellfed_formats::owner::OwnerKeyFile;
use wellfed_formats::private::{self, SYSTEM_RANDOM};
use wellfed_model::{
    Generation, Op, OpenRefusal, Refusal, Settings, SettingsPatch, Stats, StatsSection, Status,
    TimeView, UpdateView, Wifi, WifiSource,
};
use wellfed_proto::api::{PORT, RequestId};
use wellfed_session::noise::Keypair;

use crate::server::NotReady;

/// The owner key file the bench opens sessions with (root-relative).
pub const OWNER_KEYS: &str = "private/wellfed-owner.json";

/// mDNS answers on a LAN arrive within a second or two.
const DISCOVERY: Duration = Duration::from_secs(4);
const CONNECT: Duration = Duration::from_secs(5);
/// A request is answered at once, or after its store write.
const ANSWER: Duration = Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    #[error("{}: {error}", path.display())]
    Keys {
        path: PathBuf,
        error: wellfed_formats::Error,
    },
    #[error("{}: the secret or a feeder identity is not 32 bytes of hex", .0.display())]
    BadKey(PathBuf),
    #[error("{}: {count} feeders enrolled this key; the bench talks to exactly one", path.display())]
    Feeders { path: PathBuf, count: usize },
    #[error("the random source: {0}")]
    Random(wellfed_formats::Error),
    #[error(
        "feeder {fingerprint} did not answer over mDNS and {interface}'s neighbour table has no {mac}"
    )]
    NotFound {
        fingerprint: String,
        interface: String,
        mac: String,
    },
    #[error("connect to {0}: {1}")]
    Connect(SocketAddr, std::io::Error),
    #[error("the feeder closed the connection")]
    Closed,
    #[error("the feeder did not answer within {} s", .0.as_secs())]
    Timeout(Duration),
    #[error("the feeder refused the session: {0:?}")]
    Refused(OpenRefusal),
    #[error("the feeder spoke before its welcome")]
    Unwelcome,
    #[error("the feeder's message does not decode")]
    Decode,
    #[error("no held session is open")]
    NotHeld,
    #[error("time zone {0:?}: {1}")]
    Zone(String, jiff::Error),
    #[error(transparent)]
    Session(#[from] wellfed_client::api::Error),
    #[error(transparent)]
    Install(#[from] InstallError),
    #[error("the install was stopped")]
    Stopped,
    #[error("the feeder did not answer again within {} s", .0.as_secs())]
    Gone(Duration),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// What the suite asks the feeder; borrowed into an [`Op`] when sent.
#[derive(Clone, PartialEq, Eq)]
pub enum Ask {
    Status,
    GetSettings,
    PutSettings {
        base: u32,
        changes: SettingsPatch,
    },
    /// Credentials for the next join, or `None` to clear them.
    PutWifi {
        base: u32,
        wifi: Option<Credentials>,
    },
    /// The host's wall clock and the server config's time zone, read when
    /// the request goes out.
    SyncClock,
    /// The panel's child lock.
    SetLock {
        locked: bool,
    },
    /// Moves the motor: only a scenario the operator allowed feeds sends it.
    Feed {
        portions: u8,
    },
    /// Stop the feed a [`Answer::Feeding`] named.
    CancelFeed {
        feed: u32,
    },
    UpdateState,
    /// Stop any install and forget the recorded candidate, so bank 0 takes
    /// another manifest.
    CancelUpdate,
    /// Restart the feeder at rest; refused during a feed.
    Reboot,
    /// The time settings and the clock's state, with its error bound.
    GetTime,
    /// One section of the feeder's statistics.
    Stats {
        section: StatsSection,
    },
}

impl Ask {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::GetSettings => "get-settings",
            Self::PutSettings { .. } => "put-settings",
            Self::PutWifi { .. } => "put-wifi",
            Self::SyncClock => "time-sync",
            Self::SetLock { .. } => "set-lock",
            Self::Feed { .. } => "feed",
            Self::CancelFeed { .. } => "cancel-feed",
            Self::UpdateState => "update-state",
            Self::CancelUpdate => "cancel-update",
            Self::Reboot => "reboot",
            Self::GetTime => "get-time",
            Self::Stats { .. } => "stats",
        }
    }

    /// Whether a send that failed may go again on a fresh session. The
    /// feeder remembers a write id's outcome only once its answer resolves,
    /// so a feed resent after a lost answer can start a second feed; a feed
    /// or a cancel (of a feed or an update) is sent once, and a failure
    /// leaves it uncertain. So is a reboot: one resent after the restart it
    /// asked for would restart the feeder again.
    pub fn resendable(&self) -> bool {
        !matches!(
            self,
            Self::Feed { .. } | Self::CancelFeed { .. } | Self::CancelUpdate | Self::Reboot
        )
    }
}

impl fmt::Debug for Ask {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Wi-Fi credentials for [`Ask::PutWifi`]. The suite stores only invented
/// ones, but they stay out of every log all the same.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub ssid: String,
    pub psk: String,
}

/// The feeder's answer, owned so it outlives the session's buffer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Answer {
    Done {
        generation: Option<u32>,
    },
    Status(DeviceStatus),
    Settings(Settings),
    Refused(Refusal),
    /// The feed is reserved under `feed` and runs now.
    Feeding {
        feed: u32,
    },
    Update(UpdateView),
    Time(Box<TimeView>),
    Stats(Box<Stats>),
    /// An answer no [`Ask`] expects (clients, an invite, a trace).
    Unexpected,
}

impl Answer {
    fn of(answer: wellfed_model::Answer) -> Self {
        use wellfed_model::Answer as Model;
        match answer {
            Model::Done { generation } => Self::Done {
                generation: generation.map(|generation| generation.0),
            },
            Model::Status(status) => Self::Status(DeviceStatus::of(&status)),
            Model::Settings(settings) => Self::Settings(settings),
            Model::Refused(refusal) => Self::Refused(refusal),
            Model::Feeding { feed } => Self::Feeding { feed },
            Model::Update(view) => Self::Update(view),
            Model::Time(view) => Self::Time(Box::new(view)),
            Model::Stats(stats) => Self::Stats(Box::new(stats)),
            Model::Clients(_)
            | Model::Invite(_)
            | Model::FeedTrace(_)
            | Model::Calibration(_)
            | Model::Bank1(_)
            | Model::Plans(_)
            | Model::StockPlans(_) => Self::Unexpected,
        }
    }
}

/// The parts of the feeder's `Status` the bench checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceStatus {
    /// The startup identity, hex.
    pub build: String,
    pub bank: u8,
    pub uptime_ms: u32,
    pub boot_count: u32,
    pub wifi: WifiSource,
    /// Wi-Fi losses this boot, whether the link dropped or a DNS check
    /// failed first (an outage shows as either).
    pub losses: u32,
    /// Unix milliseconds, `None` while the clock is invalid.
    pub clock_ms: Option<u64>,
    pub settings_generation: u32,
    pub wifi_generation: u32,
    /// The panel's child lock; `None` before it is known.
    pub locked: Option<bool>,
}

impl DeviceStatus {
    fn of(status: &Status) -> Self {
        Self {
            build: status.build.to_ascii_lowercase(),
            bank: status.bank,
            uptime_ms: status.uptime_ms,
            boot_count: status.boot_count,
            wifi: status.wifi,
            losses: status.link_losses.saturating_add(status.dns_losses),
            clock_ms: status
                .clock
                .and_then(|clock| u64::try_from(clock.as_millisecond()).ok()),
            settings_generation: status.settings_generation.0,
            wifi_generation: status.wifi_generation.0,
            locked: status.locked,
        }
    }

    /// Whether `build` (lowercase) runs on bank 1: a one-line report of it.
    pub fn matches(&self, build: &str) -> Result<String, NotReady> {
        if self.build != build {
            let short = self.build.get(..12).filter(|s| !s.is_empty());
            return Err(NotReady::Running(short.unwrap_or("none").into()));
        }
        if self.bank != 1 {
            return Err(NotReady::OnBank(self.bank));
        }
        let clock = if self.clock_ms.is_some() {
            "valid"
        } else {
            "not set"
        };
        Ok(format!(
            "Status: build {} on bank 1, up {} s, boot {}, Wi-Fi {:?}, clock {clock}",
            &build[..build.len().min(12)],
            self.uptime_ms / 1000,
            self.boot_count,
            self.wifi
        ))
    }
}

/// A fault the bench puts into its own transfer, to see the feeder stop it
/// before activation. Offsets are image bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    None,
    /// Flip one bit of the image byte here: the whole-image hash fails.
    Corrupt {
        at: u32,
    },
    /// Once the chunk here is sent, send the first chunk two sectors on,
    /// and nothing else until it is answered: the install stops in order.
    Unwanted {
        at: u32,
    },
    /// Send nothing from here on for longer than the install waits.
    Stall {
        at: u32,
    },
    /// Drop the connection once the chunk here is answered: the install
    /// resumes on a new session.
    Drop {
        at: u32,
    },
    /// End once bank 1 has given way, leaving bank 0 waiting for the offer:
    /// another manifest offered to it then is refused.
    GiveWayOnly,
}

/// How long [`Device::install`] waits for the feeder to answer again after
/// a restart, a power cut or a drop.
pub(crate) const REACH: Duration = Duration::from_secs(10 * 60);
/// How long a stalled transfer stays quiet: past the install's own minute.
const STALL: Duration = Duration::from_secs(75);
/// Between polls of the connection while chunks are out.
const POLL: Duration = Duration::from_millis(100);
/// Undecodable feeder messages one install resumes past. A feeder that
/// garbles every answer (another protocol, or a fault that persists) still
/// ends the install with the decode error.
const GARBLED_MAX: u32 = 3;

/// A connection the feeder must not let in, for `request-auth`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intruder {
    /// A well-formed session from a key the feeder never enrolled.
    UnenrolledKey,
    /// The owner's own first handshake message with one byte flipped.
    Tampered,
}

/// How the feeder met an [`Intruder`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Rebuffed {
    /// It answered the handshake with a refusal.
    Refused(OpenRefusal),
    /// It closed the connection without a session.
    Dropped,
    /// It opened a session: the check failed.
    Opened,
    /// It sent bytes that are no welcome (the tampered handshake cannot open).
    Answered,
    /// Nothing within the answer time.
    Silent,
}

/// The parts of `wellfed-stock`'s config the device client uses.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Hotspot {
    #[serde(default)]
    pub hotspot_interface: String,
    #[serde(default)]
    pub target_mac: String,
    /// IANA name; the clock sync carries its offset and next change.
    #[serde(default = "utc")]
    pub timezone: String,
}

fn utc() -> String {
    "UTC".into()
}

/// One `ip -j neigh` entry; incomplete entries have no `lladdr`. The same
/// lookup as `wellfed-stock`'s `neighbour::discover`.
#[derive(Deserialize)]
struct Neighbour {
    dst: String,
    lladdr: Option<String>,
}

struct Connection {
    stream: TcpStream,
    session: Session,
}

/// The owner's sessions with the one feeder its key file names.
pub struct Device {
    keys: Keypair,
    identity: [u8; 32],
    hotspot: Hotspot,
    zone: jiff::tz::TimeZone,
    address: Option<SocketAddr>,
    /// The address was given, not found: it is never looked up again.
    pinned: bool,
    connection: Option<Connection>,
    /// [`Self::hold`]'s session, apart from `connection`.
    held: Option<Connection>,
}

fn random<const N: usize>() -> Result<[u8; N], DeviceError> {
    private::random(Path::new(SYSTEM_RANDOM)).map_err(DeviceError::Random)
}

impl Device {
    /// The key file at `keys` and the hotspot fields of the server config
    /// at `config`, both under `root`.
    pub fn open(root: &Path, keys: &Path, config: &Path) -> crate::Result<Self> {
        let file: OwnerKeyFile =
            wellfed_formats::load(&root.join(keys)).map_err(|error| DeviceError::Keys {
                path: keys.to_path_buf(),
                error,
            })?;
        let secret =
            hex::decode::<32>(&file.secret).ok_or_else(|| DeviceError::BadKey(keys.into()))?;
        let [feeder] = file.feeders.as_slice() else {
            return Err(DeviceError::Feeders {
                path: keys.into(),
                count: file.feeders.len(),
            }
            .into());
        };
        let identity =
            hex::decode::<32>(&feeder.identity).ok_or_else(|| DeviceError::BadKey(keys.into()))?;

        let hotspot: Hotspot = crate::load(root, config)?;
        let zone = jiff::tz::TimeZone::get(&hotspot.timezone)
            .map_err(|error| DeviceError::Zone(hotspot.timezone.clone(), error))?;
        Ok(Self {
            keys: Keypair::from_secret(secret),
            identity,
            hotspot,
            zone,
            address: None,
            pinned: false,
            connection: None,
            held: None,
        })
    }

    /// The owner with `secret` reaching the feeder `identity` at `address`
    /// alone (host tests, against the simulator).
    #[cfg(test)]
    pub(crate) fn at(secret: [u8; 32], identity: [u8; 32], address: SocketAddr) -> Self {
        Self {
            keys: Keypair::from_secret(secret),
            identity,
            hotspot: Hotspot {
                hotspot_interface: String::new(),
                target_mac: String::new(),
                timezone: utc(),
            },
            zone: jiff::tz::TimeZone::UTC,
            address: Some(address),
            pinned: true,
            connection: None,
            held: None,
        }
    }

    /// One request and its answer; see [`Ask::resendable`] for which a
    /// failure sends again.
    pub fn ask(&mut self, ask: &Ask) -> Result<Answer, DeviceError> {
        let id: RequestId = random()?;
        let reused = self.connection.is_some();
        match self.exchange(id, ask) {
            // A kept session may have died with a reboot or a bounce.
            Err(_) if reused && ask.resendable() => self.exchange(id, ask),
            other => other,
        }
    }

    /// Open a second session now and keep it for [`Self::ask_held`],
    /// replacing any held before.
    pub fn hold(&mut self) -> Result<(), DeviceError> {
        self.held = None;
        self.held = Some(self.connect()?);
        Ok(())
    }

    /// One request on the held session. A failure closes it rather than
    /// open another, so every answer comes from the session held before.
    pub fn ask_held(&mut self, ask: &Ask) -> Result<Answer, DeviceError> {
        let mut connection = self.held.take().ok_or(DeviceError::NotHeld)?;
        let answer = self.send(&mut connection, random()?, ask)?;
        self.held = Some(connection);
        Ok(answer)
    }

    /// Close the held session.
    pub fn release(&mut self) {
        self.held = None;
    }

    fn exchange(&mut self, id: RequestId, ask: &Ask) -> Result<Answer, DeviceError> {
        let mut connection = match self.connection.take() {
            Some(connection) => connection,
            None => self.connect()?,
        };
        let answer = self.send(&mut connection, id, ask)?;
        self.connection = Some(connection);
        Ok(answer)
    }

    fn send(
        &self,
        connection: &mut Connection,
        id: RequestId,
        ask: &Ask,
    ) -> Result<Answer, DeviceError> {
        let op = match ask {
            Ask::Status => Op::Status,
            Ask::GetSettings => Op::GetSettings,
            Ask::PutSettings { base, changes } => Op::PutSettings {
                base: Generation(*base),
                changes: *changes,
            },
            Ask::PutWifi { base, wifi } => Op::PutWifi {
                base: Generation(*base),
                wifi: wifi.as_ref().map(|wifi| Wifi {
                    ssid: wifi.ssid.as_bytes().to_vec(),
                    psk: wifi.psk.clone(),
                }),
            },
            Ask::SyncClock => Op::TimeSync(wellfed_client::time::sync(
                jiff::Timestamp::now(),
                &self.zone,
            )),
            Ask::SetLock { locked } => Op::SetLock { locked: *locked },
            Ask::Feed { portions } => Op::Feed {
                portions: *portions,
            },
            Ask::CancelFeed { feed } => Op::CancelFeed { feed: *feed },
            Ask::UpdateState => Op::UpdateState,
            Ask::CancelUpdate => Op::CancelUpdate,
            Ask::Reboot => Op::Reboot,
            Ask::GetTime => Op::GetTime,
            Ask::Stats { section } => Op::Stats { section: *section },
        };
        let bytes = connection.session.request(id, &op)?;
        connection.stream.write_all(&bytes)?;

        let deadline = Instant::now() + ANSWER;
        loop {
            while let Some(incoming) = connection.session.next_incoming()? {
                if let Incoming::Answer { id: got, answer } = incoming
                    && got == id
                {
                    return Ok(Answer::of(answer));
                }
            }
            read(connection, deadline)?;
        }
    }

    /// Open a session as the owner: at the known address, or one found
    /// again if that no longer answers.
    fn connect(&mut self) -> Result<Connection, DeviceError> {
        let known = self.address;
        let address = match known {
            Some(address) => address,
            None => self.locate()?,
        };
        match self.handshake(address) {
            Err(DeviceError::Connect(..) | DeviceError::Timeout(_))
                if known.is_some() && !self.pinned =>
            {
                self.address = None;
                let address = self.locate()?;
                self.handshake(address)
            }
            other => other,
        }
    }

    fn handshake(&mut self, address: SocketAddr) -> Result<Connection, DeviceError> {
        let (session, hello) = Session::connect(&self.keys, self.identity, random()?)?;
        let mut connection = Connection {
            stream: dial(address)?,
            session,
        };
        connection.stream.write_all(&hello)?;
        self.address = Some(address);

        let deadline = Instant::now() + ANSWER;
        loop {
            read(&mut connection, deadline)?;
            let Some(incoming) = connection.session.next_incoming()? else {
                continue;
            };
            return match incoming {
                Incoming::Opened(_) => Ok(connection),
                Incoming::Refused(refusal) => Err(DeviceError::Refused(refusal)),
                Incoming::Answer { .. } | Incoming::Pushed(_) => Err(DeviceError::Unwelcome),
            };
        }
    }

    /// The feeder's identity key, which a downgrade statement names.
    pub(crate) fn identity(&self) -> [u8; 32] {
        self.identity
    }

    /// A session as soon as the feeder answers, retrying for up to `wait`.
    fn reach(&mut self, wait: Duration, stop: &AtomicBool) -> Result<Connection, DeviceError> {
        let deadline = Instant::now() + wait;
        loop {
            if stop.load(Ordering::Acquire) {
                return Err(DeviceError::Stopped);
            }
            match self.connect() {
                Ok(connection) => return Ok(connection),
                Err(error @ (DeviceError::Refused(_) | DeviceError::Keys { .. })) => {
                    return Err(error);
                }
                Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_secs(2)),
                Err(_) => return Err(DeviceError::Gone(wait)),
            }
        }
    }

    /// The bank a fresh session answers from.
    fn bank(&self, connection: &mut Connection) -> Result<u8, DeviceError> {
        match self.send(connection, random()?, &Ask::Status)? {
            Answer::Status(status) => Ok(status.bank),
            _ => Err(DeviceError::Unwelcome),
        }
    }

    /// Install `image` over the device API, with `fault` spoiling the
    /// transfer, until bank 1 is activated (the feeder then restarts into
    /// it; its trial is the caller's to watch) or the install stops. `note`
    /// hears each step. Setting `stop` cancels the install on the feeder
    /// (so no recorded candidate holds bank 0 to it) and ends the push.
    pub(crate) fn install(
        &mut self,
        image: Image<'_>,
        mut fault: Fault,
        stop: &AtomicBool,
        note: &mut dyn FnMut(String),
    ) -> Result<(), DeviceError> {
        let mut installer = Installer::new(image);
        // One session for the push: a kept one would hold a slot it never uses.
        self.connection = None;
        let mut connection = self.reach(REACH, stop)?;
        let mut bank = self.bank(&mut connection)?;
        note(format!("Session open on bank {bank}"));
        installer.opened(bank);
        let mut out: Vec<(RequestId, Sent, Instant)> = Vec::new();
        let mut written = 0;
        // The unwanted chunk, sent until the stage takes it (a full mailbox
        // answers Busy), and the request it went under. Until it is taken
        // no wanted chunk goes, so the install cannot reach its offset.
        let mut unwanted: Option<(u32, Option<RequestId>)> = None;
        let mut cancelling: Option<Instant> = None;
        let mut garbled = 0;

        loop {
            if stop.load(Ordering::Acquire) && cancelling.is_none() {
                note("Stopping: cancelling the install on the feeder".into());
                installer.cancel();
                cancelling = Some(Instant::now());
                // The injected chunk is moot once the install is cancelled,
                // and resent until taken it would go ahead of the cancel.
                unwanted = None;
            }
            if cancelling.is_some_and(|since| since.elapsed() > ANSWER) {
                return Err(DeviceError::Stopped);
            }
            let mut lost = false;
            if let Some((ahead, None)) = unwanted {
                let data = image.payload.get(ahead as usize..ahead as usize + 512);
                let id = random()?;
                let bytes = connection.session.request(
                    id,
                    &Op::UpdateChunk {
                        offset: ahead,
                        data: data.unwrap_or(&[0; 512]).to_vec(),
                    },
                )?;
                if connection.stream.write_all(&bytes).is_err() {
                    lost = true;
                }
                unwanted = Some((ahead, Some(id)));
            }
            while !lost && unwanted.is_none() {
                let next = installer.poll();
                let (sent, op) = match next {
                    Next::Send(sent, op) => (sent, op),
                    Next::Wait => break,
                    Next::Reconnect if fault == Fault::GiveWayOnly => {
                        note("Bank 1 gave way; stopping before bank 0's offer".into());
                        return Ok(());
                    }
                    Next::Reconnect => {
                        note("The feeder restarts; waiting for it on bank 0".into());
                        lost = true;
                        break;
                    }
                    Next::Activated => {
                        note("Activated: the feeder restarts into the new build".into());
                        return Ok(());
                    }
                    Next::Failed(error) => return Err(error.into()),
                    Next::Cancelled => {
                        note("Cancelled: bank 0 holds no candidate".into());
                        return Err(DeviceError::Stopped);
                    }
                };

                let op = match (sent, op, fault) {
                    (Sent::Chunk(offset), _, Fault::Stall { at }) if offset >= at => {
                        note(format!(
                            "Stalling at byte {offset} for {} s",
                            STALL.as_secs()
                        ));
                        fault = Fault::None;
                        wait(STALL, stop)?;
                        installer.lost();
                        lost = true;
                        break;
                    }
                    // Every send of the chunk is spoiled, a resend after a
                    // Busy included, so the hash check always sees it.
                    (
                        Sent::Chunk(offset),
                        Op::UpdateChunk { mut data, .. },
                        Fault::Corrupt { at },
                    ) if (offset..offset + wellfed_core::ota::MAX_CHUNK).contains(&at) => {
                        note(format!("Corrupting byte {at}"));
                        data[(at - offset) as usize] ^= 0x10;
                        Op::UpdateChunk { offset, data }
                    }
                    (_, op, _) => op,
                };
                let id = random()?;
                let bytes = connection.session.request(id, &op)?;
                if connection.stream.write_all(&bytes).is_err() {
                    lost = true;
                    break;
                }
                out.push((id, sent, Instant::now()));

                if let (Sent::Chunk(offset), Fault::Unwanted { at }) = (sent, fault)
                    && offset >= at
                {
                    // Two sectors on: the sector this chunk is in may
                    // complete before the stage takes the fault, and then the
                    // next sector's first chunk is the one it wants.
                    let sector = wellfed_core::ota::SECTOR_BYTES;
                    let ahead = (offset / sector + 2) * sector;
                    note(format!("Sending byte {ahead} before it is wanted"));
                    fault = Fault::None;
                    unwanted = Some((ahead, None));
                    break;
                }
            }

            if !lost {
                match read_for(&mut connection, POLL) {
                    Ok(()) => {}
                    Err(DeviceError::Timeout(_)) => {}
                    Err(_) => lost = true,
                }
                loop {
                    let incoming = match connection.session.next_incoming() {
                        Ok(Some(incoming)) => incoming,
                        Ok(None) => break,
                        // The message authenticated, so the feeder sealed
                        // bytes that are not a message of this protocol: a
                        // garbled answer, which a new session resumes past
                        // as it does a dropped one. The feeder's readback
                        // and whole-image hash still guard what is written.
                        Err(SessionError::Malformed(evidence)) if garbled < GARBLED_MAX => {
                            garbled += 1;
                            // Only update ops are outstanding here, so the
                            // plaintext holds no secret.
                            note(format!(
                                "A feeder message did not decode ({garbled} of {GARBLED_MAX} allowed) on bank {bank}: {} bytes, starting {}; outstanding {}; resuming on a new session",
                                evidence.len,
                                hex::encode(evidence.prefix()),
                                outstanding(&out, unwanted)
                            ));
                            lost = true;
                            break;
                        }
                        Err(error) => return Err(error.into()),
                    };
                    let Incoming::Answer { id, answer } = incoming else {
                        continue;
                    };
                    if let Some((ahead, Some(sent))) = unwanted
                        && sent == id
                    {
                        if answer == wellfed_model::Answer::Refused(Refusal::Busy) {
                            unwanted = Some((ahead, None));
                        } else {
                            unwanted = None;
                            installer.answer(Sent::Chunk(ahead), &answer)?;
                        }
                        continue;
                    }
                    let Some(index) = out.iter().position(|(sent, ..)| *sent == id) else {
                        continue;
                    };
                    let (_, sent, _) = out.swap_remove(index);
                    installer.answer(sent, &answer)?;
                    if let Sent::Chunk(offset) = sent {
                        if let Some(view) = installer.view()
                            && view.written >= written + 64 * 1024
                        {
                            written = view.written;
                            note(format!("Written {written} of {} bytes", view.image_len));
                        }
                        if let Fault::Drop { at } = fault
                            && offset >= at
                        {
                            note(format!("Dropping the connection at byte {offset}"));
                            fault = Fault::None;
                            lost = true;
                        }
                    }
                }
                lost |= out.iter().any(|(.., at)| at.elapsed() > ANSWER);
            }

            if lost {
                out.clear();
                if let Some((ahead, Some(_))) = unwanted {
                    unwanted = Some((ahead, None));
                }
                installer.lost();
                self.connection = None;
                // Closed now, not when the next session replaces it: the
                // feeder frees the slot rather than holding it through the
                // wait. A connection the feeder already dropped has nothing
                // left to close.
                let _ = connection.stream.shutdown(Shutdown::Both);
                std::thread::sleep(Duration::from_secs(2));
                connection = match cancelling {
                    // The stop is what the cancel answers, so it does not
                    // end the reconnect: the cancel goes again, within its
                    // own deadline, or no candidate is known to be freed.
                    Some(since) => self
                        .reach(
                            ANSWER.saturating_sub(since.elapsed()),
                            &AtomicBool::new(false),
                        )
                        .map_err(|_| DeviceError::Stopped)?,
                    None => self.reach(REACH, stop)?,
                };
                bank = self.bank(&mut connection)?;
                note(format!("Session open again on bank {bank}"));
                installer.opened(bank);
            }
        }
    }

    /// Connect as `intruder` and report how the feeder met it.
    pub fn intrude(&mut self, intruder: Intruder) -> Result<Rebuffed, DeviceError> {
        let address = match self.address {
            Some(address) => address,
            None => self.locate()?,
        };
        let unenrolled;
        let keys = match intruder {
            Intruder::UnenrolledKey => {
                unenrolled = Keypair::from_secret(random()?);
                &unenrolled
            }
            Intruder::Tampered => &self.keys,
        };
        let (session, mut hello) = Session::connect(keys, self.identity, random()?)?;
        if intruder == Intruder::Tampered
            && let Some(last) = hello.last_mut()
        {
            // The handshake message's tag: the feeder's Noise read fails.
            *last ^= 0x01;
        }
        let mut connection = Connection {
            stream: dial(address)?,
            session,
        };
        connection.stream.write_all(&hello)?;

        let deadline = Instant::now() + ANSWER;
        loop {
            match read(&mut connection, deadline) {
                Ok(()) => {}
                Err(DeviceError::Closed) => return Ok(Rebuffed::Dropped),
                Err(DeviceError::Io(error)) if error.kind() == ErrorKind::ConnectionReset => {
                    return Ok(Rebuffed::Dropped);
                }
                Err(DeviceError::Timeout(_)) => return Ok(Rebuffed::Silent),
                Err(DeviceError::Session(_)) => return Ok(Rebuffed::Answered),
                Err(error) => return Err(error),
            }
            let Ok(next) = connection.session.next_incoming() else {
                return Ok(Rebuffed::Answered);
            };
            match next {
                None => {}
                Some(Incoming::Refused(refusal)) => return Ok(Rebuffed::Refused(refusal)),
                Some(Incoming::Opened(_)) => return Ok(Rebuffed::Opened),
                Some(Incoming::Answer { .. } | Incoming::Pushed(_)) => {
                    return Ok(Rebuffed::Answered);
                }
            }
        }
    }

    /// The feeder's address: over mDNS by its identity, else by its MAC on
    /// the hotspot.
    fn locate(&mut self) -> Result<SocketAddr, DeviceError> {
        let found = self.browse().or_else(|| self.neighbour());
        let address = found.ok_or_else(|| DeviceError::NotFound {
            fingerprint: hex::encode(&self.identity[..8]),
            interface: self.hotspot.hotspot_interface.clone(),
            mac: self.hotspot.target_mac.clone(),
        })?;
        self.address = Some(address);
        Ok(address)
    }

    fn browse(&self) -> Option<SocketAddr> {
        let daemon = mdns_sd::ServiceDaemon::new().ok()?;
        let receiver = daemon.browse("_wellfed._tcp.local.").ok()?;
        let want = hex::encode(&self.identity);
        let deadline = Instant::now() + DISCOVERY;
        let found = loop {
            let left = deadline.saturating_duration_since(Instant::now());
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
        let _ = daemon.shutdown();
        found
    }

    fn neighbour(&self) -> Option<SocketAddr> {
        let interface = &self.hotspot.hotspot_interface;
        if interface.is_empty() || self.hotspot.target_mac.is_empty() {
            return None;
        }
        let output = Command::new("ip")
            .args(["-j", "neigh", "show", "dev", interface])
            .stdin(Stdio::null())
            .output()
            .ok()
            .filter(|output| output.status.success())?;
        let neighbours: Vec<Neighbour> = serde_json::from_slice(&output.stdout).ok()?;
        neighbour_address(&neighbours, &self.hotspot.target_mac)
    }
}

/// The IPv4 address `mac` holds among `neighbours`, on the API port.
fn neighbour_address(neighbours: &[Neighbour], mac: &str) -> Option<SocketAddr> {
    neighbours
        .iter()
        .filter(|entry| {
            entry
                .lladdr
                .as_deref()
                .is_some_and(|lladdr| lladdr.eq_ignore_ascii_case(mac))
        })
        .find_map(|entry| match entry.dst.parse::<IpAddr>() {
            Ok(ip @ IpAddr::V4(_)) => Some(SocketAddr::new(ip, PORT)),
            _ => None,
        })
}

fn dial(address: SocketAddr) -> Result<TcpStream, DeviceError> {
    let stream = TcpStream::connect_timeout(&address, CONNECT)
        .map_err(|error| DeviceError::Connect(address, error))?;
    stream.set_nodelay(true)?;
    Ok(stream)
}

/// An interruptible sleep.
fn wait(time: Duration, stop: &AtomicBool) -> Result<(), DeviceError> {
    let deadline = Instant::now() + time;
    while Instant::now() < deadline {
        if stop.load(Ordering::Acquire) {
            return Err(DeviceError::Stopped);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Ok(())
}

/// The install's requests still unanswered, for a garbled message's note:
/// which one the garbled answer could have been.
fn outstanding(
    out: &[(RequestId, Sent, Instant)],
    unwanted: Option<(u32, Option<RequestId>)>,
) -> String {
    let mut sent: Vec<String> = out.iter().map(|(_, sent, _)| format!("{sent:?}")).collect();
    if let Some((ahead, Some(_))) = unwanted {
        sent.push(format!("unwanted Chunk({ahead})"));
    }
    if sent.is_empty() {
        "none".into()
    } else {
        sent.join(", ")
    }
}

/// Read what arrives within `time` into the session.
fn read_for(connection: &mut Connection, time: Duration) -> Result<(), DeviceError> {
    read(connection, Instant::now() + time)
}

/// Read what arrives before `deadline` into the session.
fn read(connection: &mut Connection, deadline: Instant) -> Result<(), DeviceError> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err(DeviceError::Timeout(ANSWER));
    }
    connection.stream.set_read_timeout(Some(left))?;
    let mut buffer = [0u8; 2048];
    let n = match connection.stream.read(&mut buffer) {
        Ok(n) => n,
        Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
            return Err(DeviceError::Timeout(ANSWER));
        }
        Err(error) => return Err(error.into()),
    };
    if n == 0 {
        return Err(DeviceError::Closed);
    }
    connection.session.receive(&buffer[..n])?;
    Ok(())
}

#[cfg(test)]
mod install_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_neighbour_table_gives_the_feeder_s_ipv4_address_on_the_api_port() {
        let table: Vec<Neighbour> = serde_json::from_str(
            r#"[{"dst":"10.42.0.9","lladdr":"aa:bb:cc:00:00:01","state":["STALE"]},
                {"dst":"fe80::1","lladdr":"AA:BB:CC:DD:EE:FF","state":["REACHABLE"]},
                {"dst":"10.42.0.23","lladdr":"AA:BB:CC:DD:EE:FF","state":["REACHABLE"]},
                {"dst":"10.42.0.40","state":["INCOMPLETE"]}]"#,
        )
        .unwrap();

        assert_eq!(
            neighbour_address(&table, "aa:bb:cc:dd:ee:ff"),
            Some("10.42.0.23:4747".parse().unwrap())
        );
        assert_eq!(neighbour_address(&table, "aa:bb:cc:dd:ee:00"), None);
    }

    #[test]
    fn a_status_matches_only_its_build_on_bank_1() {
        let status = DeviceStatus {
            build: "8a13e59efa15e56b".into(),
            bank: 1,
            uptime_ms: 61_000,
            boot_count: 4,
            wifi: WifiSource::Build,
            losses: 0,
            clock_ms: None,
            settings_generation: 3,
            wifi_generation: 1,
            locked: Some(false),
        };
        assert_eq!(
            status.matches("8a13e59efa15e56b").unwrap(),
            "Status: build 8a13e59efa15 on bank 1, up 61 s, boot 4, Wi-Fi Build, clock not set"
        );
        assert_eq!(
            status.matches("0123").unwrap_err(),
            NotReady::Running("8a13e59efa15".into())
        );
        let bank0 = DeviceStatus { bank: 0, ..status };
        assert_eq!(
            bank0.matches("8a13e59efa15e56b").unwrap_err(),
            NotReady::OnBank(0)
        );
    }

    #[test]
    fn feeds_and_cancels_are_never_resent() {
        assert!(!Ask::Feed { portions: 2 }.resendable());
        assert!(!Ask::CancelFeed { feed: 7 }.resendable());
        assert!(Ask::SetLock { locked: true }.resendable());
        assert!(Ask::Status.resendable());
    }

    #[test]
    fn credentials_stay_out_of_debug() {
        let ask = Ask::PutWifi {
            base: 1,
            wifi: Some(Credentials {
                ssid: "lab".into(),
                psk: "secret-passphrase".into(),
            }),
        };
        assert_eq!(format!("{ask:?}"), "put-wifi");
    }
}
