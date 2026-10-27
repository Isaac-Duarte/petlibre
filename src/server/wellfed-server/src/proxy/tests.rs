use axum::http::HeaderValue;

use super::*;

fn ip(text: &str) -> IpAddr {
    text.parse().unwrap()
}

fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (name, value) in pairs {
        map.append(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }
    map
}

fn proxies(ranges: &[&str]) -> TrustedProxies {
    TrustedProxies::new(ranges.iter().map(|range| range.parse().unwrap()))
}

#[test]
fn a_range_is_an_address_or_cidr() {
    let lan: ProxyRange = "10.0.0.0/8".parse().unwrap();
    assert!(lan.contains(ip("10.200.3.4")));
    assert!(!lan.contains(ip("11.0.0.1")));
    assert!(
        lan.contains(ip("::ffff:10.0.0.1")),
        "an IPv4-mapped address is its IPv4 one"
    );

    let one: ProxyRange = "127.0.0.1".parse().unwrap();
    assert!(one.contains(ip("127.0.0.1")));
    assert!(!one.contains(ip("127.0.0.2")));

    let ula: ProxyRange = "fd00::/8".parse().unwrap();
    assert!(ula.contains(ip("fd12:3456::1")));
    assert!(!ula.contains(ip("fe80::1")));
    assert!(!ula.contains(ip("10.0.0.1")));

    let everything: ProxyRange = "0.0.0.0/0".parse().unwrap();
    assert!(everything.contains(ip("203.0.113.9")));
    let all_v6: ProxyRange = "::/0".parse().unwrap();
    assert!(all_v6.contains(ip("2001:db8::1")));

    for bad in [
        "",
        "10.0.0.0/33",
        "::/129",
        "10.0.0/8",
        "host.lan",
        "10.0.0.0/x",
        "1.2.3.4/",
    ] {
        assert!(bad.parse::<ProxyRange>().is_err(), "{bad:?}");
    }
}

#[test]
fn an_untrusted_peers_forwarded_headers_are_ignored() {
    let trusted = proxies(&["127.0.0.1"]);
    let forged = headers(&[
        ("x-forwarded-for", "198.51.100.1"),
        ("x-forwarded-proto", "https"),
        ("forwarded", "for=198.51.100.2;proto=https"),
    ]);

    assert_eq!(
        trusted.client(ip("192.0.2.7"), &forged),
        (ip("192.0.2.7"), Scheme::Http)
    );
    assert_eq!(
        TrustedProxies::default().client(ip("127.0.0.1"), &forged),
        (ip("127.0.0.1"), Scheme::Http)
    );
}

#[test]
fn behind_a_trusted_proxy_the_client_is_the_first_untrusted_hop_from_the_right() {
    let trusted = proxies(&["127.0.0.1", "10.0.0.0/8"]);
    let peer = ip("127.0.0.1");

    let one = headers(&[
        ("x-forwarded-for", "203.0.113.9"),
        ("x-forwarded-proto", "https"),
    ]);
    assert_eq!(
        trusted.client(peer, &one),
        (ip("203.0.113.9"), Scheme::Https)
    );

    let forged_left = headers(&[("x-forwarded-for", "1.1.1.1, 203.0.113.9, 10.1.2.3")]);
    assert_eq!(trusted.client(peer, &forged_left).0, ip("203.0.113.9"));

    let lines = headers(&[
        ("x-forwarded-for", "1.1.1.1"),
        ("x-forwarded-for", "203.0.113.9:4711"),
    ]);
    assert_eq!(trusted.client(peer, &lines).0, ip("203.0.113.9"));

    let all_proxies = headers(&[("x-forwarded-for", "10.0.0.2, 10.0.0.1")]);
    assert_eq!(trusted.client(peer, &all_proxies).0, ip("10.0.0.2"));

    let unknown = headers(&[("x-forwarded-for", "unknown")]);
    assert_eq!(trusted.client(peer, &unknown).0, peer);

    assert_eq!(trusted.client(peer, &headers(&[])), (peer, Scheme::Http));
}

#[test]
fn forwarded_is_read_when_x_forwarded_for_is_absent() {
    let trusted = proxies(&["127.0.0.1"]);
    let peer = ip("127.0.0.1");

    let forwarded = headers(&[(
        "forwarded",
        r#"for=198.51.100.1, for="[2001:db8::7]:4711";proto=https;by=127.0.0.1"#,
    )]);
    assert_eq!(
        trusted.client(peer, &forwarded),
        (ip("2001:db8::7"), Scheme::Https)
    );

    let both = headers(&[
        ("forwarded", "for=198.51.100.1"),
        ("x-forwarded-for", "203.0.113.9"),
    ]);
    assert_eq!(trusted.client(peer, &both).0, ip("203.0.113.9"));
}

#[test]
fn the_scheme_is_the_last_proto_given() {
    let trusted = proxies(&["127.0.0.1"]);
    let peer = ip("127.0.0.1");
    let scheme = |pairs| trusted.client(peer, &headers(pairs)).1;

    assert_eq!(scheme(&[("x-forwarded-proto", "HTTPS")]), Scheme::Https);
    assert_eq!(
        scheme(&[("x-forwarded-proto", "https, http")]),
        Scheme::Http
    );
    assert_eq!(scheme(&[("x-forwarded-proto", "ftp")]), Scheme::Http);
    assert_eq!(
        scheme(&[(
            "forwarded",
            "for=1.2.3.4;proto=http, for=5.6.7.8;proto=https"
        )]),
        Scheme::Https
    );
}

/// A forwarded line that is not text still counts as sent, and each of its
/// entries is read alone: the proxy's clean entry on the right is the
/// client, never a `Forwarded` header the client wrote.
#[test]
fn a_line_that_is_not_text_never_hands_over_to_forwarded() {
    let trusted = proxies(&["127.0.0.1"]);
    let peer = ip("127.0.0.1");
    let mut garbled = headers(&[("forwarded", "for=198.51.100.1;proto=https")]);
    garbled.append(
        X_FORWARDED_FOR,
        HeaderValue::from_bytes(b"\xff\xfe, 203.0.113.9").unwrap(),
    );

    assert_eq!(trusted.client(peer, &garbled).0, ip("203.0.113.9"));

    let mut only_garbage = headers(&[("forwarded", "for=198.51.100.1")]);
    only_garbage.append(X_FORWARDED_FOR, HeaderValue::from_bytes(b"\xff").unwrap());
    assert_eq!(
        trusted.client(peer, &only_garbage).0,
        peer,
        "an entry that names nobody leaves the proxy"
    );
}

/// `X-Forwarded-Proto` sent at all is the scheme's source, garbled or not.
#[test]
fn a_garbled_proto_is_plain_http_not_forwardeds() {
    let trusted = proxies(&["127.0.0.1"]);
    let mut garbled = headers(&[("forwarded", "for=198.51.100.1;proto=https")]);
    garbled.append(X_FORWARDED_PROTO, HeaderValue::from_bytes(b"\xff").unwrap());

    assert_eq!(trusted.client(ip("127.0.0.1"), &garbled).1, Scheme::Http);
}

#[test]
fn a_mapped_range_is_its_ipv4_range() {
    let mapped: ProxyRange = "::ffff:10.0.0.0/104".parse().unwrap();

    assert_eq!(mapped, "10.0.0.0/8".parse().unwrap());
    assert!(mapped.contains(ip("10.1.2.3")));
    assert!(mapped.contains(ip("::ffff:10.1.2.3")));
}
