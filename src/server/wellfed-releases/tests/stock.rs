//! The feeders' stock backups over a temporary directory: a copy is
//! checked against the pins before it is kept, kept per feeder as a file
//! with its record, checked again on every read, and never replaced. The
//! pins are a synthetic stock's: the vendor's image stays out of the
//! repository.

#[expect(dead_code, reason = "the release helpers serve the other test files")]
mod support;

use support::{now, open, stock_bank, stock_pins};
use wellfed_api::{StockBackupState, StockSource};
use wellfed_releases::{Added, Error, Releases, StockConfig, StockPins, Taken};

const FEEDER: [u8; 32] = [0xab; 32];

/// The bank as stock leaves it once it installed a build into bank 0: the
/// marker's first byte zeroed.
fn given_way() -> Vec<u8> {
    let mut bank = stock_bank();
    bank[0] = 0;
    bank
}

#[tokio::test]
async fn bank_1_read_from_a_feeder_is_verified_and_kept_as_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let releases = open(&dir, None).await;
    let stock = releases.stock();
    assert_eq!(stock.view(FEEDER).await.unwrap(), None);

    let (kept, added) = stock
        .keep(FEEDER, Taken::Bank(given_way()), now())
        .await
        .unwrap();

    assert_eq!(added, Added::New);
    assert_eq!(kept.version, "1.1.15");
    assert_eq!(kept.state, StockBackupState::Verified);
    assert_eq!(kept.source, StockSource::Feeder);
    assert_eq!(kept.kept, now());
    assert_eq!(kept.bytes, 32 + 0x15_0000);

    // The file is the vendor's container around the bank, marker restored.
    let file = dir
        .path()
        .join("stock")
        .join("ab".repeat(32))
        .join("stock-1.1.15.bin");
    let bytes = std::fs::read(&file).unwrap();
    assert_eq!(&bytes[8..12], b"OTA1");
    assert_eq!(&bytes[32..], &stock_bank()[..]);

    let (read, backup) = stock.backup(FEEDER).await.unwrap();
    assert_eq!(read, kept);
    assert_eq!(backup.bytes(), &bytes[..]);
    assert_eq!(stock.view(FEEDER).await.unwrap(), Some(kept));
    assert!(matches!(
        stock.backup([0xcd; 32]).await,
        Err(Error::NoStock)
    ));
}

#[tokio::test]
async fn the_same_copy_again_is_known_and_a_foreign_one_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let releases = open(&dir, None).await;
    let stock = releases.stock();
    let (kept, _) = stock
        .keep(FEEDER, Taken::Bank(given_way()), now())
        .await
        .unwrap();
    let (_, backup) = stock.backup(FEEDER).await.unwrap();

    // The exported container, or the bare payload, is the copy kept.
    let container = backup.bytes().to_vec();
    let again = stock.keep(FEEDER, Taken::Upload(container), now()).await;
    assert_eq!(again.unwrap(), (kept.clone(), Added::Known));
    let payload = stock.keep(FEEDER, Taken::Upload(stock_bank()), now()).await;
    assert_eq!(payload.unwrap(), (kept, Added::Known));

    // Anything else is not stock, and nothing is written for it.
    let mut damaged = stock_bank();
    damaged[0x1_0000] ^= 1;
    let refused = stock.keep([0xcd; 32], Taken::Upload(damaged), now()).await;
    assert!(matches!(refused, Err(Error::Stock(_))));
    assert_eq!(stock.view([0xcd; 32]).await.unwrap(), None);
}

#[tokio::test]
async fn a_kept_copy_is_never_replaced_by_another() {
    let dir = tempfile::tempdir().unwrap();
    let releases = open(&dir, None).await;
    releases
        .stock()
        .keep(FEEDER, Taken::Upload(stock_bank()), now())
        .await
        .unwrap();

    // Another stock, as a cache pinned to it would take it.
    let mut other = stock_bank();
    other[100] ^= 1;
    let elsewhere = Releases::open(wellfed_releases::Config {
        dir: dir.path().join("releases"),
        keys: support::keys(),
        fetch: None,
        stock: StockConfig {
            dir: dir.path().join("stock"),
            pins: StockPins::of_payload(&other),
        },
    })
    .await
    .unwrap();
    let refused = elsewhere
        .stock()
        .keep(FEEDER, Taken::Upload(other), now())
        .await;
    assert!(matches!(refused, Err(Error::StockDiffers)));

    let (_, backup) = releases.stock().backup(FEEDER).await.unwrap();
    assert_eq!(&backup.bytes()[32..], &stock_bank()[..]);
}

#[tokio::test]
async fn a_file_damaged_on_disk_is_neither_exported_nor_restored_until_kept_again() {
    let dir = tempfile::tempdir().unwrap();
    let releases = open(&dir, None).await;
    let stock = releases.stock();
    stock
        .keep(FEEDER, Taken::Bank(given_way()), now())
        .await
        .unwrap();
    let file = dir
        .path()
        .join("stock")
        .join("ab".repeat(32))
        .join("stock-1.1.15.bin");
    let mut bytes = std::fs::read(&file).unwrap();
    bytes[4_000] ^= 0x10;
    std::fs::write(&file, &bytes).unwrap();

    let view = stock.view(FEEDER).await.unwrap().unwrap();
    assert_eq!(view.state, StockBackupState::Damaged);
    assert!(matches!(
        stock.backup(FEEDER).await,
        Err(Error::StockDamaged)
    ));

    // The same copy again writes the file back whole.
    let (kept, added) = stock
        .keep(FEEDER, Taken::Bank(given_way()), now())
        .await
        .unwrap();
    assert_eq!(added, Added::Known);
    assert_eq!(kept.state, StockBackupState::Verified);
    assert!(stock.backup(FEEDER).await.is_ok());
}

/// The pins every test cache uses are a synthetic stock's, which the real
/// pins refuse: no test can keep or restore anything a real server would
/// not.
#[test]
fn the_test_pins_are_not_stocks() {
    assert_ne!(stock_pins(), StockPins::STOCK);
}
