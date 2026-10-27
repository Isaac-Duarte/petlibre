//! The server's crate layering, checked from `cargo metadata`.
//!
//! The compiler keeps the server's layers apart only if each crate's
//! dependencies follow the drawn arrows, so a manifest change that adds a
//! shortcut fails here instead of in review. The rules and their reasons are
//! the API design's (docs/api/README.md#crates):
//!
//! - only `wellfed-store` depends on sqlx, so SQL stays in one crate;
//! - nothing below `wellfed-server` links axum, even through another crate;
//! - a server crate depends only on the workspace crates its arrows name, and
//!   a crate outside the server takes a server crate only where drawn;
//! - `wellfed-api` builds for the app's Android library, so its own
//!   dependencies come from an allow-list.
//!
//! The arrows name crates that do not exist yet, so a new crate is checked
//! from its first manifest. Dev-dependencies are exempt from the arrows (a
//! test may build a simulated feeder or a whole server), never from sqlx.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;
use std::process::Command;
use std::{env, io};

use serde::Deserialize;

/// The crate that owns the database.
const STORE: &str = "wellfed-store";
/// The HTTP crate every client of the server shares.
const API: &str = "wellfed-api";

/// Each server crate and the workspace crates it may depend on (normal and
/// build dependencies). A crate is named here before it exists, so its first
/// manifest is checked.
const ARROWS: [(&str, &[&str]); 8] = [
    (API, &["wellfed-model"]),
    (STORE, &[API]),
    (
        "wellfed-mirror",
        &[
            STORE,
            API,
            "wellfed-connect",
            "wellfed-client",
            "wellfed-model",
            "wellfed-proto",
            "wellfed-session",
        ],
    ),
    (
        "wellfed-releases",
        &[STORE, API, "wellfed-client", "wellfed-model"],
    ),
    (
        "wellfed-service",
        &[
            "wellfed-mirror",
            "wellfed-releases",
            STORE,
            API,
            "wellfed-model",
            "wellfed-formats",
        ],
    ),
    ("wellfed-server", &["wellfed-service", API, STORE]),
    (
        "wellfed-server-testkit",
        &[
            "wellfed-server",
            "wellfed-service",
            STORE,
            API,
            "wellfed-server-client",
            "wellfed-feeder-sim",
            "wellfed-client",
            "wellfed-connect",
            "wellfed-model",
            "wellfed-session",
            "wellfed-core",
        ],
    ),
    ("wellfed-server-client", &[API]),
];

/// The only edges into a server crate from a crate outside the server: the
/// CLI speaks the server's API through its typed client, and the app's
/// flutter_rust_bridge crate hands `wellfed-api`'s types to Dart.
const INTO_SERVER: [(&str, &str); 2] = [
    ("wellfed", "wellfed-server-client"),
    ("wellfed-app-bridge", API),
];

/// The crates below the binary, which must not link axum by any path.
const BELOW_SERVER: [&str; 6] = [
    API,
    STORE,
    "wellfed-mirror",
    "wellfed-releases",
    "wellfed-service",
    "wellfed-server-client",
];

/// What `wellfed-api` may depend on: serde types, schemas, ids and times,
/// all of which build for Android. A new entry needs that check.
const API_ALLOWED: [&str; 5] = ["jiff", "wellfed-model", "serde", "utoipa", "uuid"];

/// Running or reading `cargo metadata` failed, so no rule could be checked.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("could not run cargo metadata")]
    Run(#[from] io::Error),
    #[error("cargo metadata failed ({status}): {stderr}")]
    Failed {
        status: std::process::ExitStatus,
        stderr: String,
    },
    #[error("could not read cargo metadata's output")]
    Parse(#[from] serde_json::Error),
    #[error("cargo metadata has no dependency graph")]
    NoResolve,
    #[error("cargo metadata names package {0} in its graph but not among its packages")]
    UnknownPackage(String),
}

/// The parts of `cargo metadata --format-version 1` the rules read.
#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
    workspace_members: Vec<String>,
    resolve: Option<Resolve>,
}

#[derive(Deserialize)]
struct Package {
    id: String,
    name: String,
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
    /// `null` for a normal dependency.
    kind: Option<Kind>,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Dev,
    Build,
}

/// How a dependency reaches its dependent.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Use {
    /// A normal or build dependency: it is linked into the crate.
    Links,
    /// A dev-dependency only: it is linked into the crate's tests.
    TestsOnly,
}

/// The resolved graph, by package name. A name can belong to several
/// versions of an external crate, so edges run between package ids.
struct Graph {
    names: BTreeMap<String, String>,
    members: BTreeSet<String>,
    edges: BTreeMap<String, Vec<(String, Use)>>,
}

impl Graph {
    fn from_metadata(metadata: Metadata) -> Result<Self, Error> {
        let resolve = metadata.resolve.ok_or(Error::NoResolve)?;
        let names: BTreeMap<String, String> = metadata
            .packages
            .into_iter()
            .map(|package| (package.id, package.name))
            .collect();

        let mut edges = BTreeMap::new();
        for node in resolve.nodes {
            let mut deps = Vec::with_capacity(node.deps.len());
            for dep in node.deps {
                if !names.contains_key(&dep.pkg) {
                    return Err(Error::UnknownPackage(dep.pkg));
                }
                let links = dep
                    .dep_kinds
                    .iter()
                    .any(|kind| kind.kind != Some(Kind::Dev));
                let used = if links { Use::Links } else { Use::TestsOnly };
                deps.push((dep.pkg, used));
            }
            edges.insert(node.id, deps);
        }

        Ok(Self {
            names,
            members: metadata.workspace_members.into_iter().collect(),
            edges,
        })
    }

    fn name<'a>(&'a self, id: &'a str) -> &'a str {
        self.names.get(id).map_or(id, String::as_str)
    }

    fn deps(&self, id: &str) -> &[(String, Use)] {
        self.edges.get(id).map_or(&[], Vec::as_slice)
    }

    /// The shortest linked path from `from` to a package whose name
    /// `target` accepts, as package names, or `None` when none is linked.
    fn linked_path(&self, from: &str, target: impl Fn(&str) -> bool) -> Option<Vec<String>> {
        let mut parent: BTreeMap<&str, &str> = BTreeMap::new();
        let mut queue = VecDeque::from([from]);

        while let Some(id) = queue.pop_front() {
            if id != from && target(self.name(id)) {
                let mut path = vec![self.name(id).to_string()];
                let mut at = id;
                while let Some(&up) = parent.get(at) {
                    path.push(self.name(up).to_string());
                    at = up;
                }
                path.reverse();
                return Some(path);
            }
            for (dep, used) in self.deps(id) {
                if *used == Use::Links && dep != from && !parent.contains_key(dep.as_str()) {
                    parent.insert(dep, id);
                    queue.push_back(dep);
                }
            }
        }

        None
    }
}

/// Run `cargo metadata` over the workspace at `root` and check every rule.
pub(crate) fn check(root: &Path) -> Result<Vec<String>, Error> {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--all-features",
            "--locked",
        ])
        .current_dir(root)
        .output()?;
    if !output.status.success() {
        return Err(Error::Failed {
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    let metadata: Metadata = serde_json::from_slice(&output.stdout)?;
    Ok(rules(&Graph::from_metadata(metadata)?))
}

/// axum or one of its own crates (`axum-core`, `axum-extra`, …): linking any
/// of them brings HTTP below the binary.
fn is_axum(name: &str) -> bool {
    name.starts_with("axum")
}

/// Every broken rule, each naming the edge (or path) that breaks it.
fn rules(graph: &Graph) -> Vec<String> {
    let arrows: BTreeMap<&str, &[&str]> = ARROWS.into_iter().collect();
    let mut errors = Vec::new();

    for member in &graph.members {
        let from = graph.name(member);

        for (dep, used) in graph.deps(member) {
            let to = graph.name(dep);

            if from != STORE && (to == "sqlx" || to.starts_with("sqlx-")) {
                errors.push(format!(
                    "dependency rule: {from} → {to}: only {STORE} depends on sqlx"
                ));
            }
            if *used == Use::TestsOnly {
                continue;
            }

            if from == API && !API_ALLOWED.contains(&to) {
                errors.push(format!(
                    "dependency rule: {from} → {to}: not on {API}'s allow-list ({}); it builds for Android",
                    API_ALLOWED.join(", ")
                ));
            }
            if !graph.members.contains(dep) {
                continue;
            }
            if let Some(allowed) = arrows.get(from) {
                if !allowed.contains(&to) {
                    errors.push(format!(
                        "dependency rule: {from} → {to} is not an allowed edge; {from} may depend on {}",
                        allowed.join(", ")
                    ));
                }
            } else if arrows.contains_key(to) && !INTO_SERVER.contains(&(from, to)) {
                errors.push(format!(
                    "dependency rule: {from} → {to} is not an allowed edge into the server's crates"
                ));
            }
        }

        if BELOW_SERVER.contains(&from)
            && let Some(path) = graph.linked_path(member, is_axum)
        {
            errors.push(format!(
                "dependency rule: {}: nothing below wellfed-server links axum",
                path.join(" → ")
            ));
        }
    }

    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A graph whose package ids are their names, from `(from, to, use)`
    /// edges; `members` are the workspace's crates.
    fn graph(members: &[&str], edges: &[(&str, &str, Use)]) -> Graph {
        let mut graph = Graph {
            names: BTreeMap::new(),
            members: members.iter().map(ToString::to_string).collect(),
            edges: BTreeMap::new(),
        };
        for (from, to, used) in edges {
            for name in [from, to] {
                graph.names.insert(name.to_string(), name.to_string());
            }
            graph
                .edges
                .entry(from.to_string())
                .or_default()
                .push((to.to_string(), *used));
        }
        graph
    }

    const MEMBERS: [&str; 13] = [
        "wellfed",
        "wellfed-api",
        "wellfed-store",
        "wellfed-mirror",
        "wellfed-service",
        "wellfed-server",
        "wellfed-model",
        "wellfed-client",
        "wellfed-connect",
        "wellfed-feeder-sim",
        "wellfed-stock",
        "wellfed-dashboard",
        "wellfed-server-client",
    ];

    /// Today's server, plus the CLI on the typed client and a bench tool
    /// that serves its own axum.
    fn layered() -> Vec<(&'static str, &'static str, Use)> {
        use Use::{Links, TestsOnly};
        vec![
            ("wellfed-api", "wellfed-model", Links),
            ("wellfed-api", "utoipa", Links),
            ("wellfed-api", "serde", Links),
            ("wellfed-store", "wellfed-api", Links),
            ("wellfed-store", "sqlx", Links),
            ("wellfed-store", "tempfile", TestsOnly),
            ("wellfed-mirror", "wellfed-store", Links),
            ("wellfed-mirror", "wellfed-connect", Links),
            ("wellfed-mirror", "wellfed-feeder-sim", TestsOnly),
            ("wellfed-service", "wellfed-mirror", Links),
            ("wellfed-service", "wellfed-store", Links),
            ("wellfed-service", "wellfed-feeder-sim", TestsOnly),
            ("wellfed-connect", "wellfed-client", Links),
            ("wellfed-server", "wellfed-service", Links),
            ("wellfed-server", "wellfed-api", Links),
            ("wellfed-server", "utoipa-axum", Links),
            ("wellfed-server", "wellfed-feeder-sim", TestsOnly),
            ("utoipa-axum", "axum", Links),
            ("wellfed-server-client", "wellfed-api", Links),
            ("wellfed-server-client", "axum", TestsOnly),
            ("wellfed", "wellfed-server-client", Links),
            ("wellfed", "wellfed-connect", Links),
            ("wellfed-stock", "axum", Links),
            ("wellfed-dashboard", "axum", Links),
        ]
    }

    #[test]
    fn the_drawn_layers_pass() {
        let errors = rules(&graph(&MEMBERS, &layered()));
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn each_broken_rule_names_its_edge() {
        let mut edges = layered();
        edges.extend([
            // SQL outside the store, in a test too.
            ("wellfed-service", "sqlx", Use::Links),
            ("wellfed-server", "sqlx", Use::TestsOnly),
            // A dependency off the Android allow-list.
            ("wellfed-api", "reqwest", Use::Links),
            // An upward edge, a skipped layer and an edge in from outside.
            ("wellfed-store", "wellfed-service", Use::Links),
            ("wellfed-server", "wellfed-mirror", Use::Links),
            ("wellfed-dashboard", "wellfed-api", Use::Links),
            // axum through another crate, below the binary.
            ("wellfed-mirror", "wellfed-store", Use::Links),
            ("wellfed-mirror", "utoipa-axum", Use::Links),
            // A test of a crate outside the server may take any of them.
            ("wellfed-stock", "wellfed-server-client", Use::TestsOnly),
        ]);

        assert_eq!(
            rules(&graph(&MEMBERS, &edges)),
            [
                "dependency rule: wellfed-api → reqwest: not on wellfed-api's allow-list (jiff, wellfed-model, serde, utoipa, uuid); it builds for Android",
                "dependency rule: wellfed-dashboard → wellfed-api is not an allowed edge into the server's crates",
                "dependency rule: wellfed-mirror → utoipa-axum → axum: nothing below wellfed-server links axum",
                "dependency rule: wellfed-server → sqlx: only wellfed-store depends on sqlx",
                "dependency rule: wellfed-server → wellfed-mirror is not an allowed edge; wellfed-server may depend on wellfed-service, wellfed-api, wellfed-store",
                "dependency rule: wellfed-service → sqlx: only wellfed-store depends on sqlx",
                "dependency rule: wellfed-service → wellfed-mirror → utoipa-axum → axum: nothing below wellfed-server links axum",
                "dependency rule: wellfed-store → wellfed-service is not an allowed edge; wellfed-store may depend on wellfed-api",
                "dependency rule: wellfed-store → wellfed-service → wellfed-mirror → utoipa-axum → axum: nothing below wellfed-server links axum",
            ]
        );
    }

    #[test]
    fn axum_through_the_api_is_named_from_every_crate_above_it() {
        let mut edges = layered();
        edges.push(("wellfed-api", "utoipa-axum", Use::Links));

        let errors = rules(&graph(&MEMBERS, &edges));

        let axum: Vec<_> = errors
            .iter()
            .filter(|e| e.ends_with("links axum"))
            .collect();
        assert_eq!(
            axum,
            [
                "dependency rule: wellfed-api → utoipa-axum → axum: nothing below wellfed-server links axum",
                "dependency rule: wellfed-mirror → wellfed-store → wellfed-api → utoipa-axum → axum: nothing below wellfed-server links axum",
                "dependency rule: wellfed-server-client → wellfed-api → utoipa-axum → axum: nothing below wellfed-server links axum",
                "dependency rule: wellfed-service → wellfed-store → wellfed-api → utoipa-axum → axum: nothing below wellfed-server links axum",
                "dependency rule: wellfed-store → wellfed-api → utoipa-axum → axum: nothing below wellfed-server links axum",
            ]
        );
    }

    #[test]
    fn axum_s_own_crates_count_as_axum() {
        let mut edges = layered();
        edges.extend([
            ("wellfed-mirror", "axum-core", Use::Links),
            ("wellfed-app-bridge", "wellfed-api", Use::Links),
        ]);
        let mut members = MEMBERS.to_vec();
        members.push("wellfed-app-bridge");

        assert_eq!(
            rules(&graph(&members, &edges)),
            [
                "dependency rule: wellfed-mirror → axum-core: nothing below wellfed-server links axum",
                "dependency rule: wellfed-service → wellfed-mirror → axum-core: nothing below wellfed-server links axum",
            ]
        );
    }

    #[test]
    fn a_crate_that_does_not_exist_yet_is_held_to_its_arrows() {
        let mut edges = layered();
        edges.extend([
            ("wellfed-releases", "wellfed-service", Use::Links),
            ("wellfed-releases", "axum", Use::Links),
        ]);
        let mut members = MEMBERS.to_vec();
        members.push("wellfed-releases");

        let errors = rules(&graph(&members, &edges));

        assert!(errors.contains(
            &"dependency rule: wellfed-releases → wellfed-service is not an allowed edge; wellfed-releases may depend on wellfed-store, wellfed-api, wellfed-client, wellfed-model".to_string()
        ), "{errors:?}");
        assert!(
            errors.contains(
                &"dependency rule: wellfed-releases → axum: nothing below wellfed-server links axum"
                    .to_string()
            ),
            "{errors:?}"
        );
    }

    #[test]
    fn metadata_kinds_decide_what_links() {
        let json = r#"{
            "packages": [
                {"id": "a 0.1.0", "name": "wellfed-service"},
                {"id": "b 0.9.0", "name": "sqlx"},
                {"id": "c 0.8.0", "name": "axum"},
                {"id": "d 1.0.0", "name": "cc"}
            ],
            "workspace_members": ["a 0.1.0"],
            "resolve": {"nodes": [
                {"id": "a 0.1.0", "deps": [
                    {"pkg": "b 0.9.0", "dep_kinds": [{"kind": "dev", "target": null}]},
                    {"pkg": "c 0.8.0", "dep_kinds": [{"kind": "dev", "target": null}]},
                    {"pkg": "d 1.0.0", "dep_kinds": [{"kind": "build", "target": null}]}
                ]}
            ]}
        }"#;
        let metadata: Metadata = serde_json::from_str(json).unwrap();
        let graph = Graph::from_metadata(metadata).unwrap();

        assert_eq!(
            graph.deps("a 0.1.0"),
            [
                ("b 0.9.0".to_string(), Use::TestsOnly),
                ("c 0.8.0".to_string(), Use::TestsOnly),
                ("d 1.0.0".to_string(), Use::Links),
            ]
        );
        assert_eq!(
            rules(&graph),
            ["dependency rule: wellfed-service → sqlx: only wellfed-store depends on sqlx"]
        );
    }
}
