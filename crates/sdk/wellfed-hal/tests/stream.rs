//! Speaker variant stream task against fake sockets and the fake codec ring:
//! only a signed handshake opens the output, PCM reaches the DMA scaled by
//! the volume, and every ending (close, stall, refusal) leaves the amplifier
//! off, the codec closed and the output free for tunes.

use std::sync::{Mutex, MutexGuard};

use wellfed_core::speaker_stream::{HELLO_LEN, Status, challenge, header};
use wellfed_core::tone::{Tune, gain_q15};
use wellfed_core::transport::{PlayResult, SettingsView};
use wellfed_hal::audio::{self, AMP_PIN};
use wellfed_hal::sdk::fake::api::FAKE_REQUEST_KEY;
use wellfed_hal::sdk::fake::{self, TcpRecvScript};
use wellfed_hal::settings;
use wellfed_hal::stream::{EndReason, Record, STALL_MS, Streamer};

/// The audio slot and settings are process-wide; the fake hardware is per thread.
static SERIAL: Mutex<()> = Mutex::new(());
const SOCKET: i32 = 7;

fn start(sound: bool) -> (MutexGuard<'static, ()>, Streamer) {
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    fake::reset_hardware();
    settings::reset();
    audio::reset();
    settings::publish(SettingsView {
        lights: true,
        sound,
        volume: 100,
        generation: 1,
        writable: true,
    });
    let mut streamer = Streamer::default();
    assert_eq!(streamer.step().record, Some(Record::Listen { ok: true }));
    (guard, streamer)
}

fn amp() -> bool {
    fake::with(|h| h.output[1] & (1 << (AMP_PIN - 32)) != 0)
}

/// Accept a connection and return the challenge from the hello it was sent.
fn connect(streamer: &mut Streamer) -> [u8; 16] {
    fake::with(|h| h.tcp_accepts.push_back(SOCKET));
    assert_eq!(streamer.step().record, None);
    let hello = fake::with(|h| h.tcp_tx.last().unwrap().1.clone());
    let hello: [u8; HELLO_LEN] = hello.try_into().unwrap();
    challenge(&hello).unwrap()
}

fn rx(script: TcpRecvScript) {
    fake::with(|h| h.tcp_rx.push_back(script));
}

/// Step with the clock and DMA running until a record satisfies `until`.
fn run(streamer: &mut Streamer, limit: usize, until: fn(&Record) -> bool) -> Vec<Record> {
    let mut records = Vec::new();
    for _ in 0..limit {
        let step = streamer.step();
        fake::with(|h| h.uptime_ms += step.delay_ms);
        fake::play_audio(1);
        if let Some(record) = step.record {
            records.push(record);
            if until(&record) {
                break;
            }
        }
    }
    records
}

fn ended(record: &Record) -> bool {
    matches!(
        record,
        Record::Done { .. } | Record::Refused { .. } | Record::Error { .. }
    )
}

fn status_sent() -> Option<u8> {
    fake::with(|h| {
        h.tcp_tx
            .iter()
            .rev()
            .find(|(_, data)| data.len() == 1)
            .map(|(_, data)| data[0])
    })
}

#[test]
fn a_signed_stream_plays_every_sample_then_closes() {
    let (_guard, mut streamer) = start(true);
    let challenge = connect(&mut streamer);

    let samples: Vec<i16> = (0..20_000)
        .map(|i| ((i % 200) * 100 - 10_000) as i16)
        .collect();
    let pcm: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    rx(TcpRecvScript::Data(
        header(&FAKE_REQUEST_KEY, &challenge, 32_000).to_vec(),
    ));
    for chunk in pcm.chunks(1460) {
        rx(TcpRecvScript::Data(chunk.to_vec()));
    }
    rx(TcpRecvScript::Closed);

    let records = run(&mut streamer, 2_000, ended);
    assert_eq!(
        records[0],
        Record::Open {
            rate: 32_000,
            volume: 100
        }
    );
    let Some(Record::Done {
        bytes,
        reason,
        underruns,
        ..
    }) = records.last().copied()
    else {
        panic!("{records:?}");
    };
    assert_eq!(
        (bytes as usize, reason.as_str(), underruns),
        (pcm.len(), "closed", 0)
    );
    assert_eq!(status_sent(), Some(Status::Ok as u8));

    let gain = gain_q15(100);
    let played = fake::with(|h| h.audio_played.clone());
    let expected: Vec<i16> = samples
        .iter()
        .map(|&s| ((i32::from(s) * gain) >> 15) as i16)
        .collect();
    assert_eq!(&played[..expected.len()], &expected[..]);
    assert!(played[expected.len()..].iter().all(|&s| s == 0));

    assert_eq!(fake::with(|h| h.audio_rate), 32_000);
    assert!(!fake::with(|h| h.audio_open));
    assert!(!amp());
    assert!(fake::with(|h| h.tcp_closed.contains(&SOCKET)));
    assert_eq!(audio::request(Tune::Coin), PlayResult::Started);
}

#[test]
fn a_wrong_key_is_refused_without_opening_the_output() {
    let (_guard, mut streamer) = start(true);
    let challenge = connect(&mut streamer);
    rx(TcpRecvScript::Data(
        header(&[1; 32], &challenge, 16_000).to_vec(),
    ));

    let records = run(&mut streamer, 50, ended);
    assert_eq!(
        records,
        vec![Record::Refused {
            status: Status::Unauthenticated
        }]
    );
    assert_eq!(status_sent(), Some(Status::Unauthenticated as u8));
    assert_eq!(fake::with(|h| h.audio_opens), 0);
    assert!(fake::with(|h| h.tcp_closed.contains(&SOCKET)));
}

#[test]
fn muted_and_busy_refuse_signed_streams() {
    let (_guard, mut streamer) = start(false);
    let challenge = connect(&mut streamer);
    rx(TcpRecvScript::Data(
        header(&FAKE_REQUEST_KEY, &challenge, 16_000).to_vec(),
    ));
    assert_eq!(
        run(&mut streamer, 50, ended),
        vec![Record::Refused {
            status: Status::Muted
        }]
    );

    settings::publish(SettingsView {
        lights: true,
        sound: true,
        volume: 50,
        generation: 2,
        writable: true,
    });
    assert_eq!(audio::request(Tune::Coin), PlayResult::Started);
    let challenge = connect(&mut streamer);
    rx(TcpRecvScript::Data(
        header(&FAKE_REQUEST_KEY, &challenge, 16_000).to_vec(),
    ));
    assert_eq!(
        run(&mut streamer, 50, ended),
        vec![Record::Refused {
            status: Status::Busy
        }]
    );
    assert_eq!(fake::with(|h| h.audio_opens), 0);
}

#[test]
fn a_silent_client_is_dropped_and_the_output_freed() {
    let (_guard, mut streamer) = start(true);
    let challenge = connect(&mut streamer);
    rx(TcpRecvScript::Data(
        header(&FAKE_REQUEST_KEY, &challenge, 16_000).to_vec(),
    ));
    rx(TcpRecvScript::Data(vec![0x10; 4000]));

    let begin = fake::with(|h| h.uptime_ms);
    let records = run(&mut streamer, 5_000, ended);
    assert!(
        matches!(
            records.last(),
            Some(Record::Done {
                reason: EndReason::Stall,
                bytes: 4000,
                ..
            })
        ),
        "{records:?}"
    );
    assert!(fake::with(|h| h.uptime_ms) - begin >= STALL_MS);
    assert!(!fake::with(|h| h.audio_open));
    assert!(!amp());
    assert_eq!(audio::request(Tune::Coin), PlayResult::Started);
}

#[test]
fn a_handshake_that_never_arrives_times_out() {
    let (_guard, mut streamer) = start(true);
    connect(&mut streamer);
    let records = run(&mut streamer, 1_000, ended);
    assert_eq!(
        records,
        vec![Record::Refused {
            status: Status::BadHeader
        }]
    );
    assert_eq!(fake::with(|h| h.audio_opens), 0);
}
