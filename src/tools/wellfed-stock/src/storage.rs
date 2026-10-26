//! Durable private state files: configuration, ledger, leases and credentials.
//!
//! Every file is created mode 0600 on Unix. `write_json` replaces a file
//! atomically (`wellfed_formats::private`), so a crash leaves the old or
//! the new contents, never a torn ledger. `write_new_json`
//! refuses to replace anything, and `lock` gives one server process exclusive
//! ownership of a ledger.

use std::{fs, io::Write, path::Path};

use anyhow::{Context, Result};
use serde::Serialize;

/// Atomically replace a private state file (`wellfed_formats::private`).
pub fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    wellfed_formats::private::write_json(path, value)
        .with_context(|| format!("write {}", path.display()))
}

/// Write a new private file; refuses to replace an existing one. The file
/// appears whole or not at all: the bytes go to a temporary file beside it
/// first, which is then hard-linked into place (a link never replaces), so
/// a reader polling for it never sees it half written, and a writer killed
/// partway leaves no empty file that would refuse the next attempt.
pub fn write_new_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let name = path
        .file_name()
        .with_context(|| format!("{} names no file", path.display()))?;
    let mut temporary = name.to_owned();
    temporary.push(format!(".{}.tmp", std::process::id()));
    let temporary = path.with_file_name(temporary);

    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .with_context(|| format!("create {}", temporary.display()))?;

    let result = (|| {
        serde_json::to_writer_pretty(&mut file, value)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::hard_link(&temporary, path).with_context(|| format!("create {}", path.display()))?;
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();

    let _ = fs::remove_file(&temporary);
    result
}

/// Hold an exclusive process lock beside the ledger for the controller lifetime.
pub fn lock(path: &Path) -> Result<fs::File> {
    fs::create_dir_all(path.parent().context("lock path needs a parent")?)?;
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let file = options.open(path.with_extension("lock"))?;
    file.try_lock()
        .context("another server already owns this feed ledger")?;
    Ok(file)
}
