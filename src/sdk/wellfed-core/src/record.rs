//! The firmware's UART records, as the host tools read them back.
//!
//! Every subsystem reports on the UART as `PL<tag><n> <first> key=value …`
//! lines (`PLOTA1 progress sectors=48/170`). SDK output shares the port, so
//! a record can start mid-line or follow another one on the same line. The
//! firmware writes the bodies with `core::fmt` (each module's `describe`);
//! the bench reads them with [`Record::event`], which decodes the words
//! through the same tables the firmware prints from (the install's
//! [`StopStage`] and [`RefusedReason`], the owner's numbers). The SDK's and
//! the feeder's record tests run their `describe` functions through it, so
//! a renamed field or word fails the host tests rather than a half-hour
//! bench run.

use crate::feed::controller::FeedReason;
use crate::feed::owner::OwnerState;
use crate::feed::storage::OwnerOutcome;
use crate::ota::install::{RefusedReason, StopStage};

crate::named_enum! {
    /// The tags the host tools act on.
    pub enum Tag {
        /// Startup: `start build=… bank=…`, `result diagnostic=…`.
        Start = "PLFW1",
        /// KM0's boot trial: `confirmed=0 boot=N limit=3`, `give_way=1`.
        Trial = "PLTRL1",
        /// Confirmation: `confirmed=1 healthy_ms=…`.
        Confirm = "PLCNF1",
        /// Offer check: `offer=1 version=…`, `give_way=1 version=…`.
        Watch = "PLWCH1",
        /// Bank 0's update stage: `manifest`, `progress sectors=N/T`,
        /// `activated`, `refused reason=…`, `error stage=…`.
        Update = "PLOTA1",
        /// Settings writer: `saved generation=…`, `net saved stored=…`,
        /// `keys change=…`.
        Settings = "PLCFG1",
        /// Wall clock: `sync ok=…`.
        Clock = "PLTIM1",
        /// BLE setup: `ready`, `advertising on=…`, `drop reason=…`.
        Ble = "PLBLE1",
        /// The feeding owner: `state=…`, `feed id=…`, `fed id=…`,
        /// `interlock …`.
        Owner = "PLOWN1",
        /// The device API: `session …`, `request slot=… op=… result=…`.
        Api = "PLAPI1",
        /// The panic handler: `panic file=… line=… col=…`, then a halt the
        /// watchdog resets.
        Panic = "PLPNC1",
    }
}

/// What a record says, for the tags and forms the bench acts on. A form
/// the bench does not act on decodes to `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event<'a> {
    /// `PLFW1 start`: `bank` is `None` when the startup could not tell.
    Start {
        build: &'a str,
        bank: Option<u8>,
    },
    Trial(Trial),
    Confirm(Confirm<'a>),
    Watch(Watch),
    Update(Update),
    Settings(Settings),
    Owner(Owner),
    Api(Api<'a>),
    Panic {
        file: &'a str,
        line: Option<u32>,
    },
}

/// KM0's boot trial (`PLTRL1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trial {
    /// An unconfirmed build is on its `boot`th of `limit` boots.
    Booting { boot: u32, limit: Option<u32> },
    /// Bank 1 is confirmed: no trial.
    Confirmed,
    /// The trial ran out and bank 1 gave way to bank 0.
    GiveWay,
}

/// The running build's confirmation (`PLCNF1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirm<'a> {
    /// Confirmed this boot after `healthy_ms`, or `None` when the record
    /// already named this build.
    Confirmed { healthy_ms: Option<u32> },
    /// Confirming failed at `stage`: the HAL's word (`read`, `write`,
    /// `floor`) or its flash hooks' (`timeout`, `clock`).
    Error { stage: &'a str },
}

/// Bank 1's offer check (`PLWCH1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Watch {
    Offer {
        version: Option<u32>,
    },
    GiveWay {
        version: Option<u32>,
    },
    /// The give-way could not be recorded.
    GiveWayRefused,
}

/// Bank 0's update stage (`PLOTA1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Update {
    Manifest {
        version: Option<u32>,
        bytes: Option<u32>,
    },
    Progress {
        sectors: u32,
        total: u32,
    },
    Activated {
        version: Option<u32>,
    },
    Refused {
        reason: RefusedReason,
    },
    /// The install stopped with bank 1 invalid.
    Stopped {
        stage: StopStage,
    },
}

/// The settings writer (`PLCFG1`): the flash writes the bench waits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settings {
    Saved {
        generation: Option<u64>,
    },
    /// The network credentials were written: `stored` when a network is
    /// now saved, not when it was cleared.
    NetSaved {
        stored: bool,
    },
    KeysChanged,
}

/// The feeding owner (`PLOWN1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    State {
        state: OwnerState,
        stopped: bool,
    },
    /// A feed was handed to the owner, whether or not it took it
    /// (`submit=`); the bench waits on the record, not the answer.
    Submitted {
        id: u64,
    },
    /// A feed was refused before the owner saw it.
    Held,
    Fed {
        id: u64,
        requested: u16,
        completed: u16,
        outcome: Option<OwnerOutcome>,
        reason: Option<FeedReason>,
    },
    Interlock {
        writes_deferred: Option<u32>,
    },
}

/// The device API (`PLAPI1`). Its words are the product's (the device API
/// crate's), which the SDK cannot name, so they stay text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Api<'a> {
    Request {
        op: Option<&'a str>,
    },
    /// A session was accepted, opened or ended, with a `reason` when it
    /// ended or was refused.
    Session {
        reason: Option<&'a str>,
    },
}

/// One record: its tag (`PLCFG1`), first word, and `key=value` fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record<'a> {
    pub tag: &'a str,
    pub first: &'a str,
    body: &'a str,
}

/// The length of the tag starting `text`, with its space: `PL`, two to five
/// uppercase letters, a digit, then a space (`PLFW1 `, `PLCFG1 `). Five
/// letters, for `PLPANEL1`: a tag the parser skipped would run its body
/// into the record before it.
fn tag_len(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    if !bytes.starts_with(b"PL") {
        return None;
    }
    let letters = bytes[2..]
        .iter()
        .take_while(|b| b.is_ascii_uppercase())
        .count();
    let digit = 2 + letters;
    ((2..=5).contains(&letters)
        && bytes.get(digit).is_some_and(u8::is_ascii_digit)
        && bytes.get(digit + 1) == Some(&b' '))
    .then_some(digit + 2)
}

/// Where the next tag in `text` starts, and its length with the space.
fn next_tag(text: &str) -> Option<(usize, usize)> {
    text.match_indices("PL")
        .find_map(|(at, _)| Some((at, tag_len(&text[at..])?)))
}

impl<'a> Record<'a> {
    /// The first record in `line`.
    pub fn parse(line: &'a str) -> Option<Self> {
        Self::all(line).next()
    }

    /// Every record in `line`, in order; each body ends where the next tag
    /// starts.
    pub fn all(line: &'a str) -> Records<'a> {
        Records { rest: line }
    }

    pub fn field(&self, key: &str) -> Option<&'a str> {
        self.body
            .split_whitespace()
            .find_map(|word| word.strip_prefix(key)?.strip_prefix('='))
    }

    pub fn number<T: core::str::FromStr>(&self, key: &str) -> Option<T> {
        self.field(key)?.parse().ok()
    }

    /// The text after the tag.
    pub fn body(&self) -> &'a str {
        self.body
    }

    /// A `key=0|1` field.
    pub fn flag(&self, key: &str) -> Option<bool> {
        match self.field(key)? {
            "0" => Some(false),
            "1" => Some(true),
            _ => None,
        }
    }

    /// The tag, when it is one the host tools act on.
    pub fn kind(&self) -> Option<Tag> {
        Tag::parse(self.tag)
    }

    /// What the record says, for the forms the bench acts on.
    pub fn event(&self) -> Option<Event<'a>> {
        Some(match self.kind()? {
            Tag::Start => {
                if self.first != "start" {
                    return None;
                }
                Event::Start {
                    build: self.field("build")?,
                    bank: self.number("bank"),
                }
            }
            Tag::Trial => Event::Trial(self.trial()?),
            Tag::Confirm => Event::Confirm(self.confirm()?),
            Tag::Watch => Event::Watch(self.watch()?),
            Tag::Update => Event::Update(self.update()?),
            Tag::Settings => Event::Settings(self.settings()?),
            Tag::Owner => Event::Owner(self.owner()?),
            Tag::Api => Event::Api(match self.first {
                "request" => Api::Request {
                    op: self.field("op"),
                },
                "session" => Api::Session {
                    reason: self.field("reason"),
                },
                _ => return None,
            }),
            Tag::Panic => Event::Panic {
                file: self.field("file")?,
                line: self.number("line"),
            },
            Tag::Clock | Tag::Ble => return None,
        })
    }

    fn confirm(&self) -> Option<Confirm<'a>> {
        if self.first == "error" {
            return Some(Confirm::Error {
                stage: self.field("stage")?,
            });
        }
        self.flag("confirmed")?.then(|| Confirm::Confirmed {
            healthy_ms: self.number("healthy_ms"),
        })
    }

    /// The trial's first token is a field, not a word.
    fn trial(&self) -> Option<Trial> {
        if self.flag("give_way") == Some(true) {
            return Some(Trial::GiveWay);
        }
        match self.flag("confirmed")? {
            true => Some(Trial::Confirmed),
            false => Some(Trial::Booting {
                boot: self.number("boot")?,
                limit: self.number("limit"),
            }),
        }
    }

    fn watch(&self) -> Option<Watch> {
        let version = self.number("version");
        if self.flag("give_way") == Some(true) {
            Some(Watch::GiveWay { version })
        } else if self.flag("offer") == Some(true) {
            Some(Watch::Offer { version })
        } else if self.first == "error" && self.field("stage") == Some("give_way") {
            Some(Watch::GiveWayRefused)
        } else {
            None
        }
    }

    fn update(&self) -> Option<Update> {
        Some(match self.first {
            "manifest" => Update::Manifest {
                version: self.number("version"),
                bytes: self.number("bytes"),
            },
            "progress" => {
                let (sectors, total) = self.field("sectors")?.split_once('/')?;
                Update::Progress {
                    sectors: sectors.parse().ok()?,
                    total: total.parse().ok()?,
                }
            }
            "activated" => Update::Activated {
                version: self.number("version"),
            },
            "refused" => Update::Refused {
                reason: RefusedReason::parse(self.field("reason")?)?,
            },
            "error" => Update::Stopped {
                stage: StopStage::parse(self.field("stage")?)?,
            },
            _ => return None,
        })
    }

    fn settings(&self) -> Option<Settings> {
        let mut words = self.body.split_whitespace();
        Some(match (words.next()?, words.next()) {
            ("saved", _) => Settings::Saved {
                generation: self.number("generation"),
            },
            ("net", Some("saved")) => Settings::NetSaved {
                stored: self.flag("stored")?,
            },
            ("keys", Some(word)) if word.starts_with("change=") => Settings::KeysChanged,
            _ => return None,
        })
    }

    fn owner(&self) -> Option<Owner> {
        if let Some(state) = self.number("state") {
            return Some(Owner::State {
                state: OwnerState::from_code(state)?,
                stopped: self.flag("stopped")?,
            });
        }
        Some(match self.first {
            "feed" if self.field("held").is_some() => Owner::Held,
            "feed" => Owner::Submitted {
                id: self.number("id")?,
            },
            "fed" => Owner::Fed {
                id: self.number("id")?,
                requested: self.number("requested")?,
                completed: self.number("completed")?,
                outcome: self.number("outcome").and_then(OwnerOutcome::from_byte),
                reason: self.number("reason").and_then(FeedReason::from_byte),
            },
            "interlock" => Owner::Interlock {
                writes_deferred: self.number("writes_deferred"),
            },
            _ => return None,
        })
    }
}

/// The records of one line ([`Record::all`]).
pub struct Records<'a> {
    rest: &'a str,
}

impl<'a> Iterator for Records<'a> {
    type Item = Record<'a>;

    fn next(&mut self) -> Option<Record<'a>> {
        let (at, len) = next_tag(self.rest)?;
        let tag = &self.rest[at..at + len - 1];
        let after = &self.rest[at + len..];
        let end = next_tag(after).map_or(after.len(), |(next, _)| next);

        let body = after[..end].trim();
        self.rest = &after[end..];
        Some(Record {
            tag,
            first: body.split_whitespace().next().unwrap_or(""),
            body,
        })
    }
}
