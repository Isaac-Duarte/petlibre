//! The deliberately limited SDK mains of the UART-only first-boot profile.
//!
//! Each core's linked `main` must make exactly the reviewed calls, in order,
//! with constant ROM pointers and the linker's standard veneers resolved, and
//! end in its terminal loop. This checks linked direct calls, not a complete
//! startup or ROM side-effect proof.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

use super::hooks::file_offset;
use super::{ADDRESSED, Binutils, DIRECT_BRANCH, INSTRUCTION, elf, hex, parse_hex, refuse, tool};
use crate::{Result, require};

pub fn expected(core: &str) -> &'static [&'static str] {
    match core {
        "lp" => &[
            "irq_register",
            "irq_enable",
            "pmu_acquire_wakelock",
            "irq_disable",
            "km4_flash_highspeed_init",
            "wellfed_shared_pad_power_init",
            "wellfed_trial_step",
            "wellfed_wifi_fw_start",
            "km4_boot_on",
            "wellfed_flash_peer_start",
            "vTaskStartScheduler",
        ],
        _ => &[
            "irq_register",
            "irq_enable",
            "ipc_table_init",
            "pmu_acquire_wakelock",
            "pre_example_entry",
            "example_entry",
            "vTaskStartScheduler",
        ],
    }
}

static TARGET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([0-9a-f]+)\s+<([^>]+)>").unwrap());
static REGISTER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:r(?:[0-9]|1[0-2])|ip|lr)$").unwrap());
static PC_LITERAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(r[0-9]+|ip), \[pc, #(\d+)\]\s*$").unwrap());
/// The standard Thumb long-branch veneer body, before its target word.
const VENEER: [u8; 12] = [
    0x01, 0xb4, 0x02, 0x48, 0x84, 0x46, 0x01, 0xbc, 0x60, 0x47, 0x00, 0xbf,
];

/// Resolve constant ROM calls; reject unknown, missing or reordered calls.
pub fn check_main(
    listing: &str,
    address: u64,
    size: u64,
    core: &str,
    symbols: &HashMap<String, u64>,
    veneers: &HashMap<u64, String>,
) -> Result<Value> {
    let reviewed = expected(core);
    let inside = |at: u64| address <= at && at < address + size;

    let mut literals = HashMap::new();
    for line in listing.lines() {
        if let Some(m) = INSTRUCTION.captures(line).filter(|m| &m[2] == ".word") {
            literals.insert(
                parse_hex(&m[1])?,
                parse_hex(m[3].split_whitespace().next().unwrap_or_default())?,
            );
        }
    }

    let mut calls: Vec<String> = Vec::new();
    let mut registers: HashMap<String, u64> = HashMap::new();
    let mut called = |target: u64, registers: &mut HashMap<String, u64>| -> Result<()> {
        let name = match veneers.get(&(target & !1)) {
            Some(name) => name.clone(),
            None => {
                let names: Vec<&&str> = reviewed
                    .iter()
                    .filter(|name| symbols.get(**name).is_some_and(|s| s & !1 == target & !1))
                    .collect();
                match names[..] {
                    [name] => name.to_string(),
                    _ => return refuse("Unknown/ambiguous first-boot call target"),
                }
            }
        };
        calls.push(name);
        for register in ["r0", "r1", "r2", "r3", "ip", "lr"] {
            registers.remove(register);
        }
        Ok(())
    };

    let (mut instructions, mut terminal_loop) = (0u32, false);
    for line in listing.lines() {
        let Some(m) = INSTRUCTION.captures(line) else {
            if ADDRESSED.is_match(line) {
                return refuse("Unparsed first-boot instruction");
            }
            continue;
        };
        let (location, mnemonic, operands) = (parse_hex(&m[1])?, &m[2], &m[3]);
        require(inside(location), "Instruction outside first-boot main")?;
        if mnemonic == ".word" {
            continue;
        }
        require(
            ["push", "ldr", "ldr.w", "movs", "blx", "bl", "b.n", "nop"].contains(&mnemonic),
            format!("Unreviewed first-boot instruction: {mnemonic}"),
        )?;
        require(
            !terminal_loop || mnemonic == "nop",
            "Instructions after first-boot terminal loop",
        )?;
        instructions += 1;

        if mnemonic == "blx" {
            let target = registers.get(operands.trim()).copied();
            let Some(target) = target.filter(|t| t & 1 == 1) else {
                return refuse("Unresolved indirect branch in first-boot main");
            };
            called(target, &mut registers)?;
            continue;
        }
        if mnemonic == "bl" || DIRECT_BRANCH.is_match(mnemonic) {
            let Some(target) = TARGET.captures(operands) else {
                return refuse("Unresolved first-boot branch");
            };
            let target = parse_hex(&target[1])?;
            if inside(target) {
                require(
                    mnemonic == "b.n" && target == location,
                    "Unexpected branch inside first-boot main",
                )?;
                terminal_loop = true;
                continue;
            }
            called(target, &mut registers)?;
            continue;
        }

        // Literal loads may read constants; no other PC destination is allowed.
        let code = operands.split(';').next().unwrap_or_default();
        let destination = code.split(',').next().unwrap_or_default().trim();
        require(
            destination != "pc" && !(mnemonic == "pop" && code.contains("pc")),
            "Indirect PC write in first-boot main",
        )?;
        if REGISTER.is_match(destination) {
            registers.remove(destination);
            let literal = PC_LITERAL.captures(code.trim());
            if let Some(literal) = literal.filter(|_| mnemonic == "ldr" || mnemonic == "ldr.w") {
                let at = ((location + 4) & !3) + literal[2].parse::<u64>().unwrap_or(u64::MAX / 2);
                let Some(value) = literals.get(&at).filter(|_| inside(at)) else {
                    return refuse("Unresolved first-boot ROM pointer literal");
                };
                registers.insert(destination.to_string(), *value);
            }
        }
    }

    require(
        instructions > 0 && terminal_loop && calls == reviewed,
        format!("Unexpected first-boot {core} main calls: {calls:?}"),
    )?;
    Ok(json!({"instructions": instructions, "resolved_calls": calls, "unresolved_calls": 0}))
}

/// Symbol addresses by name.
pub type Symbols = HashMap<String, u64>;

/// One core's symbol addresses and every `main`'s address and size.
pub fn listing_inputs(elf: &Path, objdump: &Path) -> Result<(Symbols, Vec<(u64, u64)>)> {
    let mut symbols = HashMap::new();
    let mut mains = Vec::new();
    for line in tool(objdump, &["-t"], elf)?.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() >= 4
            && fields[0]
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            symbols.insert(fields[fields.len() - 1].to_string(), parse_hex(fields[0])?);
        }
        if fields.len() == 6 && fields[5] == "main" && fields[2] == "F" {
            mains.push((parse_hex(fields[0])?, parse_hex(fields[4])?));
        }
    }
    Ok((symbols, mains))
}

pub fn audit(sdk: &Path, binutils: &Binutils) -> Result<Value> {
    let objdump = binutils.objdump();
    let mut result = serde_json::Map::new();

    for core in ["lp", "hp"] {
        let elf = elf(sdk, core);
        let (symbols, mains) = listing_inputs(&elf, &objdump)?;
        let [(address, size)] = mains[..] else {
            return refuse("Expected exactly one linked first-boot main");
        };

        let data = fs::read(&elf)?;
        let mut veneers = HashMap::new();
        for name in expected(core) {
            let Some(&at) = symbols.get(&format!("__{name}_veneer")) else {
                continue;
            };
            let Some(&target) = symbols.get(*name) else {
                return refuse(format!("Veneer without its target: {name}"));
            };
            let offset = file_offset(&elf, &objdump, at, 16)?;
            let mut body = VENEER.to_vec();
            body.extend_from_slice(&((target | 1) as u32).to_le_bytes());
            require(
                data.get(offset..offset + 16) == Some(&body[..]),
                "Unreviewed first-boot veneer instructions/target",
            )?;
            veneers.insert(at, name.to_string());
        }

        let listing = tool(
            &objdump,
            &[
                "-d",
                &format!("--start-address={}", hex(address)),
                &format!("--stop-address={}", hex(address + size)),
            ],
            &elf,
        )?;
        let mut main = json!({"address": hex(address), "bytes": size});
        if let (Some(out), Value::Object(checked)) = (
            main.as_object_mut(),
            check_main(&listing, address, size, core, &symbols, &veneers)?,
        ) {
            out.extend(checked);
        }
        result.insert(core.to_string(), main);
    }

    Ok(
        json!({"mains": result, "network_or_shell_started_by_main": false,
              "feeder_backend_opened_by_application": false,
              "scope": "Exact linked main call sequences, constant ROM pointers and linker veneers; vendor pre-main, IRQ handlers, \
                        flash status/calibration, shared-pad supply helper and ROM effects require separate review"}),
    )
}
