use std::{fs, process::Command};

use petlibro_local::config::Config;
use tempfile::TempDir;

fn inputs(dir: &TempDir) -> Vec<String> {
    let credentials = dir.path().join("credentials.json");
    let capture = dir.path().join("capture.json");
    fs::write(
        &credentials,
        include_str!("../examples/mqtt-credentials.json"),
    )
    .unwrap();
    fs::write(&capture, include_str!("../examples/mqtt-capture.json")).unwrap();
    vec![
        "init".into(),
        "--credentials".into(),
        credentials.display().to_string(),
        "--capture".into(),
        capture.display().to_string(),
        "--mqtt-listen".into(),
        "127.0.0.1:18883".into(),
        "--hotspot-interface".into(),
        "test_wifi".into(),
        "--output".into(),
        dir.path().join("server.json").display().to_string(),
    ]
}

fn init(args: &[String]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_petlibro-local"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn imports_explicit_settings_with_utc_default_and_private_permissions() {
    let dir = TempDir::new().unwrap();
    let result = init(&inputs(&dir));
    assert!(result.status.success(), "{:?}", result.stderr);
    let path = dir.path().join("server.json");
    let config = Config::load(&path).unwrap();
    assert_eq!(config.timezone, chrono_tz::UTC);
    assert_eq!(config.mqtt_listen.to_string(), "127.0.0.1:18883");
    assert_eq!(config.hotspot_interface, "test_wifi");
    assert_eq!(config.max_portions, 1);
    assert!(config.plans.is_empty());
    assert_eq!(config.device_id, "SYNTHETIC_FEEDER");
    assert_ne!(config.api_token, config.server_password);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn accepts_an_explicit_iana_timezone_and_refuses_to_replace_configuration() {
    let dir = TempDir::new().unwrap();
    let mut args = inputs(&dir);
    args.extend(["--timezone".into(), "Europe/Paris".into()]);
    assert!(init(&args).status.success());
    let path = dir.path().join("server.json");
    assert_eq!(
        Config::load(&path).unwrap().timezone,
        chrono_tz::Europe::Paris
    );
    let original = fs::read(&path).unwrap();
    assert!(!init(&args).status.success());
    assert_eq!(fs::read(path).unwrap(), original);
}

#[test]
fn missing_inputs_or_invalid_network_settings_do_not_write_configuration() {
    let dir = TempDir::new().unwrap();
    let args = inputs(&dir);
    for flag in [
        "--credentials",
        "--capture",
        "--mqtt-listen",
        "--hotspot-interface",
    ] {
        let mut missing = args.clone();
        let at = missing.iter().position(|value| value == flag).unwrap();
        missing.drain(at..at + 2);
        let result = init(&missing);
        assert!(!result.status.success(), "accepted missing {flag}");
        assert!(!dir.path().join("server.json").exists());
    }
    for (flag, value) in [
        ("--mqtt-listen", "0.0.0.0:1883"),
        ("--mqtt-listen", "[::1]:1883"),
        ("--hotspot-interface", "bad;interface"),
    ] {
        let mut invalid = args.clone();
        let at = invalid.iter().position(|arg| arg == flag).unwrap();
        invalid[at + 1] = value.into();
        assert!(!init(&invalid).status.success());
        assert!(!dir.path().join("server.json").exists());
    }
}
