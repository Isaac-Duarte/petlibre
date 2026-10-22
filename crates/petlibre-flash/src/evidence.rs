//! Private evidence: where inputs and outputs may live, how they are written,
//! and the recording port every serial session goes through.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::canonical::sha256;
use crate::error::{Result, require};
use crate::protocol::Port;

/// ISO-8601 UTC as Python's `datetime.now(timezone.utc).isoformat()` gives.
pub fn utc() -> String {
    let now = chrono::Utc::now();
    if now.timestamp_subsec_micros() == 0 {
        now.format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
    } else {
        now.format("%Y-%m-%dT%H:%M:%S%.6f+00:00").to_string()
    }
}

/// The repository checkout whose ignored `private/` holds every input and
/// output of a device session.
#[derive(Debug, Clone)]
pub struct Root {
    root: PathBuf,
    private: PathBuf,
}

impl Root {
    pub fn new(root: &Path) -> Result<Self> {
        let root = root.canonicalize()?;
        let private = root.join("private");
        require(
            private.is_dir(),
            format!("{} has no private/ evidence directory", root.display()),
        )?;
        Ok(Self {
            private: private.canonicalize()?,
            root,
        })
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    pub fn join(&self, path: impl AsRef<Path>) -> PathBuf {
        self.root.join(path)
    }

    /// Whether `path` resolves inside `private/`; a missing path is judged by
    /// its nearest existing ancestor.
    pub fn is_private(&self, path: &Path) -> bool {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        };
        // Python's non-strict `resolve()`: resolve the longest existing
        // prefix, then apply the remaining components lexically.
        let parts: Vec<_> = absolute.components().collect();
        for split in (1..=parts.len()).rev() {
            let prefix: PathBuf = parts[..split].iter().collect();
            let Ok(mut full) = prefix.canonicalize() else {
                continue;
            };
            for part in &parts[split..] {
                match part {
                    std::path::Component::ParentDir => {
                        full.pop();
                    }
                    std::path::Component::CurDir => {}
                    other => full.push(other),
                }
            }
            return full.starts_with(&self.private);
        }
        false
    }

    /// A regular private input file.
    pub fn private_file(&self, path: &Path) -> Result<Vec<u8>> {
        require(
            self.is_private(path) && path.is_file(),
            "Regular private input required",
        )?;
        Ok(fs::read(path)?)
    }

    /// A new private output directory.
    pub fn output_directory(&self, path: &Path) -> Result<PathBuf> {
        require(
            !path.is_symlink() && self.is_private(path),
            "Output must be a new private directory",
        )?;
        fs::DirBuilder::new()
            .recursive(false)
            .mode(0o700)
            .create(path)
            .or_else(|error| {
                // Python's mkdir(parents=True) for the reader outputs.
                if error.kind() == io::ErrorKind::NotFound {
                    fs::DirBuilder::new()
                        .recursive(true)
                        .mode(0o700)
                        .create(path)
                } else {
                    Err(error)
                }
            })?;
        Ok(std::path::absolute(path)?)
    }
}

pub fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

/// Create `path` exclusively, write it in full and fsync it.
pub fn write_new(path: &Path, data: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(data)?;
    file.sync_all()?;
    Ok(())
}

/// Rewrite a session report in place (its last state is what matters).
pub fn save_report(path: &Path, report: &Value) -> Result<()> {
    let mut text = serde_json::to_string_pretty(report)?;
    text.push('\n');
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?
        .write_all(text.as_bytes())?;
    Ok(())
}

/// Hashes of the finished serial streams, as reports record them.
pub fn stream_hashes(directory: &Path, report: &mut Value) {
    for name in ["serial-rx.bin", "serial-tx.bin"] {
        if let Ok(data) = fs::read(directory.join(name)) {
            report[format!("{name}_sha256")] = Value::String(sha256(&data));
        }
    }
}

/// Every byte in either direction is kept, with a timed event per chunk.
/// Transmit intent is recorded before the bytes are written.
pub struct EvidencePort<P: Port> {
    port: P,
    rx: File,
    tx: File,
    events: File,
    offsets: [usize; 2],
}

impl<P: Port> EvidencePort<P> {
    pub fn new(port: P, directory: &Path) -> Result<Self> {
        let create = |name: &str| {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(directory.join(name))
        };
        Ok(Self {
            port,
            rx: create("serial-rx.bin")?,
            tx: create("serial-tx.bin")?,
            events: create("serial-events.jsonl")?,
            offsets: [0, 0],
        })
    }

    fn record(&mut self, transmit: bool, data: &[u8]) -> io::Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        let (stream, index, direction) = if transmit {
            (&mut self.tx, 1, "tx")
        } else {
            (&mut self.rx, 0, "rx")
        };
        stream.write_all(data)?;
        stream.flush()?;
        let event = json!({"utc": utc(), "direction": direction,
                           "offset": self.offsets[index], "bytes": data.len()});
        writeln!(self.events, "{event}")?;
        self.events.flush()?;
        self.offsets[index] += data.len();
        Ok(())
    }
}

impl<P: Port> Port for EvidencePort<P> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = self.port.read(buffer)?;
        self.record(false, &buffer[..count])?;
        Ok(count)
    }

    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.record(true, data)?;
        self.port.write(data)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.port.flush()
    }

    fn in_waiting(&mut self) -> io::Result<usize> {
        self.port.in_waiting()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_paths_resolve_through_missing_children_and_symlinks() {
        let checkout = tempfile::tempdir().unwrap();
        fs::create_dir(checkout.path().join("private")).unwrap();
        let root = Root::new(checkout.path()).unwrap();
        assert!(root.is_private(&checkout.path().join("private/new/deeper")));
        assert!(!root.is_private(&checkout.path().join("public/new")));
        assert!(!root.is_private(&checkout.path().join("private/../escape")));
        assert!(!root.is_private(&checkout.path().join("private/missing/../../escape")));
        assert!(root.is_private(&checkout.path().join("private/missing/../kept")));
        std::os::unix::fs::symlink("/tmp", checkout.path().join("private/link")).unwrap();
        assert!(!root.is_private(&checkout.path().join("private/link/x")));
        let out = root
            .output_directory(&checkout.path().join("private/a/b"))
            .unwrap();
        assert!(out.is_dir());
        assert!(
            root.output_directory(&checkout.path().join("private/a/b"))
                .is_err()
        );
    }

    #[test]
    fn utc_has_an_explicit_offset() {
        assert!(utc().ends_with("+00:00"));
    }
}
