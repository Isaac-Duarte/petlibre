//! Synthetic platform handshake for host fixtures only. No real IPC or RAM
//! execution is modeled. Owner fixtures assume a ready, inactive platform
//! boot; board fixtures also verify fake GPIO after open.
#![allow(dead_code)]

use petlibre_amebad::board::{Board, MOTOR_MASK};
use petlibre_amebad::sdk::{self, fake};
use petlibre_core::flash_coord::{FlashCoord, FlashLimits, FlashReply};

/// A lease budget the owner fixture's watchdog supervises: 40 ms plus two
/// 1 ms sample periods and two 2 ms timer periods stays below its 50 ms gap.
pub const FLASH_LIMITS: FlashLimits = FlashLimits {
    quiesce_ms: 10,
    park_ms: 10,
    active_ms: 20,
    release_ms: 10,
    total_ms: 40,
};

pub fn flash_ready() -> Box<FlashCoord> {
    let limits = FLASH_LIMITS;
    let mut c = Box::new(FlashCoord::new(&limits, 17, 0).unwrap());
    assert_eq!(c.ready(17, 29, true, 0), FlashReply::ACCEPTED);
    c
}

pub fn flash_attach(b: &mut Board) -> Box<FlashCoord> {
    assert_eq!(sdk::gpio_port_read(0, MOTOR_MASK), MOTOR_MASK);
    assert!(!sdk::gpio_read_bit(15) && !sdk::gpio_read_bit(35));
    let mut c = flash_ready();
    assert!(unsafe { b.attach_flash(&mut *c) });
    c
}

pub fn motor_pins() -> u32 {
    sdk::gpio_port_read(0, MOTOR_MASK)
}

/// Both emitters OFF: LOW pad input AND LOW output latch.
pub fn optics_off() -> bool {
    !sdk::gpio_read_bit(15)
        && !sdk::gpio_read_bit(35)
        && sdk::gpio_output_latch(0) & (1 << 15) == 0
        && sdk::gpio_output_latch(1) & (1 << 3) == 0
}

pub fn hw<R>(f: impl FnOnce(&mut fake::Hardware) -> R) -> R {
    fake::with(f)
}

/// A fresh fake and an opened board (deadtime 3, lease 10, 8 ADC polls).
pub fn open_board() -> Box<Board> {
    fake::reset_hardware();
    let mut b = Box::<Board>::default();
    assert!(b.open(3, 10, 8, 0));
    b
}
