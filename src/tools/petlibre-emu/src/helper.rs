//! The vendor UART flash helper, executing on synthetic NOR.
//!
//! The pinned helper runs from modeled RAM at `0x82020` as it does after the
//! loader uploads it. Its command parser, packet checks and program wrapper
//! execute as ARM; the ROM UART and flash calls it makes are modeled here.
//! Each modeled erase and program is refused outside the sectors a test
//! allows, which stay inside one OTA bank and never touch the factory region.
//! An unreviewed ROM call stops the session, as a real fault would.
//!
//! The host side is serial-shaped: [`Helper::write`] supplies bytes the
//! helper will read, and [`Helper::read`] runs the helper until it has output.

use std::collections::VecDeque;

use petlibre_formats::canonical::sha256;
use unicorn_engine::{Arch, ArmCpuModel, Mode, Prot, RegisterARM, Unicorn};

use crate::{BANKS, Error, FLASH_BYTES, HELPER_SHA256, Result, SECTOR, require};

const FLASH_BASE: u64 = 0x0800_0000;
const CODE: u64 = 0x8_2020;
const CODE_END: u64 = 0x8_3030;
const RAM: std::ops::Range<u64> = 0x8_0000..0x9_0000;
const HELPER_BYTES: usize = 4688;
const BANK_BYTES: u32 = 0x15_0000;

/// One modeled flash call the helper made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Call {
    Erase { offset: u32 },
    Program { offset: u32, bytes: u32 },
    ReadStatus { opcode: u8 },
}

/// What a session may touch, beyond the snapshot it starts from.
#[derive(Debug, Clone)]
pub struct Scope {
    /// The bank the allowed sectors lie in: 0 or 1.
    pub bank: u8,
    /// Sector offsets the helper may erase and program.
    pub sectors: Vec<u32>,
    /// The 05/35/15 status bytes; `None` refuses status reads.
    pub status: Option<[u8; 3]>,
}

#[derive(Default)]
struct State {
    incoming: VecDeque<u8>,
    outgoing: Vec<u8>,
    sectors: Vec<u32>,
    status: Option<[u8; 3]>,
    calls: Vec<Call>,
    empty_polls: u32,
    error: Option<Error>,
}

pub struct Helper<'a> {
    uc: Unicorn<'a, State>,
    started: bool,
    host_tx: Vec<u8>,
}

impl Helper<'_> {
    /// The helper with `snapshot` (8 MiB) as flash; erases and programs are
    /// confined to `scope`.
    pub fn new(helper: &[u8], snapshot: &[u8], scope: Scope) -> Result<Self> {
        require(
            helper.len() == HELPER_BYTES && sha256(helper) == HELPER_SHA256,
            "Pinned helper required",
        )?;
        require(
            snapshot.len() == FLASH_BYTES,
            "Synthetic snapshot must be 8 MiB",
        )?;
        let start = *BANKS
            .get(usize::from(scope.bank))
            .ok_or_else(|| Error::Refused("Synthetic scope must be one bank".into()))?;
        require(
            !scope.sectors.is_empty()
                && scope
                    .sectors
                    .iter()
                    .all(|s| (start..start + BANK_BYTES).contains(s) && s % SECTOR == 0),
            "Synthetic erase/program scope must stay in its bank",
        )?;

        let state = State {
            sectors: scope.sectors,
            status: scope.status,
            ..State::default()
        };
        let mut uc = Unicorn::new_with_data(Arch::ARM, Mode::THUMB | Mode::MCLASS, state)?;
        uc.ctl_set_cpu_model(ArmCpuModel::CORTEX_M4 as i32)?;
        uc.mem_map(0, 0x10000, Prot::ALL)?;
        uc.mem_map(RAM.start, RAM.end - RAM.start, Prot::ALL)?;
        uc.mem_write(0x8_2000, helper)?;
        uc.mem_map(FLASH_BASE, 0x80_0000, Prot::READ | Prot::EXEC)?;
        uc.mem_write(FLASH_BASE, snapshot)?;
        uc.mem_map(0x4800_0000, 0x1000, Prot::ALL)?;
        uc.mem_map(0xE000_E000, 0x2000, Prot::ALL)?;
        uc.reg_write(RegisterARM::SP, 0x8_FF00)?;

        // The helper's own code runs unhooked; everything it calls is modeled.
        for (begin, end) in [(0, CODE - 1), (CODE_END, u64::from(u32::MAX))] {
            uc.add_code_hook(begin, end, |uc, pc, _| {
                if uc.get_data().error.is_some() {
                    return;
                }
                if let Err(error) = call(uc, pc) {
                    uc.get_data_mut().error = Some(error);
                    let _ = uc.emu_stop();
                }
            })?;
        }
        Ok(Self {
            uc,
            started: false,
            host_tx: Vec::new(),
        })
    }

    /// Supply host bytes for the helper to read.
    pub fn write(&mut self, data: &[u8]) -> usize {
        self.uc.get_data_mut().incoming.extend(data);
        self.host_tx.extend_from_slice(data);
        data.len()
    }

    /// Up to `count` output bytes, running the helper first if it has none.
    pub fn read(&mut self, count: usize) -> Result<Vec<u8>> {
        if self.uc.get_data().outgoing.is_empty() {
            self.drive()?;
        }
        let outgoing = &mut self.uc.get_data_mut().outgoing;
        let count = count.min(outgoing.len());
        Ok(outgoing.drain(..count).collect())
    }

    pub fn in_waiting(&self) -> usize {
        self.uc.get_data().outgoing.len()
    }

    /// Everything the host sent, in order.
    pub fn host_tx(&self) -> &[u8] {
        &self.host_tx
    }

    pub fn clear_host_tx(&mut self) {
        self.host_tx.clear();
    }

    pub fn calls(&self) -> &[Call] {
        &self.uc.get_data().calls
    }

    /// The whole synthetic flash as it is now.
    pub fn snapshot(&self) -> Result<Vec<u8>> {
        Ok(self.uc.mem_read_as_vec(FLASH_BASE, FLASH_BYTES)?)
    }

    /// Replace flash bytes directly, as a fixture arranging a starting state.
    pub fn seed(&mut self, offset: u32, data: &[u8]) -> Result<()> {
        Ok(self.uc.mem_write(FLASH_BASE + u64::from(offset), data)?)
    }

    /// Run the helper until it waits for input with output pending.
    fn drive(&mut self) -> Result<()> {
        let pc = if self.started {
            self.uc.reg_read(RegisterARM::PC)?
        } else {
            CODE
        };
        self.started = true;
        self.uc.get_data_mut().empty_polls = 0;

        let ran = self.uc.emu_start(pc | 1, CODE_END, 3_000_000, 500_000);
        if let Some(error) = self.uc.get_data_mut().error.take() {
            return Err(error);
        }
        ran?;
        require(
            !self.uc.get_data().outgoing.is_empty(),
            "Helper produced no bounded response",
        )
    }
}

/// Model one ROM call (or refuse it), then return to the helper.
fn call(uc: &mut Unicorn<State>, pc: u64) -> Result<()> {
    let args = [
        reg(uc, RegisterARM::R0)?,
        reg(uc, RegisterARM::R1)?,
        reg(uc, RegisterARM::R2)?,
    ];
    match pc {
        // UART readable poll: pause with output pending, so the host reads
        // before the helper waits on input it has not been sent yet.
        0x6220 => {
            let state = uc.get_data_mut();
            if state.incoming.is_empty() && !state.outgoing.is_empty() {
                uc.emu_stop()?;
                return Ok(());
            }
            state.empty_polls = if state.incoming.is_empty() {
                state.empty_polls + 1
            } else {
                0
            };
            require(
                state.empty_polls < 100,
                "Helper waiting without an expected response",
            )?;
            let ready = u32::from(!state.incoming.is_empty());
            ret(uc, ready)
        }
        0x6250 => {
            let byte = uc.get_data_mut().incoming.pop_front();
            let byte = byte.ok_or_else(|| refused("UART getc without supplied host input"))?;
            ret(uc, u32::from(byte))
        }
        0x6274 => {
            uc.get_data_mut().outgoing.push(args[0].to_le_bytes()[0]);
            ret(uc, 1)
        }
        0x628C => {
            let [pointer, length, _] = args;
            require(
                within(pointer, length) && length <= 1032,
                "UART transmit escaped helper RAM",
            )?;
            let data = uc.mem_read_as_vec(u64::from(pointer), length as usize)?;
            uc.get_data_mut().outgoing.extend(data);
            ret(uc, length)
        }
        0x1D1C => {
            let [opcode, length, pointer] = args;
            let index = [0x05, 0x35, 0x15].iter().position(|&o| o == opcode);
            let value = uc.get_data().status.zip(index).map(|(s, i)| s[i]);
            let Some(value) = value.filter(|_| length == 1 && within(pointer, 1)) else {
                return Err(refused("Unexpected status-read command or buffer"));
            };
            uc.mem_write(u64::from(pointer), &[value])?;
            let opcode = u8::try_from(opcode).map_err(|_| refused("opcode"))?;
            uc.get_data_mut().calls.push(Call::ReadStatus { opcode });
            ret(uc, 0)
        }
        0x6238 => {
            require(
                uc.get_data().status.is_some(),
                "Unexpected UART-writable polling",
            )?;
            ret(uc, 1)
        }
        0x2146 => {
            let [mode, offset, _] = args;
            require(
                mode == 2 && uc.get_data().sectors.contains(&offset),
                "Erase escapes allowed application sectors",
            )?;
            uc.mem_write(FLASH_BASE + u64::from(offset), &[0xFF; SECTOR as usize])?;
            uc.get_data_mut().calls.push(Call::Erase { offset });
            ret(uc, 0)
        }
        0x2048 => {
            let [offset, length, pointer] = args;
            require(
                length == 8
                    && uc.get_data().sectors.contains(&(offset / SECTOR * SECTOR))
                    && offset % SECTOR + length <= SECTOR,
                "Program escapes allowed application sectors",
            )?;
            require(within(pointer, length), "Program buffer outside helper RAM")?;
            // NOR programming only clears bits.
            let address = FLASH_BASE + u64::from(offset);
            let existing = uc.mem_read_as_vec(address, 8)?;
            let data = uc.mem_read_as_vec(u64::from(pointer), 8)?;
            let programmed: Vec<u8> = existing.iter().zip(&data).map(|(a, b)| a & b).collect();
            uc.mem_write(address, &programmed)?;
            uc.get_data_mut().calls.push(Call::Program {
                offset,
                bytes: length,
            });
            ret(uc, 0)
        }
        0x6590 => {
            let [destination, source, length] = args;
            require(
                length <= 4096 && within(destination, length) && within(source, length),
                "Memcpy escaped helper RAM",
            )?;
            let data = uc.mem_read_as_vec(u64::from(source), length as usize)?;
            uc.mem_write(u64::from(destination), &data)?;
            ret(uc, destination)
        }
        0x66AC => {
            let [destination, value, length] = args;
            require(
                length <= 4096 && within(destination, length),
                "Memset escaped helper RAM",
            )?;
            let fill = vec![value.to_le_bytes()[0]; length as usize];
            uc.mem_write(u64::from(destination), &fill)?;
            ret(uc, destination)
        }
        0x61A0 => {
            require(
                args == [2, 0, 115_200],
                "Unexpected helper UART initialization",
            )?;
            uc.mem_write(0x8_0660, &0x4000_8000u32.to_le_bytes())?;
            ret(uc, 0)
        }
        0x3594 => {
            require(args[0] == 0, "Unexpected OTF setup")?;
            uc.mem_write(0x4800_0600, &[0; 4])?;
            ret(uc, 0)
        }
        0x1434 => {
            require(args[0] == 0, "Unexpected backup register")?;
            ret(uc, 0)
        }
        // Diagnostic, delay, UART baud and busy models only.
        0x0474 | 0x14C2 | 0x1504 | 0x40B0 | 0x43D4 => ret(uc, 0),
        _ => Err(Error::Refused(format!(
            "Unexpected helper destination (unreviewed flash/register operation forbidden): {pc:#x}"
        ))),
    }
}

/// Return `value` from the modeled call to its caller.
fn ret(uc: &mut Unicorn<State>, value: u32) -> Result<()> {
    uc.reg_write(RegisterARM::R0, u64::from(value))?;
    let back = uc.reg_read(RegisterARM::LR)?;
    uc.reg_write(RegisterARM::PC, back)?;
    Ok(())
}

/// `[pointer, pointer + length)` lies in helper RAM.
fn within(pointer: u32, length: u32) -> bool {
    let (pointer, length) = (u64::from(pointer), u64::from(length));
    RAM.start <= pointer && pointer + length <= RAM.end
}

fn reg(uc: &Unicorn<State>, register: RegisterARM) -> Result<u32> {
    u32::try_from(uc.reg_read(register)?).map_err(|_| refused("Out-of-range register"))
}

fn refused(message: &str) -> Error {
    Error::Refused(message.into())
}
