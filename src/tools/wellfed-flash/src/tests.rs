//! Host tests against a modeled helper.
//!
//! The model is synthetic NOR behind the helper's command, read, erase,
//! status and program protocol; it is not the vendor ARM code (the optional
//! Python fixture suite drives this binary against that under emulation).
//! The baseline here is synthetic, which only `Pin::Synthetic` accepts.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use serde_json::{Value, json};

use crate::evidence::Root;
use crate::evidence::journal::Receipt;
use crate::evidence::session::{Context, Pin};
use crate::link::protocol::{self, ACK, CAPACITY, EOT, NAK, Port, SECTOR, STX};
use crate::link::sector::{self, Baseline, START};
use crate::writers::development::{self, Acquired, BANK0, Layout, MARKER, Update};
use wellfed_formats::canonical::sha256;
use wellfed_image::bank::BANK1;
use wellfed_image::fixtures::{synthetic_payload, wrap};

mod banks;
mod golden;
mod recovery;
mod update;

#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
enum Fault {
    #[default]
    None,
    /// The nth erase (1-based) completes but its ACK is lost.
    LostEraseAck(usize),
    /// The nth program frame is written but its ACK is lost.
    LostProgramAck(usize),
}

enum State {
    Command,
    /// Waiting for the host's ACK of the last packet sent.
    Reading {
        at: usize,
        remaining: usize,
        number: u8,
        sent_eot: bool,
    },
    Program,
}

/// The modeled helper session.
struct Helper {
    flash: Vec<u8>,
    status: [u8; 3],
    tx: Vec<u8>,
    fault: Fault,
    input: Vec<u8>,
    output: VecDeque<u8>,
    state: State,
    erases: usize,
    frames: usize,
    erased: Vec<usize>,
}

impl Helper {
    fn new(flash: Vec<u8>) -> Self {
        assert_eq!(flash.len(), CAPACITY);
        Self {
            flash,
            status: [0, 2, 0],
            tx: Vec::new(),
            fault: Fault::None,
            input: Vec::new(),
            output: VecDeque::new(),
            state: State::Command,
            erases: 0,
            frames: 0,
            erased: Vec::new(),
        }
    }

    fn packet(&mut self, at: usize, number: u8) {
        let payload = self.flash[at..at + 1024].to_vec();
        let sum = payload.iter().fold(0u8, |s, b| s.wrapping_add(*b));
        self.output.extend([STX, number, !number]);
        self.output.extend(payload);
        self.output.push_back(sum);
    }

    fn address(&self) -> usize {
        u32::from_le_bytes([self.input[1], self.input[2], self.input[3], 0]) as usize
    }

    /// Consume complete host requests; partial ones wait for more bytes.
    fn step(&mut self) {
        loop {
            let consumed = match self.state {
                State::Reading {
                    at,
                    remaining,
                    number,
                    sent_eot,
                } => {
                    let Some(&byte) = self.input.first() else {
                        return;
                    };
                    assert_eq!(byte, ACK, "host must ACK each read packet");

                    if sent_eot {
                        self.state = State::Command;
                    } else if remaining == 0 {
                        self.output.push_back(EOT);
                        self.state = State::Reading {
                            at,
                            remaining,
                            number,
                            sent_eot: true,
                        };
                    } else {
                        self.packet(at, number);
                        self.state = State::Reading {
                            at: at + 1024,
                            remaining: remaining - 1,
                            number: number.wrapping_add(1),
                            sent_eot: false,
                        };
                    }
                    1
                }

                State::Command => match self.input.first() {
                    None => return,
                    Some(0x20) if self.input.len() >= 6 => {
                        let at = self.address();
                        let sectors = u16::from_le_bytes([self.input[4], self.input[5]]) as usize;
                        self.packet(at, 1);
                        self.state = State::Reading {
                            at: at + 1024,
                            remaining: sectors * 4 - 1,
                            number: 2,
                            sent_eot: false,
                        };
                        6
                    }

                    Some(0x17) if self.input.len() >= 6 => {
                        let at = self.address();
                        assert_eq!(&self.input[4..6], &[1, 0]);
                        assert!(at >= 0x6000, "the factory region is never erased");
                        self.flash[at..at + SECTOR].fill(0xff);
                        self.erased.push(at);
                        self.erases += 1;
                        if self.fault != Fault::LostEraseAck(self.erases) {
                            self.output.push_back(ACK);
                        }
                        6
                    }

                    Some(0x21) if self.input.len() >= 3 => {
                        let index = [0x05, 0x35, 0x15]
                            .iter()
                            .position(|op| *op == self.input[1])
                            .expect("whitelisted status opcode");
                        assert_eq!(self.input[2], 1);
                        self.output.extend([0x21, self.status[index]]);
                        3
                    }

                    Some(0x07) => {
                        self.output.push_back(ACK);
                        self.state = State::Program;
                        1
                    }

                    Some(0x20 | 0x17 | 0x21) => return,
                    Some(other) => panic!("unmodeled helper command {other:#x}"),
                },

                State::Program => match self.input.first() {
                    None => return,
                    Some(&EOT) => {
                        self.output.push_back(ACK);
                        self.state = State::Command;
                        1
                    }

                    Some(&STX) if self.input.len() >= 1032 => {
                        let frame = &self.input[..1032];
                        assert_eq!(frame[1] ^ frame[2], 0xff);
                        let sum = frame[3..1031].iter().fold(0u8, |s, b| s.wrapping_add(*b));
                        assert_eq!(sum, frame[1031]);
                        let address = u32::from_le_bytes(frame[3..7].try_into().unwrap());
                        let at = (address - sector::FLASH_BASE) as usize;
                        assert!(at >= 0x6000, "the factory region is never programmed");

                        let data = frame[7..1031].to_vec();
                        // NOR programming only clears bits.
                        for (cell, byte) in self.flash[at..at + 1024].iter_mut().zip(data) {
                            *cell &= byte;
                        }

                        self.frames += 1;
                        if self.fault != Fault::LostProgramAck(self.frames) {
                            self.output.push_back(ACK);
                        }
                        1032
                    }
                    Some(&STX) => return,
                    Some(other) => panic!("unexpected program-mode byte {other:#x}"),
                },
            };

            self.input.drain(..consumed);
        }
    }

    /// A fresh helper session over the same flash.
    fn reconnect(&mut self) {
        self.fault = Fault::None;
        self.output.clear();
        self.input.clear();
        self.state = State::Command;
    }
}

/// A shared handle, so a later session reopens the same modeled unit.
#[derive(Clone)]
struct Fake(Rc<RefCell<Helper>>);

impl Fake {
    fn new(flash: Vec<u8>) -> Self {
        Self(Rc::new(RefCell::new(Helper::new(flash))))
    }
}

impl Port for Fake {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let mut helper = self.0.borrow_mut();
        if helper.output.is_empty() {
            // Idle-ready in command mode; silence while a transfer waits.
            if matches!(helper.state, State::Command) && !buffer.is_empty() {
                buffer[0] = NAK;
                return Ok(1);
            }
            return Ok(0);
        }

        let count = buffer.len().min(helper.output.len());
        for slot in &mut buffer[..count] {
            *slot = helper.output.pop_front().unwrap();
        }
        Ok(count)
    }

    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let mut helper = self.0.borrow_mut();
        helper.tx.extend_from_slice(data);
        helper.input.extend_from_slice(data);
        helper.step();
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn in_waiting(&mut self) -> io::Result<usize> {
        Ok(self.0.borrow().output.len())
    }
}

/// The development wire restated independently of `development.rs`, as the
/// former `src/tools/flash/development-wire-policy.py` did.
fn expected_wire(old: &[u8], new: &[u8], current: &[u8], start: usize, other: usize) -> Vec<u8> {
    let read = |at: usize, size: usize| {
        let mut out = vec![0x20];
        out.extend_from_slice(&(at as u32).to_le_bytes()[..3]);
        out.extend_from_slice(&((size / SECTOR) as u16).to_le_bytes());
        out.extend(std::iter::repeat_n(ACK, size / 1024 + 1));
        out
    };

    let packet = |n: u8, at: usize, data: &[u8]| {
        let mut body = (0x0800_0000u32 + at as u32).to_le_bytes().to_vec();
        body.extend_from_slice(data);
        let sum = body.iter().fold(0u8, |s, b| s.wrapping_add(*b));
        let mut out = vec![2, n, n ^ 255];
        out.extend(body);
        out.push(sum);
        out
    };

    assert_eq!(&current[..old.len()], old);
    let mut result = new.to_vec();
    result.extend_from_slice(&current[new.len()..]);
    let mut stage = result.clone();
    stage[..8].fill(0xff);

    let acquire = [
        read(0, 0x6000),
        read(other, SECTOR),
        read(start, current.len()),
    ]
    .concat();
    let mut parts = vec![acquire.clone()];
    if result == current {
        return parts.concat();
    }

    let status: Vec<u8> = (0..2)
        .flat_map(|_| [5u8, 0x35, 0x15].map(|op| [0x21, op, 1]))
        .flatten()
        .collect();

    for i in (0..current.len()).step_by(SECTOR) {
        let data = &stage[i..i + SECTOR];
        if data == &current[i..i + SECTOR] {
            continue;
        }

        let at = start + i;
        let mut erase = vec![0x17];
        erase.extend_from_slice(&(at as u32).to_le_bytes()[..3]);
        erase.extend([1, 0]);

        parts.extend([
            read(at, SECTOR),
            status.clone(),
            erase,
            read(at, SECTOR),
            status.clone(),
            vec![7],
        ]);

        for n in 0..4 {
            parts.push(packet(
                n as u8 + 1,
                at + n * 1024,
                &data[n * 1024..(n + 1) * 1024],
            ));
        }

        parts.extend([vec![4], read(at, SECTOR)]);
    }

    parts.extend([
        acquire.clone(),
        read(start, SECTOR),
        status,
        vec![7],
        packet(1, start, &result[..1024]),
        vec![4],
        read(start, SECTOR),
        acquire,
    ]);
    parts.concat()
}

fn noise(length: usize, seed: u32) -> Vec<u8> {
    let mut state = seed.wrapping_mul(2_654_435_761) | 1;
    (0..length)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as u8
        })
        .collect()
}

/// A synthetic unit: random factory and settings, a bank 0 whose marker is
/// invalidated like the research baseline's, and `bank1` installed.
struct Unit {
    baseline: Baseline,
    flash: Vec<u8>,
}

fn unit(bank1: &[u8]) -> Unit {
    let mut baseline = noise(CAPACITY, 7);
    baseline[0x6000..0x6008].copy_from_slice(b"\x001958711");
    baseline[START..START + 8].copy_from_slice(MARKER);

    let mut flash = baseline.clone();
    flash[START..START + bank1.len()].copy_from_slice(bank1);
    // Unrelated settings drift, which every write path must preserve.
    flash[0x3e8000] ^= 1;
    flash[0x650000] ^= 1;

    Unit {
        baseline: Baseline::synthetic(baseline),
        flash,
    }
}

/// A synthetic image; its first header's magic is the loader marker.
fn image(km4: usize, seed: u8) -> Vec<u8> {
    let image = wrap(&synthetic_payload(9000, km4, seed));
    assert_eq!(&image[32..40], MARKER);
    image
}

/// A variant differing in two sectors, optionally one sector larger.
fn variant(image: &[u8], grow: bool) -> Vec<u8> {
    let mut payload = development::payload(image, &BANK1).unwrap().to_vec();
    let split = wellfed_image::ota::images(&payload).unwrap()[2].payload_header_offset;
    payload[split + 4096 + 53] ^= 1;
    payload[split + 8192 + 71] ^= 1;

    if grow {
        let size = u32::from_le_bytes(payload[split + 8..split + 12].try_into().unwrap()) as usize;
        let end = split + 32 + size;
        payload.splice(end..end, [0xa7; 4096]);
        payload[split + 8..split + 12].copy_from_slice(&((size + 4096) as u32).to_le_bytes());
    }

    wrap(&payload)
}

fn private_root() -> (tempfile::TempDir, Root) {
    let checkout = tempfile::tempdir().unwrap();
    fs::create_dir(checkout.path().join("private")).unwrap();
    let root = Root::new(checkout.path()).unwrap();
    (checkout, root)
}

fn acquire_for(
    layout: Layout,
    length: usize,
) -> impl FnMut(&mut dyn Port) -> crate::Result<Acquired> {
    move |port| {
        Ok(Acquired {
            factory: protocol::read_region(port, 0, 0x6000)?,
            other: protocol::read_region(port, layout.other.0, layout.other.1)?,
            application: protocol::read_region(port, layout.start, length)?,
        })
    }
}

/// Inspect, plan and run one transaction the way the CLI does.
fn develop(
    root: &Root,
    unit: &Unit,
    port: &mut Fake,
    update: &Update,
    output: &str,
) -> crate::Result<development::Verified> {
    let layout = update.layout;
    let length = development::extent(update.source, update.target, &layout)?;
    let current = protocol::read_region(port, layout.start, length)?;
    let other = protocol::read_region(port, layout.other.0, layout.other.1)?;
    let plan = development::plan(update, &current, Some(&other))?;

    port.0.borrow_mut().tx.clear();
    let mut acquire = acquire_for(layout, length);
    let transaction = development::Transaction {
        baseline: &unit.baseline,
        update,
        current: &current,
        other: Some(&other),
        reviewed: &plan,
    };
    development::run(
        port,
        root,
        &transaction,
        &mut acquire,
        &root.join("private").join(output),
        &mut |_, _| {},
    )
}

fn update<'a>(source: Option<&'a [u8]>, target: &'a [u8], layout: Layout) -> Update<'a> {
    Update {
        source,
        target,
        interrupted: false,
        layout,
        original: None,
        restore: false,
    }
}

fn receipt(root: &Root, output: &str) -> Receipt {
    crate::evidence::journal::inspect(
        &fs::read(root.join("private").join(output).join("journal.jsonl")).unwrap(),
    )
    .unwrap()
}

#[test]
fn the_research_pin_refuses_other_baselines() {
    assert!(matches!(
        Baseline::pinned(vec![0xff; CAPACITY]),
        Err(crate::Error::Sector(
            crate::link::sector::Error::UnpinnedBaseline { bytes: CAPACITY }
        ))
    ));
}

fn write(path: &Path, data: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, data).unwrap();
}
