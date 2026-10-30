//! Every device API op has a client in the CLI and in the app, or a stated
//! reason it has not. An op counts as covered by naming `Op::Variant` in the
//! client's own sources, or in the crates both clients share for sessions and
//! installs, whose drivers (the install, an update's push, the update key's
//! enrollment, a trace's pages, a call's upload) they each run. Tests don't
//! count, and neither does the model's own conversion to the wire.
//!
//! A request the bridge offers and no screen sends is no client either, so
//! every `FeederRequest` variant must be named by the app's Dart too.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// Where `Op` is declared.
const REQUEST: &str = "src/client/wellfed-model/src/request.rs";
/// Where the bridge declares what a screen can ask.
const BRIDGE: &str = "src/app/rust/src/api/feeder.rs";
const DART: &str = "src/app/lib/";
/// Reads the session itself keeps on the card, so no screen asks for them.
const READ_FOR_THE_CARD: [&str; 2] = ["GetSettings", "GetTime"];
/// Dart written by the code generator.
const DART_GENERATED: &str = "src/app/lib/src/rust/";
const CLI: &str = "src/tools/wellfed/src/";
const APP: &str = "src/app/rust/src/";
/// The sessions' and installs' drivers both clients run.
const SHARED: [&str; 3] = [
    "src/client/wellfed-client/src/",
    "src/client/wellfed-connect/src/",
    "src/client/wellfed-install/src/",
];
/// Written by the code generator from the bridge's own types.
const GENERATED: &str = "frb_generated.rs";

/// What the CLI must not do, with why.
const NOT_IN_CLI: [(&str, &str); 0] = [];
/// What the app must not do, with why.
const NOT_IN_APP: [(&str, &str); 1] = [(
    "UpdateKeyStatement",
    "a statement is signed offline by a built-in or the release key, and the phone holds no \
     key a feeder trusts; the owner's computer carries one to the feeder",
)];

/// The variants `source` declares for `enum`, in order.
fn variants<'a>(source: &'a str, name: &str) -> Vec<&'a str> {
    let header = format!("pub enum {name} {{");
    source
        .lines()
        .skip_while(|line| !line.starts_with(&header))
        .skip(1)
        .take_while(|line| !line.starts_with('}'))
        .filter_map(|line| {
            let name = line.strip_prefix("    ")?;
            let end = name
                .find(|c: char| !c.is_ascii_alphanumeric())
                .unwrap_or(name.len());
            let name = &name[..end];
            name.starts_with(|c: char| c.is_ascii_uppercase())
                .then_some(name)
        })
        .collect()
}

/// The variants `source` names outside its tests.
fn named(source: &str) -> BTreeSet<&str> {
    let code = source.split("#[cfg(test)]").next().unwrap_or(source);
    code.match_indices("Op::")
        .filter_map(|(at, _)| {
            let rest = &code[at + 4..];
            let end = rest
                .find(|c: char| !c.is_ascii_alphanumeric())
                .unwrap_or(rest.len());
            (end > 0).then(|| &rest[..end])
        })
        .collect()
}

/// One client's findings over the ops, given what it and the shared crates
/// name.
fn client(
    who: &str,
    ops: &[&str],
    own: &BTreeSet<&str>,
    shared: &BTreeSet<&str>,
    exempt: &[(&str, &str)],
    errors: &mut Vec<String>,
) {
    for op in ops {
        let covered = own.contains(op) || shared.contains(op);
        let excused = exempt.iter().any(|(name, _)| name == op);
        match (covered, excused) {
            (false, false) => errors.push(format!(
                "Op::{op} has no client in {who}: add one, or a reason in {}",
                file!()
            )),
            (true, true) => errors.push(format!(
                "Op::{op} is exempt from {who}, yet {who} sends it: drop the exemption"
            )),
            _ => {}
        }
    }
    for (name, _) in exempt {
        if !ops.contains(name) {
            errors.push(format!("{who}'s exemption names no op: {name}"));
        }
    }
}

fn union<'a>(sources: impl IntoIterator<Item = &'a String>) -> BTreeSet<&'a str> {
    sources
        .into_iter()
        .flat_map(|source| named(source))
        .collect()
}

/// What Dart calls a generated variant: its first letter lower-cased.
fn dart_name(variant: &str) -> String {
    let mut chars = variant.chars();
    chars
        .next()
        .map(|first| first.to_ascii_lowercase().to_string() + chars.as_str())
        .unwrap_or_default()
}

/// The bridge's requests no Dart source names as `FeederRequest.name`.
fn unsent(requests: &[&str], dart: &[String]) -> Vec<String> {
    requests
        .iter()
        .filter(|request| !READ_FOR_THE_CARD.contains(request))
        .filter(|request| {
            let call = format!("FeederRequest.{}(", dart_name(request));
            !dart.iter().any(|source| source.contains(&call))
        })
        .map(|request| format!("FeederRequest::{request} is sent by no screen in {DART}"))
        .collect()
}

/// The findings over the checkout at `root`, whose files are `names`.
pub(crate) fn check(root: &Path, names: &[String]) -> Vec<String> {
    let read = |prefixes: &[&str]| -> Vec<String> {
        names
            .iter()
            .filter(|name| {
                name.ends_with(".rs")
                    && !name.ends_with(GENERATED)
                    && prefixes.iter().any(|prefix| name.starts_with(prefix))
            })
            .filter_map(|name| fs::read_to_string(root.join(name)).ok())
            .collect()
    };
    let Ok(request) = fs::read_to_string(root.join(REQUEST)) else {
        return vec![format!("{REQUEST}: cannot be read")];
    };
    let ops = variants(&request, "Op");
    if ops.is_empty() {
        return vec![format!("{REQUEST}: no `pub enum Op` found")];
    }

    let (cli, app, shared) = (read(&[CLI]), read(&[APP]), read(&SHARED));
    let shared = union(&shared);
    let mut errors = Vec::new();
    client(
        "the CLI",
        &ops,
        &union(&cli),
        &shared,
        &NOT_IN_CLI,
        &mut errors,
    );
    client(
        "the app",
        &ops,
        &union(&app),
        &shared,
        &NOT_IN_APP,
        &mut errors,
    );

    let Ok(bridge) = fs::read_to_string(root.join(BRIDGE)) else {
        errors.push(format!("{BRIDGE}: cannot be read"));
        return errors;
    };
    let dart: Vec<String> = names
        .iter()
        .filter(|name| {
            name.ends_with(".dart") && name.starts_with(DART) && !name.starts_with(DART_GENERATED)
        })
        .filter_map(|name| fs::read_to_string(root.join(name)).ok())
        .collect();
    errors.extend(unsent(&variants(&bridge, "FeederRequest"), &dart));
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQUESTS: &str = "\
/// Docs.
pub enum Op {
    Status,
    /// Applies only if the settings are still at `base`.
    PutSettings {
        base: Generation,
        changes: SettingsPatch,
    },
    TimeSync(TimeSync),
}

pub enum Other {
    Not,
}
";

    #[test]
    fn the_ops_are_read_from_their_enum() {
        assert_eq!(
            variants(REQUESTS, "Op"),
            ["Status", "PutSettings", "TimeSync"]
        );
        assert_eq!(variants(REQUESTS, "Other"), ["Not"]);
    }

    #[test]
    fn tests_do_not_count_as_a_client() {
        let source = "fn a() { ask(Op::Status) }\n#[cfg(test)]\nmod tests { Op::Reboot }";
        assert_eq!(named(source), BTreeSet::from(["Status"]));
    }

    #[test]
    fn an_op_with_no_client_is_found_unless_exempt_and_a_stale_exemption_too() {
        let ops = ["Status", "Reboot", "Play"];
        let own = BTreeSet::from(["Status"]);
        let shared = BTreeSet::from(["Play"]);

        let mut errors = Vec::new();
        client("the app", &ops, &own, &shared, &[], &mut errors);
        assert_eq!(errors.len(), 1);
        assert!(
            errors[0].contains("Op::Reboot has no client in the app"),
            "{errors:?}"
        );

        let mut errors = Vec::new();
        client(
            "the app",
            &ops,
            &own,
            &shared,
            &[("Reboot", "why")],
            &mut errors,
        );
        assert_eq!(errors, Vec::<String>::new());

        let mut errors = Vec::new();
        let stale = [("Status", "why"), ("Gone", "why"), ("Reboot", "why")];
        client("the app", &ops, &own, &shared, &stale, &mut errors);
        assert_eq!(errors.len(), 2, "{errors:?}");
    }

    #[test]
    fn a_request_no_screen_sends_is_found() {
        let dart = ["ask(const FeederRequest.play());".to_string()];
        assert_eq!(
            unsent(&["Play", "Reboot", "GetTime"], &dart),
            [format!(
                "FeederRequest::Reboot is sent by no screen in {DART}"
            )]
        );
    }

    #[test]
    fn the_exemptions_give_their_reasons() {
        for (name, reason) in NOT_IN_CLI.iter().chain(&NOT_IN_APP) {
            assert!(reason.len() > 20, "{name}");
        }
    }
}
