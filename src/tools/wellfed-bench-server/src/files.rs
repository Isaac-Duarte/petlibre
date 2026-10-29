//! The server's own small files (the bank ledger, the event cursor),
//! replaced whole and never synced: an fsync during a UART write is what
//! the bench must never cause, and a lost update to either is read again
//! from the feeder.

use std::io::Write;
use std::path::Path;

use serde::Serialize;

/// Write `value` beside `path` and rename it over, so a reader sees the
/// old file or the new one whole.
pub(crate) fn replace_json(path: &Path, value: &impl Serialize) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let temporary = parent.join(format!(".{name}.{}.tmp", std::process::id()));

    let written = (|| {
        let mut file = std::fs::File::create(&temporary)?;
        serde_json::to_writer_pretty(&mut file, value)?;
        file.write_all(b"\n")?;
        std::fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}
