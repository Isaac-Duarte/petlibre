//! The client's setup session against the feeder's own responder
//! (`wellfed_feeder::setup`), through framed bytes as BLE carries them: the
//! feeder learns the client's key and label, the owner's code proves each
//! side to the other, the Wi-Fi crosses only after the feeder's proof, and a
//! relay in the middle gets neither an enrollment nor the passphrase.

use wellfed_client::setup::{Ask, Credentials, Deframer, Error, Session, Step, frame};
use wellfed_feeder::setup::{self as feeder, MAX_MESSAGE, Offer, Received, Responder};
use wellfed_proto::setup::{Enrolled, Refusal, Reply, WifiSaved};
use wellfed_session::noise::{self, Keypair};
use wellfed_session::setup_code::{Button, Code};

/// The reply as the client hands it on.
fn model(reply: Reply) -> wellfed_model::setup::Reply {
    wellfed_model::convert::setup_reply(reply).unwrap()
}

const CODE: u16 = 0b10_01_11_00_01_10_00_11;

fn keypair(byte: u8) -> Keypair {
    Keypair::from_secret([byte; 32])
}

fn ask() -> Ask {
    Ask {
        label: "phone".into(),
        wifi: Some(Credentials::new(b"Lab".to_vec(), "pass word!".into()).unwrap()),
    }
}

fn new_session(client: &Keypair, ask: Ask) -> Session {
    Session::new(client, [3; 32], ask, Code::from_bits(CODE), [5; 32]).unwrap()
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

/// Run the handshake; the feeder's offer, and both ends ready for the code.
fn handshake(session: &mut Session, identity: &Keypair) -> (Responder, Offer) {
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
    (responder, offer)
}

fn reply(responder: &mut Responder, session: &mut Session, reply: Reply) -> Result<Step, Error> {
    let mut out = [0u8; MAX_MESSAGE];
    let len = responder.reply(&reply, &mut out).unwrap();
    session.receive(&carry(&out[..len]))
}

/// The owner keys `code` in on the feeder; run the proof to the feeder's
/// verdict on the client.
fn prove(
    responder: &mut Responder,
    session: &mut Session,
    code: Code,
) -> Result<Received, (Option<Error>, Option<feeder::Error>)> {
    let mut out = [0u8; MAX_MESSAGE];
    let mut len = responder.prove(code, [6; 32], &mut out).unwrap();
    loop {
        let Step::Send(answer) = session
            .receive(&carry(&out[..len]))
            .map_err(|e| (Some(e), None))?
        else {
            panic!("the client answers each step")
        };
        match responder.receive(&carry(&answer), &mut out) {
            Ok(Received::Send(next)) => len = next,
            Ok(received) => return Ok(received),
            Err(e) => return Err((None, Some(e))),
        }
    }
}

#[test]
fn the_owners_code_proves_both_sides_and_then_carries_the_wifi() {
    let (identity, client) = (keypair(1), keypair(2));
    let mut session = new_session(&client, ask());
    let (mut responder, offer) = handshake(&mut session, &identity);
    assert_eq!(offer.key, *client.public());
    assert_eq!(offer.label.as_bytes(), b"phone");
    assert_eq!(session.feeder_key(), None, "not trusted before the proof");

    let entering = Reply::EnterCode { seconds: 60 };
    assert_eq!(
        reply(&mut responder, &mut session, entering).unwrap(),
        Step::Reply(model(entering))
    );
    let code = session.code();
    let Ok(Received::Proven { wifi }) = prove(&mut responder, &mut session, code) else {
        panic!("the same code proves")
    };
    let wifi = wifi.unwrap();
    assert_eq!((wifi.ssid(), wifi.psk()), (&b"Lab"[..], &b"pass word!"[..]));
    assert_eq!(session.feeder_key(), Some(*identity.public()));

    let done = Reply::Done {
        enrolled: Enrolled::New,
        wifi: WifiSaved::Saved,
    };
    assert_eq!(
        reply(&mut responder, &mut session, done).unwrap(),
        Step::Reply(model(done))
    );
    assert!(matches!(session.receive(&[0; 40]), Err(Error::Unexpected)));
}

#[test]
fn a_mistyped_code_stops_the_client_before_the_wifi() {
    let mut session = new_session(&keypair(2), ask());
    let (mut responder, _) = handshake(&mut session, &keypair(1));
    let mut presses = *session.code().presses();
    presses[3] = if presses[3] == Button::Mic {
        Button::Lock
    } else {
        Button::Mic
    };

    let result = prove(&mut responder, &mut session, Code::new(presses));
    assert!(matches!(result, Err((Some(Error::WrongCode), None))));
    assert_eq!(session.feeder_key(), None);
}

#[test]
fn a_device_without_the_code_cannot_claim_done() {
    // Whatever answered the handshake (the feeder, or a relay with its own
    // key) says `Done` without proving the code: the client keeps neither
    // its key nor the session.
    let mut session = new_session(&keypair(2), ask());
    let (mut relay, _) = handshake(&mut session, &keypair(9));
    let done = Reply::Done {
        enrolled: Enrolled::New,
        wifi: WifiSaved::Saved,
    };
    assert!(matches!(
        reply(&mut relay, &mut session, done),
        Err(Error::Unexpected)
    ));
    assert_eq!(session.feeder_key(), None);
}

#[test]
fn a_refusal_ends_the_session() {
    let mut session = new_session(&keypair(2), ask());
    let (mut responder, _) = handshake(&mut session, &keypair(1));
    let refused = Reply::Refused(Refusal::Unconfirmed);
    assert_eq!(
        reply(&mut responder, &mut session, refused).unwrap(),
        Step::Reply(model(refused))
    );
    assert!(matches!(session.receive(&[0; 40]), Err(Error::Unexpected)));
}

#[test]
fn an_ask_without_wifi_proves_none() {
    let ask = Ask {
        label: "cli".into(),
        wifi: None,
    };
    let mut session = new_session(&keypair(2), ask);
    let (mut responder, _) = handshake(&mut session, &keypair(1));
    let code = session.code();
    assert!(matches!(
        prove(&mut responder, &mut session, code),
        Ok(Received::Proven { wifi: None })
    ));
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
        Session::new(&keypair(2), [3; 32], long, Code::from_bits(CODE), [5; 32]),
        Err(Error::Label)
    ));
}

#[test]
fn a_tampered_or_foreign_handshake_fails_closed() {
    let identity = keypair(1);
    let mut session = new_session(&keypair(2), ask());
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
    let mut session = new_session(&keypair(2), ask());
    let mut foreign = wellfed_session::noise::Handshake::new(
        noise::Pattern::Xx,
        noise::Role::Responder,
        b"wellfed-setup-v0",
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
fn the_feeder_takes_nothing_between_the_ask_and_the_code() {
    let mut session = new_session(&keypair(2), ask());
    let (mut responder, _) = handshake(&mut session, &keypair(1));
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
