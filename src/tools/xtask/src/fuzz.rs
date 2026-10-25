//! `cargo xtask fuzz <target>`: one bounded libFuzzer run of a target in
//! `src/fuzz/`, the out-of-workspace cargo-fuzz crate, or one replay of a
//! saved input.
//!
//! libFuzzer's coverage and sanitizer flags are unstable compiler options.
//! Rather than require a nightly toolchain alongside the pinned one (whose
//! version the crates' `rust-version` demands), the run enables them on the
//! pinned compiler with `RUSTC_BOOTSTRAP=1`, for this process tree only.
//! Nothing here touches the main workspace's target directory or lockfile.

use std::io;
use std::path::Path;
use std::process::{Command, ExitStatus};

use clap::ValueEnum;

/// The targets in `src/fuzz/Cargo.toml`, one per network parser.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum Target {
    /// The device API from an unauthenticated peer: opening frame and handshakes.
    ApiConnection,
    /// The device API's request decoding inside an open session.
    ApiSession,
    /// The device protocol's postcard codec, both directions.
    ProtoDecode,
    /// The Noise handshake reader, every pattern's responder.
    NoiseResponder,
    /// BLE setup, the feeder's responder and the code proof.
    Setup,
    /// The update manifest, delivery messages and the bank-1 writer.
    OtaManifest,
    /// The MQTT transport codec and its session.
    Mqtt,
}

impl Target {
    fn name(self) -> &'static str {
        match self {
            Self::ApiConnection => "api_connection",
            Self::ApiSession => "api_session",
            Self::ProtoDecode => "proto_decode",
            Self::NoiseResponder => "noise_responder",
            Self::Setup => "setup",
            Self::OtaManifest => "ota_manifest",
            Self::Mqtt => "mqtt",
        }
    }
}

/// The fuzz crate, relative to the repository root.
const FUZZ_DIR: &str = "src/fuzz";

/// What one run does.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Run<'a> {
    /// Fuzz for this many seconds.
    Seconds(u32),
    /// Run this one saved input (a crash artifact) and stop.
    Replay(&'a Path),
}

/// One run of `target`; the fuzzer's exit status. A crash leaves its input
/// under `src/fuzz/artifacts/<target>/`, where `Run::Replay` takes it.
pub(crate) fn run(root: &Path, target: Target, run: Run<'_>) -> io::Result<ExitStatus> {
    let installed = Command::new("cargo")
        .args(["fuzz", "--version"])
        .current_dir(root)
        .output()
        .is_ok_and(|output| output.status.success());
    if !installed {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "cargo-fuzz is not installed (`cargo install cargo-fuzz`)",
        ));
    }

    let mut command = Command::new("cargo");
    command.args(["fuzz", "run", "--fuzz-dir", FUZZ_DIR, target.name()]);
    match run {
        Run::Seconds(seconds) => command.arg("--").arg(format!("-max_total_time={seconds}")),
        Run::Replay(input) => command.arg(input),
    };
    command
        .env("RUSTC_BOOTSTRAP", "1")
        .current_dir(root)
        .status()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every target the command offers is a `[[bin]]` of the fuzz crate.
    #[test]
    fn every_target_is_in_the_fuzz_manifest() {
        let manifest = include_str!("../../../fuzz/Cargo.toml");
        for target in Target::value_variants() {
            let entry = format!("name = \"{}\"", target.name());
            assert!(manifest.contains(&entry), "{entry} missing");
            let source = format!("fuzz_targets/{}.rs", target.name());
            assert!(manifest.contains(&source), "{source} missing");
        }
    }
}
