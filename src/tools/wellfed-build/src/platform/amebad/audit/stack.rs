//! Worst-case static stack of each KM4 application task, from the linked
//! image.
//!
//! A crate-split build overflowed the optics task's 4 KiB stack on hardware:
//! LTO inlined a 3 KiB temporary into the task's frame. The SDK's ARMv8-M
//! port faults on the stack limit, and FW11's trial caught it, but only after
//! an update. This audit catches it at build time instead.
//!
//! For each task created in the runtime or the firmware
//! (`sdk::task_create(c"name", words, ...)` or `wellfed_sdk::spawn(...)`),
//! it walks the direct call graph from the task's entry function and
//! sums the frames along the deepest path. Frames come from each function's
//! prologue (`push`, `stmdb sp!`, `vpush`, `sub sp`). Recursion is cut at the
//! cycle, and indirect calls (`blx rN`, function pointers, `dyn` formatting)
//! cannot be followed, so the bound requires a margin rather than an exact
//! fit.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

use super::writes::demangle;
use super::{HEADING, refuse, tool};
use crate::{Firmware, Repo, Result};

/// The SDK runtime's task table; the firmware's tasks are found in its
/// `src/`.
pub(crate) const RUNTIME_SOURCE: &str = "src/sdk/wellfed-sdk/src/runtime.rs";

/// The task sources' text, which the audit reads its task table from: the
/// runtime, then every Rust file of the firmware's `src/`, in path order.
pub(crate) fn task_sources(repo: &Repo, firmware: &Firmware) -> Result<String> {
    let mut paths = Vec::new();
    rust_files(&repo.path(&format!("{}/src", firmware.dir())), &mut paths)?;
    paths.sort();

    let mut text = std::fs::read_to_string(repo.path(RUNTIME_SOURCE))?;
    for path in paths {
        text.push('\n');
        text.push_str(&std::fs::read_to_string(path)?);
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
/// Unfollowed indirect calls: the bound must leave this share of the stack
/// free.
const MARGIN: f64 = 0.25;

static TASK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)(?:task_create|spawn)\(\s*c"([\w-]+)",\s*(\w+),\s*\d+,\s*(\w+)\s*[,)]"#)
        .expect("stack audit pattern")
});
static CONSTANT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"const (\w+): u16 = (\d+);").expect("stack audit pattern"));
static CALL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\t(?:bl|blx|b\.w|b)(?:\.[nw])?\t[0-9a-f]+ <([^>+]+)>")
        .expect("stack audit pattern")
});
static INDIRECT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\tblx\tr\d+\b").expect("stack audit pattern"));
static PUSH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:push(?:\.w)?|stmdb(?:\.w)?\s+sp!,)\s*\{([^}]*)\}")
        .expect("stack audit pattern")
});
static VPUSH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^vpush\s*\{d(\d+)-d(\d+)\}").expect("stack audit pattern"));
static SUB: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^sub(?:\.w)?\s+sp,\s*(?:sp,\s*)?#(\d+)").expect("stack audit pattern")
});

/// {entry function name: (task name, stack bytes)} from the task sources.
fn tasks(source: &str) -> BTreeMap<String, (String, u64)> {
    let constants: HashMap<&str, u64> = CONSTANT
        .captures_iter(source)
        .filter_map(|c| Some((c.get(1)?.as_str(), c[2].parse().ok()?)))
        .collect();
    TASK.captures_iter(source)
        .filter_map(|c| {
            let words = constants
                .get(&c[2])
                .copied()
                .or_else(|| c[2].parse().ok())?;
            Some((c[3].to_string(), (c[1].to_string(), words * 4)))
        })
        .collect()
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Function {
    frame: u64,
    calls: BTreeSet<String>,
    indirect: bool,
    sized: bool,
}

/// Each function's prologue frame, direct callees and whether it makes
/// indirect calls, from `objdump -d`, in listing order.
fn frames(listing: &str) -> Vec<(String, Function)> {
    let mut functions: Vec<(String, Function)> = Vec::new();
    for line in listing.lines() {
        if let Some(heading) = HEADING.captures(line) {
            functions.push((heading[2].to_string(), Function::default()));
            continue;
        }
        let Some((_, f)) = functions.last_mut() else {
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
            f.indirect = true;
        }
    }
    functions
}

/// The deepest path from `entry`: (bytes, path, indirect calls on any path,
/// functions where recursion was cut).
fn worst(
    functions: &HashMap<String, Function>,
    entry: &str,
) -> (u64, Vec<String>, bool, Vec<String>) {
    type Walk = (u64, Vec<String>, bool);

    fn walk(
        name: &str,
        visiting: &mut Vec<String>,
        functions: &HashMap<String, Function>,
        memo: &mut HashMap<String, Walk>,
        cut: &mut BTreeSet<String>,
    ) -> Walk {
        if visiting.iter().any(|v| v == name) {
            cut.insert(name.to_string());
            return (0, Vec::new(), false);
        }
        if let Some(known) = memo.get(name) {
            return known.clone();
        }
        let Some(f) = functions.get(name) else {
            return (0, vec![name.to_string()], false);
        };

        let (mut best, mut path, mut indirect) = (0, Vec::new(), f.indirect);
        visiting.push(name.to_string());
        for callee in &f.calls {
            let (depth, deeper, nested) = walk(callee, visiting, functions, memo, cut);
            indirect |= nested;
            if depth > best {
                (best, path) = (depth, deeper);
            }
        }
        visiting.pop();

        let mut full = vec![name.to_string()];
        full.extend(path);
        let result = (f.frame + best, full, indirect);
        memo.insert(name.to_string(), result.clone());
        result
    }

    let (mut memo, mut cut) = (HashMap::new(), BTreeSet::new());
    let (depth, path, indirect) = walk(entry, &mut Vec::new(), functions, &mut memo, &mut cut);
    (depth, path, indirect, cut.into_iter().collect())
}

/// One task's static stack bound.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct TaskBound {
    entry: String,
    stack: u64,
    bound: u64,
    /// Whether any path makes an indirect call the walk cannot follow.
    indirect: bool,
    /// Functions where recursion was cut.
    recursion: Vec<String>,
    /// The deepest path's first functions.
    path: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct StackAudit {
    /// Keyed by task name.
    tasks: BTreeMap<String, TaskBound>,
    overhead: u64,
    margin: f64,
    scope: &'static str,
}

/// Per-task bounds; refuses if a task's bound leaves less than MARGIN free.
pub(crate) fn audit(elf: &Path, objdump: &Path, source: &str) -> Result<StackAudit> {
    // A repeated heading starts that function over, as a later definition.
    let functions: HashMap<String, Function> =
        frames(&tool(objdump, &["-d"], elf)?).into_iter().collect();
    let mut by_short: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for name in functions.keys() {
        let short = demangle(name)
            .rsplit("::")
            .next()
            .unwrap_or_default()
            .to_string();
        by_short.entry(short).or_default().push(name.clone());
    }

    let mut rows = BTreeMap::new();
    for (entry, (task, stack)) in tasks(source) {
        let symbols = by_short.get(&entry).cloned().unwrap_or_default();
        let symbol = match &symbols[..] {
            // Not linked in this variant.
            [] => continue,
            [symbol] => symbol,
            _ => return refuse(format!("Ambiguous task entry {entry}: {symbols:?}")),
        };

        let (depth, path, indirect, cut) = worst(&functions, symbol);
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
                indirect,
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
        overhead: OVERHEAD,
        margin: MARGIN,
        scope: "Direct calls and prologue frames only; indirect calls are covered by the margin, \
                not bounded",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
";

    #[test]
    fn tasks_resolve_word_constants() {
        let source = "const PROBE_STACK_WORDS: u16 = 2048;\n\
                      sdk::task_create(\n    c\"wellfed-optics\",\n    PROBE_STACK_WORDS,\n    3,\n    optics_task,\n\
                      let ok = sdk::task_create(c\"wellfed-wdg\", 512, 3, feeder_task, ptr::null_mut());\n\
                      unsafe { wellfed_sdk::spawn(c\"wellfed-aud\", 1024, 2, audio_task) };";
        let expected: BTreeMap<String, (String, u64)> = [
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
        assert_eq!(tasks(source), expected);
    }

    #[test]
    fn deepest_path_sums_prologues_and_cuts_recursion() {
        let functions: HashMap<String, Function> = frames(LISTING).into_iter().collect();
        assert_eq!(functions["task"].frame, 20 + 3280);
        assert!(functions["task"].indirect);
        assert_eq!(functions["deep"].frame, 36 + 16 + 836);

        let (depth, path, indirect, cut) = worst(&functions, "task");
        assert_eq!(depth, 3300 + 888 + 8);
        assert_eq!(path, ["task", "deep", "shallow"]);
        assert!(indirect);
        assert_eq!(cut, ["deep"]);
    }
}
