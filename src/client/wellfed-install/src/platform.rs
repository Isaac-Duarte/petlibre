//! What the platform running an install owns, as the driver reaches it:
//! the person at the feeder ([`Prompter`]), the Bluetooth radio
//! ([`Bluetooth`]), finding a feeder on the network ([`Discovery`]), the
//! client key and the feeders it knows ([`Keys`]), and the progress shown
//! ([`Report`]). The CLI implements them with a terminal (or the bench's
//! JSON lines), BlueZ, mDNS and its key file; the Android app with Dart
//! callbacks, Android's Bluetooth and NSD, and its secure storage.
//!
//! A platform's own failure comes back as a [`PlatformError`], kept whole,
//! so its text reaches the person as the platform wrote it.

use std::net::SocketAddr;
use std::time::Duration;

use wellfed_formats::bench::prompt::{CodePresses, DispensePurpose, Prompt};
use wellfed_session::noise::Keypair;
use wellfed_session::setup_code::Code;

use crate::event::Event;

/// A failure of the platform's own: a prompt nobody can answer, a radio
/// that would not connect, a key that could not be saved.
pub type PlatformError = Box<dyn std::error::Error + Send + Sync>;

/// A press at the panel inside a window the feeder times: shown, never
/// waited for, since the feeder, not the answer, says whether it came.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Announcement {
    /// Key `code` in on the panel within `seconds`.
    SetupCode { code: Code, seconds: u16 },
    /// Press Dispense within the feeder's 60 s window to enroll the update
    /// key whose id is `key`.
    PressDispense { key: [u8; 8] },
}

impl Announcement {
    /// The prompt the bench's JSON lines carry for it.
    pub fn prompt(&self) -> Prompt {
        match self {
            Self::SetupCode { code, .. } => Prompt::KeySetupCode {
                presses: CodePresses::new(*code.presses()),
            },
            Self::PressDispense { .. } => Prompt::PressDispense {
                purpose: DispensePurpose::EnrollUpdateKey,
            },
        }
    }
}

/// The person at the feeder. An answer nobody can give (a closed input, a
/// dismissed screen) is an error, never "no" and never "go on": the
/// install must not carry on as if someone had answered.
#[expect(
    async_fn_in_trait,
    reason = "the run is driven on one thread (`run`'s docs), so no Send bound is asked of a \
              platform's futures"
)]
pub trait Prompter {
    /// Wait for the person to do what `prompt` says.
    async fn done(&mut self, prompt: Prompt) -> Result<(), PlatformError>;

    /// The person's yes or no to `prompt`.
    async fn yes(&mut self, prompt: Prompt) -> Result<bool, PlatformError>;

    /// Show `announcement` and return at once.
    async fn announce(&mut self, announcement: Announcement) -> Result<(), PlatformError>;

    /// Take whatever answer an earlier announcement left unread, before a
    /// request whose window a wait would spend.
    async fn settle(&mut self) -> Result<(), PlatformError>;
}

/// A connected feeder's Bluetooth setup characteristic: whole messages
/// out and in, framed by the link.
#[expect(
    async_fn_in_trait,
    reason = "the run is driven on one thread (`run`'s docs), so no Send bound is asked of a \
              platform's futures"
)]
pub trait SetupLink {
    async fn send(&mut self, message: &[u8]) -> Result<(), PlatformError>;

    /// The next message, waiting at most `wait`.
    async fn receive(&mut self, wait: Duration) -> Result<Vec<u8>, PlatformError>;

    /// Hang up, bounded: a link the feeder already dropped may stall.
    async fn close(self);
}

/// The radio that finds a feeder advertising Bluetooth setup and connects
/// to it. Which feeder (a given address, or the first heard) is the
/// platform's to know.
#[expect(
    async_fn_in_trait,
    reason = "the run is driven on one thread (`run`'s docs), so no Send bound is asked of a \
              platform's futures"
)]
pub trait Bluetooth {
    type Link: SetupLink;

    /// A link to a feeder in setup mode, or why none was found.
    async fn open(&mut self) -> Result<Self::Link, PlatformError>;
}

/// Finding a feeder on the local network by its identity: mDNS on a
/// computer, NSD on Android.
#[expect(
    async_fn_in_trait,
    reason = "the run is driven on one thread (`run`'s docs), so no Send bound is asked of a \
              platform's futures"
)]
pub trait Discovery {
    /// Where the feeder with `identity` advertises now, if it does.
    async fn locate(&self, identity: [u8; 32]) -> Option<SocketAddr>;
}

/// mDNS from this process, as the CLI finds a feeder.
pub struct Mdns;

impl Discovery for Mdns {
    async fn locate(&self, identity: [u8; 32]) -> Option<SocketAddr> {
        // Browsing blocks its thread for the discovery window.
        tokio::task::spawn_blocking(move || {
            wellfed_connect::locate(&identity, None, wellfed_connect::DISCOVERY)
        })
        .await
        .ok()
        .and_then(Result::ok)
    }
}

/// This client's key and the feeders it knows.
pub trait Keys {
    fn keypair(&self) -> &Keypair;

    /// How feeders list this client.
    fn label(&self) -> &str;

    /// The one known feeder whose identity starts with `prefix` (hex), or
    /// the only one known; `None` for none or several.
    fn feeder(&self, prefix: Option<&str>) -> Option<[u8; 32]>;

    /// Where this client last reached the feeder `identity`, if it keeps
    /// that. Only a place to try first: a session there proves the identity.
    fn last_address(&self, _identity: &[u8; 32]) -> Option<SocketAddr> {
        None
    }

    /// Note a feeder that enrolled this key.
    fn remember(&mut self, identity: &[u8; 32]) -> Result<(), PlatformError>;
}

/// Where progress goes: printed by the CLI, drawn by the app.
pub trait Report {
    fn event(&self, event: Event);
}

impl<F: Fn(Event)> Report for F {
    fn event(&self, event: Event) {
        self(event);
    }
}
