//! The stock backup that promotion waits for ([rollback]).
//!
//! Promotion writes the replacement into bank 1, which until then holds
//! the feeder's only copy of stock. So an install never promotes until it
//! holds a [`VerifiedBackup`], and the only way to make one is
//! [`VerifiedBackup::verify`]: stock 1.1.15's bare application payload,
//! whose SHA-256 is the pinned payload's, or the vendor's exact OTA
//! container, whose SHA-256 is the pinned container's. A damaged or partial
//! copy cannot pass, and the check does not care where the bytes came from.
//!
//! The payload is pinned, not only the whole container, because the bank
//! holds the payload: a factory-flashed unit may never have been written
//! from that exact container, and a backup read from bank 1 is a payload.
//! A container is never taken apart, though: any header other than the
//! vendor's over the right payload is refused, since its fields are not
//! checked here and a restore would write it as given.
//!
//! The copy comes from the feeder itself: the build on bank 0 streams bank
//! 1 over the device API ([`Bank1Reader`], a page per `ReadBank1`), and
//! [`StockBackup::from_bank`] restores the marker byte stock's activation
//! zeroed, checks the payload against the pin and rebuilds the vendor's
//! container around it. The transport carries no trust: the pin is what
//! shows the copy is intact. A copy the owner already holds passes the
//! same check ([`StockBackup::open`]).
//!
//! The rebuilt container is the owner's own stock OTA, byte for byte the
//! vendor's: it is what they keep and export unchanged, and what "Restore
//! stock" signs and installs.
//!
//! [rollback]: ../../../../../docs/firmware/consumer-installation.md#rollback

use wellfed_core::ota::MARKER;
use wellfed_model::{Answer, Bank1Page, Op, Refusal};
use sha2::{Digest, Sha256};

/// The stock release an install starts from and a backup restores.
pub const STOCK_VERSION: &str = "1.1.15";

/// SHA-256 of the whole stock 1.1.15 OTA image (container header
/// included), as pinned in the device layout and the image profile.
pub const STOCK_OTA_SHA256: [u8; 32] = [
    0x05, 0x14, 0x74, 0x76, 0xfc, 0x70, 0xc9, 0x1c, 0xc7, 0x7a, 0xd7, 0xda, 0x6d, 0x67, 0x72, 0x37,
    0x6b, 0x20, 0x81, 0xd4, 0x60, 0xac, 0x7b, 0x19, 0x12, 0xe2, 0xd7, 0x38, 0x68, 0xa0, 0x94, 0x98,
];

/// SHA-256 of that image's payload (everything after its 32-byte
/// header): the bank's contents, which the stock review records as its
/// `payload_sha256`.
pub const STOCK_PAYLOAD_SHA256: [u8; 32] = [
    0x80, 0x2e, 0xf6, 0x20, 0x4c, 0x2d, 0xce, 0x8a, 0xf8, 0xb7, 0x4d, 0x78, 0xc5, 0xb5, 0x8b, 0x1d,
    0xc5, 0xf9, 0x12, 0xc2, 0x04, 0xbd, 0x0a, 0xc6, 0x8e, 0x0a, 0x03, 0xa7, 0x21, 0x20, 0x70, 0x5a,
];

/// The pinned payload's length: the stock container's payload size field,
/// and bank 1's whole extent.
pub const STOCK_PAYLOAD_LEN: u32 = 0x15_0000;

/// The OTA container's header, which precedes the bank's contents.
pub const CONTAINER_HEADER: usize = 32;

/// Why a backup does not count.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BackupError {
    /// The bytes are neither the pinned payload nor a container of it:
    /// damaged, partial, or another release.
    #[error("the backup is not stock {STOCK_VERSION}'s pinned image")]
    NotPinned { sha256: [u8; 32] },
    /// Bank 1 does not start with the loader's marker, with or without its
    /// first byte zeroed: it holds no application stock wrote.
    #[error("bank 1 does not start with an application marker")]
    NoMarker,
}

/// Why reading bank 1 stopped.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ReadError {
    #[error("the feeder refused the read: {0:?}")]
    Refused(Refusal),
    #[error("the feeder answered something other than a page of bank 1")]
    Unexpected(Box<Answer>),
    /// The feeder serves a bank of another size than stock's payload.
    #[error("the feeder serves {total} bytes of bank 1, not stock's {STOCK_PAYLOAD_LEN}")]
    Size { total: u32 },
    /// A page that is not the next one asked for.
    #[error("the feeder's page at {offset} does not follow the bytes read so far")]
    Order { offset: u32 },
    #[error("the feeder's page ended before bank 1 did")]
    Short,
}

/// Bank 1 being read a page at a time. Reads change nothing on the feeder,
/// so any page may be asked again, and a read that stops can start over.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bank1Reader {
    bank: Vec<u8>,
}

impl Bank1Reader {
    pub const fn new() -> Self {
        Self { bank: Vec::new() }
    }

    /// The op that asks for the next page.
    pub fn next_op(&self) -> Op {
        Op::ReadBank1 {
            offset: self.offset(),
        }
    }

    /// Bytes read so far.
    pub fn read(&self) -> usize {
        self.bank.len()
    }

    fn offset(&self) -> u32 {
        u32::try_from(self.bank.len()).unwrap_or(u32::MAX)
    }

    /// Take the answer to [`Self::next_op`]: the whole bank once its last
    /// page is in, `None` while more remain.
    pub fn take(&mut self, answer: Answer) -> Result<Option<Vec<u8>>, ReadError> {
        let page = match answer {
            Answer::Bank1(page) => page,
            Answer::Refused(refusal) => return Err(ReadError::Refused(refusal)),
            other => return Err(ReadError::Unexpected(Box::new(other))),
        };
        let Bank1Page {
            offset,
            total,
            bytes,
        } = page;

        if total != STOCK_PAYLOAD_LEN {
            return Err(ReadError::Size { total });
        }
        if offset != self.offset() {
            return Err(ReadError::Order { offset });
        }
        if bytes.is_empty() {
            return Err(ReadError::Short);
        }

        self.bank.extend_from_slice(&bytes);
        match self.bank.len().cmp(&(STOCK_PAYLOAD_LEN as usize)) {
            core::cmp::Ordering::Less => Ok(None),
            core::cmp::Ordering::Equal => Ok(Some(core::mem::take(&mut self.bank))),
            core::cmp::Ordering::Greater => Err(ReadError::Order { offset }),
        }
    }
}

/// The owner's copy of stock, checked against the pins: the vendor's exact
/// container, or the bare payload a caller handed in.
#[derive(Clone, PartialEq, Eq)]
pub struct StockBackup {
    image: Vec<u8>,
    verified: VerifiedBackup,
}

impl core::fmt::Debug for StockBackup {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("StockBackup")
            .field("len", &self.image.len())
            .finish_non_exhaustive()
    }
}

impl StockBackup {
    /// Bank 1 as the bank-0 role streamed it: the marker's first byte is
    /// restored, the payload checked against its pin, and the vendor's
    /// container rebuilt around it and checked against its own pin.
    pub fn from_bank(bank: Vec<u8>) -> Result<Self, BackupError> {
        from_bank_against(bank, &STOCK_OTA_SHA256, &STOCK_PAYLOAD_SHA256)
    }

    /// A copy the owner holds (a file kept or exported earlier): the
    /// vendor's exact container as it is, or a bare payload wrapped in it.
    pub fn open(image: Vec<u8>) -> Result<Self, BackupError> {
        let sha256: [u8; 32] = Sha256::digest(&image).into();
        if sha256 == STOCK_PAYLOAD_SHA256 {
            return from_bank_against(image, &STOCK_OTA_SHA256, &STOCK_PAYLOAD_SHA256);
        }
        let verified = verify_against(&image, &STOCK_OTA_SHA256, &STOCK_PAYLOAD_SHA256)?;
        Ok(Self { image, verified })
    }

    /// The bytes to keep and export unchanged: the vendor's container.
    pub fn bytes(&self) -> &[u8] {
        &self.image
    }

    /// The bank's contents, which a restore installs.
    pub fn payload(&self) -> &[u8] {
        self.image.get(CONTAINER_HEADER..).unwrap_or_default()
    }

    pub fn verified(&self) -> VerifiedBackup {
        self.verified
    }
}

/// [`StockBackup::from_bank`] against the given pins.
fn from_bank_against(
    mut bank: Vec<u8>,
    container_pin: &[u8; 32],
    payload_pin: &[u8; 32],
) -> Result<StockBackup, BackupError> {
    // Stock's activation of bank 0 programs bank 1's first marker byte to
    // zero and leaves every other byte as it was.
    let Some(marker) = bank.get_mut(..MARKER.len()) else {
        return Err(BackupError::NoMarker);
    };
    if marker[1..] != MARKER[1..] || !(marker[0] == 0 || marker[0] == MARKER[0]) {
        return Err(BackupError::NoMarker);
    }
    marker[0] = MARKER[0];

    let payload_sha256: [u8; 32] = Sha256::digest(&bank).into();
    if payload_sha256 != *payload_pin {
        return Err(BackupError::NotPinned {
            sha256: payload_sha256,
        });
    }

    // Only the vendor's exact container counts, so the header written here
    // must hash to its pin too.
    let image = container(&bank);
    let sha256: [u8; 32] = Sha256::digest(&image).into();
    if sha256 != *container_pin {
        return Err(BackupError::NotPinned { sha256 });
    }
    Ok(StockBackup {
        image,
        verified: VerifiedBackup { _checked: () },
    })
}

/// The vendor's single-entry OTA container around `payload`: version
/// `0xffffffff`, one `OTA1` descriptor of 24 bytes, the payload's byte-sum
/// and length, its offset 32, and an unset destination ([container
/// format]).
///
/// [container format]: ../../../../../docs/devices/plaf103/stock/ota-requirements.md#the-image-container
fn container(payload: &[u8]) -> Vec<u8> {
    let sum = payload
        .iter()
        .fold(0u32, |sum, &byte| sum.wrapping_add(u32::from(byte)));
    let len = u32::try_from(payload.len()).unwrap_or(u32::MAX);
    let words = [
        0xffff_ffff,
        1,
        u32::from_le_bytes(*b"OTA1"),
        24,
        sum,
        len,
        32,
        0xffff_ffff,
    ];

    let mut image = Vec::with_capacity(CONTAINER_HEADER + payload.len());
    for word in words {
        image.extend_from_slice(&word.to_le_bytes());
    }
    image.extend_from_slice(payload);
    image
}

/// Proof that a copy of stock matching the pinned payload exists. It holds
/// no bytes: the caller keeps the file (and offers it to the owner to
/// export); the install only needs to know it was checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VerifiedBackup {
    _checked: (),
}

impl VerifiedBackup {
    /// Check a bare stock payload, or the exact pinned OTA container,
    /// against the pins.
    pub fn verify(image: &[u8]) -> Result<Self, BackupError> {
        verify_against(image, &STOCK_OTA_SHA256, &STOCK_PAYLOAD_SHA256)
    }

    /// The stock release it restores.
    pub fn version(&self) -> &'static str {
        STOCK_VERSION
    }

    /// A backup for this crate's tests, which cannot hold the vendor
    /// image. No other build can forge one.
    #[cfg(test)]
    pub(crate) fn for_tests() -> Self {
        Self { _checked: () }
    }
}

/// `image` is exactly the pinned container, or exactly the pinned payload.
/// A container is never taken apart: a header other than the vendor's
/// (lengths, byte-sum, magic) over the right payload is refused, since
/// nothing here checks its fields and a restore would write it as given.
fn verify_against(
    image: &[u8],
    container: &[u8; 32],
    payload: &[u8; 32],
) -> Result<VerifiedBackup, BackupError> {
    let sha256: [u8; 32] = Sha256::digest(image).into();
    if sha256 == *container || sha256 == *payload {
        Ok(VerifiedBackup { _checked: () })
    } else {
        Err(BackupError::NotPinned { sha256 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic bank of stock's size, starting with the marker.
    fn synthetic_bank() -> Vec<u8> {
        let mut bank: Vec<u8> = (0..STOCK_PAYLOAD_LEN)
            .map(|i| i.to_le_bytes()[0] ^ i.to_le_bytes()[1])
            .collect();
        bank[..MARKER.len()].copy_from_slice(&MARKER);
        bank
    }

    fn pins(bank: &[u8]) -> ([u8; 32], [u8; 32]) {
        (
            Sha256::digest(container(bank)).into(),
            Sha256::digest(bank).into(),
        )
    }

    /// Synthetic pins over a synthetic bank: the real payload is the
    /// vendor's and stays out of the repository.
    #[test]
    fn a_bank_with_its_marker_byte_zeroed_rebuilds_the_pinned_container() {
        let original = synthetic_bank();
        let (container_pin, payload_pin) = pins(&original);
        let mut bank = original.clone();
        bank[0] = 0;

        let backup = from_bank_against(bank, &container_pin, &payload_pin).unwrap();
        let image = backup.bytes();
        assert_eq!(<[u8; 32]>::from(Sha256::digest(image)), container_pin);
        assert_eq!(&image[CONTAINER_HEADER..], &original[..]);
        assert_eq!(backup.payload(), &original[..]);

        // The header names the payload as the vendor's does.
        let word = |at: usize| u32::from_le_bytes(image[at..at + 4].try_into().unwrap());
        assert_eq!(&image[8..12], b"OTA1");
        assert_eq!(word(20), STOCK_PAYLOAD_LEN);
        let sum = original
            .iter()
            .fold(0u32, |sum, &b| sum.wrapping_add(u32::from(b)));
        assert_eq!(word(16), sum);

        // A marker left intact is the same bank.
        assert!(from_bank_against(original, &container_pin, &payload_pin).is_ok());
    }

    #[test]
    fn a_damaged_bank_or_another_marker_is_refused() {
        let original = synthetic_bank();
        let (container_pin, payload_pin) = pins(&original);

        let mut flipped = original.clone();
        flipped[0] = 0;
        flipped[0x1_0000] ^= 0x01;
        assert!(matches!(
            from_bank_against(flipped, &container_pin, &payload_pin),
            Err(BackupError::NotPinned { .. })
        ));

        // Another byte of the marker cleared, or a first byte that is
        // neither the marker's nor zero: not what stock's activation leaves.
        for (at, value) in [(1, 0), (0, 0x01)] {
            let mut bank = original.clone();
            bank[at] = value;
            assert_eq!(
                from_bank_against(bank, &container_pin, &payload_pin),
                Err(BackupError::NoMarker)
            );
        }
        assert_eq!(
            from_bank_against(vec![0; 4], &container_pin, &payload_pin),
            Err(BackupError::NoMarker)
        );

        // The real pins refuse a synthetic bank.
        assert!(matches!(
            StockBackup::from_bank(original),
            Err(BackupError::NotPinned { .. })
        ));
    }

    fn page(offset: u32, bytes: &[u8]) -> Answer {
        Answer::Bank1(Bank1Page {
            offset,
            total: STOCK_PAYLOAD_LEN,
            bytes: bytes.to_vec(),
        })
    }

    #[test]
    fn pages_join_into_the_bank() {
        let bank = synthetic_bank();
        let mut reader = Bank1Reader::new();
        let mut whole = None;
        for chunk in bank.chunks(wellfed_proto::api::BANK1_PAGE_MAX) {
            let Op::ReadBank1 { offset } = reader.next_op() else {
                panic!("not a bank read");
            };
            whole = reader.take(page(offset, chunk)).unwrap();
        }
        assert_eq!(whole.as_deref(), Some(&bank[..]));
        assert_eq!(reader.read(), 0, "the bank is handed over");
    }

    #[test]
    fn a_page_out_of_order_short_or_refused_ends_the_read() {
        let mut reader = Bank1Reader::new();
        assert_eq!(
            reader.take(page(8, &[1; 8])),
            Err(ReadError::Order { offset: 8 })
        );
        assert_eq!(reader.take(page(0, &[])), Err(ReadError::Short));
        assert_eq!(
            reader.take(Answer::Bank1(Bank1Page {
                offset: 0,
                total: 4_096,
                bytes: vec![1; 8],
            })),
            Err(ReadError::Size { total: 4_096 })
        );
        assert_eq!(
            reader.take(Answer::Refused(Refusal::Unavailable)),
            Err(ReadError::Refused(Refusal::Unavailable))
        );
        let done = Answer::Done { generation: None };
        assert_eq!(
            reader.take(done.clone()),
            Err(ReadError::Unexpected(Box::new(done)))
        );
    }

    #[test]
    fn only_the_exact_container_or_the_bare_payload_passes() {
        let payload = b"stock payload, synthetic".to_vec();
        let mut container = vec![0x11; 32];
        container.extend_from_slice(&payload);
        let pins = (
            <[u8; 32]>::from(Sha256::digest(&container)),
            <[u8; 32]>::from(Sha256::digest(&payload)),
        );
        let check = |image: &[u8]| verify_against(image, &pins.0, &pins.1).is_ok();

        assert!(check(&container));
        assert!(check(&payload));

        // The right payload under a damaged header is refused.
        let mut corrupt = container.clone();
        corrupt[4] ^= 0x01;
        assert!(!check(&corrupt));
        // So are a truncated payload and a payload with bytes appended.
        assert!(!check(&payload[..payload.len() - 1]));
        assert!(!check(&[&payload[..], &[0]].concat()));
    }
}
