//! The log tail: the newest record lines in a RAM ring, for the device
//! API to stream to an owner while the UART prints them as before.
//!
//! Every line the SDK's `log` prints is also copied in here, from a task or
//! an interrupt, so the ring never blocks and never waits:
//! - **One writer at a time, never waiting for another.** On the KM4 a
//!   task logs with the scheduler suspended, so the only writer that can
//!   find another mid-copy is an interrupt that preempted it. That writer
//!   drops its line (the UART still prints it) and counts it, rather than
//!   wait for a writer that cannot run until it returns. A dropped line
//!   still takes a number, so a reader sees the hole as dropped lines,
//!   but only once the next line is written: until then nothing shows it.
//! - **Readers never block writers.** A reader copies a line, then checks
//!   that the oldest line kept has not moved past it meanwhile (a seqlock);
//!   if it has, it starts over from the new oldest line, a few times at
//!   most, and otherwise tries again on its next pass.
//! - **The oldest lines go first.** A line that needs room drops whole
//!   lines from the old end; a reader whose cursor fell behind them learns
//!   how many it missed from the numbers.
//!
//! Lines are numbered from 1 each boot (the ring is RAM, and nothing of it
//! is written to flash). A cursor beyond the newest line can only come
//! from an earlier boot, so a read starts again from the oldest line kept.
//! One below it cannot be told from this boot's own, and is read as one.

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering, fence};

/// The longest line the tail keeps; a longer one is counted as dropped.
/// A record line is at most 191 bytes (the SDK's record buffer), so none
/// is today.
pub const LINE_MAX: usize = 255;
/// A line's header in the ring: its number (`u32`, little-endian) and its
/// text's length (one byte).
const HEADER: usize = 5;
/// Reads started over because writers overwrote the line being read,
/// before the read gives up until its next pass.
const ATTEMPTS: usize = 4;

/// What one read copied out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Batch {
    /// The last line's number: the cursor the next read goes after.
    pub seq: u32,
    /// Lines after the cursor, up to [`Self::seq`], that the batch does
    /// not hold: overwritten before they were read, too long, or dropped
    /// by a writer that interrupted another.
    pub dropped: u32,
    /// The text's length: each line, followed by `\n`.
    pub len: usize,
}

/// A ring of `N` bytes (a power of two) holding the newest lines.
pub struct LogTail<const N: usize> {
    bytes: [AtomicU8; N],
    /// The stream position (wrapping) just past the newest line.
    head: AtomicU32,
    /// The stream position of the oldest line still whole.
    tail: AtomicU32,
    /// The newest line's number, 0 before the first. It is set before the
    /// line is published, so it is never behind a line a reader has seen.
    last: AtomicU32,
    /// A writer is copying a line in.
    writing: AtomicBool,
    /// Lines dropped since the last one written, numbered by the next.
    skipped: AtomicU32,
}

/// Why a read starts over: a writer overwrote the line it was reading.
struct Overrun;

impl<const N: usize> LogTail<N> {
    pub const fn new() -> Self {
        // The ring's size divides the wrapping `u32` stream positions, so a
        // byte's index is its position modulo `N` across the wrap; two of
        // the longest lines always fit.
        const { assert!(N.is_power_of_two() && N >= 2 * (HEADER + LINE_MAX) && N <= 1 << 30) };
        Self {
            bytes: [const { AtomicU8::new(0) }; N],
            head: AtomicU32::new(0),
            tail: AtomicU32::new(0),
            last: AtomicU32::new(0),
            writing: AtomicBool::new(false),
            skipped: AtomicU32::new(0),
        }
    }

    /// Copy one record line in, without its line ending. Safe from any task
    /// or interrupt; it never waits.
    pub fn push(&self, line: &[u8]) {
        let text = trim(line);
        if text.len() > LINE_MAX {
            self.skipped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if self.writing.swap(true, Ordering::Acquire) {
            // This writer interrupted another mid-copy, which cannot go on
            // until it returns: drop the line rather than wait.
            self.skipped.fetch_add(1, Ordering::Relaxed);
            return;
        }

        let skipped = self.skipped.swap(0, Ordering::Relaxed);
        let seq = self
            .last
            .load(Ordering::Relaxed)
            .wrapping_add(skipped)
            .wrapping_add(1);
        let size = HEADER + text.len();
        let head = self.head.load(Ordering::Relaxed);
        let mut tail = self.tail.load(Ordering::Relaxed);
        while distance(tail, head) + size > N {
            let len = self.byte(tail.wrapping_add(4));
            tail = tail.wrapping_add(position(HEADER + usize::from(len)));
        }
        // Readers learn which lines are gone before a byte of them changes:
        // a reader that sees an overwritten byte then sees this tail (the
        // fence). The store itself is a release too, so a reader that
        // acquires this tail also sees the head the last line published,
        // and never reads a tail past its head.
        self.tail.store(tail, Ordering::Release);
        fence(Ordering::Release);

        let mut at = head;
        for byte in seq.to_le_bytes() {
            self.set(at, byte);
            at = at.wrapping_add(1);
        }
        // At most LINE_MAX, which the check above made sure of.
        self.set(at, u8::try_from(text.len()).unwrap_or(u8::MAX));
        at = at.wrapping_add(1);
        for &byte in text {
            self.set(at, byte);
            at = at.wrapping_add(1);
        }

        // The number before the head: a reader that sees this line through
        // the head then sees a newest number at least its own, so its next
        // read never takes the cursor for an earlier boot's and starts over
        // from the oldest line, handing lines out twice.
        self.last.store(seq, Ordering::Release);
        self.head.store(at, Ordering::Release);
        self.writing.store(false, Ordering::Release);
    }

    /// The lines after `after` (`None`: from the oldest kept) that fit
    /// `out` whole, each followed by `\n`; `None` when there are none yet,
    /// or writers kept overwriting the read. Text is printable ASCII: any
    /// other byte reads as `?`.
    pub fn read(&self, after: Option<u32>, out: &mut [u8]) -> Option<Batch> {
        let last = self.last.load(Ordering::Acquire);
        // This boot's lines count from 1: a cursor past the newest is an
        // earlier boot's, and this boot's lines all come after it.
        let after = after.filter(|&cursor| !newer(cursor, last));
        if after == Some(last) {
            return None;
        }
        for _ in 0..ATTEMPTS {
            if let Ok(batch) = self.scan(after, out) {
                return batch;
            }
        }
        None
    }

    fn scan(&self, after: Option<u32>, out: &mut [u8]) -> Result<Option<Batch>, Overrun> {
        // Tail before head: the tail read is never past the head read.
        let mut pos = self.tail.load(Ordering::Acquire);
        let head = self.head.load(Ordering::Acquire);
        let mut used = 0;
        let mut first = None;
        let mut newest = 0;
        let mut count = 0u32;

        while pos != head {
            if distance(pos, head) > N {
                return Err(Overrun);
            }
            let seq = u32::from_le_bytes(core::array::from_fn(|i| {
                self.byte(pos.wrapping_add(position(i)))
            }));
            let len = usize::from(self.byte(pos.wrapping_add(4)));
            self.check(pos)?;
            let next = pos.wrapping_add(position(HEADER + len));

            let wanted = after.is_none_or(|cursor| newer(seq, cursor));
            // A line that fits no buffer at all would stall the cursor:
            // skip it, and the reader counts it dropped.
            let fits = used + len < out.len();
            if wanted && !fits && first.is_some() {
                break;
            }
            if wanted && fits {
                let text = pos.wrapping_add(position(HEADER));
                for (i, slot) in out[used..used + len].iter_mut().enumerate() {
                    *slot = printable(self.byte(text.wrapping_add(position(i))));
                }
                out[used + len] = b'\n';
                self.check(pos)?;
                used += len + 1;
                first.get_or_insert(seq);
                newest = seq;
                count += 1;
            }
            pos = next;
        }

        let Some(first) = first else {
            return Ok(None);
        };
        // Every number up to the newest read is either in the batch or
        // dropped: before the first line, or a hole between two.
        let base = after.unwrap_or(first.wrapping_sub(1));
        let dropped = newest.wrapping_sub(base).wrapping_sub(count);
        Ok(Some(Batch {
            seq: newest,
            dropped,
            len: used,
        }))
    }

    /// The line at `pos` was still whole when its bytes were read: no
    /// writer has moved the tail past it.
    fn check(&self, pos: u32) -> Result<(), Overrun> {
        fence(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Relaxed);
        if newer(tail, pos) {
            return Err(Overrun);
        }
        Ok(())
    }

    fn byte(&self, at: u32) -> u8 {
        self.bytes[index::<N>(at)].load(Ordering::Relaxed)
    }

    fn set(&self, at: u32, byte: u8) {
        self.bytes[index::<N>(at)].store(byte, Ordering::Relaxed);
    }
}

impl<const N: usize> Default for LogTail<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// The line without its line ending (or a leading one, as some prints
/// have).
fn trim(line: &[u8]) -> &[u8] {
    let ending = |byte: &u8| matches!(byte, b'\r' | b'\n');
    let start = line.iter().position(|b| !ending(b)).unwrap_or(line.len());
    let end = line
        .iter()
        .rposition(|b| !ending(b))
        .map_or(start, |i| i + 1);
    &line[start..end]
}

fn printable(byte: u8) -> u8 {
    if byte == b'\t' || (0x20..0x7f).contains(&byte) {
        byte
    } else {
        b'?'
    }
}

/// `a` is after `b` in wrapping order.
fn newer(a: u32, b: u32) -> bool {
    a.wrapping_sub(b).wrapping_sub(1) < crate::HALF_RANGE - 1
}

/// Bytes from stream position `from` forward to `to`.
fn distance(from: u32, to: u32) -> usize {
    usize::try_from(to.wrapping_sub(from)).unwrap_or(usize::MAX)
}

/// A length within the ring as a stream offset; `N` is at most 2^30.
fn position(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

fn index<const N: usize>(at: u32) -> usize {
    // N is a power of two no larger than 2^30, so the mask fits a u32.
    usize::try_from(at).unwrap_or(0) & (N - 1)
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::string::{String, ToString};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::vec::Vec;
    use std::{format, thread};

    use super::*;

    const RING: usize = 1024;

    fn read_all(tail: &LogTail<RING>, after: Option<u32>) -> Option<(Batch, Vec<String>)> {
        let mut out = [0; 4096];
        let batch = tail.read(after, &mut out)?;
        let text = core::str::from_utf8(&out[..batch.len]).unwrap();
        let lines = text.lines().map(ToString::to_string).collect();
        Some((batch, lines))
    }

    #[test]
    fn lines_come_back_in_order_without_their_endings() {
        let tail = LogTail::<RING>::new();
        assert_eq!(read_all(&tail, None), None);

        tail.push(b"PLAPI1 one\r\n");
        tail.push(b"\r\nWellfed local firmware\r\n");
        tail.push(b"PLAPI1 three");

        let (batch, lines) = read_all(&tail, None).unwrap();
        assert_eq!(
            lines,
            ["PLAPI1 one", "Wellfed local firmware", "PLAPI1 three"]
        );
        assert_eq!(batch.seq, 3);
        assert_eq!(batch.dropped, 0);
        assert_eq!(read_all(&tail, Some(3)), None);

        let (batch, lines) = read_all(&tail, Some(1)).unwrap();
        assert_eq!(lines, ["Wellfed local firmware", "PLAPI1 three"]);
        assert_eq!((batch.seq, batch.dropped), (3, 0));
    }

    #[test]
    fn the_oldest_lines_go_first_and_a_late_cursor_counts_them() {
        let tail = LogTail::<RING>::new();
        for i in 1..=200 {
            tail.push(format!("PLTST1 line={i:03} padding-padding\r\n").as_bytes());
        }

        let (batch, lines) = read_all(&tail, None).unwrap();
        assert_eq!(batch.seq, 200);
        assert_eq!(batch.dropped, 0, "from the oldest kept, nothing is missed");
        let oldest: u32 = lines[0][12..15].parse().unwrap();
        assert!(oldest > 150, "only the newest stay: {oldest}");
        assert_eq!(lines.len(), 201 - oldest as usize);
        assert!(lines.iter().map(String::len).sum::<usize>() < RING);

        let (batch, lines) = read_all(&tail, Some(10)).unwrap();
        assert_eq!(batch.dropped, oldest - 11);
        assert_eq!(lines.len(), 201 - oldest as usize);
    }

    #[test]
    fn a_read_takes_whole_lines_that_fit_and_the_next_goes_on_from_there() {
        let tail = LogTail::<RING>::new();
        for i in 1..=6 {
            tail.push(format!("PLTST1 line={i}").as_bytes());
        }
        let mut out = [0; 40];

        let batch = tail.read(None, &mut out).unwrap();
        assert_eq!(&out[..batch.len], b"PLTST1 line=1\nPLTST1 line=2\n");
        assert_eq!(batch.seq, 2);

        let batch = tail.read(Some(batch.seq), &mut out).unwrap();
        assert_eq!(&out[..batch.len], b"PLTST1 line=3\nPLTST1 line=4\n");
        assert_eq!((batch.seq, batch.dropped), (4, 0));
    }

    #[test]
    fn a_line_too_long_for_the_ring_or_the_buffer_counts_as_dropped() {
        let tail = LogTail::<RING>::new();
        tail.push(b"first");
        tail.push(&[b'x'; LINE_MAX + 1]);
        tail.push(&[b'y'; 50]);
        tail.push(b"last");

        let mut out = [0; 16];
        let batch = tail.read(Some(1), &mut out).unwrap();
        assert_eq!(&out[..batch.len], b"last\n");
        assert_eq!((batch.seq, batch.dropped), (4, 2));
    }

    #[test]
    fn other_bytes_read_as_question_marks() {
        let tail = LogTail::<RING>::new();
        tail.push(b"a\x00b\xffc\td\r\ne");
        let (_, lines) = read_all(&tail, None).unwrap();
        assert_eq!(lines, ["a?b?c\td??e"]);
    }

    #[test]
    fn a_cursor_from_an_earlier_boot_starts_from_the_oldest() {
        let tail = LogTail::<RING>::new();
        tail.push(b"one");
        tail.push(b"two");
        let (batch, lines) = read_all(&tail, Some(5_000)).unwrap();
        assert_eq!(lines, ["one", "two"]);
        assert_eq!((batch.seq, batch.dropped), (2, 0));
    }

    #[test]
    fn numbers_and_positions_carry_across_the_wrap() {
        let tail = LogTail::<RING>::new();
        tail.head.store(u32::MAX - 100, Ordering::Relaxed);
        tail.tail.store(u32::MAX - 100, Ordering::Relaxed);
        tail.last.store(u32::MAX - 1, Ordering::Relaxed);
        for i in 0..100 {
            tail.push(format!("PLTST1 line={i:03}").as_bytes());
        }

        let (batch, lines) = read_all(&tail, Some(u32::MAX - 1)).unwrap();
        assert_eq!(batch.seq, 98);
        assert!(batch.dropped > 0);
        assert_eq!(lines.last().unwrap(), "PLTST1 line=099");
        let (batch, _) = read_all(&tail, Some(90)).unwrap();
        assert_eq!((batch.seq, batch.dropped), (98, 0));
    }

    /// A writer that finds another mid-copy (an interrupt, on the feeder)
    /// drops its line and counts it.
    #[test]
    fn a_writer_that_interrupts_another_drops_its_line_and_counts_it() {
        let tail = LogTail::<RING>::new();
        tail.push(b"one");
        tail.writing.store(true, Ordering::Relaxed);
        tail.push(b"from an interrupt");
        tail.writing.store(false, Ordering::Relaxed);
        tail.push(b"three");

        let (batch, lines) = read_all(&tail, Some(1)).unwrap();
        assert_eq!(lines, ["three"]);
        assert_eq!((batch.seq, batch.dropped), (3, 1));
    }

    /// Writers on several threads, standing for tasks and interrupts, and a
    /// reader following them: every line the reader gets is whole, in
    /// order, and every number is either read or counted dropped.
    #[test]
    fn concurrent_writers_never_hand_a_reader_a_torn_or_missing_line() {
        let tail = Arc::new(LogTail::<RING>::new());
        let stop = Arc::new(AtomicBool::new(false));
        let writers: Vec<_> = (0..4)
            .map(|w| {
                let tail = Arc::clone(&tail);
                thread::spawn(move || {
                    for i in 0..20_000u32 {
                        // A line that says how long it is and repeats its
                        // writer's digit, so a torn copy shows.
                        let pad = usize::try_from(i % 40).unwrap();
                        let body: String =
                            core::iter::repeat_n(char::from(b'0' + w), pad).collect();
                        tail.push(format!("PLTST1 w={w} n={pad:02} {body}\r\n").as_bytes());
                    }
                })
            })
            .collect();

        let reader = {
            let tail = Arc::clone(&tail);
            let stop = Arc::clone(&stop);
            thread::spawn(move || {
                let mut out = [0; 1000];
                let mut cursor = Some(0);
                let (mut lines, mut dropped) = (0u64, 0u64);
                loop {
                    let done = stop.load(Ordering::Acquire);
                    while let Some(batch) = tail.read(cursor, &mut out) {
                        let text = core::str::from_utf8(&out[..batch.len]).unwrap();
                        for line in text.lines() {
                            let w = &line[9..10];
                            let pad: usize = line[13..15].parse().unwrap();
                            let body = &line[16..];
                            assert_eq!(body.len(), pad, "torn: {line}");
                            assert!(body.chars().all(|c| c.to_string() == w), "torn: {line}");
                            lines += 1;
                        }
                        dropped += u64::from(batch.dropped);
                        cursor = Some(batch.seq);
                    }
                    if done {
                        return (cursor, lines, dropped);
                    }
                    thread::yield_now();
                }
            })
        };

        for writer in writers {
            writer.join().unwrap();
        }
        stop.store(true, Ordering::Release);
        let (cursor, lines, dropped) = reader.join().unwrap();

        let last = tail.last.load(Ordering::Acquire);
        let pending = u64::from(tail.skipped.load(Ordering::Acquire));
        assert_eq!(cursor, Some(last), "the reader caught up");
        assert_eq!(lines + dropped, u64::from(last));
        assert_eq!(u64::from(last) + pending, 80_000, "every line has a number");
        assert!(lines > 0);
    }
}
