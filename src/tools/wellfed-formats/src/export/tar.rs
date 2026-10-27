//! The few POSIX tar (ustar) pieces an archive needs: a regular file's
//! header, the padding after its data, and the two empty blocks at the end.
//! Every header has the same mode, owner and time (zero), so an archive's
//! bytes depend only on its files. A size past ustar's eleven octal digits
//! (8 GiB) is written in base 256, as GNU and POSIX readers take it.

use super::ArchiveError;

/// A tar block: headers, data and the padding are counted in them.
pub const BLOCK: usize = 512;

/// What ends an archive: two empty blocks.
pub const END: [u8; 2 * BLOCK] = [0; 2 * BLOCK];

const NAME: usize = 100;
const SIZE_AT: usize = 124;
const CHECKSUM_AT: usize = 148;
const MAGIC_AT: usize = 257;

/// The largest size ustar's octal field holds.
const OCTAL_MAX: u64 = 0o77_777_777_777;

/// A regular file's header. `name` is at most 100 bytes.
pub fn header(name: &str, size: u64) -> Result<[u8; BLOCK], ArchiveError> {
    let name = name.as_bytes();
    if name.is_empty() || name.len() > NAME {
        return Err(ArchiveError::Tar("a name is 1-100 bytes"));
    }

    let mut block = [0; BLOCK];
    block[..name.len()].copy_from_slice(name);
    block[100..108].copy_from_slice(b"0000644\0");
    block[108..116].copy_from_slice(b"0000000\0");
    block[116..124].copy_from_slice(b"0000000\0");
    write_size(&mut block[SIZE_AT..SIZE_AT + 12], size);
    block[136..148].copy_from_slice(b"00000000000\0");
    block[156] = b'0';
    block[MAGIC_AT..MAGIC_AT + 8].copy_from_slice(b"ustar\x0000");

    let checksum = checksum(&block);
    let digits = format!("{checksum:06o}\0 ");
    block[CHECKSUM_AT..CHECKSUM_AT + 8].copy_from_slice(digits.as_bytes());
    Ok(block)
}

/// The zeros after `size` bytes of data, up to the next block.
pub fn padding(size: u64) -> &'static [u8] {
    const ZEROS: [u8; BLOCK] = [0; BLOCK];
    let rest = size % BLOCK as u64;
    let pad = if rest == 0 { 0 } else { BLOCK as u64 - rest };
    // `pad` is under a block, so it fits.
    &ZEROS[..usize::try_from(pad).unwrap_or(0)]
}

/// A parsed header: a regular file's name and size, or the end.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Header {
    File { name: String, size: u64 },
    End,
}

pub(super) fn parse(block: &[u8; BLOCK]) -> Result<Header, ArchiveError> {
    if block.iter().all(|&byte| byte == 0) {
        return Ok(Header::End);
    }
    if &block[MAGIC_AT..MAGIC_AT + 5] != b"ustar" {
        return Err(ArchiveError::Tar("no ustar magic"));
    }
    let stored = octal(&block[CHECKSUM_AT..CHECKSUM_AT + 8])
        .ok_or(ArchiveError::Tar("an unreadable checksum"))?;
    if stored != checksum(block) {
        return Err(ArchiveError::Tar("a header's checksum does not match"));
    }
    if !matches!(block[156], b'0' | 0) {
        return Err(ArchiveError::Tar("an entry that is not a regular file"));
    }

    let name_end = block[..NAME].iter().position(|&byte| byte == 0);
    let name = &block[..name_end.unwrap_or(NAME)];
    let name =
        String::from_utf8(name.to_vec()).map_err(|_| ArchiveError::Tar("a name is not UTF-8"))?;
    let size =
        read_size(&block[SIZE_AT..SIZE_AT + 12]).ok_or(ArchiveError::Tar("an unreadable size"))?;
    Ok(Header::File { name, size })
}

/// The sum of the header's bytes, its checksum field counted as spaces.
fn checksum(block: &[u8; BLOCK]) -> u64 {
    block
        .iter()
        .enumerate()
        .map(|(at, &byte)| {
            if (CHECKSUM_AT..CHECKSUM_AT + 8).contains(&at) {
                u64::from(b' ')
            } else {
                u64::from(byte)
            }
        })
        .sum()
}

fn write_size(field: &mut [u8], size: u64) {
    if size <= OCTAL_MAX {
        let digits = format!("{size:011o}\0");
        field.copy_from_slice(digits.as_bytes());
        return;
    }
    // Base 256: the high bit of the first byte set, the size big-endian in
    // the field's last eight bytes.
    field.fill(0);
    field[0] = 0x80;
    field[4..].copy_from_slice(&size.to_be_bytes());
}

fn read_size(field: &[u8]) -> Option<u64> {
    if field.first().is_some_and(|byte| byte & 0x80 != 0) {
        if field[1..4].iter().any(|&byte| byte != 0) {
            return None;
        }
        let bytes: [u8; 8] = field[4..].try_into().ok()?;
        return Some(u64::from_be_bytes(bytes));
    }
    octal(field)
}

/// An octal number, ended by a NUL or a space, leading spaces allowed.
fn octal(field: &[u8]) -> Option<u64> {
    let digits = field
        .iter()
        .skip_while(|&&byte| byte == b' ')
        .take_while(|&&byte| byte != 0 && byte != b' ');
    let mut value: u64 = 0;
    let mut any = false;
    for &digit in digits {
        if !(b'0'..=b'7').contains(&digit) {
            return None;
        }
        value = value.checked_mul(8)?.checked_add(u64::from(digit - b'0'))?;
        any = true;
    }
    any.then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_header_reads_back() {
        let block = header("users.jsonl", 1234).unwrap();

        assert_eq!(
            parse(&block).unwrap(),
            Header::File {
                name: "users.jsonl".into(),
                size: 1234
            }
        );
        assert_eq!(parse(&[0; BLOCK]).unwrap(), Header::End);
    }

    #[test]
    fn a_size_past_eight_gib_is_written_in_base_256() {
        let size = 20 * 1024 * 1024 * 1024;

        let block = header("events.jsonl", size).unwrap();

        assert_eq!(block[SIZE_AT], 0x80);
        assert!(matches!(parse(&block).unwrap(), Header::File { size: read, .. } if read == size));
    }

    #[test]
    fn a_damaged_header_is_refused() {
        let mut block = header("users.jsonl", 10).unwrap();
        block[0] = b'x';

        assert!(matches!(parse(&block), Err(ArchiveError::Tar(_))));
        assert!(header(&"x".repeat(101), 0).is_err());
    }

    #[test]
    fn data_is_padded_to_a_block() {
        assert_eq!(padding(0).len(), 0);
        assert_eq!(padding(1).len(), 511);
        assert_eq!(padding(512).len(), 0);
        assert_eq!(padding(513).len(), 511);
    }
}
