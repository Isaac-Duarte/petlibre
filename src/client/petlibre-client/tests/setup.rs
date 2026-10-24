//! The client's setup session against the feeder's own responder
//! (`petlibre_feeder::setup`), through framed bytes as BLE carries them: the
//! feeder learns the client's key and its checked ask, the client learns the
//! feeder's identity, and nothing else gets through.

use petlibre_client::setup::{Ask, Credentials, Deframer, Error, Session, Step, frame};
use petlibre_core::noise::{self, Keypair};
use petlibre_feeder::setup::{self as feeder, MAX_MESSAGE, Offer, Received, Responder};
use petlibre_proto::setup::{Enrolled, Refusal, Reply, WifiSaved};

fn keypair(byte: u8) -> Keypair {
    Keypair::from_secret([byte; 32])
}

fn ask() -> Ask {
    Ask {
        label: "phone".into(),
        wifi: Some(Credentials::new(b"Lab".to_vec(), "pass word!".into()).unwrap()),
    }
}

/// Carry one message across BLE's framing, split into 20-byte notifications.
fn carry(message: &[u8]) -> Vec<u8> {
    let framed = frame(message).unwrap();
    let mut deframer = Deframer::default();
    let mut out = None;
    for chunk in framed.chunks(20) {
        deframer.push(chunk);
        if let Some(message) = deframer.next_message().unwrap() {
            out = Some(message);
        }
    }
    out.unwrap()
}

/// Run the handshake; the feeder's offer, and both ends ready for replies.
fn handshake(identity: &Keypair, client: &Keypair, ask: Ask) -> (Session, Responder, Offer) {
    let mut session = Session::new(client, [3; 32], ask).unwrap();
    let mut responder = Responder::new(identity, [4; 32]).unwrap();
    let mut out = [0u8; MAX_MESSAGE];

    let first = carry(&session.start().unwrap());
    let Received::Send(len) = responder.receive(&first, &mut out).unwrap() else {
        panic!("the feeder answers the first message")
    };
    let Step::Send(third) = session.receive(&carry(&out[..len])).unwrap() else {
        panic!("the client sends its ask")
    };
    let Received::Offer(offer) = responder.receive(&carry(&third), &mut out).unwrap() else {
        panic!("the feeder reads the ask")
    };
    (session, responder, offer)
}

fn reply(responder: &mut Responder, session: &mut Session, reply: Reply) -> Step {
    let mut out = [0u8; MAX_MESSAGE];
    let len = responder.reply(&reply, &mut out).unwrap();
    session.receive(&carry(&out[..len])).unwrap()
}

#[test]
fn a_confirmed_setup_exchanges_both_keys_and_the_credentials() {
    let (identity, client) = (keypair(1), keypair(2));
    let (mut session, mut responder, offer) = handshake(&identity, &client, ask());

    assert_eq!(session.feeder_key(), Some(*identity.public()));
    assert_eq!(offer.key, *client.public());
    assert_eq!(offer.label.as_bytes(), b"phone");
    let wifi = offer.wifi.unwrap();
    assert_eq!((wifi.ssid(), wifi.psk()), (&b"Lab"[..], &b"pass word!"[..]));

    let confirming = Reply::Confirming { seconds: 60 };
    assert_eq!(
        reply(&mut responder, &mut session, confirming),
        Step::Reply(confirming)
    );
    let done = Reply::Done {
        enrolled: Enrolled::New,
        wifi: WifiSaved::Saved,
    };
    assert_eq!(reply(&mut responder, &mut session, done), Step::Reply(done));
    assert!(matches!(session.receive(&[0; 40]), Err(Error::Unexpected)));
}

#[test]
fn a_refusal_ends_the_session() {
    let (mut session, mut responder, _) = handshake(&keypair(1), &keypair(2), ask());
    let refused = Reply::Refused(Refusal::Unconfirmed);
    assert_eq!(
        reply(&mut responder, &mut session, refused),
        Step::Reply(refused)
    );
    assert!(matches!(session.receive(&[0; 40]), Err(Error::Unexpected)));
}

#[test]
fn an_ask_without_wifi_offers_none() {
    let ask = Ask {
        label: "cli".into(),
        wifi: None,
    };
    let (_, _, offer) = handshake(&keypair(1), &keypair(2), ask);
    assert_eq!(offer.wifi, None);
}

#[test]
fn the_client_refuses_what_the_feeder_would() {
    assert!(matches!(
        Credentials::new(Vec::new(), "password".into()),
        Err(Error::Credentials)
    ));
    assert!(matches!(
        Credentials::new(b"Lab".to_vec(), "short".into()),
        Err(Error::Credentials)
    ));
    assert!(matches!(
        Credentials::new(b"Lab".to_vec(), "tab\there!".into()),
        Err(Error::Credentials)
    ));
    let long = Ask {
        label: "a label far too long".into(),
        wifi: None,
    };
    assert!(matches!(
        Session::new(&keypair(2), [3; 32], long),
        Err(Error::Label)
    ));
}

#[test]
fn a_tampered_or_foreign_handshake_fails_closed() {
    let identity = keypair(1);
    let mut session = Session::new(&keypair(2), [3; 32], ask()).unwrap();
    let mut responder = Responder::new(&identity, [4; 32]).unwrap();
    let mut out = [0u8; MAX_MESSAGE];
    let Received::Send(len) = responder
        .receive(&session.start().unwrap(), &mut out)
        .unwrap()
    else {
        panic!("the feeder answers")
    };

    // The feeder's static key arrives encrypted; one flipped bit fails it.
    let mut tampered = out[..len].to_vec();
    tampered[40] ^= 1;
    assert!(matches!(
        session.receive(&tampered),
        Err(Error::Noise(noise::Error::Decrypt))
    ));

    // A responder on another prologue cannot read the client's ask.
    let mut session = Session::new(&keypair(2), [3; 32], ask()).unwrap();
    let mut foreign = petlibre_core::noise::Handshake::new(
        noise::Pattern::Xx,
        noise::Role::Responder,
        b"petlibre-setup-v0",
        &identity,
        None,
        [4; 32],
    )
    .unwrap();
    let mut ignored = [0u8; MAX_MESSAGE];
    foreign
        .read_message(&session.start().unwrap(), &mut ignored)
        .unwrap();
    let len = foreign.write_message(&[], &mut out).unwrap();
    assert!(matches!(
        session.receive(&out[..len]),
        Err(Error::Noise(noise::Error::Decrypt))
    ));
}

#[test]
fn the_feeder_takes_one_ask_and_nothing_after() {
    let (identity, client) = (keypair(1), keypair(2));
    let (_, mut responder, _) = handshake(&identity, &client, ask());
    let mut out = [0u8; MAX_MESSAGE];
    assert_eq!(
        responder.receive(&[0; 40], &mut out),
        Err(feeder::Error::Unexpected)
    );
}

#[test]
fn oversized_frames_are_refused_before_buffering() {
    let mut deframer = Deframer::default();
    deframer.push(&[0xff, 0xff, 1, 2]);
    assert!(matches!(
        deframer.next_message(),
        Err(Error::TooLong(0xffff))
    ));
    assert!(matches!(
        frame(&[0; MAX_MESSAGE + 1]),
        Err(Error::TooLong(_))
    ));
}
