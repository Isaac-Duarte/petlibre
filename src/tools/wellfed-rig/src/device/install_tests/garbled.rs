//! Garbled feeder messages during [`Device::install`]: answers that
//! authenticate but decode as nothing, as a feeder that sealed wrong bytes
//! sends them. Each ends only its session, the install resumes on a new
//! one as after a drop, and the note keeps the evidence.
//!
//! The fake feeder serves one connection at a time, so a session only opens
//! once the client has closed the one before: a client that kept the old
//! connection through its reconnect would never get a second session.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use wellfed_proto::api::{
    self as proto, ClientMessage, FRAME_MAX, FeederMessage, Hello, Op, Outcome, PROLOGUE,
    PROTOCOL_MAJOR, Welcome,
};
use wellfed_session::frame::{self, PREFIX};
use wellfed_session::noise::{Handshake, Keypair, Pattern, Role, Transport};

use super::{Device, DeviceError, Fault, OWNER, Offer};

const FEEDER: [u8; 32] = [0x7e; 32];
/// The chunks the fake installs: the offer names the first, the state the
/// rest, so the install asks for its state between them.
const CHUNKS: [u32; 3] = [0, 512, 1024];

/// A request, as the fake saw it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Seen {
    Status,
    Offer,
    Chunk(u32),
    State,
    Cancel,
    Other,
}

/// Which answers the fake garbles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Garble {
    Offer,
    Chunk,
    State,
    Cancel,
}

impl Garble {
    fn hits(self, seen: Seen) -> bool {
        matches!(
            (self, seen),
            (Self::Offer, Seen::Offer)
                | (Self::Chunk, Seen::Chunk(_))
                | (Self::State, Seen::State)
                | (Self::Cancel, Seen::Cancel)
        )
    }
}

struct State {
    garble: Garble,
    /// Answers still to garble.
    left: usize,
    received: Vec<u32>,
    /// Each session's requests, in order.
    sessions: Vec<Vec<Seen>>,
}

impl State {
    fn remaining(&self) -> Vec<u32> {
        CHUNKS
            .into_iter()
            .filter(|offset| !self.received.contains(offset))
            .collect()
    }

    fn view(&self, wanted: &[u32]) -> proto::UpdateView {
        let done = self.remaining().is_empty();
        let mut slots = [None; proto::WANTED_MAX];
        for (slot, &offset) in slots.iter_mut().zip(wanted) {
            *slot = Some(offset);
        }
        proto::UpdateView {
            phase: if done {
                proto::UpdatePhase::Activated
            } else {
                proto::UpdatePhase::Receiving
            },
            bank: 0,
            min_version: 1,
            digest: None,
            version: 2,
            image_len: 1536,
            written: u32::try_from(self.received.len() * 512).unwrap(),
            wanted: slots,
            stopped: None,
        }
    }

    fn answer(&mut self, op: &Op<'_>) -> Outcome<'static> {
        match op {
            Op::Status => Outcome::Status(status()),
            Op::OfferUpdate { .. } => {
                let first: Vec<u32> = self.remaining().into_iter().take(1).collect();
                Outcome::Update(self.view(&first))
            }
            Op::UpdateChunk { offset, .. } => {
                if !self.received.contains(offset) {
                    self.received.push(*offset);
                }
                // Between sectors: nothing wanted until the state is read.
                Outcome::Update(self.view(&[]))
            }
            Op::UpdateState => Outcome::Update(self.view(&self.remaining())),
            Op::CancelUpdate => Outcome::Update(proto::UpdateView {
                phase: proto::UpdatePhase::Idle,
                stopped: Some(proto::UpdateStop::Cancelled),
                ..self.view(&[])
            }),
            _ => Outcome::Refused(proto::Refusal::Invalid),
        }
    }
}

fn seen(op: &Op<'_>) -> Seen {
    match op {
        Op::Status => Seen::Status,
        Op::OfferUpdate { .. } => Seen::Offer,
        Op::UpdateChunk { offset, .. } => Seen::Chunk(*offset),
        Op::UpdateState => Seen::State,
        Op::CancelUpdate => Seen::Cancel,
        _ => Seen::Other,
    }
}

struct Feeder {
    address: SocketAddr,
    identity: [u8; 32],
    state: Arc<Mutex<State>>,
}

impl Feeder {
    /// A feeder that garbles the first `times` answers `garble` names.
    fn start(garble: Garble, times: usize) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let state = Arc::new(Mutex::new(State {
            garble,
            left: times,
            received: Vec::new(),
            sessions: Vec::new(),
        }));

        let shared = Arc::clone(&state);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                lock(&shared).sessions.push(Vec::new());
                // The client hanging up ends a session; nothing to report.
                let _ = serve(stream, &shared);
            }
        });

        Self {
            address,
            identity: *Keypair::from_secret(FEEDER).public(),
            state,
        }
    }

    fn sessions(&self) -> Vec<Vec<Seen>> {
        lock(&self.state).sessions.clone()
    }
}

fn lock(state: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

fn read_frame(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut prefix = [0; PREFIX];
    stream.read_exact(&mut prefix)?;
    let len = frame::parse_prefix(prefix, FRAME_MAX).unwrap();
    let mut message = vec![0; len];
    stream.read_exact(&mut message)?;
    Ok(message)
}

fn write_frame(stream: &mut TcpStream, message: &[u8]) -> std::io::Result<()> {
    let prefix = frame::prefix(message.len(), FRAME_MAX).unwrap();
    stream.write_all(&prefix)?;
    stream.write_all(message)
}

fn seal(stream: &mut TcpStream, transport: &mut Transport, plain: &[u8]) -> std::io::Result<()> {
    let mut out = [0; FRAME_MAX];
    let len = transport.seal(plain, &mut out).unwrap();
    write_frame(stream, &out[..len])
}

fn serve(mut stream: TcpStream, state: &Mutex<State>) -> std::io::Result<()> {
    let open: proto::Open = wellfed_proto::decode(&read_frame(&mut stream)?).unwrap();
    assert_eq!(open, proto::Open::Session);

    let keys = Keypair::from_secret(FEEDER);
    let mut handshake = Handshake::new(
        Pattern::Ik,
        Role::Responder,
        PROLOGUE,
        &keys,
        None,
        [0x3c; 32],
    )
    .unwrap();
    let mut plain = [0; FRAME_MAX];
    let len = handshake
        .read_message(&read_frame(&mut stream)?, &mut plain)
        .unwrap();
    let hello: Hello = wellfed_proto::decode(&plain[..len]).unwrap();
    let welcome = Welcome::Open {
        major: PROTOCOL_MAJOR,
        capabilities: hello.capabilities,
        build: "garbled",
        role: proto::Role::Owner,
        epoch: 1,
    };
    let mut payload = [0; 128];
    let len = wellfed_proto::encode(&welcome, &mut payload).unwrap();
    let mut out = [0; FRAME_MAX];
    let written = handshake.write_message(&payload[..len], &mut out).unwrap();
    write_frame(&mut stream, &out[..written])?;
    let mut transport = handshake.into_transport().unwrap();

    loop {
        let message = read_frame(&mut stream)?;
        let len = transport.open(&message, &mut plain).unwrap();
        let ClientMessage::Request { id, op } = wellfed_proto::decode(&plain[..len]).unwrap()
        else {
            continue;
        };

        let (garbled, result) = {
            let mut state = lock(state);
            let seen = seen(&op);
            if let Some(session) = state.sessions.last_mut() {
                session.push(seen);
            }
            let result = state.answer(&op);
            let garbled = state.left > 0 && state.garble.hits(seen);
            if garbled {
                state.left -= 1;
            }
            (garbled, result)
        };

        if garbled {
            // Authenticated, and no message of the protocol.
            seal(&mut stream, &mut transport, &[0xff; 24])?;
            continue;
        }
        let mut answer = [0; FRAME_MAX];
        let response = FeederMessage::Response { id, result };
        let len = wellfed_proto::encode(&response, &mut answer).unwrap();
        seal(&mut stream, &mut transport, &answer[..len])?;
    }
}

fn status() -> proto::Status<'static> {
    proto::Status {
        build: "garbled",
        bank: 0,
        uptime_ms: 0,
        boot_count: 1,
        reset_flags: 0,
        faults: 0,
        rssi: 0,
        wifi: proto::WifiSource::Build,
        link_losses: 0,
        dns_losses: 0,
        clock_ms: None,
        power: None,
        locked: None,
        settings_generation: 0,
        wifi_generation: 0,
        last_event: None,
        sessions: 1,
        desiccant: proto::Desiccant::Unset,
    }
}

/// Push against `feeder`, cancelling once the first session is open if
/// `cancel` (a stop set before then ends the push unconnected); the result
/// and the notes.
fn push(feeder: &Feeder, cancel: bool) -> (Result<(), DeviceError>, Vec<String>) {
    let mut device = Device::at(OWNER, feeder.identity, feeder.address);
    let offer = Offer::new(2, 0x22);
    let stop = AtomicBool::new(false);
    let mut notes = Vec::new();

    let result = device.install(offer.image(), Fault::None, &stop, &mut |note| {
        eprintln!("{note}");
        if cancel && note.starts_with("Session open on bank") {
            stop.store(true, Ordering::Release);
        }
        notes.push(note);
    });

    eprintln!("push: {result:?}");
    (result, notes)
}

fn garbled_note(notes: &[String]) -> &str {
    notes
        .iter()
        .find(|note| note.starts_with("A feeder message did not decode"))
        .expect("a note for the garbled message")
}

/// A garbled chunk answer: the next session reads the state before it
/// sends any chunk, and the note names the chunk and the bytes.
#[test]
fn a_garbled_chunk_answer_resumes_from_the_state() {
    let feeder = Feeder::start(Garble::Chunk, 1);

    let (result, notes) = push(&feeder, false);

    result.unwrap();
    let sessions = feeder.sessions();
    assert_eq!(sessions.len(), 2, "{sessions:?}");
    assert_eq!(sessions[0], [Seen::Status, Seen::Offer, Seen::Chunk(0)]);
    assert_eq!(sessions[1][..2], [Seen::Status, Seen::State]);

    let note = garbled_note(&notes);
    assert!(note.contains("(1 of 3 allowed) on bank 0"), "{note}");
    assert!(
        note.contains(&format!("24 bytes, starting {}", "ff".repeat(24))),
        "{note}"
    );
    assert!(note.contains("outstanding Chunk(0);"), "{note}");
}

#[test]
fn a_garbled_state_answer_reads_the_state_again() {
    let feeder = Feeder::start(Garble::State, 1);

    let (result, notes) = push(&feeder, false);

    result.unwrap();
    let sessions = feeder.sessions();
    assert_eq!(sessions.len(), 2, "{sessions:?}");
    assert_eq!(sessions[0].last(), Some(&Seen::State));
    assert_eq!(sessions[1][..2], [Seen::Status, Seen::State]);
    assert!(garbled_note(&notes).contains("outstanding State;"));
}

/// A cancel whose answer is garbled goes again on the next session.
#[test]
fn a_garbled_cancel_answer_cancels_again() {
    let feeder = Feeder::start(Garble::Cancel, 1);

    let (result, notes) = push(&feeder, true);

    assert!(matches!(result, Err(DeviceError::Stopped)), "{result:?}");
    assert_eq!(
        feeder.sessions(),
        [
            vec![Seen::Status, Seen::Cancel],
            vec![Seen::Status, Seen::Cancel]
        ]
    );
    assert!(garbled_note(&notes).contains("outstanding Cancel;"));
}

/// Past the allowance the decode error ends the install.
#[test]
fn a_fourth_garbled_answer_ends_the_install() {
    let feeder = Feeder::start(Garble::Offer, 4);

    let (result, _) = push(&feeder, false);

    assert!(
        matches!(
            result,
            Err(DeviceError::Session(wellfed_client::api::Error::Malformed(
                _
            )))
        ),
        "{result:?}"
    );
    let sessions = feeder.sessions();
    assert_eq!(sessions.len(), 4, "{sessions:?}");
    assert!(
        sessions
            .iter()
            .all(|session| session == &[Seen::Status, Seen::Offer])
    );
}
