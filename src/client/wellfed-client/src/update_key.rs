//! Enrolling an owner's update-signing key, sans-IO: what to send next and
//! what the feeder's answers mean, for the CLI and, later, the installer
//! right after it pairs.
//!
//! An enrolled key signs updates the feeder admits beside its built-in
//! keys, so the feeder takes one only from an owner's session that claimed
//! updates, and only once someone at the feeder presses Dispense within its
//! 60 s window; [`crate::connection::Connection::request`] waits that long
//! for the answer. The press is the proof that the person enrolling it has
//! the feeder in hand, as a setup code is for pairing.
//!
//! The steps, driven by [`Enroll::poll`] and [`Enroll::answer`]:
//! 1. [`Enroll::new`] refuses a session that cannot enroll (not an owner,
//!    or no updates on either side) before anyone is asked to press.
//! 2. The feeder's client list: a key already an update signer needs
//!    nothing sent, and a key the feeder holds under another role is a
//!    session key, never an update signer ([`Enroll::listed`]).
//! 3. `EnrollUpdateKey`, answered once Dispense is pressed or the window
//!    closes. It is sent once: a second offer would need a second press,
//!    and an unanswered one may already have enrolled the key (a press
//!    admitted before the session ended still writes), so it ends
//!    [`EnrollError::Uncertain`] and the caller reads the list again.

use wellfed_model::{Answer, Capabilities, Client, ClientRole, Op, Refusal, UpdateRefusal};
pub use wellfed_proto::setup::LABEL_MAX;

/// What [`Enroll::listed`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Listed {
    /// The feeder already trusts the key: send nothing.
    Enrolled,
    /// Send the enrollment, and have the owner press Dispense.
    Absent,
}

/// How an enrollment ended well.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Enrolled {
    /// The press confirmed it now.
    Now,
    /// The feeder already listed it; nothing was sent.
    Already,
}

/// What the caller does next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Next {
    /// Send this request under a fresh id, then pass its answer to
    /// [`Enroll::answer`], or call [`Enroll::unanswered`].
    Send(Op),
    /// Tell the owner to press Dispense on the feeder within 60 s, then
    /// send this request as [`Self::Send`].
    Press(Op),
    Done(Enrolled),
    Failed(EnrollError),
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
    /// No answer came to the enrollment: a press may have enrolled the key
    /// all the same. Read the client list to know; never send it again.
    #[error("no answer came to the enrollment; the key may be enrolled all the same")]
    Uncertain,
    #[error("the feeder refused the key: {0:?}")]
    Refused(Refusal),
    #[error("the feeder answered with something other than the enrollment's outcome")]
    Unexpected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    /// Read the client list next.
    Start,
    Listing,
    /// Absent from the list: the enrollment goes next.
    Absent,
    Enrolling,
    Ended(Result<Enrolled, EnrollError>),
}

/// One key to enroll over one owner's session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Enroll {
    key: [u8; 32],
    label: String,
    stage: Stage,
}

/// Check a label as the feeder will: 1 to [`LABEL_MAX`] bytes, no NUL.
pub fn label(text: &str) -> Result<(), EnrollError> {
    if text.is_empty() || text.len() > LABEL_MAX || text.contains('\0') {
        return Err(EnrollError::Label);
    }
    Ok(())
}

impl Enroll {
    /// Enroll the public key `key` as `label` over a session with `role`
    /// and `capabilities`; refused here when the feeder would refuse it
    /// `Forbidden`.
    pub fn new(
        key: [u8; 32],
        label_text: &str,
        role: ClientRole,
        capabilities: Capabilities,
    ) -> Result<Self, EnrollError> {
        label(label_text)?;
        if role != ClientRole::Owner {
            return Err(EnrollError::NotOwner { role });
        }
        if !capabilities.update {
            return Err(EnrollError::NoUpdates);
        }
        Ok(Self {
            key,
            label: label_text.to_owned(),
            stage: Stage::Start,
        })
    }

    pub fn key(&self) -> [u8; 32] {
        self.key
    }

    /// What to do next.
    pub fn poll(&mut self) -> Next {
        match self.stage {
            Stage::Start => {
                self.stage = Stage::Listing;
                Next::Send(Op::ListClients)
            }
            Stage::Absent => {
                self.stage = Stage::Enrolling;
                Next::Press(Op::EnrollUpdateKey {
                    key: self.key,
                    label: self.label.clone(),
                })
            }
            // The caller asks one request at a time: one in flight is a
            // caller that polled before answering it.
            Stage::Listing | Stage::Enrolling => Next::Failed(EnrollError::Unexpected),
            Stage::Ended(Ok(enrolled)) => Next::Done(enrolled),
            Stage::Ended(Err(error)) => Next::Failed(error),
        }
    }

    /// The feeder's answer to the request [`Self::poll`] gave last.
    pub fn answer(&mut self, answer: &Answer) {
        self.stage = match self.stage {
            Stage::Listing => match answer {
                Answer::Clients(clients) => match self.listed(clients) {
                    Ok(Listed::Enrolled) => Stage::Ended(Ok(Enrolled::Already)),
                    Ok(Listed::Absent) => Stage::Absent,
                    Err(error) => Stage::Ended(Err(error)),
                },
                answer => Stage::Ended(Err(refusal(answer))),
            },
            Stage::Enrolling => Stage::Ended(match answer {
                Answer::Done { .. } => Ok(Enrolled::Now),
                answer => Err(refusal(answer)),
            }),
            Stage::Start | Stage::Absent | Stage::Ended(_) => {
                Stage::Ended(Err(EnrollError::Unexpected))
            }
        };
    }

    /// No answer came to the request [`Self::poll`] gave last. Reading the
    /// list changed nothing, but an unanswered enrollment may have
    /// enrolled the key.
    pub fn unanswered(&mut self) {
        self.stage = Stage::Ended(Err(match self.stage {
            Stage::Enrolling => EnrollError::Uncertain,
            _ => EnrollError::Unexpected,
        }));
    }

    /// Whether the feeder's `clients` already hold the key.
    pub fn listed(&self, clients: &[Client]) -> Result<Listed, EnrollError> {
        match clients.iter().find(|client| client.key.0 == self.key) {
            None => Ok(Listed::Absent),
            Some(client) if client.role == ClientRole::UpdateSigner => Ok(Listed::Enrolled),
            Some(client) => Err(EnrollError::SessionKey { role: client.role }),
        }
    }
}

/// What a refusal of the list or the enrollment means.
fn refusal(answer: &Answer) -> EnrollError {
    match answer {
        Answer::Refused(Refusal::Update(UpdateRefusal::Unconfirmed)) => EnrollError::Unconfirmed,
        Answer::Refused(Refusal::Forbidden) => EnrollError::Forbidden,
        Answer::Refused(Refusal::Busy) => EnrollError::Busy,
        Answer::Refused(Refusal::Full) => EnrollError::Full,
        Answer::Refused(refusal) => EnrollError::Refused(*refusal),
        _ => EnrollError::Unexpected,
    }
}

#[cfg(test)]
mod tests {
    use wellfed_model::{Generation, Hex};

    use super::*;

    const KEY: [u8; 32] = [0x5e; 32];

    fn owner() -> Capabilities {
        Capabilities {
            update: true,
            ..Capabilities::default()
        }
    }

    fn client(key: [u8; 32], role: ClientRole) -> Client {
        Client {
            key: Hex(key),
            label: "x".into(),
            role,
            you: false,
        }
    }

    fn enroll() -> Enroll {
        Enroll::new(KEY, "laptop", ClientRole::Owner, owner()).unwrap()
    }

    /// The enrollment after a list without the key.
    fn enrolling() -> (Enroll, Op) {
        let mut enroll = enroll();
        assert_eq!(enroll.poll(), Next::Send(Op::ListClients));
        enroll.answer(&Answer::Clients(vec![client([1; 32], ClientRole::Owner)]));
        let Next::Press(op) = enroll.poll() else {
            panic!("the enrollment comes after the list");
        };
        (enroll, op)
    }

    #[test]
    fn a_session_or_label_the_feeder_would_refuse_is_refused_first() {
        for role in [ClientRole::Member, ClientRole::Server] {
            assert_eq!(
                Enroll::new(KEY, "laptop", role, owner()),
                Err(EnrollError::NotOwner { role })
            );
        }
        assert_eq!(
            Enroll::new(KEY, "laptop", ClientRole::Owner, Capabilities::default()),
            Err(EnrollError::NoUpdates)
        );
        for bad in ["", "seventeen-letters", "a\0b"] {
            assert_eq!(
                Enroll::new(KEY, bad, ClientRole::Owner, owner()),
                Err(EnrollError::Label)
            );
        }
        assert_eq!(label(&"x".repeat(LABEL_MAX)), Ok(()));
    }

    #[test]
    fn a_listed_key_sends_nothing_and_a_session_key_is_refused() {
        let mut enroll = enroll();
        assert_eq!(enroll.poll(), Next::Send(Op::ListClients));
        enroll.answer(&Answer::Clients(vec![client(
            KEY,
            ClientRole::UpdateSigner,
        )]));
        assert_eq!(enroll.poll(), Next::Done(Enrolled::Already));
        assert_eq!(enroll.poll(), Next::Done(Enrolled::Already));

        for role in [ClientRole::Owner, ClientRole::Member, ClientRole::Server] {
            let listed = [client(KEY, role)];
            assert_eq!(
                enroll.listed(&listed),
                Err(EnrollError::SessionKey { role })
            );
            let mut again = Enroll::new(KEY, "laptop", ClientRole::Owner, owner()).unwrap();
            again.poll();
            again.answer(&Answer::Clients(listed.to_vec()));
            assert_eq!(again.poll(), Next::Failed(EnrollError::SessionKey { role }));
        }
    }

    #[test]
    fn the_enrollment_is_its_key_and_label() {
        let (_, op) = enrolling();
        assert_eq!(
            op,
            Op::EnrollUpdateKey {
                key: KEY,
                label: "laptop".into()
            }
        );
    }

    #[test]
    fn every_answer_to_the_enrollment_has_one_meaning() {
        let cases = [
            (Answer::Done { generation: None }, Ok(Enrolled::Now)),
            (
                Answer::Done {
                    generation: Some(Generation(3)),
                },
                Ok(Enrolled::Now),
            ),
            (
                Answer::Refused(Refusal::Update(UpdateRefusal::Unconfirmed)),
                Err(EnrollError::Unconfirmed),
            ),
            (
                Answer::Refused(Refusal::Forbidden),
                Err(EnrollError::Forbidden),
            ),
            (Answer::Refused(Refusal::Busy), Err(EnrollError::Busy)),
            (Answer::Refused(Refusal::Full), Err(EnrollError::Full)),
            (
                Answer::Refused(Refusal::Invalid),
                Err(EnrollError::Refused(Refusal::Invalid)),
            ),
            (
                Answer::Refused(Refusal::NotWritable),
                Err(EnrollError::Refused(Refusal::NotWritable)),
            ),
            (Answer::Clients(Vec::new()), Err(EnrollError::Unexpected)),
        ];
        for (answer, outcome) in cases {
            let (mut enroll, _) = enrolling();
            enroll.answer(&answer);
            let expected = match outcome {
                Ok(enrolled) => Next::Done(enrolled),
                Err(error) => Next::Failed(error),
            };
            assert_eq!(enroll.poll(), expected, "{answer:?}");
        }
    }

    /// An unanswered enrollment may have enrolled the key, and is never
    /// sent again; an unanswered list is just a failure.
    #[test]
    fn an_unanswered_enrollment_is_uncertain_and_not_resent() {
        let (mut enroll, _) = enrolling();
        enroll.unanswered();
        assert_eq!(enroll.poll(), Next::Failed(EnrollError::Uncertain));
        assert_eq!(enroll.poll(), Next::Failed(EnrollError::Uncertain));

        let mut listing = self::enroll();
        listing.poll();
        listing.unanswered();
        assert_eq!(listing.poll(), Next::Failed(EnrollError::Unexpected));

        let mut refused = self::enroll();
        refused.poll();
        refused.answer(&Answer::Refused(Refusal::Forbidden));
        assert_eq!(refused.poll(), Next::Failed(EnrollError::Forbidden));
    }
}
