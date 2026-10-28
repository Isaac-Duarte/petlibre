//! Speaker task against the fake codec ring: the startup chime waits for
//! the settings and honors `sound`, one tune plays at a time with the
//! amplifier enabled only while the codec is open, and the ring drains
//! before everything closes.

use std::sync::{Mutex, MutexGuard};

use wellfed_core::clock::wall::{Origin, Sync};
use wellfed_core::settings::{SettingsView, Window};
use wellfed_core::tone::PlayResult;
use wellfed_core::tone::Tune;
use wellfed_hal::audio::{self, AMP_PIN, Record, Speaker};
use wellfed_hal::plaf103::storage::CALL;
use wellfed_hal::sdk::fake;
use wellfed_hal::{clock, settings};

/// The mailbox and settings are process-wide; the fake hardware is per thread.
static SERIAL: Mutex<()> = Mutex::new(());

fn start(sound: bool) -> MutexGuard<'static, ()> {
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    fake::reset_hardware();
    settings::reset();
    audio::reset();
    clock::reset();
    publish(sound);
    guard
}

fn publish(sound: bool) {
    settings::publish(SettingsView {
        lights: true,
        sound,
        volume: 40,
        generation: 1,
        writable: true,
        ..Default::default()
    });
}

fn amp() -> bool {
    fake::with(|h| h.output[1] & (1 << (AMP_PIN - 32)) != 0)
}

/// Step the speaker, the clock and the DMA together until it logs `until`
/// or `limit` steps pass; every record on the way.
fn run(speaker: &mut Speaker, limit: usize, until: fn(&Record) -> bool) -> Vec<Record> {
    let mut records = Vec::new();
    for _ in 0..limit {
        let step = speaker.step();
        fake::with(|h| h.uptime_ms += step.delay_ms);
        // 20 ms of DMA time plays a little more than one 16 ms page.
        fake::play_audio(2);
        if let Some(record) = step.record {
            records.push(record);
            if until(&record) {
                break;
            }
        }
    }
    records
}

fn done(record: &Record) -> bool {
    matches!(record, Record::Done { .. } | Record::Error { .. })
}

#[test]
fn startup_chime_plays_once_settings_load() {
    let _guard = start(true);
    settings::reset();
    let mut speaker = Speaker::default();

    assert_eq!(speaker.step().record, None);
    assert!(!fake::with(|h| h.audio_open));

    publish(true);
    let records = run(&mut speaker, 400, done);
    assert_eq!(
        records[0],
        Record::Play {
            tune: Tune::Chime,
            volume: 40
        }
    );
    let Record::Done {
        tune,
        underruns,
        drained,
        ..
    } = records[1]
    else {
        panic!("{records:?}");
    };
    assert_eq!((tune, underruns, drained), (Tune::Chime, 0, true));

    let samples = Tune::Chime.duration_ms() as usize * 16;
    let played = fake::with(|h| h.audio_played.clone());
    assert!(played.len() >= samples);
    assert!(played.iter().any(|&s| s != 0));
    assert!(!fake::with(|h| h.audio_open));
    assert!(!amp());
    assert_eq!(fake::with(|h| h.audio_opens), 1);
}

#[test]
fn muted_startup_skips_the_chime() {
    let _guard = start(false);
    let mut speaker = Speaker::default();

    assert_eq!(
        speaker.step().record,
        Some(Record::Muted { tune: Tune::Chime })
    );
    assert_eq!(audio::request(Tune::Coin), PlayResult::Muted);
    run(&mut speaker, 20, |_| false);
    assert_eq!(fake::with(|h| h.audio_opens), 0);
}

#[test]
fn one_tune_at_a_time_with_the_amplifier_only_while_open() {
    let _guard = start(true);
    let mut speaker = Speaker::default();
    run(&mut speaker, 400, done);

    assert_eq!(audio::request(Tune::Coin), PlayResult::Started);
    assert_eq!(audio::request(Tune::Meal), PlayResult::Busy);

    let mut saw_amp = false;
    for _ in 0..400 {
        let step = speaker.step();
        fake::with(|h| h.uptime_ms += step.delay_ms);
        fake::play_audio(2);
        let open = fake::with(|h| h.audio_open);
        assert!(!amp() || open, "amplifier on with the codec closed");
        saw_amp |= amp();
        if matches!(step.record, Some(Record::Done { .. })) {
            break;
        }
    }
    assert!(saw_amp);
    assert!(!amp());
    assert_eq!(audio::request(Tune::Meal), PlayResult::Started);
}

#[test]
fn a_failed_open_frees_the_slot() {
    let _guard = start(true);
    let mut speaker = Speaker::default();
    speaker.step();
    fake::with(|h| h.audio_open_fails = true);

    let records = run(&mut speaker, 5, done);
    assert_eq!(
        records,
        vec![Record::Error {
            stage: audio::ErrorStage::Open
        }]
    );
    assert!(!amp());
    assert_eq!(audio::request(Tune::Coin), PlayResult::Started);
}

/// A clip of `bytes` stream bytes in the meal call's sectors, published.
fn store_call(bytes: usize) {
    let at = (CALL.first * 4096 + 512) as usize;
    fake::with(|h| {
        for (i, byte) in h.storage[at..at + bytes].iter_mut().enumerate() {
            // Loud codes, alternating in sign, so the clip is heard.
            *byte = if i % 2 == 0 { 0x77 } else { 0xff };
        }
    });
    audio::publish_call(Some(u32::try_from(bytes).unwrap()));
}

fn call_done(record: &Record) -> bool {
    matches!(record, Record::CallDone { .. } | Record::Error { .. })
}

#[test]
fn the_meal_call_plays_the_stored_clip_and_holds_off_the_writer() {
    let _guard = start(true);
    let mut speaker = Speaker::quiet();
    assert_eq!(audio::request_call(2), PlayResult::Busy, "no clip yet");

    store_call(400);
    assert_eq!(audio::request_call(0), PlayResult::Busy);
    assert_eq!(audio::request_call(2), PlayResult::Started);
    assert_eq!(audio::request(Tune::Coin), PlayResult::Busy);
    assert_eq!(
        speaker.step().record,
        Some(Record::CallPlay {
            plays: 2,
            ms: 200,
            volume: 40
        })
    );
    assert!(!audio::call_write_begin(), "the writer waits for the call");

    let records = run(&mut speaker, 400, call_done);
    let Some(&Record::CallDone {
        ms,
        underruns,
        drained,
    }) = records.last()
    else {
        panic!("{records:?}");
    };
    assert_eq!((underruns, drained), (0, true));
    assert!(ms > 150, "{ms}: the settle, then the clip");
    let played = fake::with(|h| h.audio_played.clone());
    assert!(played.len() >= 200 * 16);
    assert!(played.iter().any(|&s| s != 0));
    assert!(!amp());

    assert!(audio::call_write_begin());
    audio::call_write_end();
    assert_eq!(audio::request(Tune::Coin), PlayResult::Started);
}

#[test]
fn a_call_while_the_writer_holds_the_clip_plays_nothing() {
    let _guard = start(true);
    let mut speaker = Speaker::quiet();
    store_call(400);
    assert_eq!(audio::request_call(1), PlayResult::Started);
    assert!(audio::call_write_begin());

    assert_eq!(
        speaker.step().record,
        Some(Record::Error {
            stage: audio::ErrorStage::Clip
        })
    );
    assert!(!fake::with(|h| h.audio_open));
    assert_eq!(fake::with(|h| h.audio_opens), 0);
    audio::call_write_end();
    assert_eq!(audio::request(Tune::Coin), PlayResult::Started);
}

#[test]
fn a_muted_feeder_plays_no_call() {
    let _guard = start(false);
    store_call(400);
    assert_eq!(audio::request_call(1), PlayResult::Muted);
}

#[test]
fn a_stalled_ring_closes_after_the_drain_bound() {
    let _guard = start(true);
    let mut speaker = Speaker::default();
    speaker.step();

    let mut records = Vec::new();
    for _ in 0..400 {
        let step = speaker.step();
        fake::with(|h| h.uptime_ms += step.delay_ms);
        // The DMA never runs: pages fill once, then nothing drains.
        if let Some(record) = step.record {
            records.push(record);
            if done(&record) {
                break;
            }
        }
    }
    assert!(
        matches!(records.last(), Some(Record::Done { drained: false, .. })),
        "{records:?}"
    );
    assert!(!fake::with(|h| h.audio_open));
    assert!(!amp());
}

#[test]
fn records_describe_without_surprises() {
    let mut line = String::new();
    audio::describe(
        Record::Done {
            tune: Tune::Coin,
            ms: 620,
            underruns: 0,
            drained: true,
        },
        &mut line,
    )
    .unwrap();
    assert_eq!(line, "done tune=coin ms=620 underruns=0 drained=1");
}

#[test]
fn volume_zero_mutes_a_request() {
    let _guard = start(true);
    settings::publish(SettingsView {
        lights: true,
        sound: true,
        volume: 0,
        generation: 1,
        writable: true,
        ..Default::default()
    });

    assert_eq!(audio::request(Tune::Coin), PlayResult::Muted);
}

#[test]
fn the_sound_window_mutes_outside_it_and_opens_without_a_clock() {
    let _guard = start(true);
    // 08:00-20:00 local.
    let view = SettingsView {
        lights: true,
        sound: true,
        volume: 40,
        sound_window: Window::Between {
            start: 8 * 60,
            end: 20 * 60,
        },
        generation: 1,
        writable: true,
        ..Default::default()
    };
    settings::publish(view);

    // No clock yet: the window is open.
    assert_eq!(audio::request(Tune::Coin), PlayResult::Started);
    audio::reset();

    // 2026-10-28 22:00 local, 04:00 UTC the next day at UTC-6.
    clock::sync(
        Sync {
            unix_ms: 1_793_246_400_000,
            offset_s: -6 * 3600,
            next_change_s: 0,
            next_offset_s: -6 * 3600,
        },
        Origin::Owner,
    )
    .unwrap();
    assert_eq!(audio::request(Tune::Coin), PlayResult::Muted);

    settings::publish(SettingsView {
        sound_window: Window::Between {
            start: 21 * 60,
            end: 23 * 60,
        },
        ..view
    });
    assert_eq!(audio::request(Tune::Coin), PlayResult::Started);
}
