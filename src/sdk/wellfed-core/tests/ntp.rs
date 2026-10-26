use wellfed_core::clock::ntp::{
    ANSWER_MS, Bad, DENIED_MS, Exchange, Outcome, PACKET, POLL_MS, RETRY_MAX_MS, RETRY_MIN_MS,
    answer, request, serve,
};
use wellfed_core::clock::wall::{Fix, Origin};
use proptest::prelude::*;

// 2026-10-22T13:00:00Z.
const NOON: u64 = 1_792_674_000_000;

#[test]
fn a_request_carries_only_its_nonce() {
    let packet = request(0x0102_0304_0506_0708);
    assert_eq!(packet[0], 0x23);
    assert!(packet[1..40].iter().all(|&b| b == 0));
    assert_eq!(&packet[40..], &[1, 2, 3, 4, 5, 6, 7, 8]);
}

#[test]
fn an_answer_is_carried_to_its_arrival_with_its_error() {
    let nonce = 77;
    // The server took 2 ms; the round trip was 30 ms, so 28 ms of network.
    let reply = serve(&request(nonce), NOON, NOON + 2, 2, 10).unwrap();
    let fix = answer(&reply, nonce, 1_000, 1_030).unwrap();
    assert_eq!(
        fix,
        Fix {
            unix_ms: NOON + 2 + 14,
            // Half the delay, the dispersion and 3 ms of rounding.
            error_ms: 14 + 10 + 3,
            origin: Origin::Ntp,
        }
    );
}

#[test]
fn bad_answers_are_named() {
    let nonce = 5;
    let good = serve(&request(nonce), NOON, NOON, 2, 0).unwrap();
    assert_eq!(answer(&good[..47], nonce, 0, 1), Err(Bad::Short));
    assert_eq!(answer(&good, nonce + 1, 0, 1), Err(Bad::Origin));

    let mut client_mode = good;
    client_mode[0] = 0x23;
    assert_eq!(answer(&client_mode, nonce, 0, 1), Err(Bad::Header));

    let mut unsynchronized = good;
    unsynchronized[0] |= 0xc0;
    assert_eq!(
        answer(&unsynchronized, nonce, 0, 1),
        Err(Bad::Unsynchronized)
    );

    let mut kiss = good;
    kiss[1] = 0;
    kiss[12..16].copy_from_slice(b"RATE");
    assert_eq!(answer(&kiss, nonce, 0, 1), Err(Bad::Kiss(*b"RATE")));

    let before_2026 = serve(&request(nonce), NOON - 400 * 86_400_000, NOON, 2, 0).unwrap();
    assert_eq!(answer(&before_2026, nonce, 0, 1), Err(Bad::Time));

    // A ten-second round trip is too loose to use.
    assert_eq!(answer(&good, nonce, 0, 30_000), Err(Bad::Loose));
}

#[test]
fn era_one_timestamps_read_after_2036() {
    // 2040-01-01T01:00:00Z, past NTP's first era (February 2036).
    let later = 2_208_992_400_000;
    let reply = serve(&request(9), later, later, 2, 0).unwrap();
    assert_eq!(answer(&reply, 9, 0, 0).unwrap().unix_ms, later);
}

#[test]
fn the_exchange_paces_itself() {
    let mut exchange = Exchange::new();
    assert!(exchange.poll(0, 1).is_some());
    assert_eq!(exchange.poll(1, 2), None, "one request at a time");

    // No answer: retried after the minimum, then backed off.
    assert_eq!(exchange.expire(ANSWER_MS - 1), None);
    assert_eq!(exchange.expire(ANSWER_MS), Some(Outcome::Lost));
    let mut now = ANSWER_MS;
    let mut retry = RETRY_MIN_MS;
    for _ in 0..5 {
        assert_eq!(exchange.poll(now + retry - 1, 3), None);
        now += retry;
        assert!(exchange.poll(now, 3).is_some());
        now += ANSWER_MS;
        assert_eq!(exchange.expire(now), Some(Outcome::Lost));
        retry = (retry * 2).min(RETRY_MAX_MS);
    }

    // A forged answer is ignored while the real one is awaited.
    assert!(exchange.poll(now + RETRY_MAX_MS, 4).is_some());
    now += RETRY_MAX_MS;
    let forged = serve(&request(99), NOON, NOON, 2, 0).unwrap();
    assert_eq!(
        exchange.received(now + 5, &forged),
        Some(Outcome::Refused(Bad::Origin))
    );
    let real = serve(&request(4), NOON, NOON, 2, 0).unwrap();
    assert!(matches!(
        exchange.received(now + 10, &real),
        Some(Outcome::Fix(_))
    ));
    assert_eq!(exchange.received(now + 11, &real), None);
    assert_eq!(exchange.poll(now + 10 + POLL_MS - 1, 5), None);
    assert!(exchange.poll(now + 10 + POLL_MS, 5).is_some());
}

#[test]
fn a_denial_stops_asking_for_a_day() {
    let mut exchange = Exchange::new();
    exchange.poll(0, 1).unwrap();
    let mut deny = serve(&request(1), NOON, NOON, 2, 0).unwrap();
    deny[1] = 0;
    deny[12..16].copy_from_slice(b"DENY");
    assert_eq!(
        exchange.received(10, &deny),
        Some(Outcome::Refused(Bad::Kiss(*b"DENY")))
    );
    assert_eq!(exchange.poll(10 + DENIED_MS - 1, 2), None);
    exchange.restart(20);
    assert!(exchange.poll(20, 2).is_some());
}

proptest! {
    #[test]
    fn no_datagram_panics(bytes in proptest::collection::vec(any::<u8>(), 0..PACKET * 2), nonce: u64, sent: u32, now: u32) {
        let _ = answer(&bytes, nonce, sent, now);
        let _ = serve(&bytes, NOON, NOON, 2, 0);
    }
}
