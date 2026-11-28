use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use once_cell::sync::Lazy;
use regex::Regex;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::redirect::Policy;
use tokio::net::lookup_host;
use url::Url;

use crate::errors::{classify_extractor_error, AppError};

pub const DOMAINS: &[(&str, &[&str])] = &[
    ("youtube", &["youtube.com", "youtu.be"]),
    ("tiktok", &["tiktok.com"]),
    ("instagram", &["instagram.com"]),
    ("facebook", &["facebook.com", "fb.watch", "fb.com"]),
    ("x", &["x.com", "twitter.com", "t.co"]),
    ("pinterest", &["pinterest.com", "pin.it"]),
    ("threads", &["threads.net", "threads.com"]),
];

const SHORT_DOMAINS: &[&str] = &["youtu.be", "fb.watch", "fb.com", "t.co", "pin.it"];
pub const RESOLVED_ALIASES: &[&str] = &[
    "fb.watch",
    "fb.com",
    "pin.it",
    "t.co",
    "vt.tiktok.com",
    "vm.tiktok.com",
];

const THUMBNAIL_HOSTS: &[&str] = &[
    "ytimg.com",
    "ggpht.com",
    "fbcdn.net",
    "cdninstagram.com",
    "twimg.com",
    "pinimg.com",
    "tiktokcdn.com",
    "tiktokcdn-us.com",
    "tiktokcdn-eu.com",
    "tiktokcdn-in.com",
    "tiktokcdn.net",
    "muscdn.com",
    "tiktok.com",
    "ibyteimg.com",
];

const VIDEO_CDN_HOSTS: &[(&str, &[&str])] = &[
    ("instagram", &["fbcdn.net", "cdninstagram.com", "akamaihd.net"]),
    ("threads", &["fbcdn.net", "cdninstagram.com", "akamaihd.net"]),
    ("facebook", &["fbcdn.net", "akamaihd.net"]),
    ("x", &["twimg.com"]),
    ("pinterest", &["pinimg.com"]),
    (
        "tiktok",
        &[
            "tiktokcdn.com",
            "tiktokcdn-us.com",
            "tiktokcdn-eu.com",
            "tiktokcdn-in.com",
            "tiktokcdn.net",
            "tiktokv.com",
            "tiktok.com",
            "byteoversea.com",
            "byteicdn.com",
            "muscdn.com",
            "ibytedtos.com",
        ],
    ),
    ("youtube", &["googlevideo.com"]),
];

/// DNS resolver that validates every resolved IP is strictly public and pins the
/// validated SocketAddrs directly into reqwest's connector, preventing DNS rebinding/TOCTOU.
struct PublicOnlyResolver;

impl Resolve for PublicOnlyResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = lookup_host((host.as_str(), 0))
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?
                .collect();
            if addrs.is_empty() || !addrs.iter().all(|a| is_public_ip(a.ip())) {
                return Err(Box::new(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "blocked non-public IP address",
                )) as Box<dyn std::error::Error + Send + Sync>);
            }
            let iter: Addrs = Box::new(addrs.into_iter());
            Ok(iter)
        })
    }
}

pub static SHARED_HTTP: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .no_proxy()
        .dns_resolver(Arc::new(PublicOnlyResolver))
        .redirect(Policy::none())
        .timeout(Duration::from_secs(15))
        .pool_idle_timeout(Duration::from_secs(60))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")
        .build()
        .expect("failed to build shared reqwest client")
});

static HOST_LABEL: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$").unwrap());
static QUERY_VAL: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[A-Za-z0-9_.-]{1,120}$").unwrap());

fn host_matches(host: &str, domain: &str) -> bool {
    if host == domain {
        return true;
    }
    !SHORT_DOMAINS.contains(&domain) && host.ends_with(&format!(".{domain}"))
}

fn cdn_host_matches(host: &str, domain: &str) -> bool {
    host == domain || host.ends_with(&format!(".{domain}"))
}

pub fn validate_url(raw: &str) -> Result<(String, String), AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.len() > 2048
        || trimmed.contains('\\')
        || trimmed.chars().any(|c| c.is_whitespace() || (c as u32) < 32 || (c as u32) == 127)
    {
        return Err(AppError::new("INVALID_URL"));
    }

    let parsed = Url::parse(trimmed).map_err(|_| AppError::new("INVALID_URL"))?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(AppError::new("INVALID_URL"));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(AppError::new("INVALID_URL"));
    }
    let expected_port = if scheme == "https" { 443 } else { 80 };
    if let Some(port) = parsed.port() {
        if port != expected_port {
            return Err(AppError::new("INVALID_URL"));
        }
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| AppError::new("INVALID_URL"))?
        .to_lowercase();
    if !host.split('.').all(|label| HOST_LABEL.is_match(label)) {
        return Err(AppError::new("INVALID_URL"));
    }

    let platform = DOMAINS
        .iter()
        .find_map(|(name, list)| {
            list.iter()
                .any(|domain| host_matches(&host, domain))
                .then_some((*name).to_string())
        })
        .ok_or_else(|| AppError::new("UNSUPPORTED_PLATFORM"))?;

    if platform == "youtube" {
        let p = parsed.path();
        if p.starts_with("/playlist")
            || p.starts_with("/channel/")
            || p.starts_with("/user/")
            || p.starts_with("/c/")
            || p.starts_with("/@")
        {
            return Err(AppError::new("UNAVAILABLE_MEDIA"));
        }
    }

    let allowed_keys: &[&str] = match platform.as_str() {
        "youtube" => &["v"],
        "facebook" => &["v", "id", "story_fbid", "video_id", "fbid", "set"],
        _ => &[],
    };

    let mut clean = Url::parse(&format!("{scheme}://{host}")).map_err(|_| AppError::new("INVALID_URL"))?;
    clean.set_path(if parsed.path().is_empty() { "/" } else { parsed.path() });
    let kept: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(k, v)| {
            allowed_keys.contains(&k.as_ref()) && QUERY_VAL.is_match(v) && !v.contains("..")
        })
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if !kept.is_empty() {
        let mut pairs = clean.query_pairs_mut();
        for (k, v) in kept {
            pairs.append_pair(&k, &v);
        }
    }
    clean.set_fragment(None);
    Ok((clean.to_string(), platform))
}

pub fn validate_image_url(raw: Option<&str>) -> Option<String> {
    let url = raw?.trim();
    if url.is_empty()
        || url.len() > 2048
        || url.contains('\\')
        || url.chars().any(|c| c.is_whitespace() || (c as u32) < 32 || (c as u32) == 127)
    {
        return None;
    }
    let parsed = Url::parse(url).ok()?;
    if parsed.scheme() != "https" || !parsed.username().is_empty() || parsed.password().is_some() {
        return None;
    }
    if let Some(p) = parsed.port() {
        if p != 443 {
            return None;
        }
    }
    if parsed.fragment().is_some() {
        return None;
    }
    let host = parsed.host_str()?.to_lowercase();
    if !host.split('.').all(|l| HOST_LABEL.is_match(l)) {
        return None;
    }
    if !THUMBNAIL_HOSTS.iter().any(|d| cdn_host_matches(&host, d)) {
        return None;
    }
    Some(parsed.to_string())
}

pub fn validate_video_cdn_url(raw: &str, platform: &str) -> Option<String> {
    let url = raw.trim();
    if url.is_empty()
        || url.len() > 4096
        || url.contains('\\')
        || url
            .chars()
            .any(|c| c.is_whitespace() || (c as u32) < 32 || (c as u32) == 127)
    {
        return None;
    }
    let parsed = Url::parse(url).ok()?;
    if parsed.scheme() != "https" || !parsed.username().is_empty() || parsed.password().is_some() {
        return None;
    }
    if let Some(p) = parsed.port() {
        if p != 443 {
            return None;
        }
    }
    if parsed.fragment().is_some() {
        return None;
    }
    let host = parsed.host_str()?.to_lowercase();
    if !host.split('.').all(|l| HOST_LABEL.is_match(l)) {
        return None;
    }
    let allowed = VIDEO_CDN_HOSTS
        .iter()
        .find_map(|(p, hosts)| (*p == platform).then_some(*hosts))?;
    if !allowed.iter().any(|d| cdn_host_matches(&host, d)) {
        return None;
    }
    Some(parsed.to_string())
}

pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_public_v4(mapped);
            }
            is_public_v6(v6)
        }
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    !(o[0] == 0 // "this network" 0.0.0.0/8
        || ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_unspecified()
        || ip.is_documentation()
        || (o[0] == 100 && (o[1] & 0b1100_0000) == 64) // CGNAT 100.64.0.0/10
        || (o[0] == 192 && o[1] == 0 && o[2] == 0) // IETF Protocol Assignments
        || (o[0] == 192 && o[1] == 88 && o[2] == 99) // 6to4 Relay Anycast
        || (o[0] == 198 && (o[1] == 18 || o[1] == 19)) // Benchmarking 198.18.0.0/15
        || o[0] >= 240)
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    let seg = ip.segments();
    // Strict allowlist: only Global Unicast 2000::/3, minus special-purpose sub-ranges.
    if (seg[0] & 0xe000) != 0x2000 {
        return false;
    }
    !(seg[0] == 0x2002 // 6to4
        || (seg[0] == 0x2001 && seg[1] == 0x0000) // Teredo 2001::/32
        || (seg[0] == 0x2001 && seg[1] == 0x0002 && seg[2] == 0x0000) // Benchmarking 2001:2::/48
        || (seg[0] == 0x2001 && ((seg[1] & 0xfff0) == 0x0010 || (seg[1] & 0xfff0) == 0x0020)) // ORCHID / ORCHIDv2
        || (seg[0] == 0x2001 && seg[1] == 0x0db8)) // Documentation 2001:db8::/32
}

pub async fn ensure_public_host(url_str: &str) -> Result<(), AppError> {
    let parsed = Url::parse(url_str).map_err(|_| AppError::new("INVALID_URL"))?;
    let host = parsed.host_str().ok_or_else(|| AppError::new("INVALID_URL"))?;
    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| AppError::new("INVALID_URL"))?;
    if port != 80 && port != 443 {
        return Err(AppError::new("FORBIDDEN"));
    }
    let addrs: Vec<_> = lookup_host((host, port))
        .await
        .map_err(|_| AppError::new("NETWORK_ERROR"))?
        .collect();
    if addrs.is_empty() || !addrs.iter().all(|a| is_public_ip(a.ip())) {
        return Err(AppError::new("FORBIDDEN"));
    }
    Ok(())
}

fn needs_alias_resolution(url_str: &str, platform: &str) -> bool {
    let Ok(parsed) = Url::parse(url_str) else {
        return false;
    };
    let host = parsed.host_str().unwrap_or("").to_lowercase();
    if RESOLVED_ALIASES.contains(&host.as_str()) {
        return true;
    }
    let path = parsed.path().to_lowercase();
    match platform {
        "threads" => path.starts_with("/share/") || path.starts_with("/t/"),
        "instagram" | "facebook" => path.starts_with("/share/"),
        _ => false,
    }
}

pub async fn resolve_alias(url: &str, platform: &str) -> Result<String, AppError> {
    let mut current = url.to_string();
    if !needs_alias_resolution(&current, platform) {
        return Ok(current);
    }

    for _ in 0..5 {
        ensure_public_host(&current).await?;
        let resp = SHARED_HTTP
            .get(&current)
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            )
            .header("Sec-Fetch-Mode", "navigate")
            .header("Sec-Fetch-Dest", "document")
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|_| AppError::new("NETWORK_ERROR"))?;
        let status = resp.status().as_u16();
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            let loc = resp
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| AppError::new("UNAVAILABLE_MEDIA"))?;
            let base = Url::parse(&current).map_err(|_| AppError::new("INVALID_URL"))?;
            let joined = base.join(loc).map_err(|_| AppError::new("INVALID_URL"))?;
            let (next_url, next_platform) = validate_url(joined.as_str())?;
            if next_platform != platform {
                return Err(AppError::new("UNSUPPORTED_PLATFORM"));
            }
            current = next_url;
            if !needs_alias_resolution(&current, platform) {
                return Ok(current);
            }
        } else if (200..300).contains(&status) {
            if needs_alias_resolution(&current, platform) {
                if platform == "threads" {
                    return Err(AppError::new("UNAVAILABLE_MEDIA"));
                }
                return Err(AppError::new("UNSUPPORTED_PLATFORM"));
            }
            return Ok(current);
        } else {
            return Err(classify_extractor_error(&format!("HTTP error {status}")));
        }
    }
    Err(AppError::new("UNAVAILABLE_MEDIA"))
}

pub fn clean_text(raw: Option<&str>, max_len: usize) -> Option<String> {
    let text = raw?;
    let cleaned: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        None
    } else {
        Some(collapsed.chars().take(max_len).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_and_sanitizes_urls() {
        assert_eq!(
            validate_url("https://www.YouTube.com/watch?v=demo&token=secret#frag").unwrap(),
            ("https://www.youtube.com/watch?v=demo".into(), "youtube".into())
        );
        assert_eq!(
            validate_url("https://vt.tiktok.com/ZSsrDaAbS/").unwrap().1,
            "tiktok"
        );
        assert_eq!(
            validate_url("https://www.threads.com/share/MqtvI-pQu").unwrap().1,
            "threads"
        );
        assert!(needs_alias_resolution("https://www.threads.com/share/MqtvI-pQu", "threads"));
        assert!(!needs_alias_resolution("https://www.threads.com/@user/post/DgMElZ0E8ej", "threads"));
        assert!(needs_alias_resolution("https://www.instagram.com/share/reel/abc123/", "instagram"));
        assert!(needs_alias_resolution("https://www.facebook.com/share/r/abc123/", "facebook"));
        assert_eq!(
            validate_url("https://www.facebook.com/photo/?fbid=1697460148416021&set=pcb.123&__cft__=drop").unwrap(),
            ("https://www.facebook.com/photo/?fbid=1697460148416021&set=pcb.123".into(), "facebook".into())
        );
        assert_eq!(
            validate_url("https://www.reddit.com/r/pics/comments/1/demo/").unwrap_err().code,
            "UNSUPPORTED_PLATFORM"
        );
        assert_eq!(
            validate_url("https://vimeo.com/76979871").unwrap_err().code,
            "UNSUPPORTED_PLATFORM"
        );
        assert!(validate_url("http://127.0.0.1/watch?v=1").is_err());
        assert!(validate_url("https://youtube.com.evil.tld/watch?v=1").is_err());
        assert!(validate_url("https://www.youtube.com/playlist?list=123").is_err());
    }

    #[test]
    fn blocks_private_ips_and_bad_image_hosts() {
        assert!(!is_public_ip("127.0.0.1".parse().unwrap()));
        assert!(!is_public_ip("0.0.0.1".parse().unwrap()));
        assert!(!is_public_ip("192.88.99.1".parse().unwrap()));
        assert!(!is_public_ip("10.0.0.5".parse().unwrap()));
        assert!(!is_public_ip("169.254.169.254".parse().unwrap()));
        assert!(!is_public_ip("::1".parse().unwrap()));
        assert!(!is_public_ip("::ffff:127.0.0.1".parse().unwrap()));
        assert!(!is_public_ip("fec0::1".parse().unwrap()));
        assert!(!is_public_ip("100::1".parse().unwrap()));
        assert!(!is_public_ip("2001:2::1".parse().unwrap()));
        assert!(is_public_ip("1.1.1.1".parse().unwrap()));
        assert!(is_public_ip("2606:4700:4700::1111".parse().unwrap()));

        assert!(validate_video_cdn_url("https://video.twimg.com/ext_tw_video/1/vid.mp4", "x").is_some());
        assert!(validate_video_cdn_url("https://scontent.cdninstagram.com/v/t50/vid.mp4", "instagram").is_some());
        assert!(validate_video_cdn_url("https://evil.example/vid.mp4", "instagram").is_none());
        assert!(validate_video_cdn_url("https://evil.example/vid.mp4", "x").is_none());

        assert!(validate_image_url(Some("https://i.pinimg.com/originals/a.jpg")).is_some());
        assert!(validate_image_url(Some("https://p19-common-sign.tiktokcdn.com/a.jpeg?x=1")).is_some());
        assert!(validate_image_url(Some("https://evil.example/a.jpg")).is_none());
        assert!(validate_image_url(Some("http://i.pinimg.com/a.jpg")).is_none());
    }
}
