//! The repository check behind `cargo xtask check-repository`.
//!
//! It scans the files Git would publish (tracked plus untracked-but-not-ignored),
//! so ignored local evidence is never read. Every finding is collected into
//! [`Summary::errors`] instead of stopping at the first, so one run reports
//! everything; only failing to resolve or list the checkout is an `Err`.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

use regex::Regex;

// Top-level directories that hold local inputs or agent state.
const UNPUBLISHED: [&str; 6] = ["private", ".venv", "target", ".aws", ".codex", ".agents"];
const EVIDENCE: [&str; 5] = ["bin", "pcap", "pcapng", "img", "apk"];
const TEXT: [&str; 10] = [
    "md", "rs", "py", "toml", "json", "yml", "txt", "c", "h", "ts",
];
/// Product code, which reaches the motor pins, the watchdog and flash only
/// through the HAL's owners: it may name neither the raw writers nor the
/// `bringup` module that re-exports them.
const PRODUCT: [&str; 2] = ["src/firmware/wellfed/feeder/src/", "src/product/"];
/// Bring-up code, which may take the raw writers, but only as `bringup::…`,
/// so the one door stays visible at every call.
const BRINGUP: [&str; 2] = ["src/firmware/wellfed/", "src/sdk/wellfed-probes/src/"];
/// Sources under `src/` that must not cite a board card.
const CARD_FREE: [&str; 6] = ["rs", "c", "h", "toml", "md", "ts"];

macro_rules! regex {
    ($name:ident, $pattern:expr) => {
        static $name: LazyLock<Regex> =
            LazyLock::new(|| Regex::new($pattern).expect("check pattern"));
    };
}

regex!(USER_PATH, r"/(?:home|Users|mnt|media)/[A-Za-z0-9_.-]+/");
regex!(DATED, r#"private/[^\s"']*202610\d{2}"#);
// A board card's id: the firmware or feature board's prefix and a number.
// Pinned policy names are lowercase and stay.
regex!(CARD, r"\b(?:FW|FP)\d{1,3}[a-z]?\b");

// The raw output writers `wellfed-amebad-sys` marks `unsafe`; reads such
// as `wdg_vendor` and `bank1_read` are not among them.
regex!(
    RAW_WRITER,
    r"(\bbringup::)?\b(gpio_init|gpio_port_write|gpio_write_bit|wdg_init|wdg_cmd|wdg_refresh|storage_erase|storage_program|bank1_erase|bank1_program|bank1_give_way)\b"
);
regex!(BRINGUP_NAME, r"\bbringup\b");

regex!(LINK, r"\]\(([^)]+)\)");
regex!(SCHEME, r"^[A-Za-z][A-Za-z0-9+.-]*:");

/// What one run found: the counts for the success line, and every finding.
pub(crate) struct Summary {
    pub files: usize,
    pub links: usize,
    pub errors: Vec<String>,
}

/// Tracked and untracked-but-not-ignored files, as Git lists them.
fn listed(root: &Path) -> io::Result<Vec<String>> {
    let output = Command::new("git")
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .current_dir(root)
        .output()?;

    if !output.status.success() {
        return Err(io::Error::other("git ls-files failed"));
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect())
}

/// Run every check over `root`. An `Err` means the checkout could not be listed;
/// failed checks are reported in [`Summary::errors`].
pub(crate) fn check(root: &Path) -> io::Result<Summary> {
    let root = root.canonicalize()?;
    let names = listed(&root)?;
    let mut summary = Summary {
        files: names.len(),
        links: 0,
        errors: Vec::new(),
    };

    for name in &names {
        check_file(&root, name, &mut summary);
    }

    Ok(summary)
}

fn check_file(root: &Path, name: &str, summary: &mut Summary) {
    let relative = Path::new(name);
    let first = relative
        .components()
        .next()
        .and_then(|c| c.as_os_str().to_str())
        .unwrap_or("");

    if UNPUBLISHED.contains(&first) {
        summary.errors.push(format!(
            "{name}: local inputs or agent state must not be published"
        ));
        return;
    }

    let extension = relative.extension().and_then(|e| e.to_str()).unwrap_or("");
    // The Python tools were ported to Rust and removed (October 23, 2026).
    if extension == "py" && first == "src" {
        summary.errors.push(format!(
            "{name}: first-party tools are Rust; no Python sources under src/"
        ));
    }
    if EVIDENCE.contains(&extension.to_lowercase().as_str()) {
        summary.errors.push(format!(
            "{name}: binary/evidence files need explicit provenance review"
        ));
    }
    if !TEXT.contains(&extension) {
        return;
    }

    let path = root.join(relative);
    let Ok(bytes) = fs::read(&path) else { return };
    let content = String::from_utf8_lossy(&bytes);

    if USER_PATH.is_match(&content) {
        summary
            .errors
            .push(format!("{name}: user-specific absolute path"));
    }
    if DATED.is_match(&content) {
        summary
            .errors
            .push(format!("{name}: dated private fixture path"));
    }
    // Comments carry the constraint; a card's history lives on the boards.
    if first == "src" && CARD_FREE.contains(&extension) && CARD.is_match(&content) {
        summary.errors.push(format!(
            "{name}: cites a board card; state the constraint instead"
        ));
    }

    if extension == "rs" {
        raw_outputs(name, &content, &mut summary.errors);
    }

    if extension == "md" {
        // Inline links only; external references are provenance links, not fetched.
        for capture in LINK.captures_iter(&content) {
            let target = capture[1].trim().trim_matches(['<', '>']);
            let Some(local) = local_path(target) else {
                continue;
            };
            summary.links += 1;
            let dest = normalize(&path.parent().unwrap_or(root).join(local));
            if !dest.starts_with(root) || !dest.exists() {
                summary
                    .errors
                    .push(format!("{name}: missing/nonlocal link {target}"));
            }
        }
    }
}

/// The local file a Markdown link names: no scheme, no host, a nonempty path.
fn local_path(target: &str) -> Option<String> {
    if SCHEME.is_match(target) || target.starts_with("//") {
        return None;
    }
    let path = target.split(['#', '?']).next().unwrap_or("");
    (!path.is_empty()).then(|| unquote(path))
}

/// Percent-decoding, as `urllib.parse.unquote`.
fn unquote(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(value) = text
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(value);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Resolve symlinks where the path exists, and `..` lexically where it does not.
fn normalize(path: &Path) -> PathBuf {
    if let Ok(real) = path.canonicalize() {
        return real;
    }
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Product code names no raw writer and no `bringup`; bring-up code names a
/// raw writer only through `bringup::`. Comments are not code.
fn raw_outputs(name: &str, source: &str, errors: &mut Vec<String>) {
    let product = PRODUCT.iter().any(|root| name.starts_with(root));
    let bringup = !product && BRINGUP.iter().any(|root| name.starts_with(root));
    if !product && !bringup {
        return;
    }

    for line in source.lines() {
        let code = line.split("//").next().unwrap_or("");
        if product && BRINGUP_NAME.is_match(code) {
            errors.push(format!(
                "{name}: product code may not use the bringup module"
            ));
        }
        for capture in RAW_WRITER.captures_iter(code) {
            let writer = &capture[2];
            if product {
                errors.push(format!(
                    "{name}: product code may not call {writer}; use the HAL's owner"
                ));
            } else if capture.get(1).is_none() {
                errors.push(format!(
                    "{name}: bring-up code calls {writer} only as bringup::{writer}"
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_link_targets() {
        assert_eq!(local_path("docs/a.md#part").as_deref(), Some("docs/a.md"));
        assert_eq!(local_path("a%20b.md").as_deref(), Some("a b.md"));
        for skipped in ["https://example.com/x", "mailto:x@y", "#anchor", "//host/x"] {
            assert_eq!(local_path(skipped), None, "{skipped}");
        }
    }

    #[test]
    fn raw_outputs_rules() {
        let mut errors = Vec::new();
        let feeder = "src/firmware/wellfed/feeder/src/app.rs";
        raw_outputs(
            feeder,
            "// gpio_port_write is the board's\nlet v = sdk::wdg_vendor();\nlet e = h.storage_erases;\n",
            &mut errors,
        );
        assert!(errors.is_empty(), "{errors:?}");

        raw_outputs(
            feeder,
            "unsafe { wellfed_sdk::hal::sdk::gpio_port_write(0, MASK, 0) };\nuse wellfed_sdk::hal::bringup;\n",
            &mut errors,
        );
        raw_outputs(
            "src/sdk/wellfed-probes/src/buttons.rs",
            "unsafe { bringup::gpio_init(pin, false) };\nunsafe { sdk::wdg_refresh() };\n",
            &mut errors,
        );
        raw_outputs(
            "src/sdk/wellfed-hal/src/board.rs",
            "unsafe { sdk::gpio_port_write(0, 0, 0) };\n",
            &mut errors,
        );

        assert_eq!(
            errors,
            [
                "src/firmware/wellfed/feeder/src/app.rs: product code may not call gpio_port_write; use the HAL's owner",
                "src/firmware/wellfed/feeder/src/app.rs: product code may not use the bringup module",
                "src/sdk/wellfed-probes/src/buttons.rs: bring-up code calls wdg_refresh only as bringup::wdg_refresh",
            ]
        );
    }

    #[test]
    fn published_paths_links_and_sources() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        fs::create_dir_all(root.join("src/tools")).unwrap();
        fs::write(root.join("src/tools/a.py"), "print()\n").unwrap();
        fs::write(
            root.join("README.md"),
            "[ok](src/tools/a.py) [gone](missing.md) [web](https://x)\n",
        )
        .unwrap();

        let mut summary = Summary {
            files: 0,
            links: 0,
            errors: Vec::new(),
        };
        let card = ["FW", "16"].concat();
        fs::write(root.join("src/tools/b.rs"), format!("// {card}\n")).unwrap();
        fs::write(root.join("src/tools/c.rs"), "// fw16-pulse-v1\n").unwrap();
        for name in [
            "README.md",
            "private/x.json",
            "dump.bin",
            "src/tools/a.py",
            "src/tools/b.rs",
            "src/tools/c.rs",
        ] {
            check_file(&root, name, &mut summary);
        }

        assert_eq!(summary.links, 2);
        assert_eq!(
            summary.errors,
            [
                "README.md: missing/nonlocal link missing.md",
                "private/x.json: local inputs or agent state must not be published",
                "dump.bin: binary/evidence files need explicit provenance review",
                "src/tools/a.py: first-party tools are Rust; no Python sources under src/",
                "src/tools/b.rs: cites a board card; state the constraint instead",
            ]
        );
    }
}
