//! A device API session over TCP (FW47): find the feeder over mDNS, open
//! `petlibre-client`'s session against its identity, and move bytes.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use petlibre_client::api::{Incoming, Session};
use petlibre_formats::hex;
use petlibre_formats::private;
use petlibre_proto::api::{FeederMessage, Op, OpenRefusal, Outcome, PORT, RequestId, Welcome};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::keys::OwnerKey;

/// mDNS answers on a LAN arrive within a second or two.
const DISCOVERY: Duration = Duration::from_secs(6);
const CONNECT: Duration = Duration::from_secs(5);
/// A request is answered at once, or after its store write.
pub(crate) const ANSWER: Duration = Duration::from_secs(10);

/// The feeder's address: given, or found over mDNS by its identity.
pub(crate) fn locate(identity: &[u8; 32], address: Option<&str>) -> Result<SocketAddr> {
    if let Some(address) = address {
        if let Ok(ip) = address.parse::<IpAddr>() {
            return Ok(SocketAddr::new(ip, PORT));
        }
        return address
            .parse()
            .with_context(|| format!("{address}: not an address or address:port"));
    }

    let daemon = mdns_sd::ServiceDaemon::new().context("start mDNS")?;
    let receiver = daemon
        .browse("_petlibre._tcp.local.")
        .context("browse mDNS")?;
    let want = hex::encode(identity);
    let deadline = std::time::Instant::now() + DISCOVERY;
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
        let ip = info.get_addresses_v4().into_iter().next();
        if let Some(ip) = ip {
            break Some(SocketAddr::new(IpAddr::V4(ip), info.get_port()));
        }
    };
    let _ = daemon.shutdown();
    found.with_context(|| {
        format!(
            "no feeder {} answered over mDNS; pass --address",
            &want[..16]
        )
    })
}

/// An open session.
pub(crate) struct Connection {
    stream: TcpStream,
    session: Session,
    pub(crate) build: String,
    pub(crate) role: petlibre_proto::api::Role,
}

impl Connection {
    pub(crate) async fn open(
        key: &OwnerKey,
        identity: [u8; 32],
        address: SocketAddr,
    ) -> Result<Self> {
        let ephemeral: [u8; 32] =
            private::random(OwnerKey::system()).context("read the system random source")?;
        let (session, hello) = Session::connect(key.keypair(), identity, ephemeral)?;
        let mut stream = timeout(CONNECT, TcpStream::connect(address))
            .await
            .with_context(|| format!("connect to {address}: timed out"))?
            .with_context(|| format!("connect to {address}"))?;
        stream.set_nodelay(true)?;
        stream.write_all(&hello).await?;

        let mut connection = Self {
            stream,
            session,
            build: String::new(),
            role: petlibre_proto::api::Role::Member,
        };
        loop {
            connection.read(ANSWER).await?;
            let Some(incoming) = connection.session.next_incoming()? else {
                continue;
            };
            let Incoming::Welcome(welcome) = incoming else {
                bail!("the feeder spoke before its welcome");
            };
            match welcome {
                Welcome::Open { build, role, .. } => {
                    connection.build = build.to_owned();
                    connection.role = role;
                    return Ok(connection);
                }
                Welcome::Refused(OpenRefusal::Unknown) => {
                    bail!("the feeder does not know this key (not enrolled, or revoked)")
                }
                Welcome::Refused(OpenRefusal::Version { supported }) => {
                    bail!("the feeder serves protocol {supported}; update this tool or the feeder")
                }
                Welcome::Refused(OpenRefusal::Full) => bail!("every session slot is taken"),
            }
        }
    }

    /// Read what arrives within `wait` into the session.
    async fn read(&mut self, wait: Duration) -> Result<()> {
        let mut buffer = [0u8; 2048];
        let n = timeout(wait, self.stream.read(&mut buffer))
            .await
            .context("the feeder did not answer in time")??;
        if n == 0 {
            bail!("the feeder closed the connection");
        }
        self.session.receive(&buffer[..n])?;
        Ok(())
    }

    pub(crate) async fn send_request(&mut self, id: RequestId, op: Op<'_>) -> Result<()> {
        let bytes = self.session.request(id, op)?;
        self.stream.write_all(&bytes).await?;
        Ok(())
    }

    pub(crate) async fn subscribe(&mut self, after: Option<u64>, samples: bool) -> Result<()> {
        let bytes = self.session.subscribe(after, samples)?;
        self.stream.write_all(&bytes).await?;
        Ok(())
    }

    /// The next message, re-encoded so it outlives the session's buffer;
    /// decode it with [`petlibre_proto::decode`] as a `FeederMessage`.
    pub(crate) async fn next(&mut self, wait: Duration) -> Result<Vec<u8>> {
        loop {
            if let Some(incoming) = self.session.next_incoming()? {
                let Incoming::Message(message) = incoming else {
                    continue;
                };
                let mut out = vec![0; petlibre_proto::api::FRAME_MAX];
                let len = petlibre_proto::encode(&message, &mut out)
                    .map_err(|e| anyhow::anyhow!("re-encode: {e}"))?;
                out.truncate(len);
                return Ok(out);
            }
            self.read(wait).await?;
        }
    }

    /// One request and its answer (other messages are dropped meanwhile).
    pub(crate) async fn ask(&mut self, op: Op<'_>) -> Result<Vec<u8>> {
        let id: RequestId =
            private::random(OwnerKey::system()).context("read the system random source")?;
        self.send_request(id, op).await?;
        loop {
            let bytes = self.next(ANSWER).await?;
            let message: FeederMessage<'_> = petlibre_proto::decode(&bytes)?;
            if let FeederMessage::Response { id: got, result } = message
                && got == id
            {
                let mut out = vec![0; petlibre_proto::api::FRAME_MAX];
                let len = petlibre_proto::encode(&result, &mut out)?;
                out.truncate(len);
                return Ok(out);
            }
        }
    }
}

/// An answer's outcome, borrowed from its bytes.
pub(crate) fn outcome(bytes: &[u8]) -> Result<Outcome<'_>> {
    Ok(petlibre_proto::decode(bytes)?)
}
