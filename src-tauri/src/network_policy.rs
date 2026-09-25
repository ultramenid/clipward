//! Only publicly routable destinations are valid download and banner sources.
use crate::security::is_web_url;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

fn is_public_v4(address: Ipv4Addr) -> bool {
    let [a, b, c, _] = address.octets();
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

fn is_public_v6(address: Ipv6Addr) -> bool {
    let groups = address.segments();
    let (g0, g1) = (groups[0], groups[1]);
    // Global unicast (2000::/3) minus 6to4, IETF protocol assignments (2001::/23)
    // and documentation (2001:db8::/32, 3fff::/20). IPv4-mapped and other
    // special forms fall below 0x2000 and are refused.
    if !(0x2000..=0x3fff).contains(&g0) || g0 == 0x2002 || (g0 == 0x3fff && g1 < 0x1000) {
        return false;
    }
    if g0 == 0x2001 {
        return g1 >= 0x200 && g1 != 0xdb8;
    }
    true
}

pub fn is_public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => is_public_v6(v6),
    }
}

const LOCAL_ERROR: &str = "Local network sources are not allowed";

pub async fn assert_public_web_url(value: &str) -> Result<(), String> {
    if !is_web_url(value) {
        return Err("A public HTTP(S) URL is required".into());
    }
    let url = url::Url::parse(value).map_err(|_| "A public HTTP(S) URL is required".to_string())?;
    let hostname = match url.host() {
        Some(url::Host::Ipv4(v4)) => {
            return if is_public_v4(v4) {
                Ok(())
            } else {
                Err(LOCAL_ERROR.into())
            }
        }
        Some(url::Host::Ipv6(v6)) => {
            return if is_public_v6(v6) {
                Ok(())
            } else {
                Err(LOCAL_ERROR.into())
            }
        }
        Some(url::Host::Domain(domain)) => domain.to_lowercase().trim_end_matches('.').to_string(),
        None => return Err("A public HTTP(S) URL is required".into()),
    };
    if hostname == "localhost" || hostname.ends_with(".localhost") || hostname.ends_with(".local") {
        return Err(LOCAL_ERROR.into());
    }
    let lookup = tokio::net::lookup_host((hostname.as_str(), 443));
    let addresses: Vec<IpAddr> = match tokio::time::timeout(Duration::from_secs(5), lookup).await {
        Err(_) => return Err("Source hostname lookup timed out".into()),
        Ok(Err(_)) => return Err("Source must resolve to a public network address".into()),
        Ok(Ok(found)) => found.map(|socket| socket.ip()).collect(),
    };
    if addresses.is_empty() || addresses.iter().any(|address| !is_public_address(*address)) {
        return Err("Source must resolve to a public network address".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_addresses() {
        for private in [
            "127.0.0.1",
            "10.1.2.3",
            "192.168.1.1",
            "169.254.1.1",
            "100.64.0.1",
            "::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "2001:db8::1",
            "fc00::1",
        ] {
            assert!(!is_public_address(private.parse().unwrap()), "{private}");
        }
        for public in ["8.8.8.8", "1.1.1.1", "2606:4700::1111", "2001:4860::8888"] {
            assert!(is_public_address(public.parse().unwrap()), "{public}");
        }
    }

    #[tokio::test]
    async fn refuses_local_hosts() {
        assert!(assert_public_web_url("http://localhost:8080/x")
            .await
            .is_err());
        assert!(assert_public_web_url("http://printer.local/x")
            .await
            .is_err());
        assert!(assert_public_web_url("http://127.0.0.1/x").await.is_err());
        assert!(assert_public_web_url("http://[::1]/x").await.is_err());
        assert!(assert_public_web_url("http://0x7f.1/x").await.is_err());
        assert!(assert_public_web_url("ftp://example.com").await.is_err());
    }
}
