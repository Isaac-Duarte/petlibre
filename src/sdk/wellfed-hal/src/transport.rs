//! Transport driver on KM4: one outbound MQTT connection to the local
//! server's broker, carrying health only. Every request goes over
//! the device API, so a request published here is ignored and never
//! answered; the bank-0 recovery role still reports health this way while
//! the broker serves the offers until it is retired.
//!
//! Network-facing: this file reaches only the core transport, the socket
//! and clock wrappers, the Wi-Fi status reader and the health snapshot (a
//! repository check enforces the list). Nothing here can touch feeding,
//! outputs, settings, flash or updates.

use core::fmt::Write;

use wellfed_core::transport::{
    self as mqtt, Buffer, Event, Fault, Packet, Role, Session, State, Tick,
};

use crate::health::{self, Boot};
use crate::net_status;
use crate::sdk::{self, TcpRecv, TransportConfig};

pub const CONNECT_TIMEOUT_MS: u32 = 3000;
/// Each receive waits at most this long, so ticks and publishes stay timely.
pub const POLL_MS: u32 = 100;
/// Delay between steps while there is no connection to try.
pub const IDLE_MS: u32 = 500;
pub const BACKOFF_MIN_MS: u32 = 1000;
pub const BACKOFF_MAX_MS: u32 = 30_000;

/// Why a step published health.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    Connect,
    Period,
    Change,
}

/// What happened, for the UART log. No record carries a credential.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    Connect {
        n: u32,
        ok: bool,
        ms: u32,
    },
    Ready {
        ms: u32,
    },
    Publish {
        seq: u32,
        bytes: usize,
        trigger: Trigger,
    },
    /// A request arrived on the request topic and was not answered.
    Ignored,
    Closed {
        reason: CloseReason,
    },
}

/// Why a session closed (`closed reason=`), also the health message's
/// `last_error`. Written out rather than with `wellfed_core::named_enum!`:
/// the isolation check lets this module name only its allowed paths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseReason {
    Wifi,
    Closed,
    Send,
    Refused,
    SubscribeRefused,
    ProtocolTooLong,
    ProtocolMalformed,
    ProtocolUnexpected,
    OutOfOrder,
    ReplyTimeout,
    Silence,
    ForeignTopic,
    HealthFormat,
}

impl CloseReason {
    /// Every variant, in declaration order.
    pub const ALL: &'static [Self] = &[
        Self::Wifi,
        Self::Closed,
        Self::Send,
        Self::Refused,
        Self::SubscribeRefused,
        Self::ProtocolTooLong,
        Self::ProtocolMalformed,
        Self::ProtocolUnexpected,
        Self::OutOfOrder,
        Self::ReplyTimeout,
        Self::Silence,
        Self::ForeignTopic,
        Self::HealthFormat,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Wifi => "wifi",
            Self::Closed => "closed",
            Self::Send => "send",
            Self::Refused => "refused",
            Self::SubscribeRefused => "subscribe_refused",
            Self::ProtocolTooLong => "protocol_too_long",
            Self::ProtocolMalformed => "protocol_malformed",
            Self::ProtocolUnexpected => "protocol_unexpected",
            Self::OutOfOrder => "out_of_order",
            Self::ReplyTimeout => "reply_timeout",
            Self::Silence => "silence",
            Self::ForeignTopic => "foreign_topic",
            Self::HealthFormat => "health_format",
        }
    }
}

impl From<Fault> for CloseReason {
    fn from(fault: Fault) -> Self {
        match fault {
            Fault::Refused(_) => Self::Refused,
            Fault::SubscribeRefused(_) => Self::SubscribeRefused,
            Fault::Protocol(mqtt::Error::TooLong) => Self::ProtocolTooLong,
            Fault::Protocol(mqtt::Error::Malformed) => Self::ProtocolMalformed,
            Fault::Protocol(mqtt::Error::Unexpected) => Self::ProtocolUnexpected,
            Fault::OutOfOrder => Self::OutOfOrder,
            Fault::ReplyTimeout => Self::ReplyTimeout,
            Fault::Silence => Self::Silence,
            Fault::ForeignTopic => Self::ForeignTopic,
        }
    }
}

struct Open {
    socket: i32,
    session: Session,
    since: u32,
    generation: u32,
}

/// The connection and its counters; one per boot.
pub struct Link {
    config: TransportConfig,
    boot: Boot,
    open: Option<Open>,
    rx: [u8; 2 * mqtt::MAX_PACKET],
    rx_len: usize,
    counters: health::Link,
    /// The wait the next failure starts.
    backoff_ms: u32,
    /// The current wait and when it started; `None` before the first failure.
    retry: Option<(u32, u32)>,
    published_at: u32,
    losses_seen: u32,
}

/// `<namespace><suffix>` on the stack.
struct Topic {
    buf: [u8; 96],
    len: usize,
}

impl Topic {
    fn new(namespace: &str, suffix: &str) -> Option<Self> {
        let mut buf = [0; 96];
        let len = mqtt::topic(&mut buf, namespace, suffix).ok()?.len();
        Some(Self { buf, len })
    }

    fn as_str(&self) -> &str {
        // Built from two `&str`s by `mqtt::topic`, so always UTF-8.
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

impl Link {
    pub fn new(config: TransportConfig, boot: Boot) -> Self {
        Self {
            config,
            boot,
            open: None,
            rx: [0; 2 * mqtt::MAX_PACKET],
            rx_len: 0,
            counters: health::Link::default(),
            backoff_ms: BACKOFF_MIN_MS,
            retry: None,
            published_at: 0,
            losses_seen: 0,
        }
    }

    pub fn ready(&self) -> bool {
        self.open
            .as_ref()
            .is_some_and(|o| o.session.state() == State::Ready)
    }

    /// One task iteration: connect, or receive and serve. Returns the delay
    /// before the next step.
    pub fn step(&mut self, log: &mut impl FnMut(Record)) -> u32 {
        if self.open.is_none() {
            return self.try_connect(log);
        }

        let wifi = net_status::read();
        // A socket opened before the latest join or loss rides a link that
        // has since gone away.
        if self
            .open
            .as_ref()
            .is_some_and(|o| o.generation != wifi.generation)
        {
            self.close(CloseReason::Wifi, log);
            return IDLE_MS;
        }

        let socket = self.open.as_ref().map_or(-1, |o| o.socket);
        match sdk::tcp_recv(socket, &mut self.rx[self.rx_len..], POLL_MS) {
            TcpRecv::Data(n) => self.rx_len += n,
            TcpRecv::Timeout => {}
            TcpRecv::Closed => {
                self.close(CloseReason::Closed, log);
                return IDLE_MS;
            }
        }

        if !self.serve(log) {
            return IDLE_MS;
        }

        let now = sdk::uptime_ms();
        let tick = match self.open.as_mut() {
            Some(open) => open.session.tick(now),
            None => return IDLE_MS,
        };

        match tick {
            Tick::Idle => {}
            Tick::Ping => {
                let mut out = [0; 2];
                let ok =
                    mqtt::encode_pingreq(&mut out).is_ok_and(|n| sdk::tcp_send(socket, &out[..n]));
                if !ok {
                    self.close(CloseReason::Send, log);
                    return IDLE_MS;
                }
            }
            Tick::Closed(fault) => {
                self.close(fault.into(), log);
                return IDLE_MS;
            }
        }

        if self.ready() {
            let wifi = net_status::read();
            let losses = wifi.link_losses.wrapping_add(wifi.check_losses);
            let trigger = if losses != self.losses_seen {
                Some(Trigger::Change)
            } else if now.wrapping_sub(self.published_at) >= mqtt::HEALTH_PERIOD_MS {
                Some(Trigger::Period)
            } else {
                None
            };
            if let Some(trigger) = trigger
                && !self.publish(trigger, log)
            {
                return IDLE_MS;
            }
        }

        1
    }

    fn try_connect(&mut self, log: &mut impl FnMut(Record)) -> u32 {
        let wifi = net_status::read();
        let now = sdk::uptime_ms();
        let waiting = self
            .retry
            .is_some_and(|(from, wait)| now.wrapping_sub(from) < wait);
        if !wifi.live || waiting {
            return IDLE_MS;
        }

        self.counters.connects = self.counters.connects.wrapping_add(1);
        let n = self.counters.connects;
        let Some(socket) =
            sdk::tcp_connect(self.config.broker, self.config.port, CONNECT_TIMEOUT_MS)
        else {
            log(Record::Connect {
                n,
                ok: false,
                ms: sdk::uptime_ms().wrapping_sub(now),
            });
            self.counters.last_error = Some("connect");
            self.back_off();
            return IDLE_MS;
        };

        log(Record::Connect {
            n,
            ok: true,
            ms: sdk::uptime_ms().wrapping_sub(now),
        });

        let sent = self.send_connect(socket);
        let now = sdk::uptime_ms();
        self.open = Some(Open {
            socket,
            session: Session::connecting(now),
            since: now,
            generation: wifi.generation,
        });

        self.rx_len = 0;
        if !sent {
            self.close(CloseReason::Send, log);
        }
        1
    }

    fn send_connect(&mut self, socket: i32) -> bool {
        let Some(health) = Topic::new(self.config.namespace, "health") else {
            return false;
        };

        let mut will = [0; mqtt::MAX_HEALTH];
        let mut will_out = Buffer::new(&mut will);
        let link = self.counters;
        if health::snapshot(self.boot, link, false, Role::Normal)
            .write(&mut will_out)
            .is_err()
        {
            return false;
        }

        let will_len = will_out.len();
        let mut out = [0; mqtt::MAX_PACKET];
        let connect = mqtt::Connect {
            client_id: self.config.client_id,
            username: self.config.username,
            password: self.config.password,
            will: Some(mqtt::Will {
                topic: health.as_str(),
                payload: &will[..will_len],
            }),
        };
        mqtt::encode_connect(&mut out, &connect).is_ok_and(|n| sdk::tcp_send(socket, &out[..n]))
    }

    /// Decode and handle every whole packet received; false when it closed.
    fn serve(&mut self, log: &mut impl FnMut(Record)) -> bool {
        loop {
            let mut frame = [0; mqtt::MAX_PACKET];
            let used = match mqtt::decode(&self.rx[..self.rx_len]) {
                Ok(None) => {
                    if self.rx_len == self.rx.len() {
                        self.close(CloseReason::ProtocolTooLong, log);
                        return false;
                    }
                    return true;
                }
                Ok(Some((_, used))) => used,
                Err(error) => {
                    let fault = match self.open.as_mut() {
                        Some(open) => open.session.on_error(error),
                        None => return false,
                    };
                    self.close(fault.into(), log);
                    return false;
                }
            };

            // Copy the packet out so handling it may send and reuse `rx`.
            frame[..used].copy_from_slice(&self.rx[..used]);
            self.rx.copy_within(used..self.rx_len, 0);
            self.rx_len -= used;
            let Ok(Some((packet, _))) = mqtt::decode(&frame[..used]) else {
                self.close(CloseReason::ProtocolMalformed, log);
                return false;
            };
            if !self.handle(packet, log) {
                return false;
            }
        }
    }

    fn handle(&mut self, packet: Packet, log: &mut impl FnMut(Record)) -> bool {
        let Some(request) = Topic::new(self.config.namespace, "request") else {
            return false;
        };
        let now = sdk::uptime_ms();
        let Some(open) = self.open.as_mut() else {
            return false;
        };

        let socket = open.socket;
        match open.session.on_packet(now, packet, request.as_str()) {
            Event::Connected => {
                let mut out = [0; 128];
                let ok = open
                    .session
                    .subscribe(now, &mut out, request.as_str())
                    .is_ok_and(|n| sdk::tcp_send(socket, &out[..n]));
                if !ok {
                    self.close(CloseReason::Send, log);
                    return false;
                }
                true
            }

            Event::Ready => {
                log(Record::Ready {
                    ms: now.wrapping_sub(open.since),
                });
                self.backoff_ms = BACKOFF_MIN_MS;
                self.publish(Trigger::Connect, log)
            }

            Event::Pong => true,
            Event::Request(_) => {
                log(Record::Ignored);
                true
            }

            // This driver never subscribes a data topic (only the recovery
            // role's updater does), so these cannot arrive here.
            Event::DataReady | Event::Data(_) => {
                self.close(CloseReason::OutOfOrder, log);
                false
            }
            Event::Closed(fault) => {
                self.close(fault.into(), log);
                false
            }
        }
    }

    fn publish(&mut self, trigger: Trigger, log: &mut impl FnMut(Record)) -> bool {
        self.counters.seq = self.counters.seq.wrapping_add(1);
        let mut body = [0; mqtt::MAX_HEALTH];
        let mut writer = Buffer::new(&mut body);
        let snapshot = health::snapshot(self.boot, self.counters, true, Role::Normal);
        if snapshot.write(&mut writer).is_err() {
            self.close(CloseReason::HealthFormat, log);
            return false;
        }

        let len = writer.len();
        if !self.send_publish("health", &body[..len]) {
            self.close(CloseReason::Send, log);
            return false;
        }

        self.published_at = sdk::uptime_ms();
        self.losses_seen = snapshot.link_losses.wrapping_add(snapshot.dns_losses);
        log(Record::Publish {
            seq: self.counters.seq,
            bytes: len,
            trigger,
        });
        true
    }

    fn send_publish(&mut self, suffix: &str, payload: &[u8]) -> bool {
        let Some(topic) = Topic::new(self.config.namespace, suffix) else {
            return false;
        };
        let Some(open) = self.open.as_mut() else {
            return false;
        };

        let mut out = [0; mqtt::MAX_PACKET];
        let ok = mqtt::encode_publish(&mut out, topic.as_str(), payload, false)
            .is_ok_and(|n| sdk::tcp_send(open.socket, &out[..n]));
        if ok {
            open.session.sent(sdk::uptime_ms());
        }
        ok
    }

    /// Every close is a failure, so no DISCONNECT is sent: the broker then
    /// publishes the will and the server sees the feeder offline.
    fn close(&mut self, reason: CloseReason, log: &mut impl FnMut(Record)) {
        if let Some(open) = self.open.take() {
            sdk::tcp_close(open.socket);
        }
        self.rx_len = 0;
        self.counters.last_error = Some(reason.as_str());
        log(Record::Closed { reason });
        self.back_off();
    }

    /// Wait 1 s after the first failure, doubling to 30 s; Ready resets it.
    fn back_off(&mut self) {
        self.retry = Some((sdk::uptime_ms(), self.backoff_ms));
        self.backoff_ms = self.backoff_ms.saturating_mul(2).min(BACKOFF_MAX_MS);
    }

    /// The transport's own counters, as the next health message reports them.
    pub fn counters(&self) -> health::Link {
        self.counters
    }
}

/// Format a record into `out` (the `PLMQT1` line body, without prefix).
pub fn describe(record: Record, out: &mut impl Write) -> core::fmt::Result {
    match record {
        Record::Connect { n, ok, ms } => write!(out, "connect n={n} ok={} ms={ms}", u8::from(ok)),
        Record::Ready { ms } => write!(out, "ready ms={ms}"),
        Record::Publish {
            seq,
            bytes,
            trigger,
        } => {
            let trigger = match trigger {
                Trigger::Connect => "connect",
                Trigger::Period => "period",
                Trigger::Change => "change",
            };
            write!(out, "publish seq={seq} bytes={bytes} trigger={trigger}")
        }

        Record::Ignored => out.write_str("request ignored=1"),
        Record::Closed { reason } => write!(out, "closed reason={}", reason.as_str()),
    }
}
