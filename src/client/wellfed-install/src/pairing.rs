//! Becoming a feeder's owner: Bluetooth setup over the platform's
//! [`SetupLink`], or the recovery window on the LAN. Both run
//! `wellfed-client`'s setup session and announce the one-time code the
//! owner keys in on the feeder's buttons; the feeder's proof of the code,
//! not the person's answer, decides the pairing.

use std::fmt;
use std::net::SocketAddr;
use std::time::Duration;

use wellfed_client::setup::{Ask, Credentials, Session, Step};
use wellfed_model::setup::{Enrolled, Refusal, Reply, WifiSaved};
use wellfed_session::setup_code::Code;

use crate::Error;
use crate::platform::{Announcement, Keys, Prompter, SetupLink};
use crate::session::random;

/// The feeder answers each handshake message at once.
const HANDSHAKE_WAIT: Duration = Duration::from_secs(10);
/// Beyond the feeder's own confirmation window, for the reply to arrive.
const CONFIRM_SLACK: Duration = Duration::from_secs(10);

/// What the feeder said when setup ended well.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    pub enrolled: Enrolled,
    pub wifi: WifiSaved,
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let enrolled = match self.enrolled {
            Enrolled::New => "enrolled as an owner",
            Enrolled::Already => "already enrolled",
        };
        let wifi = match self.wifi {
            WifiSaved::Saved => "Wi-Fi saved for its next join",
            WifiSaved::Unchanged => "Wi-Fi unchanged",
        };
        write!(f, "{enrolled}, {wifi}")
    }
}

/// Why the feeder refused a setup, as the owner reads it.
pub(crate) fn refusal_text(refusal: Refusal) -> &'static str {
    match refusal {
        Refusal::Busy => "another setup is waiting for its code",
        Refusal::Unconfirmed => "the code was not keyed in in time",
        Refusal::WrongCode => {
            "the code keyed in does not match: mistyped, or another device is relaying"
        }
        Refusal::Full => "every client slot is taken; revoke one first",
        Refusal::NotWritable => "its store cannot be written now",
        Refusal::Invalid => "it rejected the label or the Wi-Fi credentials",
    }
}

/// A setup session for `keys`, giving the feeder `wifi` when set, with a
/// fresh code.
fn session(keys: &impl Keys, wifi: Option<Credentials>) -> Result<Session, Error> {
    let ask = Ask {
        label: keys.label().to_string(),
        wifi,
    };
    let ephemeral: [u8; 32] = random()?;
    let seed: [u8; 32] = random()?;
    let bits: [u8; 2] = random()?;
    let code = Code::from_bits(u16::from_le_bytes(bits));
    Ok(Session::new(keys.keypair(), ephemeral, ask, code, seed)?)
}

/// What a reply means for the exchange: the code to key in, the end, or a
/// refusal.
enum Next {
    EnterCode(u16),
    Done(Outcome),
}

fn next(reply: Reply) -> Result<Next, Error> {
    match reply {
        Reply::EnterCode { seconds } => Ok(Next::EnterCode(seconds)),
        Reply::Done { enrolled, wifi } => Ok(Next::Done(Outcome { enrolled, wifi })),
        Reply::Refused(refusal) => Err(Error::SetupRefused(refusal)),
    }
}

/// Enroll `keys` as an owner over `link` to a feeder in setup mode, giving
/// it `wifi` when set, the owner keying in the code `prompter` announces;
/// the feeder's identity, remembered by `keys`, and what the feeder said.
/// The link is closed either way.
pub async fn over_link(
    keys: &mut impl Keys,
    wifi: Option<Credentials>,
    mut link: impl SetupLink,
    prompter: &mut impl Prompter,
) -> Result<([u8; 32], Outcome), Error> {
    let mut session = session(keys, wifi)?;
    let result = exchange(&mut session, &mut link, prompter).await;
    link.close().await;
    let outcome = result?;

    let identity = session.feeder_key().ok_or(Error::NoProof)?;
    keys.remember(&identity).map_err(Error::Platform)?;
    Ok((identity, outcome))
}

/// Run the session over `link` to its end.
async fn exchange(
    session: &mut Session,
    link: &mut impl SetupLink,
    prompter: &mut impl Prompter,
) -> Result<Outcome, Error> {
    link.send(&session.start()?)
        .await
        .map_err(Error::Platform)?;
    let mut wait = HANDSHAKE_WAIT;
    loop {
        let message = link.receive(wait).await.map_err(Error::Platform)?;
        match session.receive(&message)? {
            Step::Send(bytes) => link.send(&bytes).await.map_err(Error::Platform)?,
            Step::Reply(reply) => match next(reply)? {
                Next::EnterCode(seconds) => {
                    show_code(prompter, session.code(), seconds).await?;
                    wait = Duration::from_secs(seconds.into()) + CONFIRM_SLACK;
                }
                Next::Done(outcome) => return Ok(outcome),
            },
        }
    }
}

/// Announce the code to key in. The feeder already waits for it when it
/// asks, so the announcement goes at once and the run waits only for the
/// feeder's outcome, within its window.
async fn show_code(prompter: &mut impl Prompter, code: Code, seconds: u16) -> Result<(), Error> {
    prompter
        .announce(Announcement::SetupCode { code, seconds })
        .await
        .map_err(Error::ShowCode)
}

/// Enroll `keys` as an owner through the feeder's recovery window at
/// `address`, the owner keying in the code `prompter` announces; the
/// feeder's identity, remembered by `keys`, and what the feeder said.
pub async fn recover(
    keys: &mut impl Keys,
    address: SocketAddr,
    prompter: &mut impl Prompter,
) -> Result<([u8; 32], Outcome), Error> {
    let session = session(keys, None)?;
    let mut recovery = wellfed_connect::Recovery::open(address, session)
        .await
        .map_err(|source| Error::Connect { address, source })?;

    let mut wait = HANDSHAKE_WAIT;
    let outcome = loop {
        let reply = match recovery.reply(wait).await {
            Err(wellfed_connect::Error::Closed) => return Err(Error::RecoveryClosed),
            Err(wellfed_connect::Error::Timeout) => return Err(Error::RecoveryTimeout),
            other => other?,
        };
        match next(reply)? {
            Next::EnterCode(seconds) => {
                show_code(prompter, recovery.session().code(), seconds).await?;
                wait = Duration::from_secs(seconds.into()) + CONFIRM_SLACK;
            }
            Next::Done(outcome) => break outcome,
        }
    };

    let identity = recovery.session().feeder_key().ok_or(Error::NoProof)?;
    keys.remember(&identity).map_err(Error::Platform)?;
    Ok((identity, outcome))
}
