//! The pinned SDK checkout and Realtek toolchain: fetched once, verified on
//! every run.
//!
//! The archive's digest is pinned in `sdk.lock.json` and every executable
//! the build and audits invoke in `toolchain.lock.json`. Extraction goes to
//! a private staging directory and is renamed into place only once verified,
//! so an interrupted run never leaves a tree that a later run would accept.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::identity::TOOL_LOCK;
use crate::{Error, Lock, Repo, Result, digest, git, require, revision, run, text, which};

/// Per-file hashes of the executables the SDK build and audits invoke.
pub fn toolchain_lock(repo: &Repo, lock: &Lock) -> Result<BTreeMap<String, String>> {
    let pinned: Value = serde_json::from_slice(&fs::read(repo.path(TOOL_LOCK))?)?;
    let files: BTreeMap<String, String> = pinned["files"]
        .as_object()
        .map(|files| {
            files
                .iter()
                .filter_map(|(name, sum)| Some((name.clone(), sum.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default();
    require(
        pinned["archive_sha256"] == lock.toolchain_sha256.as_str() && !files.is_empty(),
        "Toolchain file pins do not describe the pinned SDK archive",
    )?;
    Ok(files)
}

/// Fail closed unless every pinned compiler/binutils file matches.
pub fn verify_toolchain(toolchain: &Path, pins: &BTreeMap<String, String>) -> Result<()> {
    let base = fs::canonicalize(toolchain)?;
    for (name, expected) in pins {
        let path = fs::canonicalize(toolchain.join(name)).ok();
        let Some(path) = path.filter(|p| p.starts_with(&base) && p.is_file()) else {
            return Err(Error::Refused(format!(
                "Pinned toolchain file is missing or escapes its directory: {name}"
            )));
        };
        require(
            &digest(&path)? == expected,
            format!("Pinned toolchain file changed: {name}; reinstall after review"),
        )?;
    }
    Ok(())
}

/// Lexically resolve `.`/`..` without touching the filesystem.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Refuse anything but files, directories and links that stay inside `base`,
/// as Python's tarfile data filter did.
fn check_tree(directory: &Path, base: &Path) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        let kind = fs::symlink_metadata(&path)?.file_type();
        if kind.is_symlink() {
            let target = fs::read_link(&path)?;
            let resolved = normalize(&path.parent().unwrap_or(base).join(&target));
            require(
                !target.is_absolute() && resolved.starts_with(base),
                format!(
                    "Toolchain archive link escapes its directory: {}",
                    path.display()
                ),
            )?;
        } else if kind.is_dir() {
            check_tree(&path, base)?;
        } else {
            require(
                kind.is_file(),
                format!("Toolchain archive has a special file: {}", path.display()),
            )?;
        }
    }
    Ok(())
}

/// Extract `directory` of `archive` into `destination` through a private
/// staging directory, verify it, then rename it into place.
pub fn extract_toolchain(
    archive: &Path,
    destination: &Path,
    directory: &str,
    pins: &BTreeMap<String, String>,
) -> Result<()> {
    let top = Path::new(directory)
        .components()
        .next()
        .map(|c| c.as_os_str().to_owned())
        .ok_or_else(|| Error::Refused("Empty toolchain directory".into()))?;
    let installed = destination.join(&top);
    require(
        fs::symlink_metadata(&installed).is_err(),
        format!(
            "Partial or unexpected toolchain tree; review and remove it: {}",
            installed.display()
        ),
    )?;

    // Only this run's staging directory is removed, also on failure.
    let staging = tempfile::Builder::new()
        .prefix(".extract-")
        .tempdir_in(destination)?;

    let names = text(Command::new("tar").arg("-tjf").arg(archive))?;
    require(
        !names.trim().is_empty()
            && names.lines().all(|name| {
                let path = Path::new(name);
                path.components()
                    .find(|c| *c != Component::CurDir)
                    .map(|c| c.as_os_str())
                    == Some(top.as_os_str())
                    && path
                        .components()
                        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
            }),
        "Toolchain archive has entries outside its pinned directory",
    )?;
    run(Command::new("tar")
        .args(["-xjf"])
        .arg(archive)
        .args(["--no-same-owner", "--no-same-permissions", "-C"])
        .arg(staging.path()))?;
    check_tree(staging.path(), staging.path())?;

    verify_toolchain(&staging.path().join(directory), pins)?;
    fs::rename(staging.path().join(&top), &installed)?;
    Ok(())
}

/// Fetch (once) and verify the pinned SDK checkout and toolchain; returns
/// the toolchain directory.
pub fn bootstrap(repo: &Repo) -> Result<PathBuf> {
    require(
        cfg!(target_os = "linux") && cfg!(target_arch = "x86_64"),
        "This initial toolchain setup supports Linux x86_64 only",
    )?;
    let path = std::env::var("PATH").ok();
    for tool in ["git", "make", "bash", "tar", "curl"] {
        require(
            which(tool, path.as_deref()).is_some(),
            format!("Missing host tool: {tool}"),
        )?;
    }

    let lock = repo.lock()?;
    let source = repo.source();
    if !source.exists() {
        fs::create_dir_all(source.parent().unwrap_or(&repo.root))?;
        run(Command::new("git").arg("init").arg(&source))?;
        git(&source, &["remote", "add", "origin", &lock.sdk_url])?;
        git(
            &source,
            &["fetch", "--depth", "1", "origin", &lock.sdk_revision],
        )?;
        git(&source, &["checkout", "--detach", "FETCH_HEAD"])?;
    }
    require(
        revision(&source)? == lock.sdk_revision,
        "SDK checkout does not match sdk.lock.json; left unchanged",
    )?;

    let toolchains = repo.toolchains();
    fs::create_dir_all(&toolchains)?;
    let name = lock
        .toolchain_url
        .rsplit('/')
        .next()
        .unwrap_or("toolchain.tar.bz2");
    let archive = toolchains.join(name);
    if !archive.exists() {
        let partial = toolchains.join(format!("{name}.partial"));
        println!("Downloading pinned Realtek toolchain...");
        run(Command::new("curl")
            .args(["--fail", "--location", "--silent", "--show-error"])
            .args(["--connect-timeout", "60", "--output"])
            .arg(&partial)
            .arg(&lock.toolchain_url))?;
        require(
            digest(&partial)? == lock.toolchain_sha256,
            format!("Toolchain checksum mismatch: {}", partial.display()),
        )?;
        fs::rename(&partial, &archive)?;
    }
    require(
        digest(&archive)? == lock.toolchain_sha256,
        format!("Toolchain checksum mismatch: {}", archive.display()),
    )?;

    let pins = toolchain_lock(repo, &lock)?;
    let toolchain = toolchains.join(&lock.toolchain_directory);
    if !toolchain.exists() {
        extract_toolchain(&archive, &toolchains, &lock.toolchain_directory, &pins)?;
    }
    verify_toolchain(&toolchain, &pins)?;

    let version = Command::new(toolchain.join("bin/arm-none-eabi-gcc"))
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .ok_or_else(|| {
            Error::Refused(
                "Cannot execute Realtek's i686 compiler; check the host's 32-bit \
                 Linux loader and runtime libraries"
                    .into(),
            )
        })?;
    println!(
        "{}",
        String::from_utf8_lossy(&version.stdout)
            .lines()
            .next()
            .unwrap_or_default()
    );
    Ok(toolchain)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sha256;

    const DIRECTORY: &str = "asdk-6.4.1/linux/newlib";

    fn pins() -> BTreeMap<String, String> {
        [("bin/arm-none-eabi-gcc".to_string(), sha256(b"gcc"))].into()
    }

    /// A bzip2 tarball of `members` (path, content), made by the host's tar.
    fn archive(base: &Path, members: &[(&str, &[u8])]) -> PathBuf {
        let tree = base.join("tree");
        let _ = fs::remove_dir_all(&tree);
        for (name, data) in members {
            let path = tree.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, data).unwrap();
        }
        let path = base.join("toolchain.tar.bz2");
        let mut tar = Command::new("tar");
        tar.arg("-cjf").arg(&path).arg("-C").arg(&tree);
        tar.args(members.iter().map(|(name, _)| name));
        run(&mut tar).unwrap();
        path
    }

    #[test]
    fn extraction_is_verified_and_atomic() {
        let base = tempfile::tempdir().unwrap();
        let destination = base.path().join("toolchains");
        fs::create_dir(&destination).unwrap();
        let archive = archive(
            base.path(),
            &[("asdk-6.4.1/linux/newlib/bin/arm-none-eabi-gcc", b"gcc")],
        );

        extract_toolchain(&archive, &destination, DIRECTORY, &pins()).unwrap();
        let names: Vec<_> = fs::read_dir(&destination)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["asdk-6.4.1"]);
        let toolchain = destination.join(DIRECTORY);
        verify_toolchain(&toolchain, &pins()).unwrap();

        // An existing (possibly partial) tree is never extracted over.
        let error = extract_toolchain(&archive, &destination, DIRECTORY, &pins()).unwrap_err();
        assert!(error.to_string().contains("Partial"));

        let gcc = toolchain.join("bin/arm-none-eabi-gcc");
        fs::write(&gcc, b"modified").unwrap();
        let error = verify_toolchain(&toolchain, &pins()).unwrap_err();
        assert!(error.to_string().contains("changed"));
        fs::remove_file(&gcc).unwrap();
        let error = verify_toolchain(&toolchain, &pins()).unwrap_err();
        assert!(error.to_string().contains("missing"));
    }

    #[test]
    fn wrong_or_escaping_archives_leave_no_tree() {
        let base = tempfile::tempdir().unwrap();
        let destination = base.path().join("toolchains");
        fs::create_dir(&destination).unwrap();

        let cases: [&[(&str, &[u8])]; 2] = [
            &[("asdk-6.4.1/linux/newlib/bin/arm-none-eabi-gcc", b"other")],
            &[
                ("asdk-6.4.1/linux/newlib/bin/arm-none-eabi-gcc", b"gcc"),
                ("elsewhere/x", b"x"),
            ],
        ];
        for members in cases {
            let archive = archive(base.path(), members);
            assert!(extract_toolchain(&archive, &destination, DIRECTORY, &pins()).is_err());
            assert_eq!(fs::read_dir(&destination).unwrap().count(), 0);
        }
    }

    #[test]
    fn links_out_of_the_tree_are_refused() {
        let base = tempfile::tempdir().unwrap();
        let inside = base.path().join("top");
        fs::create_dir_all(inside.join("bin")).unwrap();
        std::os::unix::fs::symlink("../../outside", inside.join("bin/escape")).unwrap();
        assert!(check_tree(base.path(), base.path()).is_ok());
        assert!(check_tree(&inside, &inside).is_err());
    }

    #[test]
    fn pins_must_describe_the_sdk_archive() {
        let repo = Repo::here().unwrap();
        let mut lock = repo.lock().unwrap();
        assert!(!toolchain_lock(&repo, &lock).unwrap().is_empty());
        lock.toolchain_sha256 = "0".repeat(64);
        assert!(toolchain_lock(&repo, &lock).is_err());
    }
}
