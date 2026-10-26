//! Per-feed traces: the recorder against drives shaped like the bench's,
//! a body that halves and one that truncates, and the storage ring's
//! order, wrap, lookups and a power cut at every mutation of an append and
//! of the erase that makes room.

use wellfed_core::feed::controller::MotorCommand;
use wellfed_core::feed::observe::{Beams, Observation};
use wellfed_feeder::store::traces::{self, MAX_UNITS, TraceRing};
use wellfed_feeder::trace::{DriveFigures, Recorder};
use wellfed_proto::api::{
    CurrentUnit, EventBody, FeedOutcome, FeedReason, Refusal, Source, TraceSummary,
};
use wellfed_proto::trace::{BODY_MAX, Entry, Motor, PAGE_MAX, Reader};
use wellfed_testkit::journal_flash::{FakeJournalFlash, FlashIo};

const SECTOR: usize = 4096;
/// The supervisor's clock when the drive starts.
const DRIVE_AT: u32 = 50_000;
/// The owner's poll on the bench: 903 polls over a 27.2 s drive.
const POLL_MS: u32 = 30;

fn fed(feed: u32, portions: u8) -> EventBody {
    EventBody::Fed {
        feed,
        requested: portions,
        completed: portions,
        outcome: FeedOutcome::Done,
        reason: FeedReason::Done,
        by: Source::Panel,
    }
}

/// One poll of a drive `at` ms in, PA18 resting HIGH and LOW for 1.2 s
/// at each mark every 9.07 s, as feed 9 measured it; the outlet beam broken
/// for one poll in each portion, and the battery sagging 100 mV while
/// the motor drives.
fn poll(at: u32, end: u32) -> Observation {
    let marks = |at: u32| {
        let mut edges = 0u16;
        let mut last = None;
        for mark in 1..=at / 9_070 + 1 {
            let fall = mark * 9_070 - 1_200;
            for (edge, high) in [(fall, false), (mark * 9_070, true)] {
                if edge <= at {
                    edges += 1;
                    last = Some((DRIVE_AT + edge, high));
                }
            }
        }
        (edges, last)
    };
    let (edges, last_edge) = marks(at);
    let rising = edges / 2;
    let driving = at < end;
    Observation {
        at_ms: DRIVE_AT + at,
        drive_at_ms: Some(DRIVE_AT),
        current: Some(if at < 20 {
            333
        } else {
            58 + (at / POLL_MS) % 5
        }),
        battery_mv: Some(if driving { 4_700 } else { 4_800 }),
        beams: Some(Beams {
            outlet_clear: at % 9_070 / POLL_MS != 100,
            food: false,
        }),
        rising,
        falling: edges - rising,
        last_edge,
        motor: if driving {
            MotorCommand::Drive0
        } else {
            MotorCommand::Stop
        },
    }
}

/// A drive of `portions` marks, recorded and sealed; the summary.
fn record(recorder: &mut Recorder, feed: u32, portions: u32) -> TraceSummary {
    recorder.begin(feed);
    recorder.observe(&Observation {
        at_ms: DRIVE_AT - 100,
        battery_mv: Some(4_810),
        ..Observation::default()
    });
    let end = portions * 9_070 + 15;
    let mut at = 0;
    while at <= end + POLL_MS {
        recorder.observe(&poll(at, end));
        at += POLL_MS;
    }
    let figures = DriveFigures {
        drive_ms: end,
        samples: end / POLL_MS,
        current_peak: 333,
        current_mean: Some(60),
        rising: u16::try_from(portions).unwrap(),
        falling: u16::try_from(portions).unwrap(),
        first_edge_ms: Some(7_870),
    };
    let portions = u8::try_from(portions).unwrap();
    recorder.seal(&fed(feed, portions), &figures).unwrap()
}

fn entries(body: &[u8]) -> Vec<Entry> {
    Reader::new(body).map(Result::unwrap).collect()
}

#[test]
fn a_three_portion_feed_fits_with_every_mark_and_change() {
    let mut recorder = Recorder::new();
    let summary = record(&mut recorder, 9, 3);

    assert_eq!(
        (
            summary.feed,
            summary.requested,
            summary.outcome,
            summary.unit
        ),
        (9, 3, FeedOutcome::Done, CurrentUnit::Scaled)
    );
    assert_eq!((summary.drive_ms, summary.current_peak), (27_225, 333));
    assert_eq!(
        (summary.battery_rest_mv, summary.battery_min_mv),
        (Some(4_810), Some(4_700))
    );
    assert_eq!((summary.beam_breaks, summary.reversals), (3, 0));
    assert!(!summary.truncated);
    // About 900 readings at two bytes each, and the marks, beams and
    // battery: halved twice, a reading every 120 ms.
    assert_eq!(summary.stride, 4);
    assert!(recorder.body().len() <= BODY_MAX);

    let read = entries(recorder.body());
    let marks: Vec<(u32, bool)> = read
        .iter()
        .filter_map(|entry| match *entry {
            Entry::Mark {
                at_ms, edges, high, ..
            } => {
                assert_eq!(edges, 1);
                Some((at_ms, high))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        marks,
        [
            (7_870, false),
            (9_070, true),
            (16_940, false),
            (18_140, true),
            (26_010, false),
            (27_210, true)
        ]
    );
    let motors: Vec<(u32, Motor)> = read
        .iter()
        .filter_map(|entry| match *entry {
            Entry::Motor { at_ms, motor } => Some((at_ms, motor)),
            _ => None,
        })
        .collect();
    assert_eq!(motors, [(0, Motor::Drive0), (27_240, Motor::Stop)]);
    let first = read.iter().find(|e| matches!(e, Entry::Current { .. }));
    assert_eq!(
        first,
        Some(&Entry::Current {
            at_ms: 0,
            value: 333
        })
    );
}

#[test]
fn the_longest_feed_decimates_and_keeps_every_mark() {
    let mut recorder = Recorder::new();
    let summary = record(&mut recorder, 1, 48);

    assert!(!summary.truncated);
    assert!(summary.stride >= 16, "{}", summary.stride);
    let read = entries(recorder.body());
    let marks = read
        .iter()
        .filter(|e| matches!(e, Entry::Mark { .. }))
        .count();
    assert_eq!(marks, 96);
    // Marks, beams and battery changes take most of the body over ten
    // minutes; the readings left still span the drive.
    let readings: Vec<u32> = read
        .iter()
        .filter_map(|e| match *e {
            Entry::Current { at_ms, .. } => Some(at_ms),
            _ => None,
        })
        .collect();
    assert!(readings.len() >= 20, "{}", readings.len());
    assert!(readings.last().is_some_and(|&at| at > 400_000));
}

#[test]
fn a_chattering_sensor_truncates_the_body_but_not_the_summary() {
    let mut recorder = Recorder::new();
    recorder.begin(4);
    for poll in 0..3_000u32 {
        let high = poll % 2 == 0;
        recorder.observe(&Observation {
            at_ms: DRIVE_AT + poll * POLL_MS,
            drive_at_ms: Some(DRIVE_AT),
            current: Some(60),
            beams: Some(Beams {
                outlet_clear: high,
                food: !high,
            }),
            rising: u16::try_from(poll).unwrap(),
            last_edge: Some((DRIVE_AT + poll * POLL_MS, true)),
            motor: MotorCommand::Drive0,
            ..Observation::default()
        });
    }
    let summary = recorder.seal(&fed(4, 1), &DriveFigures::default()).unwrap();

    assert!(summary.truncated);
    assert_eq!(summary.beam_breaks, 1_500);
    // Halving leaves the first reading, which nothing can halve further.
    let read = entries(recorder.body());
    let readings = read
        .iter()
        .filter(|e| matches!(e, Entry::Current { .. }))
        .count();
    assert_eq!(readings, 1);
    assert!(recorder.body().len() <= BODY_MAX);
}

#[test]
fn a_turn_from_one_direction_to_the_other_is_a_reversal() {
    let mut recorder = Recorder::new();
    recorder.begin(2);
    for (at, motor) in [
        (0, MotorCommand::Drive0),
        (500, MotorCommand::Stop),
        (1_000, MotorCommand::Drive1),
        (1_500, MotorCommand::Stop),
        (2_000, MotorCommand::Drive0),
        (2_500, MotorCommand::Drive0),
    ] {
        recorder.observe(&Observation {
            at_ms: DRIVE_AT + at,
            drive_at_ms: Some(DRIVE_AT),
            motor,
            ..Observation::default()
        });
    }
    let summary = recorder.seal(&fed(2, 1), &DriveFigures::default()).unwrap();
    assert_eq!(summary.reversals, 2);
}

#[test]
fn a_trace_is_sealed_once_and_only_for_its_feed() {
    let mut recorder = Recorder::new();
    recorder.begin(5);
    let mut out = [0; PAGE_MAX];
    assert_eq!(recorder.page(0, &mut out), Err(Refusal::Busy));
    assert_eq!(recorder.seal(&fed(6, 1), &DriveFigures::default()), None);
    assert!(
        recorder
            .seal(&fed(5, 1), &DriveFigures::default())
            .is_some()
    );
    assert_eq!(recorder.seal(&fed(5, 1), &DriveFigures::default()), None);
    assert_eq!(
        recorder.page(1, &mut out),
        Err(Refusal::Invalid),
        "the empty body ends at 0"
    );
    assert_eq!(recorder.page(0, &mut out).map(|read| read.len), Ok(0));
}

#[test]
fn pages_cover_the_body_from_ram() {
    let mut recorder = Recorder::new();
    record(&mut recorder, 9, 3);
    let body = recorder.body().to_vec();

    let mut out = [0; PAGE_MAX];
    let mut copied = Vec::new();
    let mut offset = 0u16;
    loop {
        let read = recorder.page(offset, &mut out).unwrap();
        assert!(!read.stored && usize::from(read.total) == body.len());
        copied.extend_from_slice(&out[..read.len]);
        offset += u16::try_from(read.len).unwrap();
        if offset == read.total {
            break;
        }
    }
    assert_eq!(copied, body);
    assert!(body.len() > PAGE_MAX, "two pages");
}

fn open(f: &mut FakeJournalFlash) -> TraceRing<FlashIo> {
    let mut ring = TraceRing::closed();
    // SAFETY: the fake flash outlives the ring in every test.
    ring.open(unsafe { f.io() }, MAX_UNITS);
    ring
}

fn io(f: &mut FakeJournalFlash) -> FlashIo {
    // SAFETY: the fake flash outlives the I/O in every test.
    unsafe { f.io() }
}

/// A sealed trace of feed `feed` whose body length varies with it.
fn sealed(feed: u32) -> (TraceSummary, Vec<u8>) {
    let mut recorder = Recorder::new();
    let summary = record(&mut recorder, feed, feed % 3 + 1);
    (summary, recorder.body().to_vec())
}

/// The stored trace of `feed`, read a page at a time, if the ring has it.
fn stored(f: &mut FakeJournalFlash, feed: u32) -> Option<(TraceSummary, Vec<u8>)> {
    let io = io(f);
    let mut out = [0; PAGE_MAX];
    let mut body = Vec::new();
    let mut offset = 0u16;
    loop {
        let read = match traces::page(&io, MAX_UNITS, feed, offset, &mut out) {
            Ok(read) => read,
            Err(Refusal::NotFound) => return None,
            Err(refusal) => panic!("{refusal:?}"),
        };
        assert!(read.stored);
        body.extend_from_slice(&out[..read.len]);
        offset += u16::try_from(read.len).unwrap();
        if offset == read.total {
            return Some((read.summary, body));
        }
    }
}

#[test]
fn traces_read_back_across_reopens_and_the_ring_keeps_the_newest() {
    let mut f = FakeJournalFlash::with_units(SECTOR, MAX_UNITS);
    let mut ring = open(&mut f);
    assert_eq!(
        (ring.kept(), ring.capacity(), ring.last_seq()),
        (0, 8, None)
    );
    assert_eq!(stored(&mut f, 1), None);

    for feed in 1..=5 {
        let (summary, body) = sealed(feed);
        assert_eq!(ring.append(&summary, &body), Ok(feed));
    }
    let mut ring = open(&mut f);
    assert_eq!((ring.kept(), ring.last_seq()), (5, Some(5)));
    for feed in 1..=5 {
        assert_eq!(stored(&mut f, feed), Some(sealed(feed)));
    }

    // Past twelve slots the oldest sector goes, four traces at a time.
    for feed in 6..=13 {
        let (summary, body) = sealed(feed);
        assert_eq!(ring.append(&summary, &body), Ok(feed));
    }
    assert_eq!(ring.kept(), 9);
    for feed in 1..=4 {
        assert_eq!(stored(&mut f, feed), None, "feed {feed}");
    }
    for feed in 5..=13 {
        assert!(stored(&mut f, feed).is_some(), "feed {feed}");
    }
    let ring = open(&mut f);
    assert_eq!((ring.kept(), ring.last_seq()), (9, Some(13)));
}

#[test]
fn a_feed_traced_twice_reads_as_the_newer() {
    let mut f = FakeJournalFlash::with_units(SECTOR, MAX_UNITS);
    let mut ring = open(&mut f);
    let (summary, body) = sealed(7);
    ring.append(&summary, &body).unwrap();
    let newer = TraceSummary {
        drive_ms: 1,
        ..summary
    };
    ring.append(&newer, &body[..10]).unwrap();
    assert_eq!(stored(&mut f, 7), Some((newer, body[..10].to_vec())));
}

#[test]
fn an_offset_past_the_body_is_invalid() {
    let mut f = FakeJournalFlash::with_units(SECTOR, MAX_UNITS);
    let mut ring = open(&mut f);
    let (summary, body) = sealed(3);
    ring.append(&summary, &body).unwrap();
    let len = u16::try_from(body.len()).unwrap();
    let mut out = [0; PAGE_MAX];
    let io = io(&mut f);
    assert_eq!(
        traces::page(&io, MAX_UNITS, 3, len, &mut out).map(|r| r.len),
        Ok(0)
    );
    assert_eq!(
        traces::page(&io, MAX_UNITS, 3, len + 1, &mut out),
        Err(Refusal::Invalid)
    );
}

#[test]
fn a_flipped_bit_hides_the_trace() {
    let mut f = FakeJournalFlash::with_units(SECTOR, MAX_UNITS);
    let mut ring = open(&mut f);
    let (summary, body) = sealed(2);
    ring.append(&summary, &body).unwrap();
    f.bytes[0][200] ^= 0x10;
    assert_eq!(stored(&mut f, 2), None);
    // Its slot stays taken: the next trace goes after it.
    let mut ring = open(&mut f);
    assert_eq!(ring.kept(), 0);
    let (summary, body) = sealed(3);
    ring.append(&summary, &body).unwrap();
    assert!(stored(&mut f, 3).is_some());
    assert!(f.bytes[0][1024..1028] == *b"PLTR");
}

/// Cut the power at every mutation of each append in a run that wraps the
/// ring twice: the trace being written is either whole or absent, every
/// trace written before it that the ring still keeps reads back whole, and
/// the next boot's append succeeds.
#[test]
fn a_power_cut_at_every_mutation_loses_at_most_the_trace_being_written() {
    let mut f = FakeJournalFlash::with_units(SECTOR, MAX_UNITS);
    // Short bodies keep the cut loops small; each append is a header of
    // 96 bytes and a body of 16.
    let short = |feed: u32| {
        let (summary, body) = sealed(feed);
        (summary, body[..16].to_vec())
    };
    for history in 1..=26u32 {
        let before = f.clone();
        let mut probe = f.clone();
        probe.mutations = 0;
        let mut ring = open(&mut probe);
        let (summary, body) = short(history);
        ring.append(&summary, &body).unwrap();
        let span = i64::from(probe.mutations);

        // Every mutation of the two programs; the erase's (4096 each) one
        // in 97.
        for cut in (0..=span).filter(|&cut| cut <= 130 || cut >= span - 130 || cut % 97 == 0) {
            let mut cut_flash = before.clone();
            let mut ring = open(&mut cut_flash);
            cut_flash.budget = cut;
            let wrote = ring.append(&summary, &body).is_ok();
            cut_flash.budget = -1;

            let mut ring = open(&mut cut_flash);
            match stored(&mut cut_flash, history) {
                Some(found) => assert_eq!(found, (summary, body.clone())),
                None => assert!(!wrote, "history {history} cut {cut}"),
            }
            let kept = ring.kept();
            for older in 1..history {
                if let Some(found) = stored(&mut cut_flash, older) {
                    assert_eq!(found, short(older), "history {history} cut {cut}");
                }
            }
            // The newest traces before this one are still there: at least
            // the four that no wrap can have reached.
            for older in history.saturating_sub(4).max(1)..history {
                assert!(
                    stored(&mut cut_flash, older).is_some(),
                    "history {history} cut {cut} lost {older}"
                );
            }
            let (next, next_body) = short(1_000);
            assert!(
                ring.append(&next, &next_body).is_ok(),
                "history {history} cut {cut}"
            );
            assert_eq!(stored(&mut cut_flash, 1_000), Some((next, next_body)));
            assert!(ring.kept() <= kept + 1);
        }

        let mut ring = open(&mut f);
        ring.append(&summary, &body).unwrap();
    }
}
