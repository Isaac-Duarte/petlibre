//! The DNS-first route from stock to the installer ([consumer installation]).
//!
//! Stock keeps its broker as a DNS name and a port in its settings sector,
//! resolves the name at every connect, and speaks plaintext MQTT there with
//! no TLS and no pinning (static analysis of 1.1.15, with a flash read and a
//! UART capture). So when the owner's own resolver answers that name with
//! the installer's address, a restarted stock connects to the installer's
//! broker with its own credentials, and nothing is written to the feeder: no
//! setup mode, no Reset hold, and stock's plans stay in their sectors for
//! the import.
//!
//! The name is the vendor's and is never built into Wellfed: the caller
//! takes it from the owner (or, on the bench, from a flash read of stock's
//! settings). Only the address changes, so the installer's broker must
//! listen on the port stock already stores.
//!
//! [consumer installation]: ../../../../../docs/firmware/consumer-installation.md

use core::net::{IpAddr, SocketAddr};

/// The port stock stores beside its broker name and connects to: plaintext
/// MQTT (static analysis; the research unit's settings sector holds it).
pub const STOCK_BROKER_PORT: u16 = 1883;

/// The longest DNS name, in its dotted form.
const NAME_MAX: usize = 253;
/// The longest label between dots.
const LABEL_MAX: usize = 63;

/// Why a DNS override cannot move stock.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DnsError {
    #[error(
        "{name:?} is not a DNS name: letters, digits and hyphens in dot-separated labels of 1-{LABEL_MAX}, at most {NAME_MAX} in all"
    )]
    Name { name: String },
    #[error(
        "the installer's broker listens on port {port}, but stock keeps port {STOCK_BROKER_PORT}: an override changes only the address"
    )]
    Port { port: u16 },
}

/// Stock's broker name, and the installer's broker the owner's resolver
/// must answer it with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DnsOverride {
    name: String,
    broker: IpAddr,
}

/// What a resolver answered for stock's broker name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DnsCheck {
    /// Among its answers is the installer's broker.
    Points,
    /// It answers with other addresses only: the override is not in place
    /// for that resolver.
    Elsewhere(Vec<IpAddr>),
    /// It has no answer for the name.
    Unresolved,
}

impl DnsOverride {
    /// Point stock's broker `name` at `broker`, which must be on stock's
    /// port.
    pub fn new(name: &str, broker: SocketAddr) -> Result<Self, DnsError> {
        let name = name.strip_suffix('.').unwrap_or(name).to_ascii_lowercase();
        if !valid_name(&name) {
            return Err(DnsError::Name { name });
        }
        if broker.port() != STOCK_BROKER_PORT {
            return Err(DnsError::Port {
                port: broker.port(),
            });
        }
        Ok(Self {
            name,
            broker: broker.ip(),
        })
    }

    /// Stock's broker name, lowercase.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The address the owner's resolver must answer it with.
    pub fn broker(&self) -> IpAddr {
        self.broker
    }

    /// Judge what a resolver answered for the name. Only advice: the
    /// installer's host may ask another resolver than the feeder does (the
    /// bench's host does), so stock reaching the broker is the real check.
    pub fn check(&self, resolved: &[IpAddr]) -> DnsCheck {
        if resolved.contains(&self.broker) {
            DnsCheck::Points
        } else if resolved.is_empty() {
            DnsCheck::Unresolved
        } else {
            DnsCheck::Elsewhere(resolved.to_vec())
        }
    }
}

/// A dotted name of letters, digits and inner hyphens, with at least two
/// labels: stock looks up a public name, never a bare host.
fn valid_name(name: &str) -> bool {
    let label = |label: &str| {
        (1..=LABEL_MAX).contains(&label.len())
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    };
    name.len() <= NAME_MAX && name.contains('.') && name.split('.').all(label)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn broker() -> SocketAddr {
        "10.42.103.1:1883".parse().unwrap()
    }

    #[test]
    fn a_name_on_stock_s_port_is_taken_lowercase() {
        let dns = DnsOverride::new("Broker.Example.NET.", broker()).unwrap();
        assert_eq!(dns.name(), "broker.example.net");
        assert_eq!(dns.broker(), broker().ip());
    }

    #[test]
    fn what_stock_could_not_look_up_or_reach_is_refused() {
        for name in ["", "localhost", "a..b", "-a.b", "a-.b", "a b.c", "a_b.c"] {
            assert!(
                matches!(DnsOverride::new(name, broker()), Err(DnsError::Name { .. })),
                "{name:?}"
            );
        }
        let long = format!("{}.net", "a".repeat(64));
        assert!(DnsOverride::new(&long, broker()).is_err());
        assert_eq!(
            DnsOverride::new("broker.example.net", "10.42.103.1:18883".parse().unwrap()),
            Err(DnsError::Port { port: 18883 })
        );
    }

    #[test]
    fn the_check_needs_the_broker_among_the_answers() {
        let dns = DnsOverride::new("broker.example.net", broker()).unwrap();
        let other: IpAddr = "192.0.2.7".parse().unwrap();
        assert_eq!(dns.check(&[other, broker().ip()]), DnsCheck::Points);
        assert_eq!(dns.check(&[other]), DnsCheck::Elsewhere(vec![other]));
        assert_eq!(dns.check(&[]), DnsCheck::Unresolved);
    }
}
