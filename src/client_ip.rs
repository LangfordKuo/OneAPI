use std::net::IpAddr;

use axum::http::{header::HeaderName, HeaderMap};

/// Settings key holding the header name used to resolve the client IP.
pub const SETTING_KEY: &str = "client_ip_header";

/// Resolve the client IP from the configured header.
///
/// An empty or missing header setting, an absent header, or a header without a
/// parseable address all fall back to the TCP peer address. The fallback keeps
/// rate limiting meaningful when a proxy is misconfigured instead of putting
/// every request into an "unknown" bucket.
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
}
