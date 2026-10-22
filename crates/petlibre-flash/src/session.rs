//! What every device session shares: where evidence lives, how the port is
//! opened and recorded, and the saved reports a session continues from.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::canonical::{canonical, sha256};
use crate::error::{Result, require};
use crate::evidence::{self, EvidencePort, Root};
use crate::protocol::{self, HELPER_SHA256, Port, ROM_SHA256};
use crate::sector::Baseline;

pub const DEFAULT_HELPER: &str =
    "private/upstream/ameba-rtos-d/tools/AmebaD/Image_Tool/imgtool_flashloader_amebad.bin";

pub type Opener = dyn Fn(&str) -> Result<Box<dyn Port>>;

/// A Unix socket serving an emulated helper; set only by offline tests.
pub const EMULATOR: &str = "PETLIBRE_FLASH_EMULATOR";

/// Which baseline a session accepts. Only host tests can name another.
#[derive(Clone, Copy)]
pub enum Pin {
    Research,
    #[cfg(test)]
    Synthetic,
}

pub struct Context {
    pub root: Root,
    /// SHA-256 of the running executable, bound into every plan the way the
    /// Python clients bind their source hashes.
    pub tool_sha256: String,
    pub open: Box<Opener>,
    pub pin: Pin,
}

impl Context {
    pub fn new(root: &Path) -> Result<Self> {
        let executable = std::env::current_exe()?;
        let open: Box<Opener> = match std::env::var_os(EMULATOR) {
            // Offline only: every session talks to an emulated helper instead.
            Some(socket) => {
                progress(&format!(
                    "{EMULATOR} is set: using the emulated helper at {}, not a device",
                    Path::new(&socket).display()
                ));
                Box::new(move |_| {
                    Ok(Box::new(crate::serial::Emulated::connect(Path::new(
                        &socket,
                    ))?))
                })
            }
            None => Box::new(|path| Ok(Box::new(crate::serial::Serial::open(path)?))),
        };
        Ok(Self {
            root: Root::new(root)?,
            tool_sha256: sha256(&fs::read(executable)?),
            open,
            pin: Pin::Research,
        })
    }

    /// The transport sessions record in their reports.
    pub fn transport(&self) -> &'static str {
        if std::env::var_os(EMULATOR).is_some() {
            "emulator"
        } else {
            "serial"
        }
    }

    pub fn tools(&self) -> Value {
        json!({"petlibre-flash": self.tool_sha256})
    }

    pub fn baseline(&self, path: &Path) -> Result<Baseline> {
        let data = self.root.private_file(path)?;
        match self.pin {
            Pin::Research => Baseline::pinned(data),
            #[cfg(test)]
            Pin::Synthetic => Ok(Baseline::synthetic(data)),
        }
    }

    pub fn helper(&self, path: Option<&Path>) -> Result<Vec<u8>> {
        let path = path.map_or_else(|| self.root.join(DEFAULT_HELPER), Path::to_path_buf);
        protocol::load_helper(fs::read(path)?)
    }

    /// A new private output directory and the recorded port inside it.
    pub fn record(&self, port: &str, directory: &Path) -> Result<EvidencePort<Box<dyn Port>>> {
        EvidencePort::new((self.open)(port)?, directory)
    }
}

impl<P: Port + ?Sized> Port for Box<P> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        (**self).read(buffer)
    }

    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        (**self).write(data)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        (**self).flush()
    }

    fn in_waiting(&mut self) -> std::io::Result<usize> {
        (**self).in_waiting()
    }
}

/// How a session ended once its port was opened.
pub struct Outcome {
    pub complete: bool,
    pub summary: Value,
}

pub fn progress(line: &str) {
    eprintln!("{line}");
}

fn report(root: &Root, path: &Path) -> Result<(Vec<u8>, Value)> {
    let raw = root.private_file(path)?;
    let value = serde_json::from_slice(&raw)?;
    Ok((raw, value))
}

fn parent(path: &Path) -> PathBuf {
    path.parent().map(Path::to_path_buf).unwrap_or_default()
}

/// The saved serial streams beside a report still match its hashes.
pub fn streams_match(root: &Root, path: &Path, report: &Value, message: &str) -> Result<()> {
    for name in ["serial-rx.bin", "serial-tx.bin"] {
        let data = root.private_file(&parent(path).join(name))?;
        require(
            report[format!("{name}_sha256")] == sha256(&data).as_str(),
            message,
        )?;
    }
    Ok(())
}

/// A successful ROM inspection that identified a pinned SDK ROM.
pub fn validate_rom_report(root: &Root, path: &Path) -> Result<Vec<u8>> {
    require(
        root.is_private(path),
        "ROM report must refer to private evidence",
    )?;
    let (raw, report) = report(root, path)?;
    let result = &report["result"];
    require(
        report["status"] == "complete"
            && report["mode"] == "inspect-rom"
            && result["returned_to_km4"] == true,
        "Successful ROM inspection required",
    )?;
    let candidates = result["matching_candidates"].as_object();
    require(
        candidates.is_some_and(|c| {
            !c.is_empty()
                && c.values()
                    .all(|v| v.as_str().is_some_and(|v| ROM_SHA256.contains(&v)))
        }),
        "ROM report does not identify a pinned candidate",
    )?;
    let expected: Vec<Value> = protocol::rom_sample_addresses()
        .iter()
        .map(|a| Value::String(format!("0x{a:08x}")))
        .collect();
    let sampled: Vec<Value> = result["sampled_words"]
        .as_array()
        .map(|words| words.iter().map(|w| w["address"].clone()).collect())
        .unwrap_or_default();
    require(sampled == expected, "Incomplete ROM sample set")?;
    streams_match(root, path, &report, "ROM serial evidence hash mismatch")?;
    Ok(raw)
}

fn bounded_read(report: &Value, port: &str) -> bool {
    report["status"] == "complete"
        && report["mode"] == "read-small"
        && report["port"] == port
        && report["helper_sha256"] == HELPER_SHA256
        && report["helper_upload_completed"] == true
        && report["result"]["bytes"].as_u64() == Some(4096)
}

/// A completed bounded read and its raw evidence, which a later session
/// continues. This checks saved evidence, not live helper identity or power
/// continuity; the supervised caller establishes those.
pub fn validate_status_session(root: &Root, path: Option<&Path>, port: &str) -> Result<Vec<u8>> {
    let path = path.ok_or_else(|| {
        crate::Error::Refused("A completed bounded-read report is required".into())
    })?;
    require(
        root.is_private(path),
        "Bounded-read report must refer to private evidence",
    )?;
    let (raw, report) = report(root, path)?;
    require(
        bounded_read(&report, port) && report["result"]["offset"].as_u64() == Some(0),
        "Bounded-read session does not validate",
    )?;
    let data = fs::read(parent(path).join("flash-read.bin"))?;
    require(
        data.len() == 4096 && report["result"]["sha256"] == sha256(&data).as_str(),
        "Bounded-read image does not validate",
    )?;
    streams_match(
        root,
        path,
        &report,
        "Bounded-read serial evidence hash mismatch",
    )?;
    Ok(raw)
}

/// The full read's precondition: the bounded read it continues. Unlike
/// `validate_status_session`, Python checks no serial hashes here.
pub fn validate_small_for_full(root: &Root, path: &Path, port: &str) -> Result<Vec<u8>> {
    let (raw, report) = report(root, path)?;
    let image = fs::read(parent(path).join("flash-read.bin"))?;
    require(
        root.is_private(path)
            && bounded_read(&report, port)
            && report["result"]["sha256"] == sha256(&image).as_str(),
        "Bounded-read evidence does not validate",
    )?;
    Ok(raw)
}

/// Write a reviewed plan exclusively and durably; returns its SHA-256.
pub fn write_plan(root: &Root, output: &Path, plan: &Value) -> Result<String> {
    require(root.is_private(output), "Private plan required")?;
    let mut raw = canonical(plan)?;
    raw.push(b'\n');
    evidence::write_new(output, &raw)?;
    evidence::sync_directory(&parent(&std::path::absolute(output)?))?;
    Ok(sha256(&raw))
}

/// The reviewed plan an execution was given, checked before serial access.
pub fn reviewed_plan(
    root: &Root,
    path: Option<&Path>,
    expected_sha256: Option<&str>,
    expected: &Value,
) -> Result<Vec<u8>> {
    let (Some(path), Some(expected_sha256)) = (path, expected_sha256) else {
        return Err(crate::Error::Refused(
            "Reviewed plan and hash required".into(),
        ));
    };
    let raw = root.private_file(path)?;
    let parsed: Value = serde_json::from_slice(&raw)?;
    require(
        sha256(&raw) == expected_sha256 && canonical(&parsed)? == canonical(expected)?,
        "Reviewed plan/source/evidence differs",
    )?;
    Ok(raw)
}

/// Keep the reviewed plan beside the session it authorizes.
pub fn keep_plan(directory: &Path, raw: &[u8]) -> Result<()> {
    evidence::write_new(&directory.join("reviewed-plan.json"), raw)?;
    evidence::sync_directory(directory)
}

/// One region read to `<label>-NN.bin`, renamed only once complete.
pub fn acquire_region(
    port: &mut dyn Port,
    directory: &Path,
    label: &str,
    number: usize,
    offset: usize,
    length: usize,
) -> Result<(Value, Vec<u8>)> {
    let partial = directory.join(format!("{label}-{number:02}.bin.part"));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&partial)?;
    let transfer = protocol::receive_flash(port, offset, length, &mut file, None)?;
    file.sync_all()?;
    drop(file);
    let data = fs::read(&partial)?;
    require(data.len() == length, "Incomplete region read")?;
    let name = format!("{label}-{number:02}.bin");
    fs::rename(&partial, directory.join(&name))?;
    evidence::sync_directory(directory)?;
    let mut entry: Map<String, Value> = Map::new();
    entry.insert("file".into(), json!(name));
    entry.insert("offset".into(), json!(offset));
    entry.extend(transfer.json());
    entry.insert("sha256".into(), json!(sha256(&data)));
    Ok((Value::Object(entry), data))
}

/// The bytes of an inspection's recorded transfers, checked against their
/// expected extents and hashes.
pub fn inspected_transfers(
    root: &Root,
    path: &Path,
    report: &Value,
    extents: &[(usize, usize)],
    message: &str,
) -> Result<Vec<Vec<u8>>> {
    let observations = report["observations"].as_array();
    let first = observations
        .filter(|o| o.len() == 1)
        .and_then(|o| o[0].as_array())
        .filter(|o| o.len() == extents.len())
        .ok_or_else(|| crate::Error::Refused(message.into()))?;
    let mut values = Vec::new();
    for (transfer, (offset, length)) in first.iter().zip(extents) {
        let file = transfer["file"].as_str().unwrap_or_default();
        require(
            !file.is_empty() && !file.contains('/'),
            "Inspection extent/hash differs",
        )?;
        let value = root.private_file(&parent(path).join(file))?;
        require(
            transfer["offset"].as_u64() == Some(*offset as u64)
                && transfer["bytes"].as_u64() == Some(*length as u64)
                && value.len() == *length
                && transfer["sha256"] == sha256(&value).as_str(),
            "Inspection extent/hash differs",
        )?;
        values.push(value);
    }
    Ok(values)
}

/// A finished inspection whose bindings match this run's.
pub fn inspection(
    root: &Root,
    path: Option<&Path>,
    bindings: &Value,
) -> Result<(PathBuf, Value, String)> {
    let path =
        path.ok_or_else(|| crate::Error::Refused("Completed inspection report required".into()))?;
    let (raw, report) = report(root, path)?;
    require(
        report["status"] == "complete"
            && report["mode"] == "inspect"
            && canonical(&report["bindings"])? == canonical(bindings)?
            && report["helper_upload_attempted"] == false,
        "Fresh matching inspection required",
    )?;
    streams_match(root, path, &report, "Inspection serial evidence mismatch")?;
    Ok((path.to_path_buf(), report, sha256(&raw)))
}
