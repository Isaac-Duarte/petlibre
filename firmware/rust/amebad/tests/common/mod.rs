//! Synthetic platform handshake for host fixtures only. No real IPC or RAM
//! execution is modeled. Owner fixtures assume a ready, inactive platform
//! boot; board fixtures also verify fake GPIO after open.
#![allow(dead_code)]

use petlibro_amebad::board::{Board, MOTOR_MASK};
use petlibro_amebad::sdk::{self, fake};
use petlibro_core::flash_coord::{FlashCoord, FlashLimits, FlashReply};

pub fn flash_ready() -> Box<FlashCoord> {
    let limits = FlashLimits {
        quiesce_ms: 100,
        park_ms: 100,
        active_ms: 100,
        release_ms: 100,
        total_ms: 300,
    };
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

pub fn optics_off() -> bool {
    !sdk::gpio_read_bit(15) && !sdk::gpio_read_bit(35)
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
