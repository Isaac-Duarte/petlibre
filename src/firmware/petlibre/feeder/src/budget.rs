//! FW44: what the device API will cost, measured inside the product image.
//!
//! The API (FW47) is not written yet, so this links and exercises its fixed
//! parts: a session table sized at build time ([`CLIENTS`] sessions, each
//! with its Noise state, a receive and a send buffer, and a ring of recent
//! request IDs) and the Noise handshakes on the KM4. No socket is opened:
//! pairing and keys are FW45's, and a listener without them would be an open
//! port with nothing behind it.
//!
//! After boot the loop runs XX and KK handshakes between two sessions of the
//! table, one handshake operation per loop step, each timed on the system
//! timer, then one transport round trip of a full frame. An operation is the
//! longest the loop will block for one client's handshake, so its worst case
//! is the latency a handshake adds to the panel and, later, to feeding. The
//! keys are throwaway, drawn from the board's entropy for this run only;
//! they are not identities and nothing is sent anywhere.

use core::fmt;

use petlibre_core::noise::{self, Handshake, Keypair, Pattern, Role, Transport};
use petlibre_core::request_auth::hmac_sha256;
use petlibre_sdk::hal::sdk;

/// Clients served at once. lwIP allows 10 TCP connections and 5 listeners
/// (the SDK's `lwipopts.h`); the MQTT transport, an OTA download and a
/// speaker stream may each hold one, which leaves this many for the API with
/// room to refuse one more.
pub const CLIENTS: usize = 4;
/// The largest API message on the wire, tag included.
pub const FRAME: usize = 1024;
/// Request IDs each client's ring remembers, so a retry inside the window
/// gets the first answer rather than running again.
pub const REQUEST_IDS: usize = 32;
/// Each pattern runs this many times; the worst run is reported.
pub const RUNS: u8 = 3;
/// The run starts this long after boot, once Wi-Fi and the runtime's
/// startup work have settled, so it measures the steady loop.
pub const START_MS: u32 = 20_000;

/// Nominal SYSTIMER rate (32.768 kHz, about 30.5 us a tick).
const TICK_HZ: u64 = 32_768;

pub const fn ticks_to_us(ticks: u32) -> u32 {
    let us = ticks as u64 * 1_000_000 / TICK_HZ;
    if us > u32::MAX as u64 {
        u32::MAX
    } else {
        #[expect(clippy::cast_possible_truncation, reason = "checked above")]
        let us = us as u32;
        us
    }
}

pub enum Noise {
    Idle,
    Handshake(Handshake),
    Transport(Transport),
}

/// One client's slot: everything the API will hold per connection.
pub struct Session {
    noise: Noise,
    rx: [u8; FRAME],
    tx: [u8; FRAME],
    rx_len: usize,
    request_ids: [[u8; 16]; REQUEST_IDS],
    next_id: usize,
}

impl Session {
    const fn new() -> Self {
        Self {
            noise: Noise::Idle,
            rx: [0; FRAME],
            tx: [0; FRAME],
            rx_len: 0,
            request_ids: [[0; 16]; REQUEST_IDS],
            next_id: 0,
        }
    }

    /// Remembers a request ID, the oldest falling out of the ring.
    fn remember(&mut self, id: [u8; 16]) {
        self.request_ids[self.next_id % REQUEST_IDS] = id;
        self.next_id = self.next_id.wrapping_add(1);
    }

    fn seen(&self, id: &[u8; 16]) -> bool {
        self.request_ids.contains(id)
    }
}

/// What one pattern's runs cost, worst run kept.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cost {
    /// One side's whole handshake: its writes and reads.
    pub initiator_us: u32,
    pub responder_us: u32,
    /// The longest single operation, on either side.
    pub worst_op_us: u32,
    /// Sealing and opening one full frame, once each way.
    pub frame_us: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    Heap {
        stage: Stage,
        free: u32,
        min: u32,
    },
    Handshake {
        pattern: Pattern,
        cost: Cost,
    },
    /// A handshake or frame failed between our own two sessions: a bug.
    Failed {
        pattern: Pattern,
        error: noise::Error,
    },
}

petlibre_core::named_enum! {
    pub enum Stage {
        Boot = "boot",
        Measured = "measured",
        Steady = "steady",
    }
}

pub fn describe(record: Record, f: &mut impl fmt::Write) -> fmt::Result {
    let pattern = |pattern: Pattern| match pattern {
        Pattern::Xx => "xx",
        Pattern::Kk => "kk",
    };
    match record {
        Record::Heap { stage, free, min } => {
            write!(f, "heap stage={stage} free={free} min={min}")
        }
        Record::Handshake { pattern: p, cost } => write!(
            f,
            "handshake pattern={} initiator_us={} responder_us={} worst_op_us={} frame_us={} frame={FRAME} clients={CLIENTS}",
            pattern(p),
            cost.initiator_us,
            cost.responder_us,
            cost.worst_op_us,
            cost.frame_us,
        ),
        Record::Failed { pattern: p, error } => {
            write!(f, "failed pattern={} error={error:?}", pattern(p))
        }
    }
}

/// Where the timing run is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Waiting,
    /// Running `pattern`, run `run`; `reading` once the last write is
    /// waiting for its reader.
    Running {
        pattern: Pattern,
        run: u8,
        reading: bool,
    },
    Done,
}

pub struct Budget {
    sessions: [Session; CLIENTS],
    phase: Phase,
    cost: Cost,
    run_cost: Cost,
    /// Length of the message waiting in the writer's `tx`.
    pending: usize,
    /// Which session wrote it.
    writer: usize,
    next_heap_ms: u32,
    /// The current step's time.
    now_ms: u32,
}

impl Budget {
    /// Const, so the firmware places it in a static: the session table is
    /// larger than the loop task's stack.
    #[expect(
        clippy::new_without_default,
        reason = "built only by this const constructor, never on a stack"
    )]
    pub const fn new() -> Self {
        const ZERO: Cost = Cost {
            initiator_us: 0,
            responder_us: 0,
            worst_op_us: 0,
            frame_us: 0,
        };
        Self {
            sessions: [const { Session::new() }; CLIENTS],
            phase: Phase::Waiting,
            cost: ZERO,
            run_cost: ZERO,
            pending: 0,
            writer: 0,
            next_heap_ms: 0,
            now_ms: 0,
        }
    }
}

/// 32 bytes for a throwaway key: the board's entropy under a label.
fn secret(label: &[u8]) -> [u8; 32] {
    let words = sdk::entropy();
    let mut bytes = [0; 16];
    let (chunks, _) = bytes.as_chunks_mut::<4>();
    for (chunk, word) in chunks.iter_mut().zip(words) {
        *chunk = word.to_le_bytes();
    }
    hmac_sha256(
        &[0x5a; 32],
        &[label, &bytes, &sdk::systimer_ticks().to_le_bytes()],
    )
}

const PROLOGUE: &[u8] = b"petlibre-fw44-budget";
const INITIATOR: usize = 0;
const RESPONDER: usize = 1;

impl Budget {
    /// One step at `now_ms`: at most one Noise operation, plus a heap
    /// record when one is due. Returns whether a timed operation ran, so
    /// the loop comes straight back.
    pub fn step(&mut self, now_ms: u32, mut record: impl FnMut(Record)) -> bool {
        self.now_ms = now_ms;
        if petlibre_core::deadline_passed(now_ms, self.next_heap_ms) {
            let stage = match self.phase {
                Phase::Waiting => Stage::Boot,
                Phase::Running { .. } => Stage::Measured,
                Phase::Done => Stage::Steady,
            };
            record(Record::Heap {
                stage,
                free: sdk::heap_free(),
                min: sdk::heap_min_free(),
            });
            self.next_heap_ms = now_ms.wrapping_add(60_000);
        }

        match self.phase {
            Phase::Waiting if petlibre_core::deadline_passed(now_ms, START_MS) => {
                self.start(Pattern::Xx, 0, &mut record);
                true
            }
            Phase::Waiting | Phase::Done => false,
            Phase::Running {
                pattern,
                run,
                reading,
            } => {
                if let Err(error) = self.operation(reading) {
                    self.fail(pattern, error, &mut record);
                    return false;
                }
                self.after(pattern, run, reading, &mut record);
                true
            }
        }
    }

    fn start(&mut self, pattern: Pattern, run: u8, record: &mut impl FnMut(Record)) {
        let initiator = Keypair::from_secret(secret(b"initiator-static"));
        let responder = Keypair::from_secret(secret(b"responder-static"));
        let remote = |keys: &Keypair| (pattern == Pattern::Kk).then_some(*keys.public());

        // Each side straight into its slot: a pair of handshakes as a
        // temporary would double the frame.
        let sides = [
            (
                INITIATOR,
                Role::Initiator,
                &initiator,
                remote(&responder),
                b"initiator-ephemeral",
            ),
            (
                RESPONDER,
                Role::Responder,
                &responder,
                remote(&initiator),
                b"responder-ephemeral",
            ),
        ];
        for (slot, role, local, remote, label) in sides {
            match Handshake::new(pattern, role, PROLOGUE, local, remote, secret(label)) {
                Ok(handshake) => self.sessions[slot].noise = Noise::Handshake(handshake),
                Err(error) => return self.fail(pattern, error, record),
            }
        }

        self.run_cost = Cost::default();
        self.phase = Phase::Running {
            pattern,
            run,
            reading: false,
        };
    }

    /// One timed operation: the writer's next message, or its reading.
    fn operation(&mut self, reading: bool) -> Result<(), noise::Error> {
        let began = sdk::systimer_ticks();

        let side = if reading {
            let reader = 1 - self.writer;
            let (first, second) = self.sessions.split_at_mut(1);
            let (from, to) = if reader == RESPONDER {
                (&first[0], &mut second[0])
            } else {
                (&second[0], &mut first[0])
            };
            let Noise::Handshake(handshake) = &mut to.noise else {
                return Err(noise::Error::WrongTurn);
            };
            to.rx_len = handshake.read_message(&from.tx[..self.pending], &mut to.rx)?;
            reader
        } else {
            let writer = if matches!(
                &self.sessions[INITIATOR].noise,
                Noise::Handshake(h) if h.is_my_turn()
            ) {
                INITIATOR
            } else {
                RESPONDER
            };
            let session = &mut self.sessions[writer];
            let Noise::Handshake(handshake) = &mut session.noise else {
                return Err(noise::Error::WrongTurn);
            };
            self.pending = handshake.write_message(&[], &mut session.tx)?;
            self.writer = writer;
            writer
        };

        let us = ticks_to_us(sdk::systimer_ticks().wrapping_sub(began));
        let total = if side == INITIATOR {
            &mut self.run_cost.initiator_us
        } else {
            &mut self.run_cost.responder_us
        };
        *total = total.saturating_add(us);
        self.run_cost.worst_op_us = self.run_cost.worst_op_us.max(us);
        Ok(())
    }

    fn after(&mut self, pattern: Pattern, run: u8, reading: bool, record: &mut impl FnMut(Record)) {
        let finished = matches!(&self.sessions[INITIATOR].noise, Noise::Handshake(h) if h.is_finished())
            && matches!(&self.sessions[RESPONDER].noise, Noise::Handshake(h) if h.is_finished());
        if !reading || !finished {
            self.phase = Phase::Running {
                pattern,
                run,
                reading: !reading,
            };
            return;
        }

        if let Err(error) = self.frame() {
            return self.fail(pattern, error, record);
        }
        let worst = |a: u32, b: u32| a.max(b);
        self.cost = Cost {
            initiator_us: worst(self.cost.initiator_us, self.run_cost.initiator_us),
            responder_us: worst(self.cost.responder_us, self.run_cost.responder_us),
            worst_op_us: worst(self.cost.worst_op_us, self.run_cost.worst_op_us),
            frame_us: worst(self.cost.frame_us, self.run_cost.frame_us),
        };

        if run + 1 < RUNS {
            return self.start(pattern, run + 1, record);
        }
        record(Record::Handshake {
            pattern,
            cost: self.cost,
        });
        self.cost = Cost::default();
        match pattern {
            Pattern::Xx => self.start(Pattern::Kk, 0, record),
            Pattern::Kk => self.finish(),
        }
    }

    /// Both sides into transport, then one full frame each way, timed,
    /// with a request ID through the initiator's ring.
    fn frame(&mut self) -> Result<(), noise::Error> {
        for session in &mut self.sessions[..2] {
            let noise = core::mem::replace(&mut session.noise, Noise::Idle);
            let Noise::Handshake(handshake) = noise else {
                return Err(noise::Error::Unfinished);
            };
            session.noise = Noise::Transport(handshake.into_transport()?);
        }

        // The payload is staged in each side's receive buffer, not on the
        // loop's stack.
        const PAYLOAD: usize = FRAME - noise::TAG_LEN;
        let (first, second) = self.sessions.split_at_mut(1);
        let (a, b) = (&mut first[0], &mut second[0]);
        a.rx[..PAYLOAD].fill(0x42);
        b.rx[..PAYLOAD].fill(0x42);
        let (Noise::Transport(ta), Noise::Transport(tb)) = (&mut a.noise, &mut b.noise) else {
            return Err(noise::Error::Unfinished);
        };

        let began = sdk::systimer_ticks();
        let len = ta.seal(&a.rx[..PAYLOAD], &mut a.tx)?;
        b.rx_len = tb.open(&a.tx[..len], &mut b.rx)?;
        let len = tb.seal(&b.rx[..PAYLOAD], &mut b.tx)?;
        a.rx_len = ta.open(&b.tx[..len], &mut a.rx)?;
        self.run_cost.frame_us = ticks_to_us(sdk::systimer_ticks().wrapping_sub(began));

        let mut id = [0; 16];
        id.copy_from_slice(&a.rx[..16]);
        if !a.seen(&id) {
            a.remember(id);
        }
        Ok(())
    }

    fn fail(&mut self, pattern: Pattern, error: noise::Error, record: &mut impl FnMut(Record)) {
        record(Record::Failed { pattern, error });
        self.finish();
    }

    fn finish(&mut self) {
        for session in &mut self.sessions {
            session.noise = Noise::Idle;
        }
        self.phase = Phase::Done;
        // The heap at the next step, then once a minute.
        self.next_heap_ms = self.now_ms.wrapping_add(1);
    }

    pub fn done(&self) -> bool {
        self.phase == Phase::Done
    }
}
