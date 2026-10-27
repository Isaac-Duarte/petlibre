//! The indicator service: which level each panel light shows, decided from
//! the requests that want it, and the driver that puts those levels on the
//! pads and checks them.
//!
//! **Requests.** A firmware asks for a light with a [`Request`]: the light,
//! a [`Pattern`] (off, steady, blink), a [`Priority`] and, optionally, how
//! long it lasts. [`Indicators::show`] gives back a [`Handle`]; the request
//! holds until [`Indicators::cancel`] takes that handle or its time runs
//! out. Per light the highest priority wins, and between equal priorities
//! the newer request; the others wait underneath, so a cancelled or expired
//! winner reveals the next one. A [`Pattern::Off`] request wins like any
//! other, which lets a firmware hold a light dark over lower requests.
//!
//! **No allocation, no task.** The table is a fixed array of `SLOTS`
//! entries and a full table refuses the request. The service keeps no clock
//! of its own: the loop that owns the panel calls [`Indicators::frame`] with
//! its time, gets each light's level and the delay until the next change (a
//! blink edge or an expiry), and hands the levels to [`Drive`]. Nothing here
//! blocks or waits, so a loop with a feed in flight pays a few comparisons.
//!
//! **Handles.** Each request gets a sequence number that never repeats in a
//! boot (64 bits), so a handle kept past its request's end cancels nothing,
//! even after its slot is reused.
//!
//! **Blinks** start lit at the request's start and are re-anchored at whole
//! periods, so a pattern that runs for weeks keeps its phase across the
//! millisecond clock's wrap. Each phase is at least [`MIN_PHASE_MS`], longer
//! than a level's settle and check ([`Lights::SETTLE_MS`]).
//!
//! **The driver** ([`Drive`]) writes a light only when its level changes,
//! lets the level settle, then checks it. A refused write or a level that
//! does not hold turns every light off and the driver stops for the rest of
//! the boot: a light that cannot be trusted shows nothing rather than a
//! wrong state. Which states show, and what the lights setting hides, are
//! the firmware's policy.

use crate::{deadline_passed, interval_valid, until};

crate::named_enum! {
    /// The panel's lights, one per icon. Each is a single-color LED on its
    /// own pad; which pad is the board's.
    pub enum Light {
        /// The Wi-Fi icon.
        Connection = "connection",
        /// The low-food icon.
        Food = "food",
        /// The lock icon.
        Lock = "lock",
        /// The error/warning icon.
        Fault = "fault",
    }
}

impl Light {
    /// How many lights the panel has.
    pub const COUNT: usize = 4;

    /// The light's place in a [`Levels`] array and the record's number.
    pub const fn index(self) -> usize {
        match self {
            Self::Connection => 0,
            Self::Food => 1,
            Self::Lock => 2,
            Self::Fault => 3,
        }
    }
}

/// Each light's level, indexed by [`Light::index`]: true lit.
pub type Levels = [bool; Light::COUNT];

/// The shortest blink phase: longer than a level's settle and check, so a
/// blink never outruns its own verification.
pub const MIN_PHASE_MS: u16 = 50;

/// How a light shows while its request wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pattern {
    /// Dark, over any lower request.
    Off,
    /// Lit.
    Steady,
    /// Lit `on_ms`, then dark `off_ms`, starting lit.
    Blink { on_ms: u16, off_ms: u16 },
}

impl Pattern {
    /// Whether the pattern can be shown: every blink phase at least
    /// [`MIN_PHASE_MS`].
    pub const fn valid(self) -> bool {
        match self {
            Self::Off | Self::Steady => true,
            Self::Blink { on_ms, off_ms } => on_ms >= MIN_PHASE_MS && off_ms >= MIN_PHASE_MS,
        }
    }

    /// The level `elapsed` milliseconds into the pattern (`elapsed` within
    /// one period for a blink), and the milliseconds until it changes:
    /// `None` for a level that never changes.
    const fn level(self, elapsed: u32) -> (bool, Option<u32>) {
        match self {
            Self::Off => (false, None),
            Self::Steady => (true, None),
            Self::Blink { on_ms, off_ms } => {
                let on = on_ms as u32;
                if elapsed < on {
                    (true, Some(on - elapsed))
                } else {
                    (false, Some(on + off_ms as u32 - elapsed))
                }
            }
        }
    }

    /// A blink's period; 0 for a level that never changes.
    const fn period(self) -> u32 {
        match self {
            Self::Off | Self::Steady => 0,
            Self::Blink { on_ms, off_ms } => on_ms as u32 + off_ms as u32,
        }
    }
}

/// A request's rank on its light: higher wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Priority(pub u8);

/// One light shown one way, for as long as the requester wants it, or for
/// `lasts_ms` from its start. `state` is the requester's own label, handed
/// back with the winner so a firmware can say what a light shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request<S> {
    pub light: Light,
    pub pattern: Pattern,
    pub priority: Priority,
    /// `None` until cancelled.
    pub lasts_ms: Option<u32>,
    pub state: S,
}

/// Why a request was not taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// Every slot holds a request.
    Full,
    /// A blink phase shorter than [`MIN_PHASE_MS`].
    Pattern,
    /// A duration of 0, or one too long for the wrapping clock.
    Duration,
}

/// A shown request, to cancel it. Stale once its request has ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handle {
    slot: u8,
    sequence: u64,
}

#[derive(Clone, Copy, Debug)]
struct Entry<S> {
    request: Request<S>,
    sequence: u64,
    /// Where the pattern's current period began.
    anchor_ms: u32,
    /// When the request ends, if it does by itself.
    until_ms: Option<u32>,
}

impl<S> Entry<S> {
    /// Ranks above `other` on the same light: higher priority, then newer.
    fn above(&self, other: &Self) -> bool {
        (self.request.priority, self.sequence) > (other.request.priority, other.sequence)
    }
}

/// What the lights show at one moment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub levels: Levels,
    /// Milliseconds until a level may change (a blink edge or an expiry);
    /// `u32::MAX` while nothing will change by itself.
    pub next_ms: u32,
}

/// The request table: up to `SLOTS` requests across the panel's lights.
#[derive(Clone, Copy, Debug)]
pub struct Indicators<S, const SLOTS: usize> {
    entries: [Option<Entry<S>>; SLOTS],
    next_sequence: u64,
}

impl<S: Copy, const SLOTS: usize> Default for Indicators<S, SLOTS> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: Copy, const SLOTS: usize> Indicators<S, SLOTS> {
    /// An empty table; const, so a firmware builds it in its static.
    pub const fn new() -> Self {
        // A slot index must fit the handle's byte.
        const { assert!(SLOTS > 0 && SLOTS <= 255) };
        Self {
            entries: [None; SLOTS],
            next_sequence: 0,
        }
    }

    /// Take `request` from `now_ms`.
    pub fn show(&mut self, now_ms: u32, request: Request<S>) -> Result<Handle, Refused> {
        if !request.pattern.valid() {
            return Err(Refused::Pattern);
        }
        if request.lasts_ms.is_some_and(|ms| !interval_valid(ms)) {
            return Err(Refused::Duration);
        }
        let Some((index, slot)) = self
            .entries
            .iter_mut()
            .enumerate()
            .find(|(_, entry)| entry.is_none())
        else {
            return Err(Refused::Full);
        };
        let Ok(slot_byte) = u8::try_from(index) else {
            return Err(Refused::Full);
        };

        let sequence = self.next_sequence;
        // Wrapping only so no build can panic here: at one request per
        // millisecond, 64 bits last 584 million years, far past any boot.
        self.next_sequence = self.next_sequence.wrapping_add(1);
        *slot = Some(Entry {
            request,
            sequence,
            anchor_ms: now_ms,
            until_ms: request.lasts_ms.map(|ms| now_ms.wrapping_add(ms)),
        });
        Ok(Handle {
            slot: slot_byte,
            sequence,
        })
    }

    /// End the request behind `handle`; false when it has already ended.
    pub fn cancel(&mut self, handle: Handle) -> bool {
        let Some(slot) = self.entries.get_mut(usize::from(handle.slot)) else {
            return false;
        };
        if slot.is_some_and(|entry| entry.sequence == handle.sequence) {
            *slot = None;
            return true;
        }
        false
    }

    /// Whether the request behind `handle` still holds (it may be hidden
    /// under a higher one). Expiry is found by [`Self::frame`].
    pub fn holds(&self, handle: Handle) -> bool {
        self.entries
            .get(usize::from(handle.slot))
            .is_some_and(|slot| slot.is_some_and(|entry| entry.sequence == handle.sequence))
    }

    /// The request `light` shows now, if any. Current as of the last
    /// [`Self::frame`]: an expired request ends there.
    pub fn winner(&self, light: Light) -> Option<&Request<S>> {
        let index = self.top(light)?;
        self.entries[index].as_ref().map(|entry| &entry.request)
    }

    /// The slot of `light`'s winning request.
    fn top(&self, light: Light) -> Option<usize> {
        let mut best: Option<(usize, &Entry<S>)> = None;
        for (index, slot) in self.entries.iter().enumerate() {
            let Some(entry) = slot.as_ref().filter(|e| e.request.light == light) else {
                continue;
            };
            if best.is_none_or(|(_, top)| entry.above(top)) {
                best = Some((index, entry));
            }
        }
        best.map(|(index, _)| index)
    }

    /// End what has expired by `now_ms`, then each light's level and the
    /// delay until the next change. Call it each pass; a blink is
    /// re-anchored here, so the gap between calls must stay under half the
    /// clock's range.
    pub fn frame(&mut self, now_ms: u32) -> Frame {
        let mut next_ms = u32::MAX;
        for slot in &mut self.entries {
            let Some(entry) = slot else { continue };
            match entry.until_ms {
                Some(until_ms) if deadline_passed(now_ms, until_ms) => *slot = None,
                Some(until_ms) => next_ms = next_ms.min(until(now_ms, until_ms)),
                None => {}
            }
        }

        let mut levels = [false; Light::COUNT];
        for light in Light::ALL {
            let Some(entry) = self
                .top(*light)
                .and_then(|index| self.entries[index].as_mut())
            else {
                continue;
            };
            let (level, change) = phase(entry, now_ms);
            levels[light.index()] = level;
            if let Some(change) = change {
                next_ms = next_ms.min(change);
            }
        }
        Frame { levels, next_ms }
    }
}

/// The entry's level at `now_ms`, re-anchoring a blink at its last whole
/// period, and the milliseconds until the level changes.
fn phase<S>(entry: &mut Entry<S>, now_ms: u32) -> (bool, Option<u32>) {
    let period = entry.request.pattern.period();
    let mut elapsed = now_ms.wrapping_sub(entry.anchor_ms);
    if period > 0 && elapsed >= period {
        let whole = elapsed - elapsed % period;
        entry.anchor_ms = entry.anchor_ms.wrapping_add(whole);
        elapsed -= whole;
    }
    entry.request.pattern.level(elapsed)
}

/// One labelled request a firmware holds while a condition does: shown when
/// the condition starts, cancelled when it ends, replaced when what it
/// wants changes. Most panel states are held this way; a request that ends
/// by itself is shown once with [`Indicators::show`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Claim<S> {
    shown: Option<(Handle, Request<S>)>,
}

impl<S> Default for Claim<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Claim<S> {
    pub const fn new() -> Self {
        Self { shown: None }
    }
}

impl<S: Copy + PartialEq> Claim<S> {
    /// Hold `want` on `indicators` from `now_ms`, or nothing when `None`.
    /// An unchanged request keeps its handle and its blink phase.
    pub fn set<const SLOTS: usize>(
        &mut self,
        indicators: &mut Indicators<S, SLOTS>,
        now_ms: u32,
        want: Option<Request<S>>,
    ) -> Result<(), Refused> {
        if let Some((handle, shown)) = self.shown {
            if want == Some(shown) && indicators.holds(handle) {
                return Ok(());
            }
            indicators.cancel(handle);
            self.shown = None;
        }
        if let Some(request) = want {
            let handle = indicators.show(now_ms, request)?;
            self.shown = Some((handle, request));
        }
        Ok(())
    }
}

/// How a settled level read back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    /// The level holds (read back, or consistent where the pad has no
    /// input readback).
    Held,
    /// The pad's input disagrees with its latch.
    InputMismatch,
    /// The latch, the pad or the pin's direction disagrees.
    Abort,
}

/// The board's lights, as [`Drive`] reaches them.
pub trait Lights {
    /// How long a requested level settles before it reads back.
    const SETTLE_MS: u32;
    /// Drive `light`; false when the request is refused.
    fn set(&mut self, light: Light, on: bool) -> bool;
    /// How `light`'s settled level reads back against `on`.
    fn check(&mut self, light: Light, on: bool) -> Check;
    /// Drive every light off, for good.
    fn all_off(&mut self);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    /// Holding a checked level.
    Shown(bool),
    /// A level was written at `since_ms` and is settling.
    Settling { high: bool, since_ms: u32 },
}

/// Why the driver stopped: the light, and how its level read back (`None`
/// when the write was refused).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Failure {
    pub light: Light,
    pub check: Option<Check>,
}

/// One driver step: the delay it asks for, and its failure, once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub delay_ms: u32,
    pub failure: Option<Failure>,
}

/// Puts [`Levels`] on the board: a write per change, then a check once the
/// level has settled. Every light starts off, as the runtime leaves them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Drive {
    stages: [Stage; Light::COUNT],
    failed: bool,
}

impl Default for Drive {
    fn default() -> Self {
        Self::new()
    }
}

impl Drive {
    pub const fn new() -> Self {
        Self {
            stages: [Stage::Shown(false); Light::COUNT],
            failed: false,
        }
    }

    /// Whether a failure has stopped the driver for the boot.
    pub const fn failed(&self) -> bool {
        self.failed
    }

    /// Move each light one stage towards `want` at `now_ms`.
    pub fn step<L: Lights>(&mut self, lights: &mut L, now_ms: u32, want: Levels) -> Step {
        let mut delay_ms = u32::MAX;
        if self.failed {
            return Step {
                delay_ms,
                failure: None,
            };
        }

        for light in Light::ALL {
            let index = light.index();
            let target = want[index];
            if let Stage::Settling { high, since_ms } = self.stages[index] {
                let waited = now_ms.wrapping_sub(since_ms);
                if waited < L::SETTLE_MS {
                    delay_ms = delay_ms.min(L::SETTLE_MS - waited);
                    continue;
                }
                let check = lights.check(*light, high);
                if check != Check::Held {
                    return self.fail(lights, *light, Some(check));
                }
                self.stages[index] = Stage::Shown(high);
            }

            // A level that changed while the last one settled is written
            // now, behind its check.
            if self.stages[index] == Stage::Shown(!target) {
                if !lights.set(*light, target) {
                    return self.fail(lights, *light, None);
                }
                self.stages[index] = Stage::Settling {
                    high: target,
                    since_ms: now_ms,
                };
                delay_ms = delay_ms.min(L::SETTLE_MS);
            }
        }
        Step {
            delay_ms,
            failure: None,
        }
    }

    fn fail<L: Lights>(&mut self, lights: &mut L, light: Light, check: Option<Check>) -> Step {
        lights.all_off();
        self.failed = true;
        Step {
            delay_ms: u32::MAX,
            failure: Some(Failure { light, check }),
        }
    }
}
