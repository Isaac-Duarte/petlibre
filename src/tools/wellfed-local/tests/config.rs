//! `init`, `firmware-credential` and `firmware-request-key` run as the real
//! binary on the synthetic example inputs: they write private (0600) files
//! and refuse to replace any secret.

use std::{fs, process::Command};

use wellfed_local::config::Config;
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
    Command::new(env!("CARGO_BIN_EXE_wellfed-local"))
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

#[test]
fn firmware_credential_adds_one_role_writes_a_private_file_and_never_rotates() {
    let dir = TempDir::new().unwrap();
    assert!(init(&inputs(&dir)).status.success());
    let config_path = dir.path().join("server.json");
    let before = Config::load(&config_path).unwrap();
    assert!(before.firmware.is_none());

    // A stock-only configuration has no firmware key at all.
    let raw: serde_json::Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
    assert!(raw.get("firmware").is_none());

    let output = dir.path().join("fw-mqtt.json");
    let args = |out: &std::path::Path| -> Vec<String> {
        vec![
            "firmware-credential".into(),
            "--config".into(),
            config_path.display().to_string(),
            "--output".into(),
            out.display().to_string(),
        ]
    };

    assert!(init(&args(&output)).status.success());
    let config = Config::load(&config_path).unwrap();
    let firmware = config.firmware.as_ref().unwrap();
    assert_eq!(
        firmware.client_id,
        format!("wellfed-fw-{}", config.device_id)
    );
    assert_eq!(firmware.username, "wellfed-fw");
    assert_eq!(firmware.password.len(), 64);

    let credential: serde_json::Value =
        serde_json::from_slice(&fs::read(&output).unwrap()).unwrap();
    assert_eq!(credential["version"], 1);
    assert_eq!(credential["broker"], "127.0.0.1");
    assert_eq!(credential["port"], 18883);
    assert_eq!(credential["device_id"], config.device_id.as_str());
    assert_eq!(credential["client_id"], firmware.client_id.as_str());
    assert_eq!(credential["username"], "wellfed-fw");
    assert_eq!(credential["password"], firmware.password.as_str());
    assert_eq!(
        credential["namespace"],
        format!("wellfed/1/{}/", config.device_id).as_str()
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&output).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    // The stock role and its secrets are untouched.
    assert_eq!(config.device_password, before.device_password);
    assert_eq!(config.server_password, before.server_password);

    // A second run refuses, whether or not the output exists, and changes nothing.
    let saved = fs::read(&config_path).unwrap();
    assert!(!init(&args(&output)).status.success());
    assert!(!init(&args(&dir.path().join("other.json"))).status.success());
    assert!(!dir.path().join("other.json").exists());
    assert_eq!(fs::read(&config_path).unwrap(), saved);
}

#[test]
fn firmware_credential_refuses_an_existing_output_without_touching_the_config() {
    let dir = TempDir::new().unwrap();
    assert!(init(&inputs(&dir)).status.success());
    let config_path = dir.path().join("server.json");
    let output = dir.path().join("fw-mqtt.json");
    fs::write(&output, "keep").unwrap();

    let saved = fs::read(&config_path).unwrap();
    let result = init(&[
        "firmware-credential".into(),
        "--config".into(),
        config_path.display().to_string(),
        "--output".into(),
        output.display().to_string(),
    ]);

    assert!(!result.status.success());
    assert_eq!(fs::read(&output).unwrap(), b"keep");
    assert_eq!(fs::read(&config_path).unwrap(), saved);
}

#[test]
fn an_invalid_firmware_section_is_refused() {
    let mut config = {
        let dir = TempDir::new().unwrap();
        assert!(init(&inputs(&dir)).status.success());
        Config::load(&dir.path().join("server.json")).unwrap()
    };

    let good = wellfed_local::config::FirmwareConfig {
        client_id: config.firmware_client_id(),
        username: "wellfed-fw".into(),
        password: "a".repeat(64),
        request_key: None,
    };

    config.firmware = Some(good.clone());
    assert!(config.validate().is_ok());

    for (client_id, username, password) in [
        (
            "wellfed-fw-other".to_owned(),
            good.username.clone(),
            good.password.clone(),
        ),
        (
            good.client_id.clone(),
            "device".to_owned(),
            good.password.clone(),
        ),
        (
            good.client_id.clone(),
            good.username.clone(),
            "short".to_owned(),
        ),
        (
            good.client_id.clone(),
            good.username.clone(),
            "g".repeat(64),
        ),
    ] {
        config.firmware = Some(wellfed_local::config::FirmwareConfig {
            client_id,
            username,
            password,
            request_key: None,
        });
        assert!(config.validate().is_err());
    }
}

/// A configuration with a firmware role and its credential file, in `dir`.
fn with_firmware_role(dir: &TempDir) -> (std::path::PathBuf, std::path::PathBuf) {
    assert!(init(&inputs(dir)).status.success());
    let config = dir.path().join("server.json");
    let credential = dir.path().join("fw-mqtt.json");
    assert!(
        init(&[
            "firmware-credential".into(),
            "--config".into(),
            config.display().to_string(),
            "--output".into(),
            credential.display().to_string(),
        ])
        .status
        .success()
    );
    (config, credential)
}

fn request_key(config: &std::path::Path, credential: &std::path::Path) -> std::process::Output {
    init(&[
        "firmware-request-key".into(),
        "--config".into(),
        config.display().to_string(),
        "--credential".into(),
        credential.display().to_string(),
    ])
}

#[test]
fn firmware_request_key_adds_one_key_to_both_files_and_never_prints_it() {
    let dir = TempDir::new().unwrap();
    let (config_path, credential_path) = with_firmware_role(&dir);
    let before: serde_json::Value =
        serde_json::from_slice(&fs::read(&credential_path).unwrap()).unwrap();

    let result = request_key(&config_path, &credential_path);
    assert!(result.status.success(), "{:?}", result.stderr);

    let config = Config::load(&config_path).unwrap();
    let firmware = config.firmware.as_ref().unwrap();
    let key = firmware.request_key.clone().unwrap();
    assert!(wellfed_local::replacement::request_auth::valid_key_hex(
        &key
    ));
    assert!(firmware.request_key().is_some());

    // The credential file keeps every field and gains the same key.
    let mut credential: serde_json::Value =
        serde_json::from_slice(&fs::read(&credential_path).unwrap()).unwrap();
    assert_eq!(credential["request_key"], key.as_str());
    credential.as_object_mut().unwrap().remove("request_key");
    assert_eq!(credential, before);

    let output = String::from_utf8_lossy(&result.stdout) + String::from_utf8_lossy(&result.stderr);
    assert!(!output.contains(&key), "the key was printed");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for path in [&config_path, &credential_path] {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    // A second run refuses and changes nothing.
    let saved = (
        fs::read(&config_path).unwrap(),
        fs::read(&credential_path).unwrap(),
    );
    assert!(!request_key(&config_path, &credential_path).status.success());
    assert_eq!(
        (
            fs::read(&config_path).unwrap(),
            fs::read(&credential_path).unwrap()
        ),
        saved
    );
}

#[test]
fn firmware_request_key_refuses_without_a_role_or_with_a_mismatched_or_keyed_file() {
    // No firmware role.
    let dir = TempDir::new().unwrap();
    assert!(init(&inputs(&dir)).status.success());
    let config_path = dir.path().join("server.json");
    let credential_path = dir.path().join("fw-mqtt.json");
    fs::write(&credential_path, "{}").unwrap();
    let saved = fs::read(&config_path).unwrap();
    assert!(!request_key(&config_path, &credential_path).status.success());
    assert_eq!(fs::read(&config_path).unwrap(), saved);
    assert_eq!(fs::read(&credential_path).unwrap(), b"{}");

    // A credential file that does not match the role, already has a key,
    // carries unknown fields or is missing.
    let edits: [(&str, serde_json::Value); 4] = [
        ("password", serde_json::json!("f".repeat(64))),
        ("port", serde_json::json!(1)),
        ("request_key", serde_json::json!("5a".repeat(32))),
        ("extra", serde_json::json!(true)),
    ];
    for (field, value) in edits {
        let dir = TempDir::new().unwrap();
        let (config_path, credential_path) = with_firmware_role(&dir);
        let mut credential: serde_json::Value =
            serde_json::from_slice(&fs::read(&credential_path).unwrap()).unwrap();
        credential[field] = value;
        fs::write(&credential_path, credential.to_string()).unwrap();

        let saved = (
            fs::read(&config_path).unwrap(),
            fs::read(&credential_path).unwrap(),
        );
        assert!(
            !request_key(&config_path, &credential_path).status.success(),
            "{field}"
        );
        assert_eq!(
            (
                fs::read(&config_path).unwrap(),
                fs::read(&credential_path).unwrap()
            ),
            saved
        );
    }

    let dir = TempDir::new().unwrap();
    let (config_path, _) = with_firmware_role(&dir);
    assert!(
        !request_key(&config_path, &dir.path().join("missing.json"))
            .status
            .success()
    );

    // A configuration that already has a key refuses even a clean credential file.
    let dir = TempDir::new().unwrap();
    let (config_path, credential_path) = with_firmware_role(&dir);
    let mut config = Config::load(&config_path).unwrap();
    config.firmware.as_mut().unwrap().request_key = Some("5a".repeat(32));
    wellfed_local::storage::write_json(&config_path, &config).unwrap();
    let saved = fs::read(&credential_path).unwrap();
    assert!(!request_key(&config_path, &credential_path).status.success());
    assert_eq!(fs::read(&credential_path).unwrap(), saved);
}

#[test]
fn a_request_key_must_be_64_lowercase_hex_characters() {
    let dir = TempDir::new().unwrap();
    let (config_path, _) = with_firmware_role(&dir);
    let mut config = Config::load(&config_path).unwrap();
    for (key, ok) in [
        (None, true),
        (Some("0123456789abcdef".repeat(4)), true),
        (Some("0123456789ABCDEF".repeat(4)), false),
        (Some("0".repeat(63)), false),
        (Some("0".repeat(65)), false),
        (Some(String::new()), false),
    ] {
        config.firmware.as_mut().unwrap().request_key = key.clone();
        assert_eq!(config.validate().is_ok(), ok, "{key:?}");
    }

    // Load refuses an invalid key too.
    config.firmware.as_mut().unwrap().request_key = Some("xyz".into());
    let raw = serde_json::to_vec(&config).unwrap();
    fs::write(&config_path, raw).unwrap();
    assert!(Config::load(&config_path).is_err());
}
