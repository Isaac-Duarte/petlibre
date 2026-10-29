//! What the app takes from the rest of the repository: its theme constants,
//! generated from `assets/brand/tokens.json` (the file the web UI's CSS
//! also comes from, so the two never disagree on a color or a size), and
//! the Rust toolchain its Android build uses.
//!
//! `src/app/lib/theme/tokens.g.dart` is committed, so the app builds
//! without Rust. This test fails when it is stale;
//! `UPDATE_BINDINGS=1 cargo test -p wellfed-app-bridge --test tokens`
//! rewrites it.

use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::PathBuf;

use serde::Deserialize;

#[derive(Deserialize)]
struct Tokens {
    color: BTreeMap<String, String>,
    theme: Themes,
    font: Font,
    size: BTreeMap<String, u32>,
    space: BTreeMap<String, u32>,
    radius: BTreeMap<String, u32>,
    motion: BTreeMap<String, u32>,
}

#[derive(Deserialize)]
struct Themes {
    light: BTreeMap<String, String>,
    dark: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct Font {
    family: String,
    mono: String,
    weights: BTreeMap<String, u32>,
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// `text_muted` as Dart spells a field: `textMuted`.
fn camel(name: &str) -> String {
    let mut out = String::new();
    let mut upper = false;
    for c in name.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// `#2563EB` as a Dart `Color`.
fn color(hex: &str) -> String {
    let digits = hex
        .strip_prefix('#')
        .expect("a palette color starts with #");
    assert_eq!(digits.len(), 6, "{hex}: palette colors are #RRGGBB");
    format!("Color(0xFF{})", digits.to_uppercase())
}

fn generated(tokens: &Tokens) -> String {
    let mut out = String::from(
        "// Generated from assets/brand/tokens.json by\n\
         // src/app/rust/tests/tokens.rs; do not edit.\n\n\
         import 'package:flutter/widgets.dart';\n",
    );

    // Both themes name the same roles; the class lists them once.
    let roles: Vec<&String> = tokens.theme.light.keys().collect();
    assert_eq!(
        roles,
        tokens.theme.dark.keys().collect::<Vec<_>>(),
        "the light and dark themes name different roles"
    );
    out.push_str("\n/// A theme's color roles.\nclass ThemeTokens {\n  const ThemeTokens({\n");
    for role in &roles {
        writeln!(out, "    required this.{},", camel(role)).unwrap();
    }
    out.push_str("  });\n\n");
    for role in &roles {
        writeln!(out, "  final Color {};", camel(role)).unwrap();
    }
    out.push_str("}\n");

    for (name, theme) in [("light", &tokens.theme.light), ("dark", &tokens.theme.dark)] {
        writeln!(out, "\nconst ThemeTokens {name}Tokens = ThemeTokens(").unwrap();
        for (role, swatch) in theme {
            let hex = tokens
                .color
                .get(swatch)
                .unwrap_or_else(|| panic!("{name}.{role} names {swatch}, not in the palette"));
            writeln!(out, "  {}: {},", camel(role), color(hex)).unwrap();
        }
        out.push_str(");\n");
    }

    out.push_str("\nabstract final class Fonts {\n");
    writeln!(out, "  static const family = '{}';", tokens.font.family).unwrap();
    writeln!(out, "  static const mono = '{}';", tokens.font.mono).unwrap();
    for (name, weight) in &tokens.font.weights {
        writeln!(
            out,
            "  static const {} = FontWeight.w{weight};",
            camel(name)
        )
        .unwrap();
    }
    out.push_str("}\n");

    for (class, values, prefix) in [
        ("Sizes", &tokens.size, ""),
        ("Space", &tokens.space, "s"),
        ("Radii", &tokens.radius, ""),
    ] {
        writeln!(out, "\nabstract final class {class} {{").unwrap();
        for (name, value) in values {
            writeln!(
                out,
                "  static const double {prefix}{} = {value};",
                camel(name)
            )
            .unwrap();
        }
        out.push_str("}\n");
    }

    out.push_str("\nabstract final class Motion {\n");
    for (name, ms) in &tokens.motion {
        writeln!(
            out,
            "  static const {} = Duration(milliseconds: {ms});",
            camel(name)
        )
        .unwrap();
    }
    out.push_str("}\n");
    out
}

#[test]
fn the_dart_tokens_match_the_brand_file() {
    let source = std::fs::read_to_string(root().join("assets/brand/tokens.json")).unwrap();
    let tokens: Tokens = serde_json::from_str(&source).unwrap();
    let path = root().join("src/app/lib/theme/tokens.g.dart");
    let want = generated(&tokens);

    if std::env::var_os("UPDATE_BINDINGS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, want).unwrap();
        return;
    }
    let have = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        have == want,
        "{} is stale: run UPDATE_BINDINGS=1 cargo test -p wellfed-app-bridge --test tokens",
        path.display()
    );
}

/// Cargokit, which builds the bridge for Android, runs the toolchain its
/// own file names; it must be the one the workspace pins.
#[test]
fn cargokit_builds_with_the_pinned_toolchain() {
    let pinned = std::fs::read_to_string(root().join("rust-toolchain.toml")).unwrap();
    let channel = pinned
        .lines()
        .find_map(|line| line.strip_prefix("channel = "))
        .expect("rust-toolchain.toml names a channel")
        .trim_matches('"');
    let cargokit = std::fs::read_to_string(root().join("src/app/rust/cargokit.yaml")).unwrap();
    let named: Vec<&str> = cargokit
        .lines()
        .filter_map(|line| line.trim().strip_prefix("toolchain: "))
        .collect();
    assert_eq!(
        named,
        [channel, channel],
        "cargokit.yaml's debug and release toolchains"
    );
}
