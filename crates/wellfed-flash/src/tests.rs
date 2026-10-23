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

use crate::development::{self, Acquired, BANK0, BANK1, Layout, MARKER, Update};
use crate::evidence::Root;
use crate::ota::tests::{synthetic_payload, wrap};
use crate::protocol::{self, ACK, CAPACITY, EOT, NAK, Port, SECTOR, STX};
use crate::sector::{self, Baseline, START};
use crate::session::{Context, Pin};
use wellfed_formats::canonical::sha256;

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
/// former `tools/flash/development-wire-policy.py` did.
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
    let split = crate::ota::regions(&payload).unwrap()[2]["payload_header_offset"]
        .as_u64()
        .unwrap() as usize;
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
) -> crate::Result<Value> {
    let layout = update.layout;
    let length = development::extent(update.source, update.target, &layout)?;
    let current = protocol::read_region(port, layout.start, length)?;
    let other = protocol::read_region(port, layout.other.0, layout.other.1)?;
    let plan = development::plan(update, &current, Some(&other))?;

    port.0.borrow_mut().tx.clear();
    let mut acquire = acquire_for(layout, length);
    development::run(
        port,
        root,
        &unit.baseline,
        update,
        &current,
        Some(&other),
        &plan,
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

fn receipt(root: &Root, output: &str) -> Value {
    crate::journal::inspect(
        &fs::read(root.join("private").join(output).join("journal.jsonl")).unwrap(),
    )
    .unwrap()
}

#[test]
fn development_update_matches_the_independent_wire_and_changes_only_its_sectors() {
    for grow in [false, true] {
        let (_checkout, root) = private_root();
        let source = image(30000, 3);
        let target = variant(&source, grow);
        let old = development::payload(&source, &BANK1).unwrap().to_vec();
        let new = development::payload(&target, &BANK1).unwrap().to_vec();
        let unit = unit(&old);
        let mut port = Fake::new(unit.flash.clone());

        let report = develop(
            &root,
            &unit,
            &mut port,
            &update(Some(&source), &target, BANK1),
            "t",
        )
        .unwrap();

        assert_eq!(report["status"], "verified_via_supplied_transports");
        let helper = port.0.borrow();
        let length = old.len().max(new.len());
        let current = &unit.flash[START..START + length];
        assert!(helper.tx == expected_wire(&old, &new, current, START, 0x6000));

        let mut want = unit.flash.clone();
        want[START..START + new.len()].copy_from_slice(&new);
        assert!(helper.flash == want, "only the update's sectors changed");

        // The marker sector is staged first, then only changed sectors.
        assert_eq!(helper.erased[0], START);
        if !grow {
            assert_eq!(helper.erased.len(), 3);
        }

        let receipt = receipt(&root, "t");
        assert_eq!(receipt["verified_terminal"], true);
        assert_eq!(receipt["erase_order"], json!(helper.erased));
    }
}

#[test]
fn a_noop_update_acquires_once_and_writes_nothing() {
    let (_checkout, root) = private_root();
    let source = image(30000, 3);
    let unit = unit(development::payload(&source, &BANK1).unwrap());
    let mut port = Fake::new(unit.flash.clone());

    let report = develop(
        &root,
        &unit,
        &mut port,
        &update(Some(&source), &source, BANK1),
        "t",
    )
    .unwrap();

    assert_eq!(report["erase_sectors"], 0);
    let helper = port.0.borrow();
    assert!(helper.erased.is_empty());
    assert!(helper.flash == unit.flash);
    assert_eq!(receipt(&root, "t")["mutation_intent_recorded"], false);
}

#[test]
fn protection_or_a_changed_guard_stops_before_any_mutation() {
    let source = image(30000, 3);
    let target = variant(&source, false);
    let unit = unit(development::payload(&source, &BANK1).unwrap());

    // Block-protect bits covering all of the array.
    let (_checkout, root) = private_root();
    let mut port = Fake::new(unit.flash.clone());
    port.0.borrow_mut().status = [7 << 2, 2, 0];

    let error = develop(
        &root,
        &unit,
        &mut port,
        &update(Some(&source), &target, BANK1),
        "t",
    )
    .unwrap_err();

    assert!(
        error.to_string().contains("software_protected_range"),
        "{error}"
    );
    assert!(port.0.borrow().erased.is_empty());
    let stopped = receipt(&root, "t");
    assert_eq!(stopped["terminal_event"], "transaction_stopped");
    assert_eq!(stopped["mutation_intent_recorded"], false);

    // Bank 0 is neither the pinned baseline nor a valid image.
    let (_checkout, root) = private_root();
    let mut flash = unit.flash.clone();
    flash[0x6100] ^= 1;
    let mut port = Fake::new(flash);

    assert!(
        develop(
            &root,
            &unit,
            &mut port,
            &update(Some(&source), &target, BANK1),
            "t"
        )
        .is_err()
    );
    assert!(port.0.borrow().erased.is_empty());
}

#[test]
fn a_lost_ack_is_terminal_and_the_interrupted_mode_finishes_from_a_fresh_read() {
    for fault in [Fault::LostEraseAck(2), Fault::LostProgramAck(6)] {
        let (_checkout, root) = private_root();
        let source = image(30000, 3);
        let target = variant(&source, false);
        let old = development::payload(&source, &BANK1).unwrap().to_vec();
        let new = development::payload(&target, &BANK1).unwrap().to_vec();
        let unit = unit(&old);
        let mut port = Fake::new(unit.flash.clone());
        port.0.borrow_mut().fault = fault;

        let stop = develop(
            &root,
            &unit,
            &mut port,
            &update(Some(&source), &target, BANK1),
            "first",
        );

        assert!(stop.is_err());
        let stopped = receipt(&root, "first");
        assert_eq!(stopped["terminal_event"], "transaction_stopped");
        assert_eq!(stopped["mutation_intent_recorded"], true);
        let report: Value =
            serde_json::from_slice(&fs::read(root.join("private/first/report.json")).unwrap())
                .unwrap();
        assert_eq!(report["status"], "stopped");
        assert_eq!(report["mutation_possible"], true);

        port.0.borrow_mut().reconnect();

        // A plain retry is refused: bank 1 is no longer the source.
        assert!(
            develop(
                &root,
                &unit,
                &mut port,
                &update(Some(&source), &target, BANK1),
                "retry"
            )
            .is_err()
        );

        let mut finish = update(Some(&source), &target, BANK1);
        finish.interrupted = true;
        develop(&root, &unit, &mut port, &finish, "finish").unwrap();

        let mut want = unit.flash.clone();
        want[START..START + new.len()].copy_from_slice(&new);
        assert!(port.0.borrow().flash == want, "{fault:?}");
    }
}

#[test]
fn interrupted_mode_refuses_unknown_sector_content() {
    let source = image(30000, 3);
    let target = variant(&source, false);
    let old = development::payload(&source, &BANK1).unwrap().to_vec();
    let mut finish = update(Some(&source), &target, BANK1);
    finish.interrupted = true;

    // The marker sector is not staged.
    assert!(development::images(&finish, &old).is_err());

    let (_, staged) = development::images(&update(Some(&source), &target, BANK1), &old).unwrap();
    let mut current = old.clone();
    current[..SECTOR].copy_from_slice(&staged[..SECTOR]);
    assert!(development::images(&finish, &current).is_ok());

    let changed: Vec<usize> = (SECTOR..old.len())
        .step_by(SECTOR)
        .filter(|at| old[*at..at + SECTOR] != staged[*at..at + SECTOR])
        .collect();

    let partial = |sector: &mut [u8], at: usize, frames: usize| {
        sector[at..at + SECTOR].fill(0xff);
        sector[at..at + frames * 1024].copy_from_slice(&staged[at..at + frames * 1024]);
    };

    // Two partially programmed sectors.
    let mut two = current.clone();
    partial(&mut two, changed[0], 1);
    partial(&mut two, changed[1], 1);
    assert!(development::images(&finish, &two).is_err());

    // One is the state a stopped program leaves.
    let mut one = current.clone();
    partial(&mut one, changed[0], 2);
    assert!(development::images(&finish, &one).is_ok());

    // Anything else is refused.
    one[changed[0] + 4000] = 0;
    assert!(development::images(&finish, &one).is_err());
}

#[test]
fn bank0_update_keeps_bank1_valid_and_never_frames_outside_bank0() {
    let (_checkout, root) = private_root();
    let installed = image(30000, 3);
    let target = image(20000, 9);
    let unit = unit(development::payload(&installed, &BANK1).unwrap());
    let new = development::payload(&target, &BANK0).unwrap().to_vec();
    let original = unit.flash[0x6000..0x6000 + new.len()].to_vec();
    let mut bank0 = update(None, &target, BANK0);
    bank0.original = Some(&original);
    let mut port = Fake::new(unit.flash.clone());

    develop(&root, &unit, &mut port, &bank0, "t").unwrap();

    {
        let helper = port.0.borrow();
        assert!(
            helper
                .erased
                .iter()
                .all(|at| (0x6000..0x156000).contains(at))
        );
        assert_eq!(&helper.flash[0x6000..0x6000 + new.len()], &new[..]);
        assert_eq!(&helper.flash[START..START + 8], MARKER);
    }

    assert!(development::frame(&BANK0, 1, 0x156000, &[0; 1024]).is_err());
    assert!(development::frame(&BANK0, 1, 0x5c00, &[0; 1024]).is_err());

    // Bank 0 requires a valid bank 1.
    let (_checkout, root) = private_root();
    let mut flash = unit.flash.clone();
    flash[START] = 0;
    let mut port = Fake::new(flash);
    assert!(develop(&root, &unit, &mut port, &bank0, "t").is_err());
    assert!(port.0.borrow().erased.is_empty());
}

#[test]
fn give_way_then_restore_marker_round_trip() {
    let installed = image(30000, 3);
    let fallback = image(20000, 9);
    let old = development::payload(&installed, &BANK1).unwrap().to_vec();
    let bank0 = development::payload(&fallback, &BANK0).unwrap().to_vec();
    let unit = unit(&old);
    let mut flash = unit.flash.clone();
    flash[0x6000..0x6000 + bank0.len()].copy_from_slice(&bank0);
    let mut port = Fake::new(flash.clone());
    let (_checkout, root) = private_root();
    let first = protocol::read_region(&mut port, START, SECTOR).unwrap();
    let plan = development::give_way_plan(&fallback, &bank0, &first).unwrap();
    let length = bank0.len();

    let mut acquire = |port: &mut dyn Port| -> crate::Result<[Vec<u8>; 3]> {
        Ok([
            protocol::read_region(port, 0, 0x6000)?,
            protocol::read_region(port, 0x6000, length)?,
            protocol::read_region(port, START, SECTOR)?,
        ])
    };

    development::run_give_way(
        &mut port,
        &root,
        &unit.baseline,
        &fallback,
        &bank0,
        &first,
        &plan,
        &mut acquire,
        &root.join("private/give-way"),
    )
    .unwrap();

    assert_eq!(port.0.borrow().flash[START], 0);
    assert!(port.0.borrow().erased.is_empty(), "a give-way never erases");

    // Restoring needs the explicit mode; a plain update refuses.
    let plain = update(Some(&installed), &installed, BANK1);
    assert!(develop(&root, &unit, &mut port, &plain, "plain").is_err());

    let mut restore = plain;
    restore.restore = true;
    develop(&root, &unit, &mut port, &restore, "restore").unwrap();
    assert!(port.0.borrow().flash == flash);
}

#[test]
fn stock_recovery_restores_bank1_and_preserves_everything_else() {
    let (_checkout, root) = private_root();
    let installed = image(30000, 3);
    let unit = unit(development::payload(&installed, &BANK1).unwrap());

    // A bootable bank is never rewritten in place.
    let mut current = unit.flash.clone();
    assert!(crate::recovery::plan(&unit.baseline, &current).is_err());
    current[START] = 0;
    let plan = crate::recovery::plan(&unit.baseline, &current).unwrap();

    let offsets: Vec<u64> = plan["ordered_sector_payloads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["offset"].as_u64().unwrap())
        .collect();
    assert!(offsets.len() > 1);
    assert_eq!(*offsets.last().unwrap(), START as u64, "marker sector last");

    let mut port = Fake::new(current.clone());
    let mut snapshot = |port: &mut dyn Port| protocol::read_region(port, 0, CAPACITY);
    let report = crate::recovery::run_transaction(
        &mut port,
        &root,
        &unit.baseline,
        &current,
        &plan,
        &mut snapshot,
        &root.join("private/recovery"),
    )
    .unwrap();

    let mut want = current.clone();
    want[START..sector::END].copy_from_slice(&unit.baseline.bytes()[START..sector::END]);
    assert!(port.0.borrow().flash == want);
    assert_eq!(report["snapshot_sha256"], sha256(&want).as_str());

    let receipt = receipt(&root, "recovery");
    assert_eq!(receipt["verified_terminal"], true);
    assert_eq!(receipt["transaction_id"], report["transaction_id"]);

    // A changed plan is refused before any journal exists.
    let mut changed = plan.clone();
    changed["changed_sector_count"] = json!(0);
    assert!(
        crate::recovery::run_transaction(
            &mut port,
            &root,
            &unit.baseline,
            &current,
            &changed,
            &mut snapshot,
            &root.join("private/second"),
        )
        .is_err()
    );
    assert!(!root.join("private/second").exists());
}

#[test]
fn the_research_pin_refuses_other_baselines() {
    assert!(Baseline::pinned(vec![0xff; CAPACITY]).is_err());
}

fn write(path: &Path, data: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, data).unwrap();
}

#[test]
fn the_cli_refuses_an_unpinned_helper_before_any_serial_access() {
    let checkout = tempfile::tempdir().unwrap();
    fs::create_dir(checkout.path().join("private")).unwrap();

    let installed = image(30000, 3);
    let unit = unit(development::payload(&installed, &BANK1).unwrap());
    write(
        &checkout.path().join("private/baseline.bin"),
        unit.baseline.bytes(),
    );

    let opened = Rc::new(RefCell::new(0));
    let count = opened.clone();
    let context = Context {
        root: Root::new(checkout.path()).unwrap(),
        tool_sha256: sha256(b"test binary"),
        open: Box::new(move |_| {
            *count.borrow_mut() += 1;
            Ok(Box::new(Fake::new(vec![0xff; CAPACITY])))
        }),
        pin: Pin::Synthetic,
    };

    let private = |name: &str| checkout.path().join("private").join(name);
    let mut helper = vec![0u8; protocol::HELPER_BYTES];
    helper[..4].copy_from_slice(&(protocol::HELPER_ADDRESS + 0x21).to_le_bytes());
    write(&private("helper.bin"), &helper);

    let evidence = |output: &str| crate::update::Evidence {
        port: "/dev/fake".into(),
        baseline: private("baseline.bin"),
        rom_report: private("rom/report.json"),
        small_report: private("small/report.json"),
        helper: Some(private("helper.bin")),
        output: private(output),
        inspection: None,
        plan: None,
        expected_plan_sha256: None,
    };

    let review = |name: &str, image: &[u8]| -> PathBuf {
        let payload = development::payload(image, &BANK1).unwrap();
        write(&private(name).join("image.bin"), image);
        write(
            &private(name).join("review.json"),
            json!({"schema_version": 1, "kind": "plaf103_development_image_review",
                   "image_kind": "stock", "ota_sha256": sha256(image),
                   "payload_sha256": sha256(payload), "payload_bytes": payload.len(),
                   "manifest_sha256": null})
            .to_string()
            .as_bytes(),
        );
        private(name)
    };

    let args = crate::update::Args {
        mode: crate::update::Mode::Inspect,
        evidence: evidence("inspect"),
        target: review("target", &variant(&installed, false)),
        source: Some(review("source", &installed)),
        original_read: None,
        bank: 1,
        restore_marker: false,
        interrupted: false,
    };

    // A synthetic image is not the pinned stock image.
    let error = crate::update::run(&context, &args).err().unwrap();
    assert!(error.to_string().contains("original stock"), "{error}");
    assert_eq!(*opened.borrow(), 0);
    assert!(!private("inspect").exists());
    let _ = evidence("unused");
}

/// A frozen review of a firstboot build, and the checkout around it.
fn frozen_build(checkout: &Path, image: &[u8]) -> (PathBuf, Value, Value) {
    let lock = json!({"sdk": {"url": "sdk", "revision": "r1"},
                      "toolchain": {"url": "toolchain", "sha256": "t".repeat(64), "directory": "asdk"}});
    write(
        &checkout.join("firmware/sdk.lock.json"),
        lock.to_string().as_bytes(),
    );

    let mains = json!({"hp": {"address": "0x1"}, "lp": {"address": "0x2"}});
    let pinmap = json!({"matches_stock": true, "runtime_address": "0x3"});
    let manifest = json!({
        "profile": "plaf103-firstboot", "sdk_revision": "r1", "toolchain_sha256": "t".repeat(64),
        "artifacts": [{"path": "image/OTA_All.bin", "sha256": sha256(image), "bytes": image.len()}],
        "firstboot_startup_audit": {"mains": mains}, "km0_pinmap_audit": pinmap,
        "checks": {"ota_payload_matches": true, "ota_checksum_matches": true,
                   "source_snapshot_unchanged": true, "staged_sources_without_diagnostics": true},
    });

    let payload = development::payload(image, &BANK1).unwrap();
    let review = json!({
        "schema_version": 1, "kind": "plaf103_development_image_review", "image_kind": "build",
        "ota_sha256": sha256(image), "payload_sha256": sha256(payload), "payload_bytes": payload.len(),
        "manifest_sha256": sha256(manifest.to_string().as_bytes()),
        "review": {"startup_mains": mains, "pinmap": pinmap, "layout": {"sha256": sha256(image)},
                   "factory_mapping": [{"bank": 0}, {"bank": 1}]},
    });

    let directory = checkout.join("private/review");
    write(&directory.join("image.bin"), image);
    write(
        &directory.join("build-manifest.json"),
        manifest.to_string().as_bytes(),
    );
    write(
        &directory.join("review.json"),
        review.to_string().as_bytes(),
    );

    (directory, review, manifest)
}

#[test]
fn frozen_reviews_bind_image_manifest_and_required_audits() {
    let (checkout, root) = private_root();
    let image = image(30000, 3);
    let (directory, review, _) = frozen_build(checkout.path(), &image);

    let frozen = crate::review::frozen(&root, &directory, &BANK1).unwrap();
    assert_eq!(frozen.image, image);

    let refused = || crate::review::frozen(&root, &directory, &BANK1).is_err();

    // A changed image byte.
    let mut changed = image.clone();
    *changed.last_mut().unwrap() ^= 1;
    write(&directory.join("image.bin"), &changed);
    assert!(refused());
    write(&directory.join("image.bin"), &image);

    // A replaced manifest.
    let manifest = fs::read(directory.join("build-manifest.json")).unwrap();
    write(&directory.join("build-manifest.json"), b"{}");
    assert!(refused());
    write(&directory.join("build-manifest.json"), &manifest);

    // A review that names another image, sizes with a bool, or lacks the mapping.
    for (path, value) in [
        ("/ota_sha256", json!("0".repeat(64))),
        ("/payload_bytes", json!(true)),
        ("/review/factory_mapping", json!([])),
        ("/kind", json!("other")),
        ("/image_kind", json!("unknown")),
    ] {
        let mut bad = review.clone();
        *bad.pointer_mut(path).unwrap() = value;
        write(&directory.join("review.json"), bad.to_string().as_bytes());
        assert!(refused(), "{path}");
    }

    // The restored review is accepted again, so each refusal above came from
    // its own change.
    write(
        &directory.join("review.json"),
        review.to_string().as_bytes(),
    );
    assert!(!refused());
}
