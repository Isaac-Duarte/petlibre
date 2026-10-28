//! The meal call's store: an upload lands chunk by chunk and the clip is
//! stored only once its stream reads back against the CRC, a restart or a
//! power cut anywhere leaves the whole clip or none, and a clip whose bytes
//! changed on flash is not played.

use wellfed_core::clip::{BYTES_MAX, CHUNK_MAX, DATA_AT, SECTOR_BYTES, SECTORS};
use wellfed_core::store::slot::crc32;
use wellfed_device_api::mailbox::CallClip;
use wellfed_feeder::store::call::{CallStatus, CallStore};
use wellfed_testkit::journal_flash::{FakeJournalFlash, FlashIo};

fn flash() -> FakeJournalFlash {
    FakeJournalFlash::with_units(SECTOR_BYTES as usize, SECTORS as usize)
}

fn open(f: &mut FakeJournalFlash) -> CallStore<FlashIo> {
    let mut s = CallStore::default();
    // SAFETY: the fake flash outlives the store in every test.
    unsafe { s.open(f.io()) };
    s
}

fn stream(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 7 + 3) as u8).collect()
}

/// Upload `data` whole, a chunk at a time; false at the first refusal.
fn upload(s: &mut CallStore<FlashIo>, data: &[u8]) -> bool {
    let crc = crc32(data);
    if !s.start(u32::try_from(data.len()).unwrap(), crc) {
        return false;
    }
    data.chunks(CHUNK_MAX as usize)
        .enumerate()
        .all(|(i, chunk)| s.chunk(u32::try_from(i).unwrap() * CHUNK_MAX, chunk))
}

#[test]
fn nothing_until_an_upload_completes_then_it_reloads() {
    let mut f = flash();
    let mut s = open(&mut f);
    assert_eq!((s.status, s.clip), (CallStatus::Ready, CallClip::None));

    // An odd length, so the last chunk is padded to whole words.
    let data = stream(3 * CHUNK_MAX as usize + 13);
    let crc = crc32(&data);
    assert!(s.start(u32::try_from(data.len()).unwrap(), crc));
    for (i, chunk) in data.chunks(CHUNK_MAX as usize).enumerate() {
        assert_eq!(s.stored(), None);
        assert!(s.chunk(u32::try_from(i).unwrap() * CHUNK_MAX, chunk));
    }
    let bytes = u32::try_from(data.len()).unwrap();
    assert_eq!(s.clip, CallClip::Stored { bytes, crc });
    assert_eq!(s.stored(), Some(bytes));

    let at = DATA_AT as usize;
    assert_eq!(&f.bytes[0][at..at + data.len()], &data[..]);
    let r = open(&mut f);
    assert_eq!(r.clip, CallClip::Stored { bytes, crc });
}

#[test]
fn chunks_land_in_order_and_a_repeat_changes_nothing() {
    let mut f = flash();
    let mut s = open(&mut f);
    let data = stream(2 * CHUNK_MAX as usize);
    assert!(s.start(1024, crc32(&data)));

    // Ahead of where the upload stands, or short of a whole chunk
    // mid-stream: refused, and the upload goes on.
    assert!(!s.chunk(CHUNK_MAX, &data[512..]));
    assert!(!s.chunk(0, &data[..100]));
    assert!(s.chunk(0, &data[..512]));
    assert!(s.chunk(0, &data[..512]), "a retry of a chunk already in");
    assert!(s.chunk(512, &data[512..]));
    assert!(matches!(s.clip, CallClip::Stored { .. }));
}

#[test]
fn a_stream_that_misses_its_crc_is_not_stored() {
    let mut f = flash();
    let mut s = open(&mut f);
    let data = stream(700);
    assert!(s.start(700, crc32(&data) ^ 1));
    assert!(s.chunk(0, &data[..512]));
    assert!(s.chunk(512, &data[512..]));
    assert_eq!(s.clip, CallClip::None);
    assert_eq!(open(&mut f).clip, CallClip::None);
}

#[test]
fn the_longest_clip_fills_every_sector() {
    let mut f = flash();
    let mut s = open(&mut f);
    let data = stream(BYTES_MAX as usize);
    assert!(upload(&mut s, &data));
    assert!(matches!(s.clip, CallClip::Stored { bytes, .. } if bytes == BYTES_MAX));
    assert_eq!(f.bytes[7][SECTOR_BYTES as usize - 1], data[data.len() - 1]);
    assert!(!s.start(BYTES_MAX + 1, 0));
    assert!(!s.start(0, 0));
}

#[test]
fn a_new_upload_or_a_delete_drops_the_old_clip_at_once() {
    let mut f = flash();
    let mut s = open(&mut f);
    assert!(upload(&mut s, &stream(1_000)));
    assert!(s.start(600, 0));
    assert!(matches!(s.clip, CallClip::Receiving { received: 0, .. }));
    assert_eq!(open(&mut f).clip, CallClip::None, "a restart forgets it");

    let mut s = open(&mut f);
    assert!(upload(&mut s, &stream(1_000)));
    assert!(s.delete());
    assert_eq!(s.clip, CallClip::None);
    assert_eq!(open(&mut f).clip, CallClip::None);
}

#[test]
fn a_clip_whose_bytes_changed_is_not_loaded() {
    let mut f = flash();
    let mut s = open(&mut f);
    assert!(upload(&mut s, &stream(2_000)));
    // A byte of the stream, in the first sector after the header.
    f.bytes[0][1_000] ^= 0x01;
    assert_eq!(open(&mut f).clip, CallClip::None);
}

#[test]
fn power_cut_at_every_mutation_leaves_the_clip_whole_or_none() {
    let old = stream(1_000);
    let new = stream(1_500).iter().map(|b| b ^ 0x55).collect::<Vec<_>>();
    let mut probe = flash();
    let mut s = open(&mut probe);
    assert!(upload(&mut s, &old));
    let start = probe.mutations;
    assert!(upload(&mut s, &new));
    let span = i64::from(probe.mutations - start);

    // Every mutation of the header's write, and a spread of the rest.
    for cut in (0..=span).filter(|cut| cut % 97 == 0 || span - cut < 200) {
        let mut f = flash();
        let mut s = open(&mut f);
        assert!(upload(&mut s, &old));
        f.budget = cut;
        let _ = upload(&mut s, &new);
        f.budget = -1;

        let r = open(&mut f);
        match r.clip {
            CallClip::None => {}
            CallClip::Stored { bytes: 1_000, crc } => assert_eq!(crc, crc32(&old), "cut {cut}"),
            CallClip::Stored { bytes: 1_500, crc } => assert_eq!(crc, crc32(&new), "cut {cut}"),
            other => panic!("cut {cut}: {other:?}"),
        }
        assert_eq!(r.status, CallStatus::Ready, "cut {cut}");
    }
}

#[test]
fn a_failed_write_keeps_no_clip_and_refuses_more() {
    let mut f = flash();
    let mut s = open(&mut f);
    let data = stream(1_000);
    assert!(s.start(1_000, crc32(&data)));
    f.fail_call = f.calls + 1;
    assert!(!s.chunk(0, &data[..512]));
    assert_eq!((s.status, s.clip), (CallStatus::Io, CallClip::None));
    assert!(!s.start(1_000, 0));
    assert!(!s.view(true).writable);
}
