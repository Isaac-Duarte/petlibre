//! The virtual panel's UART side (`wellfed_core::virtual_panel`): the
//! button task drains the log UART's receive FIFO each poll and holds the
//! buttons the lines ask for.
//!
//! Nothing else in the image reads the log UART: the build links no vendor
//! shell (the write audit refuses its entries, which carry flash and eFuse
//! commands), and the KM0 main disables the log UART's interrupt. So this
//! poll is the only reader, and it reads the FIFO's registers directly
//! rather than through a ROM routine that would need new bindings. The
//! bench paces its bytes so a 5 ms poll never lets the FIFO fill.

use wellfed_core::buttons::Buttons;
use wellfed_core::virtual_panel::{Held, Line, Reader};

/// The log UART (UART2, `UARTLOG_REG_BASE`) and its registers, from the
/// pinned SDK's `hal_platform.h` (static analysis).
#[cfg(target_os = "none")]
mod registers {
    const BASE: usize = 0x4801_2000;
    /// Line status; bit 0 is data ready.
    pub const LSR: usize = BASE + 0x14;
    /// Receive buffer (read) and transmit holding (write).
    pub const RBR: usize = BASE + 0x24;
    pub const DATA_READY: u32 = 1 << 0;
}

/// Bytes taken in one poll at most: the FIFO's depth, so one poll never
/// spins on a line held high.
const DRAIN: usize = 16;

/// The task's reader and the held buttons.
#[derive(Debug, Default)]
pub struct Panel {
    reader: Reader,
    held: Held,
}

impl Panel {
    pub const fn new() -> Self {
        Self {
            reader: Reader::new(),
            held: Held::new(),
        }
    }

    /// Reads what the UART has, applies a finished line, and returns the
    /// buttons held at `now_ms` with the line, if one finished.
    pub fn poll(&mut self, now_ms: u32) -> (Buttons, Option<Line>) {
        let mut finished = None;
        for _ in 0..DRAIN {
            let Some(byte) = receive() else { break };
            if let Some(line) = self.reader.byte(byte) {
                if let Line::Press(press) = line {
                    self.held.press(now_ms, press);
                }
                finished = Some(line);
                // One line a poll, so each is logged.
                break;
            }
        }
        (self.held.levels(now_ms), finished)
    }
}

#[cfg(target_os = "none")]
fn receive() -> Option<u8> {
    use core::ptr;

    // SAFETY: the log UART's status and receive registers are always
    // mapped on KM4 (it prints through the same block), and nothing else in
    // the image reads them (see the module docs). A read of the receive
    // register pops one byte, only after the status says one is there.
    unsafe {
        let status = ptr::read_volatile(registers::LSR as *const u32);
        if status & registers::DATA_READY == 0 {
            return None;
        }
        let word = ptr::read_volatile(registers::RBR as *const u32);
        Some(word.to_le_bytes()[0])
    }
}

/// Host builds have no UART: nothing arrives.
#[cfg(not(target_os = "none"))]
fn receive() -> Option<u8> {
    None
}
