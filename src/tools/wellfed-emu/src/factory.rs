//! The factory loader's own flash mapping and bank selection, executed.
//!
//! The loader bytes come from the supplied flash image (bank check at
//! `0x08000154`, ordinary selection at `0x080003F4`) and call the candidate
//! ROM's RSIP mapping routine at `0x3624`. MMIO is ordinary RAM. Each time a
//! mapping is configured, the requested physical flash is copied into its
//! virtual window, so the loader reads the headers it would see through the
//! mapping. These are supplied-state slices, not a full reset: earlier
//! MMIO/cache setup is modeled and MPU, TrustZone and caches are not.
//!
//! Every instruction outside the reviewed loader and ROM ranges, and every
//! mapping that disagrees with the registers it set, stops the run.

use std::fs;
use std::path::Path;

use wellfed_formats::canonical::sha256;
use wellfed_formats::require;
use wellfed_formats::review::{FactoryRun, MappingConfig, Stop};
use unicorn_engine::{Arch, ArmCpuModel, Mode, Prot, RegisterARM, Unicorn};

use crate::{BASELINE, BASELINE_SHA256, FLASH_BYTES, ROM, ROM_SHA256, Result, word};

/// A factory-loader run refused: a pinned input, a reviewed boundary or a
/// mapping that disagrees with what the loader asked for.
#[derive(Debug, thiserror::Error)]
pub enum FactoryError {
    #[error("Not the verified research-unit full backup")]
    UnverifiedBaseline,
    #[error("Unreviewed factory ROM input")]
    UnreviewedRom,
    /// The pinned input is read before any hash check; the I/O error is kept
    /// in the message so the operator sees which file and why in one line.
    #[error("Cannot read {name}: {error}")]
    Read {
        name: &'static str,
        error: std::io::Error,
    },
    #[error("Unexpected selection outcome {stop:?} {result}")]
    UnexpectedSelection { stop: Stop, result: u32 },
    #[error("Expected exactly 8 MiB of flash")]
    FlashSize { bytes: usize },
    #[error("Loader did not reach a reviewed boundary")]
    NoBoundary,
    #[error("Candidate ROM mapping registers disagree with request")]
    MappingRegisters,
    #[error("Wrong mapping direction")]
    MappingDirection,
    #[error("Mapping escapes external flash")]
    MappingEscapes,
    #[error("Unexpected mapping index or virtual start")]
    MappingRequest,
    #[error("Unexpected factory-mapping instruction: {pc:#x}")]
    UnreviewedInstruction { pc: u64 },
    #[error("Out-of-range {quantity} in factory emulation")]
    OutOfRange { quantity: Quantity },
}

/// The value a factory run could not represent in the width it needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quantity {
    Index,
    Size,
    Offset,
    Length,
    Word,
    Register,
}

impl std::fmt::Display for Quantity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Index => "index",
            Self::Size => "size",
            Self::Offset => "offset",
            Self::Length => "length",
            Self::Word => "word",
            Self::Register => "register",
        })
    }
}

/// The modeled caller the bank check returns to.
pub const EXIT: u32 = 0x8F000;
const FLASH_BASE: u32 = 0x0800_0000;
const RSIP: u32 = 0x4800_0630;
/// The ROM's RSIP mapping routine and its enable call.
const MAP: u64 = 0x3624;
const ENABLE: u64 = 0x3648;
/// The instructions a loader entry may run before the run is refused. Each
/// entry takes a few hundred; a runaway stops here. The budget is the only
/// bound: a wall-clock timeout fired under a loaded host and failed a correct
/// run, and the count is deterministic.
const BUDGET: usize = 4000;

/// Which loader entry to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    /// The bank check for one bank (0 or 1); result 1 when it is valid.
    Check(u8),
    /// The ordinary selection; result is the chosen bank plus one.
    Select,
}

/// The pinned baseline and candidate ROM, each refused unless its hash matches.
pub fn inputs(root: &Path) -> Result<(Vec<u8>, Vec<u8>)> {
    let baseline = read(root, BASELINE)?;
    require(
        baseline.len() == FLASH_BYTES && sha256(&baseline) == BASELINE_SHA256,
        || FactoryError::UnverifiedBaseline,
    )?;
    let rom = read(root, ROM)?;
    require(sha256(&rom) == ROM_SHA256, || FactoryError::UnreviewedRom)?;
    Ok((baseline, rom))
}

fn read(root: &Path, name: &'static str) -> Result<Vec<u8>> {
    Ok(fs::read(root.join(name)).map_err(|error| FactoryError::Read { name, error })?)
}

/// The selection's outcome by bank: `Some(0)`/`Some(1)`, or `None` when the
/// loader reaches its no-image wait loop.
pub fn selected(flash: &[u8], rom: &[u8]) -> Result<Option<u8>> {
    let run = execute(flash, rom, Entry::Select)?;
    match (run.stop, run.result) {
        (Stop::NoImage, _) => Ok(None),
        (Stop::Selected, 1) => Ok(Some(0)),
        (Stop::Selected, 2) => Ok(Some(1)),
        (stop, result) => Err(FactoryError::UnexpectedSelection { stop, result }.into()),
    }
}

/// A mapping request seen at the ROM routine, awaiting its return.
#[derive(Debug, Clone, Copy)]
struct Request {
    index: u32,
    start: u32,
    end: u32,
    minus: u32,
    offset: u32,
}

#[derive(Default)]
struct State {
    configs: Vec<MappingConfig>,
    commands: Vec<[u32; 2]>,
    /// The return address of the mapping call in progress, and its request.
    pending: Option<(u64, Request)>,
    outcome: Option<(Stop, u32)>,
    error: Option<crate::Error>,
}

/// Run one loader entry on `flash` (the full 8 MiB image) with `rom`.
pub fn execute(flash: &[u8], rom: &[u8], entry: Entry) -> Result<FactoryRun> {
    require(flash.len() == FLASH_BYTES, || FactoryError::FlashSize {
        bytes: flash.len(),
    })?;
    let mut uc = Unicorn::new_with_data(Arch::ARM, Mode::THUMB | Mode::MCLASS, State::default())?;
    uc.ctl_set_cpu_model(ArmCpuModel::CORTEX_M4 as i32)?;

    uc.mem_map(0, 0x10000, Prot::READ | Prot::EXEC)?;
    uc.mem_write(0, rom)?;
    uc.mem_map(u64::from(FLASH_BASE), 0x80_0000, Prot::READ | Prot::EXEC)?;
    uc.mem_write(u64::from(FLASH_BASE), flash)?;
    // The factory RAM image, as the loader's own setup leaves it.
    uc.mem_map(0x8_0000, 0x10000, Prot::ALL)?;
    uc.mem_write(0x8_2000, &flash[0x720..0x1030])?;
    for window in [0x0C00_0000, 0x0E00_0000] {
        uc.mem_map(window, 0x20_0000, Prot::READ | Prot::EXEC)?;
    }
    uc.mem_map(0x4800_0000, 0x1000, Prot::ALL)?;
    uc.mem_map(0xE000_E000, 0x2000, Prot::ALL)?;

    let (begin, bank) = match entry {
        Entry::Check(bank) => (0x0800_0155, u64::from(bank)),
        Entry::Select => (0x0800_03F5, 0),
    };
    uc.reg_write(RegisterARM::SP, 0x8_E000)?;
    uc.reg_write(RegisterARM::LR, u64::from(EXIT | 1))?;
    uc.reg_write(RegisterARM::R0, bank)?;
    uc.reg_write(RegisterARM::R4, 0x8_01CC)?;

    uc.add_code_hook(1, 0, move |uc, pc, _| {
        if uc.get_data().error.is_some() {
            return;
        }
        if let Err(error) = step(uc, flash, pc) {
            uc.get_data_mut().error = Some(error);
            let _ = uc.emu_stop();
        }
    })?;
    let started = uc.emu_start(begin, u64::from(EXIT) + 2, 0, BUDGET);

    if let Some(error) = uc.get_data_mut().error.take() {
        return Err(error);
    }
    started?;
    let state = uc.get_data();
    let (Some((stop, result)), None) = (state.outcome, state.pending) else {
        return Err(FactoryError::NoBoundary.into());
    };
    let mut final_registers = [[0; 4]; 2];
    for (index, group) in final_registers.iter_mut().enumerate() {
        *group = registers(&uc, index)?;
    }
    Ok(FactoryRun {
        stop,
        result,
        configs: state.configs.clone(),
        commands: state.commands.clone(),
        final_registers,
    })
}

fn step(uc: &mut Unicorn<State>, flash: &[u8], pc: u64) -> Result<()> {
    // A mapping call returned: its registers must say what was asked, and
    // the window then shows the requested physical flash.
    if let Some((back, request)) = uc.get_data().pending
        && pc == back
    {
        let index = usize::try_from(request.index).map_err(|_| out_of_range(Quantity::Index))?;
        let set = registers(uc, index)?;
        require(
            set[1..] == [request.start, request.end, request.offset],
            || FactoryError::MappingRegisters,
        )?;
        require(
            u64::from(set[0] & 2) == u64::from(request.minus) * 2,
            || FactoryError::MappingDirection,
        )?;

        let offset = i64::from(request.offset);
        let physical = i64::from(request.start) + if request.minus != 0 { -offset } else { offset };
        let end = i64::from(FLASH_BASE)
            + i64::try_from(FLASH_BYTES).map_err(|_| out_of_range(Quantity::Size))?;
        require((i64::from(FLASH_BASE)..end).contains(&physical), || {
            FactoryError::MappingEscapes
        })?;
        let length = (i64::from(request.end) - i64::from(request.start) + 1)
            .min(0x20_0000)
            .min(end - physical)
            .max(0);
        let from = usize::try_from(physical - i64::from(FLASH_BASE))
            .map_err(|_| out_of_range(Quantity::Offset))?;
        let length = usize::try_from(length).map_err(|_| out_of_range(Quantity::Length))?;
        uc.mem_write(u64::from(request.start), &flash[from..from + length])?;

        let state = uc.get_data_mut();
        state.configs.push(MappingConfig {
            index: request.index,
            start: request.start,
            end: request.end,
            minus: request.minus,
            offset: request.offset,
            registers: set,
        });
        state.pending = None;
    }

    match pc {
        MAP => {
            let sp = reg(uc, RegisterARM::SP)?;
            let request = Request {
                index: reg(uc, RegisterARM::R0)?,
                start: reg(uc, RegisterARM::R1)?,
                end: reg(uc, RegisterARM::R2)?,
                minus: reg(uc, RegisterARM::R3)?,
                offset: read32(uc, u64::from(sp))?,
            };
            require(
                request.index < 2 && [0x0C00_0000, 0x0E00_0000].contains(&request.start),
                || FactoryError::MappingRequest,
            )?;
            let back = u64::from(reg(uc, RegisterARM::LR)? & !1);
            uc.get_data_mut().pending = Some((back, request));
        }
        ENABLE => {
            let command = [reg(uc, RegisterARM::R0)?, reg(uc, RegisterARM::R1)?];
            uc.get_data_mut().commands.push(command);
        }
        _ => {
            if let Some(stop) = stop_at(pc) {
                let result = reg(uc, RegisterARM::R0)?;
                uc.get_data_mut().outcome = Some((stop, result));
                uc.emu_stop()?;
            } else if !reviewed(pc) {
                return Err(FactoryError::UnreviewedInstruction { pc }.into());
            }
        }
    }
    Ok(())
}

fn stop_at(pc: u64) -> Option<Stop> {
    match pc {
        0x8F000 => Some(Stop::Returned),
        0x0800_04A0 => Some(Stop::Selected),
        0x0800_0482 => Some(Stop::NoImage),
        0x0800_0182 => Some(Stop::Warning),
        _ => None,
    }
}

/// The loader's bank check and selector, and the ROM mapping routines.
fn reviewed(pc: u64) -> bool {
    (0x0800_0020..0x0800_01AE).contains(&pc)
        || (0x0800_03F4..0x0800_04A0).contains(&pc)
        || (MAP..0x3642).contains(&pc)
        || (ENABLE..0x365E).contains(&pc)
}

fn registers<D>(uc: &Unicorn<D>, index: usize) -> Result<[u32; 4]> {
    let base =
        u64::from(RSIP) + 16 * u64::try_from(index).map_err(|_| out_of_range(Quantity::Index))?;
    let mut group = [0; 4];
    for (offset, value) in (0..).step_by(4).zip(group.iter_mut()) {
        *value = read32(uc, base + offset)?;
    }
    Ok(group)
}

fn read32<D>(uc: &Unicorn<D>, address: u64) -> Result<u32> {
    let bytes = uc.mem_read_as_vec(address, 4)?;
    word(&bytes, 0).ok_or_else(|| out_of_range(Quantity::Word))
}

fn reg<D>(uc: &Unicorn<D>, register: RegisterARM) -> Result<u32> {
    u32::try_from(uc.reg_read(register)?).map_err(|_| out_of_range(Quantity::Register))
}

fn out_of_range(quantity: Quantity) -> crate::Error {
    FactoryError::OutOfRange { quantity }.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A loader that spins inside the reviewed range never reaches a boundary:
    /// the instruction budget ends the run and it is refused.
    #[test]
    fn a_runaway_loader_is_bounded_by_the_budget() {
        let mut flash = vec![0; FLASH_BYTES];
        // `b .` at the bank check's entry.
        flash[0x154..0x156].copy_from_slice(&0xE7FE_u16.to_le_bytes());
        let rom = vec![0; 0x10000];

        assert!(matches!(
            execute(&flash, &rom, Entry::Check(0)),
            Err(crate::Error::Factory(FactoryError::NoBoundary))
        ));
    }
}
