//! What a run says to logcat: each step and each ask, and each answer, so
//! that the gaps between the lines show what a stalled run waits for. Read
//! with `adb logcat | grep wellfed_app_bridge`.
//!
//! The owner's passphrase, the setup frame, the code on the screen and any
//! key must never reach this log, where any app with log access or a bug
//! report could read it. So every variant is named here rather than falling
//! to a `Debug` of the whole value: one added later fails this build until
//! someone has decided what of it may be written.

use crate::api::install::{InstallAnswer, InstallAsk, InstallEnd, InstallEvent, InstallUpdate};

/// Write `update` to the log, if it says anything.
pub(crate) fn update(update: &InstallUpdate) {
    if let Some(line) = line(update) {
        log::info!("{line}");
    }
}

/// Write the answer to ask `id`.
pub(crate) fn answer(id: u32, answer: &InstallAnswer) {
    log::info!("{}", answered(id, answer));
}

fn line(update: &InstallUpdate) -> Option<String> {
    match update {
        InstallUpdate::Event { event } => event_line(event),
        InstallUpdate::Ask { id, ask } => Some(format!("ask {id}: {}", asked(ask))),
        InstallUpdate::Ended { end } => Some(ended(end)),
    }
}

fn event_line(event: &InstallEvent) -> Option<String> {
    match event {
        // Many a second, and the steps either side say the same.
        InstallEvent::StockProgress { .. }
        | InstallEvent::BackupRead { .. }
        | InstallEvent::Written { .. } => None,

        InstallEvent::Code { seconds, .. } => {
            Some(format!("a code is on the screen for {seconds} s"))
        }
        InstallEvent::Paired {
            address, rebooting, ..
        } => Some(format!(
            "paired, answering at {address:?}, restarting: {rebooting}"
        )),

        // Status and addresses, with nothing of the owner's in them.
        InstallEvent::DnsChecked { .. }
        | InstallEvent::StockServerStarted
        | InstallEvent::AwaitingStock
        | InstallEvent::Waiting { .. }
        | InstallEvent::VersionLost
        | InstallEvent::StockServerEnded { .. }
        | InstallEvent::Requesting { .. }
        | InstallEvent::StockRestarting
        | InstallEvent::KeyKnown
        | InstallEvent::SetupStarting
        | InstallEvent::NoSetupFeeder { .. }
        | InstallEvent::PressDispense
        | InstallEvent::Joined { .. }
        | InstallEvent::ReplacementAnswers { .. }
        | InstallEvent::ReadingBackup { .. }
        | InstallEvent::BackupKept { .. }
        | InstallEvent::StockPlans { .. }
        | InstallEvent::WithoutStockPlans
        | InstallEvent::Promoting
        | InstallEvent::SessionDropped
        | InstallEvent::Activated
        | InstallEvent::PuttingPlans
        | InstallEvent::Installed { .. }
        | InstallEvent::Plans { .. }
        | InstallEvent::Note { .. } => Some(format!("{event:?}")),
    }
}

fn asked(ask: &InstallAsk) -> String {
    match ask {
        // These pieces are the setup exchange, the passphrase among them.
        InstallAsk::WriteLink { pieces } => format!("write {} pieces to the link", pieces.len()),

        InstallAsk::SetOverride { .. }
        | InstallAsk::RestartFeeder
        | InstallAsk::EraseStockPlans
        | InstallAsk::JoinFeederNetwork
        | InstallAsk::LeaveFeederNetwork
        | InstallAsk::RecognizeFeeder { .. }
        | InstallAsk::OpenRecoveryWindow
        | InstallAsk::OpenLink
        | InstallAsk::CloseLink
        | InstallAsk::Locate { .. } => format!("{ask:?}"),
    }
}

fn answered(id: u32, answer: &InstallAnswer) -> String {
    match answer {
        InstallAnswer::Done
        | InstallAnswer::Yes { .. }
        | InstallAnswer::Located { .. }
        | InstallAnswer::Failed { .. } => format!("answer {id}: {answer:?}"),
    }
}

fn ended(end: &InstallEnd) -> String {
    match end {
        InstallEnd::Done | InstallEnd::Stopped { .. } => format!("ended: {end:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The setup exchange goes out as these pieces, and the code stands for
    /// the panel presses: neither may reach the log.
    #[test]
    fn secrets_stay_out_of_the_log() {
        let write = line(&InstallUpdate::Ask {
            id: 7,
            ask: InstallAsk::WriteLink {
                pieces: vec![vec![0xde, 0xad], vec![0xbe, 0xef]],
            },
        })
        .unwrap();
        assert_eq!(write, "ask 7: write 2 pieces to the link");

        let code = line(&InstallUpdate::Event {
            event: InstallEvent::Code {
                code: vec![1, 2, 3, 4],
                seconds: 60,
            },
        })
        .unwrap();
        assert_eq!(code, "a code is on the screen for 60 s");
    }

    /// A wait reads as an ask and, later, its answer.
    #[test]
    fn a_wait_shows_as_an_ask_and_its_answer() {
        let ask = line(&InstallUpdate::Ask {
            id: 3,
            ask: InstallAsk::OpenLink,
        })
        .unwrap();
        assert_eq!(ask, "ask 3: OpenLink");
        assert_eq!(answered(3, &InstallAnswer::Done), "answer 3: Done");
    }

    /// Progress is left out, as it comes many times a second.
    #[test]
    fn progress_stays_out_of_the_log() {
        let written = InstallUpdate::Event {
            event: InstallEvent::Written {
                written: 1,
                length: 2,
            },
        };
        assert_eq!(line(&written), None);
    }
}
