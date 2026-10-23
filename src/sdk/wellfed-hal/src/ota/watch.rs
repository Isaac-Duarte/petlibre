//! FW10 offer check in the normal role (booted from bank 1), after FW11
//! confirmation.
//!
//! Every `CHECK_MS` it opens a short MQTT session of its own, as
//! `<client_id>-ota` beside the transport's session. It publishes
//! `ota/check`, and the server answers `ota/offered` with its manifest or no
//! offer. A manifest that verifies against the build's key and names a
//! different build makes the image give way:
//! - arm the flash-lock hooks if the confirmation did not;
//! - clear the first byte of bank 1's own marker, refused unless bank 0 is
//!   valid (`sdk::bank1_give_way`);
//! - clear the attempt marker and ask the glue to let the watchdog reset.
//!
//! The loader then boots bank 0, whose recovery role downloads and
//! activates the offer.

use core::fmt;

use wellfed_core::named_enum;

use wellfed_core::ota::{self, Status};
use wellfed_core::transport::{self as mqtt, Event, Session, Tick};

use crate::flash::park::Mmio;
use crate::flash_lock::arming::{self, Arming, Hooks, Poll};
use crate::flash_lock::provider::Provider;
use crate::net_status;
use crate::ota::updater::Topic;
use crate::sdk::{self, TcpRecv, TransportConfig};
use crate::trial;

pub const CHECK_MS: u32 = 60_000;
pub const ANSWER_MS: u32 = 5_000;
pub const POLL_MS: u32 = 100;
pub const CONNECT_TIMEOUT_MS: u32 = 3000;

/// Clearing bank 1's own marker; the target uses `sdk::bank1_give_way`.
pub trait GiveWay {
    fn give_way(&mut self) -> bool;
}

pub struct SdkGiveWay<H>(core::marker::PhantomData<H>);

impl<H> SdkGiveWay<H> {
    pub const fn new() -> Self {
        Self(core::marker::PhantomData)
    }
}

impl<H> Default for SdkGiveWay<H> {
    fn default() -> Self {
        Self::new()
    }
}

impl<H: Hooks> GiveWay for SdkGiveWay<H> {
    fn give_way(&mut self) -> bool {
        H::armed() && sdk::bank1_give_way()
    }
}

/// What the last check found; only changes are logged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Found {
    NoOffer,
    /// The offer is this build.
    Same,
    /// Not a manifest this build accepts (signature, bank, size).
    Refused,
}

named_enum! {
    /// Where the offer check stopped (`error stage=`).
    pub enum ErrorStage {
        Key = "key",
        GiveWay = "give_way",
        ClientId = "client_id",
        Topic = "topic",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    Found(Found),
    Offer { version: u32 },
    Arming(arming::Event),
    GaveWay { version: u32 },
    Error { stage: ErrorStage },
}

/// The body of a `PLWCH1` line.
pub fn describe(record: Record, f: &mut impl fmt::Write) -> fmt::Result {
    match record {
        Record::Found(Found::NoOffer) => f.write_str("offer=0"),
        Record::Found(Found::Same) => f.write_str("offer=1 same=1"),
        Record::Found(Found::Refused) => f.write_str("offer=1 refused=1"),
        Record::Offer { version } => write!(f, "offer=1 version={version}"),
        Record::Arming(arming::Event::Armed {
            watchdog,
            installed,
        }) => write!(
            f,
            "armed watchdog={} installed={}",
            u8::from(watchdog),
            u8::from(installed)
        ),
        Record::Arming(arming::Event::Ready { reply, inactive }) => {
            write!(f, "ready reply={} inactive={}", reply.0, u8::from(inactive))
        }
        Record::Arming(arming::Event::Skip { marker }) => write!(f, "skip marker={marker:#x}"),
        Record::Arming(arming::Event::Timeout) => f.write_str("error stage=timeout"),
        Record::Arming(arming::Event::Clock) => f.write_str("error stage=clock"),
        Record::GaveWay { version } => write!(f, "give_way=1 version={version} restart=1"),
        Record::Error { stage } => write!(f, "error stage={stage}"),
    }
}

/// `delay_ms == 0` ends the task; `restart` asks the glue to let the
/// watchdog reset the platform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub delay_ms: u32,
    pub record: Option<Record>,
    pub restart: bool,
}

fn next(delay_ms: u32, record: Option<Record>) -> Step {
    Step {
        delay_ms,
        record,
        restart: false,
    }
}

#[derive(Clone, Copy, Debug)]
struct Open {
    socket: i32,
    session: Session,
    asked_at: Option<u32>,
    opened_at: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Stage {
    #[default]
    Idle,
    Arm,
    GiveWay,
    Done,
}

pub struct Watch {
    config: TransportConfig,
    key: Option<[u8; 32]>,
    stage: Stage,
    open: Option<Open>,
    checked_at: Option<u32>,
    found: Option<Found>,
    version: u32,
    arming: Arming,
    rx: [u8; 2 * mqtt::MAX_PACKET],
    rx_len: usize,
}

impl Watch {
    pub fn new(config: TransportConfig, key: Option<[u8; 32]>) -> Self {
        Self {
            config,
            key,
            stage: Stage::Idle,
            open: None,
            checked_at: None,
            found: None,
            version: 0,
            arming: Arming::default(),
            rx: [0; 2 * mqtt::MAX_PACKET],
            rx_len: 0,
        }
    }

    fn close(&mut self) {
        if let Some(open) = self.open.take() {
            let mut out = [0; 2];
            if let Ok(n) = mqtt::encode_disconnect(&mut out) {
                // Best effort: the socket closes either way.
                let _ = sdk::tcp_send(open.socket, &out[..n]);
            }
            sdk::tcp_close(open.socket);
        }
        self.rx_len = 0;
    }

    fn found(&mut self, found: Found) -> Step {
        self.close();
        let changed = self.found != Some(found);
        self.found = Some(found);
        next(POLL_MS, changed.then_some(Record::Found(found)))
    }

    fn end(&mut self, record: Record) -> Step {
        self.close();
        self.stage = Stage::Done;
        next(0, Some(record))
    }

    /// One task iteration in task context; `now` is uptime in ms.
    ///
    /// # Safety
    /// `provider` is valid for the program's lifetime, stays at a fixed
    /// address and is not otherwise borrowed.
    pub unsafe fn step<M: Mmio, H: Hooks>(
        &mut self,
        provider: *mut Provider,
        bank: &mut impl GiveWay,
        now: u32,
    ) -> Step {
        match self.stage {
            Stage::Idle => {
                if self.key.is_none() {
                    return self.end(Record::Error {
                        stage: ErrorStage::Key,
                    });
                }

                if self.open.is_some() {
                    return self.poll(now);
                }

                if self
                    .checked_at
                    .is_some_and(|at| now.wrapping_sub(at) < CHECK_MS)
                {
                    return next(CHECK_MS / 10, None);
                }

                self.checked_at = Some(now);
                self.connect(now)
            }

            Stage::Arm => {
                if H::armed() {
                    self.stage = Stage::GiveWay;
                    return next(POLL_MS, None);
                }

                // SAFETY: the caller's contract.
                match unsafe { self.arming.step::<M, H>(provider) } {
                    Poll::Pending(event) => next(arming::POLL_MS, event.map(Record::Arming)),
                    Poll::Armed(event) => {
                        self.stage = Stage::GiveWay;
                        next(arming::POLL_MS, Some(Record::Arming(event)))
                    }
                    Poll::Failed(event) => self.end(Record::Arming(event)),
                }
            }

            Stage::GiveWay => {
                let ok = bank.give_way();
                // The recovery boot must not find the attempt marker set.
                arming::clear_attempt();
                if !ok {
                    return self.end(Record::Error {
                        stage: ErrorStage::GiveWay,
                    });
                }

                self.stage = Stage::Done;
                Step {
                    delay_ms: 0,
                    record: Some(Record::GaveWay {
                        version: self.version,
                    }),
                    restart: true,
                }
            }

            Stage::Done => next(0, None),
        }
    }

    fn connect(&mut self, now: u32) -> Step {
        if !net_status::read().dns_ok {
            return next(POLL_MS, None);
        }

        let mut id = [0u8; 80];
        let base = self.config.client_id.as_bytes();
        let Some(id) = id.get_mut(..base.len() + 4).map(|out| {
            out[..base.len()].copy_from_slice(base);
            out[base.len()..].copy_from_slice(b"-ota");
            &*out
        }) else {
            return self.end(Record::Error {
                stage: ErrorStage::ClientId,
            });
        };

        let Ok(client_id) = core::str::from_utf8(id) else {
            return self.end(Record::Error {
                stage: ErrorStage::ClientId,
            });
        };

        let Some(socket) =
            sdk::tcp_connect(self.config.broker, self.config.port, CONNECT_TIMEOUT_MS)
        else {
            return next(POLL_MS, None);
        };

        let mut out = [0; mqtt::MAX_PACKET];
        let connect = mqtt::Connect {
            client_id,
            username: self.config.username,
            password: self.config.password,
            will: None,
        };

        if !mqtt::encode_connect(&mut out, &connect).is_ok_and(|n| sdk::tcp_send(socket, &out[..n]))
        {
            sdk::tcp_close(socket);
            return next(POLL_MS, None);
        }

        self.open = Some(Open {
            socket,
            session: Session::connecting(now),
            asked_at: None,
            opened_at: now,
        });
        self.rx_len = 0;
        next(POLL_MS, None)
    }

    fn poll(&mut self, now: u32) -> Step {
        let Some(open) = self.open else {
            return next(POLL_MS, None);
        };

        if now.wrapping_sub(open.opened_at) >= ANSWER_MS + CONNECT_TIMEOUT_MS {
            self.close();
            return next(POLL_MS, None);
        }

        match sdk::tcp_recv(open.socket, &mut self.rx[self.rx_len..], POLL_MS) {
            TcpRecv::Data(n) => self.rx_len += n,
            TcpRecv::Timeout => {}
            TcpRecv::Closed => {
                self.close();
                return next(POLL_MS, None);
            }
        }

        loop {
            let used = match mqtt::decode(&self.rx[..self.rx_len]) {
                Ok(None) => break,
                Ok(Some((_, used))) => used,
                Err(_) => {
                    self.close();
                    return next(POLL_MS, None);
                }
            };
            let mut frame = [0u8; mqtt::MAX_PACKET];
            frame[..used].copy_from_slice(&self.rx[..used]);
            self.rx.copy_within(used..self.rx_len, 0);
            self.rx_len -= used;
            if let Some(step) = self.handle(now, &frame[..used]) {
                return step;
            }
        }

        if let Some(open) = self.open.as_mut()
            && let Tick::Closed(_) = open.session.tick(now)
        {
            self.close();
        }

        next(POLL_MS, None)
    }

    fn handle(&mut self, now: u32, frame: &[u8]) -> Option<Step> {
        let Ok(Some((packet, _))) = mqtt::decode(frame) else {
            self.close();
            return Some(next(POLL_MS, None));
        };
        let (Some(offered), Some(check)) = (
            Topic::new(self.config.namespace, "ota/offered"),
            Topic::new(self.config.namespace, "ota/check"),
        ) else {
            return Some(self.end(Record::Error {
                stage: ErrorStage::Topic,
            }));
        };

        let open = self.open.as_mut()?;
        // `ota/offered` takes the request topic's place in the session, so the
        // server's answer arrives as `Event::Request`.
        match open.session.on_packet(now, packet, offered.as_str()) {
            Event::Connected => {
                let mut out = [0; 128];
                let ok = open
                    .session
                    .subscribe(now, &mut out, offered.as_str())
                    .is_ok_and(|n| sdk::tcp_send(open.socket, &out[..n]));
                if !ok {
                    self.close();
                }
                None
            }

            Event::Ready => {
                let mut out = [0; 128];
                let get = ota::encode_get(ota::Part::Manifest, 0, 0);
                let ok = mqtt::encode_publish(&mut out, check.as_str(), &get, false)
                    .is_ok_and(|n| sdk::tcp_send(open.socket, &out[..n]));
                open.asked_at = Some(now);
                if !ok {
                    self.close();
                }
                None
            }

            Event::Request(payload) => Some(self.answer(payload)),
            Event::Closed(_) => {
                self.close();
                Some(next(POLL_MS, None))
            }
            _ => None,
        }
    }

    fn answer(&mut self, payload: &[u8]) -> Step {
        let Some(data) = ota::decode_data(payload) else {
            return self.found(Found::Refused);
        };
        if data.status != Status::Ok || data.part != ota::Part::Manifest {
            return self.found(Found::NoOffer);
        }
        self.verify(data.bytes)
    }

    /// Kept out of line: Ed25519 needs about 12 KiB of stack.
    #[inline(never)]
    fn verify(&mut self, bytes: &[u8]) -> Step {
        let Some(key) = self.key else {
            return self.found(Found::Refused);
        };
        let Ok(manifest) = ota::parse_manifest(bytes, &key, 0) else {
            return self.found(Found::Refused);
        };
        if manifest.build_id == trial::identity() {
            return self.found(Found::Same);
        }

        self.close();
        self.version = manifest.version;
        self.stage = Stage::Arm;
        next(
            POLL_MS,
            Some(Record::Offer {
                version: manifest.version,
            }),
        )
    }
}
