//! Stock's feeding plans, as stock 1.1.15 keeps them in flash, and their
//! conversion to the replacement's plans ([stock plan records]).
//!
//! Moving to the replacement keeps the owner's meals: the installer reads
//! stock's two plan sectors from the feeder, [`StockPlans::decode`] reads
//! them as stock's own boot does, and [`convert`] turns each plan into a
//! [`Plan`] for the same slot. A plan the replacement cannot run as stock
//! would is refused by name ([`Refused`]), never left out quietly. What a
//! converted plan loses on the way (its meal call, a meal stock still
//! owed) is a [`Note`] on it.
//!
//! Stock checks none of its fields, so the decoder takes every byte as
//! stored and the converter is where values are judged: "99:99", zero
//! portions or a weekday byte of 8 decode, then refuse.
//!
//! [stock plan records]: ../../../../../docs/devices/plaf103/stock/README.md#plan-records

use core::fmt;
use core::fmt::Write as _;
use core::ops::Range;

use wellfed_model::{PLANS, PORTIONS_MAX, Plan, Weekday, Weekdays};

/// The flash stock's plans occupy: the count sector, then the entries
/// sector. Stock erases each sector before it writes it.
pub const PLAN_SECTORS: Range<u32> = 0x7d_0000..0x7d_2000;

/// Bytes in one stock flash sector.
const SECTOR: usize = 0x1000;
/// Bytes in one stored plan entry.
pub const ENTRY_LEN: usize = 0x2c;
/// The most plans stock stores; its setter drops an eleventh.
pub const STOCK_PLANS: usize = 10;
/// The entries stock's boot can read: its ten slots.
pub const ENTRIES_LEN: usize = STOCK_PLANS * ENTRY_LEN;

/// The count byte of an erased count sector, which stock's boot reads as
/// no plans.
const ERASED: u8 = 0xff;

// Field offsets within an entry. The eight-byte times are not naturally
// aligned, so each field is read byte by byte.
const ID: Range<usize> = 0x00..0x04;
const MINUTE: usize = 0x04;
const HOUR: usize = 0x05;
const ONCE: usize = 0x06;
const DAYS: Range<usize> = 0x07..0x0e;
const CALL: usize = 0x0e;
const CALL_TIMES: usize = 0x0f;
const PORTIONS: usize = 0x10;
const STATE: usize = 0x11;
const SYNC: Range<usize> = 0x12..0x1a;
const SKIP_UNTIL: Range<usize> = 0x1a..0x22;

// The replacement's slots are stock's, one for one, and the feeder serves
// every entry stock's boot reads.
const _: () = assert!(STOCK_PLANS == PLANS);
const _: () = assert!(ENTRIES_LEN == wellfed_proto::api::STOCK_PLAN_ENTRIES_LEN);

/// Why stock's plan sectors could not be read at all. Each plan in a
/// readable table is judged on its own by [`convert`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    /// The bytes are not the two plan sectors (or, read in parts, not the
    /// ten entries).
    #[error("stock's plan bytes are {expected} long, not {len}")]
    Length { len: usize, expected: usize },
    /// A count stock's setter never writes. Stock's own boot would read
    /// past its table, so nothing in it can be trusted.
    #[error("stock's plan count is {count}, more than its {STOCK_PLANS} slots")]
    Count { count: u8 },
    /// An erased count over entries that are not erased. Stock's empty
    /// save writes count 0, and a feeder that never saved has both
    /// sectors erased, so only a cut between erasing the count and
    /// writing it leaves this: how many plans there were is lost.
    #[error("stock's plan count is erased over stored entries: a save was cut")]
    Torn,
}

/// The run state stock keeps in each entry. Stock writes it while a plan
/// is due or waiting and persists it with the table, so a table copied
/// mid-meal carries it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StockPlanState {
    /// Nothing pending.
    Idle,
    /// Selected at its minute; the feed is starting.
    Due,
    /// Came due during another feed; runs once the feeder is free.
    Waiting,
    /// Taken from waiting; its feed is running.
    Resumed,
    /// A value stock's code never writes.
    Unknown(u8),
}

impl StockPlanState {
    pub const fn from_byte(byte: u8) -> Self {
        match byte {
            0 => Self::Idle,
            8 => Self::Due,
            10 => Self::Waiting,
            11 => Self::Resumed,
            other => Self::Unknown(other),
        }
    }

    /// Whether stock still owed this plan's meal when the table was saved.
    pub const fn pending(self) -> bool {
        matches!(self, Self::Due | Self::Waiting | Self::Resumed)
    }
}

/// One stored stock plan, every field as stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StockPlan {
    /// The cloud's `planId`.
    pub id: u32,
    /// The local hour and minute, in stock's whole-hour zone.
    pub hour: u8,
    pub minute: u8,
    /// Runs at its next minute on any day, then is deleted. Stock sets it
    /// for a plan sent with no repeat days and a `syncTime`.
    pub once: bool,
    /// Repeat days, ISO numbered (1 Monday to 7 Sunday); 0 is an unused
    /// slot and any other value matches no day.
    pub days: [u8; 7],
    /// `enableAudio`: play the meal call before the feed.
    pub call: bool,
    /// `audioTimes`: how many times the call plays.
    pub call_times: u8,
    pub portions: u8,
    pub state: StockPlanState,
    /// `syncTime`, Unix milliseconds. Stock reads it only when the plan
    /// arrives, to set [`Self::once`].
    pub sync_ms: u64,
    /// `skipEndTime`, Unix milliseconds: the plan does not run before it.
    /// Stock compares it unsigned, so a negative value sent by the cloud
    /// is far in the future.
    pub skip_until_ms: u64,
}

impl StockPlan {
    fn decode(entry: &[u8; ENTRY_LEN]) -> Self {
        let mut days = [0; 7];
        days.copy_from_slice(&entry[DAYS]);

        Self {
            id: u32::from_le_bytes(word(&entry[ID])),
            hour: entry[HOUR],
            minute: entry[MINUTE],
            once: entry[ONCE] != 0,
            days,
            call: entry[CALL] != 0,
            call_times: entry[CALL_TIMES],
            portions: entry[PORTIONS],
            state: StockPlanState::from_byte(entry[STATE]),
            sync_ms: u64::from_le_bytes(word(&entry[SYNC])),
            skip_until_ms: u64::from_le_bytes(word(&entry[SKIP_UNTIL])),
        }
    }

    /// The entry stock would store for this plan: what a test builds
    /// stock's sectors from.
    pub fn encode(&self) -> [u8; ENTRY_LEN] {
        let mut entry = [0; ENTRY_LEN];
        entry[ID].copy_from_slice(&self.id.to_le_bytes());
        entry[MINUTE] = self.minute;
        entry[HOUR] = self.hour;
        entry[ONCE] = u8::from(self.once);
        entry[DAYS].copy_from_slice(&self.days);
        entry[CALL] = u8::from(self.call);
        entry[CALL_TIMES] = self.call_times;
        entry[PORTIONS] = self.portions;
        entry[STATE] = match self.state {
            StockPlanState::Idle => 0,
            StockPlanState::Due => 8,
            StockPlanState::Waiting => 10,
            StockPlanState::Resumed => 11,
            StockPlanState::Unknown(byte) => byte,
        };
        entry[SYNC].copy_from_slice(&self.sync_ms.to_le_bytes());
        entry[SKIP_UNTIL].copy_from_slice(&self.skip_until_ms.to_le_bytes());
        entry
    }
}

impl StockPlan {
    /// Whether the entry reads as erased flash: every field all ones. No
    /// save writes that; a cut between stock's count and entry writes
    /// leaves it under the new count.
    pub fn erased(&self) -> bool {
        self.id == u32::MAX
            && self.hour == u8::MAX
            && self.minute == u8::MAX
            && self.days == [u8::MAX; 7]
            && self.call_times == u8::MAX
            && self.portions == u8::MAX
            && self.state == StockPlanState::Unknown(u8::MAX)
            && self.sync_ms == u64::MAX
            && self.skip_until_ms == u64::MAX
    }
}

/// `N` bytes from a field whose range has exactly that length.
fn word<const N: usize>(field: &[u8]) -> [u8; N] {
    let mut bytes = [0; N];
    bytes.copy_from_slice(field);
    bytes
}

/// The copy of stock's plan sectors an install keeps beside the stock
/// backup: the count byte, then the [`ENTRIES_LEN`] bytes of entries, as
/// the feeder served them.
pub const KEPT_LEN: usize = 1 + ENTRIES_LEN;

/// The kept copy of the count byte and entries the feeder served.
pub fn kept(count: u8, entries: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(1 + entries.len());
    bytes.push(count);
    bytes.extend_from_slice(entries);
    bytes
}

/// Stock's plan table, a slot per stored entry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StockPlans {
    /// `None` past the count, and for an entry stock deleted: it zeroes a
    /// one-off plan's entry after its feed and keeps the count.
    pub slots: [Option<StockPlan>; STOCK_PLANS],
}

impl StockPlans {
    /// Read the two plan sectors ([`PLAN_SECTORS`]) as stock's boot does:
    /// the count byte, erased meaning none, then that many entries.
    pub fn decode(sectors: &[u8]) -> Result<Self, DecodeError> {
        let expected = PLAN_SECTORS.len();
        if sectors.len() != expected {
            return Err(DecodeError::Length {
                len: sectors.len(),
                expected,
            });
        }
        let entries = &sectors[SECTOR..];
        if sectors[0] == ERASED && entries.iter().any(|&byte| byte != ERASED) {
            return Err(DecodeError::Torn);
        }
        Self::decode_parts(sectors[0], &entries[..ENTRIES_LEN])
    }

    /// Read the count byte and the first [`ENTRIES_LEN`] bytes of the
    /// entries sector, as the bank-0 role serves them (`ReadStockPlans`):
    /// everything stock's boot can read. A save never writes past them, so
    /// the rest of the sector is erased.
    pub fn decode_parts(count: u8, entries: &[u8]) -> Result<Self, DecodeError> {
        if entries.len() != ENTRIES_LEN {
            return Err(DecodeError::Length {
                len: entries.len(),
                expected: ENTRIES_LEN,
            });
        }
        let count = match count {
            ERASED if entries.iter().any(|&byte| byte != ERASED) => {
                return Err(DecodeError::Torn);
            }
            ERASED => 0,
            count if usize::from(count) <= STOCK_PLANS => usize::from(count),
            count => return Err(DecodeError::Count { count }),
        };

        let mut plans = Self::default();
        let (entries, _) = entries.as_chunks::<ENTRY_LEN>();
        for (slot, entry) in plans.slots.iter_mut().zip(entries.iter().take(count)) {
            if entry.iter().any(|&byte| byte != 0) {
                *slot = Some(StockPlan::decode(entry));
            }
        }
        Ok(plans)
    }

    /// Read a copy an install kept ([`kept`]).
    pub fn decode_kept(bytes: &[u8]) -> Result<Self, DecodeError> {
        let Some((&count, entries)) = bytes.split_first() else {
            return Err(DecodeError::Length {
                len: 0,
                expected: KEPT_LEN,
            });
        };
        Self::decode_parts(count, entries)
    }

    /// The two plan sectors as stock's setter leaves them for these
    /// plans: count, entries, the rest erased. Deleted slots before the
    /// last plan stay zeroed and counted, as stock leaves them.
    pub fn encode(&self) -> Vec<u8> {
        let mut sectors = vec![ERASED; PLAN_SECTORS.len()];
        let count = self
            .slots
            .iter()
            .rposition(Option::is_some)
            .map_or(0, |last| last + 1);
        sectors[0] = u8::try_from(count).unwrap_or(ERASED);

        for (i, slot) in self.slots.iter().take(count).enumerate() {
            let at = SECTOR + i * ENTRY_LEN;
            let entry = slot.map_or([0; ENTRY_LEN], |plan| plan.encode());
            sectors[at..at + ENTRY_LEN].copy_from_slice(&entry);
        }
        sectors
    }
}

/// Why one stock plan does not become a replacement plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PlanRefusal {
    /// A time no day has ("99:99"): stock stores it and never runs it.
    #[error("its time {hour:02}:{minute:02} is not a time of day")]
    Time { hour: u8, minute: u8 },
    /// Outside the 1 to 48 portions every replacement feed asks for.
    #[error("it asks for {portions} portions, not 1 to {PORTIONS_MAX}")]
    Portions { portions: u8 },
    /// A repeat-day value other than 1 (Monday) to 7 (Sunday).
    #[error("its repeat days hold {value}, which is not a weekday")]
    Weekday { value: u8 },
    /// No repeat day and not a one-off: stock never runs it (the cloud
    /// sent it without days or a `syncTime`).
    #[error("it has no repeat days and is not a one-off, so stock never runs it")]
    NeverRuns,
    /// A run state stock's code never writes: the entry is not one stock
    /// saved whole.
    #[error("its run state {byte} is not one stock writes")]
    State { byte: u8 },
    /// An erased entry under the count: a save cut between writing the
    /// count and writing the entries.
    #[error("its entry is erased: stock's last save was cut")]
    Erased,
    /// A one-off whose meal stock had started or still owed. Its feed may
    /// already have run before a cut, and run late it would be a day
    /// late, so it is not imported.
    #[error("it is a one-off stock had already started or still owed ({state:?})")]
    OnceOwed { state: StockPlanState },
}

/// Something a converted plan does not carry over from stock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Note {
    /// Stock played the meal call `times` times before this feed. The
    /// replacement has no meal call yet, so the meal runs without it.
    CallDropped { times: u8 },
    /// Stock had started or still owed this repeating plan's meal (due,
    /// waiting or resumed) when the table was saved. That meal is not run
    /// late: the plan runs from its next occurrence. A one-off in this
    /// state is refused instead ([`PlanRefusal::OnceOwed`]).
    PendingDropped,
    /// The owner skipped this plan until `until_ms` (Unix milliseconds).
    /// The replacement has no skip, so the plan arrives disabled rather
    /// than feed a meal the owner turned down; enabling it again resumes
    /// it.
    SkipDisabled { until_ms: u64 },
}

/// One stock plan's result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Converted {
        slot: usize,
        stock: StockPlan,
        plan: Plan,
        call: Option<Note>,
        pending: Option<Note>,
        skip: Option<Note>,
    },
    Refused {
        slot: usize,
        stock: StockPlan,
        refusal: PlanRefusal,
    },
}

impl Outcome {
    pub const fn slot(&self) -> usize {
        match *self {
            Self::Converted { slot, .. } | Self::Refused { slot, .. } => slot,
        }
    }

    /// The notes on a converted plan, in a fixed order.
    pub fn notes(&self) -> impl Iterator<Item = Note> + '_ {
        let notes = match self {
            Self::Converted {
                call,
                pending,
                skip,
                ..
            } => [*call, *pending, *skip],
            Self::Refused { .. } => [None; 3],
        };
        notes.into_iter().flatten()
    }
}

/// A refused plan, named as the owner knew it in stock's app.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("stock plan {id} in slot {slot} ({time}) was not imported: {refusal}")]
pub struct Refused {
    pub slot: usize,
    pub id: u32,
    pub time: StockTime,
    pub refusal: PlanRefusal,
}

/// A stock plan's time as stored, valid or not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StockTime {
    pub hour: u8,
    pub minute: u8,
}

impl fmt::Display for StockTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}:{:02}", self.hour, self.minute)
    }
}

/// Stock's plans as the replacement's: the table to put, and what became
/// of every stock plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Import {
    /// The table for `PutPlans`: each converted plan in stock's slot.
    pub plans: [Option<Plan>; PLANS],
    /// Every stock plan's result, by slot.
    pub outcomes: Vec<Outcome>,
}

impl Import {
    /// The plans refused, each by name.
    pub fn refused(&self) -> impl Iterator<Item = Refused> + '_ {
        self.outcomes.iter().filter_map(|outcome| match *outcome {
            Outcome::Refused {
                slot,
                stock,
                refusal,
            } => Some(Refused {
                slot,
                id: stock.id,
                time: StockTime {
                    hour: stock.hour,
                    minute: stock.minute,
                },
                refusal,
            }),
            Outcome::Converted { .. } => None,
        })
    }
}

/// Convert stock's plans, `now_ms` (Unix milliseconds) being when the
/// import runs, which decides whether a skip is still in force.
///
/// A plan keeps its local time: stock compares it with its clock in the
/// zone the cloud last set, rounded to whole hours, and the replacement
/// with its own zone. The owner set each time in their own zone, so it
/// means the same meal; in a half-hour zone, where stock ran meals 30
/// minutes off, the replacement runs them at the time the owner set.
pub fn convert(stock: &StockPlans, now_ms: u64) -> Import {
    let mut plans = [None; PLANS];
    let mut outcomes = Vec::new();

    for (slot, entry) in stock.slots.iter().enumerate() {
        let Some(stock) = *entry else {
            continue;
        };
        let outcome = match plan(&stock, now_ms) {
            Ok(plan) => {
                plans[slot] = Some(plan);
                Outcome::Converted {
                    slot,
                    stock,
                    plan,
                    call: stock.call.then_some(Note::CallDropped {
                        times: stock.call_times,
                    }),
                    pending: stock.state.pending().then_some(Note::PendingDropped),
                    skip: (!plan.enabled).then_some(Note::SkipDisabled {
                        until_ms: stock.skip_until_ms,
                    }),
                }
            }
            Err(refusal) => Outcome::Refused {
                slot,
                stock,
                refusal,
            },
        };
        outcomes.push(outcome);
    }

    Import { plans, outcomes }
}

/// One line per stock plan: what it became, or why it was refused.
pub fn report(import: &Import) -> String {
    let mut out = String::new();
    if import.outcomes.is_empty() {
        out.push_str("Stock holds no plans.\n");
    }

    for outcome in &import.outcomes {
        let line = match outcome {
            Outcome::Converted {
                slot, stock, plan, ..
            } => format!(
                "slot {slot}: stock plan {} -> {:02}:{:02} {}, {} portions{}",
                stock.id,
                plan.minute / 60,
                plan.minute % 60,
                plan.days,
                plan.portions,
                if plan.enabled { "" } else { ", disabled" },
            ),
            Outcome::Refused {
                slot,
                stock,
                refusal,
            } => format!(
                "slot {slot}: stock plan {} ({}) refused: {refusal}",
                stock.id,
                StockTime {
                    hour: stock.hour,
                    minute: stock.minute
                },
            ),
        };
        let _ = writeln!(out, "{line}");

        for note in outcome.notes() {
            let note = match note {
                Note::CallDropped { times } => {
                    format!("its meal call ({times} plays) is not carried over")
                }
                Note::PendingDropped => {
                    "the meal stock had started or still owed is not run again; \
                     it runs from its next occurrence"
                        .to_owned()
                }
                Note::SkipDisabled { until_ms } => {
                    format!("skipped until {until_ms} ms, so it arrives disabled")
                }
            };
            let _ = writeln!(out, "  note: {note}");
        }
    }
    out
}

/// The replacement plan that runs `stock`'s meals.
fn plan(stock: &StockPlan, now_ms: u64) -> Result<Plan, PlanRefusal> {
    if stock.erased() {
        return Err(PlanRefusal::Erased);
    }
    if stock.hour >= 24 || stock.minute >= 60 {
        return Err(PlanRefusal::Time {
            hour: stock.hour,
            minute: stock.minute,
        });
    }
    if !(1..=PORTIONS_MAX).contains(&stock.portions) {
        return Err(PlanRefusal::Portions {
            portions: stock.portions,
        });
    }
    if let StockPlanState::Unknown(byte) = stock.state {
        return Err(PlanRefusal::State { byte });
    }

    let mut repeat = Vec::new();
    for &value in &stock.days {
        match value {
            0 => {}
            1..=7 => repeat.push(Weekday::ALL[usize::from(value - 1)]),
            value => return Err(PlanRefusal::Weekday { value }),
        }
    }
    // Stock runs a one-off on any day, whatever its repeat days say, and
    // deletes it afterwards: the replacement's once.
    // A one-off stock had started may have fed before a cut, and one it
    // still owed would run a day late, so neither imports.
    let days = if stock.once {
        if stock.state.pending() {
            return Err(PlanRefusal::OnceOwed { state: stock.state });
        }
        Weekdays::ONCE
    } else if repeat.is_empty() {
        return Err(PlanRefusal::NeverRuns);
    } else {
        repeat.into_iter().collect()
    };

    Ok(Plan {
        minute: u16::from(stock.hour) * 60 + u16::from(stock.minute),
        portions: stock.portions,
        days,
        enabled: stock.skip_until_ms <= now_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-28T12:00:00Z.
    const NOW: u64 = 1_792_843_200_000;

    /// A daily 07:30 plan of 2 portions, as the cloud sends one.
    fn daily() -> StockPlan {
        StockPlan {
            id: 101,
            hour: 7,
            minute: 30,
            once: false,
            days: [1, 2, 3, 4, 5, 6, 7],
            call: false,
            call_times: 0,
            portions: 2,
            state: StockPlanState::Idle,
            sync_ms: NOW - 86_400_000,
            skip_until_ms: 0,
        }
    }

    fn table(plans: &[Option<StockPlan>]) -> StockPlans {
        let mut stock = StockPlans::default();
        stock.slots[..plans.len()].copy_from_slice(plans);
        stock
    }

    fn converted(import: &Import, slot: usize) -> Plan {
        import.plans[slot].expect("the plan converted")
    }

    #[test]
    fn an_entry_reads_back_field_by_field() {
        let plan = StockPlan {
            id: 0x0102_0304,
            hour: 23,
            minute: 59,
            once: true,
            days: [7, 1, 0, 0, 0, 0, 0],
            call: true,
            call_times: 3,
            portions: 48,
            state: StockPlanState::Waiting,
            sync_ms: 0x1122_3344_5566_7788,
            skip_until_ms: u64::MAX - 1,
        };
        let entry = plan.encode();

        // The offsets stock's setter writes.
        assert_eq!(&entry[..4], &[4, 3, 2, 1]);
        assert_eq!((entry[4], entry[5], entry[6]), (59, 23, 1));
        assert_eq!(&entry[7..14], &[7, 1, 0, 0, 0, 0, 0]);
        assert_eq!((entry[14], entry[15], entry[16], entry[17]), (1, 3, 48, 10));
        assert_eq!(entry[0x12], 0x88);
        assert_eq!(entry[0x19], 0x11);
        assert_eq!(entry[0x1a], 0xfe);
        assert_eq!(&entry[0x22..], &[0; 10], "stock never writes the tail");
        assert_eq!(StockPlan::decode(&entry), plan);
    }

    #[test]
    fn the_sectors_read_as_stock_boots() {
        let stock = table(&[Some(daily()), None, Some(daily())]);
        let sectors = stock.encode();
        assert_eq!(sectors.len(), 0x2000);
        assert_eq!(sectors[0], 3, "a deleted slot stays counted");
        assert!(sectors[1..0x1000].iter().all(|&b| b == 0xff));
        assert!(
            sectors[0x1000 + ENTRY_LEN..0x1000 + 2 * ENTRY_LEN]
                .iter()
                .all(|&b| b == 0),
            "a deleted entry is zeroed"
        );
        assert_eq!(StockPlans::decode(&sectors), Ok(stock));

        // Both sectors erased (never saved) and a zero count over erased
        // entries (an empty save) hold no plans; an erased count over
        // stored entries is a cut save, not an empty table.
        let mut fresh = vec![0xff; 0x2000];
        assert_eq!(StockPlans::decode(&fresh), Ok(StockPlans::default()));
        fresh[0] = 0;
        assert_eq!(StockPlans::decode(&fresh), Ok(StockPlans::default()));
        let mut torn = sectors.clone();
        torn[0] = 0xff;
        assert_eq!(StockPlans::decode(&torn), Err(DecodeError::Torn));
        let mut none = sectors.clone();
        none[0] = 0;
        assert_eq!(StockPlans::decode(&none), Ok(StockPlans::default()));
        let mut one = sectors;
        one[0] = 1;
        assert_eq!(
            StockPlans::decode(&one),
            Ok(table(&[Some(daily())])),
            "entries past the count are ignored"
        );
    }

    #[test]
    fn a_count_past_ten_or_the_wrong_length_reads_nothing() {
        let mut sectors = table(&[Some(daily())]).encode();
        sectors[0] = 11;
        assert_eq!(
            StockPlans::decode(&sectors),
            Err(DecodeError::Count { count: 11 })
        );
        assert_eq!(
            StockPlans::decode(&sectors[..0x1000]),
            Err(DecodeError::Length {
                len: 0x1000,
                expected: 0x2000
            })
        );

        // Ten is stock's full table.
        let full = table(&[Some(daily()); STOCK_PLANS]);
        let decoded = StockPlans::decode(&full.encode()).unwrap();
        assert_eq!(decoded, full);
        assert_eq!(convert(&decoded, NOW).plans.iter().flatten().count(), 10);
    }

    /// The count byte and ten entries the feeder serves read as the whole
    /// sectors do, with the same refusals.
    #[test]
    fn the_served_parts_read_as_the_sectors_do() {
        let parts = |sectors: &[u8]| {
            StockPlans::decode_parts(sectors[0], &sectors[SECTOR..SECTOR + ENTRIES_LEN])
        };

        let full = table(&[Some(daily()); STOCK_PLANS]).encode();
        let gapped = table(&[Some(daily()), None, Some(daily())]).encode();
        let fresh = vec![0xff; 0x2000];
        for sectors in [&full, &gapped, &fresh] {
            assert_eq!(parts(sectors), StockPlans::decode(sectors));
            assert!(parts(sectors).is_ok());
        }

        let mut torn = gapped.clone();
        torn[0] = 0xff;
        assert_eq!(parts(&torn), Err(DecodeError::Torn));
        let mut eleven = gapped;
        eleven[0] = 11;
        assert_eq!(parts(&eleven), Err(DecodeError::Count { count: 11 }));
        assert_eq!(
            StockPlans::decode_parts(1, &full[SECTOR..SECTOR + ENTRY_LEN]),
            Err(DecodeError::Length {
                len: ENTRY_LEN,
                expected: ENTRIES_LEN
            })
        );
    }

    #[test]
    fn plans_keep_their_slot_time_portions_and_days() {
        let weekdays = StockPlan {
            id: 7,
            hour: 18,
            minute: 5,
            days: [5, 1, 3, 0, 0, 0, 0],
            portions: 1,
            ..daily()
        };
        let stock = table(&[Some(daily()), None, Some(weekdays)]);

        let import = convert(&stock, NOW);
        assert_eq!(
            converted(&import, 0),
            Plan {
                minute: 7 * 60 + 30,
                portions: 2,
                days: Weekdays::DAILY,
                enabled: true,
            }
        );
        assert_eq!(import.plans[1], None);
        let plan = converted(&import, 2);
        assert_eq!(plan.minute, 18 * 60 + 5);
        assert_eq!(plan.days.to_string(), "mon,wed,fri");
        assert_eq!(import.outcomes.len(), 2);
        assert!(import.outcomes.iter().all(|o| o.notes().next().is_none()));
        assert!(plan.valid());
    }

    #[test]
    fn a_one_off_runs_once_whatever_its_days() {
        for days in [[0; 7], [3, 0, 0, 0, 0, 0, 0]] {
            let once = StockPlan {
                once: true,
                days,
                ..daily()
            };
            let import = convert(&table(&[Some(once)]), NOW);
            assert_eq!(converted(&import, 0).days, Weekdays::ONCE);
        }
    }

    #[test]
    fn a_plan_stock_would_not_run_as_given_is_refused_by_name() {
        let cases = [
            (
                StockPlan {
                    hour: 99,
                    minute: 99,
                    ..daily()
                },
                PlanRefusal::Time {
                    hour: 99,
                    minute: 99,
                },
            ),
            (
                StockPlan {
                    minute: 60,
                    ..daily()
                },
                PlanRefusal::Time {
                    hour: 7,
                    minute: 60,
                },
            ),
            (
                StockPlan {
                    portions: 0,
                    ..daily()
                },
                PlanRefusal::Portions { portions: 0 },
            ),
            (
                StockPlan {
                    portions: 49,
                    ..daily()
                },
                PlanRefusal::Portions { portions: 49 },
            ),
            (
                StockPlan {
                    days: [1, 8, 0, 0, 0, 0, 0],
                    ..daily()
                },
                PlanRefusal::Weekday { value: 8 },
            ),
            (
                StockPlan {
                    days: [0; 7],
                    ..daily()
                },
                PlanRefusal::NeverRuns,
            ),
            (
                StockPlan {
                    state: StockPlanState::Unknown(3),
                    ..daily()
                },
                PlanRefusal::State { byte: 3 },
            ),
        ];

        for (bad, refusal) in cases {
            let stock = table(&[Some(daily()), Some(bad)]);
            let import = convert(&StockPlans::decode(&stock.encode()).unwrap(), NOW);

            assert!(import.plans[0].is_some(), "the good plan still converts");
            assert_eq!(import.plans[1], None);
            let refused: Vec<_> = import.refused().collect();
            assert_eq!(
                refused,
                [Refused {
                    slot: 1,
                    id: 101,
                    time: StockTime {
                        hour: bad.hour,
                        minute: bad.minute
                    },
                    refusal,
                }],
                "{bad:?}"
            );
        }
    }

    /// A cut between stock's count and entry writes leaves the entries
    /// sector erased under a nonzero count: every plan refuses, by name.
    #[test]
    fn a_torn_write_refuses_each_plan() {
        let mut sectors = vec![0xff; 0x2000];
        sectors[0] = 2;
        let import = convert(&StockPlans::decode(&sectors).unwrap(), NOW);

        assert_eq!(import.plans, [None; PLANS]);
        let refused: Vec<_> = import.refused().map(|r| (r.slot, r.refusal)).collect();
        assert_eq!(
            refused,
            [(0, PlanRefusal::Erased), (1, PlanRefusal::Erased)]
        );
        assert_eq!(
            import.refused().next().unwrap().to_string(),
            "stock plan 4294967295 in slot 0 (255:255) was not imported: \
             its entry is erased: stock's last save was cut"
        );
    }

    /// A one-off stock had started may already have fed, and one it owed
    /// would run a day late: neither imports. A repeating plan in the same
    /// state imports, its owed meal noted.
    #[test]
    fn a_one_off_stock_started_or_owed_is_refused() {
        for state in [
            StockPlanState::Due,
            StockPlanState::Waiting,
            StockPlanState::Resumed,
        ] {
            let once = StockPlan {
                once: true,
                days: [0; 7],
                state,
                ..daily()
            };
            let repeating = StockPlan { state, ..daily() };
            let import = convert(&table(&[Some(once), Some(repeating)]), NOW);

            assert_eq!(import.plans[0], None);
            let refused: Vec<_> = import.refused().map(|r| (r.slot, r.refusal)).collect();
            assert_eq!(refused, [(0, PlanRefusal::OnceOwed { state })]);
            assert_eq!(converted(&import, 1).days, Weekdays::DAILY);
            assert_eq!(
                import.outcomes[1].notes().collect::<Vec<_>>(),
                [Note::PendingDropped]
            );
        }
    }

    #[test]
    fn what_a_plan_loses_is_noted() {
        let called = StockPlan {
            call: true,
            call_times: 3,
            state: StockPlanState::Waiting,
            ..daily()
        };
        let import = convert(&table(&[Some(called)]), NOW);

        assert!(converted(&import, 0).enabled);
        let notes: Vec<_> = import.outcomes[0].notes().collect();
        assert_eq!(
            notes,
            [Note::CallDropped { times: 3 }, Note::PendingDropped]
        );
    }

    #[test]
    fn a_skip_in_force_arrives_disabled() {
        let skipped = StockPlan {
            skip_until_ms: NOW + 1,
            ..daily()
        };
        let lapsed = StockPlan {
            skip_until_ms: NOW,
            ..daily()
        };
        // A negative skip from the cloud, compared unsigned as stock does.
        let forever = StockPlan {
            skip_until_ms: (-2_i64).cast_unsigned(),
            ..daily()
        };
        let import = convert(&table(&[Some(skipped), Some(lapsed), Some(forever)]), NOW);

        assert!(!converted(&import, 0).enabled);
        assert!(converted(&import, 1).enabled);
        assert!(!converted(&import, 2).enabled);
        assert_eq!(
            import.outcomes[0].notes().collect::<Vec<_>>(),
            [Note::SkipDisabled { until_ms: NOW + 1 }]
        );
        assert_eq!(import.outcomes[1].notes().count(), 0);
    }

    /// Plan 42 at hh:15 on Monday and Tuesday, with a meal call.
    fn noted(hour: u8, portions: u8) -> StockPlan {
        StockPlan {
            id: 42,
            hour,
            minute: 15,
            once: false,
            days: [1, 2, 0, 0, 0, 0, 0],
            call: true,
            call_times: 2,
            portions,
            state: StockPlanState::Idle,
            sync_ms: 0,
            skip_until_ms: 0,
        }
    }

    #[test]
    fn the_report_names_each_plan() {
        let mut stock = StockPlans::default();
        stock.slots[0] = Some(noted(8, 3));
        stock.slots[1] = Some(noted(8, 0));

        let lines = report(&convert(&stock, 0));
        assert_eq!(
            lines,
            "slot 0: stock plan 42 -> 08:15 mon,tue, 3 portions\n  \
             note: its meal call (2 plays) is not carried over\n\
             slot 1: stock plan 42 (08:15) refused: it asks for 0 portions, not 1 to 48\n"
        );
        assert_eq!(
            report(&convert(&StockPlans::default(), 0)),
            "Stock holds no plans.\n"
        );
    }
}
