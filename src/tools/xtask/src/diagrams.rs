//! The diagrams in `docs/`: each hand-drawn SVG and the Mermaid block that
//! stands in for it on GitHub.
//!
//! A ```` ```mermaid ```` block's first line names its drawing,
//! `%% svg: name.svg`, relative to the page. The docs site draws the SVG and
//! GitHub the Mermaid, so the two must keep saying the same thing.
//! `docs/diagrams.lock` records a SHA-256 of each block's body and of its
//! SVG. A change to either side fails the check until `cargo xtask
//! bless-diagrams` records them again, which is the author's statement that
//! the two still match.
//!
//! An SVG is styled only by the `.d-` classes the site's stylesheet defines,
//! so it follows light and dark: no colors of its own, no inline styles, no
//! scripts or external references.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use wellfed_formats::canonical::sha256;

pub(crate) const LOCK: &str = "docs/diagrams.lock";
/// The stylesheet whose `.d-` classes are every class a drawing may use.
const PALETTE: &str = "src/web/apps/docs/src/lib/docs.css";
const LOCK_HEADER: &str = "# Written by `cargo xtask bless-diagrams`: each drawing in docs/, then a\n\
# SHA-256 of the Mermaid block that stands in for it and of the SVG itself.\n";

macro_rules! regex {
    ($name:ident, $pattern:expr) => {
        static $name: LazyLock<Regex> =
            LazyLock::new(|| Regex::new($pattern).expect("diagram pattern"));
    };
}

regex!(MARKER, r"^%% svg: (\S+\.svg)\s*$");
regex!(PALETTE_CLASS, r"\.(d-[a-z-]+)");
regex!(CLASS, r#"\bclass\s*=\s*"([^"]*)""#);
regex!(
    COLOR,
    r#"\b(fill|stroke|stop-color|flood-color|lighting-color|color)\s*=\s*"([^"]*)""#
);
regex!(HREF, r#"\bhref\s*=\s*"([^"]*)""#);
regex!(STYLE, r"\bstyle\s*=|<style\b");

/// Why the diagrams fail the check, or cannot be blessed.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("{page}: a mermaid block must start with `%% svg: <file>.svg`")]
    Unmarked { page: String },
    #[error("{page}: its mermaid block names {svg}, which doesn't exist")]
    Missing { page: String, svg: String },
    #[error("{svg}: no mermaid block in docs/ stands in for it")]
    Unreferenced { svg: String },
    #[error("{svg}: {first} and {second} carry different mermaid for it")]
    Diverged {
        svg: String,
        first: String,
        second: String,
    },
    #[error(
        "{svg}: not in {LOCK}; check its mermaid shows the same thing, then run `cargo xtask bless-diagrams`"
    )]
    Unblessed { svg: String },
    #[error(
        "{svg}: {side} changed since it was blessed; check the two still show the same thing, then run `cargo xtask bless-diagrams`"
    )]
    Drifted { svg: String, side: Side },
    #[error("{LOCK}: lists {svg}, which no page draws")]
    Stale { svg: String },
    #[error("{LOCK}: line {line} is not `<svg> <mermaid sha256> <svg sha256>`")]
    Lock { line: usize },
    #[error("{svg}: {rule}")]
    Drawing { svg: String, rule: Rule },
    #[error("could not read {path}")]
    Read {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("could not write {LOCK}")]
    Write(#[source] io::Error),
}

/// Which side of a pair changed since it was blessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Side {
    Mermaid,
    Svg,
    Both,
}

impl fmt::Display for Side {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Mermaid => "its mermaid",
            Self::Svg => "the SVG",
            Self::Both => "both its mermaid and the SVG",
        })
    }
}

/// What a drawing may not contain, or must.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Rule {
    InlineStyle,
    Script,
    ForeignObject,
    OwnColor { attribute: String },
    ExternalReference { href: String },
    UnknownClass { class: String },
    NoTitle,
    NoViewBox,
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InlineStyle => f.write_str("an inline style; use the .d- classes in docs.css"),
            Self::Script => f.write_str("a script"),
            Self::ForeignObject => f.write_str("a <foreignObject>"),
            Self::OwnColor { attribute } => write!(
                f,
                "sets its own {attribute}; colors come from the .d- classes, so it follows light and dark"
            ),
            Self::ExternalReference { href } => {
                write!(f, "references {href}; only #ids inside the drawing")
            }
            Self::UnknownClass { class } => {
                write!(
                    f,
                    "class {class} is not one of the .d- classes in {PALETTE}"
                )
            }
            Self::NoTitle => f.write_str("no <title>, which screen readers announce"),
            Self::NoViewBox => f.write_str("no viewBox, so it cannot scale to the column"),
        }
    }
}

/// One mermaid block: the page it is on, the drawing it names (a path from
/// the checkout's root) and its body, the lines after the marker. The marker
/// is left out because it is relative to its page, so pages in different
/// folders spell the same drawing differently.
#[derive(Debug, PartialEq, Eq)]
struct Block {
    page: String,
    svg: String,
    body: String,
}

/// The two hashes the lock records for a drawing.
#[derive(Debug, PartialEq, Eq)]
struct Hashes {
    mermaid: String,
    svg: String,
}

/// Every mermaid block on a page under `docs/`. Fences are tracked as
/// CommonMark does, so a mermaid fence quoted inside a longer one (an
/// example in the writing guide) is code, not a diagram. A block without a
/// marker is an error; one naming a path outside `docs/` resolves anyway and
/// fails as missing, since only `docs/` SVGs are loaded.
fn blocks(page: &str, text: &str, errors: &mut Vec<Error>) -> Vec<Block> {
    let directory = Path::new(page).parent().unwrap_or(Path::new(""));
    let mut found = Vec::new();
    let mut lines = text.lines();

    while let Some(line) = lines.next() {
        let Some((fence, info)) = opening(line) else {
            continue;
        };
        let body: Vec<&str> = lines.by_ref().take_while(|l| !closes(l, fence)).collect();
        if info != "mermaid" {
            continue;
        }

        let Some(name) = body
            .first()
            .and_then(|first| MARKER.captures(first))
            .map(|c| c[1].to_owned())
        else {
            errors.push(Error::Unmarked {
                page: page.to_owned(),
            });
            continue;
        };
        found.push(Block {
            page: page.to_owned(),
            svg: normalize(&directory.join(name)),
            body: body[1..].join("\n"),
        });
    }
    found
}

/// A line opening a fenced block: its fence (three or more backticks or
/// tildes, indented at most three spaces) and its info string.
fn opening(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let marker = trimmed.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let length = trimmed.chars().take_while(|c| *c == marker).count();
    (length >= 3).then(|| (&trimmed[..length], trimmed[length..].trim()))
}

/// A line closing `fence`: the same character, at least as many, nothing after.
fn closes(line: &str, fence: &str) -> bool {
    let trimmed = line.trim();
    let marker = fence.chars().next().unwrap_or('`');
    trimmed.len() >= fence.len() && trimmed.chars().all(|c| c == marker)
}

/// A relative path with `.` and `..` resolved lexically, `/`-separated.
fn normalize(path: &Path) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.iter().filter_map(|p| p.to_str()) {
        match part {
            "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// The `.d-` classes the stylesheet defines.
fn palette(css: &str) -> BTreeSet<String> {
    PALETTE_CLASS
        .captures_iter(css)
        .map(|c| c[1].to_owned())
        .collect()
}

/// Everything wrong with one drawing's source.
fn lint(svg: &str, palette: &BTreeSet<String>) -> Vec<Rule> {
    let mut rules = Vec::new();

    if STYLE.is_match(svg) {
        rules.push(Rule::InlineStyle);
    }
    if svg.contains("<script") {
        rules.push(Rule::Script);
    }
    if svg.contains("<foreignObject") {
        rules.push(Rule::ForeignObject);
    }
    for color in COLOR.captures_iter(svg) {
        if &color[2] != "none" {
            rules.push(Rule::OwnColor {
                attribute: color[1].to_owned(),
            });
        }
    }
    for href in HREF.captures_iter(svg) {
        if !href[1].starts_with('#') {
            rules.push(Rule::ExternalReference {
                href: href[1].to_owned(),
            });
        }
    }
    for list in CLASS.captures_iter(svg) {
        for class in list[1].split_whitespace() {
            if !palette.contains(class) {
                rules.push(Rule::UnknownClass {
                    class: class.to_owned(),
                });
            }
        }
    }
    if !svg.contains("<title>") && !svg.contains("<title ") {
        rules.push(Rule::NoTitle);
    }
    if !svg.contains("viewBox=") {
        rules.push(Rule::NoViewBox);
    }
    rules
}

fn parse_lock(text: &str, errors: &mut Vec<Error>) -> BTreeMap<String, Hashes> {
    let mut lock = BTreeMap::new();

    for (index, line) in text.lines().enumerate() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        let [svg, mermaid, drawing] = fields[..] else {
            errors.push(Error::Lock { line: index + 1 });
            continue;
        };
        lock.insert(
            svg.to_owned(),
            Hashes {
                mermaid: mermaid.to_owned(),
                svg: drawing.to_owned(),
            },
        );
    }
    lock
}

/// What the docs hold: every block, and each `docs/` SVG's bytes.
struct Sources {
    blocks: Vec<Block>,
    svgs: BTreeMap<String, Vec<u8>>,
    palette: BTreeSet<String>,
}

/// The pairs as they stand, keyed by drawing, with every error that is not
/// about the lock. Each drawing has one mermaid body, however many pages draw
/// it.
fn pairs(sources: &Sources, errors: &mut Vec<Error>) -> BTreeMap<String, Hashes> {
    let mut first: BTreeMap<&str, &Block> = BTreeMap::new();

    for block in &sources.blocks {
        if !sources.svgs.contains_key(&block.svg) {
            errors.push(Error::Missing {
                page: block.page.clone(),
                svg: block.svg.clone(),
            });
            continue;
        }
        match first.get(block.svg.as_str()) {
            Some(earlier) if earlier.body != block.body => errors.push(Error::Diverged {
                svg: block.svg.clone(),
                first: earlier.page.clone(),
                second: block.page.clone(),
            }),
            Some(_) => {}
            None => {
                first.insert(&block.svg, block);
            }
        }
    }

    for (svg, bytes) in &sources.svgs {
        if !first.contains_key(svg.as_str()) {
            errors.push(Error::Unreferenced { svg: svg.clone() });
        }
        for rule in lint(&String::from_utf8_lossy(bytes), &sources.palette) {
            errors.push(Error::Drawing {
                svg: svg.clone(),
                rule,
            });
        }
    }

    first
        .into_iter()
        .map(|(svg, block)| {
            let hashes = Hashes {
                mermaid: sha256(block.body.as_bytes()),
                svg: sources
                    .svgs
                    .get(svg)
                    .map_or_else(String::new, |b| sha256(b)),
            };
            (svg.to_owned(), hashes)
        })
        .collect()
}

/// Each pair against the lock.
fn compare(
    current: &BTreeMap<String, Hashes>,
    lock: &BTreeMap<String, Hashes>,
    errors: &mut Vec<Error>,
) {
    for (svg, now) in current {
        let Some(blessed) = lock.get(svg) else {
            errors.push(Error::Unblessed { svg: svg.clone() });
            continue;
        };
        let side = match (now.mermaid == blessed.mermaid, now.svg == blessed.svg) {
            (true, true) => continue,
            (false, true) => Side::Mermaid,
            (true, false) => Side::Svg,
            (false, false) => Side::Both,
        };
        errors.push(Error::Drifted {
            svg: svg.clone(),
            side,
        });
    }
    for svg in lock.keys().filter(|svg| !current.contains_key(*svg)) {
        errors.push(Error::Stale { svg: svg.clone() });
    }
}

fn read(root: &Path, name: &str) -> Result<String, Error> {
    fs::read_to_string(root.join(name)).map_err(|source| Error::Read {
        path: name.to_owned(),
        source,
    })
}

/// The pages and drawings under `docs/` among `names`, the files Git lists.
fn load(root: &Path, names: &[String], errors: &mut Vec<Error>) -> Result<Sources, Error> {
    let mut sources = Sources {
        blocks: Vec::new(),
        svgs: BTreeMap::new(),
        palette: palette(&read(root, PALETTE)?),
    };

    for name in names.iter().filter(|n| n.starts_with("docs/")) {
        if name.ends_with(".md") {
            let text = read(root, name)?;
            sources.blocks.extend(blocks(name, &text, errors));
        } else if name.ends_with(".svg") {
            let bytes = fs::read(root.join(name)).map_err(|source| Error::Read {
                path: name.clone(),
                source,
            })?;
            sources.svgs.insert(name.clone(), bytes);
        }
    }
    Ok(sources)
}

/// Every finding about the diagrams in `docs/`.
pub(crate) fn check(root: &Path, names: &[String]) -> Vec<Error> {
    let mut errors = Vec::new();

    let sources = match load(root, names, &mut errors) {
        Ok(sources) => sources,
        Err(error) => return vec![error],
    };
    let current = pairs(&sources, &mut errors);

    // No drawings and no lock is a docs tree without diagrams, not an error.
    let lock = match fs::read_to_string(root.join(LOCK)) {
        Ok(text) => parse_lock(&text, &mut errors),
        Err(error) if error.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
        Err(source) => {
            errors.push(Error::Read {
                path: LOCK.to_owned(),
                source,
            });
            return errors;
        }
    };
    compare(&current, &lock, &mut errors);

    errors
}

/// Record every pair's hashes in the lock. Refused while anything but the
/// lock is wrong, so a broken pair is never blessed.
pub(crate) fn bless(root: &Path, names: &[String]) -> Result<usize, Vec<Error>> {
    let mut errors = Vec::new();

    let sources = load(root, names, &mut errors).map_err(|error| vec![error])?;
    let current = pairs(&sources, &mut errors);
    if !errors.is_empty() {
        return Err(errors);
    }

    let mut text = String::from(LOCK_HEADER);
    for (svg, hashes) in &current {
        text.push_str(&format!("{svg} {} {}\n", hashes.mermaid, hashes.svg));
    }
    fs::write(root.join(LOCK), text).map_err(|error| vec![Error::Write(error)])?;

    Ok(current.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DRAWING: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><title>T</title><rect class="d-box"/></svg>"#;

    fn sources(pages: &[(&str, &str)], svgs: &[(&str, &str)]) -> (Sources, Vec<Error>) {
        let mut errors = Vec::new();
        let mut blocks_found = Vec::new();
        for (page, text) in pages {
            blocks_found.extend(blocks(page, text, &mut errors));
        }
        let sources = Sources {
            blocks: blocks_found,
            svgs: svgs
                .iter()
                .map(|(name, text)| ((*name).to_owned(), text.as_bytes().to_vec()))
                .collect(),
            palette: palette(".diagram .d-box { fill: x } .diagram .d-wire {}"),
        };
        (sources, errors)
    }

    fn page(marker: &str, body: &str) -> String {
        format!("# Page\n\n```mermaid\n{marker}\n{body}\n```\n\nText.\n")
    }

    #[test]
    fn a_block_names_its_drawing_relative_to_the_page() {
        let mut errors = Vec::new();
        let text = page("%% svg: ../shared/flow.svg", "flowchart LR\n  a --> b");

        let found = blocks("docs/firmware/page.md", &text, &mut errors);

        assert!(errors.is_empty());
        assert_eq!(
            found,
            [Block {
                page: "docs/firmware/page.md".to_owned(),
                svg: "docs/shared/flow.svg".to_owned(),
                body: "flowchart LR\n  a --> b".to_owned(),
            }]
        );
    }

    #[test]
    fn a_mermaid_fence_quoted_in_a_longer_one_is_code() {
        let mut errors = Vec::new();
        let text = "````markdown\n```mermaid\n%% svg: x.svg\nflowchart LR\n```\n````\n";

        let found = blocks("docs/a.md", text, &mut errors);

        assert!(
            errors.is_empty() && found.is_empty(),
            "{found:?} {errors:?}"
        );
    }

    #[test]
    fn a_block_without_a_marker_is_refused() {
        let mut errors = Vec::new();

        blocks("docs/a.md", &page("flowchart LR", "a --> b"), &mut errors);

        assert!(matches!(&errors[..], [Error::Unmarked { page }] if page == "docs/a.md"));
    }

    #[test]
    fn every_drawing_needs_a_block_and_every_block_a_drawing() {
        let (sources, mut errors) = sources(
            &[("docs/a.md", &page("%% svg: gone.svg", "flowchart LR"))],
            &[("docs/lonely.svg", DRAWING)],
        );

        pairs(&sources, &mut errors);

        assert!(matches!(&errors[0], Error::Missing { svg, .. } if svg == "docs/gone.svg"));
        assert!(matches!(&errors[1], Error::Unreferenced { svg } if svg == "docs/lonely.svg"));
        assert_eq!(errors.len(), 2);
    }

    #[test]
    fn pages_in_different_folders_share_a_drawing() {
        let (sources, mut errors) = sources(
            &[
                ("docs/a/one.md", &page("%% svg: x.svg", "flowchart LR")),
                ("docs/b/two.md", &page("%% svg: ../a/x.svg", "flowchart LR")),
            ],
            &[("docs/a/x.svg", DRAWING)],
        );

        let current = pairs(&sources, &mut errors);

        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(current.len(), 1);
    }

    #[test]
    fn two_pages_drawing_one_svg_carry_the_same_mermaid() {
        let (sources, mut errors) = sources(
            &[
                ("docs/a.md", &page("%% svg: x.svg", "flowchart LR")),
                ("docs/b.md", &page("%% svg: x.svg", "flowchart TB")),
            ],
            &[("docs/x.svg", DRAWING)],
        );

        pairs(&sources, &mut errors);

        assert!(
            matches!(&errors[..], [Error::Diverged { first, second, .. }] if first == "docs/a.md" && second == "docs/b.md")
        );
    }

    #[test]
    fn the_lock_names_the_side_that_changed() {
        let blessed = page("%% svg: x.svg", "flowchart LR");
        let (sources_then, mut errors) =
            sources(&[("docs/a.md", &blessed)], &[("docs/x.svg", DRAWING)]);
        let lock = pairs(&sources_then, &mut errors);
        assert!(errors.is_empty());

        let edited_svg = DRAWING.replace("T</title>", "U</title>");
        let cases = [
            (
                page("%% svg: x.svg", "flowchart TB"),
                DRAWING.to_owned(),
                Side::Mermaid,
            ),
            (blessed.clone(), edited_svg.clone(), Side::Svg),
            (
                page("%% svg: x.svg", "flowchart TB"),
                edited_svg,
                Side::Both,
            ),
        ];
        for (text, svg, expected) in cases {
            let (now, mut errors) = sources(&[("docs/a.md", &text)], &[("docs/x.svg", &svg)]);
            let current = pairs(&now, &mut errors);

            compare(&current, &lock, &mut errors);

            assert!(
                matches!(&errors[..], [Error::Drifted { side, .. }] if *side == expected),
                "{expected:?}: {errors:?}"
            );
        }
    }

    #[test]
    fn an_unblessed_pair_and_a_stale_lock_line_are_reported() {
        let (sources, mut errors) = sources(
            &[("docs/a.md", &page("%% svg: x.svg", "flowchart LR"))],
            &[("docs/x.svg", DRAWING)],
        );
        let current = pairs(&sources, &mut errors);
        let lock = parse_lock(
            "# header\ndocs/old.svg aa bb\nnot a lock line\n",
            &mut errors,
        );

        compare(&current, &lock, &mut errors);

        assert!(matches!(&errors[0], Error::Lock { line: 3 }));
        assert!(matches!(&errors[1], Error::Unblessed { svg } if svg == "docs/x.svg"));
        assert!(matches!(&errors[2], Error::Stale { svg } if svg == "docs/old.svg"));
    }

    #[test]
    fn a_drawing_takes_its_colors_from_the_palette() {
        let palette = palette(".diagram .d-box {} .diagram .d-wire {}");
        let svg = r##"<svg viewBox="0 0 1 1"><style>x</style><rect style="x" class="d-box d-red" fill="#f00" stroke="none"/><use href="https://x"/><use href="#arrow"/><script/></svg>"##;

        let rules = lint(svg, &palette);

        assert_eq!(
            rules,
            [
                Rule::InlineStyle,
                Rule::Script,
                Rule::OwnColor {
                    attribute: "fill".to_owned()
                },
                Rule::ExternalReference {
                    href: "https://x".to_owned()
                },
                Rule::UnknownClass {
                    class: "d-red".to_owned()
                },
                Rule::NoTitle,
            ]
        );
        assert!(lint(DRAWING, &palette).is_empty());
    }
}
