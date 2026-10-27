//! The settings file and the flags over it.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::num::NonZeroU32;
use std::path::Path;

use clap::Parser;
use wellfed_server::config::{
    ConfigError, Discovery, FILE, HostName, Label, Overrides, ServerConfig,
};

/// The server's flags alone, as `wellfed-server` takes them.
#[derive(Parser)]
struct Flags {
    #[command(flatten)]
    overrides: Overrides,
}

fn flags(args: &[&str]) -> Overrides {
    Flags::try_parse_from(std::iter::once("wellfed-server").chain(args.iter().copied()))
        .unwrap()
        .overrides
}

fn days(n: u32) -> NonZeroU32 {
    NonZeroU32::new(n).unwrap()
}

fn invalid_field(text: &str) -> String {
    match ServerConfig::parse(Path::new(FILE), text).unwrap_err() {
        ConfigError::Invalid { field, .. } => field,
        other => panic!("expected an invalid field, got {other:?}"),
    }
}

#[test]
fn no_file_is_the_defaults() {
    let data = tempfile::tempdir().unwrap();

    let config = ServerConfig::load(data.path(), Overrides::default()).unwrap();

    assert_eq!(config.listen, SocketAddr::from((Ipv4Addr::LOCALHOST, 8080)));
    assert!(config.hosts.is_empty());
    assert_eq!(config.label.as_str(), "wellfed-server");
    assert_eq!(config.discovery, Discovery::Mdns);
    assert!(config.trusted_proxies.is_empty());
    assert_eq!(config.retention.raw_sample_days, None);
    assert!(!config.releases.fetch);
    assert_eq!(config.backups.keep, days(7));
    assert_eq!(
        config,
        ServerConfig::parse(Path::new(FILE), "").unwrap(),
        "an empty file is the defaults too"
    );
}

/// The documented example parses, and says what it shows.
#[test]
fn the_example_file_parses() {
    let text = include_str!("../server.example.toml");

    let config = ServerConfig::parse(Path::new("server.example.toml"), text).unwrap();

    assert_eq!(config.hosts, ["feeder.lan".parse::<HostName>().unwrap()]);
    assert_eq!(config.trusted_proxies, [IpAddr::from(Ipv4Addr::LOCALHOST)]);
    assert_eq!(config.retention.raw_sample_days, Some(days(90)));
    assert!(!config.releases.fetch);
    assert_eq!(config.backups.keep, days(7));
}

#[test]
fn the_file_sets_every_setting() {
    let data = tempfile::tempdir().unwrap();
    std::fs::write(
        data.path().join(FILE),
        r#"
listen = "0.0.0.0:9000"
hosts = ["Feeder.LAN", "pets.home.arpa"]
label = "kitchen-server"
discovery = "off"
trusted_proxies = ["10.0.0.2", "::1"]

[retention]
raw_sample_days = 30

[releases]
fetch = true

[backups]
keep = 3
"#,
    )
    .unwrap();

    let config = ServerConfig::load(data.path(), Overrides::default()).unwrap();

    assert_eq!(config.listen, "0.0.0.0:9000".parse().unwrap());
    let hosts: Vec<&str> = config.hosts.iter().map(HostName::as_str).collect();
    assert_eq!(
        hosts,
        ["feeder.lan", "pets.home.arpa"],
        "names are lowercased"
    );
    assert_eq!(config.label.as_str(), "kitchen-server");
    assert_eq!(config.discovery, Discovery::Off);
    assert_eq!(
        config.trusted_proxies,
        [
            "10.0.0.2".parse::<IpAddr>().unwrap(),
            "::1".parse().unwrap()
        ]
    );
    assert_eq!(config.retention.raw_sample_days, Some(days(30)));
    assert!(config.releases.fetch);
    assert_eq!(config.backups.keep, days(3));
}

/// A flag replaces the file's value whole, and leaves the others alone.
#[test]
fn flags_override_the_file() {
    let data = tempfile::tempdir().unwrap();
    std::fs::write(
        data.path().join(FILE),
        r#"
listen = "0.0.0.0:9000"
hosts = ["feeder.lan", "pets.lan"]
discovery = "mdns"
trusted_proxies = ["10.0.0.2"]

[retention]
raw_sample_days = 30

[releases]
fetch = true
"#,
    )
    .unwrap();

    let overrides = flags(&[
        "--listen",
        "127.0.0.1:7000",
        "--host",
        "other.lan",
        "--no-discovery",
        "--raw-sample-days",
        "5",
        "--releases-fetch",
        "false",
        "--backups-keep",
        "2",
    ]);
    let config = ServerConfig::load(data.path(), overrides).unwrap();

    assert_eq!(config.listen, "127.0.0.1:7000".parse().unwrap());
    assert_eq!(config.hosts, ["other.lan".parse::<HostName>().unwrap()]);
    assert_eq!(config.discovery, Discovery::Off);
    assert_eq!(config.retention.raw_sample_days, Some(days(5)));
    assert!(!config.releases.fetch);
    assert_eq!(config.backups.keep, days(2));
    assert_eq!(
        config.trusted_proxies,
        ["10.0.0.2".parse::<IpAddr>().unwrap()],
        "a flag not given keeps the file's value"
    );
}

#[test]
fn flags_check_their_values() {
    let parse = |args: &[&str]| {
        Flags::try_parse_from(std::iter::once("wellfed-server").chain(args.iter().copied()))
    };

    assert!(parse(&["--label", "a-label-longer-than-16"]).is_err());
    assert!(parse(&["--host", "evil.example:80"]).is_err());
    assert!(parse(&["--raw-sample-days", "0"]).is_err());
    assert_eq!(
        flags(&["--discovery", "off"]).discovery,
        Some(Discovery::Off)
    );
    assert_eq!(
        flags(&["--host", "a.lan,b.lan"]).hosts,
        [
            "a.lan".parse::<HostName>().unwrap(),
            "b.lan".parse().unwrap()
        ]
    );
}

/// `--no-discovery` wins over `--discovery`, given as a flag or set in the
/// environment, rather than conflicting with it.
#[test]
fn no_discovery_wins_over_discovery() {
    let data = tempfile::tempdir().unwrap();
    std::fs::write(data.path().join(FILE), "discovery = \"mdns\"").unwrap();

    let config = ServerConfig::load(
        data.path(),
        flags(&["--discovery", "mdns", "--no-discovery"]),
    )
    .unwrap();
    assert_eq!(config.discovery, Discovery::Off);

    // clap counts a set variable as given, so run the binary with one set.
    // A bad settings file stops it right after the flags parse, before it
    // makes a key or listens.
    std::fs::write(data.path().join(FILE), "listen = 8080").unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_wellfed-server"))
        .arg("--data")
        .arg(data.path())
        .arg("--no-discovery")
        .env("WELLFED_SERVER_DISCOVERY", "mdns")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("`listen` is invalid"), "{stderr}");
}

#[test]
fn a_host_name_is_dns_labels() {
    for name in ["feeder.lan", "a-b.c", "x", &"a".repeat(63)] {
        assert!(name.parse::<HostName>().is_ok(), "{name}");
    }
    for name in [
        "",
        "feeder..lan",
        "-feeder.lan",
        "feeder-.lan",
        "feeder.lan.",
        "feeder lan",
        &"a".repeat(64),
        &["a"; 128].join("."),
    ] {
        assert!(name.parse::<HostName>().is_err(), "{name}");
    }
}

/// Each refusal names the key it is about.
#[test]
fn a_bad_value_names_its_field() {
    assert_eq!(invalid_field("listen = 8080"), "listen");
    assert_eq!(invalid_field("label = \"\""), "label");
    assert_eq!(invalid_field("label = \"far-too-long-a-label\""), "label");
    assert_eq!(
        invalid_field("hosts = [\"ok.lan\", \"not a name\"]"),
        "hosts[1]"
    );
    assert_eq!(invalid_field("discovery = \"sometimes\""), "discovery");
    assert_eq!(
        invalid_field("trusted_proxies = [\"10.0.0.0/8\"]"),
        "trusted_proxies[0]"
    );
    assert_eq!(
        invalid_field("[retention]\nraw_sample_days = 0"),
        "retention.raw_sample_days"
    );
    assert_eq!(
        invalid_field("[releases]\nfetch = \"yes\""),
        "releases.fetch"
    );
    assert_eq!(invalid_field("[backups]\nkeep = -1"), "backups.keep");
}

#[test]
fn an_unknown_key_is_refused() {
    let error = ServerConfig::parse(
        Path::new(FILE),
        "[releases]\nfetch = true\nchannel = \"beta\"",
    )
    .unwrap_err();

    let ConfigError::Invalid { field, source, .. } = &error else {
        panic!("expected an invalid field, got {error:?}");
    };
    assert_eq!(field, "releases.channel");
    assert_eq!(source.span().map(|span| span.start), Some(24), "{source}");
    assert_eq!(invalid_field("lisen = \"0.0.0.0:80\""), "lisen");
}

#[test]
fn a_file_that_is_not_toml_is_refused() {
    let error = ServerConfig::parse(Path::new(FILE), "listen = ").unwrap_err();

    assert!(matches!(error, ConfigError::Syntax { .. }), "{error:?}");
}

#[test]
fn a_label_is_one_to_sixteen_bytes() {
    assert!("a".parse::<Label>().is_ok());
    assert!("sixteen-bytes-ok".parse::<Label>().is_ok());
    assert!("seventeen-bytes!!".parse::<Label>().is_err());
    assert!("nul\0".parse::<Label>().is_err());
}
