//! Strict base URL and authority validation for outbound HTTP model endpoints.

use crate::port::ProviderError;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Rejects malformed URLs, embedded userinfo (`@`), query/fragments, and
/// unspecified / link-local / cloud-metadata SSRF targets while permitting
/// explicit loopback endpoints (`127.0.0.1`, `::1`, `localhost`) for local workers.
pub(crate) fn validate_base_url(raw: &str) -> Result<(), ProviderError> {
    if raw.bytes().any(|b| b <= b' ' || b == 0x7f) {
        return Err(ProviderError::InvalidConfig(
            "base_url must not contain whitespace or control characters".into(),
        ));
    }
    let rest = raw
        .strip_prefix("https://")
        .or_else(|| raw.strip_prefix("http://"))
        .ok_or_else(|| {
            ProviderError::InvalidConfig("base_url must use http:// or https://".into())
        })?;
    if rest.contains('?') || rest.contains('#') || rest.contains('\\') {
        return Err(ProviderError::InvalidConfig(
            "base_url must not contain query, fragment, or backslash characters".into(),
        ));
    }
    let authority = rest.split('/').next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return Err(ProviderError::InvalidConfig(
            "base_url must have a valid host without userinfo".into(),
        ));
    }

    let host = parse_authority_host(authority)?;
    validate_host(host)
}

fn parse_authority_host(authority: &str) -> Result<&str, ProviderError> {
    if let Some(after_open) = authority.strip_prefix('[') {
        let Some((ipv6_str, tail)) = after_open.split_once(']') else {
            return Err(ProviderError::InvalidConfig(
                "base_url has unclosed IPv6 bracket".into(),
            ));
        };
        if !tail.is_empty() {
            let port_str = tail.strip_prefix(':').ok_or_else(|| {
                ProviderError::InvalidConfig("base_url has invalid port after IPv6 host".into())
            })?;
            parse_port(port_str)?;
        }
        ipv6_str.parse::<Ipv6Addr>().map_err(|_| {
            ProviderError::InvalidConfig("base_url contains invalid IPv6 address".into())
        })?;
        return Ok(ipv6_str);
    }

    if authority.contains('[') || authority.contains(']') {
        return Err(ProviderError::InvalidConfig(
            "base_url contains unexpected IPv6 bracket".into(),
        ));
    }
    let (host, port_opt) = match authority.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (authority, None),
    };
    if let Some(port_str) = port_opt {
        parse_port(port_str)?;
    }
    if host.is_empty() {
        return Err(ProviderError::InvalidConfig(
            "base_url host must be non-empty".into(),
        ));
    }
    Ok(host)
}

fn parse_port(port_str: &str) -> Result<u16, ProviderError> {
    let port: u16 = port_str
        .parse()
        .map_err(|_| ProviderError::InvalidConfig("base_url port is invalid".into()))?;
    if port == 0 {
        return Err(ProviderError::InvalidConfig(
            "base_url port must be non-zero".into(),
        ));
    }
    Ok(port)
}

fn validate_host(host: &str) -> Result<(), ProviderError> {
    if let Ok(v6) = host.parse::<Ipv6Addr>() {
        return check_ip(IpAddr::V6(v6));
    }
    if let Ok(v4) = host.parse::<Ipv4Addr>() {
        return check_ip(IpAddr::V4(v4));
    }

    // Reject single-label numeric/hex/octal obfuscated IP representations (e.g. 2852039166, 0x7f000001).
    let lower = host.to_ascii_lowercase();
    if lower.bytes().all(|b| b.is_ascii_digit())
        || lower.starts_with("0x")
        || has_leading_zero_numeric_labels(&lower)
    {
        return Err(ProviderError::InvalidConfig(
            "base_url must not use obfuscated numeric host literals".into(),
        ));
    }

    if matches!(
        lower.as_str(),
        "metadata" | "metadata.google.internal" | "metadata.goog" | "instance-data"
    ) {
        return Err(ProviderError::InvalidConfig(
            "base_url points to a disallowed metadata hostname".into(),
        ));
    }

    if host.starts_with('-')
        || host.ends_with('-')
        || host.contains("..")
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
    {
        return Err(ProviderError::InvalidConfig(
            "base_url host contains invalid characters".into(),
        ));
    }
    Ok(())
}

fn has_leading_zero_numeric_labels(lower: &str) -> bool {
    let mut saw_label = false;
    let mut all_numeric_labels = true;
    let mut any_leading_zero = false;
    for label in lower.split('.') {
        if label.is_empty() {
            return true;
        }
        saw_label = true;
        let numeric = label.bytes().all(|b| b.is_ascii_digit())
            || label.starts_with("0x") && label[2..].bytes().all(|b| b.is_ascii_hexdigit());
        if !numeric {
            all_numeric_labels = false;
            break;
        }
        if label.len() > 1 && label.starts_with('0') {
            any_leading_zero = true;
        }
    }
    saw_label && all_numeric_labels && any_leading_zero
}

fn check_ip(ip: IpAddr) -> Result<(), ProviderError> {
    if is_forbidden_ip(ip) {
        return Err(ProviderError::InvalidConfig(
            "base_url targets an unspecified, link-local, or metadata IP address".into(),
        ));
    }
    Ok(())
}

fn is_forbidden_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_unspecified() || v4.is_link_local() || v4.is_broadcast() || v4.is_multicast()
        }
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_forbidden_ip(IpAddr::V4(mapped));
            }
            v6.is_unspecified() || v6.is_multicast() || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}
