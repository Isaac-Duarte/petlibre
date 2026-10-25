//! EXPERIMENTAL KM4 watchdog in reset mode with HS full-platform reset and
//! shared peripheral-reset scope. Requires exclusive watchdog/scope-bit
//! ownership across cores and independently verified inactive boot/reset pins.
//! Caller-owned fixed-address object, task context with interrupts enabled.

use core::ptr;

use wellfed_core::clock::tick::TickClock;
use wellfed_core::flash_coord::FlashCoord;
use wellfed_core::{HALF_RANGE, interval_valid};

use crate::board::{self, Board};
use crate::sdk::{self, IrqGuard};
use crate::supervision::active::active_pointer;

/// The vendor-register fields `open` programs: divisor (bits 0..15), count ID
/// (bits 25..28), enable and reset mode.
const CONTROL_MASK: u32 = 0x1e00_ffff | sdk::WDG_BIT_ENABLE | sdk::WDG_BIT_RST_MODE;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WatchdogConfig {
    pub systimer_hz: u32,
    pub clock_min_hz: u32,
    pub clock_max_hz: u32,
    pub min_refresh_ms: u32,
    pub max_progress_gap_ms: u32,
    /// Upper bound for counter expiry, excluding subsequent reset/boot latency.
    pub max_expiry_ms: u32,
    pub divisor: u16,
    pub count_id: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum WatchdogFault {
    #[default]
    Ok = 0,
    Setup = 1,
    Context = 2,
    Clock = 3,
    Progress = 4,
    Register = 5,
}

impl WatchdogFault {
    /// The variant's number, as records and reports print it.
    pub const fn code(self) -> u32 {
        self as u32
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Watchdog {
    pub clock: TickClock,
    pub progress: u64,
    pub progress_at: u32,
    pub refresh_at: u32,
    pub expected_control: u32,
    pub old_hs_scope: u32,
    pub old_peripheral_scope: u32,
    pub config: WatchdogConfig,
    pub fault: WatchdogFault,
    pub claimed: bool,
    pub armed: bool,
}

active_pointer!(Watchdog);

impl WatchdogConfig {
    /// Explicit measured clock bounds, count/divisor and progress limits; no
    /// defaults. The shortest hardware timeout must exceed twice the progress
    /// limit; the longest expiry must fit the configured upper bound.
    pub fn valid(&self) -> bool {
        if self.systimer_hz < 1000
            || self.systimer_hz > 1_000_000
            || self.clock_min_hz < 1000
            || self.clock_max_hz > 1_000_000
            || self.clock_min_hz > self.clock_max_hz
            || self.count_id > 11
            || self.divisor == 0
            || self.min_refresh_ms == 0
            || self.min_refresh_ms >= self.max_progress_gap_ms
            || self.max_progress_gap_ms >= HALF_RANGE
            || !interval_valid(self.max_expiry_ms)
        {
            return false;
        }

        let ticks = ((1u64 << (self.count_id + 1)) - 1) * (u64::from(self.divisor) + 1);
        let shortest_ms = ticks * 1000 / u64::from(self.clock_max_hz);
        let longest_ms = (ticks * 1000).div_ceil(u64::from(self.clock_min_hz));
        shortest_ms > u64::from(self.max_progress_gap_ms) * 2
            && longest_ms <= u64::from(self.max_expiry_ms)
    }
}

fn context() -> bool {
    sdk::primask() == 0 && sdk::ipsr() == 0
}

impl Watchdog {
    fn fail(&mut self, reason: WatchdogFault) -> bool {
        if self.fault == WatchdogFault::Ok {
            self.fault = reason;
        }
        false
    }

    fn registers(&mut self) -> bool {
        let value = sdk::wdg_vendor();
        self.armed = value & sdk::WDG_BIT_ENABLE != 0;
        value & CONTROL_MASK == self.expected_control
            && value & sdk::WDG_BIT_ISR_CLEAR == 0
            && sdk::hs_platform_para() & sdk::HS_WDTRST_OPT != 0
            && sdk::lp_km0_ctrl() & sdk::LP_WDT_RESET_HS_ALL != 0
    }

    fn release(&mut self) -> bool {
        sdk::wdg_cmd(false);
        if sdk::wdg_vendor() & sdk::WDG_BIT_ENABLE != 0 {
            return false;
        }
        self.armed = false;

        sdk::set_hs_platform_para(
            (sdk::hs_platform_para() & !sdk::HS_WDTRST_OPT) | self.old_hs_scope,
        );
        sdk::set_lp_km0_ctrl(
            (sdk::lp_km0_ctrl() & !sdk::LP_WDT_RESET_HS_ALL) | self.old_peripheral_scope,
        );

        if sdk::hs_platform_para() & sdk::HS_WDTRST_OPT != self.old_hs_scope
            || sdk::lp_km0_ctrl() & sdk::LP_WDT_RESET_HS_ALL != self.old_peripheral_scope
        {
            return false;
        }

        self.claimed = false;
        set_active(ptr::null_mut());
        true
    }

    /// Failed setup can retain claimed ownership if rollback cannot be verified.
    pub fn open(&mut self, config: &WatchdogConfig) -> bool {
        if !config.valid() || !context() {
            return false;
        }

        sdk::disable_irq();
        if !active().is_null() || sdk::wdg_vendor() & sdk::WDG_BIT_ENABLE != 0 {
            sdk::set_primask(0);
            return false;
        }

        *self = Self {
            config: *config,
            claimed: true,
            ..Self::default()
        };
        set_active(self);

        self.old_hs_scope = sdk::hs_platform_para() & sdk::HS_WDTRST_OPT;
        self.old_peripheral_scope = sdk::lp_km0_ctrl() & sdk::LP_WDT_RESET_HS_ALL;
        self.expected_control =
            (u32::from(config.count_id) << 25) | u32::from(config.divisor) | sdk::WDG_BIT_RST_MODE;
        self.clock =
            TickClock::new(config.systimer_hz, sdk::systimer_ticks(), 0).unwrap_or_default();

        sdk::set_hs_platform_para(sdk::hs_platform_para() | sdk::HS_WDTRST_OPT);
        sdk::wdg_init(u32::from(config.count_id), u32::from(config.divisor));
        let mut ok = self.registers();
        if ok {
            sdk::wdg_cmd(true);
            self.expected_control |= sdk::WDG_BIT_ENABLE;
            ok = self.registers();
        }

        self.armed = sdk::wdg_vendor() & sdk::WDG_BIT_ENABLE != 0;
        if !ok {
            self.fail(WatchdogFault::Setup);
            self.release(); // Retain ownership if disable/restore cannot be verified.
        }

        sdk::set_primask(0);
        ok
    }

    /// Called only while serialized. Failed reads never extend progress or refresh.
    fn check_locked(&mut self) -> Option<u32> {
        if active() != ptr::from_mut(self)
            || !self.claimed
            || !self.armed
            || self.fault != WatchdogFault::Ok
        {
            return None;
        }

        if !self.registers() {
            self.fail(WatchdogFault::Register);
            return None;
        }

        let Some(now) = self.clock.read(sdk::systimer_ticks()) else {
            self.fail(WatchdogFault::Clock);
            return None;
        };

        if now.wrapping_sub(self.progress_at) >= self.config.max_progress_gap_ms {
            self.fail(WatchdogFault::Progress);
            return None;
        }

        Some(now)
    }

    fn checked(&mut self) -> Option<u32> {
        let task = context();
        let _guard = IrqGuard::lock();
        if task {
            self.check_locked()
        } else {
            self.fail(WatchdogFault::Context);
            None
        }
    }

    /// Does not refresh or advance progress; rejects expired/altered supervision.
    pub fn check(&mut self) -> bool {
        self.checked().is_some()
    }

    fn checkpoint_with(&mut self, progress: u64, flash: Option<*const FlashCoord>) -> bool {
        let task = context();
        let _guard = IrqGuard::lock();
        let now = if task {
            self.check_locked()
        } else {
            self.fail(WatchdogFault::Context);
            None
        };
        let Some(now) = now else { return false };

        if progress == 0 || progress <= self.progress {
            return self.fail(WatchdogFault::Progress);
        }

        if flash.is_none_or(|flash| !board::outputs_blocked(flash)) {
            self.progress = progress;
            self.progress_at = now;
            if now.wrapping_sub(self.refresh_at) >= self.config.min_refresh_ms {
                sdk::wdg_refresh();
                self.refresh_at = now;
                if !self.registers() {
                    return self.fail(WatchdogFault::Register);
                }
            }
        }
        true
    }

    /// Owner only, AFTER a completed verified frame. Increasing nonzero token
    /// and advancing hardware time required. Failures latch until shutdown.
    #[must_use]
    pub fn checkpoint(&mut self, progress: u64) -> bool {
        self.checkpoint_with(progress, None)
    }

    /// Permission is tested under the refresh critical section. A hold
    /// returns healthy without extending progress/expiry. Requires an authority.
    ///
    /// # Safety
    /// `flash` is null or a live authority for the duration of the call.
    pub unsafe fn checkpoint_if_ready(&mut self, progress: u64, flash: *const FlashCoord) -> bool {
        !flash.is_null() && self.checkpoint_with(progress, Some(flash))
    }

    /// Explicit cooperative shutdown only, after events close and verified
    /// outputs. Restores only owned scope bits. Failure retains ownership.
    #[must_use]
    pub fn close(&mut self, board: Option<&Board>) -> bool {
        if !context() {
            return false;
        }
        sdk::disable_irq();

        let me = ptr::from_mut(self);
        let mut ok = !self.claimed && active() != me;
        let inactive = board.is_some_and(Board::inactive);

        if active() == me && self.claimed && inactive {
            ok = self.release();
            if !ok {
                self.fail(WatchdogFault::Register);
            }
        }

        sdk::set_primask(0);
        ok
    }
}
