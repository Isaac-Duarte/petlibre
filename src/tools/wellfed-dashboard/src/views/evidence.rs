//! `/api/evidence`: what the page shows beside the live state from files
//! the jobs leave: the recent history, the latest capture's tail, the
//! operator script and the guided QA run in that capture.
//!
//! The running job itself is the server's live state, never read here.

use serde::Serialize;

use super::qa::{QaView, qa_view};
use super::script::{ScriptView, script_view};
use super::tail::{TailView, rx_view};
use super::{Sources, history, latest_capture};
use wellfed_formats::bench::progress::HistoryEntry;

const HISTORY_SHOWN: usize = 15;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct EvidenceView {
    /// Newest first.
    history: Vec<HistoryEntry>,
    /// The latest capture's output.
    rx: Option<TailView>,
    script: Option<ScriptView>,
    /// The QA firmware's guided run in the latest capture.
    qa: Option<QaView>,
}

pub(crate) fn evidence(sources: &Sources) -> EvidenceView {
    let root = &sources.root;

    let mut history = history(sources);
    let shown = history.len().saturating_sub(HISTORY_SHOWN);
    let mut history = history.split_off(shown);
    history.reverse();

    let latest = latest_capture(root);
    EvidenceView {
        history,
        rx: latest.as_deref().map(|dir| rx_view(root, dir)),
        script: script_view(sources),
        qa: latest.as_deref().and_then(|dir| qa_view(root, dir)),
    }
}

/// This module's page types, for [`super::bindings`].
#[cfg(test)]
pub(super) fn bindings(cfg: &ts_rs::Config) -> Vec<String> {
    use crate::bindings::decl;
    vec![decl::<EvidenceView>(cfg)]
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::views::tests::{json, live_sources};
    use wellfed_formats::bench::CAPTURES;
    use wellfed_formats::bench::progress::BenchCommand;

    #[test]
    fn history_is_newest_first_beside_the_latest_capture() {
        let (_tmp, sources, live) = live_sources();
        let mut status = live.status(&sources);
        for name in ["one", "two"] {
            status.begin(BenchCommand::Boot, Some(name), &[]).unwrap();
            status.finish(0, None);
        }
        let dir = sources.root.join(CAPTURES).join("c");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("serial-rx.bin"), b"hello\r\n").unwrap();

        let view = json(evidence(&sources));

        assert_eq!(view["history"][0]["name"], "two");
        assert_eq!(view["history"][0]["outcome"], "pass");
        assert_eq!(view["rx"]["path"], "private/captures/c");
        assert_eq!(view["rx"]["tail"], serde_json::json!(["hello"]));
    }
}
