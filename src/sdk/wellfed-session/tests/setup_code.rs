//! The setup code's press-by-press proof: matching codes under one handshake
//! prove each side to the other; another code, another handshake or a
//! reflected message fails at the first press it touches.

use wellfed_session::setup_code::Button;
use wellfed_session::setup_code::{Code, Error, Exchange, PRESSES, Round, Side};

const HASH: [u8; 32] = [7; 32];

fn code() -> Code {
    Code::from_bits(0b11_10_01_00_00_01_10_11)
}

/// Run both sides to the end, or to the first failure; which side failed,
/// and at which of its reads.
fn run(feeder: &mut Exchange, client: &mut Exchange) -> Result<(), (Side, usize, Error)> {
    for j in 0..=PRESSES {
        let ours = feeder.write().unwrap();
        client.read(ours).map_err(|e| (Side::Client, j, e))?;
        let theirs = client.write().unwrap();
        feeder.read(theirs).map_err(|e| (Side::Feeder, j, e))?;
    }
    Ok(())
}

#[test]
fn the_same_code_under_one_handshake_proves_both_sides() {
    let mut feeder = Exchange::new(Side::Feeder, HASH, code(), [1; 32]);
    let mut client = Exchange::new(Side::Client, HASH, code(), [2; 32]);
    assert!(feeder.is_my_turn());
    assert!(!client.is_my_turn());

    for _ in 0..PRESSES {
        client.read(feeder.write().unwrap()).unwrap();
        feeder.read(client.write().unwrap()).unwrap();
    }
    client.read(feeder.write().unwrap()).unwrap();

    // The client knows before its last message, which carries the Wi-Fi.
    assert!(client.peer_proven());
    assert!(!feeder.peer_proven());
    feeder.read(client.write().unwrap()).unwrap();
    assert!(feeder.peer_proven());
    assert!(!feeder.is_my_turn() && !client.is_my_turn());
}

#[test]
fn one_wrong_press_fails_at_that_press() {
    for at in 0..PRESSES {
        let mut presses = *code().presses();
        presses[at] = if presses[at] == Button::Lock {
            Button::Mic
        } else {
            Button::Lock
        };
        let mut feeder = Exchange::new(Side::Feeder, HASH, Code::new(presses), [1; 32]);
        let mut client = Exchange::new(Side::Client, HASH, code(), [2; 32]);

        // The client checks the feeder's opening of press `at` first, in
        // the feeder's message `at + 1`, and stops before opening its own.
        assert_eq!(
            run(&mut feeder, &mut client),
            Err((Side::Client, at + 1, Error::Mismatch))
        );
        assert!(!client.peer_proven());
        assert_eq!(client.write(), Err(Error::WrongTurn));
    }
}

#[test]
fn a_relay_between_two_handshakes_fails_at_the_first_opening() {
    // The relay forwards every message between a feeder and a client that
    // finished different handshakes with it.
    let mut feeder = Exchange::new(Side::Feeder, [8; 32], code(), [1; 32]);
    let mut client = Exchange::new(Side::Client, [9; 32], code(), [2; 32]);
    assert_eq!(
        run(&mut feeder, &mut client),
        Err((Side::Client, 1, Error::Mismatch))
    );
}

#[test]
fn a_reflected_message_is_not_the_peers() {
    // A relay plays the feeder's own commitments back to it as the client's.
    let mut feeder = Exchange::new(Side::Feeder, HASH, code(), [1; 32]);
    let first = feeder.write().unwrap();
    feeder.read(first).unwrap();
    let second = feeder.write().unwrap();
    assert_eq!(feeder.read(second), Err(Error::Mismatch));
}

#[test]
fn turns_and_shapes_are_enforced() {
    let mut feeder = Exchange::new(Side::Feeder, HASH, code(), [1; 32]);
    let mut client = Exchange::new(Side::Client, HASH, code(), [2; 32]);
    assert_eq!(client.write(), Err(Error::WrongTurn));
    let empty = Round {
        open: None,
        commit: None,
    };
    assert_eq!(feeder.read(empty), Err(Error::WrongTurn));

    let first = Exchange::new(Side::Feeder, HASH, code(), [1; 32])
        .write()
        .unwrap();
    assert_eq!(
        client.read(Round {
            open: Some([0; 32]),
            ..first
        }),
        Err(Error::Malformed)
    );
    // A failure ends the exchange for good.
    assert_eq!(client.read(first), Err(Error::WrongTurn));
}

#[test]
fn every_button_appears_in_codes_and_none_in_debug() {
    let all = Code::from_bits(0b11_10_01_00_11_10_01_00);
    for button in Button::ALL {
        assert!(all.presses().contains(button));
    }
    assert_eq!(format!("{all:?}"), "Code(..)");
}
