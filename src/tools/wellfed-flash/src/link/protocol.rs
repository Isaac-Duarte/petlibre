//! The ROM loader and pinned vendor helper's UART protocol.
//!
//! Port of `src/tools/flash/uart-flash-read.py`. The helper executes in RAM and
//! changes controller state, including OTF decryption; speaking its protocol
//! here does not certify ROM or recovery safety. There are no baud changes,
//! cancellations, retries or fallback images.

use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::error::Result;
use wellfed_formats::canonical::sha256;
use wellfed_formats::require;

/// The loader, helper and console protocol's refusals. Each stops the
/// session with the transport left as it is; none is retried.
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("Interrupted by the operator; no retry")]
    Interrupted,
    #[error("Helper does not match the reviewed vendor binary")]
    HelperMismatch { bytes: usize },
    #[error("Unexpected helper entry pointer")]
    HelperEntry { entry: Option<u32> },
    #[error("Only the pinned helper may be uploaded")]
    UnpinnedHelper { bytes: usize },
    #[error("Invalid helper packet index")]
    HelperPacket { index: usize },

    #[error("Read range must be positive and aligned to 4 KiB")]
    ReadUnaligned { offset: usize, length: usize },
    #[error("Read exceeds the identified flash capacity")]
    ReadBeyondCapacity { offset: usize, length: usize },
    #[error("Read exceeds the helper's sector count")]
    ReadSectorCount { length: usize },
    #[error("Address outside the 8 MiB array")]
    AddressOutside { offset: usize },

    #[error("Serial read deadline expired")]
    SerialDeadline,
    #[error("Partial serial write; aborting without replay")]
    PartialWrite { written: usize, length: usize },
    #[error("Unexpected protocol byte {actual:#04x}; expected {expected:#04x}")]
    UnexpectedByte { actual: u8, expected: u8 },
    #[error("Loader NAK deadline expired")]
    NakDeadline,
    #[error("Unexpected {0} output")]
    UnexpectedOutput(Pending),
    #[error("BAUDCHK acknowledgement deadline expired")]
    BaudcheckDeadline,
    #[error("Unexpected BAUDCHK response")]
    BaudcheckResponse { byte: u8 },

    #[error("Overall read deadline expired")]
    ReadDeadline,
    #[error("Premature EOT; incomplete read")]
    PrematureEot { received: usize, length: usize },
    #[error("Unexpected read control {control:#04x}; no fallback")]
    ReadControl { control: u8 },
    #[error("Packet number/complement mismatch")]
    PacketComplement { number: u8, complement: u8 },
    #[error("Read packet checksum mismatch")]
    PacketChecksum { number: u8 },
    #[error("Changed or excessive duplicate packet")]
    ChangedDuplicate { number: u8 },
    #[error("Out-of-order read packet")]
    OutOfOrder { number: u8, expected: u8 },
    #[error("Read exceeded requested length")]
    ReadOverrun { length: usize },
    #[error("Incomplete region read")]
    IncompleteRegion { received: usize, length: usize },

    #[error("Unexpectedly large console response")]
    ConsoleOverflow { bytes: usize },
    #[error("Observation must be 1-180 seconds")]
    ObservationWindow { seconds: f64 },
    #[error("Console command is outside the fixed ROM-inspection profile")]
    ConsoleCommand,
    #[error("Fault or startup observed during console query; stop and inspect capture")]
    ConsoleFault,
    #[error("Unexpected pre-existing fault output")]
    PreexistingFault,
    #[error("Expected stock KM4 command table not observed; no core switch attempted")]
    Km4Table,
    #[error("Expected stock KM0 help table not observed; no ROM read attempted")]
    Km0Table,
    #[error("Expected exactly one matching ROM word; no retry")]
    RomWordCount { address: u32, found: usize },
    #[error("ROM differs from candidate at {address:#x}; helper upload blocked")]
    RomDiffers { address: u32 },
    #[error("KM4 console return not verified")]
    Km4Return,
}

/// Where unexpected bytes were already buffered: before a helper command,
/// before the helper upload, or before a read request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pending {
    Command,
    Upload,
    Read,
}

impl fmt::Display for Pending {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Command => "buffered helper",
            Self::Upload => "pending loader",
            Self::Read => "pending helper",
        })
    }
}

/// The reviewed vendor flash helper, which runs from RAM; no other bytes are
/// ever uploaded.
pub const HELPER_SHA256: &str = "9307121385cb390dfd2da64da2c6c515f17b5a9556b3d04021487c9b9f220b55";
pub const HELPER_BYTES: usize = 4688;
pub const HELPER_ADDRESS: u32 = 0x82000;

pub const CAPACITY: usize = 8 * 1024 * 1024;
pub const SECTOR: usize = 4096;

pub const ACK: u8 = 6;
pub const NAK: u8 = 21;
pub const EOT: u8 = 4;
pub const STX: u8 = 2;

/// The SDK ROM words sampled before any helper upload.
pub const ROM_POINTS: [u32; 17] = [
    0x1298, 0x0474, 0x1434, 0x14C0, 0x1504, 0x2E9C, 0x3594, 0x40B0, 0x43D4, 0x61A0, 0x61D4, 0x6220,
    0x6250, 0x6274, 0x628C, 0x6590, 0x66AC,
];
pub const ROM_BYTES: usize = 54144;
pub const ROM_SHA256: [&str; 2] = [
    "301be574cc338a0bb0a3516f691ec4409064be39fc4144ebfa46488ede9e1612",
    "db0fdd8b2232a0d9edaa403715a9992ae36944fbda4dd936e017c1fa531e66d6",
];

/// Set by the operator's Ctrl-C. Every transport wait checks it and stops
/// terminally, as Python's `KeyboardInterrupt` does; nothing is retried.
pub static INTERRUPTED: AtomicBool = AtomicBool::new(false);

pub fn interrupted() -> Result<()> {
    require(!INTERRUPTED.load(Ordering::SeqCst), || {
        ProtocolError::Interrupted.into()
    })
}

/// A serial-shaped byte transport. `read` returns 0 when nothing arrives
/// within the transport's short poll interval.
pub trait Port {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize>;
    fn write(&mut self, data: &[u8]) -> io::Result<usize>;
    fn flush(&mut self) -> io::Result<()>;
    /// Bytes already received and not yet read.
    fn in_waiting(&mut self) -> io::Result<usize>;
}

pub fn load_helper(data: Vec<u8>) -> Result<Vec<u8>> {
    require(
        data.len() == HELPER_BYTES && sha256(&data) == HELPER_SHA256,
        || ProtocolError::HelperMismatch { bytes: data.len() },
    )?;
    let entry = data.first_chunk().map(|word| u32::from_le_bytes(*word));
    require(entry == Some(HELPER_ADDRESS + 0x21), || {
        ProtocolError::HelperEntry { entry }
    })?;
    Ok(data)
}

pub fn read_request(offset: usize, length: usize) -> Result<[u8; 6]> {
    require(
        length > 0 && offset.is_multiple_of(SECTOR) && length.is_multiple_of(SECTOR),
        || ProtocolError::ReadUnaligned { offset, length },
    )?;
    require(offset + length <= CAPACITY, || {
        ProtocolError::ReadBeyondCapacity { offset, length }
    })?;
    let [low, middle, high] = address(offset)?;
    let Ok(sectors) = u16::try_from(length / SECTOR) else {
        return Err(ProtocolError::ReadSectorCount { length }.into());
    };

    let [count_low, count_high] = sectors.to_le_bytes();
    Ok([0x20, low, middle, high, count_low, count_high])
}

/// The three address bytes requests carry; the 8 MiB array needs no more.
fn address(offset: usize) -> Result<[u8; 3]> {
    require(offset < CAPACITY, || ProtocolError::AddressOutside {
        offset,
    })?;
    let Ok(word) = u32::try_from(offset) else {
        return Err(ProtocolError::AddressOutside { offset }.into());
    };

    let [low, middle, high, _] = word.to_le_bytes();
    Ok([low, middle, high])
}

pub fn erase_request(offset: usize) -> Result<[u8; 6]> {
    let [low, middle, high] = address(offset)?;
    Ok([0x17, low, middle, high, 0x01, 0x00])
}

fn checksum(body: &[u8]) -> u8 {
    body.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte))
}

/// One STX frame: packet number, its complement, a 4-byte address, 1 KiB
/// and an 8-bit sum. Callers bound the address before framing.
pub fn frame(number: u8, address: u32, payload: &[u8; 1024]) -> Vec<u8> {
    let mut body = Vec::with_capacity(1028);
    body.extend_from_slice(&address.to_le_bytes());
    body.extend_from_slice(payload);
    let mut out = vec![STX, number, number ^ 0xff];
    let sum = checksum(&body);
    out.extend_from_slice(&body);
    out.push(sum);
    out
}

fn pinned_helper(helper: &[u8]) -> Result<()> {
    require(
        helper.len() == HELPER_BYTES && sha256(helper) == HELPER_SHA256,
        || {
            ProtocolError::UnpinnedHelper {
                bytes: helper.len(),
            }
            .into()
        },
    )
}

pub fn upload_frame(helper: &[u8], index: usize) -> Result<Vec<u8>> {
    pinned_helper(helper)?;
    require(index < helper.len().div_ceil(1024), || {
        ProtocolError::HelperPacket { index }
    })?;

    let mut payload = [0xffu8; 1024];
    let chunk = &helper[index * 1024..helper.len().min((index + 1) * 1024)];
    payload[..chunk.len()].copy_from_slice(chunk);

    // The pinned helper is five packets, so both always fit.
    let (Ok(number), Ok(at)) = (u8::try_from(index + 1), u32::try_from(index * 1024)) else {
        return Err(ProtocolError::HelperPacket { index }.into());
    };
    Ok(frame(number, HELPER_ADDRESS + at, &payload))
}

pub fn exact_read(port: &mut dyn Port, count: usize, timeout: Duration) -> Result<Vec<u8>> {
    let deadline = Instant::now() + timeout;
    let mut data = vec![0; count];
    let mut filled = 0;
    while filled < count {
        require(Instant::now() < deadline, || ProtocolError::SerialDeadline)?;
        interrupted()?;
        filled += port.read(&mut data[filled..])?;
    }
    Ok(data)
}

pub fn send(port: &mut dyn Port, data: &[u8]) -> Result<()> {
    // Checked after the intent's fsync and before the bytes, as Python's
    // KeyboardInterrupt lands before `send` runs.
    interrupted()?;
    let written = port.write(data)?;
    require(written == data.len(), || ProtocolError::PartialWrite {
        written,
        length: data.len(),
    })?;
    Ok(port.flush()?)
}

pub fn expect(port: &mut dyn Port, value: u8) -> Result<()> {
    let actual = exact_read(port, 1, Duration::from_secs(3))?[0];
    require(actual == value, || {
        ProtocolError::UnexpectedByte {
            actual,
            expected: value,
        }
        .into()
    })
}

/// Require consecutive NAKs; any preceding banner stays in the transport log.
pub fn wait_naks(port: &mut dyn Port, count: usize, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let mut seen = 0;
    let mut byte = [0u8; 1];
    while seen < count {
        require(Instant::now() < deadline, || ProtocolError::NakDeadline)?;
        interrupted()?;
        if port.read(&mut byte)? == 0 {
            continue;
        }
        seen = if byte[0] == NAK { seen + 1 } else { 0 };
    }
    Ok(())
}

/// Drain only idle-ready NAK bytes that are already buffered.
pub fn drain_naks(port: &mut dyn Port, before: Pending) -> Result<()> {
    let pending = port.in_waiting()?;
    if pending > 0 {
        let data = exact_read(port, pending, Duration::from_secs(1))?;
        require(data.iter().all(|byte| *byte == NAK), || {
            ProtocolError::UnexpectedOutput(before)
        })?;
    }
    Ok(())
}

/// Helper idle-ready: fresh NAKs, then nothing else buffered.
pub fn command_ready(port: &mut dyn Port) -> Result<()> {
    wait_naks(port, 3, Duration::from_secs(5))?;
    drain_naks(port, Pending::Command)
}

/// One request; only already-in-flight NAKs may precede its ACK.
pub fn baudcheck(port: &mut dyn Port) -> Result<()> {
    send(port, &[0x07])?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        require(Instant::now() < deadline, || {
            ProtocolError::BaudcheckDeadline
        })?;
        match exact_read(port, 1, Duration::from_secs(1))?[0] {
            ACK => return Ok(()),
            NAK => {}
            byte => return Err(ProtocolError::BaudcheckResponse { byte }.into()),
        }
    }
}

/// Upload the pinned helper to an already-observed ROM loader.
pub fn upload_helper(port: &mut dyn Port, helper: &[u8]) -> Result<()> {
    pinned_helper(helper)?;
    wait_naks(port, 3, Duration::from_secs(5))?;
    drain_naks(port, Pending::Upload)?;
    baudcheck(port)?;

    for index in 0..helper.len().div_ceil(1024) {
        send(port, &upload_frame(helper, index)?)?;
        expect(port, ACK)?;
    }

    send(port, &[EOT])?;
    expect(port, ACK)?;
    wait_naks(port, 3, Duration::from_secs(5))
}

/// A completed read as reports record it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Transfer {
    pub bytes: usize,
    pub packets: usize,
    pub duplicate_packets: usize,
}

/// Exact checksum-framed read; malformed, partial or cancelled reads fail closed.
///
/// A bounded duplicate of the previous packet is accepted only with an
/// identical payload (a lost ACK). Neither requests nor damaged packets are
/// retried.
pub fn receive_flash(
    port: &mut dyn Port,
    offset: usize,
    length: usize,
    output: &mut dyn Write,
    mut progress: Option<&mut dyn FnMut(usize, usize)>,
) -> Result<Transfer> {
    let request = read_request(offset, length)?;
    wait_naks(port, 3, Duration::from_secs(5))?;
    drain_naks(port, Pending::Read)?;
    send(port, &request)?;

    let mut total = 0;
    let mut sequence: usize = 1;
    let mut previous: Option<Vec<u8>> = None;
    let mut duplicates = 0;
    // 30 s plus 1/4000 s per byte for the whole read; read_request
    // has already bounded the length by the 8 MiB capacity.
    let per_byte =
        Duration::from_micros(250).saturating_mul(u32::try_from(length).unwrap_or(u32::MAX));
    let deadline = Instant::now() + Duration::from_secs(30) + per_byte;

    loop {
        require(Instant::now() < deadline, || ProtocolError::ReadDeadline)?;
        interrupted()?;
        let control = exact_read(port, 1, Duration::from_secs(5))?[0];
        if control == EOT {
            require(total == length, || ProtocolError::PrematureEot {
                received: total,
                length,
            })?;
            send(port, &[ACK])?;
            wait_naks(port, 3, Duration::from_secs(5))?;
            return Ok(Transfer {
                bytes: total,
                packets: total / 1024,
                duplicate_packets: duplicates,
            });
        }

        require(control == STX, || ProtocolError::ReadControl { control })?;

        let frame = exact_read(port, 1027, Duration::from_secs(3))?;
        let (number, complement) = (frame[0], frame[1]);
        let payload = &frame[2..1026];
        require(number ^ complement == 0xff, || {
            ProtocolError::PacketComplement { number, complement }
        })?;
        require(checksum(payload) == frame[1026], || {
            ProtocolError::PacketChecksum { number }
        })?;

        if previous.is_some() && number == (sequence - 1).to_le_bytes()[0] {
            require(
                previous.as_deref() == Some(payload) && duplicates < 8,
                || ProtocolError::ChangedDuplicate { number },
            )?;
            duplicates += 1;
            send(port, &[ACK])?;
            continue;
        }

        let expected = sequence.to_le_bytes()[0];
        require(number == expected, || ProtocolError::OutOfOrder {
            number,
            expected,
        })?;
        require(total + payload.len() <= length, || {
            ProtocolError::ReadOverrun { length }
        })?;

        output.write_all(payload)?;
        total += payload.len();
        previous = Some(payload.to_vec());
        sequence += 1;
        send(port, &[ACK])?;

        if let Some(progress) = progress.as_mut()
            && (total % (256 * 1024) == 0 || total == length)
        {
            progress(total, length);
        }
    }
}

/// Read one region into memory.
pub fn read_region(port: &mut dyn Port, offset: usize, length: usize) -> Result<Vec<u8>> {
    let mut output = Vec::with_capacity(length);
    receive_flash(port, offset, length, &mut output, None)?;
    require(output.len() == length, || ProtocolError::IncompleteRegion {
        received: output.len(),
        length,
    })?;
    Ok(output)
}

pub fn collect(port: &mut dyn Port, timeout: Duration) -> Result<Vec<u8>> {
    let deadline = Instant::now() + timeout;
    let mut result = Vec::new();
    let mut buffer = [0u8; 4096];
    while Instant::now() < deadline {
        interrupted()?;
        let count = port.read(&mut buffer)?;
        result.extend_from_slice(&buffer[..count]);
        require(result.len() <= 512 * 1024, || {
            ProtocolError::ConsoleOverflow {
                bytes: result.len(),
            }
        })?;
    }
    Ok(result)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn count(haystack: &[u8], needle: &[u8]) -> usize {
    let mut found = 0;
    let mut at = 0;
    while at + needle.len() <= haystack.len() {
        if &haystack[at..at + needle.len()] == needle {
            found += 1;
            at += needle.len();
        } else {
            at += 1;
        }
    }
    found
}

const FAULTS: [&[u8]; 3] = [b"Hard Fault", b"HardFault", b"Fault on thread"];

/// What a receive-only observation saw.
#[derive(Debug, Clone, Serialize)]
pub struct Observation {
    pub bytes: usize,
    pub transmitted_bytes: usize,
    pub nak_bytes: usize,
    pub longest_nak_run: usize,
    pub download_banner_seen: bool,
    pub stock_startup_markers: usize,
    pub fault_marker_seen: bool,
    pub limit: &'static str,
}

/// The bounded receive-only window, checked before the port opens. The range
/// is checked on the operator's fractional seconds before they become a span,
/// so rounding to whole nanoseconds can never admit a value just outside it.
pub fn observation_window(seconds: f64) -> Result<Duration, ProtocolError> {
    require((1.0..=180.0).contains(&seconds), || {
        ProtocolError::ObservationWindow { seconds }
    })?;
    Duration::try_from_secs_f64(seconds).map_err(|_| ProtocolError::ObservationWindow { seconds })
}

/// Receive only: NAKs are observations, never an automatic loader assertion.
/// The window is checked again here, after the port opens.
pub fn observe(port: &mut dyn Port, window: Duration) -> Result<Observation> {
    require(
        (Duration::from_secs(1)..=Duration::from_secs(180)).contains(&window),
        || ProtocolError::ObservationWindow {
            seconds: window.as_secs_f64(),
        },
    )?;

    let data = collect(port, window)?;
    let longest = data
        .split(|byte| *byte != NAK)
        .map(<[u8]>::len)
        .max()
        .unwrap_or(0);

    Ok(Observation {
        bytes: data.len(),
        transmitted_bytes: 0,
        nak_bytes: data.iter().filter(|byte| **byte == NAK).count(),
        longest_nak_run: longest,
        download_banner_seen: contains(&data, b"#Flash Download Start"),
        stock_startup_markers: count(&data, b"app_main:version:"),
        fault_marker_seen: FAULTS.iter().any(|fault| contains(&data, fault)),
        limit: "Serial output only; operator must establish power/strap state and loader identity",
    })
}

fn rom_command(command: &[u8]) -> bool {
    command == b"?\r\n"
        || command == b">>\r\n"
        || (command.len() == 13
            && command.starts_with(b"DW 0000")
            && command.ends_with(b"\r\n")
            && command[7..11]
                .iter()
                .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(b)))
}

pub fn console_query(port: &mut dyn Port, command: &[u8], window: Duration) -> Result<Vec<u8>> {
    require(rom_command(command), || ProtocolError::ConsoleCommand)?;

    send(port, command)?;
    let reply = collect(port, window)?;
    require(
        !FAULTS
            .iter()
            .chain([&b"app_main:version:"[..]].iter())
            .any(|marker| contains(&reply, marker)),
        || ProtocolError::ConsoleFault,
    )?;

    Ok(reply)
}

pub fn is_km0_help(reply: &[u8]) -> bool {
    [
        &b"TEST COMMAND MODE HELP 12"[..],
        b"switch shell to KM4",
        b"HSSDM32K",
        b"CTC",
        b"FWDBG",
        b"TEST COMMAND MODE END  c",
    ]
    .iter()
    .all(|marker| contains(reply, marker))
        && !contains(reply, b"switch shell to KM0")
}

fn is_km4_help(reply: &[u8]) -> bool {
    contains(reply, b"switch shell to KM0") && contains(reply, b"0e11df78")
}

/// Every `ADDRESS:<spaces>XXXXXXXX` word in a `DW` reply.
fn dumped_words(reply: &[u8], address: u32) -> Vec<u32> {
    let label = format!("{address:08X}:");
    let label = label.as_bytes();
    let mut values = Vec::new();
    let mut at = 0;

    while at + label.len() <= reply.len() {
        if &reply[at..at + label.len()] != label {
            at += 1;
            continue;
        }

        let mut cursor = at + label.len();
        // Python's bytes `\s`, which also includes vertical tab.
        while cursor < reply.len()
            && matches!(reply[cursor], b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
        {
            cursor += 1;
        }
        let digits = &reply[cursor..reply.len().min(cursor + 8)];
        let word = std::str::from_utf8(digits)
            .ok()
            .filter(|digits| digits.len() == 8 && digits.bytes().all(|b| b.is_ascii_hexdigit()))
            .and_then(|digits| u32::from_str_radix(digits, 16).ok());
        match word {
            Some(word) => {
                values.push(word);
                at = cursor + 8;
            }
            None => at += 1,
        }
    }
    values
}

pub fn rom_sample_addresses() -> Vec<u32> {
    ROM_POINTS
        .iter()
        .flat_map(|base| (0..4).map(move |word| (base & !3) + word * 4))
        .collect()
}

/// The core that owns the stock console when ROM inspection starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Core {
    Km4,
    Km0,
}

#[derive(Debug, Clone, Serialize)]
pub struct Sample {
    pub address: String,
    pub value: String,
}

/// The sampled ROM words and the pinned candidates they still match.
#[derive(Debug, Clone, Serialize)]
pub struct RomInspection {
    pub sampled_words: Vec<Sample>,
    /// Candidate path to its SHA-256.
    pub matching_candidates: BTreeMap<String, String>,
    pub returned_to_km4: bool,
    pub limit: &'static str,
}

/// Enter KM0 only after verifying both actual help-table identities.
pub fn inspect_rom(
    port: &mut dyn Port,
    candidates: &[(String, Vec<u8>)],
    start: Core,
) -> Result<RomInspection> {
    let initial = collect(port, Duration::from_millis(500))?;
    require(!contains(&initial, b"Hard Fault"), || {
        ProtocolError::PreexistingFault
    })?;

    if start == Core::Km4 {
        let high = console_query(port, b"?\r\n", Duration::from_secs(1))?;
        require(is_km4_help(&high), || ProtocolError::Km4Table)?;
        console_query(port, b">>\r\n", Duration::from_millis(500))?;
    }

    let low = console_query(port, b"?\r\n", Duration::from_secs(1))?;
    require(is_km0_help(&low), || ProtocolError::Km0Table)?;

    let mut samples = Vec::new();
    let mut surviving: Vec<&(String, Vec<u8>)> = candidates.iter().collect();

    for address in rom_sample_addresses() {
        let command = format!("DW {address:08X}\r\n");
        let response = console_query(port, command.as_bytes(), Duration::from_millis(250))?;
        let values = dumped_words(&response, address);
        require(values.len() == 1, || ProtocolError::RomWordCount {
            address,
            found: values.len(),
        })?;

        let value = values[0];
        let at = address as usize;
        surviving.retain(|(_, rom)| rom.get(at..at + 4) == Some(&value.to_le_bytes()[..]));
        require(!surviving.is_empty(), || ProtocolError::RomDiffers {
            address,
        })?;

        samples.push(Sample {
            address: format!("0x{address:08x}"),
            value: format!("0x{value:08x}"),
        });
    }

    console_query(port, b">>\r\n", Duration::from_millis(500))?;
    let high = console_query(port, b"?\r\n", Duration::from_secs(1))?;
    require(is_km4_help(&high), || ProtocolError::Km4Return)?;

    Ok(RomInspection {
        sampled_words: samples,
        matching_candidates: surviving
            .iter()
            .map(|(name, rom)| (name.clone(), sha256(rom)))
            .collect(),
        returned_to_km4: true,
        limit: "Sampled ROM identity only; not whole-ROM or all-side-effect verification",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_aligned_and_bounded() {
        assert_eq!(
            read_request(0x186000, 0x2000).unwrap(),
            [0x20, 0x00, 0x60, 0x18, 0x02, 0x00]
        );
        assert!(matches!(
            read_request(0x100, 4096),
            Err(crate::Error::Protocol(ProtocolError::ReadUnaligned {
                offset: 0x100,
                ..
            }))
        ));
        assert!(matches!(
            read_request(0, 0),
            Err(crate::Error::Protocol(ProtocolError::ReadUnaligned {
                length: 0,
                ..
            }))
        ));
        assert!(matches!(
            read_request(CAPACITY - 4096, 8192),
            Err(crate::Error::Protocol(
                ProtocolError::ReadBeyondCapacity { .. }
            ))
        ));
        assert_eq!(
            erase_request(0x186000).unwrap(),
            [0x17, 0x00, 0x60, 0x18, 1, 0]
        );
        assert!(matches!(
            erase_request(CAPACITY),
            Err(crate::Error::Protocol(ProtocolError::AddressOutside {
                offset: CAPACITY
            }))
        ));
        assert_eq!(
            ProtocolError::UnexpectedOutput(Pending::Upload).to_string(),
            "Unexpected pending loader output"
        );
        assert_eq!(
            ProtocolError::UnexpectedByte {
                actual: 0x15,
                expected: 6
            }
            .to_string(),
            "Unexpected protocol byte 0x15; expected 0x06"
        );
    }

    #[test]
    fn frames_carry_complement_address_and_sum() {
        let frame = frame(3, 0x0818_6000, &[1; 1024]);
        assert_eq!(&frame[..7], &[STX, 3, 0xfc, 0x00, 0x60, 0x18, 0x08]);
        assert_eq!(frame.len(), 1032);
        let sum = (0x60u32 + 0x18 + 0x08 + 1024) as u8;
        assert_eq!(frame[1031], sum);
    }

    #[test]
    fn rom_profile_allows_only_fixed_commands() {
        assert!(rom_command(b"?\r\n"));
        assert!(rom_command(b"DW 00001298\r\n"));
        assert!(!rom_command(b"DW 00001298 4\r\n"));
        assert!(!rom_command(b"DW 0000129a\r\n"));
        assert!(!rom_command(b"EW 00001298\r\n"));
    }

    #[test]
    fn dumped_word_parsing_matches_the_regex() {
        let reply = b"DW 00001298\r\n00001298:  4B02B510\r\n";
        assert_eq!(dumped_words(reply, 0x1298), vec![0x4B02B510]);
        assert!(dumped_words(b"00001298: 4B02", 0x1298).is_empty());
        assert_eq!(rom_sample_addresses().len(), 68);
        assert_eq!(rom_sample_addresses()[..2], [0x1298, 0x129c]);
    }
}
