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
//! Two prompts ask for presses on the feeder's panel inside a window the
//! feeder times itself ([`Prompt::KeySetupCode`], [`Prompt::PressDispense`]).
//! The installer writes them and goes on at once, reading their answer only
//! before it writes its next prompt, so the alternation holds and a slow
//! answer never eats into the feeder's window.
//!
//! A prompt expects one kind of answer ([`Expects`]). An answer of the
//! other kind is refused, never read as "no" or as "go on": a confirmation
//! that the feeder restarted and a yes to "is this your feeder?" mean
//! different things, and neither may stand in for the other.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use wellfed_session::setup_code::{Button, PRESSES};

/// One question the installer stops at until it is answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Prompt {
    /// Stock connected but its reports were lost: a restart sends them again.
    /// The broker is already listening.
    RestartFeeder,
    /// Trust on first use admitted `device`, connecting from `address`:
    /// the owner checks the label and the router's client list before
    /// anything is sent to it.
    RecognizeFeeder { device: String, address: String },
    /// Put stock into setup mode and join its access point.
    SetupMode,
    /// Join this computer back to the owner's network after the frame.
    JoinBack,
    /// Lock, then Dispense, on the replacement's panel opens its recovery
    /// window, through which this computer becomes its owner.
    OpenRecoveryWindow,
    /// Key the one-time setup code in on the feeder's panel, one button at
    /// a time, within the window the feeder opened for it. Answered done
    /// once keyed in; the feeder's proof of the code, not the answer,
    /// decides the pairing.
    KeySetupCode {
        #[cfg_attr(
            feature = "ts",
            ts(type = r#"Array<"reset" | "mic" | "dispense" | "lock">"#)
        )]
        presses: CodePresses,
    },
    /// Press Dispense once on the feeder's panel, within the feeder's 60 s
    /// window, for `purpose`. Answered done once pressed.
    PressDispense { purpose: DispensePurpose },
}

/// What a Dispense press asked for confirms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "kebab-case")]
pub enum DispensePurpose {
    /// The installer's update-signing key, which the feeder admits only
    /// with a press at its panel.
    EnrollUpdateKey,
}

/// The presses of a setup code, in order, as a prompt carries them: on the
/// wire as the buttons' names (`["lock", "dispense", …]`), which the bench
/// server and a person both read. Like `wellfed_session`'s `Code`, it never
/// shows its presses in `Debug`, so an error or a log line that prints a
/// prompt does not carry the code.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct CodePresses([Button; PRESSES]);

impl CodePresses {
    pub const fn new(presses: [Button; PRESSES]) -> Self {
        Self(presses)
    }

    pub const fn buttons(&self) -> &[Button; PRESSES] {
        &self.0
    }
}

impl fmt::Debug for CodePresses {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CodePresses(..)")
    }
}

impl Serialize for CodePresses {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let names: Vec<&str> = self.0.iter().map(|button| button.as_str()).collect();
        names.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for CodePresses {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;

        let names = Vec::<String>::deserialize(deserializer)?;
        let buttons = names
            .iter()
            .map(|name| {
                Button::parse(name)
                    .ok_or_else(|| D::Error::custom(format!("{name:?} is no panel button")))
            })
            .collect::<Result<Vec<Button>, D::Error>>()?;
        let presses: [Button; PRESSES] = buttons.try_into().map_err(|buttons: Vec<Button>| {
            D::Error::custom(format!(
                "a setup code is {PRESSES} presses, not {}",
                buttons.len()
            ))
        })?;
        Ok(Self(presses))
    }
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
            Self::RecognizeFeeder { .. } => Expects::YesNo,
            Self::RestartFeeder
            | Self::SetupMode
            | Self::JoinBack
            | Self::OpenRecoveryWindow
            | Self::KeySetupCode { .. }
            | Self::PressDispense { .. } => Expects::Done,
        }
    }

    /// The prompt's `kind`, as its JSON line spells it: a short name for
    /// a log line or a refusal, where [`Self::text`] is too long.
    pub const fn name(&self) -> &'static str {
        match self {
            Self::RestartFeeder => "restart-feeder",
            Self::RecognizeFeeder { .. } => "recognize-feeder",
            Self::SetupMode => "setup-mode",
            Self::JoinBack => "join-back",
            Self::OpenRecoveryWindow => "open-recovery-window",
            Self::KeySetupCode { .. } => "key-setup-code",
            Self::PressDispense { .. } => "press-dispense",
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
            Self::RestartFeeder => "Restart the feeder so stock reports itself again: unplug \
                                    its adapter for about 10 s (take its batteries out too, if \
                                    fitted) and plug it back in."
                .to_string(),
            Self::RecognizeFeeder { device, address } => format!(
                "The broker trusted feeder {device}, connecting from {address}. Is this your \
                 feeder?"
            ),
            Self::SetupMode => "Put stock into setup mode: unlock its panel (hold Lock about 5 s \
                                if the lock light is on), then hold Reset (the rightmost button) \
                                about 5 s, which erases its meals, Wi-Fi and settings. Join this computer to the access point the feeder \
                                then opens."
                .to_string(),
            Self::JoinBack => "Join this computer back to your own network.".to_string(),
            Self::OpenRecoveryWindow => "Pair with the replacement: on its panel (unlocked), \
                                         hold Lock, then Dispense, to open the recovery window."
                .to_string(),
            Self::KeySetupCode { presses } => {
                let names: Vec<&str> = presses.0.iter().map(|button| button.as_str()).collect();
                format!(
                    "Key in the setup code on the feeder's panel within its window, one button \
                     at a time: {}. It proves this computer is talking to that feeder, and is \
                     used once. Answer done once it is keyed in.",
                    names.join(", ")
                )
            }
            Self::PressDispense {
                purpose: DispensePurpose::EnrollUpdateKey,
            } => "Press Dispense once on the feeder's panel within 60 s, to enroll the \
                  installer's update key. Answer done once it is pressed."
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

    const CODE: [Button; PRESSES] = [
        Button::Lock,
        Button::Dispense,
        Button::Dispense,
        Button::Mic,
        Button::Reset,
        Button::Lock,
    ];

    fn every() -> [Prompt; 7] {
        [
            Prompt::RestartFeeder,
            Prompt::RecognizeFeeder {
                device: "AF_TEST".into(),
                address: "192.168.1.31".into(),
            },
            Prompt::SetupMode,
            Prompt::JoinBack,
            Prompt::OpenRecoveryWindow,
            Prompt::KeySetupCode {
                presses: CodePresses::new(CODE),
            },
            Prompt::PressDispense {
                purpose: DispensePurpose::EnrollUpdateKey,
            },
        ]
    }

    /// The line the installer writes, and the bench server reads back.
    #[test]
    fn a_prompt_is_one_tagged_json_object_and_round_trips() {
        let line = serde_json::to_string(&every()[0]).unwrap();
        assert_eq!(line, r#"{"kind":"restart-feeder"}"#);
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

    /// The panel's prompts carry what the bench server presses: the code's
    /// buttons by name, in order, and what the Dispense press is for.
    #[test]
    fn the_panel_prompts_name_their_presses() {
        let code = Prompt::KeySetupCode {
            presses: CodePresses::new(CODE),
        };
        assert_eq!(
            serde_json::to_string(&code).unwrap(),
            r#"{"kind":"key-setup-code","presses":["lock","dispense","dispense","mic","reset","lock"]}"#
        );
        assert_eq!(
            serde_json::to_string(&Prompt::PressDispense {
                purpose: DispensePurpose::EnrollUpdateKey
            })
            .unwrap(),
            r#"{"kind":"press-dispense","purpose":"enroll-update-key"}"#
        );
        assert!(
            code.text()
                .contains("lock, dispense, dispense, mic, reset, lock")
        );

        // The code stays out of anything that prints the prompt's Debug.
        let debug = format!("{code:?}");
        assert!(
            !debug.contains("Dispense") && !debug.contains("Mic"),
            "{debug}"
        );

        // Five presses, or a button the panel has not, is no code.
        for line in [
            r#"{"kind":"key-setup-code","presses":["lock","lock","lock","lock","lock"]}"#,
            r#"{"kind":"key-setup-code","presses":["lock","lock","lock","lock","lock","power"]}"#,
            r#"{"kind":"press-dispense","purpose":"feed"}"#,
        ] {
            assert!(serde_json::from_str::<Prompt>(line).is_err(), "{line}");
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
            every()[1].expects(),
            Expects::YesNo,
            "recognizing the feeder is a question"
        );
    }
}
