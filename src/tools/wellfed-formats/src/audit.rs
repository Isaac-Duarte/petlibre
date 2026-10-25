//! The linked-image audits a build manifest records.
//!
//! `wellfed-build` writes them, its review reruns them and compares the
//! results with the recorded ones, and the flash review reads the startup
//! mains, the pad table and the layout from a frozen review. Every type
//! defaults its missing fields so the manifests and reviews of older builders
//! (which recorded fewer fields, or none of an audit) still read; a check
//! that needs a field then finds its default, not the whole file refused.
//!
//! So a recorded audit is evidence to compare, never a result to trust: the
//! review accepts it only when it equals the audit rerun on the build, and
//! no reader takes a count or flag from it on its own, since a missing field
//! reads as zero or `false`.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::layout::Hex;

/// The candidate OTA image's structure against the stock layout.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutAudit {
    pub schema_version: u32,
    pub image: String,
    pub bytes: usize,
    pub sha256: String,
    pub payload_bytes: usize,
    /// The core images, as the flash client's container check reads them.
    pub regions: Vec<LayoutRegion>,
    pub candidate_placements: Vec<Placement>,
    pub checks: LayoutChecks,
    pub boot_acceptance: String,
    pub physical_flash_capacity: String,
    pub scope: String,
}

/// One core image in a bank payload.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutRegion {
    pub name: String,
    pub payload_header_offset: usize,
    pub body_bytes: usize,
    pub runtime_address: String,
}

/// The candidate written at one bank.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Placement {
    pub bank_index: usize,
    pub flash_offset: Hex,
    pub end_exclusive: Hex,
    pub erase_bytes: usize,
}

/// What the audit refused otherwise; always `true` in a written audit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutChecks {
    pub container_and_checksum: bool,
    pub reference_table_matches: bool,
    pub known_preserve_ranges_untouched: bool,
}

/// The linked KM0 pad table against stock's.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PinmapAudit {
    pub runtime_address: Hex,
    pub rows: usize,
    pub bytes: usize,
    pub sha256: String,
    pub matches_stock: bool,
    pub scope: String,
}

/// A core's archive, as the hook and park audits record it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArchiveRecord {
    pub elf_sha256: String,
    /// Relative to the build output.
    pub object_path: String,
    pub object_sha256: String,
}

/// One text section matched byte for byte against the package.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PackagedText {
    pub section: String,
    pub address: Hex,
    pub bytes: usize,
    pub sha256: String,
}

/// The RAM parking code each core retains.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ParkAudit {
    /// Keyed KM0/KM4.
    pub images: BTreeMap<String, ParkImage>,
    pub activated: bool,
    pub global_exclusion_proven: bool,
    pub scope: String,
}

/// One core's parking functions, bound to its linked archive and package.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ParkImage {
    #[serde(flatten)]
    pub archive: ArchiveRecord,
    pub packaged_text_verified: Vec<PackagedText>,
    pub functions: BTreeMap<String, ParkFunction>,
}

/// One retained parking function.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ParkFunction {
    pub address: String,
    pub bytes: u64,
    #[serde(flatten)]
    pub leaf: Leaf,
}

/// A parking function's checked instructions; every refused kind counts 0.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Leaf {
    pub instructions: u32,
    pub barriers: u32,
    pub calls: u32,
    pub foreign_branches: u32,
    pub xip_literals: u32,
    pub flash_address_constants: u32,
}

/// The SDK's flash hooks routed through the RAM interposer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HookAudit {
    /// Keyed KM0/KM4.
    pub images: BTreeMap<String, HookImage>,
    pub protocol_installed: bool,
    pub global_exclusion_proven: bool,
    pub scope: String,
}

/// One core's hook routes, bound to its linked archive.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HookImage {
    #[serde(flatten)]
    pub archive: ArchiveRecord,
    pub symbols: BTreeMap<String, Symbol>,
    /// Each legacy entry's routes.
    pub legacy_entry_routes: BTreeMap<String, Vec<Route>>,
    pub ipc_channel_2_handler: String,
}

/// A symbol table entry: address, size, kind (`F`/`O`) and section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Symbol {
    pub address: u64,
    pub bytes: u64,
    pub kind: String,
    pub section: String,
}

/// A direct branch into a legacy entry, from its public wrapper.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Route {
    pub caller: String,
    pub address: String,
}

/// Each core's first-boot `main`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MainsAudit {
    /// Keyed by the SDK's core directory names (`hp`, `lp`).
    pub mains: BTreeMap<String, Main>,
    pub network_or_shell_started_by_main: bool,
    pub feeder_backend_opened_by_application: bool,
    pub scope: String,
}

/// One core's linked `main`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Main {
    pub address: String,
    pub bytes: u64,
    #[serde(flatten)]
    pub calls: MainCalls,
}

/// A `main`'s checked call sequence.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MainCalls {
    pub instructions: u32,
    pub resolved_calls: Vec<String>,
    /// Always 0: any unresolved call is refused.
    pub unresolved_calls: u32,
}

/// The flash and eFuse writers each core links, and what references them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WritesAudit {
    pub hp: CoreWrites,
    pub lp: CoreWrites,
    pub scope: String,
}

/// One core's linked writers and every reference to them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CoreWrites {
    pub elf_sha256: String,
    pub absent: Vec<String>,
    pub write_targets: Vec<String>,
    /// Each referenced target's enclosing symbols.
    pub references: BTreeMap<String, BTreeSet<String>>,
}

/// Each KM4 application task's static stack bound.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StackAudit {
    /// Keyed by task name.
    pub tasks: BTreeMap<String, TaskBound>,
    /// The linked tasks the firmware starts, its SDK services' included.
    pub firmware_tasks: Vec<String>,
    pub firmware_task_limit: usize,
    pub overhead: u64,
    pub margin: f64,
    /// How many levels of nested formatting each task's `fmt_bound` covers.
    pub fmt_nesting: usize,
    /// What an indirect call inside the flash hooks is charged: the deepest
    /// provider callback.
    pub provider_bound: u64,
    pub scope: String,
}

/// One task's static stack bound.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TaskBound {
    pub entry: String,
    pub stack: u64,
    pub bound: u64,
    /// What its formatting calls are charged: the deepest formatting method
    /// it can reach, nested.
    pub fmt_bound: u64,
    /// Vendor C functions on any path whose indirect calls the walk does not
    /// bound; the margin covers them.
    pub unbounded: Vec<String>,
    /// Functions where recursion was cut.
    pub recursion: Vec<String>,
    /// The deepest path's first functions.
    pub path: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Frozen reviews keep the manifests of older builders, which recorded
    /// fewer fields; `wellfed-flash` still reads them.
    #[test]
    fn audits_from_older_builders_still_read() {
        let stack: StackAudit = serde_json::from_value(json!({
            "margin": 0.25, "overhead": 256, "scope": "s",
            "tasks": {"wellfed": {"bound": 1, "entry": "e", "indirect": [], "path": [],
                                   "recursion": [], "stack": 4096}}
        }))
        .unwrap();
        assert_eq!(stack.tasks["wellfed"].fmt_bound, 0);

        let leaf: ParkFunction = serde_json::from_value(json!({
            "address": "0x1", "bytes": 4, "instructions": 3, "barriers": 1, "calls": 0,
            "foreign_branches": 0, "xip_literals": 0
        }))
        .unwrap();
        assert_eq!(leaf.leaf.instructions, 3);
    }

    #[test]
    fn flattened_records_write_flat() {
        let main = Main {
            address: "0x1".into(),
            bytes: 8,
            calls: MainCalls {
                instructions: 2,
                resolved_calls: vec!["a".into()],
                unresolved_calls: 0,
            },
        };
        assert_eq!(
            serde_json::to_value(&main).unwrap(),
            json!({"address": "0x1", "bytes": 8, "instructions": 2,
                   "resolved_calls": ["a"], "unresolved_calls": 0})
        );
    }
}
