//! The cargo environment of a firmware build, and the rustc it selects.
//!
//! Overrides that can change firmware code generation or the compiler are
//! refused rather than scrubbed: refusing keeps an explicit user choice
//! visible, while silently dropping it would make the build differ from what
//! the caller asked for. `cargo run` sets `RUSTUP_TOOLCHAIN`, so the builder
//! is run as its own executable, never through cargo.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

use wellfed_formats::manifest::RustCompiler;
use regex::Regex;

use crate::platform::TARGETS;
use crate::{Error, Repo, Result, require, text, which};

const RUST_ENVIRONMENT: [&str; 13] = [
    "RUSTFLAGS",
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_BUILD_RUSTFLAGS",
    "RUSTC",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "CARGO_BUILD_RUSTC",
    "CARGO_BUILD_RUSTC_WRAPPER",
    "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
    "RUSTUP_TOOLCHAIN",
    "RUSTC_BOOTSTRAP",
    "CARGO_INCREMENTAL",
    "CARGO_BUILD_INCREMENTAL",
];
const RUST_ENVIRONMENT_PREFIXES: [&str; 2] = ["CARGO_PROFILE_", "CARGO_UNSTABLE_"];

pub(crate) type Environment = BTreeMap<String, String>;

/// The Rust compiler a Rust profile builds with, resolved before and
/// compared after compilation.
pub(crate) struct Rust {
    pub(crate) environment: Environment,
    pub(crate) compiler: RustCompiler,
}

impl Rust {
    /// This process's environment, checked, and the rustc it selects.
    pub(crate) fn resolve(repo: &Repo) -> Result<Self> {
        let environment = rust_environment(&repo.root, process_environment())?;
        let compiler = rust_compiler(repo, &environment)?;
        Ok(Self {
            environment,
            compiler,
        })
    }
}

/// `CARGO_TARGET_<TRIPLE>_RUSTFLAGS` of a firmware target.
fn target_rustflags(name: &str) -> bool {
    TARGETS.iter().any(|target| {
        let triple = target.to_ascii_uppercase().replace(['-', '.'], "_");
        name == format!("CARGO_TARGET_{triple}_RUSTFLAGS")
    })
}
static HEADER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[\[?\s*([^\]]+?)\s*\]\]?$").expect("literal pattern"));
static ASSIGNMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([^=]+?)\s*=").expect("literal pattern"));
static KEY_PART: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""[^"]*"|'[^']*'|[^.]+"#).expect("literal pattern"));

/// Environment names that would change the firmware compilation, sorted.
fn forbidden_variables(environment: &Environment) -> Vec<String> {
    environment
        .keys()
        .filter(|name| {
            RUST_ENVIRONMENT.contains(&name.as_str())
                || RUST_ENVIRONMENT_PREFIXES
                    .iter()
                    .any(|prefix| name.starts_with(prefix))
                || target_rustflags(name)
        })
        .cloned()
        .collect()
}

/// Cargo configuration files that apply to a build run from the root.
fn cargo_config_files(root: &Path, environment: &Environment) -> Vec<PathBuf> {
    let home = environment
        .get("CARGO_HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            environment
                .get("HOME")
                .map(|home| Path::new(home).join(".cargo"))
        })
        .unwrap_or_else(|| PathBuf::from(".cargo"));

    let mut candidates: Vec<PathBuf> = root
        .ancestors()
        .flat_map(|dir| ["config.toml", "config"].map(|name| dir.join(".cargo").join(name)))
        .collect();
    candidates.extend(["config.toml", "config"].map(|name| home.join(name)));

    let mut seen = std::collections::HashSet::new();
    candidates
        .into_iter()
        .filter(|path| seen.insert(path.clone()) && path.is_file())
        .collect()
}

/// Dotted keys that can change firmware codegen or the selected compiler.
///
/// A small line-oriented reader: TOML table headers and `key = value` lines.
/// Inline tables for `build`, `profile`, `target` or `unstable` are refused
/// whole. Host-only `[target.<host triple>]` settings (such as a host linker)
/// are allowed.
fn cargo_config_overrides(text: &str) -> Vec<String> {
    let mut table = Vec::new();
    let mut found = Vec::new();

    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some(header) = HEADER.captures(line) {
            table = keys(&header[1]);
            continue;
        }
        // Anything else is the continuation of a multi-line value.
        let Some(assignment) = ASSIGNMENT.captures(line) else {
            continue;
        };

        let mut key = table.clone();
        key.extend(keys(&assignment[1]));
        let end = assignment.get(0).map_or(0, |m| m.end());
        if forbidden_cargo_key(&key, line[end..].trim_start().starts_with('{')) {
            found.push(key.join("."));
        }
    }
    found
}

fn keys(dotted: &str) -> Vec<String> {
    KEY_PART
        .find_iter(dotted)
        .map(|part| {
            part.as_str()
                .trim()
                .trim_matches(|c| c == '"' || c == '\'')
                .to_string()
        })
        .collect()
}

fn forbidden_cargo_key(key: &[String], inline: bool) -> bool {
    let head = key.first().map_or("", String::as_str);
    match head {
        "profile" | "unstable" => true,
        "build" => {
            key.len() == 1
                || [
                    "rustflags",
                    "rustc",
                    "rustc-wrapper",
                    "rustc-workspace-wrapper",
                    "incremental",
                ]
                .contains(&key[1].as_str())
        }
        "target" if key.len() >= 2 => {
            let selector = &key[1];
            let firmware = selector.starts_with("cfg(") || TARGETS.contains(&selector.as_str());
            firmware && ((key.len() == 2 && inline) || (key.len() > 2 && key[2] == "rustflags"))
        }
        "target" => inline,
        _ => false,
    }
}

/// The cargo environment, refusing output-affecting overrides in it or in
/// any cargo configuration file that applies.
pub(crate) fn rust_environment(root: &Path, environment: Environment) -> Result<Environment> {
    let names = forbidden_variables(&environment);
    require(
        names.is_empty(),
        format!(
            "Unset compiler/profile overrides before a firmware build: {}",
            names.join(", ")
        ),
    )?;

    for path in cargo_config_files(root, &environment) {
        let found = cargo_config_overrides(&String::from_utf8_lossy(&fs::read(&path)?));
        require(
            found.is_empty(),
            format!(
                "Cargo configuration overrides firmware compilation in {}: {}",
                path.display(),
                found.join(", ")
            ),
        )?;
    }
    Ok(environment)
}

/// This process's environment.
pub(crate) fn process_environment() -> Environment {
    std::env::vars().collect()
}

/// The rustc cargo resolves from the root, which must be the pinned channel.
/// Recorded in the manifest and compared again after the build.
pub(crate) fn rust_compiler(repo: &Repo, environment: &Environment) -> Result<RustCompiler> {
    static CHANNEL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?m)^channel\s*=\s*"([^"]+)""#).expect("literal pattern"));
    static RELEASE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)^release:\s*(\S+)$").expect("literal pattern"));

    let pinned = fs::read_to_string(repo.path("rust-toolchain.toml"))?;
    let verbose = text(
        Command::new("rustc")
            .arg("-vV")
            .current_dir(&repo.root)
            .env_clear()
            .envs(environment),
    )?;
    let channel = CHANNEL.captures(&pinned).map(|c| c[1].to_string());
    let release = RELEASE.captures(&verbose).map(|c| c[1].to_string());
    require(
        channel.is_some() && channel == release,
        "rustc selected from the repository does not match rust-toolchain.toml",
    )?;

    let path = match which("rustup", environment.get("PATH").map(String::as_str)) {
        Some(_) => Some(
            text(
                Command::new("rustup")
                    .args(["which", "rustc"])
                    .current_dir(&repo.root)
                    .env_clear()
                    .envs(environment),
            )?
            .trim()
            .to_string(),
        ),
        None => None,
    };
    let version = verbose
        .lines()
        .next()
        .ok_or_else(|| Error::Refused("rustc printed nothing".into()))?;
    Ok(RustCompiler {
        version: version.to_string(),
        verbose: verbose.trim().lines().map(str::to_string).collect(),
        path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean() -> Environment {
        [("PATH", "/usr/bin"), ("HOME", "/nonexistent")]
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .into()
    }

    #[test]
    fn output_affecting_overrides_are_refused() {
        let root = tempfile::tempdir().unwrap();
        assert!(rust_environment(root.path(), clean()).is_ok());

        for name in [
            "RUSTFLAGS",
            "CARGO_ENCODED_RUSTFLAGS",
            "RUSTC_WRAPPER",
            "RUSTUP_TOOLCHAIN",
            "CARGO_PROFILE_FIRMWARE_OPT_LEVEL",
            "CARGO_TARGET_THUMBV8M_MAIN_NONE_EABIHF_RUSTFLAGS",
        ] {
            let mut environment = clean();
            environment.insert(name.into(), "x".into());
            let error = rust_environment(root.path(), environment).unwrap_err();
            assert!(error.to_string().contains(name), "{name}: {error}");
        }

        let mut host = clean();
        host.insert("CARGO_BUILD_TARGET_DIR".into(), "/tmp/x".into());
        host.insert(
            "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS".into(),
            "-Clink-arg=x".into(),
        );
        assert!(rust_environment(root.path(), host).is_ok());
    }

    #[test]
    fn cargo_configuration_overrides() {
        let allowed = "# [build]\n# rustc-wrapper = \"sccache\"\n[target.x86_64-unknown-linux-gnu]\n\
                       linker = \"clang\"\nrustflags = [\"-Clink-arg=--ld-path=mold\"]\n[build]\njobs = 4\n\
                       [target.'thumbv8m.main-none-eabihf']\nrunner = \"probe\"\n";
        assert!(cargo_config_overrides(allowed).is_empty());

        for (text, key) in [
            (
                "[build]\nrustflags = [\"-Copt-level=0\"]\n",
                "build.rustflags",
            ),
            ("build.rustc-wrapper = \"sccache\"\n", "build.rustc-wrapper"),
            ("build = { rustflags = [\"x\"] }\n", "build"),
            (
                "[profile.firmware]\nopt-level = 0\n",
                "profile.firmware.opt-level",
            ),
            (
                "[target.'thumbv8m.base-none-eabi']\nrustflags = [\n  \"x\",\n]\n",
                "target.thumbv8m.base-none-eabi.rustflags",
            ),
            (
                "[target.'cfg(target_os = \"none\")']\nrustflags = [\"x\"]\n",
                "target.cfg(target_os = \"none\").rustflags",
            ),
            ("[unstable]\nbuild-std = [\"core\"]\n", "unstable.build-std"),
        ] {
            assert_eq!(cargo_config_overrides(text), [key], "{text}");
        }
    }

    #[test]
    fn configuration_files_are_refused_with_their_path() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join(".cargo");
        fs::create_dir(&config).unwrap();
        fs::write(
            config.join("config.toml"),
            "[profile.firmware]\nlto = false\n",
        )
        .unwrap();

        let error = rust_environment(root.path(), clean())
            .unwrap_err()
            .to_string();
        assert!(error.contains("profile.firmware.lto"), "{error}");
        assert!(error.contains("config.toml"), "{error}");
    }
}
