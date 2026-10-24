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

/// FW08 no-dispense rule: network-facing transport code may name only these
/// paths (full matches), so nothing reachable from the network can touch
/// feeding, outputs, settings, flash or updates. Paths whose first segment is
/// capitalized (types, enum variants, Self) are local and always allowed.
const TRANSPORT_ISOLATION: [(&str, &[&str]); 2] = [
    (
        "src/sdk/petlibre-core/src/transport.rs",
        &[
            r"core::(fmt|str)(::\w+)*",
            r"fmt::\w+",
            // FW24: a `play` request names one built-in tune.
            r"crate::tone::Tune",
        ],
    ),
    (
        "src/sdk/petlibre-hal/src/transport.rs",
        &[
            r"core::fmt::Write",
            r"core::str::from_utf8",
            r"core::fmt::(Result|Formatter|Display)",
            r"petlibre_core::transport(::\w+)?",
            r"mqtt::\w+(::\w+)?",
            r"crate::health(::(Boot|Link|self))?",
            r"health::(snapshot|Link|Boot)(::default)?",
            r"crate::net_status",
            r"net_status::read",
            // FW21: a `time` request sets the wall clock, and nothing else.
            r"crate::clock",
            r"clock::sync",
            // FW25: queue a settings change for the writer; no flash access.
            r"crate::settings",
            r"settings::request",
            r"crate::network",
            r"network::request",
            // FW24: queue a built-in tune for the speaker; no other output.
            r"crate::audio",
            r"audio::request",
            // FP11: queue a panel-lock change for the panel's owner.
            r"crate::panel",
            r"panel::request",
            // FW45: random bytes for the session nonce; nothing else.
            r"crate::rng",
            r"rng::fill",
            // Signed requests: verification only (request_auth.rs).
            r"petlibre_core::request_auth",
            r"request_auth::(split|Session)(::(new|nonce_hex))?",
            r"crate::sdk(::(TcpRecv|TransportConfig|self))?",
            r"sdk::(tcp_connect|tcp_send|tcp_recv|tcp_close|uptime_ms|entropy|TcpRecv|TransportConfig)",
        ],
    ),
];

// Primitive paths such as `u32::MAX` name no module, so they never count as a reach.
const PRIMITIVES: [&str; 13] = [
    "u8", "u16", "u32", "u64", "usize", "i8", "i16", "i32", "i64", "isize", "bool", "char", "str",
];

// Top-level directories that hold local inputs or agent state.
const UNPUBLISHED: [&str; 6] = ["private", ".venv", "target", ".aws", ".codex", ".agents"];
const EVIDENCE: [&str; 5] = ["bin", "pcap", "pcapng", "img", "apk"];
const TEXT: [&str; 7] = ["md", "rs", "py", "toml", "json", "yml", "txt"];

macro_rules! regex {
    ($name:ident, $pattern:expr) => {
        static $name: LazyLock<Regex> =
            LazyLock::new(|| Regex::new($pattern).expect("check pattern"));
    };
}

regex!(PATH, r"\b([A-Za-z_]\w*(?:::\w+)+)");
regex!(USE_GROUP, r"\buse\s+([\w:]+)::\{([^}]*)\}");
regex!(
    FORBIDDEN,
    r"\b(?:unsafe|extern|mod|include!|static\s+mut)\b"
);
regex!(GLOB, r"\buse\s+[^;]*\*");

regex!(USER_PATH, r"/(?:home|Users|mnt|media)/[A-Za-z0-9_.-]+/");
regex!(DATED, r#"private/[^\s"']*202610\d{2}"#);

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

    transport_isolation(&root, &mut summary.errors);
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

pub(crate) fn transport_isolation(root: &Path, errors: &mut Vec<String>) {
    for (name, allowed) in TRANSPORT_ISOLATION {
        let Ok(source) = fs::read_to_string(root.join(name)) else {
            errors.push(format!("{name}: transport isolation target missing"));
            continue;
        };
        isolation(name, &source, allowed, errors);
    }
}

fn isolation(name: &str, source: &str, allowed: &[&str], errors: &mut Vec<String>) {
    let rules: Vec<Regex> = allowed
        .iter()
        .map(|r| Regex::new(&format!("^(?:{r})$")).expect("rule"))
        .collect();

    // Code only: comments may name other modules.
    let code: String = source
        .lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");

    if FORBIDDEN.is_match(&code) {
        errors.push(format!(
            "{name}: transport code may not declare modules, externs, unsafe or mutable statics"
        ));
    }
    if GLOB.is_match(&code) {
        errors.push(format!("{name}: transport code may not glob-import"));
    }

    let mut paths = Vec::new();
    for capture in USE_GROUP.captures_iter(&code) {
        let base = &capture[1];
        for item in capture[2].split(',') {
            let item = item.trim().split(" as ").next().unwrap_or("").trim();
            if !item.is_empty() {
                paths.push(if item == "self" {
                    base.to_string()
                } else {
                    format!("{base}::{item}")
                });
            }
        }
    }

    // Grouped imports were expanded above; drop them so their fragments are
    // not matched again as bare paths.
    let rest = USE_GROUP.replace_all(&code, "");
    paths.extend(PATH.captures_iter(&rest).map(|c| c[1].to_string()));

    for found in paths {
        let first = found.split("::").next().unwrap_or("");
        if found.starts_with(|c: char| c.is_uppercase()) || PRIMITIVES.contains(&first) {
            continue;
        }
        if !rules.iter().any(|rule| rule.is_match(&found)) {
            errors.push(format!("{name}: transport code reaches {found}"));
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
    fn transport_isolation_rules() {
        let allowed = &[r"core::(fmt|str)(::\w+)*", r"fmt::\w+"];
        let mut errors = Vec::new();

        isolation(
            "t.rs",
            "use core::fmt::{self, Write};\nfn f(x: u8) -> Result<(), fmt::Error> { Ok(()) } // crate::flash::erase\n",
            allowed,
            &mut errors,
        );
        assert!(errors.is_empty(), "{errors:?}");

        isolation(
            "t.rs",
            "use crate::flash;\nfn f() { crate::feed::start(); }\n",
            allowed,
            &mut errors,
        );

        assert_eq!(
            errors,
            [
                "t.rs: transport code reaches crate::flash",
                "t.rs: transport code reaches crate::feed::start"
            ]
        );

        let mut errors = Vec::new();
        isolation(
            "t.rs",
            "unsafe fn f() {}\nuse core::fmt::*;\n",
            allowed,
            &mut errors,
        );
        assert_eq!(errors.len(), 2);
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
        for name in ["README.md", "private/x.json", "dump.bin", "src/tools/a.py"] {
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
            ]
        );
    }
}
