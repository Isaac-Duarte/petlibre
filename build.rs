//! Record the source commit so a running service can prove what it executes.
use std::process::Command;

const BUILD_INPUTS: [&str; 5] = [
    "src",
    "build.rs",
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
];

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn main() {
    for input in BUILD_INPUTS {
        println!("cargo:rerun-if-changed={input}");
    }
    if let (Some(dir), Some(common)) = (
        git(&["rev-parse", "--git-dir"]),
        git(&["rev-parse", "--git-common-dir"]),
    ) {
        println!("cargo:rerun-if-changed={dir}/HEAD");
        println!("cargo:rerun-if-changed={dir}/index");
        println!("cargo:rerun-if-changed={common}/refs/heads");
        println!("cargo:rerun-if-changed={common}/packed-refs");
    }
    let commit = git(&["rev-parse", "HEAD"]).filter(|c| c.len() == 40);
    // Untracked files inside build inputs count: they can be compiled in.
    let mut status = vec!["status", "--porcelain", "--"];
    status.extend(BUILD_INPUTS);
    let clean = commit.is_some() && git(&status).is_some_and(|s| s.is_empty());
    println!(
        "cargo:rustc-env=PETLIBRO_BUILD_COMMIT={}",
        commit.as_deref().unwrap_or("unknown")
    );
    println!("cargo:rustc-env=PETLIBRO_BUILD_CLEAN={clean}");
}
