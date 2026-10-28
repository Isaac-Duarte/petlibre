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
const PRODUCT: [&str; 2] = ["src/firmware/wellfed/feeder/src/", PRODUCT_CRATES];
/// The product crates, whose top-level `tests/` may name the raw writers:
/// they write the SDK fake's flash to set up what a store reads.
const PRODUCT_CRATES: &str = "src/product/";
/// Bring-up code, which may take the raw writers, but only as `bringup::…`,
/// so the one door stays visible at every call.
const BRINGUP: [&str; 2] = ["src/firmware/wellfed/", "src/sdk/wellfed-probes/src/"];
/// The stock server's crate, which runs only as its own process: the bench,
/// its suite and the page speak its loopback API through `wellfed-stock-api`.
/// cargo-deny cannot ban every dependent without banning the crate itself.
const STOCK_SERVER: &str = "src/tools/wellfed-stock/Cargo.toml";
/// The flash client's crate, the audited UART writer: every read and write
/// runs it as its own process, so a fault elsewhere never takes a write down
/// with it. What other crates check of images and reviews is `wellfed-image`.
const FLASH_CLIENT: &str = "src/tools/wellfed-flash/Cargo.toml";
/// The server crates' clippy.toml, which Clippy reads instead of the root's
/// (it takes the nearest file and merges none), so it must repeat every root
/// setting before adding its own.
const SERVER_CLIPPY: &str = "src/server/clippy.toml";
/// The server's release build, which names its own Rust image and
/// toolchain: they must be the one `rust-toolchain.toml` pins, or a release
/// builds with a compiler `make check` never ran.
const SERVER_CONTAINERFILE: &str = "src/server/wellfed-server/packaging/Containerfile";
/// The tree whose Rust sources are held to a size, so a domain that grows
/// gets a module of its own rather than a longer file.
const SIZED: &str = "src/server/";
/// The most lines a Rust source under [`SIZED`] may have.
const LINES: usize = 400;
/// The same under a `tests/` directory, where each test keeps its setup
/// and its assertions together.
const TEST_LINES: usize = 600;
/// Sources under `src/` that must not cite a board card.
const CARD_FREE: [&str; 6] = ["rs", "c", "h", "toml", "md", "ts"];
/// The one tracked page that may name cards: it tells agents how the
/// ignored boards are kept. Every other page states the fact instead.
const CARD_PAGE: &str = "AGENTS.md";

macro_rules! regex {
    ($name:ident, $pattern:expr) => {
        static $name: LazyLock<Regex> =
            LazyLock::new(|| Regex::new($pattern).expect("check pattern"));
    };
}

regex!(USER_PATH, r"/(?:home|Users|mnt|media)/[A-Za-z0-9_.-]+/");
regex!(DATED, r#"private/[^\s"']*202610\d{2}"#);
// A board card's id: a board's prefix and a number. Pinned policy names
// are lowercase and stay.
regex!(CARD, r"\b(?:FW|FP|SV|AR)\d{1,3}[a-z]?\b");
regex!(CHANNEL, r#"(?m)^channel\s*=\s*"([^"]+)""#);

// The raw output writers `wellfed-amebad-sys` marks `unsafe`; reads such
// as `wdg_vendor` and `bank1_read` are not among them.
regex!(
    RAW_WRITER,
    r"(\bbringup::)?\b(gpio_init|gpio_port_write|gpio_write_bit|wdg_init|wdg_cmd|wdg_refresh|storage_erase|storage_program|bank1_erase|bank1_program|bank1_give_way)\b"
);
regex!(BRINGUP_NAME, r"\bbringup\b");
// A manifest line depending on the stock server's crate (not its API client):
// as a key, as a `[dependencies.…]` table, or renamed through `package`.
regex!(
    STOCK_DEPENDENCY,
    r#"(?m)^\s*(?:wellfed-stock\s*[=.]|\[[^\]#]*\.wellfed-stock\s*\]|[^#\n]*\bpackage\s*=\s*"wellfed-stock")"#
);
// The same for the flash client's crate.
regex!(
    FLASH_DEPENDENCY,
    r#"(?m)^\s*(?:wellfed-flash\s*[=.]|\[[^\]#]*\.wellfed-flash\s*\]|[^#\n]*\bpackage\s*=\s*"wellfed-flash")"#
);

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
    check_release_toolchain(&root, &mut summary);

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
    // Comments and docs carry the constraint; a card's history lives on the
    // boards, which are not published.
    let cited = (first == "src" && CARD_FREE.contains(&extension))
        || (extension == "md" && name != CARD_PAGE);
    if cited && CARD.is_match(&content) {
        summary.errors.push(format!(
            "{name}: cites a board card; state the constraint instead"
        ));
    }

    if extension == "rs" {
        raw_outputs(name, &content, &mut summary.errors);
        if let Some(error) = oversized(name, &content) {
            summary.errors.push(error);
        }
    }
    if first == "src" && relative.file_name().is_some_and(|f| f == "Cargo.toml") {
        if name != STOCK_SERVER && STOCK_DEPENDENCY.is_match(&content) {
            summary.errors.push(format!(
                "{name}: links the stock server; run wellfed-stock and use wellfed-stock-api"
            ));
        }
        if name != FLASH_CLIENT && FLASH_DEPENDENCY.is_match(&content) {
            summary.errors.push(format!(
                "{name}: links the flash client; run wellfed-flash as a process and use wellfed-image"
            ));
        }
    }

    if name == SERVER_CLIPPY {
        match fs::read_to_string(root.join("clippy.toml")) {
            Ok(root_settings) => {
                for missing in missing_settings(&root_settings, &content) {
                    summary.errors.push(format!(
                        "{name}: lacks the root clippy.toml's `{missing}`; Clippy reads only the nearest file"
                    ));
                }
            }
            Err(error) => summary.errors.push(format!(
                "{name}: cannot read the root clippy.toml it must repeat: {error}"
            )),
        }
    } else if name.starts_with("src/server/")
        && relative
            .file_name()
            .is_some_and(|f| f == "clippy.toml" || f == ".clippy.toml")
    {
        summary.errors.push(format!(
            "{name}: would replace {SERVER_CLIPPY} and its one-clock rule; keep server settings there"
        ));
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

/// What the release Containerfile must say for `toolchain`'s channel (the
/// Rust image's tag and the toolchain it selects) and does not. A
/// toolchain file without a channel is reported as such.
fn unpinned_toolchain(containerfile: &str, toolchain: &str) -> Vec<String> {
    let Some(channel) = CHANNEL.captures(toolchain).map(|found| found[1].to_owned()) else {
        return vec!["a channel in rust-toolchain.toml".to_owned()];
    };

    [
        format!("rust:{channel}-alpine"),
        format!("RUSTUP_TOOLCHAIN={channel}"),
    ]
    .into_iter()
    .zip([
        names_image(containerfile, &channel),
        selects(containerfile, &channel),
    ])
    .filter(|(_, present)| !present)
    .map(|(wanted, _)| wanted)
    .collect()
}

/// A whole token naming `rust:<channel>-alpine`, after any registry path or
/// `ARG` name, with nothing but an Alpine release (`3.24`) after it, so channel
/// 1.99.1 is not met by 1.99.10.
fn names_image(containerfile: &str, channel: &str) -> bool {
    let image = format!("rust:{channel}-alpine");

    containerfile.split_whitespace().any(|token| {
        let reference = token.rsplit(['/', '=']).next().unwrap_or(token);
        reference
            .strip_prefix(image.as_str())
            .is_some_and(|alpine| alpine.chars().all(|c| c.is_ascii_digit() || c == '.'))
    })
}

/// A whole `RUSTUP_TOOLCHAIN=<channel>` token.
fn selects(containerfile: &str, channel: &str) -> bool {
    let selected = format!("RUSTUP_TOOLCHAIN={channel}");

    containerfile
        .split_whitespace()
        .any(|token| token == selected)
}

/// The release Containerfile builds with rust-toolchain.toml's channel. It is
/// read whether or not the listing has it, so a deleted or renamed
/// Containerfile fails the check instead of skipping it.
fn check_release_toolchain(root: &Path, summary: &mut Summary) {
    let read = |file: &str| fs::read_to_string(root.join(file));

    match (read(SERVER_CONTAINERFILE), read("rust-toolchain.toml")) {
        (Ok(containerfile), Ok(toolchain)) => {
            for missing in unpinned_toolchain(&containerfile, &toolchain) {
                summary.errors.push(format!(
                    "{SERVER_CONTAINERFILE}: lacks `{missing}`; build releases with rust-toolchain.toml's channel"
                ));
            }
        }
        (Err(error), _) | (_, Err(error)) => summary.errors.push(format!(
            "{SERVER_CONTAINERFILE}: cannot compare its toolchain with rust-toolchain.toml: {error}"
        )),
    }
}

/// The root's settings (its non-comment lines) that a nested clippy.toml lacks.
/// Lines are compared whole, which assumes every root setting fits on one
/// line; a multi-line array there would need a TOML-aware comparison.
fn missing_settings<'a>(root: &'a str, nested: &str) -> Vec<&'a str> {
    let nested: Vec<&str> = nested.lines().map(str::trim).collect();

    root.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter(|line| !nested.contains(line))
        .collect()
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

/// A Rust source under [`SIZED`] longer than its limit: [`TEST_LINES`] under
/// a `tests/` directory, [`LINES`] anywhere else (a `tests.rs` module beside
/// the code it tests included). Generated files (`openapi.json`, the sqlx
/// metadata) are not Rust, so their size follows what they describe.
fn oversized(name: &str, source: &str) -> Option<String> {
    let inside = name.strip_prefix(SIZED)?;
    let directories = Path::new(inside).parent()?;
    let limit = if directories.components().any(|c| c.as_os_str() == "tests") {
        TEST_LINES
    } else {
        LINES
    };

    let lines = source.lines().count();
    (lines > limit).then(|| {
        format!(
            "{name}: {lines} lines, over the {limit} a server source may have; split it by domain"
        )
    })
}

/// Product code names no raw writer and no `bringup`; bring-up code names a
/// raw writer only through `bringup::`. Comments are not code.
fn raw_outputs(name: &str, source: &str, errors: &mut Vec<String>) {
    let product_test = name
        .strip_prefix(PRODUCT_CRATES)
        .and_then(|rest| rest.split_once('/'))
        .is_some_and(|(_, inside)| inside.starts_with("tests/"));
    let product = !product_test && PRODUCT.iter().any(|root| name.starts_with(root));
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
        raw_outputs(
            "src/product/wellfed-feeder-runtime/tests/writer.rs",
            "unsafe { sdk::storage_erase(sector) };\n",
            &mut errors,
        );
        raw_outputs(
            "src/product/wellfed-feeder-runtime/src/writer.rs",
            "unsafe { sdk::storage_erase(sector) };\n",
            &mut errors,
        );
        raw_outputs(
            "src/product/wellfed-feeder-runtime/src/tests/fake.rs",
            "unsafe { sdk::storage_erase(sector) };\n",
            &mut errors,
        );

        assert_eq!(
            errors,
            [
                "src/firmware/wellfed/feeder/src/app.rs: product code may not call gpio_port_write; use the HAL's owner",
                "src/firmware/wellfed/feeder/src/app.rs: product code may not use the bringup module",
                "src/sdk/wellfed-probes/src/buttons.rs: bring-up code calls wdg_refresh only as bringup::wdg_refresh",
                "src/product/wellfed-feeder-runtime/src/writer.rs: product code may not call storage_erase; use the HAL's owner",
                "src/product/wellfed-feeder-runtime/src/tests/fake.rs: product code may not call storage_erase; use the HAL's owner",
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
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(root.join("docs/page.md"), format!("See {card}.\n")).unwrap();
        fs::write(root.join("AGENTS.md"), format!("Board card {card}.\n")).unwrap();
        fs::create_dir_all(root.join("src/tools/page")).unwrap();
        fs::write(
            root.join("src/tools/page/Cargo.toml"),
            "[dependencies]\nwellfed-stock-api = { path = \"../wellfed-stock-api\" }\nwellfed-stock.workspace = true\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("src/tools/client")).unwrap();
        fs::write(
            root.join("src/tools/client/Cargo.toml"),
            "[dependencies]\nwellfed-stock-api = { path = \"../wellfed-stock-api\" }\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("src/tools/auditor")).unwrap();
        fs::write(
            root.join("src/tools/auditor/Cargo.toml"),
            "[dependencies]\nwellfed-image.workspace = true\n\n[dev-dependencies]\nwellfed-flash = { path = \"../wellfed-flash\" }\n",
        )
        .unwrap();
        for name in [
            "README.md",
            "private/x.json",
            "dump.bin",
            "src/tools/a.py",
            "src/tools/b.rs",
            "src/tools/c.rs",
            "docs/page.md",
            "AGENTS.md",
            "src/tools/page/Cargo.toml",
            "src/tools/client/Cargo.toml",
            "src/tools/auditor/Cargo.toml",
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
                "docs/page.md: cites a board card; state the constraint instead",
                "src/tools/page/Cargo.toml: links the stock server; run wellfed-stock and use wellfed-stock-api",
                "src/tools/auditor/Cargo.toml: links the flash client; run wellfed-flash as a process and use wellfed-image",
            ]
        );
    }

    fn lines(count: usize) -> String {
        "x\n".repeat(count)
    }

    #[test]
    fn a_server_source_over_its_limit_is_named_with_its_count() {
        let source = "src/server/wellfed-store/src/lib.rs";
        let test = "src/server/wellfed-store/tests/store.rs";

        assert_eq!(oversized(source, &lines(400)), None);
        assert_eq!(
            oversized(source, &lines(401)).as_deref(),
            Some(
                "src/server/wellfed-store/src/lib.rs: 401 lines, over the 400 a server source may have; split it by domain"
            )
        );
        assert_eq!(oversized(test, &lines(600)), None);
        assert_eq!(
            oversized(test, &lines(601)).as_deref(),
            Some(
                "src/server/wellfed-store/tests/store.rs: 601 lines, over the 600 a server source may have; split it by domain"
            )
        );
        assert_eq!(
            oversized(
                "src/server/wellfed-server/tests/api/feeders.rs",
                &lines(600)
            ),
            None,
            "a module of an integration test is a test too"
        );
    }

    #[test]
    fn a_unit_test_file_beside_the_code_keeps_the_source_limit() {
        assert!(
            oversized("src/server/wellfed-service/src/tests.rs", &lines(401)).is_some(),
            "a tests.rs module under src/ is held to 400"
        );
        assert!(
            oversized("src/server/wellfed-service/src/tests/hub.rs", &lines(401)).is_none(),
            "a tests/ directory under src/ holds tests"
        );
        assert_eq!(
            oversized("src/client/wellfed-client/src/api.rs", &lines(900)),
            None
        );
        assert_eq!(
            oversized("src/tools/xtask/src/layers.rs", &lines(900)),
            None
        );
    }

    #[test]
    fn the_size_guard_reads_server_rust_sources_only() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        fs::create_dir_all(root.join("src/server/wellfed-server/src")).unwrap();
        fs::write(
            root.join("src/server/wellfed-server/src/lib.rs"),
            lines(401),
        )
        .unwrap();
        fs::write(
            root.join("src/server/wellfed-server/openapi.json"),
            lines(401),
        )
        .unwrap();
        let mut summary = Summary {
            files: 0,
            links: 0,
            errors: Vec::new(),
        };

        check_file(&root, "src/server/wellfed-server/src/lib.rs", &mut summary);
        check_file(
            &root,
            "src/server/wellfed-server/openapi.json",
            &mut summary,
        );

        assert_eq!(
            summary.errors,
            [
                "src/server/wellfed-server/src/lib.rs: 401 lines, over the 400 a server source may have; split it by domain"
            ]
        );
    }

    #[test]
    fn a_nested_clippy_toml_repeats_every_root_setting() {
        let root =
            "# comment\nallow-unwrap-in-tests = true\n\navoid-breaking-exported-api = false\n";
        let nested = "# its own comment\nallow-unwrap-in-tests = true\ndisallowed-methods = []\n";

        assert_eq!(
            missing_settings(root, nested),
            ["avoid-breaking-exported-api = false"]
        );
        assert!(missing_settings(root, &format!("{root}disallowed-methods = []\n")).is_empty());
    }

    #[test]
    fn the_release_build_uses_the_pinned_toolchain() {
        let toolchain = "[toolchain]\nchannel = \"1.99.1\"\nprofile = \"minimal\"\n";
        let pinned = "ARG RUST_IMAGE=docker.io/library/rust:1.99.1-alpine3.24\nENV RUSTUP_TOOLCHAIN=1.99.1 CARGO_INCREMENTAL=0\n";
        let stale = "ARG RUST_IMAGE=docker.io/library/rust:1.98.0-alpine3.24\nENV RUSTUP_TOOLCHAIN=1.99.1\n";
        let longer = "ARG RUST_IMAGE=docker.io/library/rust:1.99.10-alpine3.24\nENV RUSTUP_TOOLCHAIN=1.99.10\n";

        assert!(unpinned_toolchain(pinned, toolchain).is_empty());
        assert_eq!(unpinned_toolchain(stale, toolchain), ["rust:1.99.1-alpine"]);
        assert_eq!(
            unpinned_toolchain(longer, toolchain),
            ["rust:1.99.1-alpine", "RUSTUP_TOOLCHAIN=1.99.1"]
        );
        assert_eq!(
            unpinned_toolchain(pinned, "[toolchain]\n"),
            ["a channel in rust-toolchain.toml"]
        );
    }

    #[test]
    fn the_server_clippy_toml_needs_the_root_one_and_no_other_beside_it() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        fs::create_dir_all(root.join("src/server/wellfed-store")).unwrap();
        fs::write(root.join(SERVER_CLIPPY), "disallowed-methods = []\n").unwrap();
        fs::write(root.join("src/server/wellfed-store/clippy.toml"), "\n").unwrap();
        let mut summary = Summary {
            files: 0,
            links: 0,
            errors: Vec::new(),
        };

        check_file(&root, SERVER_CLIPPY, &mut summary);
        check_file(&root, "src/server/wellfed-store/clippy.toml", &mut summary);

        assert_eq!(summary.errors.len(), 2);
        assert!(
            summary.errors[0]
                .starts_with("src/server/clippy.toml: cannot read the root clippy.toml"),
            "{}",
            summary.errors[0]
        );
        assert_eq!(
            summary.errors[1],
            "src/server/wellfed-store/clippy.toml: would replace src/server/clippy.toml and its one-clock rule; keep server settings there"
        );
    }

    #[test]
    fn process_only_crates_are_found_in_every_manifest_form() {
        for manifest in [
            "wellfed-flash.workspace = true",
            "[dev-dependencies.wellfed-flash]\npath = \"../wellfed-flash\"",
            "writer = { package = \"wellfed-flash\", path = \"../wellfed-flash\" }",
        ] {
            assert!(FLASH_DEPENDENCY.is_match(manifest), "{manifest}");
        }
        for manifest in [
            "wellfed-image.workspace = true",
            "# wellfed-flash = { path = \"../wellfed-flash\" }",
            "description = \"runs wellfed-flash as a process\"",
        ] {
            assert!(!FLASH_DEPENDENCY.is_match(manifest), "{manifest}");
        }

        assert!(STOCK_DEPENDENCY.is_match("[dependencies.wellfed-stock]"));
        for manifest in [
            "[dependencies.wellfed-stock-api]",
            "api = { package = \"wellfed-stock-api\", path = \"x\" }",
        ] {
            assert!(!STOCK_DEPENDENCY.is_match(manifest), "{manifest}");
        }
    }
}
