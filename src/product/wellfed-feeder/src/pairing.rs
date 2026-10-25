//! Who may become a client, and when.
//!
//! Four ways in ([product architecture]), each needing either someone at
//! the feeder or a client already trusted:
//! - **Setup:** holding Lock and Mic together on an unlocked panel starts
//!   BLE setup mode ([`Event::SetupOpened`]). There the app offers its key
//!   and shows a one-time code, which the owner keys in on the buttons
//!   within the confirmation window ([`Event::Entered`]). The firmware's
//!   setup session then admits the app as an owner only if it proves the
//!   same code (`wellfed_session::setup_code`); a press alone is not enough,
//!   since it cannot tell the owner's phone from a relay in radio range.
//! - **Signer:** an owner offers an Ed25519 update-signing key; a
//!   Dispense press confirms it the same way.
//! - **Invite:** a paired client mints a single-use invite that expires
//!   after [`Config::invite_ms`]. Its secret is the PSK of the redeeming
//!   client's Noise XXpsk3 handshake over the LAN. An invite never grants
//!   more than its issuer holds, and invites live in RAM only, so a reboot
//!   cancels them.
//! - **Recovery:** with the panel unlocked, holding Lock and Dispense
//!   together opens a LAN window. A client that connects in it runs the same
//!   exchange as BLE setup over the device API: its offer closes the
//!   window ([`Pairing::recovery`]) and the owner keys its code in, so a
//!   neighbour on the LAN cannot slip in while the window is open. This is
//!   the way back in when every client is lost; a locked panel cannot open
//!   it.
//!
//! A pending confirmation takes the next Dispense press, and a pending code
//! every gesture until it is complete, whatever the lock says: the presses
//! are someone at the feeder answering a pairing they started, and they
//! feed nothing. [`Pairing::gesture`] returns an event for
//! every gesture it consumes, and the firmware gives the panel only the
//! rest.
//!
//! The policy holds no client keys: the firmware applies an [`Event::Admit`]
//! to the client store. Invite secrets do live here, since a handshake
//! needs its PSK before the client is known.
//!
//! [product architecture]: ../../../../docs/firmware/product-architecture.md#setup-and-pairing

use wellfed_core::buttons::{Button, Buttons, Gesture};
use wellfed_core::store::clients::Role;
use wellfed_core::{deadline_passed, interval_valid, named_enum};
use wellfed_session::setup_code::{Code, PRESSES};

use crate::panel::Lock;

/// Invites outstanding at once.
pub const INVITES: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// How long a setup offer waits for its code, or a signer offer for
    /// its press.
    pub confirm_ms: u32,
    /// An invite's lifetime.
    pub invite_ms: u32,
    /// How long the recovery window stays open.
    pub recovery_ms: u32,
    /// The hold level (of the panel's button thresholds) that opens
    /// recovery or setup mode.
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
        /// Another offer is already waiting for its press or code.
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
        /// Nobody pressed Dispense or keyed the code in in time.
        Unconfirmed = "unconfirmed",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// An offer waits for a Dispense press, or setup's for its code.
    Confirming {
        path: Path,
    },
    /// A press counted towards the code of the offer behind `path` (setup
    /// or recovery), which now has `entered`.
    Keyed {
        path: Path,
        entered: u8,
    },
    /// A chord or a hold went to that code and counted for nothing.
    Swallowed {
        path: Path,
    },
    /// The owner keyed in the code of the offer behind `path` (setup or
    /// recovery): its session must now prove it. The firmware records this
    /// without the code.
    Entered {
        path: Path,
        code: Code,
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
    /// Start BLE setup mode: the firmware brings up the radio until setup
    /// ends with a reboot.
    SetupOpened,
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
    confirm: Confirm,
}

/// What an offer waits for.
#[derive(Clone, Copy, Debug)]
enum Confirm {
    Press,
    Code {
        presses: [Button; PRESSES],
        entered: usize,
    },
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

    /// The app in BLE setup mode offers its key; the next presses are its
    /// code.
    pub fn setup(&mut self, now_ms: u32) -> Event {
        let confirm = Confirm::Code {
            presses: [Button::Reset; PRESSES],
            entered: 0,
        };
        self.offer(now_ms, Path::Setup, Role::Owner, confirm)
    }

    /// A client with role `by` offers an update-signing key.
    pub fn signer(&mut self, now_ms: u32, by: Role) -> Event {
        if by != Role::Owner {
            return refused(Path::Signer, Refusal::NotAllowed);
        }
        self.offer(now_ms, Path::Signer, Role::UpdateSigner, Confirm::Press)
    }

    /// The client behind a pending `path` offer went away: forget it, so the
    /// next Dispense press feeds again.
    pub fn cancel(&mut self, path: Path) {
        if self.pending.is_some_and(|pending| pending.path == path) {
            self.pending = None;
        }
    }

    fn offer(&mut self, now_ms: u32, path: Path, role: Role, confirm: Confirm) -> Event {
        if self.pending.is_some() {
            return refused(path, Refusal::Busy);
        }
        self.pending = Some(Pending {
            path,
            role,
            deadline_ms: now_ms.wrapping_add(self.config.confirm_ms),
            confirm,
        });
        Event::Confirming { path }
    }

    /// One gesture from the button service; `Some` means it was consumed.
    pub fn gesture(&mut self, now_ms: u32, gesture: Gesture, lock: Lock) -> Option<Event> {
        let recovery = Buttons::of(Button::Lock).with(Button::Dispense);
        let setup = Buttons::of(Button::Lock).with(Button::Mic);
        if let Some(Pending {
            confirm: Confirm::Code { .. },
            ..
        }) = self.pending
        {
            return self.code(now_ms, gesture);
        }
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
            Gesture::Hold { chord, level } if chord == setup && level == self.config.hold_level => {
                if lock.locked() {
                    return Some(refused(Path::Setup, Refusal::Locked));
                }
                Some(Event::SetupOpened)
            }
            _ => None,
        }
    }

    /// One gesture while a code (setup's or recovery's) is being keyed in. A press of one
    /// button is one press of the code. A chord or a hold is swallowed and
    /// counts for nothing, so no lock toggle or key reset fires mid-code; a
    /// release goes on to the panel, which only notes it.
    fn code(&mut self, now_ms: u32, gesture: Gesture) -> Option<Event> {
        let pending = self.pending.as_mut()?;
        let path = pending.path;
        if deadline_passed(now_ms, pending.deadline_ms) {
            self.pending = None;
            return Some(refused(path, Refusal::Unconfirmed));
        }
        let Confirm::Code { presses, entered } = &mut pending.confirm else {
            return None;
        };
        let chord = match gesture {
            Gesture::Down { chord } => chord,
            Gesture::Hold { .. } => return Some(Event::Swallowed { path }),
            Gesture::Up { .. } => return None,
        };
        let pressed = Button::ALL.iter().find(|&&b| chord == Buttons::of(b));
        let (Some(&button), Some(slot)) = (pressed, presses.get_mut(*entered)) else {
            return Some(Event::Swallowed { path });
        };

        *slot = button;
        *entered += 1;
        if *entered < PRESSES {
            return Some(keyed(path, *entered));
        }
        let code = Code::new(*presses);
        self.pending = None;
        Some(Event::Entered { path, code })
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

    /// A client in the recovery window offered its key: close the window
    /// and wait for its code, as setup does. Only the one client that
    /// proves the code becomes an owner; a second has nothing to offer to.
    pub fn recovery(&mut self, now_ms: u32) -> Event {
        match self.recovery_until {
            Some(until) if !deadline_passed(now_ms, until) => {
                if self.pending.is_some() {
                    return refused(Path::Recovery, Refusal::Busy);
                }
                self.recovery_until = None;
                let confirm = Confirm::Code {
                    presses: [Button::Reset; PRESSES],
                    entered: 0,
                };
                self.offer(now_ms, Path::Recovery, Role::Owner, confirm)
            }
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

fn keyed(path: Path, entered: usize) -> Event {
    Event::Keyed {
        path,
        entered: u8::try_from(entered).unwrap_or(u8::MAX),
    }
}

const fn refused(path: Path, refusal: Refusal) -> Event {
    Event::Refused { path, refusal }
}
