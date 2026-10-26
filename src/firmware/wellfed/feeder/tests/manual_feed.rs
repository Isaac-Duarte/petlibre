//! Manual feeding through the loop on the board's platform, with a fake
//! feeding owner: a Dispense press the panel allows queues one portion
//! through the path a client's feed takes; readiness refuses a press and
//! records its lasting reasons; and the feed call (the real speaker on the
//! fake codec) never holds up a feed, its cancel or its result, whether it
//! plays, is muted, finds the speaker busy, fails to open or stalls.

use std::sync::{Mutex, MutexGuard};

use wellfed_core::buttons::{Button, Buttons, Gesture};
use wellfed_device_api::Device;
use wellfed_device_api::mailbox::SettingsView;
use wellfed_feeder::feeder::owner::POLL_MS;
use wellfed_feeder::feeder::{Feeder, Feeds, Record};
use wellfed_feeder_runtime::events;
use wellfed_fw_feeder::platform::{Driver, Plaf103};
use wellfed_proto::api::{
    Blocked, EventBody, FeedOutcome, FeedReason, Readiness, Refusal, Source,
};
use wellfed_sdk::hal::sdk::fake;
use wellfed_sdk::hal::{audio, buttons, settings};

/// The button queue, the settings, the speaker's slot and the event queue
/// are process-wide.
static SERIAL: Mutex<()> = Mutex::new(());

/// A feeding owner that takes every feed it is handed, unless told why not.
#[derive(Default)]
struct Owner {
    submitted: Vec<(u8, Source)>,
    blocked: Option<Blocked>,
}

impl Feeds for Owner {
    fn feeding(&self) -> bool {
        false
    }

    fn busy(&self) -> bool {
        false
    }

    fn blocked(&self) -> Option<Blocked> {
        self.blocked
    }

    fn submit(&mut self, portions: u8, by: Source) -> Result<u32, Refusal> {
        self.submitted.push((portions, by));
        Ok(u32::try_from(self.submitted.len()).unwrap())
    }

    fn reserved(&self, _feed: u32) -> Option<Result<(), Refusal>> {
        Some(Ok(()))
    }

    fn cancel(&self, _feed: u32) -> bool {
        true
    }
}

struct Rig {
    _serial: MutexGuard<'static, ()>,
    feeder: Box<Feeder<Plaf103>>,
    owner: Owner,
    now: u32,
    lines: Vec<String>,
}

impl Rig {
    fn new(sound: bool) -> Self {
        let serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        fake::reset_hardware();
        buttons::reset();
        settings::reset();
        audio::reset();
        events::reset();
        settings::publish(SettingsView {
            lights: true,
            sound,
            volume: 50,
            generation: 1,
            writable: true,
        });
        let mut rig = Self {
            _serial: serial,
            feeder: Box::new(Feeder::new(Plaf103::new())),
            owner: Owner::default(),
            now: 0,
            lines: Vec::new(),
        };
        rig.pass();
        rig
    }

    /// One pass `POLL_MS` on, with `gestures` queued; its delay.
    fn pass_with(&mut self, gestures: &[Gesture]) -> u32 {
        for &gesture in gestures {
            buttons::inject(gesture);
        }
        self.now += POLL_MS;
        fake::with(|h| h.uptime_ms = self.now);
        let lines = &mut self.lines;
        self.feeder
            .pass(self.now, &mut self.owner, |r: Record<Driver>| {
                lines.push(format!("{} {r}", r.tag()));
            })
    }

    fn pass(&mut self) -> u32 {
        self.pass_with(&[])
    }

    fn unlock(&mut self) {
        let hold = Gesture::Hold {
            chord: Buttons::of(Button::Lock),
            level: 1,
        };
        self.pass_with(&[hold]);
        assert_eq!(self.feeder.lock().map(|l| l.locked()), Some(false));
    }

    fn press_dispense(&mut self) {
        let chord = Buttons::of(Button::Dispense);
        let up = Gesture::Up {
            chord,
            held_ms: 100,
            level: 0,
        };
        self.pass_with(&[Gesture::Down { chord }, up]);
    }

    fn logged(&self, line: &str) -> bool {
        self.lines.iter().any(|l| l == line)
    }

    /// The queued events' bodies, oldest first, as the writer would take
    /// them.
    fn events(&self) -> Vec<EventBody> {
        let mut bodies = Vec::new();
        while let Some(queued) = events::pending() {
            bodies.push(wellfed_proto::decode_padded(&queued.body).unwrap());
            events::finish_pending(None);
        }
        bodies
    }
}

#[test]
fn a_dispense_press_queues_one_portion_through_the_owner() {
    let mut rig = Rig::new(true);
    rig.press_dispense();
    assert!(rig.owner.submitted.is_empty(), "the panel starts locked");

    rig.unlock();
    rig.press_dispense();
    assert_eq!(rig.owner.submitted, [(1, Source::Panel)]);
    assert!(rig.logged("PLFEED1 dispense feed=1"), "{:?}", rig.lines);
    assert!(rig.logged("PLFEED1 call feed=1 result=started"));

    // The call plays on, stepped by the loop, then the speaker closes.
    for _ in 0..200 {
        rig.pass();
        fake::play_audio(2);
    }
    assert!(
        rig.lines
            .iter()
            .any(|l| l.starts_with("PLAUD1 done tune=meal")),
        "{:?}",
        rig.lines
    );
}

#[test]
fn a_press_obeys_readiness_and_a_lasting_reason_is_recorded() {
    let mut rig = Rig::new(true);
    rig.unlock();
    assert!(rig.logged("PLFEED1 readiness state=ready"));

    rig.owner.blocked = Some(Blocked::Fault);
    rig.press_dispense();
    assert!(rig.owner.submitted.is_empty());
    assert!(rig.logged("PLFEED1 readiness state=blocked reason=fault"));
    assert!(rig.logged("PLFEED1 dispense refused=fault"));

    // A passing reason is logged but never recorded.
    rig.owner.blocked = Some(Blocked::Joining);
    rig.pass();
    rig.owner.blocked = None;
    rig.pass();
    assert!(rig.logged("PLFEED1 readiness state=blocked reason=joining"));

    let changes: Vec<_> = rig
        .events()
        .into_iter()
        .filter(|body| matches!(body, EventBody::FeedingBlocked { .. }))
        .collect();
    assert_eq!(
        changes,
        [
            EventBody::FeedingBlocked {
                reason: Some(Blocked::Fault)
            },
            EventBody::FeedingBlocked { reason: None },
        ]
    );

    // The device API's view is the same decision.
    rig.owner.blocked = Some(Blocked::Resolving);
    let readiness = rig.feeder.device(rig.now, &mut rig.owner).readiness();
    assert_eq!(readiness, Readiness::Blocked(Blocked::Resolving));
}

/// A feed's path past the call: queued, cancelled through the device API,
/// and its `Fed` event recorded, with the loop's delay never longer than
/// its poll.
fn feed_despite_the_speaker(rig: &mut Rig, call: &str) {
    rig.unlock();
    rig.press_dispense();
    let feed = u32::try_from(rig.owner.submitted.len()).unwrap();
    assert_eq!(rig.owner.submitted.last(), Some(&(1, Source::Panel)));
    assert!(
        rig.logged(&format!("PLFEED1 call feed={feed} result={call}")),
        "{:?}",
        rig.lines
    );

    for _ in 0..10 {
        assert!(rig.pass() <= POLL_MS, "the speaker never holds the loop");
    }
    assert!(rig.feeder.device(rig.now, &mut rig.owner).cancel_feed(feed));
    let fed = EventBody::Fed {
        feed,
        requested: 1,
        completed: 0,
        outcome: FeedOutcome::Cancelled,
        reason: FeedReason::Cancelled,
        by: Source::Panel,
    };
    rig.feeder.record_outcome(rig.now, fed);
    assert!(rig.events().contains(&fed));
}

#[test]
fn a_muted_call_holds_up_nothing() {
    let mut rig = Rig::new(false);
    feed_despite_the_speaker(&mut rig, "muted");
}

#[test]
fn a_busy_speaker_keeps_its_tune_and_the_feed_goes_on() {
    let mut rig = Rig::new(true);
    assert_eq!(
        audio::request(wellfed_core::tone::Tune::Chime),
        wellfed_core::tone::PlayResult::Started
    );
    feed_despite_the_speaker(&mut rig, "busy");
}

#[test]
fn a_codec_that_will_not_open_holds_up_nothing() {
    let mut rig = Rig::new(true);
    fake::with(|h| h.audio_open_fails = true);
    feed_despite_the_speaker(&mut rig, "started");
    assert!(
        rig.lines
            .iter()
            .any(|l| l.starts_with("PLAUD1 error stage=open")),
        "{:?}",
        rig.lines
    );
}

#[test]
fn a_stalled_ring_holds_up_nothing_and_still_ends() {
    let mut rig = Rig::new(true);
    // The DMA never takes a page: the tune cannot finish.
    feed_despite_the_speaker(&mut rig, "started");
    for _ in 0..300 {
        rig.pass();
    }
    assert!(
        rig.lines
            .iter()
            .any(|l| l.starts_with("PLAUD1 done tune=meal")),
        "the speaker gives up on its own bound: {:?}",
        rig.lines
    );
}
