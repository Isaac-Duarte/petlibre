//! FW08 local health transport: an MQTT 3.1.1 client subset, its session
//! state machine and the health encoder. Since FW47 it carries only health
//! and the FW10 offer check and download; requests go over the device API,
//! and the mailbox types the API fills (settings, Wi-Fi, lock, tune, clock)
//! still live here until FW48 retires the broker.
//!
//! Sans-IO: the caller owns the socket, feeds received bytes in and writes the
//! returned bytes out. Nothing here allocates. This module reaches no other
//! part of the crate (a repository check enforces it), so nothing reachable
//! from the network can touch feeding, outputs, settings, flash or updates.
//! See docs/firmware/local-transport.md.

use core::fmt::{self, Write};

/// Largest packet either side may send; bigger input closes the session.
pub const MAX_PACKET: usize = 832;
/// Health payload limit (512 bytes in FW08, 640 with the clock and nonce,
/// 704 with the FW14 power inputs).
pub const MAX_HEALTH: usize = 704;
/// Largest payload on the session's subscribed topic (an FW10 offer).
pub const MAX_PAYLOAD: usize = 448;
pub const KEEPALIVE_S: u16 = 30;
/// Send PINGREQ after this long without sending anything.
pub const PING_AFTER_MS: u32 = 15_000;
/// Close when a CONNACK, SUBACK or PINGRESP takes longer than this.
pub const REPLY_TIMEOUT_MS: u32 = 10_000;
/// Close after this long with nothing received (1.5 keepalives).
pub const SILENCE_MS: u32 = 45_000;
pub const HEALTH_PERIOD_MS: u32 = 30_000;

pub const SCHEMA: &str = "petlibre.health";
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The output buffer is too small, or a field exceeds its MQTT length.
    TooLong,
    /// A packet violates the subset this client accepts.
    Malformed,
    /// A well-formed packet this client never expects (or a QoS > 0 publish).
    Unexpected,
}

/// Fixed-capacity output cursor.
struct Out<'a> {
    buf: &'a mut [u8],
    at: usize,
}

impl<'a> Out<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, at: 0 }
    }

    fn byte(&mut self, value: u8) -> Result<(), Error> {
        *self.buf.get_mut(self.at).ok_or(Error::TooLong)? = value;
        self.at += 1;
        Ok(())
    }

    fn bytes(&mut self, value: &[u8]) -> Result<(), Error> {
        let end = self.at.checked_add(value.len()).ok_or(Error::TooLong)?;
        self.buf
            .get_mut(self.at..end)
            .ok_or(Error::TooLong)?
            .copy_from_slice(value);
        self.at = end;
        Ok(())
    }

    fn u16(&mut self, value: u16) -> Result<(), Error> {
        self.bytes(&value.to_be_bytes())
    }

    /// Length-prefixed MQTT string or binary field.
    fn field(&mut self, value: &[u8]) -> Result<(), Error> {
        self.u16(u16::try_from(value.len()).map_err(|_| Error::TooLong)?)?;
        self.bytes(value)
    }
}

fn remaining_length(length: usize) -> Result<([u8; 4], usize), Error> {
    if length >= MAX_PACKET {
        return Err(Error::TooLong);
    }

    let mut out = [0; 4];
    let mut value = length;
    let mut used = 0;

    loop {
        let mut digit = (value % 128).to_le_bytes()[0];
        value /= 128;
        if value > 0 {
            digit |= 0x80;
        }
        out[used] = digit;
        used += 1;
        if value == 0 {
            return Ok((out, used));
        }
    }
}

/// One whole packet: fixed header, then `body` written by `write`.
fn packet(
    buf: &mut [u8],
    header: u8,
    length: usize,
    write: impl FnOnce(&mut Out) -> Result<(), Error>,
) -> Result<usize, Error> {
    let (encoded, used) = remaining_length(length)?;
    let mut out = Out::new(buf);
    out.byte(header)?;
    out.bytes(&encoded[..used])?;

    let start = out.at;
    write(&mut out)?;
    // A length mismatch is a bug in this module, not a runtime condition.
    debug_assert_eq!(out.at - start, length);
    Ok(out.at)
}

fn field_len(value: &[u8]) -> usize {
    2 + value.len()
}

#[derive(Clone, Copy, Debug)]
pub struct Will<'a> {
    pub topic: &'a str,
    pub payload: &'a [u8],
}

/// CONNECT parameters. The session is always clean; the will is QoS 0 and not
/// retained (the server's broker has retained messages disabled).
#[derive(Clone, Copy, Debug)]
pub struct Connect<'a> {
    pub client_id: &'a str,
    pub username: &'a str,
    pub password: &'a [u8],
    pub will: Option<Will<'a>>,
}

pub fn encode_connect(buf: &mut [u8], c: &Connect) -> Result<usize, Error> {
    let mut flags = 0x80 | 0x40 | 0x02; // username, password, clean session
    let mut length = field_len(b"MQTT") + 1 + 1 + 2 + field_len(c.client_id.as_bytes());
    if let Some(w) = c.will {
        flags |= 0x04; // will flag, QoS 0, not retained
        length += field_len(w.topic.as_bytes()) + field_len(w.payload);
    }
    length += field_len(c.username.as_bytes()) + field_len(c.password);

    packet(buf, 0x10, length, |o| {
        o.field(b"MQTT")?;
        o.byte(4)?; // protocol level 3.1.1
        o.byte(flags)?;
        o.u16(KEEPALIVE_S)?;
        o.field(c.client_id.as_bytes())?;
        if let Some(w) = c.will {
            o.field(w.topic.as_bytes())?;
            o.field(w.payload)?;
        }
        o.field(c.username.as_bytes())?;
        o.field(c.password)
    })
}

/// QoS 0 PUBLISH.
pub fn encode_publish(
    buf: &mut [u8],
    topic: &str,
    payload: &[u8],
    retain: bool,
) -> Result<usize, Error> {
    let length = field_len(topic.as_bytes()) + payload.len();
    packet(buf, 0x30 | u8::from(retain), length, |o| {
        o.field(topic.as_bytes())?;
        o.bytes(payload)
    })
}

/// SUBSCRIBE to one topic at QoS 0.
pub fn encode_subscribe(buf: &mut [u8], packet_id: u16, topic: &str) -> Result<usize, Error> {
    let length = 2 + field_len(topic.as_bytes()) + 1;
    packet(buf, 0x82, length, |o| {
        o.u16(packet_id)?;
        o.field(topic.as_bytes())?;
        o.byte(0)
    })
}

pub fn encode_pingreq(buf: &mut [u8]) -> Result<usize, Error> {
    packet(buf, 0xc0, 0, |_| Ok(()))
}

pub fn encode_disconnect(buf: &mut [u8]) -> Result<usize, Error> {
    packet(buf, 0xe0, 0, |_| Ok(()))
}

/// A packet the broker may send this client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Packet<'a> {
    ConnAck {
        session_present: bool,
        code: u8,
    },
    SubAck {
        packet_id: u16,
        code: u8,
    },
    PingResp,
    Publish {
        topic: &'a str,
        payload: &'a [u8],
        retain: bool,
    },
}

fn u16_at(data: &[u8], at: usize) -> Result<u16, Error> {
    let bytes = data.get(at..at + 2).ok_or(Error::Malformed)?;
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}

/// Decode the first packet in `data`. `Ok(None)` means more bytes are needed;
/// otherwise the packet and the bytes it used.
pub fn decode(data: &[u8]) -> Result<Option<(Packet<'_>, usize)>, Error> {
    let Some(&header) = data.first() else {
        return Ok(None);
    };

    let mut length = 0usize;
    let mut at = 1;
    loop {
        let Some(&digit) = data.get(at) else {
            return Ok(None);
        };
        length |= usize::from(digit & 0x7f) << (7 * (at - 1));
        at += 1;
        if digit & 0x80 == 0 {
            break;
        }
        // MQTT caps the remaining length at four bytes.
        if at > 4 {
            return Err(Error::Malformed);
        }
    }

    if length >= MAX_PACKET {
        return Err(Error::TooLong);
    }
    let Some(body) = data.get(at..at + length) else {
        return Ok(None);
    };
    let used = at + length;

    let packet = match (header >> 4, header & 0x0f) {
        (2, 0) if length == 2 => {
            if body[0] & 0xfe != 0 {
                return Err(Error::Malformed);
            }
            Packet::ConnAck {
                session_present: body[0] & 1 == 1,
                code: body[1],
            }
        }
        (9, 0) if length == 3 => Packet::SubAck {
            packet_id: u16_at(body, 0)?,
            code: body[2],
        },
        (13, 0) if length == 0 => Packet::PingResp,

        (3, flags) => {
            if flags & 0x06 != 0 {
                return Err(Error::Unexpected); // QoS 1/2 never requested
            }
            let topic_len = usize::from(u16_at(body, 0)?);
            let topic = body.get(2..2 + topic_len).ok_or(Error::Malformed)?;
            let topic = core::str::from_utf8(topic).map_err(|_| Error::Malformed)?;
            Packet::Publish {
                topic,
                payload: &body[2 + topic_len..],
                retain: flags & 1 == 1,
            }
        }

        // A packet type this client accepts, but with the wrong flags or length.
        (2 | 9 | 13, _) => return Err(Error::Malformed),
        _ => return Err(Error::Unexpected),
    };

    Ok(Some((packet, used)))
}

/// Why a session closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Refused(u8),
    SubscribeRefused(u8),
    Protocol(Error),
    /// A packet arrived in a state that does not allow it.
    OutOfOrder,
    ReplyTimeout,
    Silence,
    /// A publish on a topic other than the request topic.
    ForeignTopic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    AwaitConnAck,
    AwaitSubAck,
    Ready,
    Closed(Fault),
}

/// What the caller should do after a packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event<'a> {
    /// CONNACK accepted: send the SUBSCRIBE from [`Session::subscribe`].
    Connected,
    /// SUBACK accepted: the session is ready; publish health now.
    Ready,
    Pong,
    /// A payload on the subscribed topic (an offer, for the FW10 check).
    Request(&'a [u8]),
    /// SUBACK for [`Session::subscribe_data`]: data publishes may now arrive.
    DataReady,
    /// A payload on the data topic named to [`Session::on_packet_with`].
    Data(&'a [u8]),
    Closed(Fault),
}

/// Ping or close decision from [`Session::tick`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tick {
    Idle,
    Ping,
    Closed(Fault),
}

const SUBSCRIBE_ID: u16 = 1;
/// The optional second subscription (the updater's `ota/data`).
const DATA_SUBSCRIBE_ID: u16 = 2;

/// One connection's protocol state. A new connection gets a new session.
#[derive(Clone, Copy, Debug)]
pub struct Session {
    state: State,
    /// Last time anything was sent or received.
    sent_ms: u32,
    heard_ms: u32,
    /// When the outstanding CONNACK, SUBACK or PINGRESP was asked for.
    waiting_since: Option<u32>,
}

impl Session {
    /// The caller has just sent CONNECT at `now`.
    pub fn connecting(now: u32) -> Self {
        Self {
            state: State::AwaitConnAck,
            sent_ms: now,
            heard_ms: now,
            waiting_since: Some(now),
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    fn close(&mut self, fault: Fault) -> Fault {
        self.state = State::Closed(fault);
        fault
    }

    /// Write the SUBSCRIBE that must follow [`Event::Connected`].
    pub fn subscribe(&mut self, now: u32, buf: &mut [u8], topic: &str) -> Result<usize, Error> {
        let used = encode_subscribe(buf, SUBSCRIBE_ID, topic)?;
        self.sent(now);
        self.waiting_since = Some(now);
        Ok(used)
    }

    /// After [`Event::Ready`]: subscribe the data topic too (packet ID 2).
    /// Only the updater does this; [`Event::DataReady`] follows its SUBACK.
    pub fn subscribe_data(
        &mut self,
        now: u32,
        buf: &mut [u8],
        topic: &str,
    ) -> Result<usize, Error> {
        if self.state != State::Ready || self.waiting_since.is_some() {
            return Err(Error::Malformed);
        }
        let used = encode_subscribe(buf, DATA_SUBSCRIBE_ID, topic)?;
        self.sent(now);
        self.waiting_since = Some(now);
        Ok(used)
    }

    /// Record that the caller sent bytes (a publish or ping) at `now`.
    pub fn sent(&mut self, now: u32) {
        self.sent_ms = now;
    }

    /// Ping when nothing has been sent for a while; close on timeouts.
    pub fn tick(&mut self, now: u32) -> Tick {
        if let State::Closed(fault) = self.state {
            return Tick::Closed(fault);
        }

        if let Some(since) = self.waiting_since
            && now.wrapping_sub(since) >= REPLY_TIMEOUT_MS
        {
            return Tick::Closed(self.close(Fault::ReplyTimeout));
        }

        if now.wrapping_sub(self.heard_ms) >= SILENCE_MS {
            return Tick::Closed(self.close(Fault::Silence));
        }

        if self.state == State::Ready
            && self.waiting_since.is_none()
            && now.wrapping_sub(self.sent_ms) >= PING_AFTER_MS
        {
            self.sent_ms = now;
            self.waiting_since = Some(now);
            return Tick::Ping;
        }

        Tick::Idle
    }

    /// Handle one decoded packet; `request_topic` is the only accepted
    /// incoming publish topic.
    pub fn on_packet<'a>(
        &mut self,
        now: u32,
        packet: Packet<'a>,
        request_topic: &str,
    ) -> Event<'a> {
        self.on_packet_with(now, packet, request_topic, None)
    }

    /// As [`Session::on_packet`], also accepting publishes on `data_topic`
    /// (once subscribed) as [`Event::Data`]. Any other topic still closes.
    pub fn on_packet_with<'a>(
        &mut self,
        now: u32,
        packet: Packet<'a>,
        request_topic: &str,
        data_topic: Option<&str>,
    ) -> Event<'a> {
        if let State::Closed(fault) = self.state {
            return Event::Closed(fault);
        }
        self.heard_ms = now;

        match (self.state, packet) {
            (
                State::AwaitConnAck,
                Packet::ConnAck {
                    session_present: false,
                    code: 0,
                },
            ) => {
                self.state = State::AwaitSubAck;
                self.waiting_since = None;
                Event::Connected
            }
            (State::AwaitConnAck, Packet::ConnAck { code, .. }) if code != 0 => {
                Event::Closed(self.close(Fault::Refused(code)))
            }

            (
                State::AwaitSubAck,
                Packet::SubAck {
                    packet_id: SUBSCRIBE_ID,
                    code: 0,
                },
            ) => {
                self.state = State::Ready;
                self.waiting_since = None;
                Event::Ready
            }
            (
                State::AwaitSubAck,
                Packet::SubAck {
                    packet_id: SUBSCRIBE_ID,
                    code,
                },
            ) => Event::Closed(self.close(Fault::SubscribeRefused(code))),

            (
                State::Ready,
                Packet::SubAck {
                    packet_id: DATA_SUBSCRIBE_ID,
                    code: 0,
                },
            ) if data_topic.is_some() && self.waiting_since.is_some() => {
                self.waiting_since = None;
                Event::DataReady
            }
            (
                State::Ready,
                Packet::SubAck {
                    packet_id: DATA_SUBSCRIBE_ID,
                    code,
                },
            ) if data_topic.is_some() => Event::Closed(self.close(Fault::SubscribeRefused(code))),

            (State::Ready, Packet::PingResp) if self.waiting_since.is_some() => {
                self.waiting_since = None;
                Event::Pong
            }

            (
                State::Ready,
                Packet::Publish {
                    topic,
                    payload,
                    retain: false,
                },
            ) => {
                if data_topic == Some(topic) {
                    return Event::Data(payload);
                }
                if topic != request_topic {
                    return Event::Closed(self.close(Fault::ForeignTopic));
                }
                if payload.len() > MAX_PAYLOAD {
                    return Event::Closed(self.close(Fault::Protocol(Error::TooLong)));
                }
                Event::Request(payload)
            }

            _ => Event::Closed(self.close(Fault::OutOfOrder)),
        }
    }

    /// Feed a protocol error from [`decode`].
    pub fn on_error(&mut self, error: Error) -> Fault {
        self.close(Fault::Protocol(error))
    }
}

/// The fields of a `settings` request; none present reads the settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SettingsFields {
    pub lights: Option<bool>,
    pub sound: Option<bool>,
    pub volume: Option<u8>,
}

impl SettingsFields {
    /// Whether the request changes anything (and so must be signed).
    pub fn changes(&self) -> bool {
        self.lights.is_some() || self.sound.is_some() || self.volume.is_some()
    }
}

/// The fields of a `lock` request; none present reads the lock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LockFields {
    pub locked: Option<bool>,
}

impl LockFields {
    /// Whether the request changes anything (and so must be signed).
    pub fn changes(&self) -> bool {
        self.locked.is_some()
    }
}

/// Largest `volume` a `settings` request may set, and the store accepts.
pub const MAX_VOLUME: u8 = 100;

/// The fields of a `time` request, as sent (validated by the clock).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TimeFields {
    pub unix_ms: u64,
    pub offset_s: i32,
    pub next_change_s: u64,
    pub next_offset_s: i32,
}

/// `fmt::Write` into a fixed buffer; overflow is an error, not truncation.
pub struct Buffer<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl<'a> Buffer<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, len: 0 }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

impl Write for Buffer<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let end = self.len.checked_add(s.len()).ok_or(fmt::Error)?;
        self.buf
            .get_mut(self.len..end)
            .ok_or(fmt::Error)?
            .copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

/// Wi-Fi loss cause, as the FW07 shim reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Loss {
    None,
    Link,
    Dns,
}

/// Output state as health reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outputs {
    /// Motor STOP and both emitters OFF, freshly read back.
    Stopped,
    /// Motor STOP read back; emitters on under the supervised sensor probe.
    Sensing,
    Unverified,
}

/// Which role is reporting. The bank-0 recovery role reports while it waits
/// for an update, so a feeder whose bank 1 is invalid (just installed by
/// stock, FW40) is still visible; only that role writes the field, so normal
/// health is unchanged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Role {
    #[default]
    Normal,
    Recovery,
}

/// Everything a health message reports. Built by the board port from data it
/// already has; the transport only formats it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Health {
    pub online: bool,
    pub seq: u32,
    pub uptime_ms: u32,

    /// The embedded startup identity (hex on SDK builds).
    pub build: &'static str,
    pub bank: u8,
    pub reset_flags: u32,
    pub boot_count: u32,
    pub outputs: Outputs,

    /// Bit set of [`FAULTS`].
    pub faults: u32,

    pub rssi: i32,
    pub wifi_attempt: u32,
    pub link_losses: u32,
    pub dns_losses: u32,
    pub last_loss: Loss,

    pub connects: u32,
    /// FW25: which credentials joined this boot.
    pub wifi_source: WifiSource,
    /// Fault code of the last closed session, if any.
    pub last_error: Option<&'static str>,

    /// FW21: the wall clock, `None` while invalid.
    pub clock: Option<ClockHealth>,
    pub clock_syncs: u32,
    /// FW14: the power inputs, `None` without a fresh sample.
    pub power: Option<PowerHealth>,
    pub role: Role,
}

/// A fresh power sample as health reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PowerHealth {
    /// PA19: the adapter is connected.
    pub adapter: bool,
    /// The pack voltage, `None` when no pack is fitted.
    pub battery_mv: Option<u32>,
    /// Milliseconds since the sample was taken.
    pub age_ms: u32,
}

/// A valid wall-clock reading as health reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockHealth {
    pub unix_ms: u64,
    pub offset_s: i32,
    pub uncertainty_ms: u64,
}

/// Fault names by bit, in health order.
pub const FAULTS: [&str; 3] = ["watchdog", "flash_hook", "stopped_readback"];

impl Health {
    pub fn write(&self, out: &mut impl Write) -> fmt::Result {
        let build = self.build;
        // `build` is written unescaped, so refuse anything that could break the JSON.
        if build.len() > 64 || !build.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(fmt::Error);
        }

        write!(
            out,
            "{{\"schema\":\"{SCHEMA}\",\"v\":{SCHEMA_VERSION},\"online\":{},\"seq\":{},\"uptime_ms\":{},\
             \"build\":\"{build}\",\"bank\":{},\"reset_flags\":\"{:08x}\",\"boot_count\":{},\"outputs\":\"{}\",\"faults\":[",
            self.online,
            self.seq,
            self.uptime_ms,
            self.bank,
            self.reset_flags,
            self.boot_count,
            match self.outputs {
                Outputs::Stopped => "stopped",
                Outputs::Sensing => "sensing",
                Outputs::Unverified => "unverified",
            },
        )?;

        let mut first = true;
        for (bit, name) in FAULTS.iter().enumerate() {
            if self.faults & (1 << bit) != 0 {
                out.write_str(if first { "\"" } else { ",\"" })?;
                out.write_str(name)?;
                out.write_str("\"")?;
                first = false;
            }
        }

        let loss = match self.last_loss {
            Loss::None => "null",
            Loss::Link => "\"link\"",
            Loss::Dns => "\"dns\"",
        };
        write!(
            out,
            "],\"wifi\":{{\"rssi\":{},\"attempt\":{},\"losses\":{{\"link\":{},\"dns\":{}}},\"last_loss\":{loss},\"source\":\"{}\"}},\
             \"transport\":{{\"connects\":{},\"last_error\":",
            self.rssi,
            self.wifi_attempt,
            self.link_losses,
            self.dns_losses,
            self.wifi_source.name(),
            self.connects,
        )?;

        match self.last_error {
            Some(code) if code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') => {
                write!(out, "\"{code}\"}}")?
            }
            Some(_) => return Err(fmt::Error),
            None => out.write_str("null}")?,
        }

        if self.role == Role::Recovery {
            out.write_str(",\"role\":\"recovery\"")?;
        }

        match self.clock {
            Some(clock) => write!(
                out,
                ",\"clock\":{{\"syncs\":{},\"unix_ms\":{},\"offset_s\":{},\"uncertainty_ms\":{}}}",
                self.clock_syncs, clock.unix_ms, clock.offset_s, clock.uncertainty_ms
            )?,
            None => write!(out, ",\"clock\":{{\"syncs\":{}}}", self.clock_syncs)?,
        }

        match self.power {
            Some(power) => {
                write!(
                    out,
                    ",\"power\":{{\"adapter\":{},\"battery_mv\":",
                    power.adapter
                )?;
                match power.battery_mv {
                    Some(mv) => write!(out, "{mv}")?,
                    None => out.write_str("null")?,
                }
                write!(out, ",\"age_ms\":{}}}}}", power.age_ms)
            }
            None => out.write_str(",\"power\":null}"),
        }
    }
}

/// How a `time` request ended, for its reply: the jump against the running
/// clock in milliseconds (`None` for a first sync), or the refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeResult {
    Synced { jump_ms: Option<i64> },
    Refused(&'static str),
}

/// The settings as a `settings` reply reports them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SettingsView {
    pub lights: bool,
    pub sound: bool,
    pub volume: u8,
    /// The stored record's generation; 0 for defaults never saved.
    pub generation: u32,
    /// Whether NOR holds a record this firmware can update.
    pub writable: bool,
}

/// How a `settings` request ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsResult {
    /// The current settings; `pending` once a change is queued for the writer.
    Current { view: SettingsView, pending: bool },
    /// Not loaded yet, or a change already pending.
    Busy,
    /// The store cannot be written (corrupt, newer schema, worn or unarmed).
    ReadOnly,
}

/// How a `play` request ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayResult {
    /// Queued for the speaker.
    Started,
    /// The `sound` setting is off.
    Muted,
    /// Settings not loaded yet, or a tune is playing.
    Busy,
}

/// How a `lock` request ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockResult {
    /// The lock as the panel's owner last published it; `pending` once a
    /// change is queued for that owner.
    Current { locked: bool, pending: bool },
    /// A change is already queued.
    Busy,
    /// This firmware runs no panel owner, so there is no lock to set.
    Unserved,
}

/// Which credentials the Wi-Fi join uses this boot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WifiSource {
    #[default]
    Build,
    Stored,
    /// Stored credentials failed to join; the build's took over this boot.
    Fallback,
}

impl WifiSource {
    /// The source as the C join shim and the port's atomics carry it.
    pub const fn code(self) -> u8 {
        match self {
            Self::Build => 0,
            Self::Stored => 1,
            Self::Fallback => 2,
        }
    }

    /// The inverse of `code`; anything else reads as the build's.
    pub const fn from_code(code: u8) -> Self {
        match code {
            1 => Self::Stored,
            2 => Self::Fallback,
            _ => Self::Build,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Stored => "stored",
            Self::Fallback => "fallback",
        }
    }
}

/// The Wi-Fi credentials as a `wifi` reply reports them: never the passphrase.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WifiView {
    /// CRC-32 of the stored SSID; `None` when the build's credentials apply.
    pub stored_ssid_crc: Option<u32>,
    pub generation: u32,
    pub writable: bool,
    pub source: WifiSource,
}

/// How a `wifi` request ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WifiResult {
    /// The current state; `pending` once a change is queued for the writer.
    Current {
        view: WifiView,
        pending: bool,
    },
    Busy,
    ReadOnly,
    /// The SSID or passphrase failed the store's checks.
    Invalid,
}

/// The topic `<namespace><suffix>` into `buf`.
pub fn topic<'a>(buf: &'a mut [u8], namespace: &str, suffix: &str) -> Result<&'a str, Error> {
    let mut out = Buffer::new(buf);
    out.write_str(namespace).map_err(|_| Error::TooLong)?;
    out.write_str(suffix).map_err(|_| Error::TooLong)?;
    let len = out.len();
    core::str::from_utf8(&buf[..len]).map_err(|_| Error::TooLong)
}
