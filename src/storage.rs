use std::{fs, io::Write, path::Path};

use anyhow::{Context, Result};
use serde::Serialize;
use uuid::Uuid;

/// Atomically replace a private state file, syncing data before committing it.
pub fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().context("state path needs a parent")?;
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(".{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut options = fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        serde_json::to_writer_pretty(&mut file, value)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
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
