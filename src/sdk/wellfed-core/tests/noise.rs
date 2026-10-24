//! The device API's Noise handshakes against `snow`, an independent
//! implementation: XX and KK in both roles, then transport messages each
//! way. With the same keys the two write the same bytes, so a mismatch in
//! any token, hash or key schedule fails here. Tampering, misordering and a
//! wrong static key must all fail closed.

use wellfed_core::noise::{Error, Handshake, Keypair, Pattern, Role, TAG_LEN, Transport};

const PROLOGUE: &[u8] = b"wellfed/1";

/// Fixed secrets: clamped by X25519 itself, so any 32 bytes will do.
const INITIATOR_STATIC: [u8; 32] = [0x11; 32];
const RESPONDER_STATIC: [u8; 32] = [0x22; 32];
const INITIATOR_EPHEMERAL: [u8; 32] = [0x33; 32];
const RESPONDER_EPHEMERAL: [u8; 32] = [0x44; 32];

fn name(pattern: Pattern) -> &'static str {
    std::str::from_utf8(pattern.protocol_name()).unwrap()
}

fn ours(pattern: Pattern, role: Role) -> Handshake {
    let (local, remote, ephemeral) = match role {
        Role::Initiator => (INITIATOR_STATIC, RESPONDER_STATIC, INITIATOR_EPHEMERAL),
        Role::Responder => (RESPONDER_STATIC, INITIATOR_STATIC, RESPONDER_EPHEMERAL),
    };
    let remote = Keypair::from_secret(remote).public().to_owned();
    Handshake::new(
        pattern,
        role,
        PROLOGUE,
        &Keypair::from_secret(local),
        (pattern == Pattern::Kk).then_some(remote),
        ephemeral,
    )
    .unwrap()
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
    if pattern == Pattern::Kk {
        builder = builder.remote_public_key(&remote).unwrap();
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
fn xx_and_kk_agree_with_snow_in_both_roles() {
    for pattern in [Pattern::Xx, Pattern::Kk] {
        for role in [Role::Initiator, Role::Responder] {
            let (mut ours, mut theirs) = run(pattern, role);
            exchange(&mut ours, &mut theirs);
        }
    }
}

#[test]
fn kk_is_one_round_trip_and_xx_is_three_messages() {
    for (pattern, messages) in [(Pattern::Xx, 3), (Pattern::Kk, 2)] {
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
