//! The feeder's statistics: what the device API's `Stats` op reports and
//! the live metrics beyond power and signal (behind `Capabilities::STATS`).
//!
//! [`Keeper`] lives in the loop and is fed by what the pass already sees:
//! the feeding owner's results, the server's session records, the panel's
//! gestures, the power samples, the link's counters and the loop's own pass
//! times. Nothing here starts a task, waits or reads flash; each update is
//! a few additions, so the loop's timing and stack hold.
//!
//! Everything here counts since boot, in RAM: nothing is written to flash
//! here, so no figure costs a write turn or wears a sector. The lifetime
//! figures are this boot's added to the [`Totals`] the settings writer
//! keeps in the stats store (`store::stats`), which it loads at boot and
//! saves at most every [`SAVE_MS`], so a power cut loses at most that much
//! of a boot's counts.

use wellfed_proto::api::{
    self as proto, Counters, Counts, CurrentUnit, EventBody, FLASH_SECTORS, FeedOutcome, FeedStats,
    Food, Metric, POWER_HISTORY, Portions, Power, PowerStats, ResetCause, Resets, TraceSummary,
};

use crate::feeder::Marks;
use crate::feeder::platform::{Link, TracesFill};

/// One point of the battery's history.
pub const HISTORY_MS: u32 = 60 * 60_000;

/// The timed (settled) saves of the lifetime totals: every this long of a
/// boot's uptime, the first this long in. The most of a boot a power cut
/// loses.
pub const SAVE_MS: u32 = 60 * 60_000;
/// The early saves' budget: each boot's own save as it starts (so its
/// reset counts however short it is) and an intentional restart's spend a
/// token, and each settled save gives one back. So early saves never
/// outnumber settled ones by more than this, and the store's wear is at
/// most this many erases plus two per hour of uptime: about 17,500 a year,
/// 8,800 of each of its two sectors, whatever the crash pattern.
pub const EARLY_SAVES: u8 = 6;

/// The storage sectors the totals count erases of: the whole region, the
/// board's `STORAGE_SECTORS` (which the runtime asserts). The v1 wire's
/// [`proto::Erases`] lists only the first [`FLASH_SECTORS`]; it never
/// gains a field, so the rest are kept here for a later variant.
pub const TOTAL_SECTORS: usize = 48;

const _: () = assert!(TOTAL_SECTORS >= FLASH_SECTORS);

/// What the feeder counted over its life, up to the stats store's last
/// save: every figure of [`Counts`], the boots by their reset's cause and
/// the erases by storage sector and of bank 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Totals {
    pub counts: Counts,
    pub resets: Resets,
    /// Each storage sector's erases, by its index in the region.
    pub storage: [u32; TOTAL_SECTORS],
    /// Bank 1's sectors, erased by updates.
    pub update: u32,
}

impl Default for Totals {
    fn default() -> Self {
        Self::ZERO
    }
}

/// Two counts' sum, field by field, held at the top rather than wrapped.
pub fn add_counts(a: &Counts, b: &Counts) -> Counts {
    Counts {
        uptime_s: a.uptime_s.saturating_add(b.uptime_s),
        feeds: a.feeds.saturating_add(b.feeds),
        feeds_failed: a.feeds_failed.saturating_add(b.feeds_failed),
        portions: a.portions.saturating_add(b.portions),
        presses: a.presses.saturating_add(b.presses),
        sessions: a.sessions.saturating_add(b.sessions),
        reconnects: a.reconnects.saturating_add(b.reconnects),
        dhcp_renewals: a.dhcp_renewals.saturating_add(b.dhcp_renewals),
        outages: a.outages.saturating_add(b.outages),
        battery_s: a.battery_s.saturating_add(b.battery_s),
    }
}

impl Totals {
    pub const ZERO: Self = Self {
        counts: Counts {
            uptime_s: 0,
            feeds: 0,
            feeds_failed: 0,
            portions: 0,
            presses: 0,
            sessions: 0,
            reconnects: 0,
            dhcp_renewals: 0,
            outages: 0,
            battery_s: 0,
        },
        resets: Resets {
            power_on: 0,
            watchdog: 0,
            software: 0,
            brownout: 0,
            other: 0,
        },
        storage: [0; TOTAL_SECTORS],
        update: 0,
    };

    /// One more boot, for `cause`.
    pub fn count_boot(&mut self, cause: ResetCause) {
        let resets = &mut self.resets;
        let count = match cause {
            ResetCause::PowerOn => &mut resets.power_on,
            ResetCause::Watchdog => &mut resets.watchdog,
            ResetCause::Software => &mut resets.software,
            ResetCause::Brownout => &mut resets.brownout,
            ResetCause::Other => &mut resets.other,
        };
        *count = count.saturating_add(1);
    }

    /// These totals with a boot's own figures added: its `counts`, its
    /// erases by storage sector (`storage`, as far as it reaches) and of
    /// bank 1.
    pub fn plus(&self, counts: &Counts, storage: &[u32], update: u32) -> Self {
        let mut sum = *self;
        sum.counts = add_counts(&self.counts, counts);
        for (total, boot) in sum.storage.iter_mut().zip(storage) {
            *total = total.saturating_add(*boot);
        }
        sum.update = self.update.saturating_add(update);
        sum
    }

    /// The erases as the v1 wire carries them: the first
    /// [`FLASH_SECTORS`].
    pub fn erases(&self) -> proto::Erases {
        let mut storage = [0; FLASH_SECTORS];
        for (out, total) in storage.iter_mut().zip(self.storage) {
            *out = total;
        }
        proto::Erases {
            storage,
            update: self.update,
        }
    }
}

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

    /// The counters section; the lifetime figures from `stored`, the
    /// totals the stats store holds (this boot's reset counted), when it
    /// keeps them.
    pub fn counters(&self, now_ms: u32, stored: Option<&Totals>) -> Counters {
        let mean = self
            .handshake_total_ms
            .checked_div(u64::from(self.sessions))
            .unwrap_or(0);
        let boot = self.counts(now_ms);
        Counters {
            boot,
            reset: self.reset,
            lifetime: stored.map(|stored| add_counts(&stored.counts, &boot)),
            resets: stored.map(|stored| stored.resets),
            handshake_max_ms: self.handshake_max_ms,
            handshake_mean_ms: u32::try_from(mean).unwrap_or(u32::MAX),
        }
    }

    /// The flash section, with the platform's event-ring figures and, when
    /// the stats store keeps them, the `stored` totals.
    pub fn flash(
        erases: proto::Erases,
        stored: Option<&Totals>,
        events_kept: u32,
        events_capacity: u32,
    ) -> proto::Flash {
        let lifetime = stored.map(|stored| {
            stored
                .plus(&Counts::default(), &erases.storage, erases.update)
                .erases()
        });
        proto::Flash {
            boot: erases,
            lifetime,
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

    /// The portions section: the credited portions, the owner's mark
    /// counts, and their weight by `portion_mg`, the calibration's.
    pub fn portions_stats(&self, marks: Option<Marks>, portion_mg: Option<u32>) -> Portions {
        let marks = marks.unwrap_or(Marks {
            phase: proto::Phase::Unknown,
            homed: 0,
            outlet_breaks: 0,
        });
        Portions {
            portions: self.portions,
            homed: marks.homed,
            outlet_breaks: marks.outlet_breaks,
            portion_mg,
            delivered_mg: portion_mg.map(|mg| u64::from(mg) * u64::from(self.portions)),
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
