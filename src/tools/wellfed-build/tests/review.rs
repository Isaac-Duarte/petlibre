//! Frozen reviews of the real first-boot build and stock image, as
//! `wellfed-flash` then reads them. Offline: the factory loader is emulated.
//!
//! Needs a fresh ignored PLAF103 first-boot build, the pinned binutils, the
//! baseline, SDK ROM and stock OTA. `WELLFED_FIXTURE_ROOT` names another
//! checkout to review in; outputs go to temporary directories under its
//! `private/`.

use std::fs;
use std::path::PathBuf;

use wellfed_build::Repo;
use wellfed_build::review::{bank_payload, freeze, mapping_cases};
use wellfed_emu::factory;
use wellfed_flash::evidence::Root;
use wellfed_flash::writers::development::BANK1;
use wellfed_formats::manifest::BuildManifest;
use wellfed_formats::review::{Details, ImageKind};

fn repo() -> Repo {
    let root = std::env::var_os("WELLFED_FIXTURE_ROOT")
        .map_or_else(|| Repo::here().unwrap().root, PathBuf::from);
    Repo {
        root: root.canonicalize().unwrap(),
    }
}

#[test]
#[ignore = "needs a fresh first-boot build, pinned binutils, baseline, SDK ROM and stock OTA"]
fn a_reviewed_build_is_frozen_and_accepted_by_the_flash_client() {
    let repo = repo();
    let scratch = tempfile::tempdir_in(repo.root.join("private")).unwrap();
    let output = scratch.path().join("review");
    let build = repo.root.join("private/firmware-build/plaf103/firstboot");

    let review = freeze(&repo, ImageKind::Build, &build, &output).unwrap();

    let Details::Build(details) = &review.review else {
        panic!("a build review has build details");
    };
    let banks: Vec<u8> = details
        .factory_mapping
        .iter()
        .map(|case| case.bank)
        .collect();
    assert_eq!(banks, [0, 1]);
    let root = Root::new(&repo.root).unwrap();
    let frozen = wellfed_flash::evidence::review::frozen(&root, &output, &BANK1).unwrap();
    assert_eq!(frozen.image, fs::read(output.join("image.bin")).unwrap());

    // A second freeze into the same directory is refused.
    assert!(freeze(&repo, ImageKind::Build, &build, &output).is_err());
}

#[test]
#[ignore = "needs the stock OTA"]
fn stock_is_frozen_by_hash_and_accepted_by_the_flash_client() {
    let repo = repo();
    let scratch = tempfile::tempdir_in(repo.root.join("private")).unwrap();
    let output = scratch.path().join("stock");

    freeze(&repo, ImageKind::Stock, &PathBuf::new(), &output).unwrap();

    let root = Root::new(&repo.root).unwrap();
    assert!(wellfed_flash::evidence::review::frozen(&root, &output, &BANK1).is_ok());
}

/// The actual factory loader accepts the first-boot build in either bank of
/// the current full read, as well as of the baseline the review uses.
#[test]
#[ignore = "needs a first-boot build, the current full read, baseline and SDK ROM"]
fn the_factory_loader_accepts_the_first_boot_build_in_both_banks_of_the_current_read() {
    let repo = repo();
    let build = repo.root.join("private/firmware-build/plaf103/firstboot");
    let manifest: BuildManifest = wellfed_formats::load(&build.join("manifest.json")).unwrap();
    let ota = manifest.artifact("OTA_All.bin").unwrap();
    let image = fs::read(build.join(&ota.path)).unwrap();
    let current = fs::read(
        repo.root
            .join("private/fixtures/current-read/flash-read.bin"),
    )
    .unwrap();
    let (_, rom) = factory::inputs(&repo.root).unwrap();

    let cases = mapping_cases(&current, &rom, &image).unwrap();
    let results: Vec<u32> = cases.iter().map(|case| case.selection.result).collect();
    assert_eq!(results, [1, 2]);
    assert!(bank_payload(&image).is_ok());
}
