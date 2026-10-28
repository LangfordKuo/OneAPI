use std::net::IpAddr;

use axum::http::{header::HeaderName, HeaderMap};

/// Settings key holding the header name used to resolve the client IP.
pub const SETTING_KEY: &str = "client_ip_header";

/// Header used when no explicit setting is configured.
pub const DEFAULT_HEADER: &str = "x-forwarded-for";

/// Resolve the client IP for a login attempt.
///
/// An explicitly configured header is trusted as-is: the administrator opted
/// in, typically because a CDN or reverse proxy in front of the app appends it.
/// When the setting is empty, `X-Forwarded-For` is used only for loopback and
/// private peers, so a directly exposed public client cannot spoof it.
pub fn resolve_client_ip(
    headers: &HeaderMap,
    configured_header: &str,
    peer: Option<IpAddr>,
) -> Option<IpAddr> {
    let header = effective_header(configured_header, peer);
    resolve(headers, header.unwrap_or_default(), peer)
}

fn effective_header(configured_header: &str, peer: Option<IpAddr>) -> Option<&str> {
    let configured = configured_header.trim();
    if !configured.is_empty() {
        return Some(configured);
    }
    if peer.is_some_and(is_trusted_proxy_peer) {
        return Some(DEFAULT_HEADER);
    }
    None
}

/// Resolve the client IP from the configured header.
///
/// Pass `""` to ignore all headers. An empty header setting, an absent header,
/// or a header without a parseable address all fall back to the TCP peer
/// address. The fallback keeps rate limiting meaningful when a proxy is
/// misconfigured instead of putting every request into an "unknown" bucket.
pub fn resolve(headers: &HeaderMap, header: &str, peer: Option<IpAddr>) -> Option<IpAddr> {
    let header = header.trim();
    if header.is_empty() {
        return peer;
    }
    headers
        .get(header)
        .and_then(|value| value.to_str().ok())
        .and_then(first_ip)
        .or(peer)
}

/// Whether a TCP peer is likely a local reverse proxy.
///
/// Loopback, private, link-local and CGNAT peers are the normal source for
/// Docker/Kubernetes sidecars, Cloudflare Tunnel and reverse proxies on the
/// same host or network. Header-derived client IPs may be trusted for those
/// peers without a global flag; a direct public client cannot spoof them.
/// IPv4-mapped IPv6 addresses are classified by their IPv4 form.
pub fn is_trusted_proxy_peer(peer: IpAddr) -> bool {
    match peer {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                // 100.64.0.0/10 (CGNAT / shared address space).
                || (v4.octets()[0] == 100 && (64..=127).contains(&v4.octets()[1]))
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_trusted_proxy_peer(IpAddr::V4(v4));
            }
            let octets = v6.octets();
            // fc00::/7 unique-local, fe80::/10 link-local. Written manually to
            // keep the crate MSRV at 1.82.
            v6.is_loopback()
                || (octets[0] & 0xfe) == 0xfc
                || (octets[0] == 0xfe && (octets[1] & 0xc0) == 0x80)
        }
    }
}

/// Validate and normalize a configured header name.
///
/// Returns `Some("")` for an empty setting (use the peer address). Header
/// names are stored lowercase so lookups and display stay canonical.
pub fn parse_header_name(value: &str) -> Option<String> {
    let name = value.trim();
    if name.is_empty() {
        return Some(String::new());
    }
    let name = name.to_ascii_lowercase();
    HeaderName::from_bytes(name.as_bytes()).ok()?;
    Some(name)
}

fn first_ip(value: &str) -> Option<IpAddr> {
    value.split(',').find_map(parse_ip)
}

fn parse_ip(raw: &str) -> Option<IpAddr> {
    let raw = raw.trim().trim_matches('"').trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(ip) = raw.parse::<IpAddr>() {
        return Some(ip);
    }
    if let Some(rest) = raw.strip_prefix('[') {
        let (inside, _) = rest.split_once(']')?;
        return inside.parse().ok();
    }
    if let Some((host, port)) = raw.rsplit_once(':') {
        if port.parse::<u16>().is_ok() {
            return host.parse().ok();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use std::net::Ipv4Addr;

    fn headers(name: &'static str, value: &str) -> HeaderMap {
        let mut map = HeaderMap::new();
        map.insert(name, HeaderValue::from_str(value).expect("header"));
        map
    }

    #[test]
    fn empty_setting_uses_peer_address() {
        let peer = Some(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)));
        let map = headers("cf-connecting-ip", "203.0.113.9");
        assert_eq!(resolve(&map, "", peer), peer);
    }

    #[test]
    fn configured_header_wins_over_peer() {
        let peer = Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)));
        let map = headers("cf-connecting-ip", "203.0.113.9");
        assert_eq!(
            resolve(&map, "cf-connecting-ip", peer),
            Some("203.0.113.9".parse().expect("ip"))
        );
    }

    #[test]
    fn forwarded_for_uses_first_parseable_address() {
        let map = headers("x-forwarded-for", "unknown, 203.0.113.9, 10.0.0.2");
        assert_eq!(
            resolve(&map, "x-forwarded-for", None),
            Some("203.0.113.9".parse().expect("ip"))
        );
    }

    #[test]
    fn parses_ports_and_bracketed_ipv6() {
        assert_eq!(
            parse_ip("203.0.113.9:443"),
            Some("203.0.113.9".parse().expect("ip"))
        );
        assert_eq!(
            parse_ip("[2001:db8::1]:443"),
            Some("2001:db8::1".parse().expect("ip"))
        );
        assert_eq!(
            parse_ip("2001:db8::1"),
            Some("2001:db8::1".parse().expect("ip"))
        );
    }

    #[test]
    fn invalid_header_value_falls_back_to_peer() {
        let peer = Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)));
        let map = headers("x-real-ip", "not-an-ip");
        assert_eq!(resolve(&map, "x-real-ip", peer), peer);
    }

    #[test]
    fn header_name_is_lowercased_and_validated() {
        assert_eq!(
            parse_header_name(" CF-Connecting-IP "),
            Some("cf-connecting-ip".to_string())
        );
        assert_eq!(parse_header_name("   "), Some(String::new()));
        assert_eq!(parse_header_name("bad header"), None);
    }

    #[test]
    fn local_and_private_peers_are_trusted_proxies() {
        for raw in [
            "127.0.0.1",
            "::1",
            "10.1.2.3",
            "172.16.0.1",
            "172.31.255.254",
            "192.168.1.10",
            "169.254.1.1",
            "100.64.0.1",
            "100.127.255.255",
            "fd00::1",
            "fe80::1",
            "::ffff:10.0.0.1",
        ] {
            let ip: IpAddr = raw.parse().expect(raw);
            assert!(is_trusted_proxy_peer(ip), "{raw} should be trusted");
        }
    }

    #[test]
    fn public_peers_are_not_trusted_proxies() {
        for raw in [
            "8.8.8.8",
            "203.0.113.9",
            "172.32.0.1",
            "100.128.0.1",
            "2001:db8::1",
            "::ffff:8.8.8.8",
        ] {
            let ip: IpAddr = raw.parse().expect(raw);
            assert!(!is_trusted_proxy_peer(ip), "{raw} should not be trusted");
        }
    }

    #[test]
    fn configured_header_is_trusted_even_from_a_public_peer() {
        let map = headers("cf-connecting-ip", "203.0.113.9");
        let public: IpAddr = "198.51.100.1".parse().expect("peer");
        assert_eq!(
            resolve_client_ip(&map, "cf-connecting-ip", Some(public)),
            Some("203.0.113.9".parse().expect("client"))
        );
    }

    #[test]
    fn empty_setting_uses_forwarded_for_only_for_private_peers() {
        let map = headers("x-forwarded-for", "203.0.113.9, 10.0.0.2");
        let private: IpAddr = "127.0.0.1".parse().expect("private peer");
        assert_eq!(
            resolve_client_ip(&map, "", Some(private)),
            Some("203.0.113.9".parse().expect("client"))
        );

        let public: IpAddr = "198.51.100.10".parse().expect("public peer");
        assert_eq!(resolve_client_ip(&map, "", Some(public)), Some(public));
    }

    #[test]
    fn empty_setting_without_forwarded_for_falls_back_to_peer() {
        let map = HeaderMap::new();
        let peer: IpAddr = "10.0.0.2".parse().expect("peer");
        assert_eq!(resolve_client_ip(&map, "", Some(peer)), Some(peer));
    }
}
