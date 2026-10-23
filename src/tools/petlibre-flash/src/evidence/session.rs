//! What every device session shares: where evidence lives, how the port is
//! opened and recorded, and the saved reports a session continues from.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::de::{DeserializeOwned, IgnoredAny};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Result, refuse, require};
use crate::evidence::report::{Status, Streams, Transport};
use crate::evidence::{self, EvidencePort, Root};
use crate::link::protocol::{self, HELPER_SHA256, Port, ROM_SHA256, Transfer};
use crate::link::sector::Baseline;
use petlibre_formats::canonical::{canonical, canonical_of, sha256};
use petlibre_formats::flash_plan::ToolHashes;

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
                    Ok(Box::new(crate::link::serial::Emulated::connect(
                        Path::new(&socket),
                    )?))
                })
            }
            None => Box::new(|path| Ok(Box::new(crate::link::serial::Serial::open(path)?))),
        };

        Ok(Self {
            root: Root::new(root)?,
            tool_sha256: sha256(&fs::read(executable)?),
            open,
            pin: Pin::Research,
        })
    }

    /// Evidence from an emulated session never continues a serial one, nor
    /// the reverse.
    pub fn same_transport(&self, report: Transport) -> Result<()> {
        require(
            report == self.transport(),
            "Report comes from a different transport than this session",
        )
    }

    /// The transport sessions record in their reports.
    pub fn transport(&self) -> Transport {
        if std::env::var_os(EMULATOR).is_some() {
            Transport::Emulator
        } else {
            Transport::Serial
        }
    }

    pub fn tools(&self) -> ToolHashes {
        ToolHashes {
            petlibre_flash: self.tool_sha256.clone(),
        }
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
        protocol::interrupted()?;
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

/// Whether a session ran to its end once its port was opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completion {
    Complete,
    /// Stopped terminally; the evidence says how far it got.
    Stopped,
}

/// How a session ended: the completion and the one JSON summary the CLI
/// prints. Each command builds its summary as a type; it is held as JSON
/// here only because commands differ in what they report.
pub struct Outcome {
    pub completion: Completion,
    pub summary: Value,
}

impl Outcome {
    /// A command that finished before any serial access.
    pub fn done(summary: &impl Serialize) -> Result<Self> {
        Ok(Self {
            completion: Completion::Complete,
            summary: serde_json::to_value(summary)?,
        })
    }
}

/// A session's end once its port was opened. A failed final report save is
/// a stop too, never a refusal: the device may already have been written.
pub fn closed(completion: Completion, summary: &impl Serialize, saved: Result<()>) -> Outcome {
    let (mut summary, error) = match (serde_json::to_value(summary), saved) {
        (Ok(summary), Ok(())) => {
            return Outcome {
                completion,
                summary,
            };
        }
        (Ok(summary), Err(error)) => (summary, error),
        (Err(error), _) => (Value::Object(Default::default()), error.into()),
    };

    progress(&format!(
        "Final report not saved: {error}; inspect the journal"
    ));
    summary["report_error"] = Value::String(error.to_string());
    Outcome {
        completion: Completion::Stopped,
        summary,
    }
}

pub fn progress(line: &str) {
    eprintln!("{line}");
}

/// A saved session report, as far as later sessions read it. Every field
/// is optional so that a missing one fails the check that needs it, with
/// that check's message; the result type is the mode's.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Saved<R> {
    pub status: Option<Status>,
    pub mode: Option<String>,
    pub transport: Transport,
    pub port: Option<String>,
    pub helper_sha256: Option<String>,
    pub helper_upload_attempted: Option<bool>,
    pub helper_upload_completed: Option<bool>,
    pub bounded_read_report_sha256: Option<String>,
    pub result: Option<R>,
    /// An inspection's bindings, compared whole in canonical form.
    pub bindings: Option<Value>,
    pub observations: Option<Vec<Vec<Observed>>>,
    pub status_checks: Option<Vec<StatusCheck>>,
    #[serde(flatten)]
    pub streams: Streams,
}

impl<R> Saved<R> {
    fn is(&self, status: Status, mode: &str) -> bool {
        self.status == Some(status) && self.mode.as_deref() == Some(mode)
    }
}

/// A ROM inspection's result.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct RomResult {
    pub returned_to_km4: Option<bool>,
    pub matching_candidates: Option<BTreeMap<String, String>>,
    pub sampled_words: Option<Vec<SampledWord>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct SampledWord {
    pub address: Option<String>,
}

/// A bounded or full read's result.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ReadResult {
    pub offset: Option<u64>,
    pub bytes: Option<u64>,
    pub sha256: Option<String>,
}

/// One recorded region transfer of an inspection.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Observed {
    pub file: Option<String>,
    pub offset: Option<u64>,
    pub bytes: Option<u64>,
    pub sha256: Option<String>,
}

/// One inspection status event.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct StatusCheck {
    pub event: Option<String>,
    pub assessment: Option<CheckedAssessment>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct CheckedAssessment {
    pub software_preconditions_met: Option<bool>,
}

/// A report's exact bytes, which plans bind, and its fields.
fn saved<T: DeserializeOwned>(root: &Root, path: &Path) -> Result<(Vec<u8>, T)> {
    let raw = root.private_file(path)?;
    let value = serde_json::from_slice(&raw)?;
    Ok((raw, value))
}

fn parent(path: &Path) -> PathBuf {
    path.parent().map(Path::to_path_buf).unwrap_or_default()
}

/// The saved serial streams beside a report still match its hashes.
pub fn streams_match(root: &Root, path: &Path, streams: &Streams, message: &str) -> Result<()> {
    for (name, recorded) in [
        ("serial-rx.bin", &streams.rx_sha256),
        ("serial-tx.bin", &streams.tx_sha256),
    ] {
        let data = root.private_file(&parent(path).join(name))?;
        require(recorded.as_deref() == Some(&sha256(&data)), message)?;
    }
    Ok(())
}

/// A successful ROM inspection that identified a pinned SDK ROM.
pub fn validate_rom_report(context: &Context, path: &Path) -> Result<Vec<u8>> {
    let root = &context.root;
    require(
        root.is_private(path),
        "ROM report must refer to private evidence",
    )?;

    let (raw, report): (_, Saved<RomResult>) = saved(root, path)?;
    context.same_transport(report.transport)?;
    let result = report.result.as_ref();
    require(
        report.is(Status::Complete, "inspect-rom")
            && result.is_some_and(|r| r.returned_to_km4 == Some(true)),
        "Successful ROM inspection required",
    )?;

    let candidates = result.and_then(|r| r.matching_candidates.as_ref());
    require(
        candidates
            .is_some_and(|c| !c.is_empty() && c.values().all(|v| ROM_SHA256.contains(&v.as_str()))),
        "ROM report does not identify a pinned candidate",
    )?;

    let expected: Vec<Option<String>> = protocol::rom_sample_addresses()
        .iter()
        .map(|a| Some(format!("0x{a:08x}")))
        .collect();
    let sampled: Vec<Option<String>> = result
        .and_then(|r| r.sampled_words.as_ref())
        .map(|words| words.iter().map(|w| w.address.clone()).collect())
        .unwrap_or_default();
    require(sampled == expected, "Incomplete ROM sample set")?;

    streams_match(
        root,
        path,
        &report.streams,
        "ROM serial evidence hash mismatch",
    )?;
    Ok(raw)
}

fn bounded_read(report: &Saved<ReadResult>, port: &str) -> bool {
    report.is(Status::Complete, "read-small")
        && report.port.as_deref() == Some(port)
        && report.helper_sha256.as_deref() == Some(HELPER_SHA256)
        && report.helper_upload_completed == Some(true)
        && report.result.as_ref().and_then(|r| r.bytes) == Some(4096)
}

fn read_sha256(report: &Saved<ReadResult>) -> Option<&str> {
    report.result.as_ref().and_then(|r| r.sha256.as_deref())
}

/// A completed bounded read and its raw evidence, which a later session
/// continues. This checks saved evidence, not live helper identity or power
/// continuity; the supervised caller establishes those.
pub fn validate_status_session(
    context: &Context,
    path: Option<&Path>,
    port: &str,
) -> Result<Vec<u8>> {
    let root = &context.root;
    let Some(path) = path else {
        return refuse("A completed bounded-read report is required");
    };

    require(
        root.is_private(path),
        "Bounded-read report must refer to private evidence",
    )?;

    let (raw, report): (_, Saved<ReadResult>) = saved(root, path)?;
    context.same_transport(report.transport)?;
    require(
        bounded_read(&report, port) && report.result.as_ref().and_then(|r| r.offset) == Some(0),
        "Bounded-read session does not validate",
    )?;

    let data = fs::read(parent(path).join("flash-read.bin"))?;
    require(
        data.len() == 4096 && read_sha256(&report) == Some(&sha256(&data)),
        "Bounded-read image does not validate",
    )?;

    streams_match(
        root,
        path,
        &report.streams,
        "Bounded-read serial evidence hash mismatch",
    )?;
    Ok(raw)
}

/// The full read's precondition: the bounded read it continues. Unlike
/// `validate_status_session`, Python checks no serial hashes here.
pub fn validate_small_for_full(context: &Context, path: &Path, port: &str) -> Result<Vec<u8>> {
    let root = &context.root;
    let (raw, report): (_, Saved<ReadResult>) = saved(root, path)?;
    context.same_transport(report.transport)?;

    let image = fs::read(parent(path).join("flash-read.bin"))?;
    require(
        root.is_private(path)
            && bounded_read(&report, port)
            && read_sha256(&report) == Some(&sha256(&image)),
        "Bounded-read evidence does not validate",
    )?;
    Ok(raw)
}

/// Write a reviewed plan exclusively and durably; returns its SHA-256.
pub fn write_plan(root: &Root, output: &Path, plan: &impl Serialize) -> Result<String> {
    require(root.is_private(output), "Private plan required")?;
    let mut raw = canonical_of(plan)?;
    raw.push(b'\n');
    evidence::write_new(output, &raw)?;
    evidence::sync_directory(&parent(&std::path::absolute(output)?))?;
    Ok(sha256(&raw))
}

/// The reviewed plan an execution was given, checked before serial access.
/// The file is compared whole in canonical form, so it stays JSON.
pub fn reviewed_plan(
    root: &Root,
    path: Option<&Path>,
    expected_sha256: Option<&str>,
    expected: &impl Serialize,
) -> Result<Vec<u8>> {
    let (Some(path), Some(expected_sha256)) = (path, expected_sha256) else {
        return refuse("Reviewed plan and hash required");
    };

    let raw = root.private_file(path)?;
    let parsed: Value = serde_json::from_slice(&raw)?;
    require(
        sha256(&raw) == expected_sha256 && canonical(&parsed)? == canonical_of(expected)?,
        "Reviewed plan/source/evidence differs",
    )?;
    Ok(raw)
}

/// Keep the reviewed plan beside the session it authorizes.
pub fn keep_plan(directory: &Path, raw: &[u8]) -> Result<()> {
    evidence::write_new(&directory.join("reviewed-plan.json"), raw)?;
    evidence::sync_directory(directory)
}

/// One region transfer as a session report records it.
#[derive(Debug, Clone, Serialize)]
pub struct Region {
    pub file: String,
    pub offset: usize,
    #[serde(flatten)]
    pub transfer: Transfer,
    pub sha256: String,
}

/// One region read to `<label>-NN.bin`, renamed only once complete.
pub fn acquire_region(
    port: &mut dyn Port,
    directory: &Path,
    label: &str,
    number: usize,
    offset: usize,
    length: usize,
) -> Result<(Region, Vec<u8>)> {
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

    let region = Region {
        file: name,
        offset,
        transfer,
        sha256: sha256(&data),
    };
    Ok((region, data))
}

/// The bytes of an inspection's recorded transfers, checked against their
/// expected extents and hashes.
pub(crate) fn inspected_transfers<const N: usize>(
    root: &Root,
    path: &Path,
    report: &Saved<IgnoredAny>,
    extents: &[(usize, usize); N],
    message: &str,
) -> Result<[Vec<u8>; N]> {
    let Some([first]) = report.observations.as_deref() else {
        return refuse(message);
    };
    require(first.len() == N, message)?;

    let mut values = Vec::new();
    for (transfer, (offset, length)) in first.iter().zip(extents) {
        let file = transfer.file.as_deref().unwrap_or_default();
        require(
            !file.is_empty() && !file.contains('/'),
            "Inspection extent/hash differs",
        )?;

        let value = root.private_file(&parent(path).join(file))?;
        require(
            transfer.offset == u64::try_from(*offset).ok()
                && transfer.bytes == u64::try_from(*length).ok()
                && value.len() == *length
                && transfer.sha256.as_deref() == Some(&sha256(&value)),
            "Inspection extent/hash differs",
        )?;
        values.push(value);
    }

    let Ok(values) = values.try_into() else {
        return refuse(message);
    };
    Ok(values)
}

/// A finished inspection whose bindings match this run's: its path, its
/// report and the SHA-256 of the report's bytes.
pub(crate) fn inspection(
    context: &Context,
    path: Option<&Path>,
    bindings: &impl Serialize,
) -> Result<(PathBuf, Saved<IgnoredAny>, String)> {
    let root = &context.root;
    let Some(path) = path else {
        return refuse("Completed inspection report required");
    };
    let (raw, report): (_, Saved<IgnoredAny>) = saved(root, path)?;
    context.same_transport(report.transport)?;

    let recorded = match &report.bindings {
        Some(bindings) => canonical(bindings)?,
        None => canonical(&Value::Null)?,
    };
    require(
        report.is(Status::Complete, "inspect")
            && recorded == canonical_of(bindings)?
            && report.helper_upload_attempted == Some(false),
        "Fresh matching inspection required",
    )?;

    streams_match(
        root,
        path,
        &report.streams,
        "Inspection serial evidence mismatch",
    )?;
    Ok((path.to_path_buf(), report, sha256(&raw)))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn small(value: Value) -> Saved<ReadResult> {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn saved_reports_refuse_what_they_lack() {
        let complete = json!({
            "status": "complete", "mode": "read-small", "port": "/dev/ttyUSB0",
            "helper_sha256": HELPER_SHA256, "helper_upload_completed": true,
            "result": {"offset": 0, "bytes": 4096, "sha256": "s"},
        });
        let report = small(complete.clone());
        assert!(bounded_read(&report, "/dev/ttyUSB0"));
        assert!(!bounded_read(&report, "/dev/ttyUSB1"));

        // Python reports name no transport and are serial.
        assert_eq!(report.transport, Transport::Serial);

        for field in [
            "status",
            "mode",
            "port",
            "helper_sha256",
            "helper_upload_completed",
        ] {
            let mut partial = complete.clone();
            partial.as_object_mut().unwrap().remove(field);
            assert!(!bounded_read(&small(partial), "/dev/ttyUSB0"), "{field}");
        }

        let mut no_bytes = complete.clone();
        no_bytes["result"].as_object_mut().unwrap().remove("bytes");
        assert!(!bounded_read(&small(no_bytes), "/dev/ttyUSB0"));

        // An explicit false is required where the old check compared with false.
        let inspection: Saved<IgnoredAny> = serde_json::from_value(json!({})).unwrap();
        assert_eq!(inspection.helper_upload_attempted, None);

        let mut emulated = complete;
        emulated["transport"] = json!("emulator");
        assert_eq!(small(emulated).transport, Transport::Emulator);
    }
}
