//! Writing the private files under `private/`: secrets, ledgers and keys.
//!
//! Every file is created mode 0600 on Unix. [`write_json`] replaces a file
//! atomically (temporary file, fsync, rename, directory fsync), so a crash
//! leaves the old or the new contents, never a torn file. [`write_new`]
//! writes evidence that must never replace anything: it refuses a path that
//! exists. New secrets come from [`random`].

use std::fs;
use std::io::{Read, Write};
use std::path::Path;

use serde::Serialize;

use crate::Result;

/// The operating system's CSPRNG, which the kernel seeds before userspace
/// can run a tool.
pub const SYSTEM_RANDOM: &str = "/dev/urandom";

/// `N` bytes from `source` (normally [`SYSTEM_RANDOM`]), for new secrets.
pub fn random<const N: usize>(source: &Path) -> Result<[u8; N]> {
    let mut bytes = [0u8; N];
    fs::File::open(source)?.read_exact(&mut bytes)?;
    Ok(bytes)
}

/// Create `path` exclusively, write it in full and fsync it.
pub fn write_new(path: &Path, data: &[u8]) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut file = options.open(path)?;
    file.write_all(data)?;
    file.sync_all()?;
    Ok(())
}

/// Fsync a directory, so the entries just created in it are durable.
pub fn sync_directory(path: &Path) -> Result<()> {
    fs::File::open(path)?.sync_all()?;
    Ok(())
}

/// Atomically replace a private file, syncing data before committing it.
pub fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let tag = crate::hex::encode(&random::<8>(Path::new(SYSTEM_RANDOM))?);
    let temp = parent.join(format!(".{tag}.tmp"));

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_replaced_file_is_private_and_whole() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        write_json(&path, &[1, 2]).unwrap();
        write_json(&path, &[3]).unwrap();
        assert_eq!(crate::load::<Vec<u8>>(&path).unwrap(), [3]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1, "no temp left");
    }
}
