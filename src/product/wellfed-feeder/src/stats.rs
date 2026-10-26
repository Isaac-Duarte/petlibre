//! The feeder's statistics: what the device API's `Stats` op reports and
//! the live metrics beyond power and signal (behind `Capabilities::STATS`).
//!
//! [`Keeper`] lives in the loop and is fed by what the pass already sees:
//! the feeding owner's results, the server's session records, the panel's
//! gestures, the power samples, the link's counters and the loop's own pass
//! times. Nothing here starts a task, waits or reads flash; each update is
//! a few additions, so the loop's timing and stack hold.
//!
//! Everything here counts since boot, in RAM: nothing is written to flash,
//! so no figure costs a write turn or wears a sector. The wire's lifetime
//! fields stay `None` until storage sectors are allocated to lifetime
//! totals (the storage region has none free).

use wellfed_proto::api::{
    self as proto, Counters, Counts, CurrentUnit, EventBody, FeedOutcome, FeedStats, Food, Metric,
    POWER_HISTORY, Power, PowerStats, ResetCause, TraceSummary,
};

use crate::feeder::platform::{Link, TracesFill};

/// One point of the battery's history.
pub const HISTORY_MS: u32 = 60 * 60_000;

/// Why a boot started, from the ROM's reset flags (`BOOT_Reason` bits
/// 16-21: system, watchdog, brownout, KM4 system, KM4 watchdog, deep
/// sleep); none set is a power-on.
pub const fn reset_cause(flags: u32) -> ResetCause {
    const SYSTEM: u32 = 1 << 16;
    const WATCHDOG: u32 = 1 << 17;
    const BROWNOUT: u32 = 1 << 18;
    const KM4_SYSTEM: u32 = 1 << 19;
    const KM4_WATCHDOG: u32 = 1 << 20;
    const CAUSES: u32 = 0x3f << 16;

    if flags & CAUSES == 0 {
        ResetCause::PowerOn
    } else if flags & BROWNOUT != 0 {
        ResetCause::Brownout
    } else if flags & (WATCHDOG | KM4_WATCHDOG) != 0 {
        ResetCause::Watchdog
    } else if flags & (SYSTEM | KM4_SYSTEM) != 0 {
        ResetCause::Software
    } else {
        ResetCause::Other
    }
}

/// Buckets of [`PassTimes`]: eight under 8 µs, then eight per octave up
/// to about a second.
const BUCKETS: usize = 8 + 8 * 18;

/// The loop's pass times over one sample window, in buckets an eighth of
/// an octave wide, so the 99th percentile costs no sort and no history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PassTimes {
    passes: u32,
    max_us: u32,
    buckets: [u16; BUCKETS],
}

impl PassTimes {
    pub const fn new() -> Self {
        Self {
            passes: 0,
            max_us: 0,
            buckets: [0; BUCKETS],
        }
    }

    fn bucket(us: u32) -> usize {
        let index = if us < 8 {
            us
        } else {
            let octave = 31 - us.leading_zeros();
            let sub = (us >> (octave - 3)) & 7;
            8 + (octave - 3) * 8 + sub
        };
        usize::try_from(index).map_or(BUCKETS - 1, |index| index.min(BUCKETS - 1))
    }

    /// The largest time bucket `index` holds.
    fn upper(index: usize) -> u32 {
        let Ok(index) = u32::try_from(index) else {
            return u32::MAX;
        };
        if index < 8 {
            return index;
        }
        let octave = (index - 8) / 8 + 3;
        let sub = (index - 8) % 8;
        ((8 + sub + 1) << (octave - 3)) - 1
    }

    pub fn add(&mut self, us: u32) {
        self.passes = self.passes.saturating_add(1);
        self.max_us = self.max_us.max(us);
        if let Some(count) = self.buckets.get_mut(Self::bucket(us)) {
            *count = count.saturating_add(1);
        }
    }

    /// The window's metric, and a fresh window.
    fn take(&mut self) -> Metric {
        let rank = self.passes - self.passes / 100;
        let mut seen = 0u32;
        let mut p99_us = 0;
        for (index, &count) in self.buckets.iter().enumerate() {
            seen = seen.saturating_add(u32::from(count));
            if seen >= rank && count > 0 {
                p99_us = Self::upper(index).min(self.max_us);
                break;
            }
        }
        let metric = Metric::Loop {
            passes: self.passes,
            max_us: self.max_us,
            p99_us,
        };
        *self = Self::new();
        metric
    }
}

impl Default for PassTimes {
    fn default() -> Self {
        Self::new()
    }
}

/// The supply since boot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Supply {
    adapter: Option<bool>,
    changes: u32,
    outages: u32,
    battery_ms: u64,
    /// When the supply was last sampled.
    sampled_ms: Option<u32>,
    min_mv: Option<u32>,
    max_mv: Option<u32>,
    latest_mv: Option<u32>,
    history: [Option<u16>; POWER_HISTORY],
    /// When the next history point is taken.
    point_at: u32,
}

impl Supply {
    const fn new() -> Self {
        Self {
            adapter: None,
            changes: 0,
            outages: 0,
            battery_ms: 0,
            sampled_ms: None,
            min_mv: None,
            max_mv: None,
            latest_mv: None,
            history: [None; POWER_HISTORY],
            point_at: HISTORY_MS,
        }
    }

    fn sample(&mut self, now_ms: u32, power: Power) {
        // Time since the last sample counts as on battery if the batteries
        // carried it then.
        if let (Some(at), Some(false)) = (self.sampled_ms, self.adapter) {
            self.battery_ms = self
                .battery_ms
                .saturating_add(u64::from(now_ms.wrapping_sub(at)));
        }
        self.sampled_ms = Some(now_ms);

        if let Some(was) = self.adapter
            && was != power.adapter
        {
            self.changes = self.changes.saturating_add(1);
            if !power.adapter {
                self.outages = self.outages.saturating_add(1);
            }
        }
        self.adapter = Some(power.adapter);

        if let Some(mv) = power.battery_mv {
            self.min_mv = Some(self.min_mv.map_or(mv, |min| min.min(mv)));
            self.max_mv = Some(self.max_mv.map_or(mv, |max| max.max(mv)));
        }
        self.latest_mv = power.battery_mv;
    }

    /// Take a history point when one is due.
    fn tick(&mut self, now_ms: u32) {
        if !wellfed_core::deadline_passed(now_ms, self.point_at) {
            return;
        }
        self.point_at = self.point_at.wrapping_add(HISTORY_MS);
        self.history.rotate_left(1);
        if let Some(last) = self.history.last_mut() {
            *last = self.latest_mv.and_then(|mv| u16::try_from(mv).ok());
        }
    }

    fn battery_s(&self) -> u32 {
        u32::try_from(self.battery_ms / 1000).unwrap_or(u32::MAX)
    }
}

/// The food beam since boot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Beam {
    present: Option<bool>,
    emptied: u32,
    refilled: u32,
    feeds_since_refill: Option<u32>,
}

impl Beam {
    fn reading(&mut self, present: bool) {
        match self.present {
            Some(true) if !present => self.emptied = self.emptied.saturating_add(1),
            Some(false) if present => {
                self.refilled = self.refilled.saturating_add(1);
                self.feeds_since_refill = Some(0);
            }
            _ => {}
        }
        self.present = Some(present);
    }
}

/// This boot's feeds as their traces summed them up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Traced {
    traced: u32,
    drive_ms: u32,
    samples: u32,
    /// Each feed's mean times its readings, for the boot's mean.
    current_total: u64,
    current_peak: u32,
    rising: u32,
    falling: u32,
    beam_breaks: u32,
    reversals: u32,
    last: Option<TraceSummary>,
}

impl Traced {
    const fn new() -> Self {
        Self {
            traced: 0,
            drive_ms: 0,
            samples: 0,
            current_total: 0,
            current_peak: 0,
            rising: 0,
            falling: 0,
            beam_breaks: 0,
            reversals: 0,
            last: None,
        }
    }
}

/// The feeder's statistics, kept by the loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Keeper {
    reset: ResetCause,
    /// The uptime clock's wraps (every 49.7 days) and its last reading, so
    /// the uptime counts on past them.
    wraps: u32,
    last_ms: u32,
    feeds: u32,
    feeds_failed: u32,
    portions: u32,
    presses: u32,
    sessions: u32,
    handshake_max_ms: u32,
    handshake_total_ms: u64,
    link: Link,
    /// The link's gateway answers when the last gateway metric was taken.
    answers_seen: u32,
    supply: Supply,
    beam: Beam,
    passes: PassTimes,
    traces: Traced,
}

impl Default for Keeper {
    fn default() -> Self {
        Self::new()
    }
}

impl Keeper {
    pub const fn new() -> Self {
        Self {
            reset: ResetCause::PowerOn,
            wraps: 0,
            last_ms: 0,
            feeds: 0,
            feeds_failed: 0,
            portions: 0,
            presses: 0,
            sessions: 0,
            handshake_max_ms: 0,
            handshake_total_ms: 0,
            link: Link {
                rssi: 0,
                link_losses: 0,
                check_losses: 0,
                dhcp_renewals: 0,
                gateway_answers: 0,
                gateway_ms: 0,
                channel: None,
                ap_changes: 0,
            },
            answers_seen: 0,
            supply: Supply::new(),
            beam: Beam {
                present: None,
                emptied: 0,
                refilled: 0,
                feeds_since_refill: None,
            },
            passes: PassTimes::new(),
            traces: Traced::new(),
        }
    }

    /// A feed's trace, once sealed.
    pub fn traced(&mut self, summary: &TraceSummary) {
        let traces = &mut self.traces;
        traces.traced = traces.traced.saturating_add(1);
        traces.drive_ms = traces.drive_ms.saturating_add(summary.drive_ms);
        traces.samples = traces.samples.saturating_add(summary.samples);
        let total = u64::from(summary.current_mean.unwrap_or(0)) * u64::from(summary.samples);
        traces.current_total = traces.current_total.saturating_add(total);
        traces.current_peak = traces.current_peak.max(summary.current_peak);
        traces.rising = traces.rising.saturating_add(u32::from(summary.rising));
        traces.falling = traces.falling.saturating_add(u32::from(summary.falling));
        traces.beam_breaks = traces
            .beam_breaks
            .saturating_add(u32::from(summary.beam_breaks));
        traces.reversals = traces
            .reversals
            .saturating_add(u32::from(summary.reversals));
        traces.last = Some(*summary);
    }

    /// The feeds section, with the platform's trace-ring figures.
    pub fn feed_stats(&self, fill: TracesFill) -> FeedStats {
        let traces = &self.traces;
        let mean = traces
            .current_total
            .checked_div(u64::from(traces.samples))
            .map(|mean| u32::try_from(mean).unwrap_or(u32::MAX));
        FeedStats {
            traced: traces.traced,
            drive_ms: traces.drive_ms,
            samples: traces.samples,
            unit: CurrentUnit::Scaled,
            current_peak: traces.current_peak,
            current_mean: mean,
            rising: traces.rising,
            falling: traces.falling,
            beam_breaks: traces.beam_breaks,
            reversals: traces.reversals,
            kept: fill.kept,
            capacity: fill.capacity,
            unsaved: fill.unsaved,
            last: traces.last,
        }
    }

    /// A feed's `Fed` event, the one record of how it ended.
    pub fn fed(&mut self, body: &EventBody) {
        let EventBody::Fed {
            completed, outcome, ..
        } = *body
        else {
            return;
        };
        self.feeds = self.feeds.saturating_add(1);
        if outcome != FeedOutcome::Done {
            self.feeds_failed = self.feeds_failed.saturating_add(1);
        }
        self.portions = self.portions.saturating_add(u32::from(completed));
        if let Some(feeds) = self.beam.feeds_since_refill.as_mut() {
            *feeds = feeds.saturating_add(1);
        }
    }

    /// A session opened after `handshake_ms`.
    pub fn opened(&mut self, handshake_ms: u32) {
        self.sessions = self.sessions.saturating_add(1);
        self.handshake_max_ms = self.handshake_max_ms.max(handshake_ms);
        self.handshake_total_ms = self
            .handshake_total_ms
            .saturating_add(u64::from(handshake_ms));
    }

    /// The panel's presses this boot so far.
    pub fn presses(&mut self, presses: u32) {
        self.presses = presses;
    }

    /// The link as the platform reports it this pass.
    pub fn link(&mut self, link: Link) {
        self.link = link;
    }

    pub fn power(&mut self, now_ms: u32, power: Power) {
        self.supply.sample(now_ms, power);
    }

    /// The food beam's reading, when the feeding owner took one.
    pub fn food(&mut self, present: bool) {
        self.beam.reading(present);
    }

    /// One loop pass took `us`.
    pub fn pass_took(&mut self, us: u32) {
        self.passes.add(us);
    }

    /// Housekeeping each pass: the battery's history.
    pub fn tick(&mut self, now_ms: u32) {
        if now_ms < self.last_ms {
            self.wraps = self.wraps.saturating_add(1);
        }
        self.last_ms = now_ms;
        self.supply.tick(now_ms);
    }

    /// This boot's reset flags, as the ROM left them.
    pub fn boot(&mut self, reset_flags: u32) {
        self.reset = reset_cause(reset_flags);
    }

    /// Seconds since boot, past the millisecond clock's wraps; the caller
    /// ticks every pass, so at most one wrap goes unseen.
    fn uptime_s(&self, now_ms: u32) -> u32 {
        let ms = (u64::from(self.wraps) << 32) | u64::from(now_ms);
        u32::try_from(ms / 1000).unwrap_or(u32::MAX)
    }

    /// The loop's pass times since the last call.
    pub fn loop_metric(&mut self) -> Metric {
        self.passes.take()
    }

    /// The latest gateway round trip answered since the last call.
    pub fn gateway_metric(&mut self) -> Metric {
        let answered = self.link.gateway_answers != self.answers_seen;
        self.answers_seen = self.link.gateway_answers;
        Metric::Gateway {
            round_trip_ms: answered.then_some(self.link.gateway_ms),
        }
    }

    pub fn wifi_metric(&self) -> Metric {
        Metric::Wifi {
            channel: self.link.channel,
            ap_changes: self.link.ap_changes,
        }
    }

    /// This boot's counts at `now_ms`.
    pub fn counts(&self, now_ms: u32) -> Counts {
        Counts {
            uptime_s: self.uptime_s(now_ms),
            feeds: self.feeds,
            feeds_failed: self.feeds_failed,
            portions: self.portions,
            presses: self.presses,
            sessions: self.sessions,
            reconnects: self.link.link_losses.saturating_add(self.link.check_losses),
            dhcp_renewals: self.link.dhcp_renewals,
            outages: self.supply.outages,
            battery_s: self.supply.battery_s(),
        }
    }

    /// The counters section.
    pub fn counters(&self, now_ms: u32) -> Counters {
        let mean = self
            .handshake_total_ms
            .checked_div(u64::from(self.sessions))
            .unwrap_or(0);
        Counters {
            boot: self.counts(now_ms),
            reset: self.reset,
            lifetime: None,
            resets: None,
            handshake_max_ms: self.handshake_max_ms,
            handshake_mean_ms: u32::try_from(mean).unwrap_or(u32::MAX),
        }
    }

    /// The flash section, with the platform's event-ring figures.
    pub fn flash(erases: proto::Erases, events_kept: u32, events_capacity: u32) -> proto::Flash {
        proto::Flash {
            boot: erases,
            lifetime: None,
            events_kept,
            events_capacity,
        }
    }

    pub fn power_stats(&self) -> PowerStats {
        let supply = &self.supply;
        PowerStats {
            adapter: supply.adapter,
            adapter_changes: supply.changes,
            outages: supply.outages,
            battery_s: supply.battery_s(),
            battery_min_mv: supply.min_mv,
            battery_max_mv: supply.max_mv,
            battery_mv: supply.history,
            period_s: HISTORY_MS / 1000,
        }
    }

    pub fn food_stats(&self) -> Food {
        let beam = &self.beam;
        Food {
            present: beam.present,
            emptied: beam.emptied,
            refilled: beam.refilled,
            feeds_since_refill: beam.feeds_since_refill,
        }
    }
}
