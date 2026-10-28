//! The call trail: how the next boot validates the ring and the sticky
//! marks the shim's wrappers leave, ties them to the boot that wrote them,
//! the lines it prints, and that every boot starts a fresh one.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};

use wellfed_hal::flash_lock::provider::BOOT_COUNT_REGISTER;
use wellfed_hal::sdk::{TRAIL_MAGIC, TRAIL_VERSION, TRAIL_WORDS, fake};
use wellfed_hal::supervision::trail::{
    self, DEPTH_SHIFT, ENTRIES, Entry, ISR_SHIFT, MARKS, Mark, Refusal, SEQ_SHIFT, Step, Trail,
};

/// The fake hardware is process-wide.
static SERIAL: Mutex<()> = Mutex::new(());

fn start() -> MutexGuard<'static, ()> {
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    fake::reset_hardware();
    guard
}

const HEADER: usize = 8;
const MARKS_AT: usize = HEADER;
const ENTRIES_AT: usize = MARKS_AT + MARKS * 4;
/// The boot that wrote the trail; the next boot's count is one more.
const BOOT: u32 = 41;

/// The RAM as the shim leaves it, noted the way `wellfed_trail_note` does.
struct Ram {
    words: [u32; TRAIL_WORDS],
}

impl Ram {
    fn started(boot: u32) -> Self {
        let mut words = [0; TRAIL_WORDS];
        words[0] = TRAIL_MAGIC;
        words[1] = boot;
        words[2] = !(TRAIL_MAGIC ^ boot ^ TRAIL_VERSION);
        Self { words }
    }

    fn slot(n: u32) -> usize {
        ENTRIES_AT + (n as usize % ENTRIES) * 3
    }

    /// One note; `change` +1 for a critical section's entry, -1 for its exit.
    fn note(&mut self, step: Step, change: i32, isr: bool, lr: u32, arg: u32) {
        let depth = self.words[4] + u32::from(change > 0);
        self.words[4] = depth;
        let n = self.words[3];
        self.words[3] = n.wrapping_add(1);
        let meta = u32::from(step.code())
            | (depth.min(127) << DEPTH_SHIFT)
            | (u32::from(isr) << ISR_SHIFT)
            | (n << SEQ_SHIFT);
        let at = Self::slot(n);
        self.words[at + 1] = lr;
        self.words[at + 2] = arg;
        self.words[at] = meta;
        if let Some(mark) = step.mark() {
            let at = MARKS_AT + usize::from(mark.code()) * 4;
            self.words[at + 1..at + 4].copy_from_slice(&[meta, lr, arg]);
            self.words[at] = n;
        }
        if change < 0 && depth > 0 {
            self.words[4] = depth - 1;
        }
    }

    fn trail(&self) -> Trail<'_, [u32; TRAIL_WORDS]> {
        Trail::decode(&self.words, BOOT + 1).expect("the last boot's trail")
    }
}

/// Why `words` hold no trail of the boot before `current`, if they don't.
fn refusal(words: &[u32; TRAIL_WORDS], current: u32) -> Option<Refusal> {
    Trail::decode(words, current).err()
}

fn steps(trail: &Trail<'_, [u32; TRAIL_WORDS]>) -> Vec<(u32, Step)> {
    trail.entries().map(|e| (e.seq, e.step)).collect()
}

/// Masked traffic: a packet buffer.s critical section, as `alloc_skb` makes.
fn masked(ram: &mut Ram, times: u32) {
    for _ in 0..times {
        ram.note(Step::SaveAndCli, 1, false, 0x1000_d01b, 0);
        ram.note(Step::RestoreFlags, -1, false, 0x1000_d02f, 0);
    }
}

#[test]
fn a_power_on_or_a_build_without_the_trail_is_no_trail() {
    assert_eq!(refusal(&[0; TRAIL_WORDS], 1), Some(Refusal::NotStarted));
    assert_eq!(
        refusal(&[0xa5a5_a5a5; TRAIL_WORDS], 1),
        Some(Refusal::NotStarted)
    );

    // The magic alone, or with a check that does not cover the boot, is
    // not enough.
    let mut words = Ram::started(BOOT).words;
    words[2] = !TRAIL_MAGIC;
    assert_eq!(refusal(&words, BOOT + 1), Some(Refusal::NotStarted));
    words[2] = !(TRAIL_MAGIC ^ BOOT) ^ 1;
    assert_eq!(refusal(&words, BOOT + 1), Some(Refusal::NotStarted));
    words[0] ^= 1 << 31;
    words[2] = !(words[0] ^ BOOT);
    assert_eq!(refusal(&words, BOOT + 1), Some(Refusal::NotStarted));
}

#[test]
fn only_the_boot_just_before_is_read() {
    let mut ram = Ram::started(BOOT);
    ram.note(Step::Connect, 0, false, 0, 1);

    assert!(refusal(&ram.words, BOOT + 1).is_none());
    // Two boots on: a build without the trail, or one that never armed,
    // came between.
    assert_eq!(
        refusal(&ram.words, BOOT + 2),
        Some(Refusal::OtherBoot {
            boot: BOOT,
            current: BOOT + 2
        })
    );
    // The boot that wrote it never counted itself.
    assert_eq!(
        refusal(&ram.words, BOOT),
        Some(Refusal::OtherBoot {
            boot: BOOT,
            current: BOOT
        })
    );
    // Across the count's wrap.
    let ram = Ram::started(u32::MAX);
    assert!(refusal(&ram.words, 0).is_none());
}

#[test]
fn a_started_trail_without_notes_is_valid_and_empty() {
    let ram = Ram::started(BOOT);
    let trail = ram.trail();
    assert_eq!((trail.count, trail.depth, trail.skipped()), (0, 0, 0));
    assert_eq!(trail.entries().count(), 0);
    assert!(trail.marks().all(|(_, entry)| entry.is_none()));
    assert_eq!(
        trail.to_string(),
        "boot=41 count=0 n=0 skipped=0 depth=0 last=none"
    );
}

#[test]
fn notes_read_oldest_first_with_their_depth_and_context() {
    let mut ram = Ram::started(BOOT);
    ram.note(Step::Connect, 0, false, 0, 1);
    ram.note(Step::SaveAndCli, 1, false, 0x1001_936d, 0);
    ram.note(Step::SaveAndCli, 1, true, 0x1000_d01b, 0);
    ram.note(Step::RestoreFlags, -1, true, 0x1000_d02f, 0);

    let trail = ram.trail();
    assert_eq!((trail.count, trail.depth, trail.skipped()), (4, 1, 0));
    assert_eq!(
        steps(&trail),
        [
            (0, Step::Connect),
            (1, Step::SaveAndCli),
            (2, Step::SaveAndCli),
            (3, Step::RestoreFlags),
        ]
    );
    let entries: Vec<Entry> = trail.entries().collect();
    // An entry counts itself, and its exit still counts it.
    assert_eq!(
        entries.iter().map(|e| e.depth).collect::<Vec<_>>(),
        [0, 1, 2, 2]
    );
    assert_eq!(
        entries.iter().map(|e| e.isr).collect::<Vec<_>>(),
        [false, false, true, true]
    );
    assert_eq!(
        entries[2].to_string(),
        "seq=2 step=save_and_cli depth=2 isr=1 lr=0x1000d01b arg=0x00000000"
    );
    assert_eq!(
        trail.to_string(),
        "boot=41 count=4 n=4 skipped=0 depth=1 last=restore_flags"
    );
}

#[test]
fn the_ring_keeps_the_last_sixty_four_across_its_wrap() {
    let mut ram = Ram::started(BOOT);
    for i in 0..200u32 {
        let (step, change) = if i % 2 == 0 {
            (Step::SaveAndCli, 1)
        } else {
            (Step::RestoreFlags, -1)
        };
        ram.note(step, change, false, 0x1000_0001 + i, i);
    }
    ram.note(Step::DelTimer, 0, false, 0x1001_9711, 0x1003_0430);

    let trail = ram.trail();
    assert_eq!(trail.count, 201);
    let seqs: Vec<u32> = trail.entries().map(|e| e.seq).collect();
    assert_eq!(seqs, (137..201).collect::<Vec<_>>());
    let last = trail.last().expect("a last note");
    assert_eq!((last.step, last.arg), (Step::DelTimer, 0x1003_0430));
    assert_eq!(trail.entries().next().map(|e| e.arg), Some(137));
}

#[test]
fn the_marks_outlast_the_masked_traffic_that_flushes_the_ring() {
    let mut ram = Ram::started(BOOT);
    ram.note(Step::Connect, 0, false, 0, 1);
    ram.note(Step::SetTimer, 0, false, 0x0e03_96f5, 0x1003_0430);
    ram.note(Step::Indication, 0, false, 0x0e05_ae01, 1);
    ram.note(Step::IndicationReturned, 0, false, 0x0e05_ae01, 1);
    ram.note(Step::IndicateDisconnect, 0, false, 0x0e03_8b71, 0x1003_5000);
    ram.note(Step::DelTimer, 0, false, 0x0e05_ac91, 0x1003_0510);
    masked(&mut ram, 500);

    let trail = ram.trail();
    assert!(
        trail.entries().all(|e| e.step.mark().is_none()),
        "the ring holds only masked traffic"
    );
    let marks: BTreeMap<&str, Option<(u32, Step, u32)>> = trail
        .marks()
        .map(|(mark, e)| (mark.name(), e.map(|e| (e.seq, e.step, e.arg))))
        .collect();
    assert_eq!(marks["connect"], Some((0, Step::Connect, 1)));
    assert_eq!(marks["connect_ret"], None, "wifi_connect never returned");
    assert_eq!(marks["indication"], Some((3, Step::IndicationReturned, 1)));
    assert_eq!(
        marks["disconnect"],
        Some((4, Step::IndicateDisconnect, 0x1003_5000)),
        "the disconnect never returned"
    );
    assert_eq!(marks["timer"], Some((5, Step::DelTimer, 0x1003_0510)));
    assert_eq!(
        trail.mark(Mark::Timer).map(|e| e.to_string()).as_deref(),
        Some("seq=5 step=rtw_del_timer depth=0 isr=0 lr=0x0e05ac91 arg=0x10030510")
    );
}

#[test]
fn a_torn_or_misplaced_mark_is_none() {
    let mut ram = Ram::started(BOOT);
    ram.note(Step::Connect, 0, false, 0, 1);
    ram.note(Step::ConnectReturned, 0, false, 0, 0xffff_ffff);
    masked(&mut ram, 3);
    let connect = MARKS_AT + usize::from(Mark::Connect.code()) * 4;
    let returned = MARKS_AT + usize::from(Mark::ConnectReturned.code()) * 4;
    let timer = MARKS_AT + usize::from(Mark::Timer.code()) * 4;

    // A reset after a new note's first word, before its number.
    ram.words[connect + 1] = u32::from(Step::Connect.code()) | (ram.words[3] << SEQ_SHIFT);
    // A number past the count.
    ram.words[returned] = ram.words[3];
    // A step that is not the mark's kind.
    ram.words[timer..timer + 4].copy_from_slice(&[0, u32::from(Step::Connect.code()), 0, 0]);

    let trail = ram.trail();
    assert!(trail.marks().all(|(_, entry)| entry.is_none()));
}

#[test]
fn note_numbers_past_sixteen_bits_still_match() {
    let mut ram = Ram::started(BOOT);
    ram.words[3] = 0x0001_fff8;
    for _ in 0..12 {
        ram.note(Step::SetTimer, 0, false, 0x0e03_96f5, 0x1003_0430);
    }
    let trail = ram.trail();
    assert_eq!(
        trail.skipped(),
        52,
        "the slots before the first note are empty"
    );
    let seqs: Vec<u32> = trail.entries().map(|e| e.seq).collect();
    assert_eq!(seqs, (0x0001_fff8..0x0002_0004).collect::<Vec<_>>());
    assert_eq!(trail.mark(Mark::Timer).map(|e| e.seq), Some(0x0002_0003));
}

#[test]
fn a_note_cut_by_the_reset_is_dropped() {
    let mut ram = Ram::started(BOOT);
    masked(&mut ram, 50);
    // The reset landed after the next note claimed its slot and stored
    // `lr`, before its first word: the slot is the window's newest and
    // still holds the number it had a ring earlier.
    let n = ram.words[3];
    ram.words[3] = n + 1;
    ram.words[Ram::slot(n) + 1] = 0x1001_9711;

    let trail = ram.trail();
    assert_eq!(trail.skipped(), 1);
    assert_eq!(trail.entries().count(), ENTRIES - 1);
    assert_eq!(
        trail.entries().next().map(|e| e.seq),
        Some(n + 1 - ENTRIES as u32)
    );
    assert_eq!(trail.last().map(|e| e.seq), Some(n - 1));
    assert!(trail.entries().all(|e| e.lr != 0x1001_9711));
}

#[test]
fn an_unknown_step_is_skipped_not_shown() {
    let mut ram = Ram::started(BOOT);
    ram.note(Step::Indication, 0, false, 0x0e05_ae01, 1);
    ram.note(Step::SaveAndCli, 1, false, 0x1001_936d, 0);
    let at = Ram::slot(1);
    ram.words[at] = (ram.words[at] & !0xff) | 0xee;

    let trail = ram.trail();
    assert_eq!(trail.skipped(), 1);
    assert_eq!(steps(&trail), [(0, Step::Indication)]);
}

#[test]
fn every_step_round_trips_and_has_its_name() {
    for step in Step::ALL {
        assert_eq!(Step::from_byte(step.code()), Some(step));
    }
    assert_eq!(Step::from_byte(0), None);
    assert_eq!(Step::from_byte(12), None);
    assert_eq!(Step::ConnectReturned.name(), "wifi_connect_ret");
    assert_eq!(
        Step::IndicateDisconnect.name(),
        "rtw_os_indicate_disconnect"
    );
}

#[test]
fn the_boot_reads_the_last_trail_in_place_then_starts_its_own() {
    let _guard = start();
    fake::with(|h| h.backup[BOOT_COUNT_REGISTER as usize] = BOOT);
    let mut seen = None;
    trail::take(|trail| seen = trail.err());
    assert_eq!(seen, Some(Refusal::NotStarted), "a power-on has none");
    assert_eq!(fake::with(|h| h.trail_starts), 1);

    // That boot armed its flash hooks, which counted it, then hung.
    let mut ram = Ram::started(BOOT);
    ram.note(Step::Connect, 0, false, 0, 2);
    ram.note(Step::IndicateDisconnect, 0, false, 0x0e03_8b71, 0x1003_5000);
    fake::with(|h| {
        h.trail = ram.words;
        h.backup[BOOT_COUNT_REGISTER as usize] = BOOT + 1;
    });

    let mut lines = Vec::new();
    trail::take(|trail| {
        let trail = trail.expect("the last boot's trail");
        lines.push(trail.to_string());
        lines.extend(trail.entries().map(|e| e.to_string()));
    });
    let expected = ram.trail();
    assert_eq!(lines[0], expected.to_string());
    assert_eq!(lines.len(), 3);
    assert_eq!(fake::with(|h| h.trail_starts), 2);

    // The fresh trail is this boot's, read by the next.
    let fresh = fake::with(|h| h.trail);
    let fresh = Trail::decode(&fresh, BOOT + 2).expect("a fresh trail");
    assert_eq!((fresh.boot, fresh.count), (BOOT + 1, 0));
}

#[test]
fn a_trail_of_the_last_layout_is_refused() {
    let mut words = Ram::started(BOOT).words;
    words[2] = !(TRAIL_MAGIC ^ BOOT ^ (TRAIL_VERSION - 1));
    assert_eq!(refusal(&words, BOOT + 1), Some(Refusal::NotStarted));
}

#[test]
fn a_mark_cut_by_the_reset_after_its_lr_is_none() {
    let mut ram = Ram::started(BOOT);
    ram.note(Step::SetTimer, 0, false, 0x0e03_96f5, 0x1003_0430);
    masked(&mut ram, 2);
    // The next timer note: its ring slot is whole, but the reset landed
    // in its mark after the cleared meta and the new `lr`.
    ram.note(Step::DelTimer, 0, false, 0x0e05_ac91, 0x1003_0510);
    let at = MARKS_AT + usize::from(Mark::Timer.code()) * 4;
    ram.words[at] = 0;
    ram.words[at + 1] = 0;
    ram.words[at + 2] = 0x0e05_ac91;
    ram.words[at + 3] = 0x1003_0430;

    let trail = ram.trail();
    assert_eq!(trail.mark(Mark::Timer), None, "neither note, nor a mix");
    assert_eq!(trail.last().map(|e| e.step), Some(Step::DelTimer));
}

/// The shim's C constants are the ones this module decodes, both ways.
#[test]
fn the_shim_shares_the_layout_and_codes() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../firmware/vendor/ports/amebad/example_entry.c"
    );
    let source = std::fs::read_to_string(path).expect("the shim's source");
    let define = |name: &str| -> String {
        source
            .lines()
            .find_map(|line| line.strip_prefix(&format!("#define {name} ")))
            .unwrap_or_else(|| panic!("no {name}"))
            .split_whitespace()
            .next()
            .unwrap()
            .to_string()
    };
    let enumerated = |prefix: &str| -> BTreeMap<String, i64> {
        source
            .lines()
            .filter_map(|line| line.trim().strip_prefix(prefix))
            .filter_map(|rest| {
                let (name, value) = rest.split_once(" = ")?;
                Some((name.to_string(), value.trim_end_matches(',').parse().ok()?))
            })
            .collect()
    };

    assert_eq!(
        define("WELLFED_TRAIL_MAGIC"),
        format!("{TRAIL_MAGIC:#010x}u")
    );
    assert_eq!(define("WELLFED_TRAIL_VERSION"), format!("{TRAIL_VERSION}u"));
    assert_eq!(define("WELLFED_TRAIL_ENTRIES"), ENTRIES.to_string());
    assert_eq!(define("WELLFED_TRAIL_MARKS"), MARKS.to_string());
    assert_eq!(define("WELLFED_TRAIL_WORDS"), TRAIL_WORDS.to_string());
    assert_eq!(define("WELLFED_TRAIL_DEPTH_SHIFT"), DEPTH_SHIFT.to_string());
    assert_eq!(define("WELLFED_TRAIL_ISR_SHIFT"), ISR_SHIFT.to_string());
    assert_eq!(define("WELLFED_TRAIL_SEQ_SHIFT"), SEQ_SHIFT.to_string());

    // Every C step is a Rust step and every Rust step a C one.
    let c_steps = enumerated("WELLFED_STEP_");
    let rust_steps: BTreeMap<String, i64> = Step::ALL
        .into_iter()
        .map(|step| {
            let name = step.name();
            let base = name
                .trim_end_matches("_ret")
                .trim_start_matches("rtw_os_")
                .trim_start_matches("rtw_")
                .trim_start_matches("wifi_")
                .to_ascii_uppercase();
            let suffix = if name.ends_with("_ret") {
                "_RETURNED"
            } else {
                ""
            };
            (format!("{base}{suffix}"), i64::from(step.code()))
        })
        .collect();
    assert_eq!(c_steps, rust_steps);

    // The marks too, less the shim's "none".
    let mut c_marks = enumerated("WELLFED_MARK_");
    assert_eq!(c_marks.remove("NONE"), Some(-1));
    let rust_marks: BTreeMap<String, i64> = Mark::ALL
        .into_iter()
        .map(|mark| {
            let name = mark
                .name()
                .to_ascii_uppercase()
                .replace("_RET", "_RETURNED");
            (name, i64::from(mark.code()))
        })
        .collect();
    assert_eq!(c_marks, rust_marks);

    // Each step is noted once, with its mark and its depth change.
    let mut noted = BTreeMap::new();
    for call in source.split("wellfed_trail_note(WELLFED_STEP_").skip(1) {
        let mut args = call.split(',').map(str::trim);
        let (Some(step), Some(change), Some(mark)) = (args.next(), args.next(), args.next()) else {
            panic!("a note call's arguments: {call}");
        };
        let code = rust_steps[step];
        let step = Step::ALL
            .into_iter()
            .find(|s| i64::from(s.code()) == code)
            .unwrap();
        let mark = mark.strip_prefix("WELLFED_MARK_").expect("a mark argument");
        let expected_mark = step.mark().map_or("NONE".to_string(), |m| {
            m.name().to_ascii_uppercase().replace("_RET", "_RETURNED")
        });
        assert_eq!(mark, expected_mark, "{step:?}");
        let expected_change = match step {
            Step::SaveAndCli => "1",
            Step::RestoreFlags => "-1",
            _ => "0",
        };
        assert_eq!(change, expected_change, "{step:?}");
        assert!(
            noted.insert(step.code(), ()).is_none(),
            "{step:?} noted twice"
        );
    }
    assert_eq!(noted.len(), Step::ALL.len(), "every step is noted");
}
