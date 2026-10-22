//! FW08 local health transport: an MQTT 3.1.1 client subset, its session
//! state machine, the request parser and the health/reply encoders.
//!
//! Sans-IO: the caller owns the socket, feeds received bytes in and writes the
//! returned bytes out. Nothing here allocates. This module reaches no other
//! part of the crate (a repository check enforces it), so nothing reachable
//! from the network can touch feeding, outputs, settings, flash or updates.
//! See docs/firmware/local-transport.md.

use core::fmt::{self, Write};

/// Largest packet either side may send; bigger input closes the session.
pub const MAX_PACKET: usize = 640;
/// Health payload limit (design: 512 bytes).
pub const MAX_HEALTH: usize = 512;
/// Request payload limit (design: 256 bytes).
pub const MAX_REQUEST: usize = 256;
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
        let mut digit = (value % 128) as u8;
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
    /// A request payload on the request topic, not yet parsed.
    Request(&'a [u8]),
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
                if topic != request_topic {
                    return Event::Closed(self.close(Fault::ForeignTopic));
                }
                if payload.len() > MAX_REQUEST {
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

/// A v1 diagnostic operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Health,
    Ping,
}

/// A request: `{"id":"<uuid>","op":"<name>"}`, nothing else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request<'a> {
    pub id: &'a str,
    /// `Err(name)` for an operation outside the allowlist.
    pub op: Result<Op, &'a str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestError {
    TooLong,
    Malformed,
    BadId,
}

fn is_uuid(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
}

/// A JSON string with no inner quote, surrounding whitespace removed.
fn unquote(s: &str) -> Option<&str> {
    let s = s.trim().strip_prefix('"')?.strip_suffix('"')?;
    (!s.contains('"')).then_some(s)
}

/// Parse a flat JSON object of exactly two string fields, `id` and `op`, in
/// either order. Escapes, nesting and extra fields are refused.
pub fn parse_request(payload: &[u8]) -> Result<Request<'_>, RequestError> {
    if payload.len() > MAX_REQUEST {
        return Err(RequestError::TooLong);
    }
    let text = core::str::from_utf8(payload).map_err(|_| RequestError::Malformed)?;
    let inner = text
        .trim()
        .strip_prefix('{')
        .and_then(|t| t.strip_suffix('}'))
        .ok_or(RequestError::Malformed)?;
    if inner.contains('\\') {
        return Err(RequestError::Malformed);
    }
    let (mut id, mut op) = (None, None);
    for pair in inner.split(',') {
        let (key, value) = pair.split_once(':').ok_or(RequestError::Malformed)?;
        let (key, value) = (
            unquote(key).ok_or(RequestError::Malformed)?,
            unquote(value).ok_or(RequestError::Malformed)?,
        );
        let slot = match key {
            "id" => &mut id,
            "op" => &mut op,
            _ => return Err(RequestError::Malformed),
        };
        if slot.replace(value).is_some() {
            return Err(RequestError::Malformed);
        }
    }
    let (id, op) = (
        id.ok_or(RequestError::Malformed)?,
        op.ok_or(RequestError::Malformed)?,
    );
    if !is_uuid(id) {
        return Err(RequestError::BadId);
    }
    let op = match op {
        "health" => Ok(Op::Health),
        "ping" => Ok(Op::Ping),
        other => Err(other),
    };
    Ok(Request { id, op })
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
    /// Fault code of the last closed session, if any.
    pub last_error: Option<&'static str>,
}

/// Fault names by bit, in health order.
pub const FAULTS: [&str; 3] = ["watchdog", "flash_hook", "stopped_readback"];

impl Health {
    pub fn write(&self, out: &mut impl Write) -> fmt::Result {
        let build = self.build;
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
            "],\"wifi\":{{\"rssi\":{},\"attempt\":{},\"losses\":{{\"link\":{},\"dns\":{}}},\"last_loss\":{loss}}},\
             \"transport\":{{\"connects\":{},\"last_error\":",
            self.rssi, self.wifi_attempt, self.link_losses, self.dns_losses, self.connects,
        )?;
        match self.last_error {
            Some(code) if code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') => {
                write!(out, "\"{code}\"}}}}")
            }
            Some(_) => Err(fmt::Error),
            None => out.write_str("null}}"),
        }
    }
}

/// The reply to a request; `uptime_ms` is included for `ping`.
pub fn write_reply(out: &mut impl Write, request: &Request, uptime_ms: u32) -> fmt::Result {
    match request.op {
        Ok(op) => {
            let name = match op {
                Op::Health => "health",
                Op::Ping => "ping",
            };
            write!(
                out,
                "{{\"id\":\"{}\",\"op\":\"{name}\",\"ok\":true",
                request.id
            )?;
            if op == Op::Ping {
                write!(out, ",\"uptime_ms\":{uptime_ms}")?;
            }
            out.write_str("}")
        }
        // The unsupported name is not echoed: it came from the network.
        Err(_) => write!(
            out,
            "{{\"id\":\"{}\",\"ok\":false,\"error\":\"unsupported\"}}",
            request.id
        ),
    }
}

/// The topic `<namespace><suffix>` into `buf`.
pub fn topic<'a>(buf: &'a mut [u8], namespace: &str, suffix: &str) -> Result<&'a str, Error> {
    let mut out = Buffer::new(buf);
    out.write_str(namespace).map_err(|_| Error::TooLong)?;
    out.write_str(suffix).map_err(|_| Error::TooLong)?;
    let len = out.len();
    core::str::from_utf8(&buf[..len]).map_err(|_| Error::TooLong)
}
