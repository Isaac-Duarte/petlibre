//! The SDK surface used by the PLAF103 AmebaD ports.
//!
//! Target builds call the pinned AmebaD SDK through bindgen-generated
//! declarations; host builds use [`fake`], a deterministic model of the same
//! calls for tests and the virtual feeder. Both expose identical functions.
#![cfg_attr(target_os = "none", no_std)]
#![allow(clippy::missing_safety_doc)]

use core::ffi::c_void;

/// Interrupt service routine registered with the SDK vector table.
pub type Isr = unsafe extern "C" fn(context: *mut c_void) -> u32;

/// One task iteration: returns the delay in milliseconds before the next
/// iteration, or zero to delete the task.
pub type TaskStep = unsafe extern "C" fn(context: *mut c_void) -> u32;

/// Pinned GPIO enable bits, checked against SDK constants in target builds.
pub const GPIO_ENABLE_BIT: u32 = 1 << 20;
/// Pinned LP ADC function and clock enable bits, in the same two registers.
pub const ADC_ENABLE_BIT: u32 = 1 << 8;
/// `REG_SWD_PMUX_EN` bit 0 (reset 1): the SWD function holds PA27 and PB3
/// (SWD_CLK). While set, GPIO writes to PB3 do not reach the pad.
pub const SWD_PMUX_BIT: u32 = 1;

/// One LP SYSCTRL function/clock enable pair (`REG_LP_FUNC_EN1`,
/// `REG_LP_CLK_CTRL1`) around an enable of `bit`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LpClock {
    pub bit: u32,
    pub function_before: u32,
    pub clock_before: u32,
    pub function_after: u32,
    pub clock_after: u32,
}

impl LpClock {
    /// `bit` is set in both registers and no other bit changed.
    pub fn verified(self) -> bool {
        self.function_after & self.bit != 0
            && self.clock_after & self.bit != 0
            && (self.function_after & !self.bit) == (self.function_before & !self.bit)
            && (self.clock_after & !self.bit) == (self.clock_before & !self.bit)
    }
}

/// Read-only shared-pad enables and GPIO data-source selectors.
/// Values are observations; no mapping from group bits to physical pins is assumed.
#[derive(Clone, Copy, Debug, Default)]
pub struct GpioRouting {
    pub groups: u32,
    pub aon: u32,
    pub control_a: u32,
    pub control_b: u32,
    /// KM4's view of the KM0-owned shared-pad supply control.
    pub share_pad: u32,
    /// Raw Port A input word, including non-indicator pads.
    pub ext_a: u32,
}

/// One flash MMU entry's control, start and offset registers.
#[derive(Clone, Copy, Debug, Default)]
pub struct FlashMmuEntry {
    pub control: u32,
    pub start: u32,
    pub offset: u32,
}

impl FlashMmuEntry {
    /// Zero-based research-board bank this KM0 mapping runs from, or `None`
    /// for an unrecognized mapping.
    pub fn bank(self) -> Option<u8> {
        if self.control & 1 == 0 || self.start != 0x0c00_0000 {
            return None;
        }

        let physical = if self.control & 2 != 0 {
            self.start.checked_sub(self.offset)?
        } else {
            self.start.checked_add(self.offset)?
        };

        match physical {
            0x0800_6000 => Some(0),
            0x0818_6000 => Some(1),
            _ => None,
        }
    }
}

/// Bank 1 as the updater may write it: the reviewed 0x150000-byte footprint
/// from 0x186000, in 4 KiB sectors. Only while running from bank 0.
pub const BANK1_OFFSET: u32 = 0x18_6000;
pub const BANK1_SECTORS: u32 = 0x15_0000 / 0x1000;

/// Bank 0, the factory loader's fallback (read only, for the give-way guard).
pub const BANK0_OFFSET: u32 = 0x6000;
/// The factory loader's valid-image marker at the start of each bank.
pub const MARKER: [u8; 8] = *b"81958711";

/// FW24: samples in one codec DMA page (16 ms at 16 kHz, 16-bit mono),
/// `WELLFED_AUDIO_PAGE` in `example_entry.c`.
pub const AUDIO_PAGE_SAMPLES: usize = 256;
/// Pages in the ring.
pub const AUDIO_PAGES: usize = 8;

/// One event from the KM4 Wi-Fi shim's ring (`example_entry.c`), numbered
/// from 1. `kind`: 1 on, 2 scan, 3 connect, 4 DHCP, 5 lost, 6 backoff.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WifiEvent {
    pub seq: u32,
    pub kind: u32,
    pub attempt: u32,
    /// SDK result: `RTW_SUCCESS` (0) or an error; for DHCP the `LwIP_DHCP` state.
    pub result: i32,
    /// Scan: network found; DHCP: assigned IPv4; lost: ms connected; backoff: ms.
    pub value: u32,
    /// Scan only: the network's RSSI (dBm) and `rtw_security_t`.
    pub rssi: i32,
    pub security: u32,
    pub ms: u32,
}

/// `wifi_event(seq)` outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WifiPoll {
    Pending,
    Event(WifiEvent),
    /// The ring overwrote it before it was read.
    Lost,
}

/// FW08 broker settings, from the generated credentials header the build
/// writes into the SDK worktree (`example_entry.c`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransportConfig {
    /// IPv4, host order.
    pub broker: u32,
    pub port: u16,
    pub client_id: &'static str,
    pub username: &'static str,
    pub password: &'static [u8],
    /// Topic prefix, ending in `/`.
    pub namespace: &'static str,
    /// The key that signs server requests, when the image has one.
    pub request_key: Option<[u8; 32]>,
}

/// `tcp_recv` outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TcpRecv {
    Data(usize),
    Timeout,
    /// The peer closed the connection, or the socket failed.
    Closed,
}

/// Interrupt lines claimed by the ports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Irq {
    Timer2,
    GpioA,
}

#[cfg(all(target_os = "none", feature = "km4"))]
mod target;
#[cfg(all(target_os = "none", feature = "km4"))]
pub use target::*;

#[cfg(not(target_os = "none"))]
pub mod fake;
#[cfg(not(target_os = "none"))]
pub use fake::api::*;

#[cfg(any(not(target_os = "none"), feature = "km4"))]
mod guard {
    /// Interrupt mask held for the guard's lifetime, restoring the entry mask.
    pub struct IrqGuard(u32);

    impl IrqGuard {
        #[must_use]
        pub fn lock() -> Self {
            let saved = super::primask();
            super::disable_irq();
            Self(saved)
        }
    }

    impl Drop for IrqGuard {
        fn drop(&mut self) {
            super::set_primask(self.0);
        }
    }
}

#[cfg(any(not(target_os = "none"), feature = "km4"))]
pub use guard::IrqGuard;
