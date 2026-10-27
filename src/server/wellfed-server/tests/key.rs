//! The sealing key's file, and the move from `config.json`.

use std::path::{Path, PathBuf};

use wellfed_server::config::DATABASE;
use wellfed_server::key::{KEY_DIR, KEY_FILE, KeyError, KeySource, sealing_key};
use wellfed_server_testkit::fixed::SeededRandom;

fn key_path(data: &Path) -> PathBuf {
    data.join(KEY_DIR).join(KEY_FILE)
}

/// `config.json` as servers before the settings file wrote it.
fn write_old(data: &Path, key: [u8; 32]) {
    let hex: String = key.iter().map(|byte| format!("{byte:02x}")).collect();
    std::fs::write(
        data.join("config.json"),
        format!("{{\n  \"key\": \"{hex}\"\n}}"),
    )
    .unwrap();
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn the_first_start_makes_a_key_only_its_user_reads() {
    let data = tempfile::tempdir().unwrap();

    let (key, source) = sealing_key(data.path(), &SeededRandom::new(1)).unwrap();

    assert_eq!(source, KeySource::Made);
    assert_eq!(std::fs::read(key_path(data.path())).unwrap(), key);
    #[cfg(unix)]
    {
        assert_eq!(mode(&key_path(data.path())), 0o600);
        assert_eq!(mode(&data.path().join(KEY_DIR)), 0o700);
    }

    let (again, source) = sealing_key(data.path(), &SeededRandom::new(2)).unwrap();
    assert_eq!(
        (again, source),
        (key, KeySource::Read),
        "a restart reads it"
    );
}

/// A server from before the settings file keeps its key, and the old file
/// holding it is gone.
#[test]
fn the_key_moves_from_config_json() {
    let data = tempfile::tempdir().unwrap();
    let old = [0x5a; 32];
    write_old(data.path(), old);
    std::fs::write(data.path().join(DATABASE), b"a database").unwrap();

    let (key, source) = sealing_key(data.path(), &SeededRandom::new(1)).unwrap();

    assert_eq!((key, source), (old, KeySource::Moved));
    assert_eq!(std::fs::read(key_path(data.path())).unwrap(), old);
    #[cfg(unix)]
    assert_eq!(mode(&key_path(data.path())), 0o600);
    assert!(
        !data.path().join("config.json").exists(),
        "the old file is deleted"
    );
    assert!(
        !key_path(data.path()).with_extension("key.new").exists(),
        "nothing is left staged"
    );

    let (again, source) = sealing_key(data.path(), &SeededRandom::new(2)).unwrap();
    assert_eq!((again, source), (old, KeySource::Read));
}

/// A start stopped after the key file was written and before `config.json`
/// was deleted: the next one finishes the move.
#[test]
fn an_interrupted_move_finishes() {
    let data = tempfile::tempdir().unwrap();
    let old = [0x5a; 32];
    std::fs::create_dir(data.path().join(KEY_DIR)).unwrap();
    std::fs::write(key_path(data.path()), old).unwrap();
    write_old(data.path(), old);

    let (key, source) = sealing_key(data.path(), &SeededRandom::new(1)).unwrap();

    assert_eq!((key, source), (old, KeySource::Read));
    assert!(!data.path().join("config.json").exists());
}

#[test]
fn two_different_keys_are_refused() {
    let data = tempfile::tempdir().unwrap();
    std::fs::create_dir(data.path().join(KEY_DIR)).unwrap();
    std::fs::write(key_path(data.path()), [1; 32]).unwrap();
    write_old(data.path(), [2; 32]);

    let error = sealing_key(data.path(), &SeededRandom::new(1)).unwrap_err();

    assert!(matches!(error, KeyError::Conflict { .. }), "{error:?}");
    assert!(
        data.path().join("config.json").exists(),
        "neither key is deleted"
    );
    assert_eq!(std::fs::read(key_path(data.path())).unwrap(), [1; 32]);
}

/// A database restored without its key gets no new key, which could not
/// open the feeder keys it holds.
#[test]
fn a_database_without_its_key_is_refused() {
    let data = tempfile::tempdir().unwrap();
    std::fs::write(data.path().join(DATABASE), b"a database").unwrap();

    let error = sealing_key(data.path(), &SeededRandom::new(1)).unwrap_err();

    assert!(matches!(error, KeyError::Missing { .. }), "{error:?}");
    assert!(!key_path(data.path()).exists());
}

#[test]
fn a_short_key_file_is_refused() {
    let data = tempfile::tempdir().unwrap();
    std::fs::create_dir(data.path().join(KEY_DIR)).unwrap();
    std::fs::write(key_path(data.path()), [1; 31]).unwrap();

    let error = sealing_key(data.path(), &SeededRandom::new(1)).unwrap_err();

    assert!(
        matches!(error, KeyError::Length { len: 31, .. }),
        "{error:?}"
    );
}

#[test]
fn a_garbled_config_json_is_refused() {
    let data = tempfile::tempdir().unwrap();
    std::fs::write(data.path().join("config.json"), b"{\"key\": \"00\"}").unwrap();

    let error = sealing_key(data.path(), &SeededRandom::new(1)).unwrap_err();

    assert!(matches!(error, KeyError::OldFile { .. }), "{error:?}");
    assert!(data.path().join("config.json").exists());
}
