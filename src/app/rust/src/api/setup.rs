//! Becoming a feeder's owner: BLE setup (Lock then Mic held) and LAN
//! recovery (Lock then Dispense held). Both run `wellfed_client::setup`'s
//! session and show the same 6-press code; only the carrier differs.
//!
//! BLE is Dart's: it writes [`SetupDriver::start`]'s pieces, then feeds
//! each notification to [`SetupDriver::notified`] and writes what comes
//! back, as `wellfed setup` does through BlueZ. Recovery runs here over
//! `wellfed_connect::Recovery`.

use std::sync::Mutex;
use std::time::Duration;

use flutter_rust_bridge::frb;
use tokio::sync::Notify;
use wellfed_client::setup::{
    Ask, CONTROL, Credentials, Deframer, SERVICE, Session, Step, frame, panel_position,
};
use wellfed_model::setup::Reply;
use wellfed_session::setup_code::Code;

use crate::api::error::{BridgeError, ErrorKind};
use crate::api::keys::{ClientKey, random};
use crate::api::model::{Enrolled, SetupRefusal, WifiSaved};
use crate::frb_generated::StreamSink;

/// Writes fit the smallest ATT payload, so they never depend on the MTU
/// exchange having happened.
const PIECE: usize = 20;
/// The feeder answers each handshake message at once.
const HANDSHAKE_WAIT: Duration = Duration::from_secs(10);
/// Beyond the feeder's own code window, for its reply to arrive.
const CONFIRM_SLACK: Duration = Duration::from_secs(10);

/// The setup service a feeder in setup mode advertises.
#[frb(sync)]
pub fn setup_service() -> String {
    uuid(SERVICE)
}

/// The control characteristic every setup message travels on.
#[frb(sync)]
pub fn setup_control() -> String {
    uuid(CONTROL)
}

fn uuid(value: u128) -> String {
    let hex = format!("{value:032x}");
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

pub struct WifiCredentials {
    pub ssid: String,
    pub passphrase: String,
}

/// What the screen does next.
pub enum SetupStep {
    /// Write these pieces to the control characteristic, in order, each
    /// with a write request.
    Write { pieces: Vec<Vec<u8>> },
    /// Show the code; the feeder takes presses for `seconds`. Wait that
    /// long plus 10 s for the next message.
    EnterCode { seconds: u16 },
    /// The phone is an owner. `feeder` is the feeder's identity key, which
    /// it proved by the code.
    Done {
        enrolled: Enrolled,
        wifi: WifiSaved,
        feeder: Vec<u8>,
    },
    /// The feeder said no; the link closes and setup mode stays on.
    Refused { refusal: SetupRefusal },
}

/// The fresh secrets one session needs, and its code.
fn session(
    key: &ClientKey,
    label: String,
    wifi: Option<WifiCredentials>,
) -> Result<Session, BridgeError> {
    let wifi = match wifi {
        Some(wifi) => Some(Credentials::new(wifi.ssid.into_bytes(), wifi.passphrase)?),
        None => None,
    };
    let bits: [u8; 2] = random()?;
    let session = Session::new(
        key.keypair(),
        random()?,
        Ask { label, wifi },
        Code::from_bits(u16::from_le_bytes(bits)),
        random()?,
    )?;
    Ok(session)
}

/// The code as panel positions, 1 (Lock) to 4 (Reset) from the left.
fn positions(session: &Session) -> Vec<u8> {
    session
        .code()
        .presses()
        .iter()
        .map(|&button| panel_position(button))
        .collect()
}

/// The screen's step for a reply. `Done` carries the key the feeder proved.
fn reply_step(session: &Session, reply: Reply) -> Result<SetupStep, BridgeError> {
    match reply {
        Reply::EnterCode { seconds } => Ok(SetupStep::EnterCode { seconds }),
        Reply::Refused(refusal) => Ok(SetupStep::Refused { refusal }),
        Reply::Done { enrolled, wifi } => {
            let Some(feeder) = session.feeder_key() else {
                return Err(BridgeError::new(
                    ErrorKind::CodeMismatch,
                    "the feeder never proved the code",
                ));
            };
            Ok(SetupStep::Done {
                enrolled,
                wifi,
                feeder: feeder.to_vec(),
            })
        }
    }
}

/// One BLE setup session.
#[frb(opaque)]
pub struct SetupDriver {
    session: Session,
    deframer: Deframer,
}

impl SetupDriver {
    /// `label` is how the feeder lists this phone (1-16 bytes); `wifi` is
    /// the network for its next join, or `None` to keep its own.
    #[frb(sync)]
    pub fn new(
        key: &ClientKey,
        label: String,
        wifi: Option<WifiCredentials>,
    ) -> Result<Self, BridgeError> {
        Ok(Self {
            session: session(key, label, wifi)?,
            deframer: Deframer::default(),
        })
    }

    #[frb(sync)]
    pub fn code(&self) -> Vec<u8> {
        positions(&self.session)
    }

    /// The first handshake message, as pieces to write once notifications
    /// are on.
    #[frb(sync)]
    pub fn start(&mut self) -> Result<Vec<Vec<u8>>, BridgeError> {
        pieces(&self.session.start()?)
    }

    /// One notification's bytes: the steps for every message it completed
    /// (none while a frame is still split).
    #[frb(sync)]
    pub fn notified(&mut self, bytes: Vec<u8>) -> Result<Vec<SetupStep>, BridgeError> {
        self.deframer.push(&bytes);
        let mut steps = Vec::new();
        while let Some(message) = self.deframer.next_message()? {
            let step = match self.session.receive(&message)? {
                Step::Send(bytes) => SetupStep::Write {
                    pieces: pieces(&bytes)?,
                },
                Step::Reply(reply) => reply_step(&self.session, reply)?,
            };
            steps.push(step);
        }
        Ok(steps)
    }
}

fn pieces(message: &[u8]) -> Result<Vec<Vec<u8>>, BridgeError> {
    Ok(frame(message)?.chunks(PIECE).map(<[u8]>::to_vec).collect())
}

/// LAN recovery: the code is known before connecting, so the screen can
/// show the instructions while the owner opens the window.
#[frb(opaque)]
pub struct Recovery {
    session: Mutex<Option<Session>>,
    code: Vec<u8>,
    closed: Notify,
}

impl Recovery {
    #[frb(sync)]
    pub fn new(key: &ClientKey, label: String) -> Result<Self, BridgeError> {
        let session = session(key, label, None)?;
        Ok(Self {
            code: positions(&session),
            session: Mutex::new(Some(session)),
            closed: Notify::new(),
        })
    }

    #[frb(sync)]
    pub fn code(&self) -> Vec<u8> {
        self.code.clone()
    }

    /// Run the exchange with the feeder at `address` (`ip` or `ip:port`),
    /// whose recovery window is open, reporting each reply. Runs once.
    /// Errors arrive on the stream: Dart holds only the stream, not this
    /// call's own result.
    pub async fn run(&self, address: String, steps: StreamSink<SetupStep>) {
        let session = self
            .session
            .lock()
            .ok()
            .and_then(|mut session| session.take());
        let result = match session {
            Some(session) => {
                recover(session, &address, &self.closed, |step| {
                    steps.add(step).is_ok()
                })
                .await
            }
            None => Err(BridgeError::new(
                ErrorKind::Ended,
                "this recovery already ran",
            )),
        };
        if let Err(error) = result {
            // Nobody listening means nobody to tell.
            let _ = steps.add_error(error);
        }
    }

    /// Hang up now: the feeder's window stays open for the next try rather
    /// than held by a screen that went away.
    #[frb(sync)]
    pub fn close(&self) {
        // A permit is kept, so a close before `run` waits ends it at once.
        self.closed.notify_one();
    }
}

/// The recovery exchange, reporting each reply to `step` until the last,
/// until `step` returns `false` because the screen went away, or until
/// `closed` is notified; dropping the connection hangs up.
#[frb(ignore)]
pub async fn recover(
    session: Session,
    address: &str,
    closed: &Notify,
    mut step: impl FnMut(SetupStep) -> bool,
) -> Result<(), BridgeError> {
    let address = wellfed_connect::parse_address(address)?;
    let mut recovery = wellfed_connect::Recovery::open(address, session).await?;

    let mut wait = HANDSHAKE_WAIT;
    loop {
        let reply = tokio::select! {
            reply = recovery.reply(wait) => reply?,
            () = closed.notified() => return Ok(()),
        };
        let next = reply_step(recovery.session(), reply)?;
        let last = !matches!(next, SetupStep::EnterCode { .. });
        if let SetupStep::EnterCode { seconds } = next {
            wait = Duration::from_secs(seconds.into()) + CONFIRM_SLACK;
        }
        if !step(next) || last {
            return Ok(());
        }
    }
}
