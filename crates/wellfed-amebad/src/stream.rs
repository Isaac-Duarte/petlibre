//! Speaker variant: plays audio streamed over the network.
//!
//! The task listens on `speaker_stream::PORT`, one client at a time. A client
//! must pass the signed `PLSPK1` handshake (`wellfed_core::speaker_stream`)
//! under the FW08 request key; an image without a key refuses every stream.
//! After `Ok` the connection carries 16-bit mono PCM, which fills a 64 KiB
//! jitter buffer (1 s at 32 kHz). Playback starts once the buffer is half
//! full and feeds the codec's DMA pages as they free up. TCP backpressure
//! paces the sender, so no clock is shared. The stream ends when the client
//! closes, or after `STALL_MS` without data; the ring drains, then the
//! amplifier and codec close.
//!
//! The output is shared with the tune player through `audio::claim_stream`;
//! the FW15 `sound` setting refuses streams (`muted`) and `volume` scales
//! them. Nothing here touches the motor, the emitters or flash.

use core::fmt;

use wellfed_core::deadline_passed;
use wellfed_core::named_enum;
use wellfed_core::speaker_stream::{CHALLENGE_LEN, HEADER_LEN, PORT, Status, check, hello};
use wellfed_core::tone::gain_q15;

use crate::audio;
use crate::cell::TaskCell;
use crate::sdk::{self, AUDIO_PAGE_SAMPLES, TcpRecv};
use crate::settings;

pub const ACCEPT_MS: u32 = 500;
pub const HANDSHAKE_MS: u32 = 3000;
/// A connected client that sends nothing this long is dropped.
pub const STALL_MS: u32 = 3000;
const LISTEN_RETRY_MS: u32 = 5000;
const RECV_MS: u32 = 10;
const DRAIN_MS: u32 = 400;
pub const BUFFER: usize = 64 * 1024;
const PAGE_BYTES: usize = AUDIO_PAGE_SAMPLES * 2;

/// Kept out of `Streamer` so the task's slot is built without a 64 KiB
/// temporary on its stack. Only the stream task touches the buffer (see
/// `Streamer`).
static RING: TaskCell<[u8; BUFFER]> = TaskCell::new([0; BUFFER]);

named_enum! {
    /// Why a stream ended (`done reason=`).
    pub enum EndReason {
        Closed = "closed",
        Stall = "stall",
    }
}

named_enum! {
    /// What failed to start a stream (`error stage=`).
    pub enum ErrorStage {
        Open = "open",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    Listen {
        ok: bool,
    },
    Refused {
        status: Status,
    },
    Open {
        rate: u32,
        volume: u8,
    },
    Done {
        bytes: u32,
        ms: u32,
        underruns: u32,
        reason: EndReason,
    },
    Error {
        stage: ErrorStage,
    },
}

/// The body of a `PLSPK1` line.
pub fn describe(record: Record, f: &mut impl fmt::Write) -> fmt::Result {
    match record {
        Record::Listen { ok } => write!(f, "listen port={PORT} ok={}", u8::from(ok)),
        Record::Refused { status } => write!(f, "refused status={}", status.name()),
        Record::Open { rate, volume } => write!(f, "open rate={rate} volume={volume}"),
        Record::Done {
            bytes,
            ms,
            underruns,
            reason,
        } => write!(
            f,
            "done bytes={bytes} ms={ms} underruns={underruns} reason={reason}"
        ),
        Record::Error { stage } => write!(f, "error stage={stage}"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub delay_ms: u32,
    pub record: Option<Record>,
}

fn next(delay_ms: u32, record: Option<Record>) -> Step {
    Step { delay_ms, record }
}

/// The connected stages own the client's socket, so a stage that needs one
/// always has it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Listen,
    Accept,
    Handshake { socket: i32 },
    Stream { socket: i32 },
    Drain { socket: i32, since: u32 },
}

#[derive(Debug)]
pub struct Streamer {
    stage: Stage,
    listener: Option<i32>,
    challenge: [u8; CHALLENGE_LEN],
    header: [u8; HEADER_LEN],
    received: usize,
    deadline: u32,
    /// Ring read and write positions, and bytes held.
    head: usize,
    tail: usize,
    held: usize,
    playing: bool,
    ended: bool,
    gain: i32,
    started: u32,
    last_data: u32,
    bytes: u32,
    /// The DMA's underrun count when playback began, then the stream's own.
    underruns: u32,
    reason: EndReason,
}

impl Default for Streamer {
    fn default() -> Self {
        Self {
            stage: Stage::Listen,
            listener: None,
            challenge: [0; CHALLENGE_LEN],
            header: [0; HEADER_LEN],
            received: 0,
            deadline: 0,
            head: 0,
            tail: 0,
            held: 0,
            playing: false,
            ended: false,
            gain: 0,
            started: 0,
            last_data: 0,
            bytes: 0,
            underruns: 0,
            reason: EndReason::Closed,
        }
    }
}

fn challenge() -> [u8; CHALLENGE_LEN] {
    let mut out = [0; CHALLENGE_LEN];
    for (chunk, word) in out.chunks_mut(4).zip(sdk::entropy()) {
        chunk.copy_from_slice(&word.to_le_bytes());
    }
    out
}

impl Streamer {
    fn ring(&mut self) -> &mut [u8; BUFFER] {
        // SAFETY: the stream task owns `Streamer` and is the only user.
        unsafe { &mut *RING.get() }
    }

    fn hang_up(&mut self, socket: i32) {
        sdk::tcp_close(socket);
        self.stage = Stage::Accept;
    }

    fn refuse(&mut self, socket: i32, status: Status) -> Step {
        let _ = sdk::tcp_send(socket, &[status as u8]);
        self.hang_up(socket);
        next(1, Some(Record::Refused { status }))
    }

    /// Receive into the ring's free space until it is full, the socket
    /// waits, or the client closes.
    fn receive(&mut self, socket: i32, now: u32) {
        for _ in 0..8 {
            if self.held == BUFFER {
                return;
            }
            let (tail, room) = (self.tail, (BUFFER - self.held).min(BUFFER - self.tail));
            let ring = self.ring();
            match sdk::tcp_recv(socket, &mut ring[tail..tail + room], RECV_MS) {
                TcpRecv::Data(n) => {
                    self.tail = (self.tail + n) % BUFFER;
                    self.held += n;
                    self.bytes = self
                        .bytes
                        .wrapping_add(u32::try_from(n).unwrap_or(u32::MAX));
                    self.last_data = now;
                }
                TcpRecv::Timeout => return,
                TcpRecv::Closed => {
                    self.ended = true;
                    self.reason = EndReason::Closed;
                    return;
                }
            }
        }
    }

    /// Hand whole pages to the DMA; at the end, a last partial page padded
    /// with silence.
    fn feed(&mut self) {
        while self.held >= PAGE_BYTES || (self.ended && self.held >= 2) {
            let Some(page) = sdk::audio_page() else {
                return;
            };
            let take = self.held.min(PAGE_BYTES) & !1;
            let (head, gain) = (self.head, self.gain);
            // SAFETY: as in `ring`; `page` is the DMA ring's, not this buffer.
            let ring = unsafe { &*RING.get() };
            for (i, sample) in page.iter_mut().enumerate() {
                *sample = if 2 * i < take {
                    let at = (head + 2 * i) % BUFFER;
                    let raw = i16::from_le_bytes([ring[at], ring[(at + 1) % BUFFER]]);
                    // `gain` is at most 2^15, so the scaled sample stays in range.
                    i16::try_from((i32::from(raw) * gain) >> 15).unwrap_or(raw)
                } else {
                    0
                };
            }
            sdk::audio_submit();
            self.head = (self.head + take) % BUFFER;
            self.held -= take;
        }
        if self.ended {
            // An odd trailing byte is not a sample.
            self.held = 0;
        }
    }

    fn finish(&mut self, socket: i32, now: u32) -> Record {
        audio::close_output();
        audio::release_stream();
        self.hang_up(socket);
        Record::Done {
            bytes: self.bytes,
            ms: now.wrapping_sub(self.started),
            underruns: self.underruns,
            reason: self.reason,
        }
    }

    pub fn step(&mut self) -> Step {
        let now = sdk::uptime_ms();
        match self.stage {
            Stage::Listen => {
                self.listener = sdk::tcp_listen(PORT);
                let ok = self.listener.is_some();
                if ok {
                    self.stage = Stage::Accept;
                }
                next(
                    if ok { 1 } else { LISTEN_RETRY_MS },
                    Some(Record::Listen { ok }),
                )
            }

            Stage::Accept => {
                let Some(listener) = self.listener else {
                    self.stage = Stage::Listen;
                    return next(1, None);
                };
                let Some(socket) = sdk::tcp_accept(listener, ACCEPT_MS) else {
                    return next(1, None);
                };
                self.challenge = challenge();
                if !sdk::tcp_send(socket, &hello(&self.challenge)) {
                    self.hang_up(socket);
                    return next(1, None);
                }
                self.received = 0;
                self.deadline = now.wrapping_add(HANDSHAKE_MS);
                self.stage = Stage::Handshake { socket };
                next(1, None)
            }

            Stage::Handshake { socket } => {
                if deadline_passed(now, self.deadline) {
                    return self.refuse(socket, Status::BadHeader);
                }
                let received = self.received;
                match sdk::tcp_recv(socket, &mut self.header[received..], RECV_MS) {
                    TcpRecv::Data(n) => self.received += n,
                    TcpRecv::Timeout => {}
                    TcpRecv::Closed => {
                        self.hang_up(socket);
                        return next(1, None);
                    }
                }
                if self.received < HEADER_LEN {
                    return next(1, None);
                }

                let key = sdk::transport_config().and_then(|config| config.request_key);
                let rate = match key.map(|key| check(&key, &self.challenge, &self.header)) {
                    None => Err(Status::Unauthenticated),
                    Some(result) => result,
                };
                let rate = match rate {
                    Ok(rate) => rate,
                    Err(status) => return self.refuse(socket, status),
                };
                let Some(view) = settings::current() else {
                    return self.refuse(socket, Status::Busy);
                };
                if !view.sound {
                    return self.refuse(socket, Status::Muted);
                }
                if !audio::claim_stream() {
                    return self.refuse(socket, Status::Busy);
                }
                if !audio::open_output(rate) {
                    audio::release_stream();
                    let _ = sdk::tcp_send(socket, &[Status::Busy as u8]);
                    self.hang_up(socket);
                    return next(
                        1,
                        Some(Record::Error {
                            stage: ErrorStage::Open,
                        }),
                    );
                }
                if !sdk::tcp_send(socket, &[Status::Ok as u8]) {
                    audio::close_output();
                    audio::release_stream();
                    self.hang_up(socket);
                    return next(1, None);
                }

                (self.head, self.tail, self.held) = (0, 0, 0);
                (self.playing, self.ended) = (false, false);
                self.gain = gain_q15(view.volume);
                (self.started, self.last_data) = (now, now);
                self.bytes = 0;
                self.reason = EndReason::Closed;
                self.stage = Stage::Stream { socket };
                next(
                    1,
                    Some(Record::Open {
                        rate,
                        volume: view.volume,
                    }),
                )
            }

            Stage::Stream { socket } => {
                if !self.ended {
                    self.receive(socket, now);
                    if !self.ended && now.wrapping_sub(self.last_data) >= STALL_MS {
                        self.ended = true;
                        self.reason = EndReason::Stall;
                    }
                }
                if !self.playing && (self.held >= BUFFER / 2 || self.ended) {
                    self.playing = true;
                    self.underruns = sdk::audio_underruns();
                }
                if self.playing {
                    self.feed();
                }
                if self.ended && self.held == 0 {
                    // Counted now: once the ring drains, the DMA plays zero
                    // pages by design.
                    self.underruns = sdk::audio_underruns().wrapping_sub(self.underruns);
                    self.stage = Stage::Drain { socket, since: now };
                }
                next(1, None)
            }

            Stage::Drain { socket, since } => {
                if sdk::audio_drained() || now.wrapping_sub(since) >= DRAIN_MS {
                    return next(1, Some(self.finish(socket, now)));
                }
                next(5, None)
            }
        }
    }
}
