//! The QA firmware's guided run in the latest capture, as it arrives: which
//! check is running, what the operator is being asked right now, and each
//! answer's outcome, so the "Now" view can coach a run the owner performs at
//! the feeder.
//!
//! This only follows the records. `wellfed-audit qa` grades a finished run,
//! and the panel never stands in for it.

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use wellfed_formats::qa::{Check, Item, Prompt, Record};
use serde::Serialize;

use crate::status;

/// The capture's tail searched for the newest run. A whole guided run is
/// about 16 KB of UART, other tasks' records included.
const WINDOW_BYTES: u64 = 128 * 1024;
const RUN_START: &[u8] = b"PLQA1 start stage=optics";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
enum QaState {
    Waiting,
    Running,
    Pass,
    Fail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
enum QaAsk {
    /// Press the named button.
    Press,
    /// Answer whether the indicator on the named pin lit.
    Look,
    /// Answer whether the named tune was heard.
    Listen,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
struct QaPromptView {
    ask: QaAsk,
    /// A button (`reset`), a pin (`PA0`) or a tune (`test`).
    target: String,
    /// When the prompt arrived, if the capture's events place it.
    asked: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
struct QaItemView {
    ask: QaAsk,
    target: String,
    passed: bool,
    /// The firmware's reason when it failed: `timeout`, `wrong`, `no`…
    reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
struct QaCheckView {
    #[cfg_attr(
        test,
        ts(type = r#""optics" | "power" | "buttons" | "indicators" | "speaker""#)
    )]
    check: Check,
    state: QaState,
    items: Vec<QaItemView>,
}

/// The newest QA run in the latest capture.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(super) struct QaView {
    /// Root-relative capture directory.
    capture: String,
    /// All five checks, in sequence order.
    checks: Vec<QaCheckView>,
    /// The question waiting for an answer, if any.
    prompt: Option<QaPromptView>,
    /// The result line's overall status, once it has arrived.
    passed: Option<bool>,
    /// Records the parser could not read; the audit would refuse the run.
    unreadable: u32,
}

/// The newest QA run in `directory`'s capture, or `None` if its tail holds none.
pub(super) fn qa_view(root: &Path, directory: &Path) -> Option<QaView> {
    let (window, base) = tail(&directory.join("serial-rx.bin"))?;
    let start = window
        .windows(RUN_START.len())
        .rposition(|w| w == RUN_START)?;

    let mut view = QaView {
        capture: status::relative(root, directory),
        checks: Check::ALL
            .into_iter()
            .map(|check| QaCheckView {
                check,
                state: QaState::Waiting,
                items: Vec::new(),
            })
            .collect(),
        prompt: None,
        passed: None,
        unreadable: 0,
    };
    let mut prompt_offset = None;

    let mut offset = start;
    for line in window[start..].split_inclusive(|b| *b == b'\n') {
        let at = offset;
        offset += line.len();

        // Lines arrive as "\rPLQA1 …\r" (the console adds "\r" after "\n").
        let line = line.trim_ascii();
        if !line.starts_with(b"PLQA1 ") {
            continue;
        }
        let Some(record) = std::str::from_utf8(line).ok().and_then(Record::parse) else {
            view.unreadable = view.unreadable.saturating_add(1);
            continue;
        };
        if let Record::Prompt(_) = &record {
            prompt_offset = Some(at);
        }
        view.apply(record);
    }

    // Placing a prompt in time means reading the events log, so only the
    // pending one is placed.
    if let (Some(prompt), Some(at)) = (view.prompt.as_mut(), prompt_offset) {
        let events = fs::read_to_string(directory.join("serial-events.jsonl")).ok();
        prompt.asked = events.and_then(|events| {
            let offset = base + u64::try_from(at).ok()?;
            crate::commands::ota::arrival(&events, offset)
        });
    }
    Some(view)
}

impl QaView {
    fn apply(&mut self, record: Record) {
        match record {
            Record::Start(check) => {
                self.checks[check.index()].state = QaState::Running;
                self.prompt = None;
            }
            Record::Prompt(prompt) => {
                let (ask, target) = match prompt {
                    Prompt::Press(button) => (QaAsk::Press, button),
                    Prompt::Look(pin) => (QaAsk::Look, pin),
                    Prompt::Listen(tune) => (QaAsk::Listen, tune),
                };
                self.prompt = Some(QaPromptView {
                    ask,
                    target,
                    asked: None,
                });
            }
            Record::Item {
                check,
                item,
                failure,
            } => {
                let (ask, target) = match item {
                    Item::Press(button) => (QaAsk::Press, button),
                    Item::Led(pin) => (QaAsk::Look, pin),
                    Item::Tune(tune) => (QaAsk::Listen, tune),
                };
                self.checks[check.index()].items.push(QaItemView {
                    ask,
                    target,
                    passed: failure.is_none(),
                    reason: failure,
                });
                self.prompt = None;
            }
            Record::Verdict { check, passed } => {
                self.checks[check.index()].state =
                    if passed { QaState::Pass } else { QaState::Fail };
                self.prompt = None;
            }
            Record::Result { passed, .. } => {
                self.passed = Some(passed);
                self.prompt = None;
            }
        }
    }
}

/// The last [`WINDOW_BYTES`] of `file`, and the file offset they start at.
fn tail(file: &Path) -> Option<(Vec<u8>, u64)> {
    let mut opened = fs::File::open(file).ok()?;
    let size = opened.metadata().ok()?.len();
    let base = size.saturating_sub(WINDOW_BYTES);
    opened.seek(SeekFrom::Start(base)).ok()?;

    let mut data = Vec::new();
    opened.read_to_end(&mut data).ok()?;
    Some((data, base))
}

/// This module's page types, for [`super::bindings`].
#[cfg(test)]
pub(super) fn bindings(cfg: &ts_rs::Config) -> Vec<String> {
    use crate::ui::bindings::decl;
    vec![
        decl::<QaState>(cfg),
        decl::<QaAsk>(cfg),
        decl::<QaPromptView>(cfg),
        decl::<QaItemView>(cfg),
        decl::<QaCheckView>(cfg),
        decl::<QaView>(cfg),
    ]
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::config::CAPTURES;
    use crate::ui::views::tests::{json, sources};

    /// Records as the console frames them, with another task's line between.
    fn wire(lines: &[&str]) -> Vec<u8> {
        lines
            .iter()
            .flat_map(|line| format!("{line}\r\n\r").into_bytes())
            .collect()
    }

    fn capture(root: &Path, rx: &[u8]) -> std::path::PathBuf {
        let dir = root.join(CAPTURES).join("qa");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("serial-rx.bin"), rx).unwrap();
        dir
    }

    #[test]
    fn a_capture_without_qa_records_has_no_view() {
        let (_tmp, sources) = sources();
        let dir = capture(&sources.root, &wire(&["PLFW1 start build=ab bank=1"]));

        assert!(qa_view(&sources.root, &dir).is_none());
    }

    #[test]
    fn a_pending_prompt_follows_the_run() {
        let (_tmp, sources) = sources();
        let dir = capture(
            &sources.root,
            &wire(&[
                "PLQA1 start stage=optics",
                "PLQA1 check stage=optics status=pass",
                "PLQA1 start stage=power",
                "PLQA1 check stage=power status=pass",
                "PLQA1 start stage=buttons",
                "PLQA1 prompt press=reset",
                "PLBTN1 change n=0 ms=10 from=f0 to=e0 edges=1 bounce_ms=0 settled=1 stopped=1",
                "PLQA1 item stage=buttons press=reset status=pass",
                "PLQA1 prompt press=mic",
            ]),
        );

        let view = json(qa_view(&sources.root, &dir).unwrap());
        assert_eq!(view["capture"], "private/captures/qa");
        assert_eq!(view["checks"][0]["state"], "pass");
        assert_eq!(view["checks"][2]["check"], "buttons");
        assert_eq!(view["checks"][2]["state"], "running");
        assert_eq!(
            view["checks"][2]["items"],
            json!([{"ask": "press", "target": "reset", "passed": true, "reason": null}])
        );
        assert_eq!(view["checks"][3]["state"], "waiting");
        assert_eq!(
            view["prompt"],
            json!({"ask": "press", "target": "mic", "asked": null})
        );
        assert!(view["passed"].is_null());
    }

    #[test]
    fn the_newest_run_is_shown_and_its_result_ends_the_prompts() {
        let (_tmp, sources) = sources();
        let dir = capture(
            &sources.root,
            &wire(&[
                "PLQA1 start stage=optics",
                "PLQA1 check stage=optics status=fail",
                "PLFW1 start build=ab bank=1",
                "PLQA1 start stage=optics",
                "PLQA1 check stage=optics status=pass",
                "PLQA1 start stage=indicators",
                "PLQA1 prompt look=PA0 yes=dispense no=lock",
                "PLQA1 item stage=indicators led=PA0 status=fail reason=no",
                "PLQA1 check stage=indicators status=fail",
                "PLQA1 strange",
                "PLQA1 result status=fail optics=pass power=pass buttons=pass indicators=fail speaker=pass",
            ]),
        );

        let view = json(qa_view(&sources.root, &dir).unwrap());
        assert_eq!(view["checks"][0]["state"], "pass");
        assert_eq!(
            view["checks"][3]["items"][0],
            json!({"ask": "look", "target": "PA0", "passed": false, "reason": "no"})
        );
        assert!(view["prompt"].is_null());
        assert_eq!(view["passed"], false);
        assert_eq!(view["unreadable"], 1);
    }
}
