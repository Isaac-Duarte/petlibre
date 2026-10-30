//! The install's progress as this command line prints it: each
//! [`Event`] as the line (or lines) it has always printed, on standard
//! error, so standard output carries only what `--prompts lines` puts there.

use wellfed_client::install::{DnsCheck, STOCK_VERSION};
use wellfed_client::update_key::Enrolled;
use wellfed_install::Report;
use wellfed_install::event::{Enrollment, Event, PlansResult, Wait};

use crate::keys::fingerprint;

/// Prints each event on standard error.
pub(crate) struct Printer;

impl Report for Printer {
    fn event(&self, event: Event) {
        eprintln!("{}", line(&event));
    }
}

/// The first 12 hex digits of a build identity.
pub(crate) fn short(build: &str) -> &str {
    &build[..build.len().min(12)]
}

/// The words for `event`.
pub(crate) fn line(event: &Event) -> String {
    match event {
        Event::DnsChecked {
            name,
            broker,
            check,
        } => match check {
            DnsCheck::Points => format!("This computer resolves {name} to {broker}, as it should."),
            DnsCheck::Mixed(addresses) => format!(
                "This computer resolves {name} to {addresses:?}: {broker} and others. Stock may \
                 take any of them, so make {broker} the only answer."
            ),
            DnsCheck::Elsewhere(addresses) => format!(
                "This computer resolves {name} to {addresses:?}, not {broker}. If it asks the \
                 same resolver as the feeder, the override is not in place yet."
            ),
            DnsCheck::Unresolved => format!("This computer cannot resolve {name}."),
        },
        Event::AwaitingStock => "Waiting for stock to connect here. If it does not within a few \
                                 minutes, check the override in the resolver your router hands \
                                 out and restart the feeder again; nothing is sent to stock \
                                 until it does."
            .to_string(),
        Event::StockServerStarted => {
            "Started the stock server (its broker trusts the first feeder that connects)."
                .to_string()
        }
        Event::FrameSent {
            answer: Some(answer),
        } => format!(
            "Sent the setup frame once; the feeder answered {}",
            wellfed_formats::hex::encode(answer)
        ),
        Event::FrameSent { answer: None } => {
            "Sent the setup frame once; no answer within 5 s (not resent)".to_string()
        }
        Event::Waiting(wait) => match wait {
            Wait::NoFeeder => {
                "Waiting: no stock feeder has connected to the broker yet".to_string()
            }
            Wait::Unreadable { error } => {
                format!("Waiting: the stock server cannot read its enrollment ({error})")
            }
            Wait::Stock(reason) => format!("Waiting: {reason}"),
        },
        Event::VersionLost => "Stock connected but sent no version: its startup reports came \
                               before the server listened. Restarting it once replays them."
            .to_string(),
        Event::StockServerEnded { error } => {
            format!("The stock server stopped ({error}); nothing more can connect to it.")
        }
        Event::DevicePinned { device, address } => format!(
            "The broker enrolled device {device} from {address}, the device --expect-device \
             pins; no question asked."
        ),
        Event::Requesting { build, request } => format!(
            "Asking stock once to install {} (request {request})",
            short(build)
        ),
        Event::StockProgress(percent) => format!("Stock: {percent}% downloaded"),
        Event::StockRestarting => "Stock went quiet: it restarts into the replacement.".to_string(),
        Event::KeyKnown => "The replacement knows this computer's key already.".to_string(),
        Event::KeyKnownAt { identity, address } => format!(
            "The replacement knows this computer's key already: feeder {} at {address}.",
            fingerprint(identity)
        ),
        Event::Rejoining { identity } => format!(
            "Feeder {} paired with this computer in an earlier run; waiting for it to join the network.",
            fingerprint(identity)
        ),
        Event::KnownKeyRefused { error } => {
            format!("No session with the known feeder key ({error}).")
        }
        Event::Recovered { identity, outcome } => {
            format!("Paired with feeder {} ({outcome}).", fingerprint(identity))
        }
        Event::SetupStarting => "The replacement has no network yet, so it starts Bluetooth \
                                 setup by itself. Keep this computer within Bluetooth range of \
                                 the feeder."
            .to_string(),
        Event::NoSetupFeeder { error } => {
            format!("No feeder in Bluetooth setup mode ({error}).")
        }
        Event::SetUp { identity, outcome } => format!(
            "Paired with feeder {} ({outcome}); it reboots to join the network.",
            fingerprint(identity)
        ),
        Event::Joined { address } => {
            format!("The replacement joined the network and answers at {address}.")
        }
        Event::ReplacementAnswers { build, bank } => format!(
            "The replacement answers: build {} on bank {bank}",
            short(build)
        ),
        Event::TrialClosed => {
            "The stock server closed its record: replacement healthy.".to_string()
        }
        Event::TrialOpen { error } => format!("The stock server kept its record open ({error})."),
        Event::KeyEnrollment { key, outcome } => {
            let id = wellfed_formats::hex::encode(key);
            match outcome {
                Enrollment::Enrolled(Enrolled::Now) => format!("Enrolled update key {id}."),
                Enrollment::Enrolled(Enrolled::Already) => {
                    format!("The replacement already trusts update key {id}; nothing sent.")
                }
                Enrollment::Unanswered => format!(
                    "No answer came to the enrollment of update key {id}; it is not sent again. \
                     Reading the feeder's signers instead."
                ),
            }
        }
        Event::EnrollUnanswered { error } => format!("No answer: {error}"),
        Event::PressNotAnnounced { error } => {
            format!("Could not ask for the Dispense press: {error}")
        }
        Event::SignersUnread { error } => {
            format!("The feeder's clients could not be read ({error}).")
        }
        Event::RestoreSigned {
            version,
            floor,
            key,
        } => format!(
            "Signed stock {STOCK_VERSION} as version {version} (a downgrade at floor {floor}) \
             with update key {}.",
            wellfed_formats::hex::encode(key)
        ),
        Event::Restoring => {
            "Offering stock: bank 1 gives way, and bank 0 writes stock into bank 1.".to_string()
        }
        Event::RestoreKeyKept { error } => {
            format!("The restore's update key file could not be removed: {error}")
        }
        Event::StaleInstallCancelled => {
            "An earlier install was left pending; cancelling it before this offer.".to_string()
        }
        Event::FeederTried { address, answered } => {
            if *answered {
                format!("  The feeder answers at {address}.")
            } else {
                format!("  No answer from the feeder at {address}.")
            }
        }
        Event::Floor(floor) => format!("The feeder's floor is version {floor}."),
        Event::FloorUnread { error } => format!("The feeder's floor could not be read ({error})."),
        Event::Signed {
            build,
            version,
            key,
            path,
        } => format!(
            "Signed build {} as version {version} with update key {}; kept at {}.",
            short(build),
            wellfed_formats::hex::encode(key),
            path.display()
        ),
        Event::GivenBackupKept { path } => {
            format!("Kept the --stock-backup copy at {}.", path.display())
        }
        Event::BackupFound { path } => {
            format!("Using the stock backup kept at {}.", path.display())
        }
        Event::ReadingBank1 { identity } => format!(
            "Reading bank 1 from feeder {} (about 1.3 MB)...",
            fingerprint(identity)
        ),
        Event::Bank1Read(read) => format!("  {read} bytes read"),
        Event::BackupKept { path } => {
            format!("Kept the verified stock backup at {}.", path.display())
        }
        Event::PlansKept { path } => format!("Kept stock's plan sectors at {}.", path.display()),
        Event::StockPlans { import, kept } => {
            let report = wellfed_client::install::plans::report(import);
            let report = report.trim_end_matches('\n');
            match kept {
                None => format!("Stock's plans, as the replacement will run them:\n{report}"),
                Some(path) => format!(
                    "Stock's plans, from the copy kept at {}:\n{report}",
                    path.display()
                ),
            }
        }
        Event::WithoutStockPlans { skipped } => {
            format!("Going on without stock's plans (--without-stock-plans): {skipped}.")
        }
        Event::Promoting => "Promoting: bank 0 writes the replacement into bank 1.".to_string(),
        Event::Written { written, length } => format!("  {written} of {length} bytes written"),
        Event::SessionDropped { error } => {
            format!("  The session dropped ({error}); resuming on a new one.")
        }
        Event::AlreadyActivated => "Bank 1 already runs the build: it was activated.".to_string(),
        Event::Activated => "Activated: the feeder restarts into bank 1.".to_string(),
        Event::PlansUnread { error } => {
            format!("The replacement's plans could not be read ({error}).")
        }
        Event::PuttingPlans => "Setting stock's converted plans on the replacement.".to_string(),
        Event::EarlierPut { base } => format!(
            "An earlier run may have sent stock's plans, against generation {base}: they go \
             again only if the feeder's plans are still at it."
        ),
        Event::Installed { build } => format!(
            "Installed: build {} confirmed on bank 1. This computer is its owner.",
            short(build)
        ),
        Event::Plans(result) => match result {
            PlansResult::Skipped(skipped) => {
                format!("Stock's plans were not imported: {skipped}.")
            }
            PlansResult::NotRead => "Stock's plans were not read by this run, and no earlier run \
                                     kept a copy it has not applied: nothing was set."
                .to_string(),
            PlansResult::Set => "Stock's converted plans are set on the replacement.".to_string(),
            PlansResult::None => "Stock had no plans to set on the replacement.".to_string(),
            PlansResult::NoneConverted => "None of stock's plans could be converted (see the \
                                           report above): nothing was set."
                .to_string(),
        },
        Event::PlansSetAside { path } => {
            format!("Set the applied copy aside at {}.", path.display())
        }
        Event::ResumeRecordKept { error } => {
            format!("warning: the resume record was not removed: {error}")
        }
    }
}
