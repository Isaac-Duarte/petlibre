//! The panel's lights in the loop: the feeder's states, each held as a
//! request on the SDK's indicator service
//! ([`wellfed_core::indicator`]), and the driver that puts the winners on
//! the pads.
//!
//! **Provisional mapping.** Which states show, and how, is still to be
//! settled against stock. Until then the loop shows enough to exercise the
//! service on the board ([indicators](../../../../../docs/firmware/indicators.md)):
//!
//! | State | Light | Pattern | Priority |
//! | --- | --- | --- | --- |
//! | the feeding owner holds a fault, or its journal failed | fault | steady | 90 |
//! | an install runs | connection | blink 150/150 ms | 80 |
//! | a Dispense press was refused | fault | blink 150/150 ms, 3 s | 70 |
//! | BLE setup mode runs | connection | blink 500/500 ms | 70 |
//! | an offer waits for a press or its code | lock | blink 250/250 ms | 70 |
//! | the battery holds feeds | fault | blink 1000/1000 ms | 60 |
//! | a feed is queued or runs | food | blink 500/500 ms | 50 |
//! | the last feed's food beam read low | food | steady | 10 |
//! | the panel is locked | lock | steady | 10 |
//! | the feeder has an address | connection | steady | 10 |
//!
//! The food beam is lit only during feeds, so low food shows the last reading:
//! it stays on until a feed reads food again, even after a refill, and shows
//! nothing until this boot's first feed has read.
//!
//! With the `lights` setting off, or outside its window on the local clock,
//! every light is dark, faults included, as stock drives them all LOW; the
//! requests still hold, so the lights come back as they were. The loop
//! decides that ([`States::lights`]) each pass, which comes at least every
//! 20 ms, so a window opens and closes within a pass of its minute.
//!
//! **Never in a feed's way.** A step is a few comparisons and at most one
//! masked port write per light that changes, made whatever the motor does
//! (the board's driver needs no motor lease and no stopped outputs), and a
//! check once the level has settled. A light that fails turns every light
//! off for the rest of the boot; feeding, the lock and the device API go on.

use core::fmt;

use wellfed_core::indicator::{
    Check, Claim, Drive, Failure, Handle, Indicators, Light, Lights as Board, Pattern, Priority,
    Refused, Request,
};

use super::platform::Platform;

wellfed_core::named_enum! {
    /// What a light shows, as its record names it.
    pub enum State {
        Fault = "fault",
        Updating = "updating",
        Refused = "refused",
        Setup = "setup",
        Pairing = "pairing",
        Battery = "battery",
        Feeding = "feeding",
        LowFood = "low-food",
        Locked = "locked",
        Online = "online",
    }
}

/// The states held while a condition lasts, in [`State::ALL`] order but
/// for [`State::Refused`], which ends by itself.
const HELD: [State; 9] = [
    State::Fault,
    State::Updating,
    State::Setup,
    State::Pairing,
    State::Battery,
    State::Feeding,
    State::LowFood,
    State::Locked,
    State::Online,
];

/// Every held state and the refused flash.
const SLOTS: usize = HELD.len() + 1;

/// How long a refused Dispense press flashes.
pub const REFUSED_MS: u32 = 3_000;

const fn blink(on_ms: u16, off_ms: u16) -> Pattern {
    Pattern::Blink { on_ms, off_ms }
}

impl State {
    /// The state's light, pattern and priority.
    pub const fn request(self) -> Request<Self> {
        let (light, pattern, priority) = match self {
            Self::Fault => (Light::Fault, Pattern::Steady, 90),
            Self::Updating => (Light::Connection, blink(150, 150), 80),
            Self::Refused => (Light::Fault, blink(150, 150), 70),
            Self::Setup => (Light::Connection, blink(500, 500), 70),
            Self::Pairing => (Light::Lock, blink(250, 250), 70),
            Self::Battery => (Light::Fault, blink(1_000, 1_000), 60),
            Self::Feeding => (Light::Food, blink(500, 500), 50),
            Self::LowFood => (Light::Food, Pattern::Steady, 10),
            Self::Locked => (Light::Lock, Pattern::Steady, 10),
            Self::Online => (Light::Connection, Pattern::Steady, 10),
        };
        Request {
            light,
            pattern,
            priority: Priority(priority),
            lasts_ms: match self {
                Self::Refused => Some(REFUSED_MS),
                _ => None,
            },
            state: self,
        }
    }
}

/// The feeder's states this pass, as the loop reads them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct States {
    pub fault: bool,
    pub updating: bool,
    pub setup: bool,
    pub pairing: bool,
    pub battery: bool,
    pub feeding: bool,
    pub low_food: bool,
    pub locked: bool,
    pub online: bool,
    /// The `lights` setting inside its window; true until the settings
    /// load, and while the clock is invalid.
    pub lights: bool,
}

impl States {
    const fn holds(&self, state: State) -> bool {
        match state {
            State::Fault => self.fault,
            State::Updating => self.updating,
            State::Setup => self.setup,
            State::Pairing => self.pairing,
            State::Battery => self.battery,
            State::Feeding => self.feeding,
            State::LowFood => self.low_food,
            State::Locked => self.locked,
            State::Online => self.online,
            State::Refused => false,
        }
    }
}

/// What the lights did, for `PLIND1` lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    /// `light` now shows `state`, or nothing.
    Shows { light: Light, state: Option<State> },
    /// The service refused a state's request (a bug: the table is sized
    /// for every state).
    Refused { state: State, refused: Refused },
    /// A light failed and every light is off for the rest of the boot.
    Failed(Failure),
}

pub fn describe(record: Record, f: &mut impl fmt::Write) -> fmt::Result {
    match record {
        Record::Shows { light, state } => match state {
            Some(state) => write!(f, "show light={light} state={state}"),
            None => write!(f, "show light={light} state=none"),
        },
        Record::Refused { state, refused } => {
            let refused = match refused {
                Refused::Full => "full",
                Refused::Pattern => "pattern",
                Refused::Duration => "duration",
            };
            write!(f, "refused state={state} reason={refused}")
        }
        Record::Failed(Failure { light, check }) => {
            let check = match check {
                None => "refused",
                Some(Check::Held) => "verified",
                Some(Check::InputMismatch) => "input-mismatch",
                Some(Check::Abort) => "abort",
            };
            write!(f, "fail light={light} check={check}")
        }
    }
}

/// The platform's indicator calls, as the driver reaches them.
struct Pads<'a, P>(&'a mut P);

impl<P: Platform> Board for Pads<'_, P> {
    const SETTLE_MS: u32 = P::INDICATOR_SETTLE_MS;

    fn set(&mut self, light: Light, on: bool) -> bool {
        self.0.indicator(light, on)
    }

    fn check(&mut self, light: Light, on: bool) -> Check {
        self.0.indicator_check(light, on)
    }

    fn all_off(&mut self) {
        self.0.indicators_off();
    }
}

pub struct Lights {
    table: Indicators<State, SLOTS>,
    claims: [Claim<State>; HELD.len()],
    /// The refused flash, while it may still hold.
    flash: Option<Handle>,
    drive: Drive,
    /// What each light last showed, for the records.
    shown: [Option<State>; Light::COUNT],
}

impl Default for Lights {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Lights {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Lights")
            .field("shown", &self.shown)
            .field("failed", &self.drive.failed())
            .finish_non_exhaustive()
    }
}

impl Lights {
    pub const fn new() -> Self {
        Self {
            table: Indicators::new(),
            claims: [const { Claim::new() }; HELD.len()],
            flash: None,
            drive: Drive::new(),
            shown: [None; Light::COUNT],
        }
    }

    /// A Dispense press was refused at `now_ms`: flash it for
    /// [`REFUSED_MS`].
    pub fn refused(&mut self, now_ms: u32, mut record: impl FnMut(Record)) {
        // A new press restarts the flash rather than stacking another.
        if let Some(handle) = self.flash.take() {
            self.table.cancel(handle);
        }
        match self.table.show(now_ms, State::Refused.request()) {
            Ok(handle) => self.flash = Some(handle),
            Err(refused) => record(Record::Refused {
                state: State::Refused,
                refused,
            }),
        }
    }

    /// One step at `now_ms` with the feeder's `states`; the delay until the
    /// next change.
    pub fn step<P: Platform>(
        &mut self,
        platform: &mut P,
        now_ms: u32,
        states: States,
        mut record: impl FnMut(Record),
    ) -> u32 {
        for (claim, state) in self.claims.iter_mut().zip(HELD) {
            let want = states.holds(state).then(|| state.request());
            if let Err(refused) = claim.set(&mut self.table, now_ms, want) {
                record(Record::Refused { state, refused });
            }
        }

        let frame = self.table.frame(now_ms);
        for light in Light::ALL {
            let state = self.table.winner(*light).map(|request| request.state);
            let shown = &mut self.shown[light.index()];
            if *shown != state {
                *shown = state;
                record(Record::Shows {
                    light: *light,
                    state,
                });
            }
        }

        let levels = if states.lights {
            frame.levels
        } else {
            [false; Light::COUNT]
        };
        let step = self.drive.step(&mut Pads(platform), now_ms, levels);
        if let Some(failure) = step.failure {
            record(Record::Failed(failure));
        }
        if self.drive.failed() {
            return u32::MAX;
        }
        frame.next_ms.min(step.delay_ms)
    }
}
