//! Scheduling gaps (the `gap-probe` variant): how long a task waited past
//! the delay it asked for, measured by the task itself on the 32 kHz system
//! timer, so a handler that masks the FreeRTOS tick cannot hide its hold.
//!
//! The feeding owner is stepped from the priority-2 loop and beats only
//! when the loop runs, and the Wi-Fi driver's WPA3 handshake has held that
//! loop past the beat's gap. Whether a task at priority 4 would have been
//! held as long decides where the owner runs, so the variant measures both
//! side by side: a probe task at priority 4 that wakes every
//! [`PERIOD_MS`] and does nothing else, and the loop itself. Every
//! [`WINDOW_MS`] the loop logs one record for each, and one with the
//! Wi-Fi counters that mark a rejoin.
//!
//! A [`Probe`] is all atomics: its task records into it, and the loop takes
//! its window from another task. Each of a sample's counts lands in exactly
//! one window, but a sample that lands while the loop takes the window may
//! split: its bucket in one window, its wake and maximum in the next.

use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use wellfed_core::named_enum;
use wellfed_sdk::hal::net_status::Status;
use wellfed_sdk::hal::supervision::config::SYSTIMER_HZ;

/// The priority-4 probe's wake period: the supervision timer's, and well
/// under the owner's 100 ms observation gap.
pub const PERIOD_MS: u32 = 10;
/// Between records: several windows to a rejoin, and few enough lines that
/// logging, which holds the scheduler while UART drains, stays a small
/// share of what the probe sees.
pub const WINDOW_MS: u32 = 10_000;
/// Lateness thresholds counted per window: the owner's observation gap
/// (rounded down), a stall the panel would show, and the beat's gap while
/// outputs may move.
pub const OVER_MS: [u32; 3] = [50, 200, 1000];

/// Lateness below this has a bucket of its own millisecond.
const EXACT: u32 = 32;
/// One bucket a millisecond below [`EXACT`], then one a power of two up to
/// `u32::MAX`.
const BUCKETS: usize = EXACT as usize + (u32::BITS - EXACT.ilog2()) as usize;

named_enum! {
    /// Which task a record measures.
    pub enum Task {
        /// The priority-2 loop, which steps the owner.
        Loop = "loop",
        /// The priority-4 probe task.
        High = "p4",
    }
}

/// One task's scheduling, recorded from its own wakes.
pub struct Probe {
    /// The delay the task asked for before its next wake.
    asked_ms: AtomicU32,
    /// System timer ticks at the last wake; meaningful once `seen`.
    last: AtomicU32,
    seen: AtomicBool,
    wakes: AtomicU32,
    max_gap_ms: AtomicU32,
    max_late_ms: AtomicU32,
    over: [AtomicU32; OVER_MS.len()],
    buckets: [AtomicU32; BUCKETS],
}

/// A probe's window, as [`Probe::take`] ends it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Window {
    /// Gaps measured (wakes after the first).
    pub wakes: u32,
    /// The longest time between two wakes, the asked delay included.
    pub max_gap_ms: u32,
    /// The longest time past the asked delay.
    pub max_late_ms: u32,
    /// The 99th percentile of lateness, rounded up to its bucket's top.
    pub p99_late_ms: u32,
    /// Wakes later than each of [`OVER_MS`].
    pub over: [u32; OVER_MS.len()],
}

impl Probe {
    /// A probe whose task first asks for `asked_ms`.
    pub const fn new(asked_ms: u32) -> Self {
        Self {
            asked_ms: AtomicU32::new(asked_ms),
            last: AtomicU32::new(0),
            seen: AtomicBool::new(false),
            wakes: AtomicU32::new(0),
            max_gap_ms: AtomicU32::new(0),
            max_late_ms: AtomicU32::new(0),
            over: [const { AtomicU32::new(0) }; OVER_MS.len()],
            buckets: [const { AtomicU32::new(0) }; BUCKETS],
        }
    }

    /// The task woke at `now` (system timer ticks). Only the probed task
    /// calls it, so the gap is between its own wakes; the first wake only
    /// starts the clock.
    pub fn wake(&self, now: u32) {
        let last = self.last.swap(now, Ordering::Relaxed);
        if !self.seen.swap(true, Ordering::Relaxed) {
            return;
        }

        let gap_ms = ticks_ms(now.wrapping_sub(last));
        let late_ms = gap_ms.saturating_sub(self.asked_ms.load(Ordering::Relaxed));
        self.wakes.fetch_add(1, Ordering::Relaxed);
        self.max_gap_ms.fetch_max(gap_ms, Ordering::Relaxed);
        self.max_late_ms.fetch_max(late_ms, Ordering::Relaxed);
        for (count, &limit) in self.over.iter().zip(OVER_MS.iter()) {
            if late_ms > limit {
                count.fetch_add(1, Ordering::Relaxed);
            }
        }
        if let Some(count) = self.buckets.get(bucket(late_ms)) {
            count.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The delay the task asks for before its next wake. Only the probed
    /// task calls it, between wakes.
    pub fn asked(&self, delay_ms: u32) {
        self.asked_ms.store(delay_ms, Ordering::Relaxed);
    }

    /// Ends the window: what it held, with the counts cleared for the next.
    pub fn take(&self) -> Window {
        let mut counts = [0; BUCKETS];
        for (count, bucket) in counts.iter_mut().zip(self.buckets.iter()) {
            *count = bucket.swap(0, Ordering::Relaxed);
        }

        let mut over = [0; OVER_MS.len()];
        for (count, total) in over.iter_mut().zip(self.over.iter()) {
            *count = total.swap(0, Ordering::Relaxed);
        }
        Window {
            wakes: self.wakes.swap(0, Ordering::Relaxed),
            max_gap_ms: self.max_gap_ms.swap(0, Ordering::Relaxed),
            max_late_ms: self.max_late_ms.swap(0, Ordering::Relaxed),
            p99_late_ms: p99(&counts),
            over,
        }
    }
}

fn ticks_ms(ticks: u32) -> u32 {
    // At most 2^32 * 1000 / 32768, so it fits.
    u32::try_from(u64::from(ticks) * 1000 / u64::from(SYSTIMER_HZ)).unwrap_or(u32::MAX)
}

fn bucket(late_ms: u32) -> usize {
    let index = if late_ms < EXACT {
        late_ms
    } else {
        EXACT + late_ms.ilog2() - EXACT.ilog2()
    };
    usize::try_from(index).unwrap_or(BUCKETS - 1)
}

/// The largest lateness bucket `index` holds.
fn ceiling(index: usize) -> u32 {
    let index = u32::try_from(index).unwrap_or(u32::MAX);
    if index < EXACT {
        return index;
    }
    let bits = index - EXACT + EXACT.ilog2() + 1;
    1u32.checked_shl(bits).map_or(u32::MAX, |top| top - 1)
}

fn p99(counts: &[u32; BUCKETS]) -> u32 {
    let total: u64 = counts.iter().map(|&n| u64::from(n)).sum();
    let mut seen = 0u64;
    for (index, &count) in counts.iter().enumerate() {
        seen += u64::from(count);
        if count > 0 && seen * 100 >= total * 99 {
            return ceiling(index);
        }
    }
    0
}

/// The Wi-Fi counters at a window's end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Net {
    /// Bumped by every join and every loss.
    pub generation: u32,
    /// The join attempt the last scan belonged to.
    pub attempt: u32,
    pub link_losses: u32,
    pub dns_losses: u32,
    /// The link was lost or joined during the window.
    pub changed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    Task {
        task: Task,
        now_ms: u32,
        /// How long the window ran, by the loop's clock.
        span_ms: u32,
        window: Window,
        /// The longest gap since boot.
        worst_ms: u32,
    },
    Net {
        now_ms: u32,
        net: Net,
    },
}

/// Formats a record as the `PLGAP1` line's body. Each line stays under the
/// 192-byte record buffer whatever its values.
impl fmt::Display for Record {
    // Inline, so only the image that logs these records compiles them: the
    // linker keeps a string section whole, so an unused formatter's strings
    // would stay in every image.
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Record::Task {
                task,
                now_ms,
                span_ms,
                window: w,
                worst_ms,
            } => write!(
                f,
                "{task} t={now_ms} w={span_ms} n={} max={} late={} p99={} over={}/{}/{} worst={worst_ms}",
                w.wakes,
                w.max_gap_ms,
                w.max_late_ms,
                w.p99_late_ms,
                w.over[0],
                w.over[1],
                w.over[2],
            ),
            Record::Net { now_ms, net } => write!(
                f,
                "net t={now_ms} gen={} att={} lost={} dns={} rejoin={}",
                net.generation,
                net.attempt,
                net.link_losses,
                net.dns_losses,
                u8::from(net.changed),
            ),
        }
    }
}

/// The loop's side: closes a window every [`WINDOW_MS`] and reports both
/// probes and the Wi-Fi counters.
#[derive(Debug, Default)]
pub struct Windows {
    /// The open window's start, once the first pass ran.
    start_ms: Option<u32>,
    worst_loop_ms: u32,
    worst_high_ms: u32,
    generation: u32,
}

impl Windows {
    pub const fn new() -> Self {
        Self {
            start_ms: None,
            worst_loop_ms: 0,
            worst_high_ms: 0,
            generation: 0,
        }
    }

    /// At the end of a loop pass: once a window has run, its three records.
    pub fn step(
        &mut self,
        now_ms: u32,
        looped: &Probe,
        high: &Probe,
        net: Status,
        mut record: impl FnMut(Record),
    ) {
        let Some(start_ms) = self.start_ms else {
            self.start_ms = Some(now_ms);
            self.generation = net.generation;
            return;
        };
        let span_ms = now_ms.wrapping_sub(start_ms);
        if span_ms < WINDOW_MS {
            return;
        }
        self.start_ms = Some(now_ms);

        for (task, probe, worst_ms) in [
            (Task::High, high, &mut self.worst_high_ms),
            (Task::Loop, looped, &mut self.worst_loop_ms),
        ] {
            let window = probe.take();
            *worst_ms = (*worst_ms).max(window.max_gap_ms);
            record(Record::Task {
                task,
                now_ms,
                span_ms,
                window,
                worst_ms: *worst_ms,
            });
        }

        let changed = net.generation != self.generation;
        self.generation = net.generation;
        record(Record::Net {
            now_ms,
            net: Net {
                generation: net.generation,
                attempt: net.attempt,
                link_losses: net.link_losses,
                dns_losses: net.check_losses,
                changed,
            },
        });
    }
}
