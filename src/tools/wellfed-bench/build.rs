//! The bench's build fingerprint: one hash over the sources of every
//! first-party crate `wellfed-bench` and `wellfed-itest` link, the suite
//! itself and the lockfile.
//!
//! `wellfed-bench` hands the suite's commands to the `wellfed-itest` beside
//! it, which drives relays and restores the bench on its own copy of this
//! library. Both binaries carry this constant, the bench passes its own at the
//! hand-off, and the suite refuses a different one, so a stale suite binary
//! never runs old relay or cleanup code against the feeder.

use std::fs;
use std::path::{Path, PathBuf};

/// Relative to this crate. The suite is included so that a change to it
/// alone also moves the fingerprint.
const CRATES: &[&str] = &[
    ".",
    "../wellfed-itest",
    "../wellfed-relay",
    "../wellfed-formats",
    "../wellfed-local-api",
    "../wellfed-dashboard",
    "../wellfed-build",
    "../wellfed-flash",
    "../wellfed-emu",
    "../../client/wellfed-client",
    "../../client/wellfed-model",
    "../../sdk/wellfed-core",
    "../../sdk/wellfed-proto",
    "../../sdk/wellfed-session",
];

fn main() {
    let mut files: Vec<PathBuf> = vec![PathBuf::from("../../../Cargo.lock")];
    for krate in CRATES {
        let krate = Path::new(krate);
        files.push(krate.join("Cargo.toml"));
        files.push(krate.join("build.rs"));
        collect(&krate.join("src"), &mut files);
        println!("cargo::rerun-if-changed={}", krate.join("src").display());
        println!(
            "cargo::rerun-if-changed={}",
            krate.join("Cargo.toml").display()
        );
    }
    println!("cargo::rerun-if-changed=../../../Cargo.lock");
    files.sort();

    // Each file as its length-prefixed name and contents, so no two trees
    // hash alike by moving bytes across a boundary.
    let mut input = Vec::new();
    for file in &files {
        // A crate without a build script hashes as absent, consistently.
        let Ok(data) = fs::read(file) else { continue };
        let name = file.to_string_lossy();
        input.extend_from_slice(&(name.len() as u64).to_le_bytes());
        input.extend_from_slice(name.as_bytes());
        input.extend_from_slice(&(data.len() as u64).to_le_bytes());
        input.extend_from_slice(&data);
    }
    let digest = wellfed_formats::canonical::sha256(&input);
    println!("cargo::rustc-env=WELLFED_BENCH_FINGERPRINT={digest}");
}

/// Every file under `directory`, recursively.
fn collect(directory: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, files);
        } else {
            files.push(path);
        }
    }
}
