//! The device API's Noise handshakes against `snow`, an independent
//! implementation: XX, IK and KK in both roles, then transport messages each
//! way. With the same keys the two write the same bytes, so a mismatch in
//! any token, hash or key schedule fails here. Tampering, misordering and a
//! wrong static key must all fail closed.

use wellfed_session::noise::{Error, Handshake, Keypair, Pattern, Role, TAG_LEN, Transport};

const PROLOGUE: &[u8] = b"wellfed/1";

/// Fixed secrets: clamped by X25519 itself, so any 32 bytes will do.
const INITIATOR_STATIC: [u8; 32] = [0x11; 32];
const RESPONDER_STATIC: [u8; 32] = [0x22; 32];
const INITIATOR_EPHEMERAL: [u8; 32] = [0x33; 32];
const RESPONDER_EPHEMERAL: [u8; 32] = [0x44; 32];
/// An invite's secret, for XXpsk3.
const PSK: [u8; 32] = [0x66; 32];

/// KK always starts with the peer's static key; IK only on the initiator.
fn needs_remote(pattern: Pattern, role: Role) -> bool {
    pattern == Pattern::Kk || (pattern == Pattern::Ik && role == Role::Initiator)
}

fn name(pattern: Pattern) -> &'static str {
    std::str::from_utf8(pattern.protocol_name()).unwrap()
}

fn ours(pattern: Pattern, role: Role) -> Handshake {
    let (local, remote, ephemeral) = match role {
        Role::Initiator => (INITIATOR_STATIC, RESPONDER_STATIC, INITIATOR_EPHEMERAL),
        Role::Responder => (RESPONDER_STATIC, INITIATOR_STATIC, RESPONDER_EPHEMERAL),
    };
    let remote = Keypair::from_secret(remote).public().to_owned();
    let mut handshake = Handshake::new(
        pattern,
        role,
        PROLOGUE,
        &Keypair::from_secret(local),
        needs_remote(pattern, role).then_some(remote),
        ephemeral,
    )
    .unwrap();
    if pattern == Pattern::XxPsk3 {
        handshake.set_psk(PSK);
    }
    handshake
}

fn snow(pattern: Pattern, role: Role) -> snow::HandshakeState {
    let (local, remote, ephemeral) = match role {
        Role::Initiator => (INITIATOR_STATIC, RESPONDER_STATIC, INITIATOR_EPHEMERAL),
        Role::Responder => (RESPONDER_STATIC, INITIATOR_STATIC, RESPONDER_EPHEMERAL),
    };
    let remote = *Keypair::from_secret(remote).public();
    let mut builder = snow::Builder::new(name(pattern).parse().unwrap())
        .prologue(PROLOGUE)
        .unwrap()
        .local_private_key(&local)
        .unwrap()
        .fixed_ephemeral_key_for_testing_only(&ephemeral);
    if needs_remote(pattern, role) {
        builder = builder.remote_public_key(&remote).unwrap();
    }
    if pattern == Pattern::XxPsk3 {
        builder = builder.psk(3, &PSK).unwrap();
    }
    match role {
        Role::Initiator => builder.build_initiator().unwrap(),
        Role::Responder => builder.build_responder().unwrap(),
    }
}

/// Our side against snow's, every message checked against snow writing the
/// same message to itself; returns our transport and snow's.
fn run(pattern: Pattern, our_role: Role) -> (Transport, snow::TransportState) {
    let their_role = match our_role {
        Role::Initiator => Role::Responder,
        Role::Responder => Role::Initiator,
    };
    let mut us = ours(pattern, our_role);
    let mut them = snow(pattern, their_role);
    // snow in our role, to compare the bytes we write.
    let mut mirror = snow(pattern, our_role);
    let mut mirror_peer = snow(pattern, their_role);

    let (mut message, mut expected, mut payload) = ([0; 512], [0; 512], [0; 512]);
    let mut turn = 0u8;
    while !us.is_finished() {
        let sent = [b'm', turn];
        if us.is_my_turn() {
            let len = us.write_message(&sent, &mut message).unwrap();
            let mirrored = mirror.write_message(&sent, &mut expected).unwrap();
            assert_eq!(&message[..len], &expected[..mirrored], "message {turn}");
            mirror_peer
                .read_message(&expected[..mirrored], &mut payload)
                .unwrap();

            let read = them.read_message(&message[..len], &mut payload).unwrap();
            assert_eq!(&payload[..read], &sent);
        } else {
            let len = them.write_message(&sent, &mut message).unwrap();
            mirror.read_message(&message[..len], &mut payload).unwrap();
            mirror_peer.write_message(&sent, &mut expected).unwrap();

            let read = us.read_message(&message[..len], &mut payload).unwrap();
            assert_eq!(&payload[..read], &sent);
        }
        turn += 1;
    }

    assert!(them.is_handshake_finished());
    assert_eq!(us.handshake_hash().as_slice(), them.get_handshake_hash());
    assert_eq!(
        us.remote_static().map(|key| key.as_slice()),
        Some(
            Keypair::from_secret(match our_role {
                Role::Initiator => RESPONDER_STATIC,
                Role::Responder => INITIATOR_STATIC,
            })
            .public()
            .as_slice()
        )
    );
    (
        us.into_transport().unwrap(),
        them.into_transport_mode().unwrap(),
    )
}

fn exchange(ours: &mut Transport, theirs: &mut snow::TransportState) {
    let (mut message, mut payload) = ([0; 256], [0; 256]);
    for round in 0..3u8 {
        let request = [b'q', round];
        let len = ours.seal(&request, &mut message).unwrap();
        assert_eq!(len, request.len() + TAG_LEN);
        let read = theirs.read_message(&message[..len], &mut payload).unwrap();
        assert_eq!(&payload[..read], &request);

        let reply = [b'r', round, 0xff];
        let len = theirs.write_message(&reply, &mut message).unwrap();
        let read = ours.open(&message[..len], &mut payload).unwrap();
        assert_eq!(&payload[..read], &reply);
    }
}

#[test]
fn every_pattern_agrees_with_snow_in_both_roles() {
    for pattern in [Pattern::Xx, Pattern::Ik, Pattern::Kk, Pattern::XxPsk3] {
        for role in [Role::Initiator, Role::Responder] {
            let (mut ours, mut theirs) = run(pattern, role);
            exchange(&mut ours, &mut theirs);
        }
    }
}

#[test]
fn ik_and_kk_are_one_round_trip_and_xx_is_three_messages() {
    for (pattern, messages) in [
        (Pattern::Xx, 3),
        (Pattern::Ik, 2),
        (Pattern::Kk, 2),
        (Pattern::XxPsk3, 3),
    ] {
        let mut initiator = ours(pattern, Role::Initiator);
        let mut responder = ours(pattern, Role::Responder);
        let (mut message, mut payload) = ([0; 256], [0; 256]);
        let mut count = 0;
        while !initiator.is_finished() {
            let (writer, reader) = if initiator.is_my_turn() {
                (&mut initiator, &mut responder)
            } else {
                (&mut responder, &mut initiator)
            };
            let len = writer.write_message(&[], &mut message).unwrap();
            reader.read_message(&message[..len], &mut payload).unwrap();
            count += 1;
        }
        assert!(responder.is_finished());
        assert_eq!(count, messages, "{pattern:?}");
    }
}

#[test]
fn a_tampered_or_replayed_message_fails_closed() {
    let mut initiator = ours(Pattern::Kk, Role::Initiator);
    let mut responder = ours(Pattern::Kk, Role::Responder);
    let (mut message, mut payload) = ([0; 256], [0; 256]);

    let len = initiator.write_message(b"hi", &mut message).unwrap();
    let mut tampered = message;
    tampered[len - 1] ^= 1;
    assert_eq!(
        responder.read_message(&tampered[..len], &mut payload),
        Err(Error::Decrypt)
    );

    // A fresh responder takes the genuine message; the transport then
    // refuses a replay, since the counter has moved on.
    let mut responder = ours(Pattern::Kk, Role::Responder);
    responder
        .read_message(&message[..len], &mut payload)
        .unwrap();
    let len = responder.write_message(&[], &mut message).unwrap();
    initiator
        .read_message(&message[..len], &mut payload)
        .unwrap();
    let (mut a, mut b) = (
        initiator.into_transport().unwrap(),
        responder.into_transport().unwrap(),
    );
    let len = a.seal(b"feed", &mut message).unwrap();
    assert_eq!(b.open(&message[..len], &mut payload), Ok(4));
    assert_eq!(b.open(&message[..len], &mut payload), Err(Error::Decrypt));
}

#[test]
fn kk_with_the_wrong_static_key_fails() {
    let stranger = *Keypair::from_secret([0x55; 32]).public();
    let mut initiator = Handshake::new(
        Pattern::Kk,
        Role::Initiator,
        PROLOGUE,
        &Keypair::from_secret(INITIATOR_STATIC),
        Some(stranger),
        INITIATOR_EPHEMERAL,
    )
    .unwrap();
    let mut responder = ours(Pattern::Kk, Role::Responder);
    let (mut message, mut payload) = ([0; 256], [0; 256]);

    let len = initiator.write_message(&[], &mut message).unwrap();
    assert_eq!(
        responder.read_message(&message[..len], &mut payload),
        Err(Error::Decrypt)
    );
}

#[test]
fn misuse_is_refused() {
    assert!(matches!(
        Handshake::new(
            Pattern::Kk,
            Role::Initiator,
            PROLOGUE,
            &Keypair::from_secret(INITIATOR_STATIC),
            None,
            INITIATOR_EPHEMERAL,
        ),
        Err(Error::MissingRemoteKey)
    ));

    let mut responder = ours(Pattern::Xx, Role::Responder);
    let (mut message, mut payload) = ([0; 256], [0; 256]);
    assert_eq!(
        responder.write_message(&[], &mut message),
        Err(Error::WrongTurn)
    );
    assert_eq!(
        responder.read_message(&[0; 10], &mut payload),
        Err(Error::BadLength),
        "shorter than an ephemeral key"
    );

    let mut initiator = ours(Pattern::Xx, Role::Initiator);
    assert_eq!(
        initiator.write_message(&[], &mut [0; 16]),
        Err(Error::BufferTooSmall)
    );
    assert!(matches!(initiator.into_transport(), Err(Error::Unfinished)));

    // A low-order ephemeral (all zeros) gives an all-zero shared secret.
    let mut initiator = ours(Pattern::Xx, Role::Initiator);
    initiator.write_message(&[], &mut message).unwrap();
    let mut forged = [0; 64 + 2 * TAG_LEN];
    forged[..32].copy_from_slice(&[0; 32]);
    assert_eq!(
        initiator.read_message(&forged, &mut payload),
        Err(Error::WeakKey)
    );
}

#[test]
fn xxpsk3_needs_the_same_invite_secret() {
    let (mut message, mut payload) = ([0; 256], [0; 256]);
    let mut initiator = ours(Pattern::XxPsk3, Role::Initiator);
    let mut responder = ours(Pattern::XxPsk3, Role::Responder);
    responder.set_psk([0x67; 32]);

    // The first two messages carry no PSK: the responder reads the invite's
    // id from the first payload before choosing the key.
    let len = initiator.write_message(b"invite 7", &mut message).unwrap();
    let read = responder
        .read_message(&message[..len], &mut payload)
        .unwrap();
    assert_eq!(&payload[..read], b"invite 7");
    let len = responder.write_message(&[], &mut message).unwrap();
    initiator
        .read_message(&message[..len], &mut payload)
        .unwrap();

    let len = initiator.write_message(&[], &mut message).unwrap();
    assert_eq!(
        responder.read_message(&message[..len], &mut payload),
        Err(Error::Decrypt)
    );
}

#[test]
fn xxpsk3_without_a_psk_is_refused() {
    let mut initiator = Handshake::new(
        Pattern::XxPsk3,
        Role::Initiator,
        PROLOGUE,
        &Keypair::from_secret(INITIATOR_STATIC),
        None,
        INITIATOR_EPHEMERAL,
    )
    .unwrap();
    let mut responder = ours(Pattern::XxPsk3, Role::Responder);
    let (mut message, mut payload) = ([0; 256], [0; 256]);
    for _ in 0..2 {
        let (writer, reader) = if initiator.is_my_turn() {
            (&mut initiator, &mut responder)
        } else {
            (&mut responder, &mut initiator)
        };
        let len = writer.write_message(&[], &mut message).unwrap();
        reader.read_message(&message[..len], &mut payload).unwrap();
    }
    assert_eq!(
        initiator.write_message(&[], &mut message),
        Err(Error::MissingPsk)
    );
}

/// The feeder (IK's responder) learns the client's key from the first
/// message, before it answers, so it can refuse a key it never enrolled.
#[test]
fn ik_shows_the_responder_the_clients_key_after_the_first_message() {
    let mut initiator = ours(Pattern::Ik, Role::Initiator);
    let mut responder = ours(Pattern::Ik, Role::Responder);
    let (mut message, mut payload) = ([0; 256], [0; 256]);
    assert_eq!(responder.remote_static(), None);

    let len = initiator.write_message(b"hello", &mut message).unwrap();
    let read = responder
        .read_message(&message[..len], &mut payload)
        .unwrap();
    assert_eq!(&payload[..read], b"hello");
    assert_eq!(
        responder.remote_static(),
        Some(Keypair::from_secret(INITIATOR_STATIC).public())
    );
}

/// A client holding the wrong feeder key cannot open a session: the feeder
/// cannot read its first message.
#[test]
fn ik_to_the_wrong_feeder_fails() {
    let stranger = *Keypair::from_secret([0x55; 32]).public();
    let mut initiator = Handshake::new(
        Pattern::Ik,
        Role::Initiator,
        PROLOGUE,
        &Keypair::from_secret(INITIATOR_STATIC),
        Some(stranger),
        INITIATOR_EPHEMERAL,
    )
    .unwrap();
    let mut responder = ours(Pattern::Ik, Role::Responder);
    let (mut message, mut payload) = ([0; 256], [0; 256]);

    let len = initiator.write_message(&[], &mut message).unwrap();
    assert_eq!(
        responder.read_message(&message[..len], &mut payload),
        Err(Error::Decrypt)
    );
}

/// Bounded properties: the responder survives any bytes, every pattern
/// carries any payload under any keys, and a flipped bit never reads.
mod properties {
    use super::*;
    use proptest::prelude::*;

    const PATTERNS: [Pattern; 4] = [Pattern::Xx, Pattern::Ik, Pattern::Kk, Pattern::XxPsk3];

    #[derive(Clone, Copy, Debug)]
    struct Keys {
        initiator: [u8; 32],
        responder: [u8; 32],
        initiator_ephemeral: [u8; 32],
        responder_ephemeral: [u8; 32],
        psk: [u8; 32],
    }

    fn keys() -> impl Strategy<Value = Keys> {
        any::<[[u8; 32]; 5]>().prop_map(|[a, b, c, d, e]| Keys {
            initiator: a,
            responder: b,
            initiator_ephemeral: c,
            responder_ephemeral: d,
            psk: e,
        })
    }

    fn pattern() -> impl Strategy<Value = Pattern> {
        proptest::sample::select(PATTERNS.to_vec())
    }

    fn pair(pattern: Pattern, keys: &Keys) -> (Handshake, Handshake) {
        let initiator_static = Keypair::from_secret(keys.initiator);
        let responder_static = Keypair::from_secret(keys.responder);
        let mut initiator = Handshake::new(
            pattern,
            Role::Initiator,
            PROLOGUE,
            &initiator_static,
            needs_remote(pattern, Role::Initiator).then_some(*responder_static.public()),
            keys.initiator_ephemeral,
        )
        .unwrap();
        let mut responder = Handshake::new(
            pattern,
            Role::Responder,
            PROLOGUE,
            &responder_static,
            needs_remote(pattern, Role::Responder).then_some(*initiator_static.public()),
            keys.responder_ephemeral,
        )
        .unwrap();
        if pattern == Pattern::XxPsk3 {
            initiator.set_psk(keys.psk);
            responder.set_psk(keys.psk);
        }
        (initiator, responder)
    }

    proptest! {
        /// What the feeder reads first from an unauthenticated peer: any
        /// bytes, into any payload buffer, fail cleanly or read.
        #[test]
        fn a_responder_survives_any_first_message(
            pattern in pattern(),
            message in proptest::collection::vec(any::<u8>(), 0..160),
            room in 0usize..96,
        ) {
            let keys = Keys {
                initiator: INITIATOR_STATIC,
                responder: RESPONDER_STATIC,
                initiator_ephemeral: INITIATOR_EPHEMERAL,
                responder_ephemeral: RESPONDER_EPHEMERAL,
                psk: PSK,
            };
            let (_, mut responder) = pair(pattern, &keys);
            let mut payload = vec![0; room];
            if let Ok(read) = responder.read_message(&message, &mut payload) {
                prop_assert!(read <= room);
                // Not authentication: anyone holding the responder's static key
                // can write a valid IK or XXpsk3 first message. Random bytes just
                // carry a valid tag with odds of 2^-128, so in practice only XX,
                // whose first message has no tag, reads them.
                prop_assert_eq!(pattern, Pattern::Xx);
            }
        }

        /// Every pattern, any keys, any payloads: each side reads what the
        /// other wrote, both end on the same hash, and the transports carry
        /// a message each way.
        #[test]
        fn every_handshake_round_trips_any_payloads(
            pattern in pattern(),
            keys in keys(),
            payloads in proptest::collection::vec(
                proptest::collection::vec(any::<u8>(), 0..64), 3),
            sealed in proptest::collection::vec(any::<u8>(), 0..128),
        ) {
            let (mut initiator, mut responder) = pair(pattern, &keys);
            let (mut message, mut payload) = ([0; 512], [0; 512]);
            let mut turn = 0;
            while !initiator.is_finished() {
                let (writer, reader) = if initiator.is_my_turn() {
                    (&mut initiator, &mut responder)
                } else {
                    (&mut responder, &mut initiator)
                };
                let sent = &payloads[turn];
                let len = writer.write_message(sent, &mut message).unwrap();
                let read = reader.read_message(&message[..len], &mut payload).unwrap();
                prop_assert_eq!(&payload[..read], sent.as_slice());
                turn += 1;
            }

            prop_assert!(responder.is_finished());
            prop_assert_eq!(initiator.handshake_hash(), responder.handshake_hash());
            let client = *Keypair::from_secret(keys.initiator).public();
            prop_assert_eq!(responder.remote_static(), Some(&client));
            let mut a = initiator.into_transport().unwrap();
            let mut b = responder.into_transport().unwrap();
            for flip in [false, true] {
                let (from, to) = if flip { (&mut b, &mut a) } else { (&mut a, &mut b) };
                let len = from.seal(&sealed, &mut message).unwrap();
                let read = to.open(&message[..len], &mut payload).unwrap();
                prop_assert_eq!(&payload[..read], sealed.as_slice());
            }
        }

        /// Any one flipped bit in a handshake or transport message fails the
        /// read, except in XX's first message, which no key protects yet.
        #[test]
        fn a_flipped_bit_never_reads(
            pattern in pattern(),
            keys in keys(),
            target in 0usize..4,
            bit in any::<proptest::sample::Index>(),
            body in proptest::collection::vec(any::<u8>(), 0..32),
        ) {
            let (mut initiator, mut responder) = pair(pattern, &keys);
            let (mut message, mut payload) = ([0; 512], [0; 512]);
            let mut turn = 0;
            while !initiator.is_finished() {
                let (writer, reader) = if initiator.is_my_turn() {
                    (&mut initiator, &mut responder)
                } else {
                    (&mut responder, &mut initiator)
                };
                let len = writer.write_message(&body, &mut message).unwrap();
                if turn == target && !(pattern == Pattern::Xx && turn == 0) {
                    let at = bit.index(len * 8);
                    message[at / 8] ^= 1 << (at % 8);
                    prop_assert!(reader.read_message(&message[..len], &mut payload).is_err());
                    return Ok(());
                }
                reader.read_message(&message[..len], &mut payload).unwrap();
                turn += 1;
            }

            let mut a = initiator.into_transport().unwrap();
            let mut b = responder.into_transport().unwrap();
            let len = a.seal(&body, &mut message).unwrap();
            let at = bit.index(len * 8);
            message[at / 8] ^= 1 << (at % 8);
            prop_assert_eq!(b.open(&message[..len], &mut payload), Err(Error::Decrypt));
        }
    }
}
