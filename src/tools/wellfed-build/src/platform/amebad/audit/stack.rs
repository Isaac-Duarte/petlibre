//! Worst-case static stack of each KM4 application task, from the linked
//! image.
//!
//! A crate-split build overflowed the optics task's 4 KiB stack on hardware:
//! LTO inlined a 3 KiB temporary into the task's frame. The SDK's ARMv8-M
//! port faults on the stack limit, and the trial caught it, but only after
//! an update. This audit catches it at build time instead.
//!
//! For each task created in the runtime or its services
//! (`sdk::task_create(c"name", words, ...)`) or declared by the firmware
//! (`#[wellfed_sdk::task(name = "name", stack = WORDS, priority = N)] fn entry`),
//! it walks the direct call graph from the task's entry function and
//! sums the frames along the deepest path. Frames come from each function's
//! prologue (`push`, `stmdb sp!`, `vpush`, `sub sp`), and a linker veneer
//! (`__name_veneer`, the long branch into RAM or ROM) leads to its target.
//! Recursion is cut at the cycle.
//!
//! An indirect call (`blx rN`, or a tail call `bx rN`) is bounded by what it can reach:
//! - inside `core::fmt`, by the deepest formatting method in the image
//!   (`Display::fmt`, `Write::write_str` and the rest), nested up to
//!   [`FMT_NESTING`] levels;
//! - inside the RAM flash hooks, by the deepest provider callback the runtime
//!   installs ([`PROVIDER_CALLBACKS`]);
//! - inside vendor C (lwIP, the codec, the SDK's flash and ADC routines), not
//!   at all: those sites are listed per task, and the margin covers them.
//!
//! Any other indirect call in Rust is refused, so a new function pointer or
//! `dyn` call needs a class here or a direct call instead. The margin stays
//! for what the walk does not model: exception nesting beyond one frame and
//! the vendor C sites.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::LazyLock;

use wellfed_formats::audit::{StackAudit, TaskBound};
use regex::Regex;

use super::writes::{demangle, loaded};
use super::{HEADING, refuse, tool};
use crate::{Firmware, Repo, Result};

/// The SDK runtime's task table.
pub(crate) const RUNTIME_SOURCE: &str = "src/sdk/wellfed-sdk/src/runtime.rs";
/// SDK services a firmware may start, whose tasks count as the firmware's
/// (the button task).
pub(crate) const SERVICE_SOURCES: [&str; 1] = ["src/sdk/wellfed-sdk/src/buttons.rs"];
/// The firmware's share of the SDK's eight task slots: the runtime holds three
/// in the normal role, and one stays spare. `wellfed_sdk::FIRMWARE_TASKS` enforces the
/// same budget at run time, and a test keeps the two equal.
const FIRMWARE_TASKS: usize = 4;

/// The text the audit reads its task table from.
pub(crate) struct TaskSources {
    runtime: String,
    /// The SDK services a firmware may start.
    services: String,
    /// Every Rust file of the firmware's `src/`, in path order.
    own: String,
}

impl TaskSources {
    /// The sources whose tasks count as the firmware's: its services', then
    /// its own.
    fn firmware(&self) -> String {
        format!("{}\n{}", self.services, self.own)
    }
}

pub(crate) fn task_sources(repo: &Repo, firmware: &Firmware) -> Result<TaskSources> {
    let services: Vec<std::path::PathBuf> = SERVICE_SOURCES
        .iter()
        .map(|source| repo.path(source))
        .collect();
    let mut own = Vec::new();
    rust_files(&repo.path(&format!("{}/src", firmware.dir())), &mut own)?;
    own.sort();

    Ok(TaskSources {
        runtime: std::fs::read_to_string(repo.path(RUNTIME_SOURCE))?,
        services: concatenated(&services)?,
        own: concatenated(&own)?,
    })
}

fn concatenated(paths: &[std::path::PathBuf]) -> Result<String> {
    let mut text = String::new();
    for path in paths {
        text.push_str(&std::fs::read_to_string(path)?);
        text.push('\n');
    }
    Ok(text)
}

fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            rust_files(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    Ok(())
}

/// The SDK trampoline frame and an exception stacked onto the task's stack
/// with the FPU context (26 words), rounded up.
const OVERHEAD: u64 = 256;
/// What the walk does not model (the vendor C indirect sites, deeper
/// exception nesting): the bound must leave this share of the stack free.
const MARGIN: f64 = 0.25;
/// How deeply one formatting call may nest another: a record's `Display`
/// writes fields whose own `Display` writes through the report's `Write`.
const FMT_NESTING: usize = 4;
/// The flash-lock interposer's functions, whose indirect calls reach the
/// installed provider.
const HOOK_SITES: [&str; 5] = [
    "FLASH_Write_Lock",
    "FLASH_Write_Unlock",
    "FLASH_Write_IPC_Int",
    "hook_fail",
    "wellfed_amebad_flash_hooks_install",
];
/// The provider callbacks `wellfed-sdk`'s runtime installs into the hooks,
/// by the end of their (mangled) names.
const PROVIDER_CALLBACKS: [&str; 5] = [
    "7runtime10hook_ready",
    "7runtime12hook_acquire",
    "7runtime12hook_release",
    "7runtime9hook_peer",
    "wellfed_amebad_flash_provider_fault",
];

/// A task the runtime or a service creates: name, stack words, entry, and the
/// entry's turbofish if it names a generic instance.
static TASK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"task_create\(\s*c"([\w-]+)",\s*(\w+),\s*\d+,\s*(\w+)(::<[^>]*>)?\s*[,)]"#)
        .expect("stack audit pattern")
});
/// A firmware's task, as `#[task]` declares it: name, stack words and the
/// entry, which keeps the function's name. Doc comments and other attributes
/// may sit between the attribute and the function.
static DECLARED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r#"#\[\s*(?:(?:::)?\s*wellfed_sdk\s*::\s*)?task\s*\(\s*name\s*=\s*"([\w-]+)"\s*,"#,
        r"\s*stack\s*=\s*(\w+)\s*,\s*priority\s*=\s*\d+\s*,?\s*\)\s*\]",
        r"\s*(?:(?:#\[[^\]]*\]|///[^\n]*)\s*)*(?:pub(?:\([\w\s:]*\))?\s+)?fn\s+(\w+)\s*\(",
    ))
    .expect("stack audit pattern")
});
static CONSTANT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"const (\w+): u16 = (\d+);").expect("stack audit pattern"));
static CALL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\t(?:bl|blx|b\.w|b)(?:\.[nw])?\t[0-9a-f]+ <([^>+]+)>")
        .expect("stack audit pattern")
});
/// A call or tail call through a register. objdump names r9 to r12 `sb`, `sl`,
/// `fp` and `ip`, and a tail call through a pointer is `bx rN` (`bx ip` in
/// vendor C and `core::fmt`). `bx lr` is a return, not a call. A linker
/// veneer's `bx ip` is not counted: its target comes from its name.
static INDIRECT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\tbl?x\t(?:r\d+|sb|sl|fp|ip)\b").expect("stack audit pattern"));
/// Every task creation or declaration, whatever its shape: [`TASK`] and
/// [`DECLARED`] must account for each. A `Task` built by hand is one neither
/// reads, so it is refused.
static TASK_SITE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"task_create\(\s*c"|#\[\s*(?:(?:::)?\s*wellfed_sdk\s*::\s*)?task\s*\(|Task\s*::\s*__new\s*\("#,
    )
    .expect("stack audit pattern")
});
/// A firmware starting a task: `wellfed_sdk::spawn(entry)`, with the entry a
/// task the firmware declares.
static SPAWNED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"wellfed_sdk\s*::\s*spawn\s*\(\s*(\w+)\s*\)").expect("stack audit pattern")
});
/// Any mention of `spawn` outside a comment: [`SPAWNED`] must account for
/// each, so a renamed import or a task from elsewhere cannot start unread.
static SPAWN_WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bspawn\b").expect("stack audit pattern"));
/// Any mention of `task_create` outside a comment, whatever its arguments.
static CREATE_WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\btask_create\b").expect("stack audit pattern"));
static LINE_COMMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"//[^\n]*").expect("stack audit pattern"));
/// Half of an address built in a register: `movw r0, #51187` (low) then
/// `movt r0, #3585` (high).
static MOVE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^mov([wt])\s+(\w+), #(\d+)").expect("stack audit pattern"));
/// A literal word in a function's pool.
static WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\.word\s+0x([0-9a-f]+)").expect("stack audit pattern"));
/// How many words of a vtable (or other table) a reference may name
/// functions in: the drop, size and alignment slots, then the methods.
const TABLE_WORDS: u64 = 8;
/// A formatting trait method, by its mangled name: `<T as Display>::fmt` and
/// the other `core::fmt` traits, and `Write`'s methods.
static FMT_METHOD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^_RNv[XY].*(?:(?:7Display|5Debug|8LowerHex|8UpperHex|5Octal|6Binary|8LowerExp|8UpperExp|7Pointer)3fmt|5Write(?:9write_str|10write_char|9write_fmt))",
    )
    .expect("stack audit pattern")
});
static PUSH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:push(?:\.w)?|stmdb(?:\.w)?\s+sp!,)\s*\{([^}]*)\}")
        .expect("stack audit pattern")
});
static VPUSH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^vpush\s*\{d(\d+)-d(\d+)\}").expect("stack audit pattern"));
static SUB: LazyLock<Regex> = LazyLock::new(|| {
    // `subw` is Thumb-2's 12-bit-immediate form, which LLVM picks for frames
    // up to 4 KiB; missing it hid curve25519's 1948-byte frame.
    Regex::new(r"^sub(?:w|\.w)?\s+sp,\s*(?:sp,\s*)?#(\d+)").expect("stack audit pattern")
});

/// {entry function name: (task name, stack bytes)} from the task sources.
///
/// A task the audit cannot resolve is refused rather than skipped, since a
/// skipped task links unaudited. A generic entry (`step::<S>`) is one: its
/// monomorphized symbol carries the type arguments, so no linked function
/// matches the source's name, and the walk would take it for a variant that
/// does not link the task. So is a stack size that is neither a literal nor a
/// `const NAME: u16` in the same sources.
fn tasks(source: &str) -> Result<BTreeMap<String, (String, u64)>> {
    let constants: HashMap<&str, u64> = CONSTANT
        .captures_iter(source)
        .filter_map(|c| Some((c.get(1)?.as_str(), c[2].parse().ok()?)))
        .collect();

    let (sites, matched) = (
        TASK_SITE.find_iter(source).count(),
        TASK.captures_iter(source).count() + DECLARED.captures_iter(source).count(),
    );
    if sites != matched {
        return refuse(format!(
            "{} task creations do not have the shape the stack audit reads \
             (#[wellfed_sdk::task(name = \"name\", stack = WORDS, priority = N)] on a plain \
             function, or the runtime's task_create(c\"name\", WORDS, priority, entry)): keep \
             the arguments in that order, the priority a literal and the entry a plain \
             function name",
            sites - matched.min(sites)
        ));
    }

    let mut found = BTreeMap::new();
    for c in TASK
        .captures_iter(source)
        .chain(DECLARED.captures_iter(source))
    {
        let (task, entry) = (&c[1], &c[3]);
        if let Some(generic) = c.get(4) {
            return refuse(format!(
                "Task {task} starts the generic {entry}{}: the stack audit cannot find its \
                 symbol, so start it through a non-generic entry function",
                generic.as_str()
            ));
        }
        let Some(words) = constants.get(&c[2]).copied().or_else(|| c[2].parse().ok()) else {
            return refuse(format!(
                "Task {task}'s stack size {} is neither a literal nor a `const NAME: u16` the \
                 stack audit can read",
                &c[2]
            ));
        };
        found.insert(entry.to_string(), (task.to_string(), words * 4));
    }
    Ok(found)
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Function {
    address: u64,
    frame: u64,
    calls: BTreeSet<String>,
    indirect: bool,
    sized: bool,
    /// Addresses the function builds or loads (`movw`/`movt` pairs and
    /// literal words): the formatting methods and vtables it passes on.
    refs: BTreeSet<u64>,
}

/// Each function's prologue frame, direct callees and whether it makes
/// indirect calls, from `objdump -d`, in listing order.
fn frames(listing: &str) -> Vec<(String, Function)> {
    let mut functions: Vec<(String, Function)> = Vec::new();
    let mut low: HashMap<String, u64> = HashMap::new();
    for line in listing.lines() {
        if let Some(heading) = HEADING.captures(line) {
            let address = u64::from_str_radix(&heading[1], 16).unwrap_or_default();
            let f = Function {
                address,
                ..Function::default()
            };
            functions.push((heading[2].to_string(), f));
            low.clear();
            continue;
        }
        let Some((name, f)) = functions.last_mut() else {
            continue;
        };
        if !line.contains(":\t") {
            continue;
        }

        let insn = line.splitn(3, '\t').last().unwrap_or_default();
        if !f.sized {
            if let Some(push) = PUSH.captures(insn) {
                f.frame += 4 * push[1].split(',').filter(|r| !r.trim().is_empty()).count() as u64;
                continue;
            }
            if let Some(vpush) = VPUSH.captures(insn) {
                let (low, high): (u64, u64) =
                    (vpush[1].parse().unwrap_or(0), vpush[2].parse().unwrap_or(0));
                f.frame += 8 * (high + 1).saturating_sub(low);
                continue;
            }
            if let Some(sub) = SUB.captures(insn) {
                f.frame += sub[1].parse::<u64>().unwrap_or(0);
                f.sized = true;
                continue;
            }
        }
        if let Some(call) = CALL.captures(line) {
            f.calls.insert(call[1].to_string());
        } else if INDIRECT.is_match(line) {
            f.indirect |= veneer(name).is_none();
        } else if let Some(m) = MOVE.captures(insn) {
            let value: u64 = m[3].parse().unwrap_or_default();
            if &m[1] == "w" {
                low.insert(m[2].to_string(), value);
            } else if let Some(low) = low.remove(&m[2]) {
                f.refs.insert(value << 16 | low);
            }
        } else if let Some(word) = WORD.captures(insn) {
            f.refs
                .insert(u64::from_str_radix(&word[1], 16).unwrap_or_default());
        }
    }
    functions
}

/// What an indirect call can reach, by the function that makes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Indirect {
    /// `core::fmt`: a formatting method.
    Fmt,
    /// The flash hooks: an installed provider callback.
    Hook,
    /// Anything else: unbounded. Vendor C is listed; Rust is refused.
    Unbounded,
}

fn classify(name: &str) -> Indirect {
    if HOOK_SITES.contains(&name) {
        Indirect::Hook
    } else if name.starts_with("_R") && name.contains("4core3fmt") {
        Indirect::Fmt
    } else {
        Indirect::Unbounded
    }
}

/// What a walk charges each class of indirect call.
#[derive(Debug, Clone, Copy, Default)]
struct Charges {
    fmt: u64,
    hook: u64,
}

/// A function's deepest path: bytes, the path, and the functions on any
/// path whose indirect calls stay unbounded.
type Walk = (u64, Vec<String>, BTreeSet<String>);

struct Graph<'a> {
    functions: &'a HashMap<String, Function>,
    classes: HashMap<&'a str, Indirect>,
    by_address: HashMap<u64, &'a str>,
    /// The image's loaded sections, for the tables a reference points into.
    data: &'a [(u64, Vec<u8>)],
}

impl<'a> Graph<'a> {
    fn new(functions: &'a HashMap<String, Function>, data: &'a [(u64, Vec<u8>)]) -> Self {
        let classes = functions
            .iter()
            .filter(|(_, f)| f.indirect)
            .map(|(name, _)| (name.as_str(), classify(name)))
            .collect();
        let by_address = functions
            .iter()
            .map(|(name, f)| (f.address, name.as_str()))
            .collect();
        Self {
            functions,
            classes,
            by_address,
            data,
        }
    }

    /// The function at `value`, a Thumb code address.
    fn function_at(&self, value: u64) -> Option<&'a str> {
        self.by_address.get(&(value & !1)).copied()
    }

    /// The functions `value` names: itself, or those among the first words of
    /// the table it points into (a vtable).
    fn resolve(&self, value: u64, out: &mut BTreeSet<&'a str>) {
        if let Some(name) = self.function_at(value) {
            out.insert(name);
            return;
        }
        let Some((start, bytes)) = self
            .data
            .iter()
            .find(|(start, bytes)| (*start..*start + bytes.len() as u64).contains(&value))
        else {
            return;
        };
        for word in 0..TABLE_WORDS {
            let Ok(at) = usize::try_from(value - start + 4 * word) else {
                break;
            };
            let Some(&raw) = bytes.get(at..at + 4).and_then(|b| b.first_chunk::<4>()) else {
                break;
            };
            if let Some(name) = self.function_at(u64::from(u32::from_le_bytes(raw))) {
                out.insert(name);
            }
        }
    }

    /// The formatting methods `entry` can reach: referenced from any function
    /// it calls, and from those methods in turn.
    fn fmt_targets(&self, entry: &'a str) -> BTreeSet<&'a str> {
        let (mut seen, mut targets, mut stack) = (BTreeSet::new(), BTreeSet::new(), vec![entry]);
        while let Some(name) = stack.pop() {
            if !seen.insert(name) {
                continue;
            }
            let Some(f) = self.functions.get(name) else {
                continue;
            };
            stack.extend(f.calls.iter().map(String::as_str));

            let mut named = BTreeSet::new();
            for value in &f.refs {
                self.resolve(*value, &mut named);
            }
            for target in named {
                if FMT_METHOD.is_match(target) && targets.insert(target) {
                    stack.push(target);
                }
            }
        }
        targets
    }

    /// What `entry`'s formatting calls are charged: its deepest formatting
    /// method, nested [`FMT_NESTING`] deep; and the unbounded indirect calls
    /// those methods reach, which the task's own walk never sees.
    fn fmt_charge(&self, entry: &'a str, hook: u64) -> (u64, BTreeSet<String>) {
        let targets = self.fmt_targets(entry);
        let (mut charges, mut unbounded) = (Charges { fmt: 0, hook }, BTreeSet::new());
        for _ in 0..FMT_NESTING {
            let (depth, nested) = self.deepest(targets.iter().copied(), charges);
            charges.fmt = depth;
            unbounded.extend(nested);
        }
        (charges.fmt, unbounded)
    }

    /// The deepest path from `entry` under `charges`, and the functions where
    /// recursion was cut.
    fn worst(&self, entry: &str, charges: Charges) -> (Walk, Vec<String>) {
        let (mut memo, mut cut) = (HashMap::new(), BTreeSet::new());
        let walk = self.walk(entry, charges, &mut Vec::new(), &mut memo, &mut cut);
        (walk, cut.into_iter().collect())
    }

    fn walk(
        &self,
        name: &str,
        charges: Charges,
        visiting: &mut Vec<String>,
        memo: &mut HashMap<String, Walk>,
        cut: &mut BTreeSet<String>,
    ) -> Walk {
        if visiting.iter().any(|v| v == name) {
            cut.insert(name.to_string());
            return (0, Vec::new(), BTreeSet::new());
        }
        if let Some(known) = memo.get(name) {
            return known.clone();
        }
        let Some(f) = self.functions.get(name) else {
            return (0, vec![name.to_string()], BTreeSet::new());
        };

        let (mut best, mut path, mut vendor) = (0, Vec::new(), BTreeSet::new());
        match self.classes.get(name) {
            Some(Indirect::Fmt) => (best, path) = (charges.fmt, vec!["<fmt>".to_string()]),
            Some(Indirect::Hook) => (best, path) = (charges.hook, vec!["<provider>".to_string()]),
            Some(Indirect::Unbounded) => {
                vendor.insert(name.to_string());
            }
            None => {}
        }
        visiting.push(name.to_string());
        for callee in &f.calls {
            let (depth, deeper, nested) = self.walk(callee, charges, visiting, memo, cut);
            vendor.extend(nested);
            if depth > best {
                (best, path) = (depth, deeper);
            }
        }
        visiting.pop();

        let mut full = vec![name.to_string()];
        full.extend(path);
        let result = (f.frame + best, full, vendor);
        memo.insert(name.to_string(), result.clone());
        result
    }

    /// The deepest of `targets` under `charges`, and the unbounded indirect
    /// calls any of them reaches.
    fn deepest<'n>(
        &self,
        targets: impl Iterator<Item = &'n str>,
        charges: Charges,
    ) -> (u64, BTreeSet<String>) {
        let (mut best, mut unbounded) = (0, BTreeSet::new());
        for target in targets {
            let ((depth, _, nested), _) = self.worst(target, charges);
            best = best.max(depth);
            unbounded.extend(nested);
        }
        (best, unbounded)
    }

    /// What a call into the flash hooks is charged: the deepest provider
    /// callback, with its own formatting charged as for a task.
    /// A callback that reaches an unclassified Rust indirect call is refused,
    /// as a task would be: nothing else walks the callbacks.
    fn hook_charge(&self) -> Result<u64> {
        let hooks = self.classes.values().any(|class| *class == Indirect::Hook);
        let callbacks: Vec<&str> = self
            .functions
            .keys()
            .filter(|name| PROVIDER_CALLBACKS.iter().any(|end| name.ends_with(end)))
            .map(String::as_str)
            .collect();
        match (hooks, callbacks.is_empty()) {
            (false, true) => return Ok(0),
            (true, true) => {
                return refuse(
                    "The flash hooks link, but no provider callback the runtime installs",
                );
            }
            (false, false) => {
                return refuse(
                    "Provider callbacks link, but no flash-hook site calls them: update HOOK_SITES",
                );
            }
            (true, false) => {}
        }

        let mut best = 0;
        for callback in callbacks {
            let (fmt, mut unbounded) = self.fmt_charge(callback, 0);
            let ((depth, _, nested), _) = self.worst(callback, Charges { fmt, hook: 0 });
            unbounded.extend(nested);
            if let Some(rust) = unbounded.iter().find(|name| name.starts_with("_R")) {
                return refuse(format!(
                    "The flash-hook callback {} reaches an unbounded indirect call in {}",
                    demangle(callback),
                    demangle(rust)
                ));
            }
            best = best.max(depth);
        }
        Ok(best)
    }

    /// The charges for `entry`'s task, and the unbounded indirect calls its
    /// formatting methods reach.
    fn charges(&self, entry: &'a str, hook: u64) -> (Charges, BTreeSet<String>) {
        let (fmt, unbounded) = self.fmt_charge(entry, hook);
        (Charges { fmt, hook }, unbounded)
    }
}

/// `functions` with each veneer leading to its target.
fn link(mut functions: HashMap<String, Function>) -> HashMap<String, Function> {
    for (name, f) in &mut functions {
        if let Some(target) = veneer(name) {
            f.calls.insert(target.to_string());
        }
    }
    functions
}

/// `__name_veneer`'s target.
fn veneer(name: &str) -> Option<&str> {
    name.strip_prefix("__")?.strip_suffix("_veneer")
}

/// Per-task bounds; refuses if a task's bound leaves less than MARGIN free,
/// or if the firmware starts more tasks than its share of the slots.
pub(crate) fn audit(elf: &Path, objdump: &Path, sources: &TaskSources) -> Result<StackAudit> {
    // A repeated heading starts that function over, as a later definition.
    let functions = link(frames(&tool(objdump, &["-d"], elf)?).into_iter().collect());
    let data = loaded(elf, objdump)?;
    let graph = Graph::new(&functions, &data);
    let hook = graph.hook_charge()?;
    let mut by_short: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for name in functions.keys() {
        let short = demangle(name)
            .rsplit("::")
            .next()
            .unwrap_or_default()
            .to_string();
        by_short.entry(short).or_default().push(name.clone());
    }

    let firmware_tasks = firmware_tasks(sources, |entry| by_short.contains_key(entry))?;

    let mut rows = BTreeMap::new();
    let source = format!("{}\n{}", sources.runtime, sources.firmware());
    for (entry, (task, stack)) in tasks(&source)? {
        let symbols = by_short.get(&entry).cloned().unwrap_or_default();
        let symbol = match &symbols[..] {
            // Not linked in this variant.
            [] => continue,
            [symbol] => symbol,
            _ => return refuse(format!("Ambiguous task entry {entry}: {symbols:?}")),
        };

        let (charges, mut unbounded) = graph.charges(symbol, hook);
        let ((depth, path, direct), cut) = graph.worst(symbol, charges);
        unbounded.extend(direct);
        if let Some(rust) = unbounded.iter().find(|name| name.starts_with("_R")) {
            return refuse(format!(
                "Task {task} reaches an unbounded indirect call in {}: give it a class in the \
                 stack audit or call it directly",
                demangle(rust)
            ));
        }
        let bound = depth + OVERHEAD;
        let path: Vec<String> = path.iter().take(12).map(|n| demangle(n)).collect();
        if bound as f64 > stack as f64 * (1.0 - MARGIN) {
            return refuse(format!(
                "Task {task} may overflow: static bound {bound} bytes of {stack} (margin {:.0}%); \
                 deepest path {:?}",
                MARGIN * 100.0,
                &path[..path.len().min(6)]
            ));
        }
        rows.insert(
            task,
            TaskBound {
                entry,
                stack,
                bound,
                fmt_bound: charges.fmt,
                unbounded: unbounded.iter().map(|n| demangle(n)).collect(),
                recursion: cut.iter().map(|n| demangle(n)).collect(),
                path,
            },
        );
    }

    if rows.is_empty() {
        return refuse("No application task entries found in the linked image");
    }
    Ok(StackAudit {
        tasks: rows,
        firmware_tasks,
        firmware_task_limit: FIRMWARE_TASKS,
        overhead: OVERHEAD,
        margin: MARGIN,
        fmt_nesting: FMT_NESTING,
        provider_bound: hook,
        scope: "Direct calls, veneers and prologue frames; indirect calls in core::fmt and the \
                flash hooks bounded by class; vendor C indirect calls listed and covered by the \
                margin"
            .into(),
    })
}

/// Every task the firmware starts is one it declares with `#[task]` in its
/// own sources, where the audit reads its stack. A task declared in a
/// library, behind a renamed attribute or inside `cfg_attr` would otherwise
/// link with its stack unchecked, so starting one is refused; so is a `spawn`
/// the audit cannot read, such as a renamed import.
fn spawned_tasks_are_declared(own: &str) -> Result<()> {
    let code = LINE_COMMENT.replace_all(own, "");
    let declared: BTreeSet<&str> = DECLARED
        .captures_iter(own)
        .filter_map(|c| c.get(3).map(|entry| entry.as_str()))
        .collect();

    let spawned: Vec<String> = SPAWNED
        .captures_iter(&code)
        .map(|c| c[1].to_string())
        .collect();
    let mentions = SPAWN_WORD.find_iter(&code).count();
    if mentions != spawned.len() {
        return refuse(format!(
            "{} uses of spawn are not wellfed_sdk::spawn(entry): start each task by that \
             call, so the stack audit can read it",
            mentions - spawned.len().min(mentions)
        ));
    }
    if let Some(entry) = spawned
        .iter()
        .find(|entry| !declared.contains(entry.as_str()))
    {
        return refuse(format!(
            "The firmware spawns {entry}, which no #[wellfed_sdk::task] in its own sources \
             declares: declare the task in the firmware crate, so the stack audit reads its stack"
        ));
    }
    Ok(())
}

/// The firmware's tasks that `linked` finds in the image, refused past its
/// share: a firmware that takes a runtime task's slot leaves the build unable
/// to save settings or see an update offer. A firmware that creates a task
/// itself, past `wellfed_sdk::spawn`, is refused too, since that task would
/// not count against the share at run time.
fn firmware_tasks(sources: &TaskSources, linked: impl Fn(&str) -> bool) -> Result<Vec<String>> {
    if CREATE_WORD.is_match(&LINE_COMMENT.replace_all(&sources.own, "")) {
        return refuse(
            "The firmware creates a task with task_create: declare it with \
             #[wellfed_sdk::task] and start it with wellfed_sdk::spawn",
        );
    }
    spawned_tasks_are_declared(&sources.own)?;

    let found: Vec<String> = tasks(&sources.firmware())?
        .into_iter()
        .filter(|(entry, _)| linked(entry))
        .map(|(_, (task, _))| task)
        .collect();
    if found.len() > FIRMWARE_TASKS {
        return refuse(format!(
            "The firmware starts {} tasks ({found:?}); the runtime leaves it {FIRMWARE_TASKS} of the \
             SDK's eight slots",
            found.len()
        ));
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A task the SDK creates outside these files would link unaudited.
    #[test]
    fn every_sdk_file_that_creates_a_task_is_audited() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let mut files = Vec::new();
        rust_files(&root.join("src/sdk/wellfed-sdk/src"), &mut files).unwrap();

        for file in files {
            let text = std::fs::read_to_string(&file).unwrap();
            let relative = file
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if TASK.is_match(&text) {
                assert!(
                    relative == RUNTIME_SOURCE || SERVICE_SOURCES.contains(&relative.as_str()),
                    "{relative}"
                );
            }
        }
    }

    #[test]
    fn the_firmware_share_matches_the_sdk_budget() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let tasks =
            std::fs::read_to_string(root.join("src/sdk/wellfed-sdk/src/tasks.rs")).unwrap();

        assert!(
            tasks.contains(&format!("pub const FIRMWARE_TASKS: u8 = {FIRMWARE_TASKS};")),
            "wellfed-sdk's FIRMWARE_TASKS differs from the audit's"
        );
    }

    #[test]
    fn only_linked_firmware_tasks_count_and_one_past_the_share_is_refused() {
        let sources = |own: &str| TaskSources {
            runtime: "sdk::task_create(c\"wellfed-net\", 1536, 2, net_task, ptr::null_mut());"
                .into(),
            services: "sdk::task_create(c\"wellfed-key\", 512, 3, key_task, ptr::null_mut());"
                .into(),
            own: own.into(),
        };
        let four: String = ["a", "b", "c", "d"]
            .map(|t| format!("#[wellfed_sdk::task(name = \"fw-{t}\", stack = 512, priority = 2)]\nfn {t}_task() -> u32 {{ 0 }}\n"))
            .concat();

        let linked = |entry: &str| entry != "key_task";
        assert_eq!(
            firmware_tasks(&sources(&four), linked).unwrap(),
            ["fw-a", "fw-b", "fw-c", "fw-d"],
            "an unused service links no task"
        );

        let refused = firmware_tasks(&sources(&four), |_| true)
            .unwrap_err()
            .to_string();
        assert!(refused.contains("starts 5 tasks"), "{refused}");
    }

    /// A task whose declaration the audit cannot read would link with its
    /// stack unchecked.
    #[test]
    fn a_task_started_but_not_declared_here_is_refused() {
        let own = |text: &str| TaskSources {
            runtime: String::new(),
            services: String::new(),
            own: text.into(),
        };
        let declared = "#[wellfed_sdk::task(name = \"fw-a\", stack = 512, priority = 2)]\n\
                        fn a_task() -> u32 { 0 }\n";

        let fine =
            format!("{declared}// spawn in a comment is not a call\nwellfed_sdk::spawn(a_task);");
        assert_eq!(firmware_tasks(&own(&fine), |_| true).unwrap(), ["fw-a"]);

        for (shape, why) in [
            (
                "wellfed_sdk::spawn(wellfed_probes::x_task);",
                "uses of spawn",
            ),
            ("wellfed_sdk::spawn(x_task);", "spawns x_task"),
            (
                "use wellfed_sdk::spawn as go;\ngo(a_task);",
                "uses of spawn",
            ),
            (
                "#[cfg_attr(feature = \"x\", wellfed_sdk::task(name = \"fw-x\", stack = 512, priority = 2))]\n\
                 fn x_task() -> u32 { 0 }\nwellfed_sdk::spawn(x_task);",
                "spawns x_task",
            ),
        ] {
            let text = format!("{declared}{shape}");
            let refused = firmware_tasks(&own(&text), |_| true)
                .unwrap_err()
                .to_string();
            assert!(refused.contains(why), "{shape}: {refused}");
        }
    }

    /// A task the firmware creates itself would pass the run-time budget.
    #[test]
    fn a_firmware_that_creates_a_task_itself_is_refused() {
        let sources = TaskSources {
            runtime: String::new(),
            services: String::new(),
            own: "unsafe { sdk::task_create(c\"fw-raw\", 512, 2, raw_task, ptr::null_mut()) };"
                .into(),
        };

        let refused = firmware_tasks(&sources, |_| true).unwrap_err().to_string();
        assert!(
            refused.contains("creates a task with task_create"),
            "{refused}"
        );

        let named = TaskSources {
            own: "unsafe { sdk::task_create(name, 512, 2, raw_task, ptr::null_mut()) };".into(),
            ..sources
        };
        let refused = firmware_tasks(&named, |_| true).unwrap_err().to_string();
        assert!(
            refused.contains("creates a task with task_create"),
            "{refused}"
        );
    }

    const LISTING: &str = "\
00001000 <task>:
    1000:\tb5f0      \tpush\t{r4, r5, r6, r7, lr}
    1002:\tf5ad 6d4d \tsub.w\tsp, sp, #3280\t; 0xcd0
    1006:\tf000 f800 \tbl\t2000 <deep>
    100a:\tf000 f800 \tbl\t3000 <shallow>
    100e:\t4798      \tblx\tr3

00002000 <deep>:
    2000:\te92d 4ff0 \tstmdb\tsp!, {r4, r5, r6, r7, r8, r9, sl, fp, lr}
    2004:\ted2d 8b04 \tvpush\t{d8-d9}
    2008:\tb0d1      \tsub\tsp, #836
    200a:\tf000 f800 \tbl\t2000 <deep>
    200e:\tf000 b800 \tb.w\t3000 <shallow>

00003000 <shallow>:
    3000:\tb510      \tpush\t{r4, lr}
    3002:\tbd10      \tpop\t{r4, pc}

00004000 <wide>:
    4000:\tb580      \tpush\t{r7, lr}
    4002:\tf2ad 7d9c \tsubw\tsp, sp, #1948\t; 0x79c
";

    #[test]
    fn tasks_resolve_word_constants() {
        let source = "const PROBE_STACK_WORDS: u16 = 2048;\n\
                      sdk::task_create(\n    c\"wellfed-optics\",\n    PROBE_STACK_WORDS,\n    3,\n    optics_task,\n\
                      let ok = sdk::task_create(c\"wellfed-wdg\", 512, 3, feeder_task, ptr::null_mut());\n\
                      /// The chime.\n\
                      #[wellfed_sdk::task(name = \"wellfed-aud\", stack = 1024, priority = 2)]\n\
                      fn audio_task() -> u32 {\n\
                      #[wellfed_sdk::task(\n    name = \"wellfed-loop\",\n    stack = PROBE_STACK_WORDS,\n    priority = 2,\n)]\n\
                      /// The loop.\n\
                      #[cfg(feature = \"x\")]\n\
                      pub(crate) fn loop_task() -> u32 {";
        let expected: BTreeMap<String, (String, u64)> = [
            ("loop_task".to_string(), ("wellfed-loop".to_string(), 8192)),
            (
                "optics_task".to_string(),
                ("wellfed-optics".to_string(), 8192),
            ),
            (
                "feeder_task".to_string(),
                ("wellfed-wdg".to_string(), 2048),
            ),
            ("audio_task".to_string(), ("wellfed-aud".to_string(), 4096)),
        ]
        .into();
        assert_eq!(tasks(source).unwrap(), expected);
    }

    #[test]
    fn a_task_the_audit_cannot_resolve_is_refused() {
        let generic = "sdk::task_create(c\"wellfed-gen\", 2048, 3, generic_step::<S>, state);";
        let refused = tasks(generic).unwrap_err().to_string();
        assert!(refused.contains("generic generic_step::<S>"), "{refused}");

        let unknown = "#[wellfed_sdk::task(name = \"wellfed-x\", stack = ELSEWHERE_WORDS, priority = 2)]\n\
                       fn x_task() -> u32 {";
        let refused = tasks(unknown).unwrap_err().to_string();
        assert!(refused.contains("ELSEWHERE_WORDS"), "{refused}");

        for shape in [
            "sdk::task_create(c\"wellfed-x\", 512, PRIORITY, x_task, ptr::null_mut());",
            "sdk::task_create(c\"wellfed-x\", 512, 2, tasks::run, ptr::null_mut());",
            "#[wellfed_sdk::task(name = \"wellfed-x\", stack = 512, priority = PRIORITY)]\nfn x_task() -> u32 {",
            "#[wellfed_sdk::task(stack = 512, name = \"wellfed-x\", priority = 2)]\nfn x_task() -> u32 {",
            "#[task(name = \"wellfed-x\", stack = 512, priority = 2)]\nstatic X: u32 = 0;",
            "const X: Task = Task::__new(c\"wellfed-x\", 512, 2, x_task);",
        ] {
            let refused = tasks(shape).unwrap_err().to_string();
            assert!(refused.contains("1 task creations"), "{shape}: {refused}");
        }
    }

    #[test]
    fn register_calls_and_tail_calls_are_indirect() {
        for call in [
            "blx\tr3", "blx\tip", "blx\tfp", "blx\tsl", "blx\tsb", "bx\tr2", "bx\tip", "bx\tsb",
        ] {
            assert!(
                INDIRECT.is_match(&format!("    1000:\t4798      \t{call}")),
                "{call}"
            );
        }
        for other in ["bx\tlr", "blx\t2000 <callee>", "bl\t2000 <callee>"] {
            assert!(
                !INDIRECT.is_match(&format!("    1000:\t4798      \t{other}")),
                "{other}"
            );
        }
    }

    #[test]
    fn deepest_path_sums_prologues_and_cuts_recursion() {
        let functions: HashMap<String, Function> = frames(LISTING).into_iter().collect();
        assert_eq!(functions["task"].frame, 20 + 3280);
        assert!(functions["task"].indirect);
        assert_eq!(functions["deep"].frame, 36 + 16 + 836);
        assert_eq!(functions["wide"].frame, 8 + 1948, "subw counts");

        let graph = Graph::new(&functions, &[]);
        let ((depth, path, unbounded), cut) = graph.worst("task", Charges::default());
        assert_eq!(depth, 3300 + 888 + 8);
        assert_eq!(path, ["task", "deep", "shallow"]);
        assert_eq!(unbounded, ["task".to_string()].into(), "vendor C, listed");
        assert_eq!(cut, ["deep"]);
    }

    /// A Rust task formatting a record through `core::fmt::write`, whose
    /// `Display` writes again, then calls into RAM through a veneer that
    /// takes the flash lock.
    const CLASSES: &str = "\
00001000 <_RNvNtCs1_3app4task>:
    1000:\tb500      \tpush\t{lr}
    100a:\tf243 0001 \tmovw\tr0, #12289\t; 0x3001
    100e:\tf2c0 0000 \tmovt\tr0, #0
    1002:\tf000 f800 \tbl\t2000 <_RNvNtCs2_4core3fmt5write>
    1006:\tf000 f800 \tbl\t5000 <__FLASH_Write_Lock_veneer>

00002000 <_RNvNtCs2_4core3fmt5write>:
    2000:\tb5f0      \tpush\t{r4, r5, r6, r7, lr}
    2002:\tb090      \tsub\tsp, #64
    2004:\t4798      \tblx\tr3

00003000 <_RNvXNtCs1_3app4LineNtNtCs2_4core3fmt7Display3fmt>:
    3000:\tb580      \tpush\t{r7, lr}
    3002:\tb088      \tsub\tsp, #32
    3004:\tf000 f800 \tbl\t2000 <_RNvNtCs2_4core3fmt5write>
    3008:\t00004001 \t.word\t0x00004001

00004000 <_RNvXNtCs1_3app6ReportNtNtCs2_4core3fmt5Write9write_str>:
    4000:\tb510      \tpush\t{r4, lr}

00005000 <__FLASH_Write_Lock_veneer>:
    5000:\tb401      \tpush\t{r0}
    5002:\t4760      \tbx\tip

00006000 <FLASH_Write_Lock>:
    6000:\tb538      \tpush\t{r3, r4, r5, lr}
    6002:\t4790      \tblx\tr2

00007000 <_RNvNtCs3_12wellfed_sdk7runtime12hook_acquire>:
    7000:\tb5f0      \tpush\t{r4, r5, r6, r7, lr}
    7002:\tb0c0      \tsub\tsp, #256

00008000 <_RNvNtCs1_3app6callee>:
    8000:\tb500      \tpush\t{lr}
    8002:\t4798      \tblx\tr3
";

    #[test]
    fn indirect_calls_are_charged_by_class_through_veneers() {
        let functions = link(frames(CLASSES).into_iter().collect());
        let graph = Graph::new(&functions, &[]);
        let task = "_RNvNtCs1_3app4task";
        assert_eq!(
            graph.fmt_targets(task).into_iter().collect::<Vec<_>>(),
            [
                "_RNvXNtCs1_3app4LineNtNtCs2_4core3fmt7Display3fmt",
                "_RNvXNtCs1_3app6ReportNtNtCs2_4core3fmt5Write9write_str"
            ],
            "the task's Display, and the Write it references"
        );
        assert!(graph.fmt_targets("_RNvNtCs1_3app6callee").is_empty());
        let (charges, through_fmt) = graph.charges(task, graph.hook_charge().unwrap());
        assert!(through_fmt.is_empty());

        // `write` (84) under each nesting level: Display (40) + write again,
        // four levels deep, the innermost charged nothing.
        let write = 20 + 64;
        let display = 8 + 32;
        assert_eq!(charges.fmt, 4 * (display + write));
        assert_eq!(charges.hook, 20 + 256, "the acquire callback");

        let ((depth, path, unbounded), _) = graph.worst(task, charges);
        let lock = 4 + 16 + charges.hook;
        assert_eq!(depth, 4 + (write + charges.fmt).max(lock));
        assert_eq!(
            path[..3],
            ["_RNvNtCs1_3app4task", "_RNvNtCs2_4core3fmt5write", "<fmt>"]
        );
        assert!(unbounded.is_empty());
    }

    #[test]
    fn an_unclassified_rust_indirect_call_is_reported_for_refusal() {
        let functions = link(frames(CLASSES).into_iter().collect());
        let graph = Graph::new(&functions, &[]);

        let ((_, _, unbounded), _) = graph.worst("_RNvNtCs1_3app6callee", Charges::default());
        assert_eq!(unbounded, ["_RNvNtCs1_3app6callee".to_string()].into());
        assert!(FMT_METHOD.is_match("_RNvXNtCs1_3app4LineNtNtCs2_4core3fmt7Display3fmt"));
        assert!(!FMT_METHOD.is_match("_RNvNtCs2_4core3fmt5write"));
    }

    #[test]
    fn an_unbounded_call_reached_only_through_formatting_is_reported() {
        let listing = CLASSES.replace(
            "    3008:\t00004001",
            "    3006:\tf000 f800 \tbl\t8000 <_RNvNtCs1_3app6callee>\n    3008:\t00004001",
        );
        let functions = link(frames(&listing).into_iter().collect());
        let graph = Graph::new(&functions, &[]);

        let task = "_RNvNtCs1_3app4task";
        let (charges, through_fmt) = graph.charges(task, graph.hook_charge().unwrap());
        let ((_, _, direct), _) = graph.worst(task, charges);
        assert!(direct.is_empty(), "the task's own walk never meets it");
        assert_eq!(through_fmt, ["_RNvNtCs1_3app6callee".to_string()].into());
    }

    #[test]
    fn a_hook_callback_reaching_an_unbounded_rust_call_is_refused() {
        let listing = CLASSES.replace(
            "    7002:\tb0c0      \tsub\tsp, #256\n",
            "    7002:\tb0c0      \tsub\tsp, #256\n    7004:\tf000 f800 \tbl\t8000 <_RNvNtCs1_3app6callee>\n",
        );
        assert_ne!(listing, CLASSES);
        let functions = link(frames(&listing).into_iter().collect());
        assert!(Graph::new(&functions, &[]).hook_charge().is_err());
    }

    #[test]
    fn provider_callbacks_without_a_hook_site_are_refused() {
        let listing = "\
00007000 <_RNvNtCs3_12wellfed_sdk7runtime12hook_acquire>:
    7000:\tb5f0      \tpush\t{r4, r5, r6, r7, lr}
";
        let functions = link(frames(listing).into_iter().collect());
        assert!(Graph::new(&functions, &[]).hook_charge().is_err());
    }

    #[test]
    fn hooks_without_a_provider_callback_are_refused() {
        let listing = "\
00006000 <FLASH_Write_Lock>:
    6000:\tb538      \tpush\t{r3, r4, r5, lr}
    6002:\t4790      \tblx\tr2
";
        let functions = link(frames(listing).into_iter().collect());
        assert!(Graph::new(&functions, &[]).hook_charge().is_err());
    }
}
