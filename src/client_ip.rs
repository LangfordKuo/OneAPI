use std::net::IpAddr;

use axum::http::{header::HeaderName, HeaderMap};

/// Settings key holding the header name used to resolve the client IP.
pub const SETTING_KEY: &str = "client_ip_header";

/// Header used when no explicit setting is configured.
pub const DEFAULT_HEADER: &str = "x-forwarded-for";

/// Resolve the client IP for a login or API-key authentication attempt.
///
/// An explicitly configured header is authoritative: the administrator opted
/// in, typically because a CDN or reverse proxy in front of the app sets it.
/// If that header is missing or malformed the result is `None`, never the TCP
/// peer, so a shared CDN egress address cannot be rate-limited by mistake.
/// When the setting is empty, `X-Forwarded-For` is used for loopback/private
/// peers, and the TCP peer is the final fallback for direct clients.
pub fn resolve_client_ip(
    headers: &HeaderMap,
    configured_header: &str,
    peer: Option<IpAddr>,
) -> Option<IpAddr> {
    let configured = configured_header.trim();
    if !configured.is_empty() {
        return resolve(headers, configured, peer);
    }
    if peer.is_some_and(is_trusted_proxy_peer) {
        return resolve(headers, DEFAULT_HEADER, peer).or(peer);
    }
    peer
}

/// Resolve the client IP from the configured header.
///
/// Pass `""` to ignore all headers and use the TCP peer. When a header is in
/// effect (explicitly configured, or the default `X-Forwarded-For` for a
/// trusted proxy peer), the header is the only source of truth: a missing or
/// malformed value returns `None` instead of falling back to the connection
/// peer. Falling back would make rate limiting punish the CDN/reverse-proxy
/// egress address shared by every client.
pub fn resolve(headers: &HeaderMap, header: &str, peer: Option<IpAddr>) -> Option<IpAddr> {
    let header = header.trim();
    if header.is_empty() {
        return peer;
    }
    headers
        .get(header)
        .and_then(|value| value.to_str().ok())
        .and_then(first_ip)
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

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(
                HeaderName::from_static(name),
                value.parse().expect("valid header value"),
            );
        }
        headers
    }

    fn ip(value: &str) -> IpAddr {
        value.parse().expect("valid ip address")
    }

    #[tokio::test]
    async fn configured_header_is_authoritative_over_peer() {
        let headers = headers(&[("x-real-ip", "203.0.113.7")]);

        let resolved = resolve_client_ip(&headers, "x-real-ip", Some(ip("10.0.0.1")));

        assert_eq!(resolved, Some(ip("203.0.113.7")));
    }

    #[tokio::test]
    async fn configured_header_missing_or_invalid_never_falls_back_to_peer() {
        let missing = headers(&[("x-forwarded-for", "198.51.100.9")]);
        assert_eq!(
            resolve_client_ip(&missing, "x-real-ip", Some(ip("10.0.0.1"))),
            None,
            "a missing configured header must not fall back to the TCP peer"
        );

        let invalid = headers(&[("x-real-ip", "not-an-ip")]);
        assert_eq!(
            resolve_client_ip(&invalid, "x-real-ip", Some(ip("10.0.0.1"))),
            None,
            "a malformed configured header must not fall back to the TCP peer"
        );
    }

    #[tokio::test]
    async fn empty_setting_uses_forwarded_for_behind_a_loopback_peer() {
        let headers = headers(&[("x-forwarded-for", "198.51.100.9, 10.0.0.2")]);

        let resolved = resolve_client_ip(&headers, "", Some(ip("127.0.0.1")));

        assert_eq!(resolved, Some(ip("198.51.100.9")));
    }

    #[tokio::test]
    async fn empty_setting_and_no_forwarded_for_returns_the_direct_peer() {
        let headers = headers(&[]);

        let resolved = resolve_client_ip(&headers, "", Some(ip("127.0.0.1")));

        assert_eq!(resolved, Some(ip("127.0.0.1")));
    }
}
