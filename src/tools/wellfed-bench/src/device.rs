//! The feeder's device API (FW47) as the bench speaks it: a Noise session
//! over TCP port 4747 as the owner's enrolled key (`wellfed setup` wrote
//! it to `private/wellfed-owner.json`), carrying the requests the suite and
//! the `ota` command make.
//!
//! Blocking, like the rest of the bench. It mirrors the `wellfed` CLI's
//! tokio `device.rs` (find the feeder, open a session, ask): both only move
//! bytes for `wellfed_client::api::Session`, and should share one crate
//! once there is one for it.
//!
//! The feeder is found by its identity over mDNS (`_wellfed._tcp`, TXT
//! `id=`), and otherwise by its MAC in the hotspot's neighbour table: the
//! bench host's firewall drops inbound UDP 5353, so mDNS answers may never
//! arrive. The address and the session are kept between requests; a request
//! on a kept session that fails is sent once more on a fresh one under the
//! same id, which the feeder answers without running a write twice.

use std::fmt;
use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use wellfed_client::api::{Incoming, Session};
use wellfed_core::noise::Keypair;
use wellfed_formats::hex;
use wellfed_formats::owner::OwnerKeyFile;
use wellfed_formats::private::{self, SYSTEM_RANDOM};
use wellfed_proto::api::{
    FeederMessage, Op, OpenRefusal, Outcome, PORT, Refusal, RequestId, Settings, SettingsChanges,
    Status, Welcome, Wifi, WifiSource,
};
use serde::{Deserialize, Serialize};

use crate::server::NotReady;

/// The owner key file the bench opens sessions with (root-relative).
pub(crate) const OWNER_KEYS: &str = "private/wellfed-owner.json";

/// mDNS answers on a LAN arrive within a second or two.
const DISCOVERY: Duration = Duration::from_secs(4);
const CONNECT: Duration = Duration::from_secs(5);
/// A request is answered at once, or after its store write.
const ANSWER: Duration = Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    #[error("{}: {source}", path.display())]
    Keys {
        path: PathBuf,
        #[source]
        source: wellfed_formats::Error,
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
    #[error("time zone {0:?}: {1}")]
    Zone(String, jiff::Error),
    #[error(transparent)]
    Session(#[from] wellfed_client::api::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// What the suite asks the feeder; borrowed into an [`Op`] when sent.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Ask {
    Status,
    GetSettings,
    PutSettings {
        base: u32,
        changes: SettingsChanges,
    },
    /// Credentials for the next join, or `None` to clear them.
    PutWifi {
        base: u32,
        wifi: Option<Credentials>,
    },
    /// The host's wall clock and the server config's time zone, read when
    /// the request goes out.
    SyncClock,
}

impl Ask {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::GetSettings => "get-settings",
            Self::PutSettings { .. } => "put-settings",
            Self::PutWifi { .. } => "put-wifi",
            Self::SyncClock => "time-sync",
        }
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
pub(crate) struct Credentials {
    pub ssid: String,
    pub psk: String,
}

/// The feeder's answer, owned so it outlives the session's buffer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) enum Answer {
    Done {
        generation: Option<u32>,
    },
    Status(DeviceStatus),
    Settings(Settings),
    Refused(Refusal),
    /// An answer no [`Ask`] expects (clients, an invite).
    Unexpected,
}

impl Answer {
    fn of(outcome: Outcome<'_>) -> Self {
        match outcome {
            Outcome::Done { generation } => Self::Done { generation },
            Outcome::Status(status) => Self::Status(DeviceStatus::of(&status)),
            Outcome::Settings(settings) => Self::Settings(settings),
            Outcome::Refused(refusal) => Self::Refused(refusal),
            Outcome::Clients(_) | Outcome::Invite(_) => Self::Unexpected,
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
}

impl DeviceStatus {
    fn of(status: &Status<'_>) -> Self {
        Self {
            build: status.build.to_ascii_lowercase(),
            bank: status.bank,
            uptime_ms: status.uptime_ms,
            boot_count: status.boot_count,
            wifi: status.wifi,
            losses: status.link_losses.saturating_add(status.dns_losses),
            clock_ms: status.clock_ms,
            settings_generation: status.settings_generation,
            wifi_generation: status.wifi_generation,
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

/// A connection the feeder must not let in, for `request-auth`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Intruder {
    /// A well-formed session from a key the feeder never enrolled.
    UnenrolledKey,
    /// The owner's own first handshake message with one byte flipped.
    Tampered,
}

/// How the feeder met an [`Intruder`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) enum Rebuffed {
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

/// The parts of `wellfed-local`'s config the device client uses.
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
/// lookup as `wellfed-local`'s `neighbour::discover`.
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
pub(crate) struct Device {
    keys: Keypair,
    identity: [u8; 32],
    hotspot: Hotspot,
    zone: jiff::tz::TimeZone,
    address: Option<SocketAddr>,
    connection: Option<Connection>,
}

fn random<const N: usize>() -> Result<[u8; N], DeviceError> {
    private::random(Path::new(SYSTEM_RANDOM)).map_err(DeviceError::Random)
}

impl Device {
    /// The key file at `keys` and the hotspot fields of the server config
    /// at `config`, both under `root`.
    pub(crate) fn open(root: &Path, keys: &Path, config: &Path) -> crate::Result<Self> {
        let file: OwnerKeyFile =
            wellfed_formats::load(&root.join(keys)).map_err(|source| DeviceError::Keys {
                path: keys.to_path_buf(),
                source,
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
            connection: None,
        })
    }

    /// One request and its answer.
    pub(crate) fn ask(&mut self, ask: &Ask) -> Result<Answer, DeviceError> {
        let id: RequestId = random()?;
        let reused = self.connection.is_some();
        match self.exchange(id, ask) {
            // A kept session may have died with a reboot or a bounce.
            Err(_) if reused => self.exchange(id, ask),
            other => other,
        }
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
                base: *base,
                changes: *changes,
            },
            Ask::PutWifi { base, wifi } => Op::PutWifi {
                base: *base,
                wifi: wifi.as_ref().map(|wifi| Wifi {
                    ssid: wifi.ssid.as_bytes(),
                    psk: &wifi.psk,
                }),
            },
            Ask::SyncClock => Op::TimeSync(wellfed_client::time::sync(
                jiff::Timestamp::now(),
                &self.zone,
            )),
        };
        let bytes = connection.session.request(id, op)?;
        connection.stream.write_all(&bytes)?;

        let deadline = Instant::now() + ANSWER;
        loop {
            while let Some(incoming) = connection.session.next_incoming()? {
                if let Incoming::Message(FeederMessage::Response { id: got, result }) = incoming
                    && got == id
                {
                    return Ok(Answer::of(result));
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
            Err(DeviceError::Connect(..) | DeviceError::Timeout(_)) if known.is_some() => {
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
                Incoming::Welcome(Welcome::Open { .. }) => Ok(connection),
                Incoming::Welcome(Welcome::Refused(refusal)) => Err(DeviceError::Refused(refusal)),
                Incoming::Message(_) => Err(DeviceError::Unwelcome),
            };
        }
    }

    /// Connect as `intruder` and report how the feeder met it.
    pub(crate) fn intrude(&mut self, intruder: Intruder) -> Result<Rebuffed, DeviceError> {
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
                Some(Incoming::Welcome(Welcome::Refused(refusal))) => {
                    return Ok(Rebuffed::Refused(refusal));
                }
                Some(Incoming::Welcome(Welcome::Open { .. })) => return Ok(Rebuffed::Opened),
                Some(Incoming::Message(_)) => return Ok(Rebuffed::Answered),
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
