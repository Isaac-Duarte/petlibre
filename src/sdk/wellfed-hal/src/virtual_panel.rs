//! The virtual panel's UART side (`wellfed_core::virtual_panel`): the
//! button task drains the log UART's receive FIFO each poll and holds the
//! buttons the lines ask for.
//!
//! Nothing else in the image reads the log UART: the build links no vendor
//! shell (the write audit refuses its entries, which carry flash and eFuse
//! commands), KM0 disables the log UART's interrupt, and KM4 holds its PMU
//! wakelock, so KM0's tickless drain of the FIFO never runs. The bench
//! paces its bytes, but the button task can be held off longer than the
//! 16-byte FIFO lasts (a flash erase stall), so an overrun refuses the line
//! it hit rather than read the bytes left as another press.

use wellfed_core::buttons::Buttons;
use wellfed_core::virtual_panel::{Held, Line, Reader};

use crate::sdk::{self, LogUartReceive};

/// Bytes taken in one poll at most: the FIFO's depth, so one poll never
/// spins on a line held high.
const DRAIN: usize = 16;

/// The task's reader and the held buttons.
#[derive(Debug, Default)]
pub(crate) struct Panel {
    reader: Reader,
    held: Held,
}

impl Panel {
    pub(crate) const fn new() -> Self {
        Self {
            reader: Reader::new(),
            held: Held::new(),
        }
    }

    /// Reads what the UART has, applies a finished line, and returns the
    /// buttons held at `now_ms` with the line, if one finished.
    pub(crate) fn poll(&mut self, now_ms: u32) -> (Buttons, Option<Line>) {
        let mut finished = None;
        for _ in 0..DRAIN {
            let LogUartReceive::Byte { byte, overrun } = sdk::log_uart_receive() else {
                break;
            };
            if overrun {
                self.reader.lost();
            }
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
