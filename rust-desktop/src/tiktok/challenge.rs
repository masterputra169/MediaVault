//! Solver for the TikTok "SlardarWAF" JavaScript challenge.
//!
//! When the WAF does not hand back the rehydration payload it serves a tiny
//! script that embeds a SHA-256 proof-of-work plus the name of the cookie the
//! answer must be stored in. yt-dlp brute-forces the nonce in Python; this is
//! the same search, done natively.

use base64::Engine as _;
use once_cell::sync::Lazy;
use regex::Regex;
use sha2::{Digest, Sha256};

use crate::errors::AppError;

const MAX_NONCE: u32 = 1_000_000;

static TAG_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?is)<[a-z][a-z0-9]*[^>]*\bid\s*=\s*["']?([a-z0-9_-]+)["']?[^>]*>"#).unwrap());
static CLASS_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?is)\bclass\s*=\s*["']([^"']+)["']"#).unwrap());

pub struct SolvedChallenge {
    pub cookie_name: String,
    pub cookie_value: String,
    pub extra: Vec<(String, String)>,
}

fn attr_of(html: &str, id: &str) -> Option<String> {
    for caps in TAG_RE.captures_iter(html) {
        if !caps[1].eq_ignore_ascii_case(id) {
            continue;
        }
        if let Some(class) = CLASS_RE.captures(&caps[0]) {
            let value = class[1].trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn b64_decode(raw: &str) -> Option<Vec<u8>> {
    let trimmed = raw.trim().trim_end_matches('=');
    let mut padded = trimmed.to_string();
    while padded.len() % 4 != 0 {
        padded.push('=');
    }
    base64::engine::general_purpose::STANDARD
        .decode(padded)
        .ok()
}

/// True when the page looks like the WAF challenge interstitial.
pub fn looks_like_challenge(html: &str) -> bool {
    attr_of(html, "cs").is_some() && attr_of(html, "wci").is_some()
}

/// Brute-force the challenge nonce and build the answer cookie.
pub fn solve(html: &str) -> Result<SolvedChallenge, AppError> {
    let payload = attr_of(html, "cs").ok_or_else(|| AppError::new("WAF_BLOCKED"))?;
    let cookie_name = attr_of(html, "wci").ok_or_else(|| AppError::new("WAF_BLOCKED"))?;

    let mut data: serde_json::Value =
        serde_json::from_slice(&b64_decode(&payload).ok_or_else(|| AppError::new("WAF_BLOCKED"))?)
            .map_err(|_| AppError::new("WAF_BLOCKED"))?;

    let expected = data
        .pointer("/v/c")
        .and_then(|v| v.as_str())
        .and_then(b64_decode)
        .ok_or_else(|| AppError::new("WAF_BLOCKED"))?;
    let base = data
        .pointer("/v/a")
        .and_then(|v| v.as_str())
        .and_then(b64_decode)
        .ok_or_else(|| AppError::new("WAF_BLOCKED"))?;

    // The page hashes sha256(a || decimal_nonce) and compares against c.
    let mut found: Option<u32> = None;
    for nonce in 0..=MAX_NONCE {
        let mut hasher = Sha256::new();
        hasher.update(&base);
        hasher.update(nonce.to_string().as_bytes());
        if hasher.finalize().as_slice() == expected.as_slice() {
            found = Some(nonce);
            break;
        }
    }
    let nonce = found.ok_or_else(|| AppError::new("WAF_BLOCKED"))?;

    let answer = base64::engine::general_purpose::STANDARD.encode(nonce.to_string());
    if let Some(slot) = data.get_mut("d") {
        *slot = serde_json::Value::String(answer);
    } else if let Some(obj) = data.as_object_mut() {
        obj.insert("d".into(), serde_json::Value::String(answer));
    }

    let serialized = serde_json::to_vec(&data).map_err(|_| AppError::new("WAF_BLOCKED"))?;
    let cookie_value = base64::engine::general_purpose::STANDARD.encode(serialized);

    let mut extra = Vec::new();
    if let (Some(name), Some(value)) = (attr_of(html, "rci"), attr_of(html, "rs")) {
        extra.push((name, value));
    }

    Ok(SolvedChallenge {
        cookie_name,
        cookie_value,
        extra,
    })
}
