//! Durable private state files: configuration, ledger, leases and credentials.
//!
//! Every file is created mode 0600 on Unix. `write_json` replaces a file
//! atomically (`petlibre_formats::private`), so a crash leaves the old or
//! the new contents, never a torn ledger. `write_new_json`
//! refuses to replace anything, and `lock` gives one server process exclusive
//! ownership of a ledger.

use std::{fs, io::Write, path::Path};

use anyhow::{Context, Result};
use serde::Serialize;

pub(crate) use petlibre_formats::private::SYSTEM_RANDOM;

/// `N` bytes from `source` (normally [`SYSTEM_RANDOM`]), for new secrets.
pub(crate) fn random<const N: usize>(source: &Path) -> Result<[u8; N]> {
    petlibre_formats::private::random(source).context("read the system random source")
}

/// Atomically replace a private state file (`petlibre_formats::private`).
pub fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    petlibre_formats::private::write_json(path, value)
        .with_context(|| format!("write {}", path.display()))
}

/// Write a new private file; refuses to replace an existing one.
pub fn write_new_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut file = options
        .open(path)
        .with_context(|| format!("create {}", path.display()))?;

    let result = (|| {
        serde_json::to_writer_pretty(&mut file, value)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(path);
    }
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
