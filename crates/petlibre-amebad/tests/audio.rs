//! FW24 speaker task against the fake codec ring: the startup chime waits for
//! the settings and honors `sound`, one tune plays at a time with the
//! amplifier enabled only while the codec is open, and the ring drains
//! before everything closes.

use std::sync::{Mutex, MutexGuard};

use petlibre_amebad::audio::{self, AMP_PIN, Record, Speaker};
use petlibre_amebad::sdk::fake;
use petlibre_amebad::settings;
use petlibre_core::tone::Tune;
use petlibre_core::transport::{PlayResult, SettingsView};

/// The mailbox and settings are process-wide; the fake hardware is per thread.
static SERIAL: Mutex<()> = Mutex::new(());

fn start(sound: bool) -> MutexGuard<'static, ()> {
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    fake::reset_hardware();
    settings::reset();
    audio::reset();
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
    assert_eq!(records, vec![Record::Error { stage: "open" }]);
    assert!(!amp());
    assert_eq!(audio::request(Tune::Coin), PlayResult::Started);
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
