//! The SDK surface used by the PLAF103 AmebaD ports.
//!
//! Target builds call the pinned AmebaD SDK through bindgen-generated
//! declarations; host builds use [`fake`], a deterministic model of the same
//! calls for tests and the virtual feeder. Both expose identical functions.
//! The board's pins and storage region are `wellfed-board-plaf103`'s; the
//! panic and RAM-park output writes and the storage bounds read them there.
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

/// One NOR erase sector.
pub const SECTOR_BYTES: u32 = 0x1000;

/// Words of `wellfed_time_carry`, the 64 bytes the builder's KM4 linker
/// script leaves above the heap at the top of main RAM (`example_entry.c`).
pub const TIME_CARRY_WORDS: usize = 16;

/// Samples in one codec DMA page (16 ms at 16 kHz, 16-bit mono),
/// `WELLFED_AUDIO_PAGE` in `example_entry.c`.
pub const AUDIO_PAGE_SAMPLES: usize = 256;
/// Pages in the ring.
pub const AUDIO_PAGES: usize = 8;

/// One event from the KM4 Wi-Fi shim's ring (`example_entry.c`), numbered
/// from 1. `kind` is one of the associated constants: 1 on, 2 scan,
/// 3 connect, 4 DHCP, 5 lost, 6 backoff, 8 gateway, 9 leave, 10 access
/// point, 11 renewal. 7 was the internet DNS check the gateway probe
/// replaced; it is never reused.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WifiEvent {
    pub seq: u32,
    pub kind: u32,
    pub attempt: u32,
    /// SDK result: `RTW_SUCCESS` (0) or an error; for DHCP the `LwIP_DHCP`
    /// state; for a loss its cause (`wellfed_core::liveness::Cause`); for a
    /// gateway probe 0 answered or a negative failure.
    pub result: i32,
    /// Scan: network found; DHCP: assigned IPv4; lost: ms connected;
    /// backoff: ms; gateway: the gateway's IPv4; leave: still associated;
    /// access point: its BSSID's last 4 bytes.
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
    /// One ICMP echo to the lease's gateway: `result` 0 when it answered.
    pub const GATEWAY: u32 = 8;
    /// The leave before an intentional restart: `result` is
    /// `wifi_disconnect`'s, `value` 1 if still associated at the shim's bound.
    pub const LEAVE: u32 = 9;
    /// The access point a join landed on, once it has an address: `result`
    /// is its channel (-1 unread), `value` its BSSID's last 4 bytes (0
    /// unread).
    pub const AP: u32 = 10;
    /// lwIP's DHCP client took a new ACK without a rejoin.
    pub const RENEW: u32 = 11;
}

/// One task's stack, as `wellfed_task_stacks` (`example_entry.c`) reports
/// it: the name, NUL-padded, and the fewest free bytes since it started.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TaskStack {
    pub name: [u8; 16],
    pub free_min: u32,
}

/// What `task_stacks` filled, and how many tasks the RTOS runs: more
/// than listed when they did not all fit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TaskCount {
    pub listed: usize,
    pub total: u32,
}

impl TaskStack {
    /// The name up to its first NUL, if it is UTF-8.
    pub fn name(&self) -> &str {
        let len = self
            .name
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(self.name.len());
        core::str::from_utf8(&self.name[..len]).unwrap_or("")
    }
}

/// `wifi_event(seq)` outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WifiPoll {
    Pending,
    Event(WifiEvent),
    /// The ring overwrote it before it was read.
    Lost,
}

/// One event from the BLE peripheral's ring (`wellfed_ble.c`),
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

/// What a non-blocking device API receive found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApiRecv {
    Data(usize),
    /// Nothing waiting now.
    Empty,
    /// The peer closed the connection, or the socket failed.
    Closed,
}

/// What a non-blocking accept found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Accepted {
    Socket(i32),
    /// No connection waits.
    None,
    /// The listener failed; open a new one.
    Error,
}

/// One mDNS datagram, if one waits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Datagram {
    Got { len: usize, ip: u32, port: u16 },
    None,
    Error,
}

/// The core's SysTick, which counts down at the CPU clock from `reload` to
/// zero and FreeRTOS reloads for its tick (the entropy source reads it as noise).
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

/// The log tail's size: about 70 record lines of the usual length.
pub const LOG_TAIL_BYTES: usize = 4096;

/// The newest lines [`log`] printed, for the device API to stream. A task
/// copies its line in while the scheduler is still suspended for the
/// print, so only an interrupt can find another writer mid-copy (it drops
/// its line and counts it). On the target it exists only with the
/// `log-tail` feature, so a firmware that serves no API spends no RAM on
/// it.
#[cfg(any(not(target_os = "none"), feature = "log-tail"))]
pub static LOG_TAIL: wellfed_core::log_tail::LogTail<LOG_TAIL_BYTES> =
    wellfed_core::log_tail::LogTail::new();

#[cfg(all(target_os = "none", feature = "km4"))]
mod target;
#[cfg(all(target_os = "none", feature = "km4"))]
pub use target::*;

#[cfg(not(target_os = "none"))]
pub mod fake;
#[cfg(not(target_os = "none"))]
pub use fake::api::*;

// The calls the shared wrappers below (`Audio`) build on.
#[cfg(not(target_os = "none"))]
use fake::api as backend;
#[cfg(all(target_os = "none", feature = "km4"))]
use target as backend;

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

#[cfg(any(not(target_os = "none"), feature = "km4"))]
mod audio {
    use super::{AUDIO_PAGE_SAMPLES, backend};
    use core::marker::PhantomData;

    /// The codec's DMA ring, as one handle. A [`Page`] borrows it mutably, so
    /// one page is out at a time and none outlives the handle; [`Audio::take`]
    /// hands out at most one handle, so two callers never fill the same page.
    #[derive(Debug)]
    pub struct Audio {
        /// Not `Send`: the fake keeps the take flag and the page per thread,
        /// and the ring belongs to the task that took it.
        _owned: PhantomData<*const ()>,
    }

    impl Audio {
        /// The ring's one handle, or `None` while another is out.
        pub fn take() -> Option<Self> {
            backend::audio_take().then_some(Self {
                _owned: PhantomData,
            })
        }

        /// The next free ring page, while the codec is open and the DMA does
        /// not own every page. Fill it, then [`Page::submit`].
        pub fn page(&mut self) -> Option<Page<'_>> {
            let page = backend::audio_page_ptr();
            // SAFETY: a non-null page is an aligned ring page of
            // `AUDIO_PAGE_SAMPLES` samples that the DMA does not own until it is
            // submitted; borrowing `self` keeps it the only reference to it.
            let samples = unsafe { page.as_mut() }?;
            Some(Page { samples })
        }
    }

    impl Drop for Audio {
        fn drop(&mut self) {
            backend::audio_release();
        }
    }

    /// One free ring page, borrowed from the [`Audio`] handle. Dropping it
    /// unsubmitted leaves the page free, so the next [`Audio::page`] returns it
    /// again.
    #[derive(Debug)]
    pub struct Page<'a> {
        samples: &'a mut [i16; AUDIO_PAGE_SAMPLES],
    }

    impl Page<'_> {
        /// Hand the filled page to the DMA.
        pub fn submit(self) {
            backend::audio_submit();
        }
    }

    impl core::ops::Deref for Page<'_> {
        type Target = [i16; AUDIO_PAGE_SAMPLES];

        fn deref(&self) -> &Self::Target {
            self.samples
        }
    }

    impl core::ops::DerefMut for Page<'_> {
        fn deref_mut(&mut self) -> &mut Self::Target {
            self.samples
        }
    }
}

#[cfg(any(not(target_os = "none"), feature = "km4"))]
pub use audio::{Audio, Page};
