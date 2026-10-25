//! The replacement firmware's broker roles, beside stock's
//! (`wellfed_stock::mqtt::broker`): the firmware's transport, its `-ota`
//! offer watcher, and the server's side of both, each limited to literal
//! topics in the firmware namespace. Without a configured role the
//! namespace is closed to everyone.

use wellfed_stock::mqtt::broker::Roles;
use subtle::ConstantTimeEq;

use crate::config::Config;

/// The roles the configured firmware credential admits.
pub struct Firmware(pub Config);

impl Firmware {
    /// The topic's suffix in the replacement-firmware namespace, only when
    /// that role is configured. Wildcards never match: every allowed suffix
    /// is literal.
    fn suffix<'a>(&self, topic: &'a str) -> Option<&'a str> {
        self.0.firmware.as_ref()?;
        topic.strip_prefix(&self.0.firmware_namespace())
    }
}

impl Roles for Firmware {
    fn authenticate(
        &self,
        client_id: &str,
        username: &str,
        password: &[u8],
    ) -> Option<&'static str> {
        let firmware = self.0.firmware.as_ref()?;
        let login = |expected: &str| {
            client_id == expected
                && username == firmware.username
                && bool::from(password.ct_eq(firmware.password.as_bytes()))
        };

        // The normal role's offer check, a second session beside the
        // transport's, so it needs its own client ID.
        if login(&firmware.client_id) {
            Some("firmware")
        } else if login(&format!("{}-ota", firmware.client_id)) {
            Some("firmware-watch")
        } else {
            None
        }
    }

    fn publish_allowed(&self, user: Option<&str>, topic: &str) -> bool {
        let firmware = self.suffix(topic);
        match user {
            Some("firmware") => matches!(firmware, Some("health" | "reply" | "ota/get")),
            Some("firmware-watch") => matches!(firmware, Some("ota/check")),
            Some("server") => matches!(firmware, Some("request" | "ota/data" | "ota/offered")),
            _ => false,
        }
    }

    fn subscribe_allowed(&self, user: Option<&str>, topic: &str) -> bool {
        let firmware = self.suffix(topic);
        match user {
            Some("firmware") => matches!(firmware, Some("request" | "ota/data")),
            Some("firmware-watch") => matches!(firmware, Some("ota/offered")),
            Some("server") => {
                matches!(firmware, Some("health" | "reply" | "ota/get" | "ota/check"))
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wellfed_stock::mqtt::broker as stock;

    fn config(firmware: bool) -> Config {
        let mut value = serde_json::json!({
            "version":1,"device_id":"AF_TEST","device_client_id":"AF_TEST",
            "device_username":"dev","device_password":"pw",
            "server_password":"s".repeat(32),"api_token":"t".repeat(32),
            "mqtt_listen":"127.0.0.1:1883","http_listen":"127.0.0.1:8765",
            "timezone":"UTC","plans":[],"binding_response":null,
            "hotspot_interface":"wlan1","target_mac":"02:00:00:00:00:01"
        });

        if firmware {
            value["firmware"] = serde_json::json!({
                "client_id":"wellfed-fw-AF_TEST","username":"wellfed-fw","password":"a".repeat(64)
            });
        }

        let config = Config::parse(&serde_json::to_vec(&value).unwrap()).unwrap();
        config.validate().unwrap();
        config
    }

    /// What the broker grants: stock's allowlist or these roles', as
    /// `wellfed_stock`'s `DeviceAuth` composes them.
    fn publish_allowed(c: &Config, user: Option<&str>, topic: &str) -> bool {
        stock::publish_allowed(c, user, topic) || Firmware(c.clone()).publish_allowed(user, topic)
    }

    fn subscribe_allowed(c: &Config, user: Option<&str>, topic: &str) -> bool {
        stock::subscribe_allowed(c, user, topic)
            || Firmware(c.clone()).subscribe_allowed(user, topic)
    }

    #[test]
    fn logins_need_the_configured_role_and_its_password() {
        let c = Firmware(config(true));
        let password = "a".repeat(64);
        let password = password.as_bytes();

        assert_eq!(
            c.authenticate("wellfed-fw-AF_TEST", "wellfed-fw", password),
            Some("firmware")
        );
        assert_eq!(
            c.authenticate("wellfed-fw-AF_TEST-ota", "wellfed-fw", password),
            Some("firmware-watch")
        );
        assert_eq!(
            c.authenticate("wellfed-fw-AF_TEST", "wellfed-fw", b"wrong"),
            None
        );
        assert_eq!(c.authenticate("AF_TEST", "wellfed-fw", password), None);
        assert_eq!(
            Firmware(config(false)).authenticate("wellfed-fw-AF_TEST", "wellfed-fw", password),
            None
        );
    }

    const FW: &str = "wellfed/1/AF_TEST/";
    const STOCK: &str = "dl/PLAF103/AF_TEST/device/";

    #[test]
    fn firmware_role_reaches_only_its_literal_topics() {
        let c = config(true);
        let user = Some("firmware");

        for (topic, publish, subscribe) in [
            ("health", true, false),
            ("reply", true, false),
            ("request", false, true),
            ("ota/get", true, false),
            ("ota/data", false, true),
            ("ota/+", false, false),
            ("ota/#", false, false),
            ("+", false, false),
            ("#", false, false),
            ("other", false, false),
        ] {
            let topic = format!("{FW}{topic}");
            assert_eq!(publish_allowed(&c, user, &topic), publish, "{topic}");
            assert_eq!(subscribe_allowed(&c, user, &topic), subscribe, "{topic}");
        }

        for topic in ["event/post", "event/sub", "+/sub", "+/post"] {
            let topic = format!("{STOCK}{topic}");
            assert!(!publish_allowed(&c, user, &topic) && !subscribe_allowed(&c, user, &topic));
        }

        assert!(!subscribe_allowed(&c, user, "wellfed/#"));
        assert!(!subscribe_allowed(&c, user, "wellfed/1/OTHER/request"));
    }

    #[test]
    fn server_gains_the_reverse_topics_and_stock_device_gains_none() {
        let c = config(true);
        let server = Some("server");

        assert!(publish_allowed(&c, server, &format!("{FW}request")));
        assert!(!publish_allowed(&c, server, &format!("{FW}health")));
        assert!(subscribe_allowed(&c, server, &format!("{FW}health")));
        assert!(subscribe_allowed(&c, server, &format!("{FW}reply")));
        assert!(!subscribe_allowed(&c, server, &format!("{FW}request")));
        assert!(!subscribe_allowed(&c, server, &format!("{FW}+")));
        assert!(publish_allowed(&c, server, &format!("{FW}ota/data")));
        assert!(!publish_allowed(&c, server, &format!("{FW}ota/get")));
        assert!(subscribe_allowed(&c, server, &format!("{FW}ota/get")));
        assert!(!subscribe_allowed(&c, server, &format!("{FW}ota/data")));

        // Existing server access is unchanged.
        assert!(subscribe_allowed(&c, server, &format!("{STOCK}+/post")));
        assert!(publish_allowed(&c, server, &format!("{STOCK}service/sub")));

        let device = Some("device");
        for topic in [
            "health", "reply", "request", "ota/get", "ota/data", "+", "#",
        ] {
            let topic = format!("{FW}{topic}");
            assert!(!publish_allowed(&c, device, &topic) && !subscribe_allowed(&c, device, &topic));
        }

        assert!(publish_allowed(&c, device, &format!("{STOCK}event/post")));
        assert!(!publish_allowed(&c, None, &format!("{FW}health")));
    }

    #[test]
    fn the_offer_watcher_reaches_only_the_check_topics() {
        let c = config(true);
        let watcher = Some("firmware-watch");

        for (topic, publish, subscribe) in [
            ("ota/check", true, false),
            ("ota/offered", false, true),
            ("health", false, false),
            ("request", false, false),
            ("ota/get", false, false),
            ("ota/data", false, false),
            ("ota/#", false, false),
        ] {
            let topic = format!("{FW}{topic}");
            assert_eq!(publish_allowed(&c, watcher, &topic), publish, "{topic}");
            assert_eq!(subscribe_allowed(&c, watcher, &topic), subscribe, "{topic}");
        }

        // The transport role cannot use them, and the server answers them.
        let firmware = Some("firmware");
        assert!(!publish_allowed(&c, firmware, &format!("{FW}ota/check")));
        assert!(!subscribe_allowed(
            &c,
            firmware,
            &format!("{FW}ota/offered")
        ));

        let server = Some("server");
        assert!(subscribe_allowed(&c, server, &format!("{FW}ota/check")));
        assert!(publish_allowed(&c, server, &format!("{FW}ota/offered")));

        let c = config(false);
        assert!(!publish_allowed(&c, watcher, &format!("{FW}ota/check")));
    }

    #[test]
    fn without_a_firmware_role_the_namespace_is_closed_to_everyone() {
        let c = config(false);

        for user in ["firmware", "server", "device"] {
            for topic in ["health", "reply", "request", "ota/get", "ota/data"] {
                let topic = format!("{FW}{topic}");
                assert!(!publish_allowed(&c, Some(user), &topic));
                assert!(!subscribe_allowed(&c, Some(user), &topic));
            }
        }
    }
}
