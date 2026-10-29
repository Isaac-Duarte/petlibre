//! Enrolling an owner's update-signing key, sans-IO: whether to ask, what
//! to send and what the feeder's answer means, for the CLI and, later, the
//! installer right after it pairs.
//!
//! An enrolled key signs updates the feeder admits beside its built-in
//! keys, so the feeder takes one only from an owner's session that claimed
//! updates, and only once someone at the feeder presses Dispense within its
//! 60 s window; [`crate::connection::Connection::request`] waits that long
//! for the answer. The press is the proof that the person enrolling it has
//! the feeder in hand, as a setup code is for pairing.
//!
//! The steps:
//! 1. [`Enroll::session`] refuses a session that cannot enroll: not an
//!    owner, or no updates on either side.
//! 2. [`Enroll::listed`] reads the feeder's client list: a key already an
//!    update signer needs nothing sent, and a key the feeder holds under
//!    another role is a session key, never an update signer.
//! 3. [`Enroll::request`] is the op, and [`Enroll::answer`] reads its
//!    answer. Nothing here sends it twice: a second offer would need a
//!    second press.

use wellfed_model::{Answer, Capabilities, Client, ClientRole, Op, Refusal, UpdateRefusal};

/// The longest label the feeder stores for a client.
pub const LABEL_MAX: usize = 16;

/// What [`Enroll::listed`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Listed {
    /// The feeder already trusts the key: send nothing.
    Enrolled,
    /// Send [`Enroll::request`], and have the owner press Dispense.
    Absent,
}

/// Why a key was not enrolled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EnrollError {
    #[error("the label must be 1-{LABEL_MAX} bytes with no NUL")]
    Label,
    #[error("only an owner enrolls an update key; this session is a {role:?}")]
    NotOwner { role: ClientRole },
    #[error("this feeder takes no updates over the device API")]
    NoUpdates,
    /// The feeder's own check of the session: owners whose session claimed
    /// updates only.
    #[error("the feeder takes update keys from an owner's session with updates only")]
    Forbidden,
    /// The key is enrolled on the feeder as a client that opens sessions.
    /// The feeder itself refuses such a key `Invalid`, but only after the
    /// press; the list read first names its role.
    #[error("the feeder holds this key as a {role:?}; an update key must be a key of its own")]
    SessionKey { role: ClientRole },
    #[error("nobody pressed Dispense on the feeder within its 60 s window")]
    Unconfirmed,
    /// Another enrollment waits for its press, or a feed or a store write
    /// is in progress.
    #[error("the feeder is busy (another key waits for its press, or a store write runs)")]
    Busy,
    #[error("every client slot on the feeder is taken")]
    Full,
    #[error("the feeder refused the key: {0:?}")]
    Refused(Refusal),
    #[error("the feeder answered with something other than the enrollment's outcome")]
    Unexpected,
}

/// One key to enroll, under the label the owner sees it by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Enroll {
    key: [u8; 32],
    label: String,
}

impl Enroll {
    /// The public key `key`, labelled `label`.
    pub fn new(key: [u8; 32], label: &str) -> Result<Self, EnrollError> {
        if label.is_empty() || label.len() > LABEL_MAX || label.contains('\0') {
            return Err(EnrollError::Label);
        }
        Ok(Self {
            key,
            label: label.to_owned(),
        })
    }

    pub fn key(&self) -> [u8; 32] {
        self.key
    }

    /// Refuse a session the feeder would refuse `Forbidden` anyway, before
    /// anyone is asked to press.
    pub fn session(role: ClientRole, capabilities: Capabilities) -> Result<(), EnrollError> {
        if role != ClientRole::Owner {
            return Err(EnrollError::NotOwner { role });
        }
        if !capabilities.update {
            return Err(EnrollError::NoUpdates);
        }
        Ok(())
    }

    /// Whether the feeder's `clients` already hold the key.
    pub fn listed(&self, clients: &[Client]) -> Result<Listed, EnrollError> {
        match clients.iter().find(|client| client.key.0 == self.key) {
            None => Ok(Listed::Absent),
            Some(client) if client.role == ClientRole::UpdateSigner => Ok(Listed::Enrolled),
            Some(client) => Err(EnrollError::SessionKey { role: client.role }),
        }
    }

    /// The request; its answer comes once Dispense is pressed, or when the
    /// feeder's window closes.
    pub fn request(&self) -> Op {
        Op::EnrollUpdateKey {
            key: self.key,
            label: self.label.clone(),
        }
    }

    /// What the feeder's answer to [`Self::request`] means.
    pub fn answer(&self, answer: &Answer) -> Result<(), EnrollError> {
        match answer {
            Answer::Done { .. } => Ok(()),
            Answer::Refused(Refusal::Update(UpdateRefusal::Unconfirmed)) => {
                Err(EnrollError::Unconfirmed)
            }
            Answer::Refused(Refusal::Forbidden) => Err(EnrollError::Forbidden),
            Answer::Refused(Refusal::Busy) => Err(EnrollError::Busy),
            Answer::Refused(Refusal::Full) => Err(EnrollError::Full),
            Answer::Refused(refusal) => Err(EnrollError::Refused(*refusal)),
            _ => Err(EnrollError::Unexpected),
        }
    }
}
