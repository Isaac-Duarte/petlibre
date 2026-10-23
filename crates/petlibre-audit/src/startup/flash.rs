//! The FW06/FW15 flash-coordination records: the KM0/KM4 HELLO/READY
//! binding, then whichever stage the build's policy runs after READY.

use regex::bytes::{Captures, Match};
use serde::Serialize;

use super::*;

pattern!(
    READ_START,
    r"PLRDS1 start marker=([0-9a-f]{8}) watchdog=(pass|fail) installed=([01])\r\n"
);
pattern!(
    READ_BATCH,
    r"PLRDS1 batch reads=([0-9]{1,10}) id=([0-9a-f]{6}) mismatches=([0-9]{1,10}) park_max=([0-9]{1,10}) release_max=([0-9]{1,10}) frozen=([0-9]{1,10}) alive=([0-9]{1,10})\r\n"
);
pattern!(
    READ_DONE_RE,
    r"PLRDS1 done reads=([0-9]{1,10}) leases=([0-9]{1,10}) phase=([0-9]{1,10}) fault=([0-9]{1,10})\r\n"
);
pattern!(READ_SKIP, r"PLRDS1 skip marker=([0-9a-f]{8})\r\n");

pub(super) const READ_BATCHES: [u64; 4] = [25, 50, 75, 100];
/// JEDEC ID of the research unit's GigaDevice flash (boot log `Flash ID:c8, 40, 17`).
const READ_ID: &str = "c84017";
/// Reads, leases, phase IDLE, no fault.
const READ_DONE: [u64; 4] = [100, 100, 1, 0];
/// 40 ms park/release exchange limit at 32 768 Hz.
const EXCHANGE_TICKS: u64 = 1310;

const DEC: &str = "([0-9]{1,10})";
static WRITE_START: LazyLock<Regex> = LazyLock::new(|| {
    re(r"PLWRS1 start marker=([0-9a-f]{8}) watchdog=(pass|fail) installed=([01])\r\n")
});
static WRITE_PREIMAGE: LazyLock<Regex> = LazyLock::new(|| {
    re(&format!(
        r"PLWRS1 preimage id=([0-9a-f]{{6}}) blank={DEC}\r\n"
    ))
});
static WRITE_CYCLE: LazyLock<Regex> = LazyLock::new(|| {
    re(&format!(
        r"PLWRS1 cycle n={DEC} erase_ticks={DEC} blank={DEC} program_max={DEC} errors={DEC}\r\n"
    ))
});
static WRITE_RESTORE: LazyLock<Regex> = LazyLock::new(|| {
    re(&format!(
        r"PLWRS1 restore erase_ticks={DEC} blank={DEC}\r\n"
    ))
});
static WRITE_DONE_RE: LazyLock<Regex> = LazyLock::new(|| {
    re(&format!(
        r"PLWRS1 done cycles={DEC} leases={DEC} phase={DEC} fault={DEC} park_max={DEC} release_max={DEC} frozen={DEC}\r\n"
    ))
});
static WRITE_SKIP: LazyLock<Regex> = LazyLock::new(|| re(r"PLWRS1 skip marker=([0-9a-f]{8})\r\n"));

pub(super) const WRITE_CYCLES: [u64; 3] = [1, 2, 3];
/// Every sampled word of the 4 KiB scratch sector reads erased.
const WRITE_BLANK: u64 = 1024;
/// Cycles, leases, phase IDLE, no fault; frozen equals leases.
const WRITE_DONE: [u64; 4] = [3, 53, 1, 0];
const TICK_HZ: f64 = 32768.0;
/// 500 ms lease active budget.
const ERASE_TICKS: u64 = 16384;
/// 50 ms.
const PROGRAM_TICKS: u64 = 1638;

pattern!(
    FLASH_HELLO,
    r"PLFLS1 hello authority=([0-9a-f]{16}) count=([0-9]{1,10})\r\n"
);
pattern!(FLASH_FOREIGN, r"PLFLS1 foreign\r\n");
pattern!(
    FLASH_READY_RE,
    r"PLFLS1 ready peer=([0-9a-f]{16}) reply=([0-9]{1,10}) phase=([0-9]{1,10}) fault=([0-9]{1,10}) inactive=([01])\r\n"
);
pattern!(
    FLASH_PEER_RE,
    r"PLFLS1 peer peer=([0-9a-f]{16}) reply=([0-9]{1,10}) phase=([0-9]{1,10}) fault=([0-9]{1,10}) blocked=([01])\r\n"
);
pattern!(
    FLASH_DONE_RE,
    r"PLFLS1 done phase=([0-9]{1,10}) fault=([0-9]{1,10}) blocked=([01])\r\n"
);
pattern!(
    FLASH_ERROR,
    r"PLFLS1 error (?:timeout ms=[0-9]{1,10}|clock)\r\n"
);

// Reply ACCEPTED/FAILED, phase IDLE/FAULT, fault PEER_LOST; the test KM0
// republishes its session ~10 s after READY.
const FLASH_READY: [u64; 4] = [0, 1, 0, 1];
const FLASH_PEER: [u64; 4] = [5, 8, 7, 1];
const FLASH_DONE: [u64; 3] = [8, 7, 1];

/// The stage a build runs after the HELLO/READY binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FlashPolicy {
    /// fw06-hello-v1: one injected peer change and the 30 s watch.
    Hello,
    /// fw06-park-v1: the park round trip.
    Park,
    /// fw06-read-v1: the mediated flash read.
    Read,
    /// fw06-write-v1: the scratch-sector write.
    Write,
    /// fw15-journal-v1: the flash journal.
    Journal,
    /// fw15-settings-v1 and -v2: the settings store.
    Settings,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct FlashReport {
    pub(crate) flash_handshake_verified: bool,
    pub(crate) flash_authority_session: Option<String>,
    pub(crate) flash_boot_count: Option<u64>,
    pub(crate) flash_peer_session: Option<String>,
    pub(crate) flash_injected_peer_session: Option<String>,
    pub(crate) flash_ready_seconds: Option<f64>,
    pub(crate) flash_peer_change_seconds: Option<f64>,
    pub(crate) flash_done_seconds: Option<f64>,
    /// fw06-read-v1 only.
    #[serde(flatten)]
    pub(crate) read: Option<FlashRead>,
    /// fw06-write-v1 only.
    #[serde(flatten)]
    pub(crate) write: Option<FlashWrite>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct FlashRead {
    pub(crate) flash_read_verified: bool,
    pub(crate) flash_read_id: Option<String>,
    pub(crate) flash_read_reads: Option<u64>,
    pub(crate) flash_read_leases: Option<u64>,
    pub(crate) flash_read_park_max: Option<u64>,
    pub(crate) flash_read_release_max: Option<u64>,
    pub(crate) flash_read_frozen: Option<u64>,
    pub(crate) flash_read_alive: Option<u64>,
    pub(crate) flash_read_seconds: Option<f64>,
    pub(crate) flash_read_tail_seconds: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct FlashWrite {
    pub(crate) flash_write_verified: bool,
    pub(crate) flash_write_id: Option<String>,
    pub(crate) flash_write_preimage_blank: Option<u64>,
    pub(crate) flash_write_cycles: Vec<WriteCycle>,
    pub(crate) flash_write_restore_erase_ticks: Option<u64>,
    pub(crate) flash_write_restore_blank: Option<u64>,
    pub(crate) flash_write_leases: Option<u64>,
    pub(crate) flash_write_park_max: Option<u64>,
    pub(crate) flash_write_release_max: Option<u64>,
    pub(crate) flash_write_frozen: Option<u64>,
    pub(crate) flash_write_erase_ms_max: Option<f64>,
    pub(crate) flash_write_seconds: Option<f64>,
    pub(crate) flash_write_tail_seconds: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct WriteCycle {
    pub(crate) n: u64,
    pub(crate) erase_ticks: u64,
    pub(crate) blank: u64,
    pub(crate) program_max: u64,
    pub(crate) errors: u64,
}

struct Flash<'h> {
    hello: Vec<Captures<'h>>,
    foreign: Vec<Captures<'h>>,
    ready: Vec<Captures<'h>>,
    peer: Vec<Captures<'h>>,
    done: Vec<Captures<'h>>,
    error: Vec<Captures<'h>>,
}

impl<'h> Flash<'h> {
    fn find(rx: &'h [u8]) -> Self {
        Self {
            hello: all(&FLASH_HELLO, rx),
            foreign: all(&FLASH_FOREIGN, rx),
            ready: all(&FLASH_READY_RE, rx),
            peer: all(&FLASH_PEER_RE, rx),
            done: all(&FLASH_DONE_RE, rx),
            error: all(&FLASH_ERROR, rx),
        }
    }

    fn total(&self) -> usize {
        self.hello.len()
            + self.foreign.len()
            + self.ready.len()
            + self.peer.len()
            + self.done.len()
            + self.error.len()
    }

    /// The handshake's fixed fields, from whichever records occur once.
    fn report(&self, policy: FlashPolicy) -> FlashReport {
        let single = |list: &[Captures<'h>]| match list {
            [one] => Some(text(one, 1)),
            _ => None,
        };
        let boot_count = match &self.hello[..] {
            [hello] => Some(dec(hello, 2)),
            _ => None,
        };

        FlashReport {
            flash_handshake_verified: false,
            flash_authority_session: single(&self.hello),
            flash_boot_count: boot_count,
            flash_peer_session: single(&self.ready),
            flash_injected_peer_session: single(&self.peer),
            flash_ready_seconds: None,
            flash_peer_change_seconds: None,
            flash_done_seconds: None,
            read: (policy == FlashPolicy::Read).then(FlashRead::default),
            write: (policy == FlashPolicy::Write).then(FlashWrite::default),
        }
    }
}

/// FW06 HELLO/READY binding, then one injected peer change and the 30 s watch.
///
/// The park policy checks only HELLO/READY: it refuses peer/done records and
/// requires the park round trip's start record after READY instead. The read
/// policy checks HELLO/READY the same way, then the mediated flash read
/// records; `end` is the capture end time. The write policy does the same for
/// the scratch-sector write. The journal policy checks HELLO/READY as park
/// does, then requires the journal's start record after READY. The settings
/// policies do the same with the settings store's start record and also
/// refuse journal records.
pub(crate) fn classify_flash(
    rx: &[u8],
    arrival: Arrival,
    policy: FlashPolicy,
    end: Option<Time>,
) -> Result<Checked<FlashReport>> {
    let found = Flash::find(rx);
    let mut report = found.report(policy);

    let outcome = flash_outcome(rx, arrival, policy, end, &found, &mut report)?;

    if outcome == Outcome::Pass {
        report.flash_handshake_verified = true;
        if let Some(read) = &mut report.read {
            read.flash_read_verified = true;
        }
        if let Some(write) = &mut report.write {
            write.flash_write_verified = true;
        }
    }
    Ok(Checked {
        outcome,
        fields: report,
    })
}

fn flash_outcome(
    rx: &[u8],
    arrival: Arrival,
    policy: FlashPolicy,
    end: Option<Time>,
    found: &Flash,
    report: &mut FlashReport,
) -> Result<Outcome> {
    if !contains(rx, b"PLFLS1") {
        return Ok(Outcome::FlashHandshakeMissing);
    }

    if count(rx, b"PLFLS1") != found.total()
        || found.foreign.len() > 1
        || [&found.hello, &found.ready, &found.peer, &found.done]
            .iter()
            .any(|l| l.len() > 1)
    {
        return Ok(Outcome::FlashHandshakeMalformed);
    }

    let hello_only = policy == FlashPolicy::Hello;
    if !hello_only && (!found.peer.is_empty() || !found.done.is_empty()) {
        return Ok(Outcome::FlashHandshakeMalformed);
    }

    // Under the read and write policies an error after READY belongs to that
    // stage; earlier ones fail the handshake.
    let staged_errors = matches!(policy, FlashPolicy::Read | FlashPolicy::Write);
    if let Some(error) = found.error.first()
        && (!staged_errors
            || found.ready.len() != 1
            || whole(error).start() < whole(&found.ready[0]).end())
    {
        return Ok(Outcome::FlashHandshakeFailed);
    }

    let singles = [&found.hello, &found.ready, &found.peer, &found.done]
        .iter()
        .filter(|l| l.len() == 1)
        .count();
    if singles != if hello_only { 4 } else { 2 } {
        return Ok(Outcome::FlashHandshakeMissing);
    }

    // Without peer/done records the singles are hello and ready.
    let handshake = Handshake {
        hello: &found.hello[0],
        ready: &found.ready[0],
        foreign: &found.foreign,
        led_done: find(rx, LED_DONE),
    };

    // The FW15 stages run alone: no earlier stage's records may appear.
    let earlier: &[&[u8]] = match policy {
        FlashPolicy::Journal => &[b"PLPRK1", b"PLRDS1", b"PLWRS1"],
        FlashPolicy::Settings => &[b"PLPRK1", b"PLRDS1", b"PLWRS1", b"PLJNL1"],
        _ => &[],
    };
    if earlier.iter().any(|m| contains(rx, m)) {
        return Ok(Outcome::FlashHandshakeMalformed);
    }

    match policy {
        FlashPolicy::Hello => classify_flash_hello(arrival, &handshake, found, report),
        FlashPolicy::Park => classify_flash_park(rx, arrival, &handshake, report, PARK_MARK),
        FlashPolicy::Read => classify_flash_read(rx, arrival, end, &handshake, found, report),
        FlashPolicy::Write => classify_flash_write(rx, arrival, end, &handshake, found, report),
        FlashPolicy::Journal => classify_flash_park(rx, arrival, &handshake, report, JOURNAL_MARK),
        FlashPolicy::Settings => {
            classify_flash_park(rx, arrival, &handshake, report, SETTINGS_MARK)
        }
    }
}

/// The single HELLO and READY records every policy binds first.
struct Handshake<'a, 'h> {
    hello: &'a Captures<'h>,
    ready: &'a Captures<'h>,
    foreign: &'a [Captures<'h>],
    led_done: Option<usize>,
}

impl Handshake<'_, '_> {
    /// HELLO authority carries the boot count; READY binds a distinct,
    /// nonzero peer session while stopped.
    fn fields_ok(&self) -> bool {
        let (authority, count, peer) = (hex(self.hello, 1), dec(self.hello, 2), hex(self.ready, 1));
        authority != 0
            && authority >> 32 == count
            && count >= 1
            && peer != 0
            && peer != authority
            && peer >> 32 >= 1
            && decs(self.ready, 2) == FLASH_READY
    }

    /// HELLO follows the LED probe, READY follows HELLO (and precedes
    /// `until`), no foreign record precedes HELLO, and READY came within 2.5 s.
    fn check(
        &self,
        arrival: Arrival,
        ready_seconds: &mut Option<f64>,
        until: Option<usize>,
    ) -> Result<Option<Outcome>> {
        let (h, r) = (whole(self.hello), whole(self.ready));
        let ordered = self.led_done.is_some_and(|done| done < h.start())
            && h.start() < h.end()
            && h.end() <= r.start()
            && until.is_none_or(|start| r.start() < r.end() && r.end() <= start);

        if !ordered || self.foreign.iter().any(|m| whole(m).start() < h.end()) {
            return Ok(Some(Outcome::FlashHandshakeOrder));
        }

        if !self.fields_ok() {
            return Ok(Some(Outcome::FlashHandshakeFailed));
        }

        let seconds = seconds(arrival(r.end() - 1)?, arrival(h.end() - 1)?);
        *ready_seconds = Some(seconds);
        if !(0.0..=2.5).contains(&seconds) {
            return Ok(Some(Outcome::FlashHandshakeTiming));
        }
        Ok(None)
    }
}

/// fw06-hello-v1: HELLO, READY, one injected peer change about 10 s later
/// and done after the 30 s watch, each in order.
fn classify_flash_hello(
    arrival: Arrival,
    handshake: &Handshake,
    found: &Flash,
    report: &mut FlashReport,
) -> Result<Outcome> {
    let (hello, ready) = (handshake.hello, handshake.ready);
    let (peer, done) = (&found.peer[0], &found.done[0]);
    let (h, r, p, d) = (whole(hello), whole(ready), whole(peer), whole(done));
    let ordered = handshake.led_done.is_some_and(|led| led < h.start())
        && h.start() < h.end()
        && h.end() <= r.start()
        && r.start() < r.end()
        && r.end() <= p.start()
        && p.start() < p.end()
        && p.end() <= d.start();

    if !ordered || found.foreign.iter().any(|m| whole(m).start() < h.end()) {
        return Ok(Outcome::FlashHandshakeOrder);
    }

    let (first, second) = (hex(ready, 1), hex(peer, 1));
    let ok = handshake.fields_ok()
        && second != first
        && second >> 32 == (first >> 32) + 1
        && decs(peer, 2) == FLASH_PEER
        && decs(done, 1) == FLASH_DONE;
    if !ok {
        return Ok(Outcome::FlashHandshakeFailed);
    }

    let at = |m: &Match| arrival(m.end() - 1);
    let (th, tr, tp, td) = (at(&h)?, at(&r)?, at(&p)?, at(&d)?);
    let (ready_s, peer_s, done_s) = (seconds(tr, th), seconds(tp, tr), seconds(td, tr));
    report.flash_ready_seconds = Some(ready_s);
    report.flash_peer_change_seconds = Some(peer_s);
    report.flash_done_seconds = Some(done_s);

    if !((0.0..=2.5).contains(&ready_s)
        && (9.0..=12.0).contains(&peer_s)
        && (29.0..=32.0).contains(&done_s))
    {
        return Ok(Outcome::FlashHandshakeTiming);
    }
    Ok(Outcome::Pass)
}

/// fw06-park-v1: HELLO/READY fields and timing, then the first park start
/// record after READY. fw15-journal-v1 and fw15-settings-v1 apply the same
/// checks with their start record as `mark`.
fn classify_flash_park(
    rx: &[u8],
    arrival: Arrival,
    handshake: &Handshake,
    report: &mut FlashReport,
    mark: &[u8],
) -> Result<Outcome> {
    let Some(start) = find(rx, mark) else {
        return Ok(Outcome::FlashHandshakeMissing);
    };

    Ok(handshake
        .check(arrival, &mut report.flash_ready_seconds, Some(start))?
        .unwrap_or(Outcome::Pass))
}

/// Records of one kind, sorted and deduplicated.
fn distinct<T: Ord + Clone>(values: &[T]) -> Vec<T> {
    let mut distinct = values.to_vec();
    distinct.sort();
    distinct.dedup();
    distinct
}

/// fw06-read-v1: HELLO/READY as in park-v1, then one start, four batches
/// and done of the mediated read.
fn classify_flash_read(
    rx: &[u8],
    arrival: Arrival,
    end: Option<Time>,
    handshake: &Handshake,
    flash: &Flash,
    report: &mut FlashReport,
) -> Result<Outcome> {
    let (start, batches, done, skip) = (
        all(&READ_START, rx),
        all(&READ_BATCH, rx),
        all(&READ_DONE_RE, rx),
        all(&READ_SKIP, rx),
    );

    let read = report.read.get_or_insert_default();
    if let Some(last) = batches.last() {
        let ids = distinct(&batches.iter().map(|m| text(m, 2)).collect::<Vec<_>>());
        read.flash_read_id = match &ids[..] {
            [id] => Some(id.clone()),
            _ => None,
        };
        read.flash_read_park_max = batches.iter().map(|m| dec(m, 4)).max();
        read.flash_read_release_max = batches.iter().map(|m| dec(m, 5)).max();
        read.flash_read_frozen = Some(dec(last, 6));
        read.flash_read_alive = Some(dec(last, 7));
    }
    if let [done] = &done[..] {
        read.flash_read_reads = Some(dec(done, 1));
        read.flash_read_leases = Some(dec(done, 2));
    }

    let reads: Vec<u64> = batches.iter().map(|m| dec(m, 1)).collect();
    if count(rx, b"PLRDS1") != start.len() + batches.len() + done.len() + skip.len()
        || contains(rx, b"PLPRK1")
        || [&start, &done, &skip].iter().any(|l| l.len() > 1)
        || distinct(&reads).len() != reads.len()
    {
        return Ok(Outcome::FlashReadMalformed);
    }

    if !skip.is_empty() || !flash.error.is_empty() {
        return Ok(Outcome::FlashReadFailed);
    }

    if let Some(outcome) = handshake.check(arrival, &mut report.flash_ready_seconds, None)? {
        return Ok(outcome);
    }

    let ([start], [done]) = (&start[..], &done[..]) else {
        return Ok(Outcome::FlashReadMissing);
    };

    let (s, d, r) = (whole(start), whole(done), whole(handshake.ready));
    // Batches lie between start and done in ascending order; no PLFLS1 record follows done.
    if !(r.end() <= s.start() && s.start() < s.end() && s.end() <= d.start())
        || !reads.is_sorted()
        || batches
            .iter()
            .any(|m| !(s.end() <= whole(m).start() && whole(m).start() < d.start()))
        || flash.foreign.iter().any(|m| whole(m).start() > d.start())
    {
        return Ok(Outcome::FlashReadOrder);
    }

    let ok = started_cleanly(start)
        && reads == READ_BATCHES
        && batches.iter().all(|m| {
            text(m, 2) == READ_ID
                && dec(m, 3) == 0
                && dec(m, 6) == dec(m, 1)
                && dec(m, 7) + 1 >= dec(m, 1)
                && 0 < dec(m, 4)
                && dec(m, 4) <= EXCHANGE_TICKS
                && 0 < dec(m, 5)
                && dec(m, 5) <= EXCHANGE_TICKS
        })
        && decs(done, 1) == READ_DONE;
    if !ok {
        return Ok(Outcome::FlashReadFailed);
    }

    let finished = arrival(d.end() - 1)?;
    let read_s = seconds(finished, arrival(s.end() - 1)?);
    let end =
        end.ok_or_else(|| Error::Refused("Flash read requires the capture end time".into()))?;
    let tail_s = seconds(end, finished);
    let read = report.read.get_or_insert_default();
    read.flash_read_seconds = Some(read_s);
    read.flash_read_tail_seconds = Some(tail_s);
    if !((0.0..=30.0).contains(&read_s) && tail_s >= 30.0) {
        return Ok(Outcome::FlashReadTiming);
    }
    Ok(Outcome::Pass)
}

/// A start record of marker 00000000, watchdog pass, installed 1.
fn started_cleanly(start: &Captures) -> bool {
    group(start, 1) == b"00000000" && group(start, 2) == b"pass" && group(start, 3) == b"1"
}

/// The scratch-sector write's records, each kind in capture order.
struct WriteRecords<'h> {
    start: Vec<Captures<'h>>,
    preimage: Vec<Captures<'h>>,
    cycles: Vec<Captures<'h>>,
    restore: Vec<Captures<'h>>,
    done: Vec<Captures<'h>>,
    skip: Vec<Captures<'h>>,
}

impl<'h> WriteRecords<'h> {
    fn find(rx: &'h [u8]) -> Self {
        Self {
            start: all(&WRITE_START, rx),
            preimage: all(&WRITE_PREIMAGE, rx),
            cycles: all(&WRITE_CYCLE, rx),
            restore: all(&WRITE_RESTORE, rx),
            done: all(&WRITE_DONE_RE, rx),
            skip: all(&WRITE_SKIP, rx),
        }
    }

    fn total(&self) -> usize {
        self.start.len()
            + self.preimage.len()
            + self.cycles.len()
            + self.restore.len()
            + self.done.len()
            + self.skip.len()
    }

    fn cycles(&self) -> Vec<WriteCycle> {
        self.cycles
            .iter()
            .map(|m| WriteCycle {
                n: dec(m, 1),
                erase_ticks: dec(m, 2),
                blank: dec(m, 3),
                program_max: dec(m, 4),
                errors: dec(m, 5),
            })
            .collect()
    }

    /// The fields reported whatever the outcome, from records that occur once.
    fn record(&self, write: &mut FlashWrite) {
        write.flash_write_cycles = self.cycles();

        if let [preimage] = &self.preimage[..] {
            write.flash_write_id = Some(text(preimage, 1));
            write.flash_write_preimage_blank = Some(dec(preimage, 2));
        }
        if let [restore] = &self.restore[..] {
            write.flash_write_restore_erase_ticks = Some(dec(restore, 1));
            write.flash_write_restore_blank = Some(dec(restore, 2));
        }
        if let [done] = &self.done[..] {
            let fields = decs(done, 1);
            write.flash_write_leases = Some(fields[1]);
            write.flash_write_park_max = Some(fields[4]);
            write.flash_write_release_max = Some(fields[5]);
            write.flash_write_frozen = Some(fields[6]);
        }

        let erase = write
            .flash_write_cycles
            .iter()
            .map(|c| c.erase_ticks)
            .chain(write.flash_write_restore_erase_ticks);
        write.flash_write_erase_ms_max = erase
            .max()
            .map(|max| crate::round(max as f64 * 1000.0 / TICK_HZ, 1));
    }
}

/// fw06-write-v1: HELLO/READY as in park-v1, then start, preimage, three
/// erase/program/verify cycles, restore and done.
fn classify_flash_write(
    rx: &[u8],
    arrival: Arrival,
    end: Option<Time>,
    handshake: &Handshake,
    flash: &Flash,
    report: &mut FlashReport,
) -> Result<Outcome> {
    let records = WriteRecords::find(rx);
    records.record(report.write.get_or_insert_default());

    let cycles = records.cycles();
    let numbers: Vec<u64> = cycles.iter().map(|c| c.n).collect();
    if count(rx, b"PLWRS1") != records.total()
        || contains(rx, b"PLPRK1")
        || contains(rx, b"PLRDS1")
        || [
            &records.start,
            &records.preimage,
            &records.restore,
            &records.done,
            &records.skip,
        ]
        .iter()
        .any(|l| l.len() > 1)
        || distinct(&numbers).len() != numbers.len()
    {
        return Ok(Outcome::FlashWriteMalformed);
    }

    if !records.skip.is_empty() || !flash.error.is_empty() {
        return Ok(Outcome::FlashWriteFailed);
    }

    if let Some(outcome) = handshake.check(arrival, &mut report.flash_ready_seconds, None)? {
        return Ok(outcome);
    }

    let ([start], [preimage], [restore], [done]) = (
        &records.start[..],
        &records.preimage[..],
        &records.restore[..],
        &records.done[..],
    ) else {
        return Ok(Outcome::FlashWriteMissing);
    };

    let (s, p, rs, d, r) = (
        whole(start),
        whole(preimage),
        whole(restore),
        whole(done),
        whole(handshake.ready),
    );
    // start, preimage, ascending cycles, restore, done; no PLFLS1 record follows done.
    if !(r.end() <= s.start()
        && s.start() < s.end()
        && s.end() <= p.start()
        && p.start() < p.end()
        && p.end() <= rs.start()
        && rs.start() < rs.end()
        && rs.end() <= d.start())
        || !numbers.is_sorted()
        || records
            .cycles
            .iter()
            .any(|m| !(p.end() <= whole(m).start() && whole(m).start() < rs.start()))
        || flash.foreign.iter().any(|m| whole(m).start() > d.start())
    {
        return Ok(Outcome::FlashWriteOrder);
    }

    let done_fields = decs(done, 1);
    let ok = started_cleanly(start)
        && text(preimage, 1) == READ_ID
        && dec(preimage, 2) == WRITE_BLANK
        && numbers == WRITE_CYCLES
        && cycles.iter().all(|c| {
            c.blank == WRITE_BLANK
                && c.errors == 0
                && 0 < c.erase_ticks
                && c.erase_ticks <= ERASE_TICKS
                && 0 < c.program_max
                && c.program_max <= PROGRAM_TICKS
        })
        && dec(restore, 2) == WRITE_BLANK
        && 0 < dec(restore, 1)
        && dec(restore, 1) <= ERASE_TICKS
        && done_fields[..4] == WRITE_DONE
        && done_fields[6] == WRITE_DONE[1]
        && 0 < done_fields[4]
        && done_fields[4] <= EXCHANGE_TICKS
        && 0 < done_fields[5]
        && done_fields[5] <= EXCHANGE_TICKS;
    if !ok {
        return Ok(Outcome::FlashWriteFailed);
    }

    let finished = arrival(d.end() - 1)?;
    let write_s = seconds(finished, arrival(s.end() - 1)?);
    let end =
        end.ok_or_else(|| Error::Refused("Flash write requires the capture end time".into()))?;
    let tail_s = seconds(end, finished);
    let write = report.write.get_or_insert_default();
    write.flash_write_seconds = Some(write_s);
    write.flash_write_tail_seconds = Some(tail_s);
    if !((0.0..=60.0).contains(&write_s) && tail_s >= 30.0) {
        return Ok(Outcome::FlashWriteTiming);
    }
    Ok(Outcome::Pass)
}
