//! Bank 1 read through the PLAF103's platform on the SDK fake, for the
//! stock backup: the SDK reads a sector at a time, so a page that crosses a
//! sector boundary is read in two, and nothing is read unless the flash
//! maps bank 0.

use wellfed_feeder::feeder::Platform;
use wellfed_fw_feeder::platform::Plaf103;
use wellfed_proto::api::BANK1_PAGE_MAX;
use wellfed_sdk::hal::sdk::fake;

#[test]
fn a_page_across_a_sector_boundary_reads_whole_and_only_from_bank_0() {
    fake::reset_hardware();
    fake::with(|h| {
        for (at, byte) in h.bank1.iter_mut().enumerate() {
            *byte = (at % 251) as u8;
        }
        h.running_bank = Some(0);
    });
    let mut platform = Plaf103::new();

    // The sixth page of 768 bytes starts at 3,840 and ends in sector 1.
    let offset = 5 * BANK1_PAGE_MAX as u32;
    let mut page = [0; BANK1_PAGE_MAX];
    assert!(platform.bank1_read(offset, &mut page));
    let expected: Vec<u8> = (offset as usize..offset as usize + BANK1_PAGE_MAX)
        .map(|at| (at % 251) as u8)
        .collect();
    assert_eq!(&page[..], &expected[..]);

    // Past the bank's end, and from bank 1, nothing is read.
    let mut tail = [0; 8];
    assert!(!platform.bank1_read(0x15_0000 - 4, &mut tail));
    fake::with(|h| h.running_bank = Some(1));
    assert!(!platform.bank1_read(0, &mut page));
}
