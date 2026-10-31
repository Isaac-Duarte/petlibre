//! Gateway rule rendering only: nothing here runs `nft` or changes routing.

mod common;

use wellfed_stock::network::{self, Redirect};

#[test]
fn routing_is_scoped_to_device_mac_and_hotspot_and_blocks_its_wan_only() {
    let mut config = common::config();
    config.mqtt_listen = "10.42.103.1:1883".parse().unwrap();

    let rules = network::rules(&config, Redirect::Broker).unwrap();
    assert!(rules.contains("ether saddr 02:00:00:00:00:42 tcp dport 1883"));
    assert!(rules.contains("dnat ip to 10.42.103.1:1883"));
    // Stock downloads updates from the image server on its fixed port.
    assert!(rules.contains("ip daddr 10.42.103.1 tcp dport 8088 counter accept"));
    // The table and the rule tag are what earlier releases installed on
    // bench hosts, so `remove` still finds those rules.
    assert!(rules.starts_with("table inet wellfed_local {"));
    assert_eq!(rules.matches("comment \"wellfed-local-broker\"").count(), 2);
    assert!(rules.contains("iifname \"wlan1\" ether saddr 02:00:00:00:00:42 counter reject"));
    assert!(!rules.contains("flush ruleset"));

    config.hotspot_interface = Some("wlan1\"; flush ruleset; #".to_owned());
    assert!(network::rules(&config, Redirect::Broker).is_err());
}

/// Nothing redirected, so stock reaches the broker only where a setup frame
/// named it, but its WAN stays blocked and both input accepts stay.
#[test]
fn without_the_redirect_the_wan_block_and_accepts_stay() {
    let mut config = common::config();
    config.mqtt_listen = "10.42.103.1:1883".parse().unwrap();

    let rules = network::rules(&config, Redirect::Off).unwrap();
    assert!(!rules.contains("dnat"));
    assert!(!rules.contains("prerouting"));
    assert!(rules.starts_with("table inet wellfed_local {"));
    assert!(rules.contains("iifname \"wlan1\" ether saddr 02:00:00:00:00:42 meta l4proto tcp counter reject with tcp reset"));
    assert!(rules.contains("iifname \"wlan1\" ether saddr 02:00:00:00:00:42 counter reject"));
    assert!(rules.contains("ip daddr 10.42.103.1 tcp dport 1883 counter accept"));
    assert!(rules.contains("ip daddr 10.42.103.1 tcp dport 8088 counter accept"));
    assert_eq!(rules.matches("comment \"wellfed-local-broker\"").count(), 2);
}

#[test]
fn redirect_cannot_bind_to_loopback_or_modify_an_unvalidated_mac() {
    let mut config = common::config();
    assert!(network::rules(&config, Redirect::Broker).is_err());

    config.mqtt_listen = "10.42.103.1:1883".parse().unwrap();
    config.target_mac = Some("not:a:mac".to_owned());
    assert!(network::rules(&config, Redirect::Broker).is_err());
}
