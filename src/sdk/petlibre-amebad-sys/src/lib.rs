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

/// The PLAF103 board: every GPIO and ADC channel the ports use, in one place
/// (FW50). A GPIO is `port * 32 + bit` (PA13 is 13, PB3 is 35). Roles and
/// polarities are stock-derived and bench-checked where the cards say so;
/// [device layer](../../../../docs/firmware/device-layer.md) lists what else
/// is board-specific and where it lives.
pub mod board {
    /// Motor driver inputs; both HIGH requests STOP (brake).
    pub const MOTOR_A: u32 = 13;
    pub const MOTOR_B: u32 = 14;
    /// Infrared emitter latches, LOW for OFF. PB3 is also SWD_CLK.
    pub const IR_PA: u32 = 15;
    pub const IR_PB: u32 = 32 + 3;
    /// Sensor inputs: outlet beam, food beam, feedback switch, adapter.
    pub const OUTLET: u32 = 16;
    pub const FOOD: u32 = 17;
    pub const FEEDBACK: u32 = 18;
    pub const ADAPTER: u32 = 19;
    /// Panel buttons, active LOW: reset, mic, dispense, lock (PB4–PB7).
    pub const BUTTONS: [u32; 4] = [32 + 4, 32 + 5, 32 + 6, 32 + 7];
    /// Indicators: connection, food, lock (PA0, PA2, PA4) and fault (PB22).
    pub const LED_CONNECTION: u32 = 0;
    pub const LED_FOOD: u32 = 2;
    pub const LED_LOCK: u32 = 4;
    pub const LED_FAULT: u32 = 32 + 22;
    /// Speaker amplifier enable, HIGH while playing.
    pub const AMP: u32 = 32 + 23;
    /// ADC channels: motor current and the battery pack. Channel 10, the
    /// chip's own supply, is the AmebaD's, not the board's.
    pub const ADC_CURRENT: u32 = 4;
    pub const ADC_BATTERY: u32 = 5;
    pub const ADC_SUPPLY: u32 = 10;

    /// The pin's bit in its port register.
    pub const fn bit(pin: u32) -> u32 {
        1 << (pin & 31)
    }
}

/// Motor driver inputs PA13 and PA14 (port A); both HIGH requests STOP.
pub const MOTOR_A_BIT: u32 = board::bit(board::MOTOR_A);
pub const MOTOR_B_BIT: u32 = board::bit(board::MOTOR_B);
pub const MOTOR_MASK: u32 = MOTOR_A_BIT | MOTOR_B_BIT;
/// Infrared emitter latches, LOW for OFF: PA15 on port A and PB3 (also
/// SWD_CLK) on port B.
pub const IR_PA_BIT: u32 = board::bit(board::IR_PA);
pub const IR_PB_BIT: u32 = board::bit(board::IR_PB);

/// The architectural SysTick control and status register (CTRL).
pub const SYSTICK_CTRL: u32 = 0xe000_e010;

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
pub const BANK1_SECTORS: u32 = 0x15_0000 / SECTOR_BYTES;

/// Bank 0, the factory loader's fallback (read only, for the give-way guard).
pub const BANK0_OFFSET: u32 = 0x6000;
/// The factory loader's valid-image marker at the start of each bank.
pub const MARKER: [u8; 8] = *b"81958711";

/// Owner-approved replacement-firmware storage: 32 sectors at
/// 0x340000-0x360000. FW15 approved the first 16; FW44 extended it with the
/// owner's approval (October 23, 2026) for client keys, plans and the 7-day
/// event ring. A static pass over stock 1.1.15 found no fixed or bounded
/// computed flash access in 0x306000-0x3e8000, and every full dump of the
/// research unit is blank there. Clear of bank 0, of a full-size bank 1
/// (ends no later than 0x306000) and of the stock settings (0x3e8000).
/// Callers name a sector index; nothing takes a flash address.
pub const STORAGE_OFFSET: u32 = 0x34_0000;
pub const STORAGE_SECTORS: u32 = 32;
/// One NOR erase sector.
pub const SECTOR_BYTES: u32 = 0x1000;

/// FW45: who owns each sector of the storage region, in one place. Each
/// store keeps its sectors to itself, so a torn record is always its own.
pub mod storage {
    use super::STORAGE_SECTORS;

    /// `count` sectors from region sector `first`.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Sectors {
        pub first: u32,
        pub count: u32,
    }

    impl Sectors {
        const fn end(self) -> u32 {
            self.first + self.count
        }
    }

    /// The feed journal's slot pair (FW15).
    pub const JOURNAL: Sectors = Sectors { first: 0, count: 2 };
    /// Device settings (FW15).
    pub const SETTINGS: Sectors = Sectors { first: 2, count: 2 };
    /// The boot confirmation record (FW11).
    pub const CONFIRM: Sectors = Sectors { first: 4, count: 1 };
    /// Wi-Fi credentials (FW25).
    pub const NET: Sectors = Sectors { first: 5, count: 2 };
    /// The random generator's seed carried across boots (FW45).
    pub const SEED: Sectors = Sectors { first: 7, count: 2 };
    /// The feeder's identity key and enrolled clients (FW45).
    pub const CLIENTS: Sectors = Sectors { first: 9, count: 2 };
    /// Reserved for feeding plans (FP03).
    pub const PLANS: Sectors = Sectors {
        first: 11,
        count: 2,
    };
    /// Reserved for the 7-day event ring: 11 sectors of records and one
    /// being erased (FW47, sized by FW44).
    pub const EVENTS: Sectors = Sectors {
        first: 13,
        count: 12,
    };

    /// Every allocation, in region order; sectors 25-31 are free.
    pub const ALL: [Sectors; 8] = [
        JOURNAL, SETTINGS, CONFIRM, NET, SEED, CLIENTS, PLANS, EVENTS,
    ];

    const fn disjoint_and_inside() -> bool {
        let mut i = 1;
        while i < ALL.len() {
            if ALL[i - 1].end() > ALL[i].first || ALL[i].count == 0 {
                return false;
            }
            i += 1;
        }
        ALL[ALL.len() - 1].end() <= STORAGE_SECTORS
    }
    const _: () = assert!(disjoint_and_inside());
}

/// FW24: samples in one codec DMA page (16 ms at 16 kHz, 16-bit mono),
/// `PETLIBRE_AUDIO_PAGE` in `example_entry.c`.
pub const AUDIO_PAGE_SAMPLES: usize = 256;
/// Pages in the ring.
pub const AUDIO_PAGES: usize = 8;

/// One event from the KM4 Wi-Fi shim's ring (`example_entry.c`), numbered
/// from 1. `kind` is one of the associated constants: 1 on, 2 scan,
/// 3 connect, 4 DHCP, 5 lost, 6 backoff, 7 DNS.
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

impl WifiEvent {
    pub const ON: u32 = 1;
    pub const SCAN: u32 = 2;
    pub const CONNECT: u32 = 3;
    pub const DHCP: u32 = 4;
    pub const LOST: u32 = 5;
    pub const BACKOFF: u32 = 6;
    /// A DNS check of the server name: `result` 0 and `value` nonzero on success.
    pub const DNS: u32 = 7;
}

/// `wifi_event(seq)` outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WifiPoll {
    Pending,
    Event(WifiEvent),
    /// The ring overwrote it before it was read.
    Lost,
}

/// FW34: one event from the BLE peripheral's ring (`petlibre_ble.c`),
/// numbered from 1 as [`WifiEvent`] is. `kind` is one of the associated
/// constants.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BleEvent {
    pub seq: u32,
    pub kind: u32,
    /// Ready: 0 or a failure (`READY_*`); disconnect: the HCI cause.
    pub result: i32,
    /// Advertising: 1 started, 0 stopped; connect: the interval in 1.25 ms
    /// units; MTU; notify: 1 enabled, 0 disabled.
    pub value: u32,
}

impl BleEvent {
    pub const READY: u32 = 1;
    pub const ADVERTISING: u32 = 2;
    pub const CONNECT: u32 = 3;
    pub const DISCONNECT: u32 = 4;
    pub const MTU: u32 = 5;
    pub const NOTIFY: u32 = 8;

    /// Ready failures: the radio never came up, then the stack's own steps.
    pub const READY_WIFI: i32 = -1;
    pub const READY_BTE: i32 = -2;
    pub const READY_SERVICE: i32 = -3;
    pub const READY_STACK: i32 = -4;
}

/// `ble_event(seq)` outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlePoll {
    Pending,
    Event(BleEvent),
    /// The ring overwrote it before it was read.
    Lost,
}

/// What the BLE peripheral advertises and serves: the name in its scan
/// response, and its service's 128-bit UUIDs as the stack takes them
/// (little-endian).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BleService {
    pub name: &'static [u8],
    pub service: [u8; 16],
    pub control: [u8; 16],
}

/// Whether the peripheral advertises while no host is connected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Advertising {
    On,
    Off,
}

/// The BLE peripheral's counters and link state.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BleStats {
    /// Bytes refused because a ring was full.
    pub control_dropped: u32,
    pub notify_dropped: u32,
    /// The FreeRTOS heap's free bytes, which the BT stack allocates from.
    pub heap_free: u32,
    pub mtu: u32,
    pub connected: u32,
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

/// The core's SysTick, which counts down at the CPU clock from `reload` to
/// zero and FreeRTOS reloads for its tick (FW45 reads it as a noise source).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Systick {
    pub value: u32,
    pub reload: u32,
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
