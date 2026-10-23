//! The QA firmware's `PLQA1` UART records (`src/firmware/wellfed/qa`): the
//! firmware prints them, `wellfed-audit qa` grades a finished run, and the
//! bench page follows a run while the operator answers its prompts.
//!
//! Parsing is exact. A record with an unknown word, a missing field or an
//! extra one reads as `None`, so the audit can refuse it rather than guess.

use std::fmt;

use serde::Serialize;

/// The checks, in the order the firmware runs them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Check {
    Optics,
    Power,
    Buttons,
    Indicators,
    Speaker,
}

impl Check {
    pub const ALL: [Self; 5] = [
        Self::Optics,
        Self::Power,
        Self::Buttons,
        Self::Indicators,
        Self::Speaker,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Optics => "optics",
            Self::Power => "power",
            Self::Buttons => "buttons",
            Self::Indicators => "indicators",
            Self::Speaker => "speaker",
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }

    fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|check| check.as_str() == text)
    }
}

impl fmt::Display for Check {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One check's verdict as the result line prints it; `none` for a check
/// that never ran to a verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Pass,
    Fail,
    None,
}

impl Verdict {
    fn parse(text: &str) -> Option<Self> {
        match text {
            "pass" => Some(Self::Pass),
            "fail" => Some(Self::Fail),
            "none" => Some(Self::None),
            _ => None,
        }
    }
}

/// What the operator is asked to do. Every look and listen prompt is
/// answered with Dispense for yes and Lock for no.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prompt {
    /// Press this button (`reset`, `mic`, `dispense`, `lock`).
    Press(String),
    /// Say whether the indicator on this pin (`PA0`) lit.
    Look(String),
    /// Say whether this tune (`test`) was heard.
    Listen(String),
}

/// A guided item: the button, indicator or tune a prompt asked about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Press(String),
    Led(String),
    Tune(String),
}

impl fmt::Display for Item {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Press(button) => write!(f, "press={button}"),
            Self::Led(pin) => write!(f, "led={pin}"),
            Self::Tune(tune) => write!(f, "tune={tune}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Record {
    Start(Check),
    Prompt(Prompt),
    /// A guided item's outcome; `failure` is the firmware's reason
    /// (`timeout`, `wrong`, `no`, `muted`…) when it failed.
    Item {
        check: Check,
        item: Item,
        failure: Option<String>,
    },
    Verdict {
        check: Check,
        passed: bool,
    },
    /// The last record: the overall status and each check's verdict, in
    /// [`Check::ALL`] order.
    Result {
        passed: bool,
        verdicts: [Verdict; 5],
    },
}

impl Record {
    /// One trimmed line, `PLQA1 ` prefix included.
    pub fn parse(line: &str) -> Option<Self> {
        let words: Vec<&str> = line.strip_prefix("PLQA1 ")?.split(' ').collect();
        match words.as_slice() {
            ["start", stage] => Some(Self::Start(stage_field(stage)?)),
            ["prompt", press] => Some(Self::Prompt(Prompt::Press(lower(field(press, "press")?)?))),
            ["prompt", what, "yes=dispense", "no=lock"] => {
                if let Some(pin) = field(what, "look") {
                    Some(Self::Prompt(Prompt::Look(pin_name(pin)?)))
                } else {
                    Some(Self::Prompt(Prompt::Listen(lower(field(what, "listen")?)?)))
                }
            }
            ["item", stage, item, status @ ..] => {
                let failure = match status {
                    ["status=pass"] => None,
                    ["status=fail", reason] => Some(lower(field(reason, "reason")?)?),
                    _ => return None,
                };
                Some(Self::Item {
                    check: stage_field(stage)?,
                    item: item_field(item)?,
                    failure,
                })
            }
            ["check", stage, status] => Some(Self::Verdict {
                check: stage_field(stage)?,
                passed: pass_fail(field(status, "status")?)?,
            }),
            ["result", status, checks @ ..] => {
                let passed = pass_fail(field(status, "status")?)?;
                if checks.len() != Check::ALL.len() {
                    return None;
                }
                let mut verdicts = [Verdict::None; 5];
                for ((verdict, check), word) in verdicts.iter_mut().zip(Check::ALL).zip(checks) {
                    *verdict = Verdict::parse(field(word, check.as_str())?)?;
                }
                Some(Self::Result { passed, verdicts })
            }
            _ => None,
        }
    }
}

/// `key=value`'s value, if the key matches and the value is not empty.
fn field<'a>(word: &'a str, key: &str) -> Option<&'a str> {
    word.strip_prefix(key)?
        .strip_prefix('=')
        .filter(|value| !value.is_empty())
}

fn stage_field(word: &str) -> Option<Check> {
    Check::parse(field(word, "stage")?)
}

fn pass_fail(text: &str) -> Option<bool> {
    match text {
        "pass" => Some(true),
        "fail" => Some(false),
        _ => None,
    }
}

fn lower(text: &str) -> Option<String> {
    text.bytes()
        .all(|b| b.is_ascii_lowercase())
        .then(|| text.to_string())
}

/// A GPIO pin name such as `PA0` or `PB22`.
fn pin_name(text: &str) -> Option<String> {
    let number = text
        .strip_prefix("PA")
        .or_else(|| text.strip_prefix("PB"))?;
    (!number.is_empty() && number.bytes().all(|b| b.is_ascii_digit())).then(|| text.to_string())
}

fn item_field(word: &str) -> Option<Item> {
    let (key, value) = word.split_once('=')?;
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    let value = value.to_string();
    match key {
        "press" => Some(Item::Press(value)),
        "led" => Some(Item::Led(value)),
        "tune" => Some(Item::Tune(value)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_every_record_the_firmware_prints() {
        let cases = [
            ("PLQA1 start stage=optics", Record::Start(Check::Optics)),
            (
                "PLQA1 prompt press=reset",
                Record::Prompt(Prompt::Press("reset".into())),
            ),
            (
                "PLQA1 prompt look=PB22 yes=dispense no=lock",
                Record::Prompt(Prompt::Look("PB22".into())),
            ),
            (
                "PLQA1 prompt listen=test yes=dispense no=lock",
                Record::Prompt(Prompt::Listen("test".into())),
            ),
            (
                "PLQA1 item stage=buttons press=mic status=pass",
                Record::Item {
                    check: Check::Buttons,
                    item: Item::Press("mic".into()),
                    failure: None,
                },
            ),
            (
                "PLQA1 item stage=indicators led=PA0 status=fail reason=no",
                Record::Item {
                    check: Check::Indicators,
                    item: Item::Led("PA0".into()),
                    failure: Some("no".into()),
                },
            ),
            (
                "PLQA1 check stage=power status=fail",
                Record::Verdict {
                    check: Check::Power,
                    passed: false,
                },
            ),
            (
                "PLQA1 result status=fail optics=pass power=pass buttons=pass indicators=fail speaker=none",
                Record::Result {
                    passed: false,
                    verdicts: [
                        Verdict::Pass,
                        Verdict::Pass,
                        Verdict::Pass,
                        Verdict::Fail,
                        Verdict::None,
                    ],
                },
            ),
        ];

        for (line, record) in cases {
            assert_eq!(Record::parse(line), Some(record), "{line}");
        }
    }

    #[test]
    fn anything_else_is_unreadable() {
        for line in [
            "PLQA1 start stage=motor",
            "PLQA1 start stage=optics extra",
            "PLQA1 prompt press=",
            "PLQA1 prompt press=Reset",
            "PLQA1 prompt look=PC1 yes=dispense no=lock",
            "PLQA1 prompt look=PA0 yes=lock no=dispense",
            "PLQA1 item stage=buttons press=mic status=fail",
            "PLQA1 item stage=buttons knob=mic status=pass",
            "PLQA1 check stage=optics status=maybe",
            "PLQA1 result status=pass optics=pass power=pass buttons=pass indicators=pass",
            "PLQA1 result status=pass power=pass optics=pass buttons=pass indicators=pass speaker=pass",
            "PLQA1 result status=pass optics=ok power=pass buttons=pass indicators=pass speaker=pass",
            "PLOPT1 start stage=optics",
        ] {
            assert_eq!(Record::parse(line), None, "{line}");
        }
    }
}
