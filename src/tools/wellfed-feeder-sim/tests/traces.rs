//! Per-feed traces end to end: a three-portion drive of the measured
//! mechanism (`wellfed_sim::plant`) recorded by the product's recorder as
//! the firmware's loop records one, appended to the storage ring as its
//! settings writer appends it, then read by a client over a session a page
//! at a time, before and after a restart.

use std::rc::Rc;

use tokio::task::LocalSet;
use wellfed_client::trace::{Pages, TraceError};
use wellfed_connect::Connection;
use wellfed_core::adc_scale::{AdcCalibration, SAMPLES};
use wellfed_core::feed::controller::MotorCommand;
use wellfed_core::feed::observe::{Beams, Observation};
use wellfed_feeder::trace::{DriveFigures, Recorder};
use wellfed_feeder_sim::flash::Flash;
use wellfed_feeder_sim::harness;
use wellfed_model::{
    Answer, FeedOutcome, Op, Refusal, Stats, StatsSection, Trace, TraceEntry, TraceMotor,
};
use wellfed_proto::api::{self as proto, EventBody};
use wellfed_session::noise::Keypair;
use wellfed_sim::plant::{Plant, PlantConfig, Turn};

const OWNER: [u8; 32] = [0x21; 32];
/// The supervisor's clock at the drive's start.
const DRIVE_AT: u32 = 40_000;
/// The owner's poll as the bench measured it (903 polls in 27.2 s).
const POLL_MS: u32 = 30;
const FEED: u32 = 9;

/// A drive of the plant until its third mark's rise, polled as the owner
/// polls it; the sealed trace, and every edge the plant made (from the
/// drive's start, and the level after).
fn three_portions() -> (proto::TraceSummary, Vec<u8>, Vec<(u32, bool)>) {
    let config = PlantConfig::default();
    let calibration = AdcCalibration {
        offset: 2480,
        gain: 12050,
        ..AdcCalibration::default()
    };
    let scaled = |raw: u16| calibration.scale_16(&[raw; SAMPLES]).unwrap();
    let mut plant = Plant::new(config, 0);
    let mut recorder = Recorder::new();
    recorder.begin(FEED);
    recorder.observe(&Observation {
        at_ms: DRIVE_AT - 25,
        battery_mv: Some(4_810),
        ..Observation::default()
    });

    let mut edges = Vec::new();
    let (mut rising, mut falling) = (0u16, 0u16);
    let (mut peak, mut total, mut samples) = (0, 0u64, 0u32);
    let mut ms = 0u32;
    let stop = loop {
        ms += 1;
        for flip in plant.drive(Turn::Forward) {
            if flip.captured {
                edges.push((ms, flip.high));
                if flip.high {
                    rising += 1;
                } else {
                    falling += 1;
                }
            }
        }
        // The owner stops on the third rise, at its next poll. Its first
        // poll came 17 ms into the bench's drives, inside the surge.
        let done = rising == 3;
        if ms % POLL_MS == 17 || done {
            let raw = if ms < config.surge_ms {
                config.surge_adc
            } else {
                plant.drive_reading()
            };
            let current = scaled(raw);
            peak = peak.max(current);
            total += u64::from(current);
            samples += 1;
            recorder.observe(&Observation {
                at_ms: DRIVE_AT + ms,
                drive_at_ms: Some(DRIVE_AT),
                current: Some(current),
                battery_mv: Some(4_700),
                beams: Some(Beams {
                    outlet_clear: true,
                    food: false,
                }),
                rising,
                falling,
                last_edge: edges.last().map(|&(at, high)| (DRIVE_AT + at, high)),
                motor: if done {
                    MotorCommand::Stop
                } else {
                    MotorCommand::Drive0
                },
            });
        }
        if done {
            break ms;
        }
    };

    let fed = EventBody::Fed {
        feed: FEED,
        requested: 3,
        completed: 3,
        outcome: proto::FeedOutcome::Done,
        reason: proto::FeedReason::Done,
        hopper: proto::FoodLevel::Present,
        by: proto::Source::Panel,
    };
    let figures = DriveFigures {
        drive_ms: stop,
        samples,
        current_peak: peak,
        current_mean: u32::try_from(total / u64::from(samples)).ok(),
        rising,
        falling,
        first_edge_ms: edges.first().map(|&(at, _)| at),
        reversals: 0,
    };
    let summary = recorder.seal(&fed, &figures).unwrap();
    (summary, recorder.body().to_vec(), edges)
}

async fn open(sim: &harness::Running) -> Connection {
    let keys = Keypair::from_secret(OWNER);
    Connection::open(&keys, sim.identity, sim.address, [0xa1; 32])
        .await
        .unwrap()
}

/// Feed `feed`'s trace, a page at a time, as `wellfed trace` reads it.
async fn read(connection: &mut Connection, ids: &mut u8, feed: u32) -> Result<Trace, TraceError> {
    let mut pages = Pages::new(feed);
    let mut asked = 0;
    loop {
        *ids += 1;
        asked += 1;
        let answer = connection.ask([*ids; 16], &pages.next_op()).await.unwrap();
        if let Some(trace) = pages.take(answer)? {
            assert_eq!(asked, 2, "a three-portion trace is two pages");
            return Ok(trace);
        }
    }
}

#[test]
fn a_feed_trace_reads_back_over_the_api_before_and_after_a_restart() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(LocalSet::new().run_until(async {
        let (summary, body, edges) = three_portions();
        assert!(body.len() > wellfed_proto::trace::PAGE_MAX);
        let flash: Rc<Flash> = Flash::memory();
        assert_eq!(harness::store_trace(&flash, &summary, &body), Ok(1));

        for boot in 0..2 {
            let sim =
                harness::Running::start(&flash, Some(*Keypair::from_secret(OWNER).public())).await;
            let mut owner = open(&sim).await;
            assert!(owner.capabilities().traces && owner.capabilities().stats);
            let mut ids = 0;

            let trace = read(&mut owner, &mut ids, FEED).await.unwrap();
            assert!(trace.stored, "boot {boot}");
            assert_eq!(trace.summary, summary.into());
            assert_eq!(trace.summary.outcome, FeedOutcome::Done);
            assert_eq!((trace.summary.rising, trace.summary.falling), (3, 3));
            let marks: Vec<(u32, bool)> = trace
                .entries
                .iter()
                .filter_map(|entry| match *entry {
                    TraceEntry::Mark {
                        at_ms,
                        edges: 1,
                        high,
                    } => Some((at_ms, high)),
                    _ => None,
                })
                .collect();
            assert_eq!(marks, edges, "every PA18 edge at its millisecond");
            let motors: Vec<TraceMotor> = trace
                .entries
                .iter()
                .filter_map(|entry| match *entry {
                    TraceEntry::Motor { motor, .. } => Some(motor),
                    _ => None,
                })
                .collect();
            assert_eq!(motors, [TraceMotor::Drive0, TraceMotor::Stop]);
            let first = trace.entries.iter().find_map(|entry| match *entry {
                TraceEntry::Current { at_ms, value } => Some((at_ms, value)),
                _ => None,
            });
            assert_eq!(
                first,
                Some((17, summary.current_peak)),
                "the start-up surge"
            );

            ids += 1;
            let missing = owner
                .ask([ids; 16], &Op::FeedTrace { feed: 8, offset: 0 })
                .await
                .unwrap();
            assert_eq!(missing, Answer::Refused(Refusal::NotFound));
            ids += 1;
            let past = owner
                .ask(
                    [ids; 16],
                    &Op::FeedTrace {
                        feed: FEED,
                        offset: u16::MAX,
                    },
                )
                .await
                .unwrap();
            assert_eq!(past, Answer::Refused(Refusal::Invalid));

            ids += 1;
            let section = StatsSection::Feeds;
            let answer = owner.ask([ids; 16], &Op::Stats { section }).await.unwrap();
            let Answer::Stats(Stats::Feeds(feeds)) = answer else {
                panic!("{answer:?}");
            };
            assert_eq!((feeds.kept, feeds.capacity, feeds.traced), (1, 8, 0));
            sim.stop().await;
        }
    }));
}
