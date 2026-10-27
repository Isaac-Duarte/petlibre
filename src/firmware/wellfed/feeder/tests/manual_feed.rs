//! Manual feeding through the loop on the board's platform, with a fake
//! feeding owner: a Dispense press the panel allows queues one portion
//! through the path a client's feed takes; readiness refuses a press and
//! records its lasting reasons; a feed plays no tune; and a tune a client
//! asked for (the real speaker on the fake codec) never holds up a feed,
//! its cancel or its result, whether it plays, fails to open or stalls.

use std::sync::{Mutex, MutexGuard};

use wellfed_core::buttons::{Button, Buttons, Gesture};
use wellfed_core::tone::{PlayResult, Tune};
use wellfed_device_api::Device;
use wellfed_device_api::mailbox::SettingsView;
use wellfed_feeder::feeder::owner::POLL_MS;
use wellfed_feeder::feeder::{Feeder, Feeds, Record};
use wellfed_feeder_runtime::events;
use wellfed_fw_feeder::platform::{Driver, Plaf103};
use wellfed_proto::api::{
    Battery, Blocked, EventBody, FeedOutcome, FeedReason, Readiness, Refusal, Source, Supply,
};
use wellfed_sdk::hal::power::{self, Monitor};
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
    below_floor: Option<bool>,
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

    fn below_floor(&self) -> Option<bool> {
        self.below_floor
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
    /// Sound on, so a tune that should not play would.
    fn new() -> Self {
        let serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        fake::reset_hardware();
        buttons::reset();
        settings::reset();
        audio::reset();
        events::reset();
        settings::publish(SettingsView {
            lights: true,
            sound: true,
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
    let mut rig = Rig::new();
    rig.press_dispense();
    assert!(rig.owner.submitted.is_empty(), "the panel starts locked");

    rig.unlock();
    rig.press_dispense();
    assert_eq!(rig.owner.submitted, [(1, Source::Panel)]);
    assert!(rig.logged("PLFEED1 dispense feed=1"), "{:?}", rig.lines);

    // Sound is on, yet the feed plays nothing: the speaker stays closed.
    for _ in 0..200 {
        rig.pass();
        fake::play_audio(2);
    }
    assert!(
        !rig.lines.iter().any(|l| l.starts_with("PLAUD1")),
        "a feed plays no tune: {:?}",
        rig.lines
    );
    assert_eq!(audio::request(Tune::Coin), PlayResult::Started);
}

#[test]
fn a_press_obeys_readiness_and_a_lasting_reason_is_recorded() {
    let mut rig = Rig::new();
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

#[test]
fn a_client_feed_plays_no_tune() {
    let mut rig = Rig::new();
    let by = Source::Client([7; 4]);

    let feed = rig.feeder.device(rig.now, &mut rig.owner).feed(1, by);
    assert_eq!(feed, Ok(1));
    assert_eq!(rig.owner.submitted, [(1, by)]);
    for _ in 0..200 {
        rig.pass();
        fake::play_audio(2);
    }

    assert!(
        !rig.lines.iter().any(|l| l.starts_with("PLAUD1")),
        "a feed plays no tune: {:?}",
        rig.lines
    );
    assert_eq!(audio::request(Tune::Coin), PlayResult::Started);
}

/// A feed's path while the speaker is busy with a requested tune: queued,
/// cancelled through the device API, and its `Fed` event recorded, with the
/// loop's delay never longer than its poll.
fn feed_despite_the_speaker(rig: &mut Rig) {
    rig.unlock();
    rig.press_dispense();
    let feed = u32::try_from(rig.owner.submitted.len()).unwrap();
    assert_eq!(rig.owner.submitted.last(), Some(&(1, Source::Panel)));

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
fn a_playing_tune_keeps_playing_and_the_feed_goes_on() {
    let mut rig = Rig::new();
    assert_eq!(audio::request(Tune::Chime), PlayResult::Started);

    feed_despite_the_speaker(&mut rig);
    for _ in 0..200 {
        rig.pass();
        fake::play_audio(2);
    }

    let tunes: Vec<_> = rig
        .lines
        .iter()
        .filter(|l| l.starts_with("PLAUD1 play"))
        .collect();
    assert_eq!(
        tunes,
        ["PLAUD1 play tune=chime volume=50"],
        "{:?}",
        rig.lines
    );
}

#[test]
fn a_codec_that_will_not_open_holds_up_nothing() {
    let mut rig = Rig::new();
    fake::with(|h| h.audio_open_fails = true);
    assert_eq!(audio::request(Tune::Coin), PlayResult::Started);

    feed_despite_the_speaker(&mut rig);
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
    let mut rig = Rig::new();
    assert_eq!(audio::request(Tune::Coin), PlayResult::Started);

    // The DMA never takes a page: the tune cannot finish.
    feed_despite_the_speaker(&mut rig);
    for _ in 0..300 {
        rig.pass();
    }
    assert!(
        rig.lines
            .iter()
            .any(|l| l.starts_with("PLAUD1 done tune=coin")),
        "the speaker gives up on its own bound: {:?}",
        rig.lines
    );
}

/// The owner's battery reason refuses a press like any passing reason:
/// logged, refused `Busy`, never a `FeedingBlocked` event.
#[test]
fn a_press_on_a_low_pack_is_refused_and_not_recorded_as_blocked() {
    let mut rig = Rig::new();
    rig.unlock();
    rig.owner.blocked = Some(Blocked::Battery);
    rig.press_dispense();
    assert!(rig.owner.submitted.is_empty());
    assert!(rig.logged("PLFEED1 readiness state=blocked reason=battery"));
    assert!(
        rig.logged("PLFEED1 dispense refused=busy"),
        "{:?}",
        rig.lines
    );
    assert!(
        !rig.events()
            .iter()
            .any(|body| matches!(body, EventBody::FeedingBlocked { .. }))
    );
}

/// The board's own samples end to end: the supply as the power monitor
/// publishes it, judged, its first reading a baseline and the adapter's
/// loss a `Supply` event with the owner's verdict.
#[test]
fn the_adapter_leaving_is_a_supply_event_with_the_pack_reading() {
    let mut rig = Rig::new();
    power::reset();
    let mut monitor = Monitor::default();
    // A pack: 2117 mV at the pin with the default calibration, 4.37 V.
    fake::with(|h| {
        h.input[0] |= 1 << 19;
        h.battery_adc = 2800;
    });
    monitor.step();
    monitor.step();
    rig.owner.below_floor = Some(false);
    rig.pass();
    let _ = rig.events();

    fake::with(|h| h.input[0] &= !(1 << 19));
    monitor.step();
    rig.pass();
    let events = rig.events();
    let supply: Vec<_> = events
        .iter()
        .filter_map(|body| match body {
            EventBody::Supply(supply) => Some(*supply),
            _ => None,
        })
        .collect();
    let [
        Supply {
            adapter: Some(false),
            battery: Battery::Reading { mv },
            below_floor: Some(false),
        },
    ] = supply.as_slice()
    else {
        panic!("{events:?}");
    };
    assert!((4000..5000).contains(mv), "{mv}");
}
