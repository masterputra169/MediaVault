//! Impersonating HTTP layer for the TikTok extractor.
//!
//! Two client profiles are used, mirroring what yt-dlp does with
//! impersonate=True:
//!
//! * a Safari iOS profile for the public web front-end (TLS + HTTP/2 + H2
//!   framing exactly like the real browser),
//! * an OkHttp profile for the Android app API endpoint.
//!
//! Both are BoringSSL-backed, so the TLS ClientHello / JA3 / JA4 and the
//! Akamai fingerprint match the emulated device instead of looking like a
//! generic Rust HTTP stack.

use std::sync::Mutex;
use std::time::Duration;

use once_cell::sync::Lazy;

use crate::errors::AppError;

pub const WEB_ORIGIN: &str = "https://www.tiktok.com";

/// App API front-ends (same rotating pool yt-dlp defaults to).
pub const API_HOSTS: &[&str] = &[
    "api16-normal-c-useast1a.tiktokv.com",
    "api22-normal-c-useast2a.tiktokv.com",
    "api19-normal-c-useast1a.tiktokv.com",
];

const MAX_REDIRECTS: usize = 3;
const MAX_PAGE_BYTES: usize = 6 * 1024 * 1024;

/// Which cookie jar a cookie belongs to; keeps web cookies out of API calls.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Jar {
    Web,
    Api,
}

#[derive(Default)]
pub struct CookieStore {
    web: Mutex<Vec<(String, String)>>,
    api: Mutex<Vec<(String, String)>>,
}

impl CookieStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn slot(&self, jar: Jar) -> &Mutex<Vec<(String, String)>> {
        match jar {
            Jar::Web => &self.web,
            Jar::Api => &self.api,
        }
    }

    /// Store a name=value cookie, replacing an earlier value with the same name.
    pub fn set(&self, jar: Jar, name: &str, value: &str) {
        if name.is_empty() || value.is_empty() {
            return;
        }
        let mut guard = self.slot(jar).lock().unwrap();
        if let Some(slot) = guard.iter_mut().find(|(n, _)| n == name) {
            slot.1 = value.to_string();
        } else {
            guard.push((name.to_string(), value.to_string()));
        }
    }

    pub fn clear(&self, jar: Jar, name: &str) {
        self.slot(jar).lock().unwrap().retain(|(n, _)| n != name);
    }

    pub fn header(&self, jar: Jar) -> Option<String> {
        let guard = self.slot(jar).lock().unwrap();
        if guard.is_empty() {
            return None;
        }
        Some(
            guard
                .iter()
                .map(|(n, v)| format!("{n}={v}"))
                .collect::<Vec<_>>()
                .join("; "),
        )
    }

    pub fn value(&self, jar: Jar, name: &str) -> Option<String> {
        let guard = self.slot(jar).lock().unwrap();
        guard
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
    }

    /// Parse every Set-Cookie header of a response into the given jar.
    pub fn absorb(&self, jar: Jar, headers: &[(String, String)]) {
        for (name, value) in headers {
            if !name.eq_ignore_ascii_case("set-cookie") {
                continue;
            }
            let (pair, attrs) = match value.split_once(';') {
                Some((pair, attrs)) => (pair, attrs),
                None => (value.as_str(), ""),
            };
            if let Some((key, val)) = pair.split_once('=') {
                let key = key.trim();
                let val = val.trim();
                if key.is_empty() || val.is_empty() {
                    continue;
                }
                if max_age_zero(attrs) {
                    self.clear(jar, key);
                } else {
                    self.set(jar, key, val);
                }
            }
        }
    }
}

fn max_age_zero(attrs: &str) -> bool {
    for part in attrs.split(';') {
        if let Some((k, v)) = part.split_once('=') {
            if k.trim().eq_ignore_ascii_case("max-age") {
                return v.trim() == "0";
            }
        }
    }
    false
}

fn map_err(err: &wreq::Error) -> AppError {
    if err.is_timeout() {
        AppError::new("TIMEOUT")
    } else {
        AppError::new("NETWORK_ERROR")
    }
}

/// Browser profile used for the public web front-end. Two are kept because the
/// WAF scores them differently: when one starts answering with challenges the
/// extractor falls back to the other.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WebProfile {
    Safari,
    Chrome,
}

static WEB_CLIENT_SAFARI: Lazy<wreq::Client> = Lazy::new(|| build_web_client(wreq_util::Emulation::Safari26));
static WEB_CLIENT_CHROME: Lazy<wreq::Client> = Lazy::new(|| build_web_client(wreq_util::Emulation::Chrome142));

fn build_web_client(emulation: wreq_util::Profile) -> wreq::Client {
    wreq::Client::builder()
        .emulation(emulation)
        .redirect(wreq::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .pool_idle_timeout(Duration::from_secs(45))
        .build()
        .expect("failed to build TikTok web client")
}

fn web_client(profile: WebProfile) -> &'static wreq::Client {
    match profile {
        WebProfile::Safari => &WEB_CLIENT_SAFARI,
        WebProfile::Chrome => &WEB_CLIENT_CHROME,
    }
}

static API_CLIENT: Lazy<wreq::Client> = Lazy::new(|| {
    wreq::Client::builder()
        .emulation(wreq_util::Emulation::OkHttp5)
        .redirect(wreq::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .pool_idle_timeout(Duration::from_secs(45))
        .build()
        .expect("failed to build TikTok app client")
});

pub struct Page {
    pub status: u16,
    pub final_url: String,
    pub body: String,
}

/// Fetch a TikTok web page with browser-emulating TLS, following same-host
/// redirects manually so every hop can be validated first.
pub async fn get_web_page(
    profile: WebProfile,
    url: &str,
    extra_headers: &[(String, String)],
    jar: &CookieStore,
) -> Result<Page, AppError> {
    let client = web_client(profile);
    let mut current = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        crate::url_policy::ensure_public_host(&current).await?;
        let mut req = client
            .get(&current)
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            )
            .header("Accept-Language", "en-US,en;q=0.9")
            .header("Sec-Fetch-Mode", "navigate")
            .header("Sec-Fetch-Dest", "document")
            .header("Sec-Fetch-Site", "none")
            .header("Upgrade-Insecure-Requests", "1");
        if let Some(cookie) = jar.header(Jar::Web) {
            req = req.header("Cookie", cookie);
        }
        for (name, value) in extra_headers {
            req = req.header(name.as_str(), value.as_str());
        }
        let resp = req.send().await.map_err(|e| map_err(&e))?;

        let headers = collect_headers(&resp);
        jar.absorb(Jar::Web, &headers);
        let status = resp.status().as_u16();

        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            let location = headers
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case("location"))
                .map(|(_, v)| v.clone())
                .ok_or_else(|| AppError::new("UNAVAILABLE_MEDIA"))?;
            let base = url::Url::parse(&current).map_err(|_| AppError::new("INVALID_URL"))?;
            let joined = base
                .join(&location)
                .map_err(|_| AppError::new("INVALID_URL"))?;
            let next = joined.to_string();
            if !next.starts_with("https://") {
                return Err(AppError::new("UNSUPPORTED_PLATFORM"));
            }
            current = next;
            continue;
        }

        let body = resp.text().await.map_err(|e| map_err(&e))?;
        if body.len() > MAX_PAGE_BYTES {
            return Err(AppError::new("EXTRACTOR_FAILED"));
        }
        return Ok(Page {
            status,
            final_url: current,
            body,
        });
    }
    Err(AppError::new("NETWORK_ERROR"))
}

/// POST an application/x-www-form-urlencoded body to one of the app API hosts.
pub async fn post_app_api(
    host: &str,
    endpoint: &str,
    query: &str,
    body: &str,
    user_agent: &str,
    extra_headers: &[(String, String)],
    jar: &CookieStore,
) -> Result<(u16, String), AppError> {
    let url = if query.is_empty() {
        format!("https://{host}/aweme/v1/{endpoint}/")
    } else {
        format!("https://{host}/aweme/v1/{endpoint}/?{query}")
    };
    crate::url_policy::ensure_public_host(&url).await?;

    let mut req = API_CLIENT
        .post(&url)
        .header("User-Agent", user_agent)
        .header("Accept", "application/json")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body.to_string());
    if let Some(cookie) = jar.header(Jar::Api) {
        req = req.header("Cookie", cookie);
    }
    for (name, value) in extra_headers {
        req = req.header(name.as_str(), value.as_str());
    }
    let resp = req.send().await.map_err(|e| map_err(&e))?;
    let status = resp.status().as_u16();
    let headers = collect_headers(&resp);
    jar.absorb(Jar::Api, &headers);
    let text = resp.text().await.map_err(|e| map_err(&e))?;
    Ok((status, text))
}

fn collect_headers(resp: &wreq::Response) -> Vec<(String, String)> {
    resp.headers()
        .iter()
        .map(|(n, v)| {
            (
                n.as_str().to_string(),
                v.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// yt-dlp's 403-busting trick: sprinkle random, otherwise meaningless headers
/// into the request so the WAF sees a less templated header fingerprint.
pub fn blockbuster_headers() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let count = 2 + (random_u8() % 7) as usize;
    for _ in 0..count {
        let name_len = 8 + (random_u8() % 17) as usize;
        let value_len = 16 + (random_u8() % 17) as usize;
        out.push((random_consonants(name_len), random_consonants(value_len)));
    }
    out
}

fn random_consonants(len: usize) -> String {
    const ALPHABET: &[u8] = b"bcdfghjklmnpqrstvwxz";
    let mut out = String::with_capacity(len);
    let mut seed = uuid::Uuid::new_v4().as_u128();
    for _ in 0..len {
        out.push(ALPHABET[(seed & 0x1f) as usize % ALPHABET.len()] as char);
        seed >>= 5;
    }
    out
}

pub fn random_u8() -> u8 {
    (uuid::Uuid::new_v4().as_u128() & 0xff) as u8
}

pub fn random_hex(len: usize) -> String {
    const HEX: &[u8] = b"0123456789abcdef";
    let mut seed = uuid::Uuid::new_v4().as_u128();
    let mut out = String::with_capacity(len);
    for _ in 0..len {
        out.push(HEX[(seed & 0x0f) as usize] as char);
        seed = seed.rotate_right(4);
    }
    out
}
