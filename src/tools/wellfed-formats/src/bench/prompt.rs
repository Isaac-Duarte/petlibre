//! What `wellfed install` asks the person at the feeder, and what they
//! answer, as values rather than terminal text.
//!
//! With `--prompts lines` the installer writes each [`Prompt`] as one JSON
//! line on its standard output and reads one [`Answer`] line back from its
//! standard input; the bench server runs it that way as a job and shows the
//! question to whoever answers. The words live here, with the type, so the
//! installer's terminal and every reader of the bench show the same ones.
//!
//! An answer line carries no sequence number and does not name its prompt:
//! the installer reads exactly one answer after each prompt it writes, and
//! that strict alternation is the only thing that pairs them. So a reader
//! (the bench server) writes an answer only to the prompt it has read and
//! not yet answered, never one queued ahead for a prompt still to come,
//! and never a second one: an extra line would answer the next prompt.
//!
//! A prompt expects one kind of answer ([`Expects`]). An answer of the
//! other kind is refused, never read as "no" or as "go on": a confirmation
//! that the feeder restarted and a yes to "is this your feeder?" mean
//! different things, and neither may stand in for the other.

use std::fmt;
use std::net::IpAddr;

use serde::{Deserialize, Serialize};

/// One question the installer stops at until it is answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Prompt {
    /// The DNS route: the owner's resolver is to answer stock's broker
    /// `name` with `broker`, this computer.
    SetOverride { name: String, broker: IpAddr },
    /// Stock looks its broker name up again only when it restarts; the
    /// broker is already listening.
    RestartFeeder,
    /// Trust on first use admitted `device`, connecting from `address`:
    /// the owner checks the label and the router's client list before
    /// anything is sent to it.
    RecognizeFeeder { device: String, address: String },
    /// The setup-frame route starts with stock's Reset hold, which erases
    /// its plans before the installer can read them.
    EraseStockPlans,
    /// Put stock into setup mode and join its access point.
    SetupMode,
    /// Join this computer back to the owner's network after the frame.
    JoinBack,
    /// Lock, then Dispense, on the replacement's panel opens its recovery
    /// window, through which this computer becomes its owner.
    OpenRecoveryWindow,
}

/// The kind of answer a prompt takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "kebab-case")]
pub enum Expects {
    /// The person did what the prompt says: [`Answer::Done`].
    Done,
    /// A question: [`Answer::Yes`] or [`Answer::No`].
    YesNo,
}

/// The person's answer to one prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "kebab-case")]
pub enum Answer {
    Done,
    Yes,
    No,
}

impl Prompt {
    /// The kind of answer this prompt takes.
    pub const fn expects(&self) -> Expects {
        match self {
            Self::RecognizeFeeder { .. } | Self::EraseStockPlans => Expects::YesNo,
            Self::SetOverride { .. }
            | Self::RestartFeeder
            | Self::SetupMode
            | Self::JoinBack
            | Self::OpenRecoveryWindow => Expects::Done,
        }
    }

    /// The prompt's `kind`, as its JSON line spells it: a short name for
    /// a log line or a refusal, where [`Self::text`] is too long.
    pub const fn name(&self) -> &'static str {
        match self {
            Self::SetOverride { .. } => "set-override",
            Self::RestartFeeder => "restart-feeder",
            Self::RecognizeFeeder { .. } => "recognize-feeder",
            Self::EraseStockPlans => "erase-stock-plans",
            Self::SetupMode => "setup-mode",
            Self::JoinBack => "join-back",
            Self::OpenRecoveryWindow => "open-recovery-window",
        }
    }

    /// Whether `answer` is the kind this prompt takes.
    pub const fn accepts(&self, answer: Answer) -> bool {
        matches!(
            (self.expects(), answer),
            (Expects::Done, Answer::Done) | (Expects::YesNo, Answer::Yes | Answer::No)
        )
    }

    /// What the person reads, without how to answer it: a terminal adds
    /// "Then press Enter." or "[y/N]", a page its buttons.
    pub fn text(&self) -> String {
        match self {
            Self::SetOverride { name, broker } => format!(
                "Point stock's broker name at this computer. In your router's DNS settings (or a \
                 resolver you run, if your router hands that one out), make {name} resolve to \
                 {broker}. Only that name changes; stock keeps its Wi-Fi, its credentials and its \
                 plans, and nothing is written to it. Remove the override once the install is \
                 done: a feeder rolled back to stock would otherwise keep looking for this \
                 computer. While it stands, any other device that uses the same name reaches \
                 this computer too, and loses its connection to the vendor. Set the override."
            ),
            Self::RestartFeeder => "Restart the feeder so stock looks the name up again: unplug \
                                    its adapter for about 10 s (take its batteries out too, if \
                                    fitted) and plug it back in."
                .to_string(),
            Self::RecognizeFeeder { device, address } => format!(
                "The broker trusted feeder {device}, connecting from {address}. Is this your \
                 feeder?"
            ),
            Self::EraseStockPlans => "The fallback route starts with stock's Reset hold, which \
                                      runs its factory restore: it erases stock's feeding plans, \
                                      Wi-Fi and settings before the installer can read them, so \
                                      no plans are imported. Note your plans in the vendor app \
                                      now, to set them again on the replacement. Where your \
                                      router (or a resolver you run) can override a DNS name, \
                                      stop here and run with --broker-name instead: that keeps \
                                      the plans. Erase stock's plans with the Reset hold and go \
                                      on?"
            .to_string(),
            Self::SetupMode => "Put stock into setup mode: unlock its panel (hold Lock about 5 s \
                                if the lock light is on), then hold Reset (the rightmost button) \
                                about 5 s. Join this computer to the access point the feeder \
                                then opens."
                .to_string(),
            Self::JoinBack => "Join this computer back to your own network.".to_string(),
            Self::OpenRecoveryWindow => "Pair with the replacement: on its panel (unlocked), \
                                         hold Lock, then Dispense, to open the recovery window."
                .to_string(),
        }
    }
}

impl fmt::Display for Prompt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text())
    }
}

impl fmt::Display for Expects {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Done => "done",
            Self::YesNo => "yes or no",
        })
    }
}

impl fmt::Display for Answer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Done => "done",
            Self::Yes => "yes",
            Self::No => "no",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every() -> [Prompt; 7] {
        [
            Prompt::SetOverride {
                name: "broker.example.net".into(),
                broker: "192.168.1.20".parse().unwrap(),
            },
            Prompt::RestartFeeder,
            Prompt::RecognizeFeeder {
                device: "AF_TEST".into(),
                address: "192.168.1.31".into(),
            },
            Prompt::EraseStockPlans,
            Prompt::SetupMode,
            Prompt::JoinBack,
            Prompt::OpenRecoveryWindow,
        ]
    }

    /// The line the installer writes, and the bench server reads back.
    #[test]
    fn a_prompt_is_one_tagged_json_object_and_round_trips() {
        let line = serde_json::to_string(&every()[0]).unwrap();
        assert_eq!(
            line,
            r#"{"kind":"set-override","name":"broker.example.net","broker":"192.168.1.20"}"#
        );
        assert_eq!(
            serde_json::to_string(&Prompt::OpenRecoveryWindow).unwrap(),
            r#"{"kind":"open-recovery-window"}"#
        );

        for prompt in every() {
            let line = serde_json::to_string(&prompt).unwrap();
            assert!(!line.contains('\n'), "{line}");
            let tagged = format!(r#"{{"kind":"{}""#, prompt.name());
            assert!(line.starts_with(&tagged), "{line}");
            assert_eq!(serde_json::from_str::<Prompt>(&line).unwrap(), prompt);
        }
    }

    #[test]
    fn an_answer_is_one_json_string() {
        for (answer, line) in [
            (Answer::Done, r#""done""#),
            (Answer::Yes, r#""yes""#),
            (Answer::No, r#""no""#),
        ] {
            assert_eq!(serde_json::to_string(&answer).unwrap(), line);
            assert_eq!(serde_json::from_str::<Answer>(line).unwrap(), answer);
        }
        assert!(serde_json::from_str::<Answer>(r#""y""#).is_err());
    }

    /// A question takes yes or no, an instruction takes done, and neither
    /// takes the other's.
    #[test]
    fn each_prompt_accepts_only_its_own_kind_of_answer() {
        for prompt in every() {
            let (fits, misfits) = match prompt.expects() {
                Expects::Done => (&[Answer::Done][..], &[Answer::Yes, Answer::No][..]),
                Expects::YesNo => (&[Answer::Yes, Answer::No][..], &[Answer::Done][..]),
            };
            assert!(
                fits.iter().all(|answer| prompt.accepts(*answer)),
                "{prompt:?}"
            );
            assert!(
                !misfits.iter().any(|answer| prompt.accepts(*answer)),
                "{prompt:?}"
            );
        }
        assert_eq!(Prompt::RestartFeeder.expects(), Expects::Done);
        assert_eq!(
            every()[2].expects(),
            Expects::YesNo,
            "recognizing the feeder is a question"
        );
    }
}
