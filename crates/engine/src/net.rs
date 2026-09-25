//! Public network destinations for caller-supplied media URLs.
//! Port of network_policy.py + src/main/network-policy.ts: resolve once, reject
//! any non-public answer, then pin the connection to the checked address.
use crate::error::{EngineError, Result};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;
use url::Url;

const LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);

/// Only publicly routable addresses are valid download sources.
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => is_public_v4(v4),
            None => is_public_v6(v6),
        },
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(a == 0
        || a == 10
        || a == 127
        || a >= 224
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && (b == 168 || b == 0 || (b == 88 && c == 99)))
        || (a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
        || (a == 203 && b == 0 && c == 113))
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    let [g0, g1, ..] = ip.segments();
    // Global unicast (2000::/3) minus 6to4, IETF protocol assignments
    // (2001::/23), and documentation (2001:db8::/32, 3fff::/20).
    if !(0x2000..=0x3fff).contains(&g0) || g0 == 0x2002 || (g0 == 0x3fff && g1 < 0x1000) {
        return false;
    }
    if g0 == 0x2001 {
        return g1 >= 0x200 && g1 != 0xdb8;
    }
    true
}

/// A URL whose host was resolved and checked; connect only to `addr`.
#[derive(Debug, Clone)]
pub struct PinnedDestination {
    pub url: Url,
    pub host: String,
    pub addr: SocketAddr,
}

fn invalid() -> EngineError {
    EngineError::msg("video destination is not public")
}

/// Validates scheme, port and host, resolves the host and requires every answer to be public.
pub async fn resolve_public_destination(raw: &str) -> Result<PinnedDestination> {
    if raw.len() > 8192 || raw.chars().any(|c| (c as u32) < 32) {
        return Err(invalid());
    }
    let url = Url::parse(raw).map_err(|_| invalid())?;
    if !matches!(url.scheme(), "http" | "https") || !url.username().is_empty() || url.password().is_some() {
        return Err(invalid());
    }
    if !matches!(url.port(), None | Some(80) | Some(443)) {
        return Err(invalid());
    }
    let port = url.port_or_known_default().ok_or_else(invalid)?;
    let host = match url.host() {
        Some(url::Host::Domain(d)) => d.trim_end_matches('.').to_ascii_lowercase(),
        Some(url::Host::Ipv4(ip)) => ip.to_string(),
        Some(url::Host::Ipv6(ip)) => ip.to_string(),
        None => return Err(invalid()),
    };
    if host == "localhost" || [".local", ".localhost", ".internal"].iter().any(|s| host.ends_with(s)) {
        return Err(invalid());
    }
    let addrs: Vec<SocketAddr> = if let Ok(ip) = host.parse::<IpAddr>() {
        vec![SocketAddr::new(ip, port)]
    } else {
        let lookup = tokio::net::lookup_host((host.as_str(), port));
        tokio::time::timeout(LOOKUP_TIMEOUT, lookup)
            .await
            .map_err(|_| EngineError::msg("source hostname lookup timed out"))?
            .map_err(|_| invalid())?
            .collect()
    };
    if addrs.is_empty() || !addrs.iter().all(|a| is_public_ip(a.ip())) {
        return Err(invalid());
    }
    Ok(PinnedDestination { url, host, addr: addrs[0] })
}

/// Checks the initial source URL before accepting a job.
pub async fn is_public_source_url(raw: &str) -> bool {
    resolve_public_destination(raw).await.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_addresses() {
        for private in ["127.0.0.1", "10.1.2.3", "192.168.0.1", "172.20.0.1", "169.254.1.1", "100.64.0.1", "0.0.0.0", "::1", "fe80::1", "fc00::1", "2001:db8::1", "::ffff:10.0.0.1", "2002::1"] {
            assert!(!is_public_ip(private.parse().unwrap()), "{private}");
        }
        for public in ["8.8.8.8", "1.1.1.1", "2606:4700::1111", "2001:4860::8888", "::ffff:8.8.8.8"] {
            assert!(is_public_ip(public.parse().unwrap()), "{public}");
        }
    }

    #[tokio::test]
    async fn rejects_bad_urls() {
        for url in ["ftp://example.com/a", "http://localhost/a", "http://user:pw@8.8.8.8/", "http://8.8.8.8:8080/", "http://10.0.0.1/", "http://printer.local/"] {
            assert!(resolve_public_destination(url).await.is_err(), "{url}");
        }
        assert!(resolve_public_destination("https://8.8.8.8/video.mp4").await.is_ok());
    }
}
