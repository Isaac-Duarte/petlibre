//! Bank 0's install of an admitted offer: chunks pushed by a client, taken
//! idempotently, with every writer check intact. Nothing an install does
//! after a stop can reach the marker.

use sha2::{Digest, Sha256};
use wellfed_core::ota::install::{IDLE_MS, Install, Stop, Taken, WANTED};
use wellfed_core::ota::offer::{Admitted, ImageKind, Signed, Trust};
use wellfed_core::ota::{BANK1_OFFSET, MARKER, MAX_CHUNK, Manifest, SECTOR};

fn image(len: usize) -> Vec<u8> {
    let mut out: Vec<u8> = (0..len).map(|i| (i * 7 % 253) as u8).collect();
    out[..8].copy_from_slice(&MARKER);
    out
}

fn admitted(payload: &[u8]) -> Admitted {
    Admitted {
        signed: Signed {
            manifest: Manifest {
                version: 2,
                image_len: u32::try_from(payload.len()).unwrap(),
                target_bank: 1,
                image_sha256: Sha256::digest(payload).into(),
                build_id: [1; 32],
            },
            kind: ImageKind::Replacement,
            key_id: [0; 8],
        },
        trust: Trust::BuiltIn,
        digest: [2; 32],
        downgrade: None,
    }
}

/// Push every wanted chunk until nothing is wanted, programming each
/// sector into `flash`; the first stop.
fn push(install: &mut Install, payload: &[u8], flash: &mut Vec<u8>, now: u32) -> Result<(), Stop> {
    while install.wanted()[0].is_some() {
        let wanted: Vec<u32> = install.wanted().into_iter().flatten().collect();
        for offset in wanted.into_iter().rev() {
            let end = (offset as usize + MAX_CHUNK as usize).min(payload.len());
            let chunk = &payload[offset as usize..end];
            if let Taken::Program(program) = install.chunk(offset, chunk, now)? {
                let at = (program.offset - BANK1_OFFSET) as usize;
                flash.resize(flash.len().max(at + program.data.len()), 0xff);
                flash[at..at + program.data.len()].copy_from_slice(program.data);
                install.programmed(true)?;
            }
        }
    }
    Ok(())
}

#[test]
fn every_chunk_once_makes_the_install_ready_and_the_marker_comes_last() {
    let payload = image(2 * SECTOR + 700);
    let mut install = Install::new(admitted(&payload), 0).unwrap();
    assert_eq!(install.wanted().iter().flatten().count(), WANTED);

    let mut flash = Vec::new();
    push(&mut install, &payload, &mut flash, 10).unwrap();
    assert!(install.ready());
    assert_eq!(&flash[..8], &[0xff; 8], "the marker is withheld");
    assert_eq!(&flash[8..], &payload[8..]);
    assert_eq!(install.commit(), Ok((BANK1_OFFSET, MARKER)));
    install.marked(true).unwrap();
    assert!(install.activated());
    assert_eq!(install.wanted(), [None; WANTED]);
}

#[test]
fn a_chunk_already_held_is_answered_and_changes_nothing() {
    let payload = image(2 * SECTOR);
    let mut install = Install::new(admitted(&payload), 0).unwrap();
    let first = &payload[..MAX_CHUNK as usize];
    assert_eq!(install.chunk(0, first, 1), Ok(Taken::Staged));
    assert_eq!(install.chunk(0, first, 2), Ok(Taken::Held));
    // Even with other bytes: held chunks are not compared, the hash is.
    assert_eq!(install.chunk(0, &[0; 512], 3), Ok(Taken::Held));
    assert!(!install.wanted().contains(&Some(0)));

    let mut flash = Vec::new();
    push(&mut install, &payload, &mut flash, 4).unwrap();
    // A whole sector already written, sent again.
    assert_eq!(install.chunk(512, &payload[512..1024], 5), Ok(Taken::Held));
    assert!(install.ready());
}

#[test]
fn an_unwanted_chunk_or_a_wrong_length_stops_the_install_for_good() {
    let payload = image(2 * SECTOR);
    let ahead = &payload[SECTOR..SECTOR + 512];
    let mut install = Install::new(admitted(&payload), 0).unwrap();
    assert_eq!(install.chunk(SECTOR as u32, ahead, 1), Err(Stop::Order));
    assert_eq!(install.stopped(), Some(Stop::Order));
    assert_eq!(install.chunk(0, &payload[..512], 2), Err(Stop::Order));
    assert_eq!(install.wanted(), [None; WANTED]);
    assert_eq!(install.commit(), Err(Stop::Order));

    let mut install = Install::new(admitted(&payload), 0).unwrap();
    assert_eq!(install.chunk(512, &payload[512..600], 1), Err(Stop::Order));

    let mut no_marker = payload.clone();
    no_marker[0] = b'0';
    let mut install = Install::new(admitted(&no_marker), 0).unwrap();
    assert_eq!(install.chunk(0, &no_marker[..512], 1), Err(Stop::Marker));
}

#[test]
fn a_flipped_byte_fails_the_hash_and_a_bad_readback_fails_the_write() {
    let payload = image(SECTOR + 100);
    let mut flipped = payload.clone();
    flipped[SECTOR / 2] ^= 1;
    let mut install = Install::new(admitted(&payload), 0).unwrap();
    assert_eq!(
        push(&mut install, &flipped, &mut Vec::new(), 1),
        Err(Stop::Hash)
    );
    assert!(!install.ready());
    assert_eq!(install.commit(), Err(Stop::Hash));

    let mut install = Install::new(admitted(&payload), 0).unwrap();
    for offset in (0..SECTOR as u32).step_by(MAX_CHUNK as usize) {
        let chunk = &payload[offset as usize..offset as usize + 512];
        if let Taken::Program(_) = install.chunk(offset, chunk, 1).unwrap() {
            assert_eq!(install.programmed(false), Err(Stop::Write));
        }
    }
    assert_eq!(install.stopped(), Some(Stop::Write));
}

#[test]
fn a_quiet_install_times_out_and_a_cancelled_one_never_activates() {
    let payload = image(2 * SECTOR);
    let mut install = Install::new(admitted(&payload), 1_000).unwrap();
    install.chunk(0, &payload[..512], 2_000).unwrap();
    assert_eq!(install.tick(2_000 + IDLE_MS - 1), None);
    assert_eq!(install.tick(2_000 + IDLE_MS), Some(Stop::Timeout));
    assert_eq!(
        install.chunk(512, &payload[512..1024], 1),
        Err(Stop::Timeout)
    );

    let mut install = Install::new(admitted(&payload), 0).unwrap();
    push(&mut install, &payload, &mut Vec::new(), 1).unwrap();
    install.abort(Stop::Cancelled);
    assert_eq!(install.commit(), Err(Stop::Cancelled));
    assert_eq!(install.marked(true), Err(Stop::Cancelled));
    assert!(!install.activated());
}

#[test]
fn a_marker_that_does_not_read_back_is_a_stop() {
    let payload = image(SECTOR);
    let mut install = Install::new(admitted(&payload), 0).unwrap();
    push(&mut install, &payload, &mut Vec::new(), 1).unwrap();
    install.commit().unwrap();
    assert_eq!(install.marked(false), Err(Stop::Activate));
    assert!(!install.activated());
}
