//! A file's printable tail: the latest capture on the "Now" view, a running
//! capture's progress, and a test run's captures and server log.

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde::Serialize;

use super::{modified, printable};
use crate::status;

const TAIL_BYTES: u64 = 12_000;
const TAIL_LINES: usize = 60;

/// A file's size, modification time and printable tail: a capture's
/// serial-rx.bin or a run's server log.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct TailView {
    /// Root-relative: the capture directory, or the file itself.
    path: String,
    bytes: u64,
    modified: Option<f64>,
    tail: Vec<String>,
}

/// A capture directory's received bytes.
pub(crate) fn rx_view(root: &Path, directory: &Path) -> TailView {
    tail_view(
        &directory.join("serial-rx.bin"),
        status::relative(root, directory),
    )
}

/// The last [`TAIL_LINES`] non-blank printable lines of `file`.
pub(crate) fn tail_view(file: &Path, path: String) -> TailView {
    let Ok(mut opened) = fs::File::open(file) else {
        return TailView {
            path,
            bytes: 0,
            modified: None,
            tail: Vec::new(),
        };
    };

    let size = opened.metadata().map_or(0, |m| m.len());
    let mut data = Vec::new();
    if opened
        .seek(SeekFrom::Start(size.saturating_sub(TAIL_BYTES)))
        .is_ok()
    {
        let _ = opened.read_to_end(&mut data);
    }

    let text = printable(&data);
    let lines: Vec<&str> = text.split('\n').filter(|l| !l.trim().is_empty()).collect();
    let tail = lines[lines.len().saturating_sub(TAIL_LINES)..]
        .iter()
        .map(|line| (*line).to_string())
        .collect();
    TailView {
        path,
        bytes: size,
        modified: modified(file),
        tail,
    }
}

/// This module's page types, for [`super::bindings`].
#[cfg(test)]
pub(super) fn bindings(cfg: &ts_rs::Config) -> Vec<String> {
    use crate::ui::bindings::decl;
    vec![decl::<TailView>(cfg)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CAPTURES;
    use crate::ui::views::tests::{json, sources};

    #[test]
    fn live_capture_tail_is_printable_and_bounded() {
        let (_tmp, sources) = sources();
        let dir = sources.root.join(CAPTURES).join("cap");
        fs::create_dir_all(&dir).unwrap();
        let mut data = b"\x00\x15junk\r\n".to_vec();
        for i in 0..200 {
            data.extend(format!("line {i}\r\n").as_bytes());
        }
        fs::write(dir.join("serial-rx.bin"), &data).unwrap();

        let view = rx_view(&sources.root, &dir);
        assert_eq!(view.tail.len(), TAIL_LINES);
        assert_eq!(view.tail.last().unwrap(), "line 199");
        assert_eq!(view.bytes, data.len() as u64);
        assert_eq!(json(&view)["path"], "private/captures/cap");
    }
}
