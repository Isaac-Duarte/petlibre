//! The retained, leaf MMIO parking routines in both packaged core images.
//!
//! While flash is parked for a write, these routines run from RAM, so none
//! may call out, branch away, or form an address in the flash map. This
//! checks emitted instructions and placement, not MMIO ownership, boot
//! readiness, interrupt/fault-vector behavior or global SDK flash
//! interception.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;
use std::sync::LazyLock;

use wellfed_formats::audit::{Leaf, ParkAudit, ParkFunction, ParkImage};
use wellfed_image::ota::{container, images};
use regex::Regex;

use super::packaged;
use super::{
    ADDRESSED, Archives, Binutils, DIRECT_BRANCH, INSTRUCTION, archive_record, elf, hex, ota_image,
    parse_hex, tool,
};
use crate::Result;
use crate::platform::amebad::Core;
use wellfed_formats::require;

/// Why a RAM parking routine is not a retained leaf. Locations are the
/// instruction addresses the listing names.
#[derive(Debug, thiserror::Error)]
pub enum ParkError {
    #[error("Unparsed parking instruction/data")]
    Unparsed { line: String },
    #[error("Instruction outside expected function")]
    Outside { location: u64 },
    #[error("Flash/XIP literal in RAM parking code")]
    FlashLiteral { location: u64, value: u64 },
    #[error("Unsupported instruction/call in RAM parking code: {line}")]
    Unsupported { line: String },
    #[error("Indirect branch in RAM parking code")]
    IndirectBranch { location: u64 },
    #[error("Flash/XIP address formed by movw/movt in RAM parking code")]
    FlashAddress { location: u64, value: u64 },
    #[error("Indirect PC destination in RAM parking code")]
    PcDestination { location: u64 },
    #[error("Indirect PC write in RAM parking code")]
    PcWrite { location: u64 },
    #[error("Unexpected system-register access")]
    SystemRegister { location: u64 },
    #[error("Branch leaves RAM parking function")]
    Branch { location: u64 },
    #[error("Literal load leaves RAM parking function")]
    Literal { location: u64 },
    #[error("Missing parking instructions/barriers")]
    Barriers { instructions: u32, barriers: u32 },
    #[error("Parking function is not in application RAM: {name}")]
    Placement { name: &'static str },
    #[error("Missing retained parking functions in {core}")]
    Missing { core: Core },
}

pub(crate) const SYMBOLS: [&str; 6] = [
    "wellfed_amebad_flash_park_publish",
    "wellfed_amebad_flash_park_wait",
    "wellfed_amebad_flash_sem_try_claim",
    "wellfed_amebad_flash_sem_release",
    "wellfed_amebad_flash_owner_begin",
    "wellfed_amebad_flash_owner_end",
];
/// KM4 only: the flash-lock provider's fault callback runs inside hook_fail.
pub const KM4_SYMBOLS: [&str; 1] = ["wellfed_amebad_flash_provider_fault"];

/// SPI flash is readable through the direct map at 0x08000000 and the RSIP
/// XIP windows at 0x0c000000/0x0e000000; parked code must not address any
/// of it.
const FLASH_MAPPING: std::ops::Range<u64> = 0x0800_0000..0x1000_0000;
const CONDITIONS: [&str; 16] = [
    "eq", "ne", "cs", "hs", "cc", "lo", "mi", "pl", "vs", "vc", "hi", "ls", "ge", "lt", "gt", "le",
];
const INTEGER: &str = "adc adcs add adds addw and ands asr asrs bic bics clz cmn cmp eor eors \
    ldm ldmia ldr ldrb ldrd ldrh ldrsb ldrsh lsl lsls lsr lsrs mov movs \
    movw movt mul muls mvn mvns negs nop orr orrs pop push rev rev16 ror rors \
    rsb rsbs sbc sbcs stm stmia stmdb str strb strd strh sub subs subw tst uxtb uxth";

static IMMEDIATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^#(-?(?:0x[0-9a-f]+|\d+))\b").expect("immediate pattern"));
static IT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^it[te]{0,3}$").expect("IT block pattern"));
static PC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bpc\b").expect("pc pattern"));
static TARGET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b([0-9a-f]+)\s+<").expect("target pattern"));
static LITERAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r";\s*\(?([0-9a-f]+)\s+<").expect("literal pattern"));

/// An immediate operand (`#4096`, `#-0x10`) as a 32-bit value: negative
/// ones as their two's complement.
fn immediate(text: &str) -> Option<u64> {
    let value = &IMMEDIATE.captures(text.trim())?[1];
    let (negative, digits) = value
        .strip_prefix('-')
        .map_or((false, value), |rest| (true, rest));
    let magnitude = match digits.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16).ok()?,
        None => digits.parse().ok()?,
    };
    let value = if negative {
        magnitude.wrapping_neg()
    } else {
        magnitude
    };
    Some(value & 0xffff_ffff)
}

fn integer(mnemonic: &str) -> bool {
    INTEGER.split_whitespace().any(|m| m == mnemonic)
}

/// Strip a condition suffix (`bxeq` is `bx`).
fn unconditional(base: &str) -> &str {
    match base.len().checked_sub(2) {
        Some(at) if base.is_char_boundary(at) && CONDITIONS.contains(&&base[at..]) => &base[..at],
        _ => base,
    }
}

/// Fail closed on calls, foreign branches, indirect jumps or flash
/// addresses in one function's listing.
///
/// Flash addresses are refused as literal-pool words and as movw/movt pairs
/// (a movt high half in the flash map is refused even without a movw).
/// Bit-mask immediates are not treated as addresses, and values computed by
/// other arithmetic are not proven.
pub fn check_leaf(
    disassembly: &str,
    address: u64,
    size: u64,
    minimum_barriers: u32,
) -> Result<Leaf> {
    let (mut count, mut barriers) = (0u32, 0u32);
    let mut low_halves: HashMap<String, u64> = HashMap::new();
    let inside = |at: u64| address <= at && at < address + size;

    for line in disassembly.lines() {
        let Some(m) = INSTRUCTION.captures(line) else {
            if ADDRESSED.is_match(line) {
                return Err(ParkError::Unparsed { line: line.into() }.into());
            }
            continue;
        };
        let (location, mnemonic, operands) = (parse_hex(&m[1])?, &m[2], &m[3]);
        require(inside(location), || ParkError::Outside { location })?;

        if mnemonic == ".word" {
            let value = parse_hex(operands.split_whitespace().next().unwrap_or_default())?;
            require(!FLASH_MAPPING.contains(&value), || {
                ParkError::FlashLiteral { location, value }
            })?;
            continue;
        }

        let base = mnemonic.split('.').next().unwrap_or_default();
        let conditional = unconditional(base);
        let branch = DIRECT_BRANCH.is_match(mnemonic) || base == "cbz" || base == "cbnz";
        // Conditional returns (`bxeq lr` inside an IT block) are still leaf returns.
        if !(integer(base)
            || integer(conditional)
            || branch
            || ["bx", "mrs", "dsb"].contains(&base)
            || conditional == "bx"
            || IT.is_match(base))
        {
            return Err(ParkError::Unsupported { line: line.into() }.into());
        }
        if conditional == "bx" && operands.trim() != "lr" {
            return Err(ParkError::IndirectBranch { location }.into());
        }

        let code = operands.split(';').next().unwrap_or_default();
        let fields: Vec<&str> = code.split(',').map(str::trim).collect();
        let operand = (fields.len() == 2).then(|| immediate(fields[1])).flatten();
        if let (Some(low), "movw") = (operand, conditional) {
            low_halves.insert(fields[0].into(), low & 0xffff);
        } else if let (Some(high), "movt") = (operand, conditional) {
            let value = (high & 0xffff) << 16 | low_halves.remove(fields[0]).unwrap_or(0);
            require(!FLASH_MAPPING.contains(&value), || {
                ParkError::FlashAddress { location, value }
            })?;
        } else if !fields[0].is_empty() {
            // Any other write to the register discards a pending low half.
            low_halves.remove(fields[0]);
        }

        require(fields[0] != "pc", || ParkError::PcDestination { location })?;
        let returns = base == "pop" || (base.starts_with("ldm") && code.starts_with("sp!,"));
        let literal_load = base.starts_with("ldr") && code.contains("[pc,");
        require(!PC.is_match(code) || returns || literal_load, || {
            ParkError::PcWrite { location }
        })?;
        if base == "mrs" {
            let register = operands.rsplit(',').next().unwrap_or_default();
            require(register.trim().eq_ignore_ascii_case("primask"), || {
                ParkError::SystemRegister { location }
            })?;
        }

        if branch {
            let target = TARGET
                .captures(operands)
                .map(|t| parse_hex(&t[1]))
                .transpose()?;
            require(target.is_some_and(inside), || ParkError::Branch {
                location,
            })?;
        }
        if operands.contains("[pc,") {
            let literal = LITERAL
                .captures(operands)
                .map(|l| parse_hex(&l[1]))
                .transpose()?;
            require(literal.is_some_and(inside), || ParkError::Literal {
                location,
            })?;
        }

        count += 1;
        barriers += u32::from(mnemonic == "dsb");
    }

    require(count > 0 && barriers >= minimum_barriers, || {
        ParkError::Barriers {
            instructions: count,
            barriers,
        }
    })?;
    Ok(Leaf {
        instructions: count,
        barriers,
        calls: 0,
        foreign_branches: 0,
        xip_literals: 0,
        flash_address_constants: 0,
    })
}

/// `objects` maps KM0/KM4 to the linked Rust archive containing this code.
pub(crate) fn audit(sdk: &Path, binutils: &Binutils, objects: &Archives) -> Result<ParkAudit> {
    let image = fs::read(ota_image(sdk))?;
    let payload = container(&image)?;
    let regions = images(payload)?;
    let objdump = binutils.objdump();

    let mut images = BTreeMap::new();
    for core in Core::BOTH {
        let elf = elf(sdk, core);
        let paired = packaged::verify(&elf, &objdump, payload, &regions, core)?;

        let mut expected: Vec<&'static str> = SYMBOLS.to_vec();
        if core == Core::Hp {
            expected.extend(KM4_SYMBOLS);
        }
        let table = tool(&objdump, &["-t"], &elf)?;
        let mut symbols = BTreeMap::new();
        for line in table.lines() {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() == 6
                && let Some(&name) = expected.iter().find(|n| **n == fields[5])
            {
                require(fields[2] == "F" && fields[3] == ".ram_image2.text", || {
                    ParkError::Placement { name }
                })?;
                symbols.insert(name, (parse_hex(fields[0])?, parse_hex(fields[4])?));
            }
        }
        require(symbols.len() == expected.len(), || ParkError::Missing {
            core,
        })?;

        let mut functions = BTreeMap::new();
        for (name, (address, size)) in symbols {
            let listing = tool(
                &objdump,
                &[
                    "-d",
                    &format!("--start-address={}", hex(address)),
                    &format!("--stop-address={}", hex(address + size)),
                ],
                &elf,
            )?;
            let minimum = if name.contains("_sem_") || KM4_SYMBOLS.contains(&name) {
                1
            } else {
                3
            };
            let leaf = check_leaf(&listing, address, size, minimum)?;
            functions.insert(
                name.to_string(),
                ParkFunction {
                    address: hex(address),
                    bytes: size,
                    leaf,
                },
            );
        }

        let archive = archive_record(core, &elf, objects, sdk)?;
        images.insert(
            core.name().to_string(),
            ParkImage {
                archive,
                packaged_text_verified: paired,
                functions,
            },
        );
    }

    Ok(ParkAudit {
        images,
        activated: false,
        global_exclusion_proven: false,
        scope: "Retained leaf code, local branches/literals and packaged RAM placement; \
                runtime pointers, MMIO ownership, vectors, reset and SDK mediation require further evidence"
            .into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;

    const ADDRESS: u64 = 0x1000_5000;

    /// A synthetic listing in the pinned objdump format; no SDK build required.
    fn check(body: &[&str]) -> Result<Leaf> {
        let lines: Vec<String> = body
            .iter()
            .chain(&[
                "f3bf 8f4f dsb sy",
                "f3bf 8f4f dsb sy",
                "f3bf 8f4f dsb sy",
                "4770 bx lr",
            ])
            .enumerate()
            .map(|(i, line)| format!("{:x}:\t{line}\n", ADDRESS + 4 * i as u64))
            .collect();
        check_leaf(&lines.concat(), ADDRESS, 4 * (body.len() as u64 + 4), 3)
    }

    /// The park refusal of a listing that must be refused.
    fn rejection(body: &[&str]) -> ParkError {
        match check(body) {
            Err(Error::Park(error)) => error,
            other => panic!("{body:?}: {other:?}"),
        }
    }

    #[test]
    fn mmio_and_ram_constants_are_accepted() {
        let result = check(&[
            "f242 0014 movw r0, #8212 ; 0x2014",
            "f6c4 0000 movt r0, #18432 ; 0x4800",
            "f241 0100 movw r1, #4096 ; 0x1000",
            "f2c1 0100 movt r1, #4096 ; 0x1000",
            "f04f 6300 mov.w r3, #134217728 ; 0x8000000",
        ])
        .unwrap();
        assert_eq!(result.flash_address_constants, 0);
        assert_eq!(result.barriers, 3);
    }

    #[test]
    fn flash_literal_words_are_rejected_across_the_whole_mapping() {
        for value in [
            0x0800_0000u64,
            0x0818_6000,
            0x0BFF_FFFF,
            0x0C00_0020,
            0x0E00_0020,
            0x0FFF_FFFF,
        ] {
            let error = rejection(&[&format!("{value:08x} .word 0x{value:08x}")]);
            assert!(
                matches!(error, ParkError::FlashLiteral { value: found, .. } if found == value),
                "{error}"
            );
        }
        for value in [0x07FF_FFFFu64, 0x1000_0000, 0x4800_2014] {
            check(&[&format!("{value:08x} .word 0x{value:08x}")]).unwrap();
        }
    }

    #[test]
    fn movw_movt_flash_addresses_are_rejected() {
        let cases: [&[&str]; 4] = [
            &[
                "f246 0000 movw r0, #24576 ; 0x6000",
                "f6c0 0000 movt r0, #2048 ; 0x800",
            ],
            &["f6c0 6000 movt r0, #3584 ; 0xe00"],
            &[
                "f240 0320 movw r3, #32 ; 0x20",
                "2200 movs r2, #0",
                "f6c0 4300 movt r3, #3072 ; 0xc00",
            ],
            &["f6c0 0300 movteq r3, #2048 ; 0x800"],
        ];
        for body in cases {
            let error = rejection(body);
            assert!(
                matches!(error, ParkError::FlashAddress { .. }),
                "{body:?}: {error}"
            );
        }
    }

    #[test]
    fn unrelated_register_write_discards_low_half() {
        // r0's low half is overwritten before movt; the high half alone is RAM.
        check(&[
            "f246 0000 movw r0, #24576 ; 0x6000",
            "2000 movs r0, #0",
            "f2c1 0000 movt r0, #4096 ; 0x1000",
        ])
        .unwrap();
    }

    #[test]
    fn calls_foreign_branches_and_unreviewed_instructions_are_rejected() {
        for instruction in [
            "f000 f800 bl 1010b478 <ROM_call>".to_string(),
            "e000 b.n 0e000020 <XIP_target>".into(),
            "4700 bx r0".into(),
            "e8b0 8000 ldmia.w r0!, {pc}".into(),
            "4687 mov pc, r0".into(),
            format!("f8df f000 ldr.w pc, [pc, #0] ; ({ADDRESS:x} <bad_jump>)"),
            "4800 ldr r0, [pc, #0] ; (0e000020 <bad_literal>)".into(),
            "ed90 0a00 vldr s0, [r0]".into(),
            "f000 ????".into(),
        ] {
            rejection(&[&instruction]);
        }
    }
}
