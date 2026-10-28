//! The feeding plans on NOR (storage sectors 11 and 12): the plan in each
//! slot, each slot's mark, and the occurrence the loop last took.
//!
//! A 128-byte record in the dual-slot store (`wellfed_core::store::slot`),
//! little-endian:
//!
//! | Bytes | Field |
//! | --- | --- |
//! | 0..12 | magic `PLFPLN01`, version, record size |
//! | 12..16 | sequence: the save count, which picks the newest record |
//! | 16..20 | revision: bumped by every change to the table |
//! | 20..70 | ten plans of 5 bytes: flags (bit 0 present, bit 1 enabled), minute (u16), portions, weekdays |
//! | 70..110 | ten marks (Unix seconds, u32; 0 for none) |
//! | 110..120 | the claim: flags (bit 0 present, bit 1 it disabled a plan that runs once), slot, due (Unix seconds, u32), the journal's high water (u32) |
//! | 120..128 | CRC-32/ISO-HDLC of bytes 0..120, and the `CMIT` commit marker |
//!
//! An empty slot and an absent claim are all zero, and a plan is stored
//! only if it is valid, so one encoding exists per value.
//!
//! **Marks** make each occurrence run at most once. A mark is written when
//! the loop takes an occurrence (the claim), passes one it will never run,
//! or a client saves the plan; the loop never runs an occurrence at or
//! before it, whatever the clock does after.
//!
//! The two sectors were reserved for plans before this store, so a record
//! here is ours; the newest valid one wins, and a torn newer slot falls
//! back to the older. With none the table starts empty and writable. Only
//! a newer schema version makes the store read-only, with no plans, so
//! nothing runs that this build cannot read.

use core::ffi::c_uint;

use wellfed_core::store::slot::{
    CRC_AT, DualSlot, Extent, Layout, Pick, Prepare, RECORD_BYTES, SlotIo, get_u32, newest,
    put_u32, seal, target,
};
use wellfed_device_api::mailbox::{Claim, PLANS, Plan, PlansChange, PlansView, Table, Weekdays};

pub const VERSION: u16 = 1;
const LAYOUT: Layout = Layout {
    magic: *b"PLFPLN01",
    version: VERSION,
    extent: Extent::Record,
};
const SEQUENCE_AT: usize = 12;
const REVISION_AT: usize = 16;
const PLANS_AT: usize = 20;
const PLAN_BYTES: usize = 5;
const MARKS_AT: usize = PLANS_AT + PLANS * PLAN_BYTES;
const CLAIM_AT: usize = MARKS_AT + PLANS * 4;
const CLAIM_END: usize = CLAIM_AT + 10;

const PRESENT: u8 = 1;
const ENABLED: u8 = 2;
const DISABLED: u8 = 2;

const _: () = assert!(CLAIM_END <= CRC_AT);

/// What the store holds: the table and what the loop keeps beside it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Plans {
    pub table: Table,
    pub marks: [u32; PLANS],
    pub claim: Option<Claim>,
    pub revision: u32,
}

impl Plans {
    /// The view the writer publishes.
    pub const fn view(&self, writable: bool) -> PlansView {
        PlansView {
            table: self.table,
            marks: self.marks,
            claim: self.claim,
            revision: self.revision,
            replaced: 0,
            writable,
        }
    }

    /// The plans after `change`; `None` when it is dropped (the loop's,
    /// taken against a table or a claim that has moved on, or naming no
    /// plan). Shared by the writer and the simulator.
    pub fn changed(&self, change: &PlansChange) -> Option<Self> {
        let mut next = *self;
        match *change {
            PlansChange::Replace { table, at_s } => {
                for (slot, (new, old)) in table.iter().zip(&self.table).enumerate() {
                    next.marks[slot] = match (new, old) {
                        (None, _) => 0,
                        // Re-enabling starts the plan afresh, as a new one:
                        // its occurrences while it was off never run.
                        (Some(new), Some(old))
                            if new.same_times(old) && (old.enabled || !new.enabled) =>
                        {
                            self.marks[slot]
                        }
                        (Some(_), _) => at_s.unwrap_or(0),
                    };
                }
                next.table = table;
                next.revision = self.revision.wrapping_add(1);
            }
            PlansChange::Claim { claim, revision } => {
                let plan = self.plan(claim.plan, revision)?;
                if let Some(old) = self.claim {
                    next.fold(old);
                }
                let disabled = plan.days.once();
                if disabled {
                    next.set_enabled(claim.plan, false);
                }
                next.claim = Some(Claim { disabled, ..claim });
            }
            PlansChange::Release { claim } => {
                if self.claim != Some(claim) {
                    return None;
                }
                next.claim = None;
                if claim.disabled {
                    next.set_enabled(claim.plan, true);
                }
            }
            PlansChange::Settle { claim } => {
                if self.claim != Some(claim) {
                    return None;
                }
                next.fold(claim);
                next.claim = None;
            }
            PlansChange::Pass {
                plan,
                due_s,
                revision,
            } => {
                let found = self.plan(plan, revision)?;
                // A plan saved without a clock never expected the
                // occurrence passed before the clock returned: one that
                // runs once keeps waiting for its next.
                let expected = self.marked(plan);
                let mark = next.marks.get_mut(usize::from(plan))?;
                *mark = (*mark).max(due_s);
                if found.days.once() && expected {
                    next.set_enabled(plan, false);
                }
            }
        }
        Some(next)
    }

    /// The enabled plan in `slot`, if the table is still at `revision`.
    fn plan(&self, slot: u8, revision: u32) -> Option<Plan> {
        let plan = (*self.table.get(usize::from(slot))?)?;
        (revision == self.revision && plan.enabled).then_some(plan)
    }

    /// Whether slot `slot` has a mark, the claim's occurrence included.
    fn marked(&self, slot: u8) -> bool {
        let claimed = self.claim.is_some_and(|claim| claim.plan == slot);
        claimed
            || self
                .marks
                .get(usize::from(slot))
                .is_some_and(|&mark| mark != 0)
    }

    /// The claim's occurrence into its plan's mark.
    fn fold(&mut self, claim: Claim) {
        if let Some(mark) = self.marks.get_mut(usize::from(claim.plan)) {
            *mark = (*mark).max(claim.due_s);
        }
    }

    fn set_enabled(&mut self, slot: u8, enabled: bool) {
        if let Some(Some(plan)) = self.table.get_mut(usize::from(slot))
            && plan.enabled != enabled
        {
            plan.enabled = enabled;
            self.revision = self.revision.wrapping_add(1);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlansStatus {
    Unopened,
    /// No record yet: no plans, writable.
    Empty,
    Loaded,
    /// A newer schema version: no plans, read-only.
    ReadOnly,
    Io,
}

impl PlansStatus {
    /// The status's record word.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Unopened => "unopened",
            Self::Empty => "empty",
            Self::Loaded => "loaded",
            Self::ReadOnly => "read-only",
            Self::Io => "io",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PlansStore<I> {
    units: DualSlot<I>,
    pub status: PlansStatus,
    /// The save count of the record loaded or last saved.
    pub sequence: u32,
    pub slot: c_uint,
    pub plans: Plans,
    /// The revision this boot's last client table produced once saved (0
    /// for none): how a client's write learns it landed, whatever the loop
    /// wrote after it. RAM only.
    pub replaced: u32,
}

impl<I> Default for PlansStore<I> {
    fn default() -> Self {
        Self {
            units: DualSlot::unbound(&LAYOUT),
            status: PlansStatus::Unopened,
            sequence: 0,
            slot: 0,
            plans: Plans::default(),
            replaced: 0,
        }
    }
}

fn encode(sequence: u32, plans: &Plans) -> [u8; RECORD_BYTES] {
    let mut p = LAYOUT.record();
    put_u32::<SEQUENCE_AT, _>(&mut p, sequence);
    put_u32::<REVISION_AT, _>(&mut p, plans.revision);

    for (slot, plan) in plans.table.iter().enumerate() {
        let at = PLANS_AT + slot * PLAN_BYTES;
        if let Some(plan) = plan {
            p[at] = PRESENT | if plan.enabled { ENABLED } else { 0 };
            p[at + 1..at + 3].copy_from_slice(&plan.minute.to_le_bytes());
            p[at + 3] = plan.portions;
            p[at + 4] = plan.days.0;
        }
    }
    for (slot, mark) in plans.marks.iter().enumerate() {
        let at = MARKS_AT + slot * 4;
        p[at..at + 4].copy_from_slice(&mark.to_le_bytes());
    }
    if let Some(claim) = plans.claim {
        p[CLAIM_AT] = PRESENT | if claim.disabled { DISABLED } else { 0 };
        p[CLAIM_AT + 1] = claim.plan;
        put_u32::<{ CLAIM_AT + 2 }, _>(&mut p, claim.due_s);
        put_u32::<{ CLAIM_AT + 6 }, _>(&mut p, claim.high_water);
    }
    seal(&mut p);
    p
}

/// The sequence and the plans of a record whose header and checksum hold.
/// A plan outside its limits, a flag this build does not know or a stray
/// byte is no record of ours.
fn decode(p: &[u8; RECORD_BYTES]) -> Option<(u32, Plans)> {
    let sequence = get_u32::<SEQUENCE_AT, _>(p);
    let mut plans = Plans {
        revision: get_u32::<REVISION_AT, _>(p),
        ..Plans::default()
    };

    for (slot, entry) in plans.table.iter_mut().enumerate() {
        let at = PLANS_AT + slot * PLAN_BYTES;
        let bytes = p.get(at..at + PLAN_BYTES)?;
        *entry = match bytes[0] {
            0 if bytes.iter().all(|&b| b == 0) => None,
            flags if flags & !(PRESENT | ENABLED) == 0 && flags & PRESENT != 0 => {
                let plan = Plan {
                    minute: u16::from_le_bytes([bytes[1], bytes[2]]),
                    portions: bytes[3],
                    days: Weekdays(bytes[4]),
                    enabled: flags & ENABLED != 0,
                };
                if !plan.valid() {
                    return None;
                }
                Some(plan)
            }
            _ => return None,
        };
    }
    for (slot, mark) in plans.marks.iter_mut().enumerate() {
        let at = MARKS_AT + slot * 4;
        *mark = u32::from_le_bytes(p.get(at..at + 4)?.try_into().ok()?);
    }
    plans.claim = match p[CLAIM_AT] {
        0 if p[CLAIM_AT..CLAIM_END].iter().all(|&b| b == 0) => None,
        flags if flags & !(PRESENT | DISABLED) == 0 && flags & PRESENT != 0 => {
            let claim = Claim {
                plan: p[CLAIM_AT + 1],
                due_s: get_u32::<{ CLAIM_AT + 2 }, _>(p),
                high_water: get_u32::<{ CLAIM_AT + 6 }, _>(p),
                disabled: flags & DISABLED != 0,
            };
            if usize::from(claim.plan) >= PLANS {
                return None;
            }
            Some(claim)
        }
        _ => return None,
    };

    if sequence == 0 || p[CLAIM_END..CRC_AT].iter().any(|&b| b != 0) {
        return None;
    }
    Some((sequence, plans))
}

impl<I: SlotIo> PlansStore<I> {
    /// Bind to two exclusive erase units and load the newest valid record.
    pub fn open(&mut self, io: I) -> PlansStatus {
        let unit = io.unit_bytes();
        *self = Self {
            units: DualSlot::new(io, &LAYOUT),
            ..Self::default()
        };
        if (unit as usize) < RECORD_BYTES {
            self.status = PlansStatus::Io;
            return self.status;
        }
        let (Some(a), Some(b)) = (self.units.scan(0, decode), self.units.scan(1, decode)) else {
            self.status = PlansStatus::Io;
            return self.status;
        };

        self.status = match newest(a, b, |&(sequence, _)| u64::from(sequence)) {
            Pick::Future => PlansStatus::ReadOnly,
            Pick::Newest(slot, (sequence, plans)) => {
                self.slot = slot;
                self.sequence = sequence;
                self.plans = plans;
                PlansStatus::Loaded
            }
            // The two sectors are this store's alone, so anything else there
            // is a save of ours cut short: no plans, still writable.
            Pick::Nothing => PlansStatus::Empty,
        };
        self.status
    }

    pub fn writable(&self) -> bool {
        matches!(self.status, PlansStatus::Empty | PlansStatus::Loaded)
    }

    pub fn view(&self, armed: bool) -> PlansView {
        PlansView {
            replaced: self.replaced,
            ..self.plans.view(armed && self.writable())
        }
    }

    /// Apply `change` and save the result: `None` when the change is
    /// dropped (nothing written), else whether the save held. The writer
    /// and the simulator both apply changes through this.
    pub fn apply(&mut self, change: &PlansChange) -> Option<bool> {
        let plans = self.plans.changed(change)?;
        let saved = self.save(&plans);
        if saved && matches!(change, PlansChange::Replace { .. }) {
            self.replaced = plans.revision;
        }
        Some(saved)
    }

    /// Store `plans` to the other slot by the torn-write rule. A failure
    /// keeps the previous plans in memory, and the store refuses every
    /// later save this boot.
    pub fn save(&mut self, plans: &Plans) -> bool {
        if !self.writable() || !self.units.writable() {
            return false;
        }
        let Some(sequence) = self.sequence.checked_add(1) else {
            self.status = PlansStatus::Io;
            return false;
        };
        let slot = target(u64::from(self.sequence), self.slot);
        if !self
            .units
            .write(slot, &encode(sequence, plans), Prepare::Erase)
        {
            self.status = PlansStatus::Io;
            return false;
        }
        self.slot = slot;
        self.sequence = sequence;
        self.plans = *plans;
        true
    }
}
