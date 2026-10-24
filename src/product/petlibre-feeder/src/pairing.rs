//! FW45: who may become a client, and when.
//!
//! Four ways in ([product architecture]), each needing either someone at
//! the feeder or a client already trusted:
//! - **Setup:** in BLE setup mode the app sends Wi-Fi and its key; it is
//!   admitted as an owner only after a Dispense press within the
//!   confirmation window.
//! - **Signer:** an owner offers an Ed25519 update-signing key (FW48); a
//!   Dispense press confirms it the same way.
//! - **Invite:** a paired client mints a single-use invite that expires
//!   after [`Config::invite_ms`]. Its secret is the PSK of the redeeming
//!   client's Noise XXpsk3 handshake over the LAN. An invite never grants
//!   more than its issuer holds, and invites live in RAM only, so a reboot
//!   cancels them.
//! - **Recovery:** with the panel unlocked, holding Lock and Dispense
//!   together opens a LAN window that admits exactly one owner over a plain
//!   XX handshake, then closes. This is the way back in when every client
//!   is lost; a locked panel cannot open it.
//!
//! A pending confirmation takes the next Dispense press whatever the lock
//! says: the press is someone at the feeder answering a pairing they
//! started, and it feeds nothing. [`Pairing::gesture`] returns an event for
//! every gesture it consumes, and the firmware gives the panel only the
//! rest.
//!
//! The policy holds no client keys: the firmware applies an [`Event::Admit`]
//! to the client store. Invite secrets do live here, since a handshake
//! needs its PSK before the client is known.
//!
//! [product architecture]: ../../../../docs/firmware/product-architecture.md#setup-and-pairing

use petlibre_core::buttons::{Button, Buttons, Gesture};
use petlibre_core::store::clients::Role;
use petlibre_core::{deadline_passed, interval_valid, named_enum};

use crate::panel::Lock;

/// Invites outstanding at once.
pub const INVITES: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// How long a setup or signer offer waits for its press.
    pub confirm_ms: u32,
    /// An invite's lifetime.
    pub invite_ms: u32,
    /// How long the recovery window stays open.
    pub recovery_ms: u32,
    /// The hold level (of the panel's button thresholds) that opens recovery.
    pub hold_level: u8,
}

impl Config {
    /// Ours: stock pairs through the vendor cloud and has no equivalent.
    pub const DEFAULT: Self = Self {
        confirm_ms: 60_000,
        invite_ms: 600_000,
        recovery_ms: 120_000,
        hold_level: 1,
    };

    pub const fn valid(&self) -> bool {
        self.hold_level > 0
            && interval_valid(self.confirm_ms)
            && interval_valid(self.invite_ms)
            && interval_valid(self.recovery_ms)
    }
}

named_enum! {
    pub enum Path {
        Setup = "setup",
        Signer = "signer",
        Invite = "invite",
        Recovery = "recovery",
    }
}

named_enum! {
    pub enum Refusal {
        /// Another offer is already waiting for its press.
        Busy = "busy",
        /// The issuer may not grant that role.
        NotAllowed = "not-allowed",
        /// No such invite, or it expired or was used.
        Unknown = "unknown",
        /// Every invite slot is taken.
        Full = "full",
        /// The recovery window is closed.
        Closed = "closed",
        /// The panel is locked.
        Locked = "locked",
        /// Nobody pressed Dispense in time.
        Unconfirmed = "unconfirmed",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// An offer waits for a Dispense press.
    Confirming {
        path: Path,
    },
    /// Enroll the client behind `path` with `role`.
    Admit {
        path: Path,
        role: Role,
    },
    Invited {
        id: u32,
    },
    RecoveryOpened,
    RecoveryClosed,
    Refused {
        path: Path,
        refusal: Refusal,
    },
}

#[derive(Clone, Copy, Debug)]
struct Pending {
    path: Path,
    role: Role,
    deadline_ms: u32,
}

#[derive(Clone, Copy)]
struct Invite {
    id: u32,
    role: Role,
    secret: [u8; 32],
    deadline_ms: u32,
}

#[derive(Clone, Copy)]
pub struct Pairing {
    config: Config,
    pending: Option<Pending>,
    recovery_until: Option<u32>,
    invites: [Option<Invite>; INVITES],
    next_id: u32,
}

impl core::fmt::Debug for Pairing {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Pairing")
            .field("pending", &self.pending)
            .field("recovery_until", &self.recovery_until)
            .finish_non_exhaustive()
    }
}

impl Pairing {
    pub const fn new(config: Config) -> Self {
        Self {
            config,
            pending: None,
            recovery_until: None,
            invites: [None; INVITES],
            next_id: 1,
        }
    }

    /// The app in BLE setup mode offers its key.
    pub fn setup(&mut self, now_ms: u32) -> Event {
        self.offer(now_ms, Path::Setup, Role::Owner)
    }

    /// A client with role `by` offers an update-signing key.
    pub fn signer(&mut self, now_ms: u32, by: Role) -> Event {
        if by != Role::Owner {
            return refused(Path::Signer, Refusal::NotAllowed);
        }
        self.offer(now_ms, Path::Signer, Role::UpdateSigner)
    }

    fn offer(&mut self, now_ms: u32, path: Path, role: Role) -> Event {
        if self.pending.is_some() {
            return refused(path, Refusal::Busy);
        }
        self.pending = Some(Pending {
            path,
            role,
            deadline_ms: now_ms.wrapping_add(self.config.confirm_ms),
        });
        Event::Confirming { path }
    }

    /// One gesture from the button service; `Some` means it was consumed.
    pub fn gesture(&mut self, now_ms: u32, gesture: Gesture, lock: Lock) -> Option<Event> {
        let recovery = Buttons::of(Button::Lock).with(Button::Dispense);
        match gesture {
            Gesture::Down { chord } if chord == Buttons::of(Button::Dispense) => {
                let pending = self.pending.take()?;
                if deadline_passed(now_ms, pending.deadline_ms) {
                    return Some(refused(pending.path, Refusal::Unconfirmed));
                }
                Some(Event::Admit {
                    path: pending.path,
                    role: pending.role,
                })
            }
            Gesture::Hold { chord, level }
                if chord == recovery && level == self.config.hold_level =>
            {
                if lock.locked() {
                    return Some(refused(Path::Recovery, Refusal::Locked));
                }
                self.recovery_until = Some(now_ms.wrapping_add(self.config.recovery_ms));
                Some(Event::RecoveryOpened)
            }
            _ => None,
        }
    }

    /// A client with role `by` asks for an invite granting `role`, with a
    /// secret the firmware drew from its generator.
    pub fn invite(&mut self, now_ms: u32, by: Role, role: Role, secret: [u8; 32]) -> Event {
        let allowed = match by {
            Role::Owner => role != Role::UpdateSigner,
            Role::Member => role == Role::Member,
            Role::Server | Role::UpdateSigner => false,
        };
        if !allowed {
            return refused(Path::Invite, Refusal::NotAllowed);
        }
        self.expire_invites(now_ms);
        let Some(slot) = self.invites.iter_mut().find(|slot| slot.is_none()) else {
            return refused(Path::Invite, Refusal::Full);
        };

        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        *slot = Some(Invite {
            id,
            role,
            secret,
            deadline_ms: now_ms.wrapping_add(self.config.invite_ms),
        });
        Event::Invited { id }
    }

    /// The PSK for a handshake naming invite `id`, while it is live.
    pub fn invite_secret(&mut self, now_ms: u32, id: u32) -> Option<[u8; 32]> {
        self.expire_invites(now_ms);
        self.invites
            .iter()
            .flatten()
            .find(|invite| invite.id == id)
            .map(|invite| invite.secret)
    }

    /// The handshake for invite `id` finished: use it up.
    pub fn redeem(&mut self, now_ms: u32, id: u32) -> Event {
        self.expire_invites(now_ms);
        let found = self
            .invites
            .iter_mut()
            .find(|slot| slot.is_some_and(|invite| invite.id == id));
        match found.and_then(Option::take) {
            Some(invite) => Event::Admit {
                path: Path::Invite,
                role: invite.role,
            },
            None => refused(Path::Invite, Refusal::Unknown),
        }
    }

    /// A plain XX handshake finished during the recovery window: admit it
    /// as an owner and close the window.
    pub fn recover(&mut self, now_ms: u32) -> Event {
        match self.recovery_until.take() {
            Some(until) if !deadline_passed(now_ms, until) => Event::Admit {
                path: Path::Recovery,
                role: Role::Owner,
            },
            _ => refused(Path::Recovery, Refusal::Closed),
        }
    }

    pub const fn recovery_open(&self) -> bool {
        self.recovery_until.is_some()
    }

    /// Expire an unanswered offer or the recovery window, one event per
    /// call; the firmware calls until `None`. Invites expire silently.
    pub fn tick(&mut self, now_ms: u32) -> Option<Event> {
        self.expire_invites(now_ms);
        if let Some(pending) = self.pending
            && deadline_passed(now_ms, pending.deadline_ms)
        {
            self.pending = None;
            return Some(refused(pending.path, Refusal::Unconfirmed));
        }
        if let Some(until) = self.recovery_until
            && deadline_passed(now_ms, until)
        {
            self.recovery_until = None;
            return Some(Event::RecoveryClosed);
        }
        None
    }

    fn expire_invites(&mut self, now_ms: u32) {
        for slot in &mut self.invites {
            if slot.is_some_and(|invite| deadline_passed(now_ms, invite.deadline_ms)) {
                *slot = None;
            }
        }
    }
}

const fn refused(path: Path, refusal: Refusal) -> Event {
    Event::Refused { path, refusal }
}
