//! Private evidence: how session outputs are written, and the recording port
//! every serial session goes through. Its submodules are the records built
//! on that: session reports and transaction journals. Where inputs and
//! outputs may live is `wellfed_image::root`, and the frozen reviews a
//! writer consumes are `wellfed_image::review`.

pub mod journal;
pub mod report;
pub mod session;

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

pub(crate) use wellfed_formats::private::{sync_directory, write_new};
pub(crate) use wellfed_image::root::Root;
use serde::Serialize;

use crate::error::Result;
use crate::evidence::report::Streams;
use crate::link::protocol::Port;
use wellfed_formats::canonical::sha256;
use wellfed_formats::capture::{Direction, SerialEvent};

/// ISO-8601 UTC as Python's `datetime.now(timezone.utc).isoformat()` gives.
pub fn utc() -> String {
    let now = chrono::Utc::now();
    if now.timestamp_subsec_micros() == 0 {
        now.format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
    } else {
        now.format("%Y-%m-%dT%H:%M:%S%.6f+00:00").to_string()
    }
}

/// Rewrite a session report in place (its last state is what matters).
///
/// Written through `Value`, so keys stay sorted whatever the type declares.
pub fn save_report(path: &Path, report: &impl Serialize) -> Result<()> {
    let mut text = serde_json::to_string_pretty(&serde_json::to_value(report)?)?;
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
pub fn stream_hashes(directory: &Path) -> Streams {
    let hash = |name: &str| {
        fs::read(directory.join(name))
            .ok()
            .map(|data| sha256(&data))
    };
    Streams {
        rx_sha256: hash("serial-rx.bin"),
        tx_sha256: hash("serial-tx.bin"),
    }
}

/// Every byte in either direction is kept, with a timed event per chunk.
/// Transmit intent is recorded before the bytes are written.
pub struct EvidencePort<P: Port> {
    port: P,
    rx: File,
    tx: File,
    events: File,
    offsets: [u64; 2],
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

    fn record(&mut self, direction: Direction, data: &[u8]) -> io::Result<()> {
        if data.is_empty() {
            return Ok(());
        }

        let (stream, index) = match direction {
            Direction::Tx => (&mut self.tx, 1),
            Direction::Rx => (&mut self.rx, 0),
        };

        stream.write_all(data)?;
        stream.flush()?;

        let event = SerialEvent {
            utc: utc(),
            direction,
            offset: self.offsets[index],
            bytes: data.len() as u64,
        };
        writeln!(self.events, "{}", serde_json::to_string(&event)?)?;
        self.events.flush()?;
        self.offsets[index] += event.bytes;
        Ok(())
    }
}

impl<P: Port> Port for EvidencePort<P> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = self.port.read(buffer)?;
        self.record(Direction::Rx, &buffer[..count])?;
        Ok(count)
    }

    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.record(Direction::Tx, data)?;
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

        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), checkout.path().join("private/out")).unwrap();
        assert!(!root.is_private(&checkout.path().join("private/new/../out/x")));
        assert!(
            root.output_directory(&checkout.path().join("private/new/../out/x"))
                .is_err()
        );

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
