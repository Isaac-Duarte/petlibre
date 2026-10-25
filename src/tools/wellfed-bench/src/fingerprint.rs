//! The fingerprint's crate list (`build/crates.rs`) against what Cargo
//! resolves: a first-party crate `wellfed-itest` links but the list misses
//! would let a stale suite binary through.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

include!("../build/crates.rs");

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
    resolve: Resolve,
}

#[derive(Deserialize)]
struct Package {
    id: String,
    name: String,
    manifest_path: PathBuf,
    /// `None` for path dependencies: the first-party crates.
    source: Option<String>,
}

#[derive(Deserialize)]
struct Resolve {
    nodes: Vec<Node>,
}

#[derive(Deserialize)]
struct Node {
    id: String,
    deps: Vec<NodeDep>,
}

#[derive(Deserialize)]
struct NodeDep {
    pkg: String,
    dep_kinds: Vec<DepKind>,
}

#[derive(Deserialize)]
struct DepKind {
    /// `None` for a normal dependency.
    kind: Option<String>,
}

/// The directories of the first-party packages `wellfed-itest` links
/// through normal and build dependencies, with `features`.
fn linked(features: &[&str]) -> BTreeSet<PathBuf> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../wellfed-itest/Cargo.toml");
    let output = Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1", "--locked", "--offline"])
        .arg("--manifest-path")
        .arg(&manifest)
        .args(features.iter().flat_map(|feature| ["--features", feature]))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: Metadata = serde_json::from_slice(&output.stdout).unwrap();

    let packages: BTreeMap<&str, &Package> = metadata
        .packages
        .iter()
        .map(|p| (p.id.as_str(), p))
        .collect();
    let nodes: BTreeMap<&str, &Node> = metadata
        .resolve
        .nodes
        .iter()
        .map(|n| (n.id.as_str(), n))
        .collect();
    let root = metadata
        .packages
        .iter()
        .find(|p| p.name == "wellfed-itest")
        .unwrap();

    let mut seen = BTreeSet::new();
    let mut queue = vec![root.id.as_str()];
    while let Some(id) = queue.pop() {
        if !seen.insert(id) {
            continue;
        }
        for dep in &nodes[id].deps {
            let linked = dep
                .dep_kinds
                .iter()
                .any(|kind| kind.kind.as_deref() != Some("dev"));
            if linked {
                queue.push(&dep.pkg);
            }
        }
    }

    seen.into_iter()
        .map(|id| packages[id])
        .filter(|package| package.source.is_none())
        .map(|package| {
            let directory = package.manifest_path.parent().unwrap();
            directory.canonicalize().unwrap()
        })
        .collect()
}

#[test]
fn the_fingerprint_covers_every_first_party_crate_the_suite_links() {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let listed: BTreeSet<PathBuf> = CRATES
        .iter()
        .map(|krate| here.join(krate).canonicalize().unwrap())
        .collect();

    // With the page built in, the list is exactly what links; without it,
    // the dashboard drops out and nothing else is missing.
    assert_eq!(linked(&["ui"]), listed);
    let plain = linked(&[]);
    assert!(plain.is_subset(&listed), "{:?}", plain.difference(&listed));
    assert!(!plain.contains(&here.join("../wellfed-dashboard").canonicalize().unwrap()));
}
