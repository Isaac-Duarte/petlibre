//! The install from stock 1.1.15 to Wellfed, as one tokio driver that the
//! CLI (`wellfed install`) and the Android app both run
//! ([consumer installation]).
//!
//! The steps and every decision between them are `wellfed_client::install`'s;
//! this crate is their IO, the same on every platform:
//! - stock's side is `wellfed-stock`'s server, run in this process
//!   ([`stock`]): its broker trusts the first feeder that connects during
//!   the run, and holds that feeder's CONNACK until the server's session has
//!   subscribed, so stock's connect burst (its version report among it)
//!   reaches the server. Its image server serves the review for the one
//!   request;
//! - stock reaches the broker by DNS: the owner's resolver answers stock's
//!   broker name with this host and the owner restarts the feeder (the run
//!   guides it and looks the name up as advice; stock arriving at the broker
//!   is the check); or by the setup frame: one UDP datagram, sent after the owner
//!   joined this host to the feeder's setup access point;
//! - the replacement is reached over the device API with the platform's
//!   client key. A key the build already knows opens a session at once.
//!   Otherwise the review's provisioning picks the route: a consumer image
//!   starts Bluetooth setup by itself, so the run runs the setup exchange
//!   over the platform's radio (giving it the home Wi-Fi), waits for the
//!   reboot and finds the feeder by discovery or where stock connected
//!   from; a bench image is paired through the LAN recovery window;
//! - the promotion is signed by a key the replacement trusts: a manifest
//!   signed elsewhere, or the build signed here with the owner's update key
//!   at a version above the feeder's floor, the key enrolled first (a
//!   Dispense press) when the image does not build it in. The signed
//!   manifest is kept beside the stock backup, so a resumed run knows the
//!   version it waits for ([`storage`]).
//!
//! What the platform owns comes in through [`platform`]'s traits: the
//! person at the feeder, the Bluetooth radio, discovery, the client key and
//! the progress shown ([`Event`]). The entry point is [`run`]: a [`Plan`]
//! read and checked before the feeder is touched, the [`Platform`], and a
//! stop. Nothing is retried that the steps say must not be, and a stop
//! reports the step it came in and any write whose outcome had not settled
//! ([`track`]).
//!
//! [consumer installation]: ../../../../docs/firmware/consumer-installation.md

#![forbid(unsafe_code)]

pub mod backup;
mod error;
pub mod event;
pub mod listen;
pub mod pairing;
pub mod platform;
pub mod push;
pub mod restore;
mod review;
mod run;
pub mod session;
pub mod stock;
pub mod storage;
pub mod track;
pub mod update_key;

pub use error::Error;
pub use event::Event;
pub use platform::{
    Announcement, Bluetooth, Discovery, Keys, Mdns, PlatformError, Prompter, Report, SetupLink,
};
pub use review::Review;
pub use run::{Plan, Platform, Signer, Start, Unjoined, run};
pub use stock::StockOptions;
pub use track::{Stop, Stopped, Unsettled};
/// What stock's server is built from, which the CLI checks before it
/// dispatches an install.
pub use wellfed_stock::build_info;
/// Whether stock's clock is set even when its own reading is far off, and
/// whether stock's server answers its loopback API.
pub use wellfed_stock::{
    clock::Policy as ClockPolicy,
    server::{Api, Bind},
};
/// Why the server would not send the install request.
pub use wellfed_stock::{controller::Rejection, server::Refusal};

/// `error` and its causes on one line, as a report shows a failure the run
/// went on from.
pub(crate) fn text(error: &(dyn std::error::Error + 'static)) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

#[cfg(test)]
pub(crate) mod tests {
    use wellfed_formats::bench::prompt::Prompt;

    use crate::platform::{Announcement, PlatformError, Prompter};

    /// A prompter with nobody behind it: it records what it was shown, and
    /// any question it puts fails, as a closed input does.
    #[derive(Default)]
    pub(crate) struct Scripted {
        pub(crate) asked: Vec<Prompt>,
        pub(crate) announced: Vec<Announcement>,
    }

    impl Prompter for Scripted {
        async fn done(&mut self, prompt: Prompt) -> Result<(), PlatformError> {
            self.asked.push(prompt);
            Err("nobody answers".into())
        }

        async fn yes(&mut self, prompt: Prompt) -> Result<bool, PlatformError> {
            self.asked.push(prompt);
            Err("nobody answers".into())
        }

        async fn announce(&mut self, announcement: Announcement) -> Result<(), PlatformError> {
            self.announced.push(announcement);
            Ok(())
        }

        async fn settle(&mut self) -> Result<(), PlatformError> {
            Ok(())
        }
    }
}
