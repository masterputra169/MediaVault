use std::collections::{HashMap, HashSet};
use std::io::{BufWriter, Cursor, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use image::codecs::jpeg::JpegEncoder;
use image::{ImageFormat, Limits};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Semaphore;
use url::Url;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::errors::{classify_extractor_error, AppError};
use crate::url_policy::{
    clean_text, ensure_public_host, validate_image_url, validate_video_cdn_url, SHARED_HTTP,
};

pub const MAX_IMAGES: usize = 30;
pub const MAX_IMAGE_BYTES: usize = 50 * 1024 * 1024;
pub const MAX_HTML_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_IMAGE_PIXELS: u64 = 40_000_000;
const MAX_IMAGE_DIM: u32 = 16_384;
const MAX_IMAGE_DECODE_ALLOC: u64 = 128 * 1024 * 1024;
pub const GENERIC_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

static IG_SJS_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?s)<script\b[^>]+\bdata-sjs>(\{.+?\})</script>"#).unwrap());
static JSON_SCRIPT_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?s)<script\b[^>]*type=["']application/json["'][^>]*>(\{.+?\})</script>"#)
        .unwrap()
});
static THREADS_POST_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"/(?:post|t)/([A-Za-z0-9_-]+)").unwrap());
static IG_INIT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#""init",\[\],\[(.*?)\]\],"#).unwrap());
static IG_EMBED_IMG_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"class="EmbeddedMediaImage"[^>]+src="([^"]+)""#).unwrap());
static PIN_ORIG_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"https://i\.pinimg\.com/originals/[A-Za-z0-9/_.-]+\.(?:jpg|jpeg|png|webp)").unwrap()
});
static PIN_FALLBACK_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"https://i\.pinimg\.com/(?:736x|1200x)/[A-Za-z0-9/_.-]+\.(?:jpg|jpeg|png|webp)")
        .unwrap()
});
static PIN_VIDEO_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"https://v\d*\.pinimg\.com/videos/[A-Za-z0-9/_.-]+\.mp4").unwrap()
});
static PIN_DIM_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"_(\d{3,4})[wp]\.mp4").unwrap());
static OG_TITLE_1: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?i)<meta[^>]+(?:property|name)=["']og:title["'][^>]+content=["']([^"']+)["']"#)
        .unwrap()
});
static OG_TITLE_2: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?i)<meta[^>]+content=["']([^"']+)["'][^>]+(?:property|name)=["']og:title["']"#)
        .unwrap()
});
static OG_DESC_1: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#"(?is)<meta[^>]+(?:property|name)=["']og:description["'][^>]+content=["']([^"']+)["']"#,
    )
    .unwrap()
});
static OG_DESC_2: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#"(?is)<meta[^>]+content=["']([^"']+)["'][^>]+(?:property|name)=["']og:description["']"#,
    )
    .unwrap()
});
static OG_IMAGE_1: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?i)<meta[^>]+(?:property|name)=["']og:image["'][^>]+content=["']([^"']+)["']"#)
        .unwrap()
});
static OG_IMAGE_2: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?i)<meta[^>]+content=["']([^"']+)["'][^>]+(?:property|name)=["']og:image["']"#)
        .unwrap()
});
static FB_REACTIONS_PREFIX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^[\d.,]+[KMB]?\s+views\s*(?:·|&#xb7;)\s*[\d.,]+[KMB]?\s+reactions\s*\|\s*")
        .unwrap()
});
static FB_EFG_RES_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(\d{3,4})p").unwrap());

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageItem {
    pub index: usize,
    pub url: String,
    pub preview_token: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct ExtractedPhotoSet {
    pub title: Option<String>,
    #[allow(dead_code)]
    pub uploader: Option<String>,
    #[allow(dead_code)]
    pub media_id: Option<String>,
    pub images: Vec<ImageItem>,
}

#[derive(Debug, Clone)]
pub struct PreviewRgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

pub struct PreviewCache {
    tokens: Mutex<HashMap<String, (Instant, String)>>,
    by_url: Mutex<HashMap<String, String>>,
    rgba_cache: Mutex<HashMap<String, (Instant, Arc<PreviewRgba>)>>,
    decode_sem: Arc<Semaphore>,
}

impl PreviewCache {
    pub fn new() -> Self {
        Self {
            tokens: Mutex::new(HashMap::new()),
            by_url: Mutex::new(HashMap::new()),
            rgba_cache: Mutex::new(HashMap::new()),
            decode_sem: Arc::new(Semaphore::new(4)),
        }
    }

    pub fn register(&self, raw_url: Option<&str>) -> Option<String> {
        let safe = validate_image_url(raw_url)?;
        let now = Instant::now();
        let mut tokens = self.tokens.lock().unwrap();
        let mut by_url = self.by_url.lock().unwrap();
        if let Some(existing) = by_url.get(&safe) {
            if tokens.get(existing).is_some_and(|(exp, _)| *exp > now) {
                return Some(existing.clone());
            }
        }
        if tokens.len() > 256 || by_url.len() > 256 {
            tokens.retain(|_, (exp, _)| *exp > now);
            by_url.retain(|_, tok| tokens.contains_key(tok));
            if tokens.len() > 256 {
                tokens.clear();
                by_url.clear();
            }
        }
        let token = uuid::Uuid::new_v4().simple().to_string();
        tokens.insert(token.clone(), (now + Duration::from_secs(900), safe.clone()));
        by_url.insert(safe, token.clone());
        Some(token)
    }

    pub fn get_cached_rgba(&self, token: &str) -> Option<Arc<PreviewRgba>> {
        let now = Instant::now();
        self.rgba_cache
            .lock()
            .unwrap()
            .get(token)
            .filter(|(exp, _)| *exp > now)
            .map(|(_, img)| Arc::clone(img))
    }

    pub async fn fetch_rgba(&self, token: &str) -> Result<Arc<PreviewRgba>, AppError> {
        if let Some(hit) = self.get_cached_rgba(token) {
            return Ok(hit);
        }
        let now = Instant::now();
        let target_url = {
            let tokens = self.tokens.lock().unwrap();
            match tokens.get(token) {
                Some((exp, u)) if *exp > now => u.clone(),
                _ => return Err(AppError::new("NOT_FOUND")),
            }
        };
        let _permit = self
            .decode_sem
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| AppError::new("INTERNAL_ERROR"))?;
        let raw = fetch_bytes(&target_url, &[], 15 * 1024 * 1024, 15).await?;
        let decoded = tokio::task::spawn_blocking(move || decode_preview_rgba(&raw, 480))
            .await
            .map_err(|_| AppError::new("INTERNAL_ERROR"))??;
        let arc = Arc::new(decoded);
        let mut cache = self.rgba_cache.lock().unwrap();
        if cache.len() > 80 {
            cache.retain(|_, (exp, _)| *exp > now);
            if cache.len() > 80 {
                cache.clear();
            }
        }
        cache.insert(
            token.to_string(),
            (Instant::now() + Duration::from_secs(900), Arc::clone(&arc)),
        );
        Ok(arc)
    }
}

pub fn decode_preview_rgba(raw: &[u8], max_dim: u32) -> Result<PreviewRgba, AppError> {
    let img = decode_safe_image(raw)?;
    let resized = if img.width() > max_dim || img.height() > max_dim {
        img.thumbnail(max_dim, max_dim)
    } else {
        img
    };
    let rgba = resized.to_rgba8();
    Ok(PreviewRgba {
        width: rgba.width(),
        height: rgba.height(),
        pixels: rgba.into_raw(),
    })
}

#[allow(dead_code)]
pub fn looks_like_photo_url(url_str: &str, platform: &str) -> bool {
    let Ok(parsed) = Url::parse(url_str) else {
        return false;
    };
    let path = parsed.path().to_lowercase();
    if path.contains("/video/") {
        return false;
    }
    if platform == "pinterest" && path.contains("/pin/") {
        return true;
    }
    if platform == "tiktok" && path.contains("/photo/") {
        return true;
    }
    if platform == "facebook"
        && (path.contains("/photo") || path.contains("/posts/") || path.contains("/permalink"))
    {
        return true;
    }
    ["/p/", "/status/", "/post/"]
        .iter()
        .any(|h| path.contains(h))
}

pub async fn fetch_bytes(
    url_str: &str,
    headers: &[(&str, &str)],
    max_bytes: usize,
    timeout_secs: u64,
) -> Result<Vec<u8>, AppError> {
    let mut current = url_str.to_string();
    let is_image_cdn = validate_image_url(Some(&current)).is_some();

    for _hop in 0..=5 {
        ensure_public_host(&current).await?;
        let mut req = SHARED_HTTP
            .get(&current)
            .timeout(Duration::from_secs(timeout_secs))
            .header("Accept-Language", "en-US,en;q=0.9");
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let mut resp = req.send().await.map_err(|_| AppError::new("NETWORK_ERROR"))?;
        let status = resp.status().as_u16();
        if matches!(status, 301 | 302 | 303 | 307 | 308) && is_image_cdn {
            let loc = resp
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| AppError::new("NETWORK_ERROR"))?;
            let base = Url::parse(&current).map_err(|_| AppError::new("INVALID_URL"))?;
            let joined = base.join(loc).map_err(|_| AppError::new("INVALID_URL"))?;
            let safe_next = validate_image_url(Some(joined.as_str()))
                .ok_or_else(|| AppError::new("FORBIDDEN"))?;
            current = safe_next;
            continue;
        }
        if status == 401 || status == 403 {
            return Err(AppError::new("LOGIN_REQUIRED"));
        }
        if status == 404 {
            return Err(AppError::new("UNAVAILABLE_MEDIA"));
        }
        if !(200..300).contains(&status) {
            return Err(classify_extractor_error(&format!("HTTP error {status}")));
        }
        if let Some(len) = resp.content_length() {
            if len as usize > max_bytes {
                return Err(AppError::new("FILE_TOO_LARGE"));
            }
        }
        let mut buf = Vec::new();
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|_| AppError::new("NETWORK_ERROR"))?
        {
            if buf.len() + chunk.len() > max_bytes {
                return Err(AppError::new("FILE_TOO_LARGE"));
            }
            buf.extend_from_slice(&chunk);
        }
        return Ok(buf);
    }
    Err(AppError::new("NETWORK_ERROR"))
}

fn normalize_items(raw: Vec<(String, Option<u32>, Option<u32>)>) -> Vec<ImageItem> {
    let mut seen = HashSet::new();
    let mut items = Vec::new();
    for (u, w, h) in raw {
        let Some(safe) = validate_image_url(Some(&u)) else {
            continue;
        };
        if !seen.insert(safe.clone()) {
            continue;
        }
        items.push(ImageItem {
            index: items.len() + 1,
            url: safe,
            preview_token: None,
            width: w.filter(|&x| x > 0),
            height: h.filter(|&x| x > 0),
        });
        if items.len() >= MAX_IMAGES {
            break;
        }
    }
    items
}

fn find_key_recursive<'a>(val: &'a Value, target_key: &str) -> Option<&'a Value> {
    match val {
        Value::Object(map) => {
            if let Some(v) = map.get(target_key) {
                return Some(v);
            }
            for v in map.values() {
                if let Some(found) = find_key_recursive(v, target_key) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(arr) => {
            for v in arr {
                if let Some(found) = find_key_recursive(v, target_key) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

fn ig_best_image(item: &Value) -> Option<(String, Option<u32>, Option<u32>)> {
    if item.get("is_video").and_then(Value::as_bool) == Some(true)
        || item.get("video_versions").and_then(Value::as_array).is_some_and(|a| !a.is_empty())
        || item.get("media_type").and_then(Value::as_i64) == Some(2)
    {
        return None;
    }
    let cands = item
        .pointer("/image_versions2/candidates")
        .and_then(Value::as_array);
    let chosen = cands.and_then(|arr| {
        let sized: Vec<&Value> = arr
            .iter()
            .filter(|c| {
                c.get("width").and_then(Value::as_u64).unwrap_or(0) > 0
                    && c.get("height").and_then(Value::as_u64).unwrap_or(0) > 0
                    && c.get("url").and_then(Value::as_str).is_some()
            })
            .collect();
        if !sized.is_empty() {
            sized.into_iter().max_by_key(|c| {
                c.get("width").and_then(Value::as_u64).unwrap_or(0)
                    * c.get("height").and_then(Value::as_u64).unwrap_or(0)
            })
        } else {
            arr.iter()
                .find(|c| c.get("url").and_then(Value::as_str).is_some())
        }
    });

    let url = chosen
        .and_then(|c| c.get("url").and_then(Value::as_str))
        .or_else(|| item.get("display_uri").and_then(Value::as_str))
        .or_else(|| item.get("display_url").and_then(Value::as_str))?
        .to_string();

    let w = chosen
        .and_then(|c| c.get("width").and_then(Value::as_u64))
        .or_else(|| item.get("original_width").and_then(Value::as_u64))
        .or_else(|| item.pointer("/dimensions/width").and_then(Value::as_u64))
        .map(|x| x as u32);
    let h = chosen
        .and_then(|c| c.get("height").and_then(Value::as_u64))
        .or_else(|| item.get("original_height").and_then(Value::as_u64))
        .or_else(|| item.pointer("/dimensions/height").and_then(Value::as_u64))
        .map(|x| x as u32);
    Some((url, w, h))
}

#[derive(Debug, Clone)]
pub struct InstagramPolarisResult {
    pub title: Option<String>,
    pub uploader: Option<String>,
    pub media_id: String,
    pub duration_seconds: Option<f64>,
    pub thumbnail_url: Option<String>,
    pub video_variants: Vec<DirectVideoVariant>,
    pub images: Vec<ImageItem>,
}

fn ig_thumbnail_from_product(product: &Value) -> Option<String> {
    product
        .pointer("/image_versions2/candidates/0/url")
        .and_then(Value::as_str)
        .or_else(|| product.get("display_uri").and_then(Value::as_str))
        .or_else(|| product.get("display_url").and_then(Value::as_str))
        .and_then(|u| validate_image_url(Some(u)))
}

pub fn parse_instagram_polaris_media(html: &str, shortcode: &str) -> Option<InstagramPolarisResult> {
    for cap in IG_SJS_RE.captures_iter(html) {
        let Ok(json) = serde_json::from_str::<Value>(&cap[1]) else {
            continue;
        };
        let Some(media) = find_key_recursive(&json, "xig_polaris_media") else {
            continue;
        };
        let Some(product) = media.get("if_not_gated_logged_out") else {
            continue;
        };
        let username = clean_text(product.pointer("/user/username").and_then(Value::as_str), 120);
        let uploader = clean_text(
            product
                .pointer("/user/full_name")
                .and_then(Value::as_str)
                .or(username.as_deref()),
            120,
        );
        let caption = clean_text(
            product
                .pointer("/caption/text")
                .and_then(Value::as_str)
                .or_else(|| product.get("caption").and_then(Value::as_str)),
            240,
        );

        let mut video_variants = Vec::new();
        let orig_w = product.get("original_width").and_then(Value::as_u64).unwrap_or(0) as u32;
        let orig_h = product.get("original_height").and_then(Value::as_u64).unwrap_or(0) as u32;
        let orig_short = if orig_w > 0 && orig_h > 0 {
            orig_w.min(orig_h)
        } else {
            orig_h.max(orig_w)
        };

        if let Some(vvs) = product.get("video_versions").and_then(Value::as_array) {
            for v in vvs {
                let Some(u) = v.get("url").and_then(Value::as_str) else {
                    continue;
                };
                let Some(safe_u) = validate_video_cdn_url(u, "instagram") else {
                    continue;
                };
                let vw = v.get("width").and_then(Value::as_u64).unwrap_or(0) as u32;
                let vh = v.get("height").and_then(Value::as_u64).unwrap_or(0) as u32;
                let dim = if vw > 0 && vh > 0 {
                    vw.min(vh)
                } else {
                    vh.max(vw).max(orig_short)
                };
                video_variants.push(DirectVideoVariant {
                    url: safe_u,
                    height: dim,
                    bitrate: 0,
                });
            }
        }
        // If DASH manifest exists alongside original_width/height, advertise native height (e.g. 1080p)
        if !video_variants.is_empty() && orig_short > 0 {
            if !video_variants.iter().any(|v| v.height == orig_short) {
                let first_url = video_variants[0].url.clone();
                video_variants.push(DirectVideoVariant {
                    url: first_url,
                    height: orig_short,
                    bitrate: 0,
                });
            }
            video_variants.sort_by(|a, b| b.height.cmp(&a.height));
        }

        let mut raw_imgs = Vec::new();
        if let Some(carousel) = product.get("carousel_media").and_then(Value::as_array) {
            for slide in carousel {
                if let Some(img) = ig_best_image(slide) {
                    raw_imgs.push(img);
                }
            }
        } else if video_variants.is_empty() {
            if let Some(img) = ig_best_image(product) {
                raw_imgs.push(img);
            }
        }
        let images = normalize_items(raw_imgs);

        if video_variants.is_empty() && images.is_empty() {
            continue;
        }

        let default_title = if !video_variants.is_empty() {
            username
                .as_deref()
                .map(|u| format!("Video by {u}"))
                .or_else(|| Some(format!("Instagram {shortcode}")))
        } else {
            username
                .as_deref()
                .map(|u| format!("Photo by {u}"))
                .or_else(|| Some(format!("Instagram {shortcode}")))
        };

        let thumbnail_url = ig_thumbnail_from_product(product)
            .or_else(|| images.first().map(|i| i.url.clone()));
        let duration_seconds = product
            .get("video_duration")
            .and_then(Value::as_f64)
            .filter(|d| d.is_finite() && *d >= 0.0);

        return Some(InstagramPolarisResult {
            title: caption.or(default_title),
            uploader,
            media_id: shortcode.to_string(),
            duration_seconds,
            thumbnail_url,
            video_variants,
            images,
        });
    }
    None
}

fn parse_instagram_polaris(html: &str, shortcode: &str) -> Option<ExtractedPhotoSet> {
    let media = parse_instagram_polaris_media(html, shortcode)?;
    if media.images.is_empty() {
        return None;
    }
    Some(ExtractedPhotoSet {
        title: media.title,
        uploader: media.uploader,
        media_id: Some(media.media_id),
        images: media.images,
    })
}

pub async fn fetch_instagram_polaris(url_str: &str) -> Option<InstagramPolarisResult> {
    let re = Regex::new(r"/(?:p|reel|tv)/([A-Za-z0-9_-]+)").ok()?;
    let path = Url::parse(url_str).ok()?.path().to_string();
    let sc = re.captures(&path)?.get(1)?.as_str().to_string();
    let nav_headers = [
        (
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        ),
        ("Sec-Fetch-Mode", "navigate"),
    ];
    let bytes = fetch_bytes(
        &format!("https://www.instagram.com/p/{sc}/"),
        &nav_headers,
        MAX_HTML_BYTES,
        15,
    )
    .await
    .ok()?;
    let text = String::from_utf8_lossy(&bytes);
    parse_instagram_polaris_media(&text, &sc)
}

#[derive(Debug, Clone)]
pub struct ThreadsMediaResult {
    pub title: Option<String>,
    pub uploader: Option<String>,
    pub media_id: String,
    pub duration_seconds: Option<f64>,
    pub thumbnail_url: Option<String>,
    pub has_audio: bool,
    pub video_variants: Vec<DirectVideoVariant>,
    pub images: Vec<ImageItem>,
}

fn is_meta_cdn_video_url(raw: &str) -> bool {
    let Ok(parsed) = Url::parse(raw) else {
        return false;
    };
    if parsed.scheme() != "https" || !parsed.username().is_empty() || parsed.password().is_some() {
        return false;
    }
    if let Some(p) = parsed.port() {
        if p != 443 {
            return false;
        }
    }
    let Some(host) = parsed.host_str().map(|h| h.to_lowercase()) else {
        return false;
    };
    ["fbcdn.net", "cdninstagram.com"]
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

fn collect_meta_video_variants(item: &Value) -> Vec<DirectVideoVariant> {
    let mut variants = Vec::new();
    let mut seen_urls = HashSet::new();
    let orig_w = item.get("original_width").and_then(Value::as_u64).unwrap_or(0) as u32;
    let orig_h = item.get("original_height").and_then(Value::as_u64).unwrap_or(0) as u32;
    let orig_short = if orig_w > 0 && orig_h > 0 {
        orig_w.min(orig_h)
    } else {
        orig_h.max(orig_w)
    };

    if let Some(vvs) = item.get("video_versions").and_then(Value::as_array) {
        for v in vvs {
            let Some(u) = v.get("url").and_then(Value::as_str) else {
                continue;
            };
            if !is_meta_cdn_video_url(u) || !seen_urls.insert(u.to_string()) {
                continue;
            }
            let vw = v.get("width").and_then(Value::as_u64).unwrap_or(0) as u32;
            let vh = v.get("height").and_then(Value::as_u64).unwrap_or(0) as u32;
            let dim = if vw > 0 && vh > 0 {
                vw.min(vh)
            } else {
                vh.max(vw).max(orig_short)
            };
            variants.push(DirectVideoVariant {
                url: u.to_string(),
                height: dim,
                bitrate: 0,
            });
        }
    }
    variants.sort_by(|a, b| b.height.cmp(&a.height));
    variants
}

fn find_threads_posts_by_code<'a>(val: &'a Value, code: &str, out: &mut Vec<&'a Value>) {
    match val {
        Value::Object(map) => {
            if map.get("code").and_then(Value::as_str) == Some(code)
                && (map.contains_key("image_versions2")
                    || map.contains_key("video_versions")
                    || map.contains_key("carousel_media")
                    || map.contains_key("text_post_app_info"))
            {
                out.push(val);
            }
            for v in map.values() {
                find_threads_posts_by_code(v, code, out);
            }
        }
        Value::Array(arr) => {
            for v in arr {
                find_threads_posts_by_code(v, code, out);
            }
        }
        _ => {}
    }
}

fn extract_threads_media_from_post(
    post: &Value,
) -> (Vec<DirectVideoVariant>, Vec<ImageItem>, Option<String>, Option<f64>, bool) {
    let mut video_variants = collect_meta_video_variants(post);
    let mut raw_imgs = Vec::new();
    let mut thumb = ig_thumbnail_from_product(post);
    let mut duration = post
        .get("video_duration")
        .and_then(Value::as_f64)
        .filter(|d| d.is_finite() && *d >= 0.0);
    let mut has_audio = post
        .get("has_audio")
        .and_then(Value::as_bool)
        .unwrap_or(!video_variants.is_empty());

    if let Some(carousel) = post.get("carousel_media").and_then(Value::as_array) {
        for slide in carousel {
            let slide_videos = collect_meta_video_variants(slide);
            if !slide_videos.is_empty() {
                if video_variants.is_empty() {
                    video_variants = slide_videos;
                    if thumb.is_none() {
                        thumb = ig_thumbnail_from_product(slide);
                    }
                    if duration.is_none() {
                        duration = slide
                            .get("video_duration")
                            .and_then(Value::as_f64)
                            .filter(|d| d.is_finite() && *d >= 0.0);
                    }
                    has_audio = slide
                        .get("has_audio")
                        .and_then(Value::as_bool)
                        .unwrap_or(true);
                }
            } else if let Some(img) = ig_best_image(slide) {
                raw_imgs.push(img);
            }
        }
    } else if video_variants.is_empty() {
        if let Some(img) = ig_best_image(post) {
            raw_imgs.push(img);
        }
    }

    let images = normalize_items(raw_imgs);
    if thumb.is_none() {
        thumb = images.first().map(|i| i.url.clone());
    }
    (video_variants, images, thumb, duration, has_audio)
}

pub fn parse_threads_media(html: &str, shortcode: &str) -> Option<ThreadsMediaResult> {
    for cap in JSON_SCRIPT_RE.captures_iter(html) {
        let raw_json = &cap[1];
        if !raw_json.contains(shortcode) {
            continue;
        }
        let Ok(json) = serde_json::from_str::<Value>(raw_json) else {
            continue;
        };
        let mut refs = Vec::new();
        find_threads_posts_by_code(&json, shortcode, &mut refs);
        for post in refs {
            let (
                mut video_variants,
                mut images,
                mut thumbnail_url,
                mut duration_seconds,
                mut has_audio,
            ) = extract_threads_media_from_post(post);

            // If the main thread is text-only, fall back to a quoted or reposted thread's media.
            if video_variants.is_empty() && images.is_empty() {
                for sub_ptr in [
                    "/text_post_app_info/share_info/quoted_post",
                    "/text_post_app_info/share_info/reposted_post",
                ] {
                    if let Some(sub) = post.pointer(sub_ptr) {
                        let (sv, si, st, sd, sa) = extract_threads_media_from_post(sub);
                        if !sv.is_empty() || !si.is_empty() {
                            video_variants = sv;
                            images = si;
                            thumbnail_url = st;
                            duration_seconds = sd;
                            has_audio = sa;
                            break;
                        }
                    }
                }
            }

            if video_variants.is_empty() && images.is_empty() {
                continue;
            }

            let username =
                clean_text(post.pointer("/user/username").and_then(Value::as_str), 120);
            let uploader = clean_text(
                post.pointer("/user/full_name")
                    .and_then(Value::as_str)
                    .or(username.as_deref()),
                120,
            );
            let caption = clean_text(
                post.pointer("/caption/text")
                    .and_then(Value::as_str)
                    .or_else(|| post.get("caption").and_then(Value::as_str)),
                240,
            );
            let default_title = if !video_variants.is_empty() {
                username
                    .as_deref()
                    .map(|u| format!("Threads Video @{u}"))
                    .or_else(|| Some(format!("Threads {shortcode}")))
            } else {
                username
                    .as_deref()
                    .map(|u| format!("Threads Photo @{u}"))
                    .or_else(|| Some(format!("Threads {shortcode}")))
            };

            return Some(ThreadsMediaResult {
                title: caption.or(default_title),
                uploader,
                media_id: shortcode.to_string(),
                duration_seconds,
                thumbnail_url,
                has_audio: !video_variants.is_empty() && has_audio,
                video_variants,
                images,
            });
        }
    }
    None
}

pub async fn fetch_threads_media(url_str: &str) -> Option<ThreadsMediaResult> {
    let parsed = Url::parse(url_str).ok()?;
    let path = parsed.path().to_string();
    let shortcode = THREADS_POST_RE.captures(&path)?.get(1)?.as_str().to_string();
    let canonical_url = format!("https://www.threads.com{path}");
    let nav_headers = [
        (
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        ),
        ("Sec-Fetch-Mode", "navigate"),
        ("Sec-Fetch-Dest", "document"),
    ];
    let bytes = fetch_bytes(&canonical_url, &nav_headers, MAX_HTML_BYTES, 15)
        .await
        .ok()?;
    let text = String::from_utf8_lossy(&bytes);
    parse_threads_media(&text, &shortcode)
}

async fn extract_threads(url_str: &str) -> Option<ExtractedPhotoSet> {
    let res = fetch_threads_media(url_str).await?;
    if res.images.is_empty() {
        return None;
    }
    Some(ExtractedPhotoSet {
        title: res.title,
        uploader: res.uploader,
        media_id: Some(res.media_id),
        images: res.images,
    })
}

#[derive(Debug, Clone)]
pub struct FacebookMediaResult {
    pub title: Option<String>,
    pub uploader: Option<String>,
    pub media_id: Option<String>,
    pub duration_seconds: Option<f64>,
    pub thumbnail_url: Option<String>,
    pub video_variants: Vec<DirectVideoVariant>,
    pub dash_videos: Vec<DirectVideoVariant>,
    pub dash_audio_url: Option<String>,
    pub images: Vec<ImageItem>,
}

static FB_DASH_REP_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?is)<Representation\b([^>]*)>(.*?)</Representation>").unwrap());
static FB_DASH_BASEURL_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?is)<BaseURL[^>]*>([^<]+)</BaseURL>").unwrap());
static FB_DASH_HEIGHT_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?i)\bheight=["'](\d{3,4})["']"#).unwrap());
static FB_DASH_BW_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?i)\bbandwidth=["'](\d+)["']"#).unwrap());
static FB_DASH_MIME_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?i)\bmimeType=["']([^"']+)["']"#).unwrap());
static FB_DASH_CODECS_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?i)\bcodecs=["']([^"']+)["']"#).unwrap());

/// Parses Facebook's MPD/DASH XML manifest for high-res adaptive H.264 (up to 1080p+) + AAC audio.
fn parse_facebook_dash_xml(
    xml: &str,
    dash_videos: &mut Vec<DirectVideoVariant>,
    dash_audio: &mut Option<(u64, String)>,
) {
    for cap in FB_DASH_REP_RE.captures_iter(xml) {
        let attrs = &cap[1];
        let body = &cap[2];
        let Some(url_cap) = FB_DASH_BASEURL_RE.captures(body) else {
            continue;
        };
        let raw_url = unescape_html_entities(url_cap[1].trim());
        if !is_meta_cdn_video_url(&raw_url) {
            continue;
        }
        let mime = FB_DASH_MIME_RE
            .captures(attrs)
            .map(|c| c[1].to_ascii_lowercase())
            .unwrap_or_default();
        let codecs = FB_DASH_CODECS_RE
            .captures(attrs)
            .map(|c| c[1].to_ascii_lowercase())
            .unwrap_or_default();
        let bw = FB_DASH_BW_RE
            .captures(attrs)
            .and_then(|c| c[1].parse::<u64>().ok())
            .unwrap_or(0);

        if mime.contains("audio/mp4") || codecs.contains("mp4a") {
            let replace = dash_audio.as_ref().map(|(old_bw, _)| bw >= *old_bw).unwrap_or(true);
            if replace {
                *dash_audio = Some((bw, raw_url));
            }
        } else if (mime.contains("video/mp4") || codecs.contains("avc1"))
            && !codecs.contains("vp9")
            && !codecs.contains("av01")
        {
            let h = FB_DASH_HEIGHT_RE
                .captures(attrs)
                .and_then(|c| c[1].parse::<u32>().ok())
                .unwrap_or_else(|| fb_video_height_from_url(&raw_url, 720));
            if (144..=2160).contains(&h) {
                dash_videos.push(DirectVideoVariant {
                    url: raw_url,
                    height: h,
                    bitrate: bw,
                });
            }
        }
    }
}

static HTML_NUM_ENTITY_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"&#(?:x([0-9A-Fa-f]{1,6})|([0-9]{1,7}));").unwrap());

fn unescape_html_entities(raw: &str) -> String {
    let named = raw
        .replace("&amp;", "&")
        .replace("&#039;", "'")
        .replace("&apos;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&#xb7;", "·")
        .replace("&nbsp;", " ");
    HTML_NUM_ENTITY_RE
        .replace_all(&named, |caps: &regex::Captures| {
            let code = if let Some(hex) = caps.get(1) {
                u32::from_str_radix(hex.as_str(), 16).ok()
            } else if let Some(dec) = caps.get(2) {
                dec.as_str().parse::<u32>().ok()
            } else {
                None
            };
            code.and_then(char::from_u32)
                .map(|c| c.to_string())
                .unwrap_or_else(|| caps[0].to_string())
        })
        .into_owned()
}

fn extract_meta_tag(html: &str, r1: &Regex, r2: &Regex) -> Option<String> {
    let raw = r1
        .captures(html)
        .or_else(|| r2.captures(html))
        .and_then(|c| c.get(1))
        .map(|m| unescape_html_entities(m.as_str()))?;
    clean_text(Some(&raw), 240)
}

fn fb_image_dedup_key(url_str: &str) -> String {
    if let Ok(parsed) = Url::parse(url_str) {
        if let Some(seg) = parsed.path().rsplit('/').next() {
            if !seg.is_empty() {
                return seg.to_lowercase();
            }
        }
    }
    url_str.to_string()
}

fn fb_video_height_from_url(url_str: &str, fallback: u32) -> u32 {
    if let Ok(parsed) = Url::parse(url_str) {
        if let Some((_, efg)) = parsed.query_pairs().find(|(k, _)| k == "efg") {
            if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(efg.as_bytes()) {
                let s = String::from_utf8_lossy(&decoded);
                if let Some(cap) = FB_EFG_RES_RE.captures(&s) {
                    if let Ok(h) = cap[1].parse::<u32>() {
                        if (144..=2160).contains(&h) {
                            return h;
                        }
                    }
                }
            }
        }
    }
    fallback
}

fn fb_image_is_small_preview(url_str: &str) -> bool {
    url_str.contains("ctp=p180x")
        || url_str.contains("ctp=s150x")
        || url_str.contains("s150x150")
        || url_str.contains("s160x160")
        || url_str.contains("p50x50")
        || url_str.contains("p100x100")
        || url_str.contains("/t51.2885-19/")
        || url_str.contains("/t39.30808-1/")
}

fn fb_walk_relay(
    val: &Value,
    videos: &mut Vec<DirectVideoVariant>,
    dash_videos: &mut Vec<DirectVideoVariant>,
    dash_audio: &mut Option<(u64, String)>,
    curr_images: &mut Vec<(String, Option<u32>, Option<u32>)>,
    album_images: &mut Vec<(String, Option<u32>, Option<u32>)>,
    post_images: &mut Vec<(String, Option<u32>, Option<u32>)>,
    duration_ms: &mut Option<f64>,
    thumb: &mut Option<String>,
    caption: &mut Option<String>,
    uploader: &mut Option<String>,
    media_id: &mut Option<String>,
) {
    match val {
        Value::Object(map) => {
            if duration_ms.is_none() {
                if let Some(ms) = map.get("playable_duration_in_ms").and_then(Value::as_f64) {
                    if ms.is_finite() && ms > 0.0 {
                        *duration_ms = Some(ms);
                    }
                }
            }
            if thumb.is_none() {
                if let Some(u) = val
                    .pointer("/preferred_thumbnail/image/uri")
                    .and_then(Value::as_str)
                    .and_then(|s| validate_image_url(Some(s)))
                {
                    *thumb = Some(u);
                }
            }

            // Direct video URLs (HD + SD progressive MP4, including modern progressive_url / hd_src)
            for (key, fallback_h) in [
                ("browser_native_hd_url", 720u32),
                ("playable_url_quality_hd", 720u32),
                ("hd_src_no_ratelimit", 720u32),
                ("hd_src", 720u32),
                ("progressive_url", 720u32),
                ("browser_native_sd_url", 480u32),
                ("playable_url", 480u32),
                ("sd_src_no_ratelimit", 480u32),
                ("sd_src", 480u32),
            ] {
                if let Some(u) = map.get(key).and_then(Value::as_str) {
                    if is_meta_cdn_video_url(u) {
                        let meta_h = val
                            .pointer("/metadata/quality")
                            .and_then(Value::as_str)
                            .map(|q| if q.eq_ignore_ascii_case("HD") { 720 } else { 480 })
                            .unwrap_or(fallback_h);
                        let h = fb_video_height_from_url(u, meta_h);
                        videos.push(DirectVideoVariant {
                            url: u.to_string(),
                            height: h,
                            bitrate: 0,
                        });
                        if media_id.is_none() {
                            *media_id = map
                                .get("id")
                                .and_then(Value::as_str)
                                .map(String::from);
                        }
                    }
                }
            }

            // DASH XML manifest (often carries 1080p adaptive H.264 + AAC audio)
            for dash_key in [
                "dash_manifest",
                "playable_dash_manifest",
                "dash_manifest_xml_string",
            ] {
                if let Some(xml) = map.get(dash_key).and_then(Value::as_str) {
                    if xml.contains("<Representation") {
                        parse_facebook_dash_xml(xml, dash_videos, dash_audio);
                    }
                }
            }

            // Single photo viewer (/photo/?fbid=...)
            if let Some(cm) = map.get("currMedia").and_then(Value::as_object) {
                if let Some(u) = cm
                    .get("image")
                    .and_then(|i| i.get("uri"))
                    .and_then(Value::as_str)
                {
                    let w = cm
                        .get("image")
                        .and_then(|i| i.get("width"))
                        .and_then(Value::as_u64)
                        .map(|x| x as u32);
                    let h = cm
                        .get("image")
                        .and_then(|i| i.get("height"))
                        .and_then(Value::as_u64)
                        .map(|x| x as u32);
                    curr_images.push((u.to_string(), w, h));
                    if media_id.is_none() {
                        *media_id = cm.get("id").and_then(Value::as_str).map(String::from);
                    }
                    if caption.is_none() {
                        *caption = clean_text(
                            cm.get("message")
                                .and_then(|m| m.get("text"))
                                .and_then(Value::as_str)
                                .or_else(|| {
                                    cm.get("accessibility_caption").and_then(Value::as_str)
                                }),
                            240,
                        );
                    }
                    if uploader.is_none() {
                        *uploader = clean_text(
                            cm.get("owner")
                                .and_then(|o| o.get("name"))
                                .and_then(Value::as_str),
                            120,
                        );
                    }
                }
            }

            // Multi-photo album attachments (all_subattachments.nodes)
            if let Some(nodes) = val
                .pointer("/all_subattachments/nodes")
                .and_then(Value::as_array)
            {
                for node in nodes {
                    let media = node.get("media").unwrap_or(node);
                    let img_obj = media
                        .get("photo_image")
                        .or_else(|| media.get("viewer_image"))
                        .or_else(|| media.get("image"));
                    if let Some(u) = img_obj
                        .and_then(|i| i.get("uri"))
                        .and_then(Value::as_str)
                    {
                        let w = img_obj
                            .and_then(|i| i.get("width"))
                            .and_then(Value::as_u64)
                            .map(|x| x as u32);
                        let h = img_obj
                            .and_then(|i| i.get("height"))
                            .and_then(Value::as_u64)
                            .map(|x| x as u32);
                        album_images.push((u.to_string(), w, h));
                    }
                }
            }

            // Single post photo (photo_image / viewer_image)
            for key in ["photo_image", "viewer_image"] {
                if let Some(img_obj) = map.get(key).and_then(Value::as_object) {
                    if let Some(u) = img_obj.get("uri").and_then(Value::as_str) {
                        let w = img_obj
                            .get("width")
                            .and_then(Value::as_u64)
                            .map(|x| x as u32);
                        let h = img_obj
                            .get("height")
                            .and_then(Value::as_u64)
                            .map(|x| x as u32);
                        post_images.push((u.to_string(), w, h));
                    }
                }
            }

            for v in map.values() {
                fb_walk_relay(
                    v,
                    videos,
                    dash_videos,
                    dash_audio,
                    curr_images,
                    album_images,
                    post_images,
                    duration_ms,
                    thumb,
                    caption,
                    uploader,
                    media_id,
                );
            }
        }
        Value::Array(arr) => {
            for v in arr {
                fb_walk_relay(
                    v,
                    videos,
                    dash_videos,
                    dash_audio,
                    curr_images,
                    album_images,
                    post_images,
                    duration_ms,
                    thumb,
                    caption,
                    uploader,
                    media_id,
                );
            }
        }
        _ => {}
    }
}

pub fn parse_facebook_html(html: &str, url_str: &str) -> Option<FacebookMediaResult> {
    let mut raw_videos = Vec::new();
    let mut raw_dash_videos = Vec::new();
    let mut dash_audio: Option<(u64, String)> = None;
    let mut curr_images = Vec::new();
    let mut album_images = Vec::new();
    let mut post_images = Vec::new();
    let mut duration_ms = None;
    let mut thumb = None;
    let mut caption = None;
    let mut uploader = None;
    let mut media_id = None;

    for cap in JSON_SCRIPT_RE.captures_iter(html) {
        let s = &cap[1];
        if !s.contains("browser_native_")
            && !s.contains("playable_url")
            && !s.contains("progressive_url")
            && !s.contains("dash_manifest")
            && !s.contains("hd_src")
            && !s.contains("sd_src")
            && !s.contains("currMedia")
            && !s.contains("all_subattachments")
            && !s.contains("photo_image")
            && !s.contains("viewer_image")
        {
            continue;
        }
        let Ok(json) = serde_json::from_str::<Value>(s) else {
            continue;
        };
        fb_walk_relay(
            &json,
            &mut raw_videos,
            &mut raw_dash_videos,
            &mut dash_audio,
            &mut curr_images,
            &mut album_images,
            &mut post_images,
            &mut duration_ms,
            &mut thumb,
            &mut caption,
            &mut uploader,
            &mut media_id,
        );
    }

    // Deduplicate videos by URL, sorted highest resolution first
    let mut seen_v = HashSet::new();
    let mut video_variants: Vec<DirectVideoVariant> = raw_videos
        .into_iter()
        .filter(|v| seen_v.insert(v.url.clone()))
        .collect();
    video_variants.sort_by(|a, b| b.height.cmp(&a.height));

    let dash_audio_url = dash_audio.map(|(_, u)| u);
    let mut seen_dv = HashSet::new();
    let mut dash_videos: Vec<DirectVideoVariant> = if dash_audio_url.is_some() {
        raw_dash_videos
            .into_iter()
            .filter(|v| seen_dv.insert(v.url.clone()))
            .collect()
    } else {
        Vec::new()
    };
    dash_videos.sort_by(|a, b| b.height.cmp(&a.height).then_with(|| b.bitrate.cmp(&a.bitrate)));

    // Extract OG metadata
    let og_title = extract_meta_tag(html, &OG_TITLE_1, &OG_TITLE_2);
    let og_desc = extract_meta_tag(html, &OG_DESC_1, &OG_DESC_2);
    let og_img = OG_IMAGE_1
        .captures(html)
        .or_else(|| OG_IMAGE_2.captures(html))
        .and_then(|c| c.get(1))
        .map(|m| unescape_html_entities(m.as_str()))
        .and_then(|u| validate_image_url(Some(&u)));

    // Choose best non-duplicate photos (skip when this is a pure video post)
    let mut chosen_raw: Vec<(String, Option<u32>, Option<u32>)> = Vec::new();
    if video_variants.is_empty() && dash_videos.is_empty() {
        let mut by_key: HashMap<String, (usize, String, Option<u32>, Option<u32>, bool)> =
            HashMap::new();
        let mut order = 0usize;

        let mut consider = |u: String, w: Option<u32>, h: Option<u32>| {
            let Some(safe) = validate_image_url(Some(&u)) else {
                return;
            };
            if safe.contains("static.xx.fbcdn.net") || safe.contains("/rsrc.php/") {
                return;
            }
            let key = fb_image_dedup_key(&safe);
            let is_small = fb_image_is_small_preview(&safe);
            let new_area = w.unwrap_or(0) as u64 * h.unwrap_or(0) as u64;
            match by_key.get(&key) {
                None => {
                    by_key.insert(key, (order, safe, w, h, is_small));
                    order += 1;
                }
                Some((ord, _, ew, eh, old_small)) => {
                    let old_area = ew.unwrap_or(0) as u64 * eh.unwrap_or(0) as u64;
                    if (*old_small && !is_small)
                        || (*old_small == is_small && new_area > old_area)
                    {
                        let keep_ord = *ord;
                        by_key.insert(key, (keep_ord, safe, w, h, is_small));
                    }
                }
            }
        };

        if !curr_images.is_empty() {
            for (u, w, h) in curr_images {
                consider(u, w, h);
            }
        } else if !album_images.is_empty() {
            for (u, w, h) in album_images {
                consider(u, w, h);
            }
        } else {
            // On post pages, og:image is often full-res while photo_image is a 540px preview of the same file
            if let Some(ref oi) = og_img {
                consider(oi.clone(), None, None);
            }
            for (u, w, h) in post_images {
                consider(u, w, h);
            }
        }

        let mut ordered: Vec<_> = by_key.into_values().collect();
        ordered.sort_by_key(|x| x.0);
        for (_, u, w, h, _) in ordered {
            chosen_raw.push((u, w, h));
        }
    }

    let images = normalize_items(chosen_raw);
    if video_variants.is_empty() && dash_videos.is_empty() && images.is_empty() {
        return None;
    }

    let cleaned_og_title = og_title
        .as_deref()
        .map(|t| FB_REACTIONS_PREFIX.replace(t, "").trim().to_string())
        .filter(|t| !t.is_empty() && !t.eq_ignore_ascii_case("Facebook"));

    let title = caption
        .or(og_desc)
        .or(cleaned_og_title.clone())
        .or_else(|| {
            Some(if !video_variants.is_empty() || !dash_videos.is_empty() {
                "Facebook Video".into()
            } else {
                "Facebook Photo".into()
            })
        });

    let final_uploader = uploader.or_else(|| {
        og_title
            .filter(|t| !t.contains(" views ") && !t.eq_ignore_ascii_case("Facebook") && t.len() <= 80)
    });

    let thumbnail_url = thumb
        .or(og_img)
        .or_else(|| images.first().map(|i| i.url.clone()));

    let final_id = media_id.or_else(|| {
        Url::parse(url_str).ok().and_then(|u| {
            u.query_pairs()
                .find(|(k, _)| matches!(k.as_ref(), "v" | "fbid" | "story_fbid" | "video_id"))
                .map(|(_, v)| v.into_owned())
        })
    });

    Some(FacebookMediaResult {
        title,
        uploader: final_uploader,
        media_id: final_id,
        duration_seconds: duration_ms.map(|ms| ms / 1000.0),
        thumbnail_url,
        video_variants,
        dash_videos,
        dash_audio_url,
        images,
    })
}

pub async fn fetch_facebook_media(url_str: &str) -> Option<FacebookMediaResult> {
    let parsed = Url::parse(url_str).ok()?;
    let mut canonical = format!("https://www.facebook.com{}", parsed.path());
    if let Some(q) = parsed.query() {
        canonical.push('?');
        canonical.push_str(q);
    }

    let nav_headers = [
        (
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        ),
        ("Sec-Fetch-Mode", "navigate"),
        ("Sec-Fetch-Dest", "document"),
        ("Sec-Fetch-Site", "none"),
    ];

    // Follow up to 5 validated same-platform redirects (e.g. /watch/?v=... -> /Page/videos/.../)
    let mut current = canonical;
    for _ in 0..5 {
        ensure_public_host(&current).await.ok()?;
        let mut req = SHARED_HTTP
            .get(&current)
            .timeout(Duration::from_secs(15))
            .header("Accept-Language", "en-US,en;q=0.9");
        for (k, v) in nav_headers {
            req = req.header(k, v);
        }
        let mut resp = req.send().await.ok()?;
        let status = resp.status().as_u16();
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            let loc = resp.headers().get("location")?.to_str().ok()?;
            let base = Url::parse(&current).ok()?;
            let joined = base.join(loc).ok()?;
            let (next_url, next_plat) = crate::url_policy::validate_url(joined.as_str()).ok()?;
            if next_plat != "facebook" {
                return None;
            }
            // Avoid bouncing into Facebook's login wall
            if Url::parse(&next_url)
                .ok()
                .is_some_and(|u| u.path().starts_with("/login"))
            {
                return None;
            }
            current = next_url;
            continue;
        }
        if !(200..300).contains(&status) {
            return None;
        }
        let mut buf = Vec::new();
        while let Some(chunk) = resp.chunk().await.ok()? {
            if buf.len() + chunk.len() > MAX_HTML_BYTES {
                return None;
            }
            buf.extend_from_slice(&chunk);
        }
        let text = String::from_utf8_lossy(&buf);
        return parse_facebook_html(&text, &current);
    }
    None
}

async fn extract_facebook(url_str: &str) -> Option<ExtractedPhotoSet> {
    let res = fetch_facebook_media(url_str).await?;
    if res.images.is_empty() {
        return None;
    }
    Some(ExtractedPhotoSet {
        title: res.title,
        uploader: res.uploader,
        media_id: res.media_id,
        images: res.images,
    })
}

fn parse_instagram_embed(html: &str, shortcode: &str) -> Option<ExtractedPhotoSet> {
    let mut raw = Vec::new();
    let mut title = None;
    let mut uploader = None;
    if let Some(cap) = IG_INIT_RE.captures(html) {
        if let Ok(embed) = serde_json::from_str::<Value>(&cap[1]) {
            if let Some(ctx_str) = embed.get("contextJSON").and_then(Value::as_str) {
                if let Ok(ctx) = serde_json::from_str::<Value>(ctx_str) {
                    let media = ctx
                        .pointer("/gql_data/shortcode_media")
                        .or_else(|| ctx.get("media"));
                    if let Some(m) = media {
                        uploader = clean_text(
                            m.pointer("/owner/username")
                                .or_else(|| m.pointer("/owner/full_name"))
                                .and_then(Value::as_str),
                            120,
                        );
                        title = clean_text(
                            m.pointer("/edge_media_to_caption/edges/0/node/text")
                                .and_then(Value::as_str),
                            240,
                        );
                        if let Some(edges) = m
                            .pointer("/edge_sidecar_to_children/edges")
                            .and_then(Value::as_array)
                        {
                            for e in edges {
                                if let Some(node) = e.get("node") {
                                    if let Some(img) = ig_best_image(node) {
                                        raw.push(img);
                                    }
                                }
                            }
                        } else if let Some(img) = ig_best_image(m) {
                            raw.push(img);
                        }
                    }
                }
            }
        }
    }
    if raw.is_empty() {
        for cap in IG_EMBED_IMG_RE.captures_iter(html) {
            let unescaped = cap[1].replace("&amp;", "&");
            raw.push((unescaped, None, None));
        }
    }
    let images = normalize_items(raw);
    if images.is_empty() {
        return None;
    }
    Some(ExtractedPhotoSet {
        title: title.or_else(|| Some(format!("Instagram {shortcode}"))),
        uploader,
        media_id: Some(shortcode.to_string()),
        images,
    })
}

async fn extract_instagram(url_str: &str) -> Option<ExtractedPhotoSet> {
    let re = Regex::new(r"/(?:p|reel|tv)/([A-Za-z0-9_-]+)").ok()?;
    let path = Url::parse(url_str).ok()?.path().to_string();
    let sc = re.captures(&path)?.get(1)?.as_str().to_string();
    let nav_headers = [
        (
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        ),
        ("Sec-Fetch-Mode", "navigate"),
    ];
    if let Ok(bytes) = fetch_bytes(
        &format!("https://www.instagram.com/p/{sc}/"),
        &nav_headers,
        MAX_HTML_BYTES,
        15,
    )
    .await
    {
        let text = String::from_utf8_lossy(&bytes);
        if let Some(found) =
            parse_instagram_polaris(&text, &sc).or_else(|| parse_instagram_embed(&text, &sc))
        {
            return Some(found);
        }
    }
    let bytes = fetch_bytes(
        &format!("https://www.instagram.com/p/{sc}/embed/captioned/"),
        &nav_headers,
        MAX_HTML_BYTES,
        15,
    )
    .await
    .ok()?;
    let text = String::from_utf8_lossy(&bytes);
    parse_instagram_embed(&text, &sc)
}

#[derive(Debug, Clone)]
pub struct PinterestMediaResult {
    pub title: Option<String>,
    pub media_id: String,
    pub thumbnail_url: Option<String>,
    pub video_variants: Vec<DirectVideoVariant>,
    pub images: Vec<ImageItem>,
}

pub fn parse_pinterest_html(text: &str, pin_id: &str) -> Option<PinterestMediaResult> {
    let mut seen_v = HashSet::new();
    let mut video_variants = Vec::new();
    for m in PIN_VIDEO_RE.find_iter(text) {
        let u = m.as_str().to_string();
        if !seen_v.insert(u.clone()) {
            continue;
        }
        let h = PIN_DIM_RE
            .captures(&u)
            .and_then(|c| c[1].parse::<u32>().ok())
            .unwrap_or(720);
        video_variants.push(DirectVideoVariant {
            url: u,
            height: h,
            bitrate: 0,
        });
    }
    video_variants.sort_by(|a, b| b.height.cmp(&a.height));

    let img_link = PIN_ORIG_RE
        .find(text)
        .or_else(|| PIN_FALLBACK_RE.find(text))
        .map(|m| m.as_str().to_string());
    let images = match img_link {
        Some(link) => normalize_items(vec![(link, None, None)]),
        None => Vec::new(),
    };
    if video_variants.is_empty() && images.is_empty() {
        return None;
    }
    let title = OG_TITLE_1
        .captures(text)
        .or_else(|| OG_TITLE_2.captures(text))
        .and_then(|c| clean_text(Some(&c[1]), 240))
        .or_else(|| Some(format!("Pinterest Pin {pin_id}")));
    let thumbnail_url = images.first().map(|i| i.url.clone());
    Some(PinterestMediaResult {
        title,
        media_id: pin_id.to_string(),
        thumbnail_url,
        video_variants,
        images,
    })
}

pub async fn fetch_pinterest_media(url_str: &str) -> Option<PinterestMediaResult> {
    let re = Regex::new(r"/pin/(?:[^/]*--)?(\d+)").ok()?;
    let path = Url::parse(url_str).ok()?.path().to_string();
    let pin_id = re.captures(&path)?.get(1)?.as_str().to_string();
    let bytes = fetch_bytes(
        &format!("https://www.pinterest.com/pin/{pin_id}/"),
        &[],
        MAX_HTML_BYTES,
        15,
    )
    .await
    .ok()?;
    let text = String::from_utf8_lossy(&bytes);
    parse_pinterest_html(&text, &pin_id)
}

async fn extract_pinterest(url_str: &str) -> Option<ExtractedPhotoSet> {
    let res = fetch_pinterest_media(url_str).await?;
    if res.images.is_empty() {
        return None;
    }
    Some(ExtractedPhotoSet {
        title: res.title,
        uploader: None,
        media_id: Some(res.media_id),
        images: res.images,
    })
}

fn float_to_base36(value: f64) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let int_part = value.trunc() as u64;
    let mut frac = value - (int_part as f64);
    let mut int_digits = Vec::new();
    let mut n = int_part;
    if n == 0 {
        int_digits.push('0');
    } else {
        while n > 0 {
            int_digits.push(DIGITS[(n % 36) as usize] as char);
            n /= 36;
        }
        int_digits.reverse();
    }
    let mut out: String = int_digits.into_iter().collect();
    if frac > 0.0 {
        out.push('.');
        for _ in 0..11 {
            frac *= 36.0;
            let d = frac.trunc() as usize;
            if d < 36 {
                out.push(DIGITS[d] as char);
            }
            frac -= d as f64;
            if frac <= 1e-12 {
                break;
            }
        }
    }
    out
}

fn twitter_syndication_token(tweet_id: &str) -> Option<String> {
    let id_num: f64 = tweet_id.parse().ok()?;
    let val = (id_num / 1e15) * std::f64::consts::PI;
    let s = float_to_base36(val);
    Some(s.chars().filter(|&c| c != '0' && c != '.').collect())
}

#[derive(Debug, Clone)]
pub struct DirectVideoVariant {
    pub url: String,
    pub height: u32,
    pub bitrate: u64,
}

#[derive(Debug, Clone)]
pub struct TwitterSyndicationResult {
    pub title: Option<String>,
    pub uploader: Option<String>,
    pub media_id: String,
    pub duration_seconds: Option<f64>,
    pub thumbnail_url: Option<String>,
    pub video_variants: Vec<DirectVideoVariant>,
    pub images: Vec<ImageItem>,
    #[allow(dead_code)]
    pub quoted_tweet_url: Option<String>,
}

fn parse_mp4_variants_from_media(media_list: &[Value]) -> (Vec<DirectVideoVariant>, Option<f64>, Option<String>) {
    static DIM_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"/(\d{2,4})x(\d{2,4})/").unwrap());
    let mut variants = Vec::new();
    let mut duration = None;
    let mut thumb = None;
    for m in media_list {
        let mtype = m.get("type").and_then(Value::as_str).unwrap_or("");
        if mtype != "video" && mtype != "animated_gif" {
            continue;
        }
        if thumb.is_none() {
            thumb = validate_image_url(m.get("media_url_https").and_then(Value::as_str));
        }
        if duration.is_none() {
            duration = m
                .pointer("/video_info/duration_millis")
                .and_then(Value::as_f64)
                .map(|ms| ms / 1000.0);
        }
        let orig_h = m.pointer("/original_info/height").and_then(Value::as_u64).unwrap_or(0);
        let orig_w = m.pointer("/original_info/width").and_then(Value::as_u64).unwrap_or(0);
        if let Some(arr) = m.pointer("/video_info/variants").and_then(Value::as_array) {
            for v in arr {
                if v.get("content_type").and_then(Value::as_str) != Some("video/mp4") {
                    continue;
                }
                let Some(u) = v.get("url").and_then(Value::as_str) else {
                    continue;
                };
                let Some(safe_u) = validate_video_cdn_url(u, "x") else {
                    continue;
                };
                let bitrate = v.get("bitrate").and_then(Value::as_u64).unwrap_or(0);
                let height = if let Some(cap) = DIM_RE.captures(&safe_u) {
                    let w: u32 = cap[1].parse().unwrap_or(0);
                    let h: u32 = cap[2].parse().unwrap_or(0);
                    if w > 0 && h > 0 { w.min(h) } else { h.max(w) }
                } else {
                    orig_h.min(orig_w) as u32
                };
                variants.push(DirectVideoVariant {
                    url: safe_u,
                    height,
                    bitrate,
                });
            }
        }
        if !variants.is_empty() {
            break;
        }
    }
    variants.sort_by(|a, b| b.height.cmp(&a.height).then_with(|| b.bitrate.cmp(&a.bitrate)));
    (variants, duration, thumb)
}

fn parse_mp4_variants_from_video_obj(
    tweet: &Value,
) -> (Vec<DirectVideoVariant>, Option<f64>, Option<String>) {
    static DIM_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"/(\d{2,4})x(\d{2,4})/").unwrap());
    let Some(video) = tweet.get("video").and_then(Value::as_object) else {
        return (Vec::new(), None, None);
    };
    let thumb = validate_image_url(video.get("poster").and_then(Value::as_str));
    let duration = video
        .get("durationMs")
        .and_then(Value::as_f64)
        .map(|ms| ms / 1000.0);
    let mut variants = Vec::new();
    if let Some(arr) = video.get("variants").and_then(Value::as_array) {
        for v in arr {
            let content_type = v
                .get("type")
                .or_else(|| v.get("content_type"))
                .and_then(Value::as_str);
            if content_type != Some("video/mp4") {
                continue;
            }
            let Some(u) = v.get("src").or_else(|| v.get("url")).and_then(Value::as_str) else {
                continue;
            };
            let Some(safe_u) = validate_video_cdn_url(u, "x") else {
                continue;
            };
            let bitrate = v.get("bitrate").and_then(Value::as_u64).unwrap_or(0);
            let height = if let Some(cap) = DIM_RE.captures(&safe_u) {
                let w: u32 = cap[1].parse().unwrap_or(0);
                let h: u32 = cap[2].parse().unwrap_or(0);
                if w > 0 && h > 0 { w.min(h) } else { h.max(w) }
            } else {
                0
            };
            variants.push(DirectVideoVariant {
                url: safe_u,
                height,
                bitrate,
            });
        }
    }
    variants.sort_by(|a, b| b.height.cmp(&a.height).then_with(|| b.bitrate.cmp(&a.bitrate)));
    (variants, duration, thumb)
}

fn collect_twitter_photos(tweet: &Value) -> Vec<(String, Option<u32>, Option<u32>)> {
    let mut raw = Vec::new();
    if let Some(details) = tweet.get("mediaDetails").and_then(Value::as_array) {
        for m in details {
            if m.get("type").and_then(Value::as_str) == Some("photo") {
                if let Some(u) = m.get("media_url_https").and_then(Value::as_str) {
                    let full = if u.contains('?') {
                        u.to_string()
                    } else {
                        format!("{u}?name=orig")
                    };
                    let w = m.pointer("/original_info/width").and_then(Value::as_u64).map(|x| x as u32);
                    let h = m.pointer("/original_info/height").and_then(Value::as_u64).map(|x| x as u32);
                    raw.push((full, w, h));
                }
            }
        }
    }
    if raw.is_empty() {
        if let Some(photos) = tweet.get("photos").and_then(Value::as_array) {
            for p in photos {
                if let Some(u) = p.get("url").and_then(Value::as_str) {
                    let w = p.get("width").and_then(Value::as_u64).map(|x| x as u32);
                    let h = p.get("height").and_then(Value::as_u64).map(|x| x as u32);
                    raw.push((u.to_string(), w, h));
                }
            }
        }
    }
    raw
}

pub fn parse_twitter_syndication(data: &Value, tweet_id: &str) -> Option<TwitterSyndicationResult> {
    let main_media = data
        .get("mediaDetails")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let quoted = data.get("quoted_tweet");
    let quoted_media = quoted
        .and_then(|q| q.get("mediaDetails"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);

    let (mut video_variants, mut duration_seconds, mut video_thumb) =
        parse_mp4_variants_from_media(main_media);
    if video_variants.is_empty() {
        let (vv, vd, vt) = parse_mp4_variants_from_video_obj(data);
        if !vv.is_empty() {
            video_variants = vv;
            duration_seconds = duration_seconds.or(vd);
            video_thumb = video_thumb.or(vt);
        }
    }
    let mut from_quoted = false;
    if video_variants.is_empty() {
        if !quoted_media.is_empty() {
            let (qv, qd, qt) = parse_mp4_variants_from_media(quoted_media);
            if !qv.is_empty() {
                video_variants = qv;
                duration_seconds = qd;
                video_thumb = qt;
                from_quoted = true;
            }
        }
        if video_variants.is_empty() {
            if let Some(q) = quoted {
                let (qv, qd, qt) = parse_mp4_variants_from_video_obj(q);
                if !qv.is_empty() {
                    video_variants = qv;
                    duration_seconds = qd;
                    video_thumb = qt;
                    from_quoted = true;
                }
            }
        }
    }

    let mut photo_raw = collect_twitter_photos(data);
    if photo_raw.is_empty() {
        if let Some(q) = quoted {
            photo_raw = collect_twitter_photos(q);
        }
    }
    let images = normalize_items(photo_raw);

    if video_variants.is_empty() && images.is_empty() {
        return None;
    }

    let quoted_tweet_url = if from_quoted {
        quoted.and_then(|q| {
            let qid = q.get("id_str").and_then(Value::as_str)?;
            let quser = q
                .pointer("/user/screen_name")
                .and_then(Value::as_str)
                .unwrap_or("i");
            Some(format!("https://x.com/{quser}/status/{qid}"))
        })
    } else {
        None
    };

    let thumbnail_url = video_thumb.or_else(|| images.first().map(|i| i.url.clone()));
    let title = clean_text(data.get("text").and_then(Value::as_str), 240)
        .or_else(|| {
            quoted.and_then(|q| clean_text(q.get("text").and_then(Value::as_str), 240))
        })
        .or_else(|| Some(format!("Post {tweet_id}")));
    let uploader = clean_text(
        data.pointer("/user/name")
            .or_else(|| data.pointer("/user/screen_name"))
            .and_then(Value::as_str),
        120,
    );

    Some(TwitterSyndicationResult {
        title,
        uploader,
        media_id: tweet_id.to_string(),
        duration_seconds,
        thumbnail_url,
        video_variants,
        images,
        quoted_tweet_url,
    })
}

pub async fn fetch_twitter_syndication(url_str: &str) -> Option<TwitterSyndicationResult> {
    let re = Regex::new(r"/status/(\d+)").ok()?;
    let path = Url::parse(url_str).ok()?.path().to_string();
    let tweet_id = re.captures(&path)?.get(1)?.as_str().to_string();
    let token = twitter_syndication_token(&tweet_id)?;
    let endpoint = format!("https://cdn.syndication.twimg.com/tweet-result?id={tweet_id}&token={token}");
    let bytes = fetch_bytes(
        &endpoint,
        &[("Accept", "application/json"), ("User-Agent", "Googlebot")],
        MAX_HTML_BYTES,
        12,
    )
    .await
    .ok()?;
    let data: Value = serde_json::from_slice(&bytes).ok()?;
    parse_twitter_syndication(&data, &tweet_id)
}

async fn extract_twitter(url_str: &str) -> Option<ExtractedPhotoSet> {
    let res = fetch_twitter_syndication(url_str).await?;
    if res.images.is_empty() {
        return None;
    }
    Some(ExtractedPhotoSet {
        title: res.title,
        uploader: res.uploader,
        media_id: Some(res.media_id),
        images: res.images,
    })
}

// ============================================================================
// 100% Pure-Rust YouTube InnerTube Extractor (ANDROID_VR + IOS direct streams)
// ============================================================================

#[derive(Debug, Clone)]
pub struct YouTubeMediaResult {
    pub video_id: String,
    pub title: Option<String>,
    pub uploader: Option<String>,
    pub duration_seconds: Option<f64>,
    pub thumbnail_url: Option<String>,
    pub video_variants: Vec<DirectVideoVariant>,
    pub audio_url: Option<String>,
    #[allow(dead_code)]
    pub audio_bitrate: u64,
    pub client_ua: String,
    pub has_audio: bool,
}

static YT_ID_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[A-Za-z0-9_-]{11}$").unwrap());

pub fn extract_youtube_id(url_str: &str) -> Option<String> {
    let parsed = Url::parse(url_str).ok()?;
    let host = parsed.host_str()?.to_lowercase();
    if host == "youtu.be" {
        let seg = parsed.path().trim_matches('/').split('/').next()?;
        return YT_ID_RE.is_match(seg).then(|| seg.to_string());
    }
    let path = parsed.path();
    if path == "/watch" {
        for (k, v) in parsed.query_pairs() {
            if k == "v" && YT_ID_RE.is_match(&v) {
                return Some(v.into_owned());
            }
        }
    }
    for prefix in ["/shorts/", "/embed/", "/live/"] {
        if let Some(rest) = path.strip_prefix(prefix) {
            let seg = rest.split('/').next().unwrap_or("");
            if YT_ID_RE.is_match(seg) {
                return Some(seg.to_string());
            }
        }
    }
    None
}

pub fn parse_youtube_innertube_response(
    data: &Value,
    video_id: &str,
    client_ua: &str,
) -> Result<YouTubeMediaResult, AppError> {
    let status = data
        .pointer("/playabilityStatus/status")
        .and_then(Value::as_str)
        .unwrap_or("");
    let reason = data
        .pointer("/playabilityStatus/reason")
        .and_then(Value::as_str)
        .unwrap_or("");
    if status != "OK" {
        if status == "LOGIN_REQUIRED" {
            return Err(AppError::new("LOGIN_REQUIRED"));
        }
        if !reason.is_empty() {
            return Err(classify_extractor_error(reason));
        }
        return Err(AppError::new("UNAVAILABLE_MEDIA"));
    }

    let vd = data.get("videoDetails");
    let title = clean_text(
        vd.and_then(|v| v.get("title")).and_then(Value::as_str),
        240,
    );
    let uploader = clean_text(
        vd.and_then(|v| v.get("author")).and_then(Value::as_str),
        120,
    );
    let duration_seconds = vd
        .and_then(|v| v.get("lengthSeconds"))
        .and_then(|v| v.as_str().and_then(|s| s.parse::<f64>().ok()).or_else(|| v.as_f64()))
        .filter(|d| d.is_finite() && *d >= 0.0);
    let thumbnail_url = validate_image_url(
        vd.and_then(|v| v.pointer("/thumbnail/thumbnails"))
            .and_then(Value::as_array)
            .and_then(|a| a.last())
            .and_then(|t| t.get("url"))
            .and_then(Value::as_str),
    )
    .or_else(|| validate_image_url(Some(&format!("https://i.ytimg.com/vi/{video_id}/hqdefault.jpg"))));

    let mut adaptive_avc: Vec<DirectVideoVariant> = Vec::new();
    let mut progressive_mp4: Vec<DirectVideoVariant> = Vec::new();
    let mut best_audio: Option<(u64, String)> = None;

    let collect_arr = |ptr: &str| -> Vec<&Value> {
        data.pointer(ptr)
            .and_then(Value::as_array)
            .map(|a| a.iter().collect())
            .unwrap_or_default()
    };

    for fmt in collect_arr("/streamingData/formats") {
        let Some(u) = fmt.get("url").and_then(Value::as_str) else {
            continue;
        };
        let Some(safe_u) = validate_video_cdn_url(u, "youtube") else {
            continue;
        };
        let mime = fmt.get("mimeType").and_then(Value::as_str).unwrap_or("");
        if !mime.contains("video/mp4") {
            continue;
        }
        let h = fmt.get("height").and_then(Value::as_u64).unwrap_or(0) as u32;
        let w = fmt.get("width").and_then(Value::as_u64).unwrap_or(0) as u32;
        let dim = if w > 0 && h > 0 { w.min(h) } else { h.max(w) };
        let br = fmt.get("bitrate").and_then(Value::as_u64).unwrap_or(0);
        progressive_mp4.push(DirectVideoVariant {
            url: safe_u,
            height: dim,
            bitrate: br,
        });
    }

    for fmt in collect_arr("/streamingData/adaptiveFormats") {
        let Some(u) = fmt.get("url").and_then(Value::as_str) else {
            continue;
        };
        let Some(safe_u) = validate_video_cdn_url(u, "youtube") else {
            continue;
        };
        let mime = fmt.get("mimeType").and_then(Value::as_str).unwrap_or("");
        let br = fmt.get("bitrate").and_then(Value::as_u64).unwrap_or(0);
        if mime.starts_with("audio/mp4") && mime.contains("mp4a") {
            if best_audio.as_ref().is_none_or(|(b, _)| br > *b) {
                best_audio = Some((br, safe_u));
            }
        } else if mime.starts_with("video/mp4") && mime.contains("avc1") {
            let h = fmt.get("height").and_then(Value::as_u64).unwrap_or(0) as u32;
            let w = fmt.get("width").and_then(Value::as_u64).unwrap_or(0) as u32;
            let dim = if w > 0 && h > 0 { w.min(h) } else { h.max(w) };
            adaptive_avc.push(DirectVideoVariant {
                url: safe_u,
                height: dim,
                bitrate: br,
            });
        }
    }

    adaptive_avc.sort_by(|a, b| b.height.cmp(&a.height).then_with(|| b.bitrate.cmp(&a.bitrate)));
    progressive_mp4.sort_by(|a, b| b.height.cmp(&a.height).then_with(|| b.bitrate.cmp(&a.bitrate)));

    let (audio_bitrate, audio_url) = match best_audio {
        Some((b, u)) => (b, Some(u)),
        None => (0, None),
    };

    // Prefer high-res adaptive H.264 (muxed with AAC by our pure-Rust mp4_engine); fall back to progressive MP4.
    let video_variants = if !adaptive_avc.is_empty() && audio_url.is_some() {
        adaptive_avc
    } else if !progressive_mp4.is_empty() {
        progressive_mp4
    } else {
        adaptive_avc
    };

    if video_variants.is_empty() && audio_url.is_none() {
        return Err(AppError::new("FORMAT_UNAVAILABLE"));
    }

    Ok(YouTubeMediaResult {
        video_id: video_id.to_string(),
        title,
        uploader,
        duration_seconds,
        thumbnail_url,
        has_audio: audio_url.is_some() || !video_variants.is_empty(),
        video_variants,
        audio_url,
        audio_bitrate,
        client_ua: client_ua.to_string(),
    })
}

pub async fn fetch_youtube_innertube(url_str: &str) -> Result<YouTubeMediaResult, AppError> {
    let video_id = extract_youtube_id(url_str).ok_or_else(|| AppError::new("UNAVAILABLE_MEDIA"))?;
    let endpoint = "https://www.youtube.com/youtubei/v1/player?prettyPrint=false";
    ensure_public_host(endpoint).await?;

    let clients: &[(&str, Value)] = &[
        (
            "com.google.android.apps.youtube.vr.oculus/1.60.19 (Linux; U; Android 12L; eureka-user Build/SQ3A.220605.009.A1) gzip",
            serde_json::json!({
                "videoId": video_id,
                "context": {
                    "client": {
                        "clientName": "ANDROID_VR",
                        "clientVersion": "1.60.19",
                        "androidSdkVersion": 32,
                        "hl": "en",
                        "gl": "US"
                    }
                }
            }),
        ),
        (
            "com.google.ios.youtube/20.10.4 (iPhone16,2; U; CPU iOS 18_3_2 like Mac OS X;)",
            serde_json::json!({
                "videoId": video_id,
                "context": {
                    "client": {
                        "clientName": "IOS",
                        "clientVersion": "20.10.4",
                        "deviceMake": "Apple",
                        "deviceModel": "iPhone16,2",
                        "osName": "iPhone",
                        "osVersion": "18.3.2.22D82",
                        "hl": "en",
                        "gl": "US"
                    }
                }
            }),
        ),
    ];

    let mut last_err = AppError::new("EXTRACTOR_FAILED");
    for (ua, body) in clients {
        let Ok(resp) = SHARED_HTTP
            .post(endpoint)
            .header("User-Agent", *ua)
            .header("Content-Type", "application/json")
            .json(body)
            .timeout(Duration::from_secs(12))
            .send()
            .await
        else {
            last_err = AppError::new("NETWORK_ERROR");
            continue;
        };
        if !resp.status().is_success() {
            last_err = classify_extractor_error(&format!("HTTP error {}", resp.status().as_u16()));
            continue;
        }
        let Ok(bytes) = resp.bytes().await else {
            last_err = AppError::new("NETWORK_ERROR");
            continue;
        };
        if bytes.len() > MAX_HTML_BYTES {
            last_err = AppError::new("FILE_TOO_LARGE");
            continue;
        }
        let Ok(data) = serde_json::from_slice::<Value>(&bytes) else {
            last_err = AppError::new("EXTRACTOR_FAILED");
            continue;
        };
        match parse_youtube_innertube_response(&data, &video_id, ua) {
            Ok(res) => return Ok(res),
            Err(err) => last_err = err,
        }
    }
    Err(last_err)
}

// ============================================================================
// 100% Pure-Rust TikTok Extractor (Rehydration HTML + Official oEmbed)
// ============================================================================

#[derive(Debug, Clone)]
pub struct TikTokMediaResult {
    pub media_id: String,
    pub title: Option<String>,
    pub uploader: Option<String>,
    pub duration_seconds: Option<f64>,
    pub thumbnail_url: Option<String>,
    pub video_variants: Vec<DirectVideoVariant>,
    pub images: Vec<ImageItem>,
    pub cookies: Option<String>,
}

static TT_REHYD_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?s)<script[^>]+id=["']__UNIVERSAL_DATA_FOR_REHYDRATION__["'][^>]*>(\{.+?\})</script>"#)
        .unwrap()
});

pub fn parse_tiktok_rehydration(html: &str, url_str: &str) -> Option<TikTokMediaResult> {
    let cap = TT_REHYD_RE.captures(html)?;
    let data: Value = serde_json::from_str(&cap[1]).ok()?;
    let item = data
        .pointer("/__DEFAULT_SCOPE__/webapp.video-detail/itemInfo/itemStruct")
        .or_else(|| data.pointer("/__DEFAULT_SCOPE__/webapp.reflow.video.detail/itemInfo/itemStruct"))?;

    let media_id = item
        .get("id")
        .and_then(Value::as_str)
        .map(String::from)
        .unwrap_or_else(|| url_str.rsplit('/').next().unwrap_or("tiktok").to_string());
    let title = clean_text(item.get("desc").and_then(Value::as_str), 240);
    let uploader = clean_text(
        item.pointer("/author/nickname")
            .or_else(|| item.pointer("/author/uniqueId"))
            .and_then(Value::as_str),
        120,
    );
    let duration_seconds = item
        .pointer("/video/duration")
        .and_then(Value::as_f64)
        .filter(|d| d.is_finite() && *d >= 0.0);
    let thumbnail_url = validate_image_url(
        item.pointer("/video/cover")
            .or_else(|| item.pointer("/video/originCover"))
            .and_then(Value::as_str),
    );

    let mut video_variants = Vec::new();
    if let Some(bitrates) = item.pointer("/video/bitrateInfo").and_then(Value::as_array) {
        for b in bitrates {
            let br = b.get("Bitrate").and_then(Value::as_u64).unwrap_or(0);
            let h = b
                .pointer("/PlayAddr/Height")
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32;
            let w = b
                .pointer("/PlayAddr/Width")
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32;
            let dim = if w > 0 && h > 0 { w.min(h) } else { h.max(w) };
            if let Some(urls) = b.pointer("/PlayAddr/UrlList").and_then(Value::as_array) {
                for u in urls.iter().filter_map(Value::as_str) {
                    if let Some(safe) = validate_video_cdn_url(u, "tiktok") {
                        video_variants.push(DirectVideoVariant {
                            url: safe,
                            height: dim,
                            bitrate: br,
                        });
                        break;
                    }
                }
            }
        }
    }
    if video_variants.is_empty() {
        for ptr in ["/video/playAddr", "/video/downloadAddr"] {
            if let Some(u) = item.pointer(ptr).and_then(Value::as_str) {
                if let Some(safe) = validate_video_cdn_url(u, "tiktok") {
                    let h = item.pointer("/video/height").and_then(Value::as_u64).unwrap_or(720) as u32;
                    let w = item.pointer("/video/width").and_then(Value::as_u64).unwrap_or(0) as u32;
                    let dim = if w > 0 && h > 0 { w.min(h) } else { h };
                    video_variants.push(DirectVideoVariant {
                        url: safe,
                        height: dim,
                        bitrate: 0,
                    });
                    break;
                }
            }
        }
    }
    video_variants.sort_by(|a, b| b.height.cmp(&a.height).then_with(|| b.bitrate.cmp(&a.bitrate)));

    let mut raw_imgs = Vec::new();
    if let Some(imgs) = item.pointer("/imagePost/images").and_then(Value::as_array) {
        for im in imgs {
            let w = im.get("imageWidth").and_then(Value::as_u64).map(|x| x as u32);
            let h = im.get("imageHeight").and_then(Value::as_u64).map(|x| x as u32);
            if let Some(u) = im
                .pointer("/imageURL/urlList/0")
                .and_then(Value::as_str)
            {
                raw_imgs.push((u.to_string(), w, h));
            }
        }
    }
    let images = normalize_items(raw_imgs);

    if video_variants.is_empty() && images.is_empty() {
        return None;
    }

    Some(TikTokMediaResult {
        media_id,
        title,
        uploader,
        duration_seconds,
        thumbnail_url: thumbnail_url.or_else(|| images.first().map(|i| i.url.clone())),
        video_variants,
        images,
        cookies: None,
    })
}

pub async fn fetch_tiktok_media(url_str: &str) -> Result<TikTokMediaResult, AppError> {
    ensure_public_host(url_str).await?;
    let resp = SHARED_HTTP
        .get(url_str)
        .header("User-Agent", GENERIC_UA)
        .header("Accept", "text/html,application/xhtml+xml")
        .header("Accept-Language", "en-US,en;q=0.9")
        .timeout(Duration::from_secs(12))
        .send()
        .await
        .map_err(|_| AppError::new("NETWORK_ERROR"))?;

    let cookies: String = resp
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|s| s.split(';').next())
        .collect::<Vec<_>>()
        .join("; ");

    if !resp.status().is_success() {
        return Err(classify_extractor_error(&format!("HTTP error {}", resp.status().as_u16())));
    }
    let bytes = resp.bytes().await.map_err(|_| AppError::new("NETWORK_ERROR"))?;
    let html = String::from_utf8_lossy(&bytes);
    if let Some(mut res) = parse_tiktok_rehydration(&html, url_str) {
        if !cookies.is_empty() {
            res.cookies = Some(cookies);
        }
        return Ok(res);
    }
    if html.contains("SlardarWAF") || html.contains("verify") || html.contains("captcha") {
        return Err(AppError::new("LOGIN_REQUIRED"));
    }
    Err(AppError::new("UNAVAILABLE_MEDIA"))
}

// ============================================================================
// Pure-Rust Image Extractor Entrypoint (ZERO gallery-dl!)
// ============================================================================

pub async fn extract_images(url_str: &str, platform: &str) -> Option<ExtractedPhotoSet> {
    match platform {
        "instagram" => extract_instagram(url_str).await,
        "threads" => extract_threads(url_str).await,
        "facebook" => extract_facebook(url_str).await,
        "pinterest" => extract_pinterest(url_str).await,
        "x" => extract_twitter(url_str).await,
        "tiktok" => fetch_tiktok_media(url_str).await.ok().and_then(|t| {
            (!t.images.is_empty()).then_some(ExtractedPhotoSet {
                title: t.title,
                uploader: t.uploader,
                media_id: Some(t.media_id),
                images: t.images,
            })
        }),
        _ => None,
    }
}

#[allow(dead_code)]
pub fn encode_preview_jpeg(raw: &[u8]) -> Result<Vec<u8>, AppError> {
    let img = decode_safe_image(raw)?;
    let resized = img.thumbnail(720, 720);
    let rgb = resized.to_rgb8();
    let mut out = Cursor::new(Vec::new());
    let mut enc = JpegEncoder::new_with_quality(&mut out, 82);
    enc.encode_image(&rgb)
        .map_err(|_| AppError::new("FORMAT_UNAVAILABLE"))?;
    Ok(out.into_inner())
}

fn decode_safe_image(raw: &[u8]) -> Result<image::DynamicImage, AppError> {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIM);
    limits.max_image_height = Some(MAX_IMAGE_DIM);
    limits.max_alloc = Some(MAX_IMAGE_DECODE_ALLOC);

    let mut reader = image::ImageReader::new(Cursor::new(raw))
        .with_guessed_format()
        .map_err(|_| AppError::new("FORMAT_UNAVAILABLE"))?;
    let format = reader
        .format()
        .ok_or_else(|| AppError::new("FORMAT_UNAVAILABLE"))?;
    if !matches!(format, ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::WebP) {
        return Err(AppError::new("FORMAT_UNAVAILABLE"));
    }
    reader.limits(limits.clone());
    let (w, h) = reader
        .into_dimensions()
        .map_err(|_| AppError::new("FORMAT_UNAVAILABLE"))?;
    if (w as u64) * (h as u64) > MAX_IMAGE_PIXELS || w == 0 || h == 0 {
        return Err(AppError::new("FORMAT_UNAVAILABLE"));
    }
    let mut decoder = image::ImageReader::with_format(Cursor::new(raw), format);
    decoder.limits(limits);
    decoder
        .decode()
        .map_err(|_| AppError::new("FORMAT_UNAVAILABLE"))
}

pub fn encode_image_container(raw: &[u8], container: &str) -> Result<Vec<u8>, AppError> {
    let img = decode_safe_image(raw)?;
    let mut out = Cursor::new(Vec::new());
    match container {
        "jpg" => {
            let rgb = img.to_rgb8();
            let mut enc = JpegEncoder::new_with_quality(&mut out, 92);
            enc.encode_image(&rgb)
                .map_err(|_| AppError::new("FORMAT_UNAVAILABLE"))?;
        }
        "png" => {
            img.write_to(&mut out, ImageFormat::Png)
                .map_err(|_| AppError::new("FORMAT_UNAVAILABLE"))?;
        }
        "webp" => {
            img.write_to(&mut out, ImageFormat::WebP)
                .map_err(|_| AppError::new("FORMAT_UNAVAILABLE"))?;
        }
        _ => return Err(AppError::new("FORMAT_UNAVAILABLE")),
    }
    Ok(out.into_inner())
}

pub async fn download_photo_set<F>(
    photo_set: &ExtractedPhotoSet,
    container: &str,
    image_index: Option<i64>,
    dest_file: &Path,
    mut on_progress: F,
) -> Result<i64, AppError>
where
    F: FnMut(f64, i64),
{
    if photo_set.images.is_empty() {
        return Err(AppError::new("FORMAT_UNAVAILABLE"));
    }
    if container == "zip" {
        if photo_set.images.len() < 2 {
            return Err(AppError::new("FORMAT_UNAVAILABLE"));
        }
        let file =
            std::fs::File::create(dest_file).map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;
        let mut zip = ZipWriter::new(BufWriter::new(file));
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let total = photo_set.images.len() as f64;
        let mut downloaded_total: i64 = 0;
        for (idx, item) in photo_set.images.iter().enumerate() {
            let raw = match fetch_bytes(&item.url, &[], MAX_IMAGE_BYTES, 30).await {
                Ok(b) => b,
                Err(err) => {
                    drop(zip);
                    let _ = std::fs::remove_file(dest_file);
                    return Err(err);
                }
            };
            let jpg = match tokio::task::spawn_blocking(move || encode_image_container(&raw, "jpg"))
                .await
                .map_err(|_| AppError::new("INTERNAL_ERROR"))
                .and_then(|r| r)
            {
                Ok(b) => b,
                Err(err) => {
                    drop(zip);
                    let _ = std::fs::remove_file(dest_file);
                    return Err(err);
                }
            };
            downloaded_total = downloaded_total.saturating_add(jpg.len() as i64);
            if zip
                .start_file(format!("photo_{:02}.jpg", item.index), opts)
                .and_then(|_| zip.write_all(&jpg).map_err(zip::result::ZipError::Io))
                .is_err()
            {
                drop(zip);
                let _ = std::fs::remove_file(dest_file);
                return Err(AppError::new("STORAGE_UNAVAILABLE"));
            }
            on_progress(((idx + 1) as f64 / total) * 95.0, downloaded_total);
        }
        let mut writer = match zip.finish() {
            Ok(w) => w,
            Err(_) => {
                let _ = std::fs::remove_file(dest_file);
                return Err(AppError::new("STORAGE_UNAVAILABLE"));
            }
        };
        if writer.flush().is_err() {
            let _ = std::fs::remove_file(dest_file);
            return Err(AppError::new("STORAGE_UNAVAILABLE"));
        }
        drop(writer);
        let final_size = std::fs::metadata(dest_file)
            .map(|m| m.len() as i64)
            .unwrap_or(downloaded_total);
        return Ok(final_size);
    }

    let idx = image_index.unwrap_or(1) as usize;
    if idx < 1 || idx > photo_set.images.len() {
        return Err(AppError::new("FORMAT_UNAVAILABLE"));
    }
    let chosen = &photo_set.images[idx - 1];
    on_progress(15.0, 0);
    let raw = fetch_bytes(&chosen.url, &[], MAX_IMAGE_BYTES, 30).await?;
    on_progress(75.0, raw.len() as i64);
    let container_owned = container.to_string();
    let encoded =
        tokio::task::spawn_blocking(move || encode_image_container(&raw, &container_owned))
            .await
            .map_err(|_| AppError::new("INTERNAL_ERROR"))??;
    std::fs::write(dest_file, &encoded).map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;
    Ok(encoded.len() as i64)
}

#[derive(Debug, Clone)]
pub struct UpdateCheckResult {
    pub latest_tag: String,
    pub release_url: String,
    pub update_available: bool,
}

fn parse_semver_tuple(tag: &str) -> (u32, u32, u32) {
    let clean = tag.trim().trim_start_matches(['v', 'V']);
    let mut parts = clean.split('.').map(|s| {
        s.chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse::<u32>()
            .unwrap_or(0)
    });
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

/// Manual opt-in update check against GitHub's official Releases API (never auto-executes anything).
pub async fn check_latest_github_release(current_version: &str) -> Result<UpdateCheckResult, String> {
    let api_url = "https://api.github.com/repos/masterputra169/MediaVault/releases/latest";
    ensure_public_host(api_url)
        .await
        .map_err(|_| "Could not verify GitHub endpoint.".to_string())?;
    let resp = SHARED_HTTP
        .get(api_url)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", format!("MediaVault/{current_version}"))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|_| "Could not reach GitHub Releases. Check your internet connection.".to_string())?;
    if !resp.status().is_success() {
        return Err(format!("GitHub returned HTTP {}.", resp.status().as_u16()));
    }
    let json: Value = resp
        .json()
        .await
        .map_err(|_| "Invalid release response from GitHub.".to_string())?;
    let tag = json
        .get("tag_name")
        .and_then(Value::as_str)
        .ok_or_else(|| "Latest release tag was missing.".to_string())?
        .to_string();
    let html_url = json
        .get("html_url")
        .and_then(Value::as_str)
        .filter(|u| u.starts_with("https://github.com/masterputra169/MediaVault/releases/"))
        .unwrap_or("https://github.com/masterputra169/MediaVault/releases/latest")
        .to_string();
    let update_available = parse_semver_tuple(&tag) > parse_semver_tuple(current_version);
    Ok(UpdateCheckResult {
        latest_tag: tag,
        release_url: html_url,
        update_available,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_svg_and_converts_png_to_jpg_webp() {
        assert!(encode_image_container(b"<svg><script>alert(1)</script></svg>", "png").is_err());

        let img = image::RgbImage::from_pixel(16, 12, image::Rgb([40, 120, 220]));
        let mut png_bytes = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut png_bytes, ImageFormat::Png)
            .unwrap();
        let raw = png_bytes.into_inner();

        let jpg = encode_image_container(&raw, "jpg").unwrap();
        assert_eq!(&jpg[..2], &[0xff, 0xd8]);
        let webp = encode_image_container(&raw, "webp").unwrap();
        assert_eq!(&webp[..4], b"RIFF");
        let prev = encode_preview_jpeg(&raw).unwrap();
        assert_eq!(&prev[..2], &[0xff, 0xd8]);
    }

    #[test]
    fn parses_instagram_polaris_single_and_carousel() {
        let html = r#"<script type="application/json" data-sjs>{"require":[["ScheduledServerJS","handle",null,[{"__bbox":{"require":[["RelayPrefetchedStreamCache","next",[],["key",{"__bbox":{"result":{"data":{"xig_polaris_media":{"if_not_gated_logged_out":{"media_type":1,"original_width":2160,"original_height":2704,"user":{"username":"neuron","full_name":"Neuron ML"},"caption":{"text":"Gradient Descent"},"image_versions2":{"candidates":[{"url":"https://scontent-sin2-3.cdninstagram.com/v/full.jpg?stp=dst-jpg"}]}}}}}}}]]]}}]]]}</script>"#;
        let parsed = parse_instagram_polaris(html, "DgLJAyrIBzH").unwrap();
        assert_eq!(parsed.title.as_deref(), Some("Gradient Descent"));
        assert_eq!(parsed.uploader.as_deref(), Some("Neuron ML"));
        assert_eq!(parsed.images.len(), 1);
        assert_eq!(parsed.images[0].width, Some(2160));
    }

    #[test]
    fn parses_instagram_polaris_reel_video_versions() {
        let html = r#"<script type="application/json" data-sjs>{"require":[["ScheduledServerJS","handle",null,[{"__bbox":{"require":[["RelayPrefetchedStreamCache","next",[],["key",{"__bbox":{"result":{"data":{"xig_polaris_media":{"if_not_gated_logged_out":{"__typename":"XIGPolarisVideoMedia","media_type":2,"original_width":1080,"original_height":1920,"user":{"username":"maxims.dailylife","full_name":"Maxim"},"video_versions":[{"type":101,"url":"https://instagram.fmlg11-1.fna.fbcdn.net/v/a.mp4","width":720,"height":1280}],"image_versions2":{"candidates":[{"url":"https://scontent-sin2-3.cdninstagram.com/v/thumb.jpg"}]}}}}}}}]]]}}]]]}</script>"#;
        let parsed = parse_instagram_polaris_media(html, "DgKrEuFSXL7").unwrap();
        assert!(!parsed.video_variants.is_empty());
        assert_eq!(parsed.video_variants[0].height, 1080);
        assert!(parsed.images.is_empty());
        assert!(parsed.thumbnail_url.is_some());
    }

    #[test]
    fn parses_twitter_quote_tweet_video_and_main_photo() {
        let payload: Value = serde_json::json!({
            "id_str": "2125937180795338764",
            "text": "Quote tweet with video",
            "user": {"name": "Orpheus", "screen_name": "orpheuskaze"},
            "mediaDetails": [
                {"type": "photo", "media_url_https": "https://pbs.twimg.com/media/main.jpg", "original_info": {"width": 1000, "height": 800}}
            ],
            "quoted_tweet": {
                "id_str": "2121933725076512990",
                "user": {"screen_name": "orpheuskaze"},
                "mediaDetails": [
                    {
                        "type": "video",
                        "media_url_https": "https://pbs.twimg.com/amplify_video_thumb/1/img/thumb.jpg",
                        "original_info": {"width": 1080, "height": 1920},
                        "video_info": {
                            "duration_millis": 189141,
                            "variants": [
                                {"content_type": "application/x-mpegURL", "url": "https://video.twimg.com/a.m3u8"},
                                {"content_type": "video/mp4", "bitrate": 632000, "url": "https://video.twimg.com/amplify_video/1/vid/avc1/320x568/a.mp4"},
                                {"content_type": "video/mp4", "bitrate": 2176000, "url": "https://video.twimg.com/amplify_video/1/vid/avc1/720x1280/b.mp4"}
                            ]
                        }
                    }
                ]
            }
        });
        let parsed = parse_twitter_syndication(&payload, "2125937180795338764").unwrap();
        assert_eq!(parsed.video_variants.len(), 2);
        assert_eq!(parsed.video_variants[0].height, 720);
        assert_eq!(parsed.images.len(), 1);
        assert_eq!(
            parsed.quoted_tweet_url.as_deref(),
            Some("https://x.com/orpheuskaze/status/2121933725076512990")
        );

        // Top-level `video.variants[].src` format (without mediaDetails)
        let video_obj_payload: Value = serde_json::json!({
            "id_str": "2125937180795338765",
            "text": "Direct video object tweet",
            "user": {"name": "Creator"},
            "video": {
                "poster": "https://pbs.twimg.com/ext_tw_video_thumb/1/pu/img/poster.jpg",
                "durationMs": 42000,
                "variants": [
                    {"type": "application/x-mpegURL", "src": "https://video.twimg.com/a.m3u8"},
                    {"type": "video/mp4", "src": "https://video.twimg.com/ext_tw_video/1/pu/vid/avc1/1280x720/hi.mp4"},
                    {"type": "video/mp4", "src": "https://evil.example/bad.mp4"}
                ]
            }
        });
        let vparsed = parse_twitter_syndication(&video_obj_payload, "2125937180795338765").unwrap();
        assert_eq!(vparsed.video_variants.len(), 1);
        assert_eq!(vparsed.video_variants[0].height, 720);
        assert_eq!(vparsed.duration_seconds, Some(42.0));
        assert!(vparsed.thumbnail_url.is_some());
        assert!(!looks_like_photo_url("https://x.com/u/status/123/video/1", "x"));
    }

    #[test]
    fn parses_threads_carousel_photos_and_video_posts() {
        let photo_carousel_html = r#"<script type="application/json" data-sjs>{"require":[[{"post":{"code":"DgMElZ0E8ej","media_type":8,"user":{"username":"fuaditrockz","full_name":"Fuadit Muhammad"},"caption":{"text":"Mayan, diskon black friday."},"carousel_media":[{"media_type":1,"original_width":1440,"original_height":1264,"image_versions2":{"candidates":[{"url":"https://instagram.fmlg11-1.fna.fbcdn.net/v/1.jpg","width":1440,"height":1264}]}},{"media_type":1,"original_width":970,"original_height":1280,"image_versions2":{"candidates":[{"url":"https://instagram.fmlg11-1.fna.fbcdn.net/v/2.jpg","width":970,"height":1280}]}}]}}]]}</script>"#;
        let parsed = parse_threads_media(photo_carousel_html, "DgMElZ0E8ej").unwrap();
        assert_eq!(parsed.title.as_deref(), Some("Mayan, diskon black friday."));
        assert_eq!(parsed.uploader.as_deref(), Some("Fuadit Muhammad"));
        assert!(parsed.video_variants.is_empty());
        assert_eq!(parsed.images.len(), 2);
        assert_eq!(parsed.images[0].width, Some(1440));

        let video_html = r#"<script type="application/json" data-sjs>{"post":{"code":"DgVid12345","media_type":2,"has_audio":true,"video_duration":14.5,"original_width":1080,"original_height":1920,"user":{"username":"creator"},"caption":{"text":"Threads video demo"},"video_versions":[{"type":101,"url":"https://scontent.cdninstagram.com/v/vid1080.mp4","width":1080,"height":1920},{"type":102,"url":"https://evil.example/vid.mp4","width":720,"height":1280}],"image_versions2":{"candidates":[{"url":"https://scontent.cdninstagram.com/v/thumb.jpg","width":1080,"height":1920}]}}}</script>"#;
        let vparsed = parse_threads_media(video_html, "DgVid12345").unwrap();
        assert_eq!(vparsed.video_variants.len(), 1);
        assert_eq!(vparsed.video_variants[0].height, 1080);
        assert!(vparsed.has_audio);
        assert_eq!(vparsed.duration_seconds, Some(14.5));
        assert!(vparsed.images.is_empty());

        let quoted_html = r#"<script type="application/json" data-sjs>{"post":{"code":"DgQuote123","media_type":19,"user":{"username":"quoter"},"caption":{"text":"Look at this"},"text_post_app_info":{"share_info":{"quoted_post":{"code":"DgOrig999","media_type":1,"image_versions2":{"candidates":[{"url":"https://instagram.fmlg11-1.fna.fbcdn.net/v/q.jpg","width":800,"height":600}]}}}}}}</script>"#;
        let qparsed = parse_threads_media(quoted_html, "DgQuote123").unwrap();
        assert_eq!(qparsed.images.len(), 1);
        assert_eq!(qparsed.title.as_deref(), Some("Look at this"));
    }

    #[test]
    fn parses_pinterest_photo_and_video_pin_html() {
        let html = r#"<meta property="og:title" content="NASA Space Video" /><script>{"url":"https://v1.pinimg.com/videos/iht/expMp4/a2/a7/b9/demo_720w.mp4","poster":"https://i.pinimg.com/originals/d5/3b/01/demo.png"}</script>"#;
        let parsed = parse_pinterest_html(html, "99360735500167749").unwrap();
        assert_eq!(parsed.title.as_deref(), Some("NASA Space Video"));
        assert_eq!(parsed.video_variants.len(), 1);
        assert_eq!(parsed.video_variants[0].height, 720);
        assert_eq!(parsed.images.len(), 1);
    }

    #[test]
    fn parses_facebook_video_photo_and_album_html() {
        let vid_html = r#"<meta property="og:description" content="Europa Clipper mission preview" /><meta property="og:image" content="https://scontent.fsub9-1.fna.fbcdn.net/v/t15/thumb.jpg" /><script type="application/json">{"id":"2470894823717065","playable_duration_in_ms":87400,"browser_native_hd_url":"https://video.fsub9-1.fna.fbcdn.net/o1/v/hd.mp4","browser_native_sd_url":"https://video.fsub9-1.fna.fbcdn.net/o1/v/sd.mp4"}</script>"#;
        let v = parse_facebook_html(vid_html, "https://www.facebook.com/watch/?v=2470894823717065")
            .unwrap();
        assert_eq!(v.video_variants.len(), 2);
        assert_eq!(v.video_variants[0].height, 720);
        assert_eq!(v.duration_seconds, Some(87.4));
        assert!(v.images.is_empty());

        let photo_html = r#"<script type="application/json">{"currMedia":{"id":"1697460148416021","image":{"uri":"https://scontent.fsub8-2.fna.fbcdn.net/v/t39.30808-6/924514337_1697460158416020_6227482654830500974_n.jpg","width":1280,"height":853},"accessibility_caption":"Artemis II Moon view","owner":{"name":"NASA"}}}</script>"#;
        let p = parse_facebook_html(
            photo_html,
            "https://www.facebook.com/photo/?fbid=1697460148416021",
        )
        .unwrap();
        assert_eq!(p.images.len(), 1);

        // Facebook 1080p DASH XML manifest with adaptive video + AAC audio
        let dash_xml = r#"<MPD><Period><AdaptationSet><Representation mimeType="video/mp4" codecs="avc1.640028" height="1080" bandwidth="3200000"><BaseURL>https://video.xx.fbcdn.net/v/1080p.mp4?a=1&amp;b=2</BaseURL></Representation><Representation mimeType="audio/mp4" codecs="mp4a.40.2" bandwidth="128000"><BaseURL>https://video.xx.fbcdn.net/v/audio.m4a?a=1&amp;b=2</BaseURL></Representation></AdaptationSet></Period></MPD>"#;
        let dash_json = serde_json::json!({ "dash_manifest": dash_xml }).to_string();
        let dash_html = format!(r#"<script type="application/json">{dash_json}</script>"#);
        let parsed_dash = parse_facebook_html(&dash_html, "https://www.facebook.com/watch/?v=123").unwrap();
        assert_eq!(parsed_dash.dash_videos.len(), 1);
        assert_eq!(parsed_dash.dash_videos[0].height, 1080);
        assert!(parsed_dash.dash_audio_url.as_deref().unwrap().contains("audio.m4a"));
        assert!(parse_semver_tuple("v0.5.0") > parse_semver_tuple("0.4.0"));
        assert_eq!(p.images[0].width, Some(1280));
        assert_eq!(p.uploader.as_deref(), Some("NASA"));

        let album_html = r#"<meta property="og:title" content="NASA" /><meta property="og:description" content="A home away from home &#x1f315; &#128640;" /><script type="application/json">{"all_subattachments":{"nodes":[{"media":{"photo_image":{"uri":"https://scontent.fsub8-2.fna.fbcdn.net/v/a1.jpg","width":1080,"height":1080}}},{"media":{"photo_image":{"uri":"https://scontent.fsub8-2.fna.fbcdn.net/v/a2.jpg","width":1080,"height":720}}}]}}</script>"#;
        let a = parse_facebook_html(album_html, "https://www.facebook.com/NASA/posts/pfbid123")
            .unwrap();
        assert_eq!(a.images.len(), 2);
        assert_eq!(a.title.as_deref(), Some("A home away from home 🌕 🚀"));
    }
}
