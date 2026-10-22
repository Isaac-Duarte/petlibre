//! FW08 transport driver on KM4: one outbound MQTT connection to the local
//! server's broker, carrying health and the read-only request allowlist.
//!
//! Network-facing: this file reaches only the core transport, the socket and
//! clock wrappers, the Wi-Fi status reader and the health snapshot (a
//! repository check enforces the list). Nothing here can touch feeding,
//! outputs, settings, flash or updates.

use core::fmt::Write;

use wellfed_core::request_auth;
use wellfed_core::transport::{
    self as mqtt, Buffer, Event, Fault, Op, Packet, Session, State, Tick,
};

use crate::clock;
use crate::health::{self, Boot};
use crate::net_status;
use crate::sdk::{self, TcpRecv, TransportConfig};
use crate::settings;

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
    Request,
}

/// Request outcome for the log; the request ID is not logged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestKind {
    Health,
    Ping,
    Time,
    Settings,
    /// Refused: a bad signature, or a state change that came unsigned.
    Unauthenticated,
    Unsupported,
    Malformed,
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
    Request {
        kind: RequestKind,
        ok: bool,
    },
    Closed {
        reason: &'static str,
    },
}

fn fault_name(fault: Fault) -> &'static str {
    match fault {
        Fault::Refused(_) => "refused",
        Fault::SubscribeRefused(_) => "subscribe_refused",
        Fault::Protocol(mqtt::Error::TooLong) => "protocol_too_long",
        Fault::Protocol(mqtt::Error::Malformed) => "protocol_malformed",
        Fault::Protocol(mqtt::Error::Unexpected) => "protocol_unexpected",
        Fault::OutOfOrder => "out_of_order",
        Fault::ReplyTimeout => "reply_timeout",
        Fault::Silence => "silence",
        Fault::ForeignTopic => "foreign_topic",
    }
}

struct Open {
    socket: i32,
    session: Session,
    since: u32,
    generation: u32,
    /// Verifies signed requests; `None` without a request key.
    auth: Option<request_auth::Session>,
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
            self.close("wifi", log);
            return IDLE_MS;
        }

        let socket = self.open.as_ref().map_or(-1, |o| o.socket);
        match sdk::tcp_recv(socket, &mut self.rx[self.rx_len..], POLL_MS) {
            TcpRecv::Data(n) => self.rx_len += n,
            TcpRecv::Timeout => {}
            TcpRecv::Closed => {
                self.close("closed", log);
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
                    self.close("send", log);
                    return IDLE_MS;
                }
            }
            Tick::Closed(fault) => {
                self.close(fault_name(fault), log);
                return IDLE_MS;
            }
        }

        if self.ready() {
            let wifi = net_status::read();
            let losses = wifi.link_losses.wrapping_add(wifi.dns_losses);
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
        if !wifi.dns_ok || waiting {
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

        // A fresh nonce for every session, so no earlier request replays.
        let auth = self.config.request_key.map(|key| {
            let mut entropy = [0u8; 20];
            let words = sdk::entropy();
            for (i, word) in words.iter().chain(&[n]).enumerate() {
                entropy[4 * i..4 * i + 4].copy_from_slice(&word.to_le_bytes());
            }
            request_auth::Session::new(key, &entropy)
        });
        self.counters.nonce = auth.as_ref().map(request_auth::Session::nonce_hex);

        let sent = self.send_connect(socket);
        let now = sdk::uptime_ms();
        self.open = Some(Open {
            socket,
            session: Session::connecting(now),
            since: now,
            generation: wifi.generation,
            auth,
        });

        self.rx_len = 0;
        if !sent {
            self.close("send", log);
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
        if health::snapshot(self.boot, link, false)
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
                        self.close("protocol_too_long", log);
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
                    self.close(fault_name(fault), log);
                    return false;
                }
            };

            // Copy the packet out so handling it may send and reuse `rx`.
            frame[..used].copy_from_slice(&self.rx[..used]);
            self.rx.copy_within(used..self.rx_len, 0);
            self.rx_len -= used;
            let Ok(Some((packet, _))) = mqtt::decode(&frame[..used]) else {
                self.close("protocol_malformed", log);
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
                    self.close("send", log);
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
            Event::Request(payload) => self.answer(payload, log),

            // This driver never subscribes a data topic (only the recovery
            // role's updater does), so these cannot arrive here.
            Event::DataReady | Event::Data(_) => {
                self.close("out_of_order", log);
                false
            }
            Event::Closed(fault) => {
                self.close(fault_name(fault), log);
                false
            }
        }
    }

    fn answer(&mut self, payload: &[u8], log: &mut impl FnMut(Record)) -> bool {
        // A signed request is parsed without its MAC; the MAC, nonce and
        // sequence are then checked together (request_auth.rs).
        let signed = request_auth::split(payload);
        let mut unsigned = [0; mqtt::MAX_REQUEST];
        let text = match signed {
            Some(signed) if signed.prefix.len() < unsigned.len() => {
                let len = signed.prefix.len();
                unsigned[..len].copy_from_slice(signed.prefix);
                unsigned[len] = b'}';
                &unsigned[..=len]
            }
            Some(_) => &[][..],
            None => payload,
        };

        let request = match mqtt::parse_request(text) {
            Ok(request) => request,
            Err(_) => {
                // Nothing to correlate a reply with; refuse and keep going.
                log(Record::Request {
                    kind: RequestKind::Malformed,
                    ok: false,
                });
                return true;
            }
        };

        let authentic = match (
            signed,
            request.auth,
            self.open.as_mut().and_then(|o| o.auth.as_mut()),
        ) {
            (Some(signed), Some((nonce, seq)), Some(auth)) => auth.verify(&signed, nonce, seq),
            _ => false,
        };
        // A bad signature is refused whatever it asks; a state change needs a
        // good one, and an image without a request key refuses every change.
        let changes =
            request.op == Ok(Op::Time) || request.settings.is_some_and(|fields| fields.changes());
        let refused = (signed.is_some() || changes) && !authentic;

        let kind = match request.op {
            _ if refused => RequestKind::Unauthenticated,
            Ok(Op::Health) => RequestKind::Health,
            Ok(Op::Ping) => RequestKind::Ping,
            Ok(Op::Time) => RequestKind::Time,
            Ok(Op::Settings) => RequestKind::Settings,
            Err(_) => RequestKind::Unsupported,
        };

        let mut body = [0; mqtt::MAX_REQUEST];
        let mut writer = Buffer::new(&mut body);
        // FW21: the clock is the one thing a request may change (see clock.rs).
        let (written, ok) = match request.time {
            _ if refused => (mqtt::write_auth_refusal(&mut writer, &request), false),
            // FW25: queue a change for the settings writer, or read.
            _ if request.settings.is_some() => {
                let result = settings::request(request.settings.unwrap_or_default());
                (
                    mqtt::write_settings_reply(&mut writer, &request, result),
                    matches!(result, mqtt::SettingsResult::Current { .. }),
                )
            }
            Some(fields) => {
                let result = clock::sync(fields);
                (
                    mqtt::write_time_reply(&mut writer, &request, result),
                    matches!(result, mqtt::TimeResult::Synced { .. }),
                )
            }
            None => (
                mqtt::write_reply(&mut writer, &request, sdk::uptime_ms()),
                request.op.is_ok(),
            ),
        };
        if written.is_err() {
            return true;
        }

        let len = writer.len();
        if !self.send_publish("reply", &body[..len]) {
            self.close("send", log);
            return false;
        }

        log(Record::Request { kind, ok });
        if kind == RequestKind::Health {
            return self.publish(Trigger::Request, log);
        }
        true
    }

    fn publish(&mut self, trigger: Trigger, log: &mut impl FnMut(Record)) -> bool {
        self.counters.seq = self.counters.seq.wrapping_add(1);
        let mut body = [0; mqtt::MAX_HEALTH];
        let mut writer = Buffer::new(&mut body);
        let snapshot = health::snapshot(self.boot, self.counters, true);
        if snapshot.write(&mut writer).is_err() {
            self.close("health_format", log);
            return false;
        }

        let len = writer.len();
        if !self.send_publish("health", &body[..len]) {
            self.close("send", log);
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
    fn close(&mut self, reason: &'static str, log: &mut impl FnMut(Record)) {
        if let Some(open) = self.open.take() {
            sdk::tcp_close(open.socket);
        }
        self.rx_len = 0;
        self.counters.last_error = Some(reason);
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
                Trigger::Request => "request",
            };
            write!(out, "publish seq={seq} bytes={bytes} trigger={trigger}")
        }

        Record::Request { kind, ok } => {
            let kind = match kind {
                RequestKind::Health => "health",
                RequestKind::Ping => "ping",
                RequestKind::Time => "time",
                RequestKind::Settings => "settings",
                RequestKind::Unauthenticated => "unauthenticated",
                RequestKind::Unsupported => "unsupported",
                RequestKind::Malformed => "malformed",
            };
            write!(out, "request op={kind} ok={}", u8::from(ok))
        }
        Record::Closed { reason } => write!(out, "closed reason={reason}"),
    }
}
