//! Portion counting and direction alternation for one request.
//!
//! In DOUBLE_BOWL mode a portion is credited on every second qualified
//! feedback event, or on the next one when `credit_next_event` is set, and
//! the logical direction toggles with each portion. SINGLE_BOWL credits every
//! event.

c_enum!(BowlMode { SINGLE = 0, DOUBLE = 1 });
c_enum!(ProgressResult { WAIT = 0, PORTION = 1, FINISHED = 2 });
c_enum!(
    /// Logical direction only; no physical bowl/clockwise assignment is implied.
    Direction { D0 = 0, D1 = 1 }
);

impl BowlMode {
    pub fn valid(self) -> bool {
        self == Self::SINGLE || self == Self::DOUBLE
    }
}

impl Direction {
    pub fn valid(self) -> bool {
        self == Self::D0 || self == Self::D1
    }

    pub fn toggled(self) -> Self {
        if self == Self::D0 { Self::D1 } else { Self::D0 }
    }
}

/// Own one instance per request; modify it only through these methods.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FeedProgress {
    pub requested: u16,
    pub completed: u16,
    pub mode: BowlMode,
    pub direction: Direction,
    pub pending_events: u8,
    pub credit_next_event: bool,
    pub finished: bool,
}

impl FeedProgress {
    /// Initialize a NEW request, after the caller has stopped any prior request.
    /// Invalid input yields `None`. Product portion limits, request identity,
    /// faults and motor ownership belong to the controller.
    ///
    /// Stock DOUBLE_BOWL starts with `credit_next_event` after boot and normal
    /// completion. After an interrupted movement its physical phase is
    /// unverified; the caller must establish that phase rather than assume a
    /// fresh cycle.
    pub fn new(
        mode: BowlMode,
        direction: Direction,
        portions: u16,
        credit_next_event: bool,
    ) -> Option<Self> {
        if portions == 0 || !mode.valid() || !direction.valid() {
            return None;
        }
        Some(Self {
            requested: portions,
            mode,
            direction,
            credit_next_event,
            ..Self::default()
        })
    }

    /// Consume one QUALIFIED feedback event, not an arbitrary GPIO edge.
    /// PORTION changes direction only in DOUBLE_BOWL mode. FINISHED means the
    /// caller must stop; the stored direction is the value to persist, not a
    /// drive command. Later calls remain FINISHED without crediting portions.
    pub fn accept(&mut self) -> ProgressResult {
        if self.finished {
            return ProgressResult::FINISHED;
        }
        if self.mode == BowlMode::DOUBLE {
            self.pending_events = self.pending_events.wrapping_add(1);
            if !self.credit_next_event && self.pending_events < 2 {
                return ProgressResult::WAIT;
            }
            self.pending_events = 0;
            self.credit_next_event = false;
            self.direction = self.direction.toggled();
        }

        self.completed = self.completed.wrapping_add(1);
        if self.completed == self.requested {
            self.finished = true;
            if self.mode == BowlMode::DOUBLE {
                self.credit_next_event = true;
                self.direction = self.direction.toggled();
            }
            return ProgressResult::FINISHED;
        }
        ProgressResult::PORTION
    }
}
