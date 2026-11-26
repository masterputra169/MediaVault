use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use chrono::Utc;
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{watch, Semaphore};
use url::Url;

use crate::db::{ensure_safe_canonical_dir, validate_download_dir, Database, JobRecord};
use crate::errors::{classify_extractor_error, AppError};
use crate::images::{self, DirectVideoVariant, ImageItem, PreviewCache};
use crate::tools::{background_cmd, discover_tools, ToolPaths};
use crate::url_policy::{
    clean_text, ensure_public_host, resolve_alias, validate_image_url, validate_url,
    validate_video_cdn_url, SHARED_HTTP,
};

const QUALITIES: &[&str] = &["1080", "720", "480", "360"];
const MEDIA_PROTOCOLS: &[&str] = &["https", "http", "m3u8_native", "m3u8", "http_dash_segments"];
const HLS_PROTOCOLS: &[&str] = &["m3u8", "m3u8_native"];
const QUEUE_LIMIT: i64 = 20;
const MAX_DIRECT_VIDEO_BYTES: i64 = 2 * 1024 * 1024 * 1024;
const DOWNLOAD_TIMEOUT_SECS: u64 = 3600;
const FFMPEG_TIMEOUT_SECS: u64 = 900;

static YT_ID_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[A-Za-z0-9_-]{11}$").unwrap());

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaMetadata {
    pub url: String,
    pub platform: String,
    pub title: Option<String>,
    pub uploader: Option<String>,
    pub duration_seconds: Option<f64>,
    pub thumbnail_url: Option<String>,
    pub preview_token: Option<String>,
    pub media_id: Option<String>,
    pub media_type: Option<String>,
    pub format_count: usize,
    pub available_qualities: Vec<String>,
    pub available_audio_formats: Vec<String>,
    pub available_image_formats: Vec<String>,
    pub images: Vec<ImageItem>,
    pub has_audio: bool,
    pub supports_mp4: bool,
    pub estimated_size_bytes: Option<i64>,
    pub warnings: Vec<String>,
    #[serde(default)]
    pub needs_enrichment: bool,
}

#[derive(Debug, Clone)]
struct CachedAnalysis {
    meta: MediaMetadata,
    resolved_url: Option<String>,
    direct_videos: Vec<DirectVideoVariant>,
    download_target_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateDownloadRequest {
    pub url: String,
    pub kind: String,
    pub quality: String,
    pub container: String,
    pub audio_bitrate_kbps: Option<i64>,
    #[serde(default)]
    pub allow_quality_fallback: bool,
    pub image_index: Option<i64>,
    #[serde(default)]
    pub output_dir: Option<String>,
}

fn usable_format(fmt: &Value) -> bool {
    if fmt.get("has_drm").and_then(Value::as_bool) == Some(true) {
        return false;
    }
    let Some(u) = fmt.get("url").and_then(Value::as_str) else {
        return false;
    };
    let Ok(parsed) = Url::parse(u) else {
        return false;
    };
    if !matches!(parsed.scheme(), "http" | "https") || !parsed.username().is_empty() {
        return false;
    }
    if let Some(p) = parsed.port() {
        if p != 80 && p != 443 {
            return false;
        }
    }
    let proto = fmt
        .get("protocol")
        .and_then(Value::as_str)
        .unwrap_or(parsed.scheme());
    MEDIA_PROTOCOLS.contains(&proto)
}

fn is_compatible_video(fmt: &Value) -> bool {
    if fmt.get("ext").and_then(Value::as_str) != Some("mp4") {
        return false;
    }
    match fmt.get("vcodec").and_then(Value::as_str) {
        None | Some("") => true,
        Some("none") => false,
        Some(vc) => ["avc", "h264", "hev", "hvc", "av01", "vp09", "vp9"]
            .iter()
            .any(|p| vc.starts_with(p)),
    }
}

fn is_compatible_audio(fmt: &Value) -> bool {
    let ext = fmt.get("ext").and_then(Value::as_str).unwrap_or("");
    let ac = fmt.get("acodec").and_then(Value::as_str).unwrap_or("");
    matches!(ext, "mp4" | "m4a") && (ac.starts_with("mp4a") || ac.starts_with("aac"))
}

fn fmt_height(fmt: &Value) -> u64 {
    let h = fmt.get("height").and_then(Value::as_u64).unwrap_or(0);
    let w = fmt.get("width").and_then(Value::as_u64).unwrap_or(0);
    match (h > 0, w > 0) {
        (true, true) => h.min(w),
        (true, false) => h,
        (false, true) => w,
        (false, false) => 0,
    }
}

fn normalize_ytdlp_info(
    info: &Value,
    original_url: &str,
    platform: &str,
    can_merge: bool,
) -> Result<MediaMetadata, AppError> {
    if !info.is_object() {
        return Err(AppError::new("UNAVAILABLE_MEDIA"));
    }

    // For single social posts (e.g. X quote tweets / multi-video posts), pick first playable entry
    // while keeping YouTube playlists strictly rejected.
    let effective = if info.get("entries").is_some()
        || matches!(
            info.get("_type").and_then(Value::as_str),
            Some("playlist" | "multi_video")
        ) {
        if platform == "youtube" {
            return Err(AppError::new("UNAVAILABLE_MEDIA"));
        }
        let entries = info
            .get("entries")
            .and_then(Value::as_array)
            .ok_or_else(|| AppError::new("UNAVAILABLE_MEDIA"))?;
        entries
            .iter()
            .find(|e| {
                e.get("formats")
                    .and_then(Value::as_array)
                    .is_some_and(|arr| arr.iter().any(usable_format))
                    || usable_format(e)
            })
            .ok_or_else(|| AppError::new("UNAVAILABLE_MEDIA"))?
    } else {
        info
    };

    if effective.get("is_live").and_then(Value::as_bool) == Some(true)
        || matches!(
            effective.get("live_status").and_then(Value::as_str),
            Some("is_live" | "is_upcoming")
        )
    {
        return Err(AppError::new("UNAVAILABLE_MEDIA"));
    }
    if matches!(
        effective.get("availability").and_then(Value::as_str),
        Some("private" | "premium_only" | "subscriber_only" | "needs_auth")
    ) {
        return Err(AppError::new("LOGIN_REQUIRED"));
    }

    let raw_formats: Vec<&Value> = match effective.get("formats").and_then(Value::as_array) {
        Some(arr) => arr.iter().filter(|f| usable_format(f)).collect(),
        None => {
            if usable_format(effective) {
                vec![effective]
            } else {
                Vec::new()
            }
        }
    };

    let has_audio = raw_formats
        .iter()
        .any(|f| f.get("acodec").and_then(Value::as_str) != Some("none"));
    let best_audio = raw_formats.iter().rev().find(|f| {
        f.get("vcodec").and_then(Value::as_str) == Some("none") && is_compatible_audio(f)
    });

    let mut videos: Vec<Vec<&Value>> = Vec::new();
    for fmt in raw_formats.iter().rev() {
        if !is_compatible_video(fmt) {
            continue;
        }
        let proto = fmt.get("protocol").and_then(Value::as_str).unwrap_or("");
        if !can_merge && HLS_PROTOCOLS.contains(&proto) {
            continue;
        }
        let ac = fmt.get("acodec").and_then(Value::as_str);
        if is_compatible_audio(fmt) || matches!(ac, None | Some("")) || (ac == Some("none") && !has_audio) {
            videos.push(vec![*fmt]);
        } else if ac == Some("none") && can_merge {
            if let Some(a) = best_audio {
                videos.push(vec![*fmt, *a]);
            }
        }
    }

    let heights: HashSet<u64> = videos.iter().map(|pair| fmt_height(pair[0])).collect();
    let mut available_qualities = Vec::new();
    if !videos.is_empty() {
        available_qualities.push("best".to_string());
    }
    for &q in QUALITIES {
        if let Ok(qh) = q.parse::<u64>() {
            if heights.contains(&qh) {
                available_qualities.push(q.to_string());
            }
        }
    }

    let estimated = videos.first().and_then(|pair| {
        let mut sum: i64 = 0;
        for f in pair {
            let sz = f
                .get("filesize")
                .or_else(|| f.get("filesize_approx"))
                .and_then(Value::as_i64)?;
            if sz <= 0 {
                return None;
            }
            sum = sum.saturating_add(sz);
        }
        (sum > 0).then_some(sum)
    });

    let has_video_track = !videos.is_empty()
        || raw_formats
            .iter()
            .any(|f| !matches!(f.get("vcodec").and_then(Value::as_str), None | Some("") | Some("none")));
    let media_type = if has_video_track {
        Some("video".to_string())
    } else if has_audio {
        Some("audio".to_string())
    } else {
        None
    };

    let available_audio_formats = if has_audio && can_merge {
        vec!["mp3".to_string(), "m4a".to_string()]
    } else {
        Vec::new()
    };

    Ok(MediaMetadata {
        url: original_url.to_string(),
        platform: platform.to_string(),
        title: clean_text(
            effective
                .get("title")
                .or_else(|| info.get("title"))
                .and_then(Value::as_str),
            240,
        ),
        uploader: clean_text(
            effective
                .get("uploader")
                .or_else(|| effective.get("channel"))
                .or_else(|| info.get("uploader"))
                .and_then(Value::as_str),
            120,
        ),
        duration_seconds: effective
            .get("duration")
            .and_then(Value::as_f64)
            .filter(|d| d.is_finite() && *d >= 0.0),
        thumbnail_url: validate_image_url(
            effective
                .get("thumbnail")
                .or_else(|| info.get("thumbnail"))
                .and_then(Value::as_str),
        ),
        preview_token: None,
        media_id: clean_text(effective.get("id").and_then(Value::as_str), 80),
        media_type,
        format_count: raw_formats.len(),
        available_qualities,
        available_audio_formats,
        available_image_formats: Vec::new(),
        images: Vec::new(),
        has_audio,
        supports_mp4: !videos.is_empty(),
        estimated_size_bytes: estimated,
        warnings: if videos.is_empty() {
            vec!["Video MP4 yang kompatibel tidak tersedia.".into()]
        } else {
            Vec::new()
        },
        needs_enrichment: false,
    })
}

fn extract_youtube_id(url_str: &str) -> Option<String> {
    let parsed = Url::parse(url_str).ok()?;
    let host = parsed.host_str()?.to_lowercase();
    if host == "youtu.be" {
        let seg = parsed.path().trim_matches('/').split('/').next()?;
        return YT_ID_RE.is_match(seg).then(|| seg.to_string());
    }
    if let Some((_, v)) = parsed.query_pairs().find(|(k, _)| k == "v") {
        if YT_ID_RE.is_match(&v) {
            return Some(v.into_owned());
        }
    }
    let parts: Vec<&str> = parsed.path().trim_matches('/').split('/').collect();
    if parts.len() >= 2 && matches!(parts[0], "shorts" | "embed" | "live") && YT_ID_RE.is_match(parts[1]) {
        return Some(parts[1].to_string());
    }
    None
}

async fn fast_analyze_youtube(url_str: &str, can_merge: bool) -> Option<Result<MediaMetadata, AppError>> {
    let vid = extract_youtube_id(url_str)?;
    let watch_url = format!("https://www.youtube.com/watch?v={vid}&hl=id");
    let bytes = images::fetch_bytes(
        &watch_url,
        &[("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")],
        images::MAX_HTML_BYTES,
        10,
    )
    .await
    .ok()?;
    let html = String::from_utf8_lossy(&bytes);
    let marker = "var ytInitialPlayerResponse = ";
    let idx = html.find(marker)?;
    let slice = &html[idx + marker.len()..];
    let mut stream = serde_json::Deserializer::from_str(slice).into_iter::<Value>();
    let player = stream.next()?.ok()?;

    let status = player
        .pointer("/playabilityStatus/status")
        .and_then(Value::as_str)
        .unwrap_or("");
    if status != "OK" {
        // Fall back to yt-dlp for LOGIN_REQUIRED / UNPLAYABLE so classification stays accurate
        return None;
    }

    let details = player.get("videoDetails")?;
    if details.get("isLiveContent").and_then(Value::as_bool) == Some(true)
        && details.get("isLive").and_then(Value::as_bool) == Some(true)
    {
        return Some(Err(AppError::new("UNAVAILABLE_MEDIA")));
    }

    let streaming = player.get("streamingData")?;
    let mut all_fmts: Vec<&Value> = Vec::new();
    if let Some(arr) = streaming.get("formats").and_then(Value::as_array) {
        all_fmts.extend(arr.iter());
    }
    if let Some(arr) = streaming.get("adaptiveFormats").and_then(Value::as_array) {
        all_fmts.extend(arr.iter());
    }
    if all_fmts.is_empty() {
        return None;
    }

    let mut mp4_heights = HashSet::new();
    let mut has_audio = false;
    let mut best_video_bytes: Option<i64> = None;
    let mut best_audio_bytes: Option<i64> = None;

    for f in &all_fmts {
        let mime = f.get("mimeType").and_then(Value::as_str).unwrap_or("");
        let size = f
            .get("contentLength")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<i64>().ok())
            .filter(|&n| n > 0);
        if mime.starts_with("audio/") {
            has_audio = true;
            if let Some(sz) = size {
                best_audio_bytes = Some(best_audio_bytes.map_or(sz, |cur| cur.max(sz)));
            }
        }
        if mime.starts_with("video/mp4") {
            let h = f.get("height").and_then(Value::as_u64).unwrap_or(0);
            let w = f.get("width").and_then(Value::as_u64).unwrap_or(0);
            let dim = if h > 0 && w > 0 { h.min(w) } else { h.max(w) };
            let is_progressive = f.get("audioQuality").is_some();
            if dim > 0 && (can_merge || is_progressive) {
                mp4_heights.insert(dim);
                if let Some(sz) = size {
                    best_video_bytes = Some(best_video_bytes.map_or(sz, |cur| cur.max(sz)));
                }
            }
        }
    }

    let mut available_qualities = Vec::new();
    if !mp4_heights.is_empty() {
        available_qualities.push("best".to_string());
    }
    for &q in QUALITIES {
        if let Ok(qh) = q.parse::<u64>() {
            if mp4_heights.contains(&qh) {
                available_qualities.push(q.to_string());
            }
        }
    }
    if available_qualities.is_empty() {
        return None;
    }

    let thumb = details
        .pointer("/thumbnail/thumbnails")
        .and_then(Value::as_array)
        .and_then(|arr| arr.last())
        .and_then(|t| t.get("url").and_then(Value::as_str))
        .and_then(|u| validate_image_url(Some(u)))
        .or_else(|| Some(format!("https://i.ytimg.com/vi/{vid}/hqdefault.jpg")));

    let duration = details
        .get("lengthSeconds")
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|d| d.is_finite() && *d >= 0.0);

    let estimated_size_bytes = match (best_video_bytes, best_audio_bytes) {
        (Some(v), Some(a)) => Some(v.saturating_add(a)),
        (Some(v), None) => Some(v),
        _ => None,
    };

    Some(Ok(MediaMetadata {
        url: url_str.to_string(),
        platform: "youtube".to_string(),
        title: clean_text(details.get("title").and_then(Value::as_str), 240),
        uploader: clean_text(details.get("author").and_then(Value::as_str), 120),
        duration_seconds: duration,
        thumbnail_url: thumb,
        preview_token: None,
        media_id: Some(vid),
        media_type: Some("video".to_string()),
        format_count: all_fmts.len(),
        available_qualities,
        available_audio_formats: if has_audio && can_merge {
            vec!["mp3".into(), "m4a".into()]
        } else {
            Vec::new()
        },
        available_image_formats: Vec::new(),
        images: Vec::new(),
        has_audio,
        supports_mp4: true,
        estimated_size_bytes,
        warnings: Vec::new(),
        needs_enrichment: false,
    }))
}

async fn fast_analyze_twitter(url_str: &str, can_merge: bool) -> Option<CachedAnalysis> {
    let syn = images::fetch_twitter_syndication(url_str).await?;
    let has_video = !syn.video_variants.is_empty();
    let has_images = !syn.images.is_empty();
    if !has_video && !has_images {
        return None;
    }

    let mut available_qualities = Vec::new();
    if has_video {
        available_qualities.push("best".to_string());
        let heights: HashSet<u32> = syn.video_variants.iter().map(|v| v.height).collect();
        for &q in QUALITIES {
            if let Ok(qh) = q.parse::<u32>() {
                if heights.iter().any(|&h| h >= qh.saturating_sub(40) && h <= qh + 40) {
                    available_qualities.push(q.to_string());
                }
            }
        }
    }

    let mut image_formats = Vec::new();
    if has_images {
        image_formats.extend(["jpg".into(), "png".into(), "webp".into()]);
        if syn.images.len() > 1 {
            image_formats.push("zip".into());
        }
    }

    let media_type = if has_video {
        Some("video".into())
    } else {
        Some("image".into())
    };
    let meta = MediaMetadata {
        url: url_str.to_string(),
        platform: "x".to_string(),
        title: syn.title,
        uploader: syn.uploader,
        duration_seconds: syn.duration_seconds,
        thumbnail_url: syn.thumbnail_url,
        preview_token: None,
        media_id: Some(syn.media_id),
        media_type,
        format_count: syn.video_variants.len() + syn.images.len(),
        available_qualities,
        available_audio_formats: if has_video && can_merge {
            vec!["mp3".into(), "m4a".into()]
        } else {
            Vec::new()
        },
        available_image_formats: image_formats,
        images: syn.images,
        has_audio: has_video,
        supports_mp4: has_video,
        estimated_size_bytes: None,
        warnings: Vec::new(),
        needs_enrichment: false,
    };
    Some(CachedAnalysis {
        meta,
        resolved_url: Some(url_str.to_string()),
        direct_videos: syn.video_variants,
        download_target_url: syn.quoted_tweet_url,
    })
}

async fn fast_analyze_instagram(url_str: &str, can_merge: bool) -> Option<CachedAnalysis> {
    let ig = images::fetch_instagram_polaris(url_str).await?;
    let has_video = !ig.video_variants.is_empty();
    let has_images = !ig.images.is_empty();
    if !has_video && !has_images {
        return None;
    }

    let mut available_qualities = Vec::new();
    if has_video {
        available_qualities.push("best".to_string());
        let max_h = ig.video_variants.iter().map(|v| v.height).max().unwrap_or(0);
        for &q in QUALITIES {
            if let Ok(qh) = q.parse::<u32>() {
                if ig.video_variants.iter().any(|v| v.height == qh) || (max_h >= qh && qh <= 1080) {
                    available_qualities.push(q.to_string());
                }
            }
        }
    }

    let mut image_formats = Vec::new();
    if has_images {
        image_formats.extend(["jpg".into(), "png".into(), "webp".into()]);
        if ig.images.len() > 1 {
            image_formats.push("zip".into());
        }
    }

    let meta = MediaMetadata {
        url: url_str.to_string(),
        platform: "instagram".to_string(),
        title: ig.title,
        uploader: ig.uploader,
        duration_seconds: ig.duration_seconds,
        thumbnail_url: ig.thumbnail_url,
        preview_token: None,
        media_id: Some(ig.media_id),
        media_type: Some(if has_video { "video" } else { "image" }.into()),
        format_count: ig.video_variants.len() + ig.images.len(),
        available_qualities,
        available_audio_formats: if has_video && can_merge {
            vec!["mp3".into(), "m4a".into()]
        } else {
            Vec::new()
        },
        available_image_formats: image_formats,
        images: ig.images,
        has_audio: has_video,
        supports_mp4: has_video,
        estimated_size_bytes: None,
        warnings: Vec::new(),
        needs_enrichment: false,
    };

    Some(CachedAnalysis {
        meta,
        resolved_url: Some(url_str.to_string()),
        direct_videos: ig.video_variants,
        download_target_url: None,
    })
}

async fn fast_analyze_threads(
    resolved_url: &str,
    original_url: &str,
    can_merge: bool,
) -> Option<CachedAnalysis> {
    let th = images::fetch_threads_media(resolved_url).await?;
    let has_video = !th.video_variants.is_empty();
    let has_images = !th.images.is_empty();
    if !has_video && !has_images {
        return None;
    }

    let mut available_qualities = Vec::new();
    if has_video {
        available_qualities.push("best".to_string());
        let heights: HashSet<u32> = th.video_variants.iter().map(|v| v.height).collect();
        for &q in QUALITIES {
            if let Ok(qh) = q.parse::<u32>() {
                if heights
                    .iter()
                    .any(|&h| h >= qh.saturating_sub(40) && h <= qh + 40)
                {
                    available_qualities.push(q.to_string());
                }
            }
        }
    }

    let mut image_formats = Vec::new();
    if has_images {
        image_formats.extend(["jpg".into(), "png".into(), "webp".into()]);
        if th.images.len() > 1 {
            image_formats.push("zip".into());
        }
    }

    let meta = MediaMetadata {
        url: original_url.to_string(),
        platform: "threads".to_string(),
        title: th.title,
        uploader: th.uploader,
        duration_seconds: th.duration_seconds,
        thumbnail_url: th.thumbnail_url,
        preview_token: None,
        media_id: Some(th.media_id),
        media_type: Some(if has_video { "video" } else { "image" }.into()),
        format_count: th.video_variants.len() + th.images.len(),
        available_qualities,
        available_audio_formats: if has_video && th.has_audio && can_merge {
            vec!["mp3".into(), "m4a".into()]
        } else {
            Vec::new()
        },
        available_image_formats: image_formats,
        images: th.images,
        has_audio: has_video && th.has_audio,
        supports_mp4: has_video,
        estimated_size_bytes: None,
        warnings: Vec::new(),
        needs_enrichment: false,
    };

    Some(CachedAnalysis {
        meta,
        resolved_url: Some(resolved_url.to_string()),
        direct_videos: th.video_variants,
        download_target_url: None,
    })
}

async fn fast_analyze_pinterest(
    resolved_url: &str,
    original_url: &str,
    can_merge: bool,
) -> Option<CachedAnalysis> {
    let pin = images::fetch_pinterest_media(resolved_url).await?;
    let has_video = !pin.video_variants.is_empty();
    let has_images = !pin.images.is_empty();
    if !has_video && !has_images {
        return None;
    }

    let mut available_qualities = Vec::new();
    if has_video {
        available_qualities.push("best".to_string());
        let heights: HashSet<u32> = pin.video_variants.iter().map(|v| v.height).collect();
        for &q in QUALITIES {
            if let Ok(qh) = q.parse::<u32>() {
                if heights
                    .iter()
                    .any(|&h| h >= qh.saturating_sub(40) && h <= qh + 40)
                {
                    available_qualities.push(q.to_string());
                }
            }
        }
    }

    let mut image_formats = Vec::new();
    if has_images {
        image_formats.extend(["jpg".into(), "png".into(), "webp".into()]);
    }

    let meta = MediaMetadata {
        url: original_url.to_string(),
        platform: "pinterest".to_string(),
        title: pin.title,
        uploader: None,
        duration_seconds: None,
        thumbnail_url: pin.thumbnail_url,
        preview_token: None,
        media_id: Some(pin.media_id),
        media_type: Some(if has_video { "video" } else { "image" }.into()),
        format_count: pin.video_variants.len() + pin.images.len(),
        available_qualities,
        available_audio_formats: if has_video && can_merge {
            vec!["mp3".into(), "m4a".into()]
        } else {
            Vec::new()
        },
        available_image_formats: image_formats,
        images: pin.images,
        has_audio: has_video,
        supports_mp4: has_video,
        estimated_size_bytes: None,
        warnings: Vec::new(),
        needs_enrichment: false,
    };

    Some(CachedAnalysis {
        meta,
        resolved_url: Some(resolved_url.to_string()),
        direct_videos: pin.video_variants,
        download_target_url: None,
    })
}

async fn fast_analyze_facebook(
    resolved_url: &str,
    original_url: &str,
    can_merge: bool,
) -> Option<CachedAnalysis> {
    let fb = images::fetch_facebook_media(resolved_url).await?;
    let has_video = !fb.video_variants.is_empty();
    let has_images = !fb.images.is_empty();
    if !has_video && !has_images {
        return None;
    }

    let mut available_qualities = Vec::new();
    if has_video {
        available_qualities.push("best".to_string());
        let heights: HashSet<u32> = fb.video_variants.iter().map(|v| v.height).collect();
        for &q in QUALITIES {
            if let Ok(qh) = q.parse::<u32>() {
                if heights
                    .iter()
                    .any(|&h| h >= qh.saturating_sub(60) && h <= qh + 60)
                {
                    available_qualities.push(q.to_string());
                }
            }
        }
    }

    let mut image_formats = Vec::new();
    if has_images {
        image_formats.extend(["jpg".into(), "png".into(), "webp".into()]);
        if fb.images.len() > 1 {
            image_formats.push("zip".into());
        }
    }

    let meta = MediaMetadata {
        url: original_url.to_string(),
        platform: "facebook".to_string(),
        title: fb.title,
        uploader: fb.uploader,
        duration_seconds: fb.duration_seconds,
        thumbnail_url: fb.thumbnail_url,
        preview_token: None,
        media_id: fb.media_id,
        media_type: Some(if has_video { "video" } else { "image" }.into()),
        format_count: fb.video_variants.len() + fb.images.len(),
        available_qualities,
        available_audio_formats: if has_video && can_merge {
            vec!["mp3".into(), "m4a".into()]
        } else {
            Vec::new()
        },
        available_image_formats: image_formats,
        images: fb.images,
        has_audio: has_video,
        supports_mp4: has_video,
        estimated_size_bytes: None,
        warnings: Vec::new(),
        needs_enrichment: false,
    };

    Some(CachedAnalysis {
        meta,
        resolved_url: Some(resolved_url.to_string()),
        direct_videos: fb.video_variants,
        download_target_url: None,
    })
}

fn tiktok_user_from_url(url_str: &str) -> Option<String> {
    let parsed = Url::parse(url_str).ok()?;
    parsed
        .path()
        .split('/')
        .find(|seg| seg.starts_with('@') && seg.len() > 1)
        .and_then(|s| clean_text(Some(s), 80))
}

fn parse_tiktok_oembed(
    data: &Value,
    resolved_url: &str,
    original_url: &str,
    can_merge: bool,
) -> Option<MediaMetadata> {
    if data.get("embed_type").and_then(Value::as_str) != Some("video") {
        return None;
    }
    let media_id = clean_text(data.get("embed_product_id").and_then(Value::as_str), 80)?;
    let oembed_author = clean_text(
        data.get("author_name")
            .or_else(|| data.get("author_unique_id"))
            .and_then(Value::as_str),
        120,
    );
    let url_user = tiktok_user_from_url(resolved_url).or_else(|| tiktok_user_from_url(original_url));
    let uploader = oembed_author.clone().or(url_user);
    let thumb = validate_image_url(data.get("thumbnail_url").and_then(Value::as_str));
    let raw_title = clean_text(data.get("title").and_then(Value::as_str), 240);
    let needs_enrichment = raw_title.is_none() || thumb.is_none();
    let title = raw_title
        .or_else(|| uploader.as_deref().map(|u| format!("Video TikTok ({u})")))
        .or_else(|| Some(format!("Video TikTok #{media_id}")));

    Some(MediaMetadata {
        url: original_url.to_string(),
        platform: "tiktok".to_string(),
        title,
        uploader,
        duration_seconds: None,
        thumbnail_url: thumb,
        preview_token: None,
        media_id: Some(media_id),
        media_type: Some("video".into()),
        format_count: 1,
        available_qualities: vec!["best".into()],
        available_audio_formats: if can_merge {
            vec!["mp3".into(), "m4a".into()]
        } else {
            Vec::new()
        },
        available_image_formats: Vec::new(),
        images: Vec::new(),
        has_audio: true,
        supports_mp4: true,
        estimated_size_bytes: None,
        warnings: Vec::new(),
        needs_enrichment,
    })
}

async fn fast_analyze_tiktok_video(
    resolved_url: &str,
    original_url: &str,
    can_merge: bool,
) -> Option<MediaMetadata> {
    let path_lower = Url::parse(resolved_url).ok()?.path().to_lowercase();
    if path_lower.contains("/photo/") {
        return None;
    }
    let mut endpoint = Url::parse("https://www.tiktok.com/oembed").ok()?;
    endpoint.query_pairs_mut().append_pair("url", resolved_url);
    let bytes = images::fetch_bytes(
        endpoint.as_str(),
        &[("Accept", "application/json")],
        images::MAX_HTML_BYTES,
        8,
    )
    .await
    .ok()?;
    let data: Value = serde_json::from_slice(&bytes).ok()?;
    parse_tiktok_oembed(&data, resolved_url, original_url, can_merge)
}

fn photo_set_to_metadata(url: &str, platform: &str, photo_set: images::ExtractedPhotoSet) -> MediaMetadata {
    let mut image_formats = vec!["jpg".into(), "png".into(), "webp".into()];
    if photo_set.images.len() > 1 {
        image_formats.push("zip".into());
    }
    let count = photo_set.images.len();
    MediaMetadata {
        url: url.to_string(),
        platform: platform.to_string(),
        title: photo_set.title,
        uploader: photo_set.uploader,
        duration_seconds: None,
        thumbnail_url: Some(photo_set.images[0].url.clone()),
        preview_token: None,
        media_id: photo_set.media_id,
        media_type: Some("image".into()),
        format_count: count,
        available_qualities: Vec::new(),
        available_audio_formats: Vec::new(),
        available_image_formats: image_formats,
        images: photo_set.images,
        has_audio: false,
        supports_mp4: false,
        estimated_size_bytes: None,
        warnings: Vec::new(),
        needs_enrichment: false,
    }
}

fn build_ytdlp_base_args(tools: &ToolPaths) -> Vec<String> {
    let mut args = vec![
        "--ignore-config".into(),
        "--no-playlist".into(),
        "--no-cache-dir".into(),
        "--max-filesize".into(),
        "2048M".into(),
        "--proxy".into(),
        "".into(),
        "--socket-timeout".into(),
        "15".into(),
        "--retries".into(),
        "1".into(),
        "--fragment-retries".into(),
        "1".into(),
        "--extractor-retries".into(),
        "1".into(),
        "--hls-prefer-native".into(),
    ];
    if let Some(node) = &tools.node {
        args.push("--js-runtimes".into());
        args.push(format!("node:{}", node.display()));
    }
    if let Some(ffmpeg) = &tools.ffmpeg {
        if let Some(parent) = ffmpeg.parent() {
            args.push("--ffmpeg-location".into());
            args.push(parent.display().to_string());
        }
    }
    args
}

fn build_ytdlp_video_format_spec(quality: &str, allow_fallback: bool) -> String {
    // Prefer universally compatible H.264 (avc1) + AAC (mp4a) in MP4 so downloaded videos
    // play natively on Windows Media Player, phones, WhatsApp, and editors without requiring
    // separate AV1/VP9 codec extensions.
    if quality == "best" {
        "bestvideo[vcodec^=avc1][ext=mp4]+bestaudio[acodec^=mp4a][ext=m4a]/bestvideo[vcodec^=avc1]+bestaudio[ext=m4a]/best[vcodec^=avc1][ext=mp4]/bestvideo[ext=mp4]+bestaudio[ext=m4a]/best[ext=mp4]/bestvideo+bestaudio/best".to_string()
    } else if allow_fallback {
        format!(
            "bestvideo[height<={quality}][vcodec^=avc1][ext=mp4]+bestaudio[acodec^=mp4a][ext=m4a]/best[height<={quality}][vcodec^=avc1][ext=mp4]/bestvideo[height<={quality}][ext=mp4]+bestaudio[ext=m4a]/best[height<={quality}][ext=mp4]/bestvideo[height<={quality}]+bestaudio/best[height<={quality}]"
        )
    } else {
        format!(
            "bestvideo[height={quality}][vcodec^=avc1][ext=mp4]+bestaudio[acodec^=mp4a][ext=m4a]/best[height={quality}][vcodec^=avc1][ext=mp4]/bestvideo[height={quality}][ext=mp4]+bestaudio[ext=m4a]/best[height={quality}][ext=mp4]/bestvideo[height<={quality}][vcodec^=avc1]+bestaudio/best[height<={quality}]"
        )
    }
}

fn is_reserved_dos_stem(stem: &str) -> bool {
    let first = stem
        .split('.')
        .next()
        .unwrap_or(stem)
        .trim_end_matches([' ', '.'])
        .to_ascii_uppercase();
    matches!(
        first.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

pub fn sanitize_filename(title: Option<&str>, platform: &str, ext: &str) -> String {
    let base = title.unwrap_or(platform);
    let mut cleaned: String = base
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() {
                ' '
            } else {
                c
            }
        })
        .collect();
    cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed: String = cleaned.chars().take(90).collect();
    let stem = trimmed.trim_matches(|c| c == '.' || c == ' ');
    let safe_stem = if stem.is_empty() {
        platform.to_string()
    } else if is_reserved_dos_stem(stem) {
        format!("_{stem}")
    } else {
        stem.to_string()
    };
    let clean_ext = ext
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase();
    let safe_ext = match clean_ext.as_str() {
        "mp4" | "mkv" | "webm" | "mp3" | "m4a" | "jpg" | "jpeg" | "png" | "webp" | "zip" => {
            clean_ext.as_str()
        }
        _ => "bin",
    };
    format!("{safe_stem}.{safe_ext}")
}

pub fn unique_dest_path(download_dir: &Path, desired_name: &str) -> PathBuf {
    let path = Path::new(desired_name);
    let raw_stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("media");
    let stem = if is_reserved_dos_stem(raw_stem) {
        format!("_{raw_stem}")
    } else {
        raw_stem.to_string()
    };
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("bin");
    let first = download_dir.join(format!("{stem}.{ext}"));
    if std::fs::symlink_metadata(&first).is_err() {
        return first;
    }
    for i in 2..=999 {
        let candidate = download_dir.join(format!("{stem} ({i}).{ext}"));
        if std::fs::symlink_metadata(&candidate).is_err() {
            return candidate;
        }
    }
    download_dir.join(format!("{stem}-{}.{ext}", uuid::Uuid::new_v4().simple()))
}

fn pick_direct_variant<'a>(
    variants: &'a [DirectVideoVariant],
    quality: &str,
    allow_fallback: bool,
) -> Option<&'a DirectVideoVariant> {
    if variants.is_empty() {
        return None;
    }
    if quality == "best" {
        return variants.first();
    }
    let target: u32 = quality.parse().ok()?;
    if let Some(exact) = variants
        .iter()
        .find(|v| v.height >= target.saturating_sub(40) && v.height <= target + 40)
    {
        return Some(exact);
    }
    if allow_fallback {
        return variants
            .iter()
            .find(|v| v.height > 0 && v.height <= target)
            .or_else(|| variants.last());
    }
    variants.first()
}

pub struct AppEngine {
    pub db: Arc<Database>,
    pub previews: Arc<PreviewCache>,
    pub semaphore: Arc<Semaphore>,
    tools: RwLock<ToolPaths>,
    cancels: Arc<Mutex<HashMap<String, watch::Sender<bool>>>>,
    analyze_cache: Arc<Mutex<HashMap<String, (Instant, CachedAnalysis)>>>,
}

impl AppEngine {
    pub fn new(db: Arc<Database>, previews: Arc<PreviewCache>) -> Self {
        Self {
            db,
            previews,
            semaphore: Arc::new(Semaphore::new(2)),
            tools: RwLock::new(discover_tools()),
            cancels: Arc::new(Mutex::new(HashMap::new())),
            analyze_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn update_tools(&self, paths: ToolPaths) {
        *self.tools.write().unwrap() = paths;
    }

    pub fn current_tools(&self) -> ToolPaths {
        self.tools.read().unwrap().clone()
    }

    fn annotate_previews(&self, mut meta: MediaMetadata) -> MediaMetadata {
        meta.preview_token = self.previews.register(meta.thumbnail_url.as_deref());
        for img in &mut meta.images {
            img.preview_token = self.previews.register(Some(&img.url));
        }
        meta
    }

    fn store_cache(&self, url: &str, entry: &CachedAnalysis) {
        let mut cache = self.analyze_cache.lock().unwrap();
        if cache.len() > 64 {
            cache.clear();
        }
        cache.insert(
            url.to_string(),
            (Instant::now() + Duration::from_secs(300), entry.clone()),
        );
    }

    pub async fn analyze(&self, raw_url: &str) -> Result<MediaMetadata, AppError> {
        let (url, platform) = validate_url(raw_url)?;
        {
            let cache = self.analyze_cache.lock().unwrap();
            if let Some((exp, cached)) = cache.get(&url) {
                if *exp > Instant::now() {
                    return Ok(self.annotate_previews(cached.meta.clone()));
                }
            }
        }

        let tools = self.current_tools();
        let resolved_url = resolve_alias(&url, &platform).await?;
        let can_merge = tools.ffmpeg.is_some() && tools.ffprobe.is_some();

        // === FAST PATH 1: YouTube pure-Rust HTML player parse (~0.5–1.0s) ===
        if platform == "youtube" {
            if let Some(fast_res) = fast_analyze_youtube(&resolved_url, can_merge).await {
                let meta = fast_res?;
                self.store_cache(
                    &url,
                    &CachedAnalysis {
                        meta: meta.clone(),
                        resolved_url: Some(resolved_url.clone()),
                        direct_videos: Vec::new(),
                        download_target_url: None,
                    },
                );
                return Ok(self.annotate_previews(meta));
            }
        }

        // === FAST PATH 2: X / Twitter pure-Rust syndication (main + quote-tweet video & photos, ~0.3–0.6s) ===
        let mut direct_tried = false;
        if platform == "x" {
            direct_tried = true;
            if let Some(mut entry) = fast_analyze_twitter(&resolved_url, can_merge).await {
                entry.meta.url = url.clone();
                self.store_cache(&url, &entry);
                return Ok(self.annotate_previews(entry.meta));
            }
        }

        // === FAST PATH 3: Instagram pure-Rust Polaris (photos, carousels, AND Reels ~0.8–1.3s) ===
        if platform == "instagram" {
            direct_tried = true;
            if let Some(mut entry) = fast_analyze_instagram(&resolved_url, can_merge).await {
                entry.meta.url = url.clone();
                self.store_cache(&url, &entry);
                return Ok(self.annotate_previews(entry.meta));
            }
        }

        // === FAST PATH 3.2: Threads pure-Rust extractor (photos, carousels, videos, share links ~0.7–1.4s) ===
        if platform == "threads" {
            direct_tried = true;
            if let Some(entry) = fast_analyze_threads(&resolved_url, &url, can_merge).await {
                self.store_cache(&url, &entry);
                return Ok(self.annotate_previews(entry.meta));
            }
        }

        // === FAST PATH 3.3: Pinterest pure-Rust extractor (photo pins AND video pins ~1.0–1.5s) ===
        if platform == "pinterest" {
            direct_tried = true;
            if let Some(entry) = fast_analyze_pinterest(&resolved_url, &url, can_merge).await {
                self.store_cache(&url, &entry);
                return Ok(self.annotate_previews(entry.meta));
            }
        }

        // === FAST PATH 3.4: Facebook pure-Rust extractor (videos, reels, photos & albums ~0.9–1.5s) ===
        if platform == "facebook" {
            direct_tried = true;
            if let Some(entry) = fast_analyze_facebook(&resolved_url, &url, can_merge).await {
                self.store_cache(&url, &entry);
                return Ok(self.annotate_previews(entry.meta));
            }
        }

        // === FAST PATH 3.5: TikTok video via official oEmbed (~0.4–0.6s, zero yt-dlp during analyze!) ===
        if platform == "tiktok" {
            if let Some(meta) = fast_analyze_tiktok_video(&resolved_url, &url, can_merge).await {
                self.store_cache(
                    &url,
                    &CachedAnalysis {
                        meta: meta.clone(),
                        resolved_url: Some(resolved_url.clone()),
                        direct_videos: Vec::new(),
                        download_target_url: None,
                    },
                );
                return Ok(self.annotate_previews(meta));
            }
        }

        // === FAST PATH 4: Direct photo posts (Pinterest /pin/, TikTok /photo/) ===
        let path_lower = Url::parse(&resolved_url)
            .map(|u| u.path().to_lowercase())
            .unwrap_or_default();
        let photo_first = (platform == "pinterest" && path_lower.contains("/pin/"))
            || (platform == "tiktok" && path_lower.contains("/photo/"));

        if photo_first {
            if let Some(photo_set) = images::extract_images(&resolved_url, &platform, &tools).await {
                if !photo_set.images.is_empty() {
                    let meta = photo_set_to_metadata(&url, &platform, photo_set);
                    self.store_cache(
                        &url,
                        &CachedAnalysis {
                            meta: meta.clone(),
                            resolved_url: Some(resolved_url.clone()),
                            direct_videos: Vec::new(),
                            download_target_url: None,
                        },
                    );
                    return Ok(self.annotate_previews(meta));
                }
            }
            if platform == "tiktok" && path_lower.contains("/photo/") {
                return Err(AppError::new("UNAVAILABLE_MEDIA"));
            }
        }

        // === STANDARD PATH: yt-dlp (Reels, TikTok videos, Vimeo, Facebook, fallbacks) ===
        let mut yt_result: Option<MediaMetadata> = None;
        let mut yt_error: Option<AppError> = None;
        let mut stderr_warning_code: Option<&'static str> = None;

        if let Some(yt_dlp) = &tools.yt_dlp {
            let mut args = build_ytdlp_base_args(&tools);
            args.extend([
                "--dump-single-json".into(),
                "--skip-download".into(),
                "--ignore-no-formats-error".into(),
                "--".into(),
                resolved_url.clone(),
            ]);
            match tokio::time::timeout(
                Duration::from_secs(45),
                background_cmd(yt_dlp).args(&args).output(),
            )
            .await
            {
                Ok(Ok(output)) => {
                    let stderr_str = String::from_utf8_lossy(&output.stderr);
                    let classified = classify_extractor_error(&stderr_str);
                    if matches!(classified.code, "LOGIN_REQUIRED" | "GEO_RESTRICTED") {
                        stderr_warning_code = Some(classified.code);
                    }
                    if output.status.success() {
                        if let Ok(val) = serde_json::from_slice::<Value>(&output.stdout) {
                            match normalize_ytdlp_info(&val, &url, &platform, can_merge) {
                                Ok(meta) => yt_result = Some(meta),
                                Err(e) => yt_error = Some(e),
                            }
                        } else {
                            yt_error = Some(AppError::new("EXTRACTOR_FAILED"));
                        }
                    } else {
                        yt_error = Some(classify_extractor_error(&stderr_str));
                    }
                }
                Ok(Err(_)) => yt_error = Some(AppError::new("DEPENDENCY_MISSING")),
                Err(_) => yt_error = Some(AppError::new("TIMEOUT")),
            }
        } else {
            yt_error = Some(AppError::new("DEPENDENCY_MISSING"));
        }

        let has_video_qualities = yt_result
            .as_ref()
            .is_some_and(|r| !r.available_qualities.is_empty());
        // Avoid spawning a slow redundant gallery-dl probe when yt-dlp already found video formats
        // (especially on X `/status/` video tweets where looks_like_photo_url previously matched).
        let need_images = !photo_first
            && (yt_result.is_none()
                || !has_video_qualities
                || (!direct_tried && images::looks_like_photo_url(&resolved_url, &platform)));

        if need_images {
            if let Some(photo_set) =
                images::extract_images_with_options(&resolved_url, &platform, &tools, direct_tried)
                    .await
            {
                if !photo_set.images.is_empty() {
                    let count = photo_set.images.len();
                    let mut image_formats = vec!["jpg".into(), "png".into(), "webp".into()];
                    if count > 1 {
                        image_formats.push("zip".into());
                    }
                    let final_meta = match yt_result {
                        None => photo_set_to_metadata(&url, &platform, photo_set),
                        Some(mut meta) => {
                            if meta.thumbnail_url.is_none() {
                                meta.thumbnail_url = Some(photo_set.images[0].url.clone());
                            }
                            if meta.uploader.is_none() {
                                meta.uploader = photo_set.uploader;
                            }
                            if meta.media_id.is_none() {
                                meta.media_id = photo_set.media_id;
                            }
                            meta.available_image_formats = image_formats;
                            meta.images = photo_set.images;
                            if meta.available_qualities.is_empty() {
                                meta.media_type = Some("image".into());
                                meta.format_count = meta.format_count.max(count);
                                meta.warnings.clear();
                                if let Some(t) = photo_set.title {
                                    meta.title = Some(t);
                                }
                            } else if meta.title.is_none() {
                                meta.title = photo_set.title;
                            }
                            meta
                        }
                    };
                    self.store_cache(
                        &url,
                        &CachedAnalysis {
                            meta: final_meta.clone(),
                            resolved_url: Some(resolved_url.clone()),
                            direct_videos: Vec::new(),
                            download_target_url: None,
                        },
                    );
                    return Ok(self.annotate_previews(final_meta));
                }
            }
        }

        if let Some(meta) = yt_result {
            if meta.available_qualities.is_empty()
                && meta.available_audio_formats.is_empty()
                && meta.available_image_formats.is_empty()
            {
                if let Some(code) = stderr_warning_code {
                    return Err(AppError::new(code));
                }
            }
            self.store_cache(
                &url,
                &CachedAnalysis {
                    meta: meta.clone(),
                    resolved_url: Some(resolved_url.clone()),
                    direct_videos: Vec::new(),
                    download_target_url: None,
                },
            );
            return Ok(self.annotate_previews(meta));
        }

        Err(yt_error.unwrap_or_else(|| AppError::new("EXTRACTOR_FAILED")))
    }

    pub async fn enrich(&self, raw_url: &str) -> Result<MediaMetadata, AppError> {
        let (url, platform) = validate_url(raw_url)?;
        let tools = self.current_tools();
        let yt_dlp = tools
            .yt_dlp
            .as_ref()
            .ok_or_else(|| AppError::new("DEPENDENCY_MISSING"))?;
        let resolved_url = resolve_alias(&url, &platform).await?;
        let can_merge = tools.ffmpeg.is_some() && tools.ffprobe.is_some();

        let mut args = build_ytdlp_base_args(&tools);
        args.extend([
            "--dump-single-json".into(),
            "--skip-download".into(),
            "--ignore-no-formats-error".into(),
            "--".into(),
            resolved_url.clone(),
        ]);
        let output = tokio::time::timeout(
            Duration::from_secs(30),
            background_cmd(yt_dlp).args(&args).output(),
        )
        .await
        .map_err(|_| AppError::new("TIMEOUT"))?
        .map_err(|_| AppError::new("DEPENDENCY_MISSING"))?;

        if !output.status.success() {
            return Err(classify_extractor_error(&String::from_utf8_lossy(
                &output.stderr,
            )));
        }
        let val: Value = serde_json::from_slice(&output.stdout)
            .map_err(|_| AppError::new("EXTRACTOR_FAILED"))?;
        let meta = normalize_ytdlp_info(&val, &url, &platform, can_merge)?;
        self.store_cache(
            &url,
            &CachedAnalysis {
                meta: meta.clone(),
                resolved_url: Some(resolved_url),
                direct_videos: Vec::new(),
                download_target_url: None,
            },
        );
        Ok(self.annotate_previews(meta))
    }

    pub fn enqueue(
        self: &Arc<Self>,
        req: CreateDownloadRequest,
        retry_of: Option<String>,
    ) -> Result<JobRecord, AppError> {
        let (url, platform) = validate_url(&req.url)?;
        match req.kind.as_str() {
            "video" => {
                if req.container != "mp4"
                    || !matches!(req.quality.as_str(), "best" | "1080" | "720" | "480" | "360")
                {
                    return Err(AppError::new("INVALID_REQUEST"));
                }
            }
            "audio" => {
                if !matches!(req.container.as_str(), "mp3" | "m4a") {
                    return Err(AppError::new("INVALID_REQUEST"));
                }
                if let Some(br) = req.audio_bitrate_kbps {
                    if !matches!(br, 128 | 192 | 256 | 320) {
                        return Err(AppError::new("INVALID_REQUEST"));
                    }
                }
            }
            "image" => {
                if !matches!(req.container.as_str(), "jpg" | "png" | "webp" | "zip") {
                    return Err(AppError::new("INVALID_REQUEST"));
                }
            }
            _ => return Err(AppError::new("INVALID_REQUEST")),
        }

        let custom_output_dir = match req
            .output_dir
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(dir_str) => Some(validate_download_dir(dir_str)?),
            None => None,
        };

        let cached = self
            .analyze_cache
            .lock()
            .unwrap()
            .get(&url)
            .filter(|(exp, _)| *exp > Instant::now())
            .map(|(_, m)| m.clone());

        let now = Utc::now().to_rfc3339();
        let job = JobRecord {
            id: uuid::Uuid::new_v4().to_string(),
            url: url.clone(),
            title: cached.as_ref().and_then(|c| c.meta.title.clone()),
            platform,
            kind: req.kind.clone(),
            quality: req.quality.clone(),
            container: req.container.clone(),
            audio_bitrate_kbps: if req.kind == "audio" && req.container == "mp3" {
                Some(req.audio_bitrate_kbps.unwrap_or(192))
            } else {
                None
            },
            allow_quality_fallback: req.allow_quality_fallback,
            image_index: req.image_index,
            status: "queued".into(),
            progress_percent: Some(0.0),
            downloaded_bytes: None,
            total_bytes: None,
            speed_bytes_per_second: None,
            eta_seconds: None,
            file_size_bytes: None,
            output_filename: None,
            output_path: None,
            thumbnail_url: cached.as_ref().and_then(|c| c.meta.thumbnail_url.clone()),
            thumbnail_preview: None,
            error_code: None,
            error_message: None,
            file_available: false,
            created_at: now.clone(),
            updated_at: now,
            completed_at: None,
            retry_of,
        };

        self.db.insert_job(&job, QUEUE_LIMIT)?;
        let (tx, rx) = watch::channel(false);
        self.cancels.lock().unwrap().insert(job.id.clone(), tx);

        let engine = Arc::clone(self);
        let job_clone = job.clone();
        tauri::async_runtime::spawn(async move {
            engine.run_job(job_clone, cached, custom_output_dir, rx).await;
        });

        Ok(job)
    }

    pub fn cancel(&self, job_id: &str) -> Result<(), AppError> {
        self.db.mark_cancelled(job_id)?;
        if let Some(tx) = self.cancels.lock().unwrap().get(job_id) {
            let _ = tx.send(true);
        }
        Ok(())
    }

    async fn run_job(
        self: Arc<Self>,
        job: JobRecord,
        cached: Option<CachedAnalysis>,
        custom_output_dir: Option<PathBuf>,
        mut cancel_rx: watch::Receiver<bool>,
    ) {
        let _permit = match self.semaphore.clone().acquire_owned().await {
            Ok(p) => p,
            Err(_) => return,
        };
        if *cancel_rx.borrow() {
            self.cancels.lock().unwrap().remove(&job.id);
            return;
        }

        let res = self
            .execute_job(&job, cached, custom_output_dir, &mut cancel_rx)
            .await;
        self.cancels.lock().unwrap().remove(&job.id);

        if *cancel_rx.borrow() {
            return;
        }
        if let Err(err) = res {
            self.db.mark_failed(&job.id, err.code, err.message);
        }
    }

    async fn execute_job(
        &self,
        job: &JobRecord,
        cached: Option<CachedAnalysis>,
        custom_output_dir: Option<PathBuf>,
        cancel_rx: &mut watch::Receiver<bool>,
    ) -> Result<(), AppError> {
        let prefs = self.db.get_preferences()?;
        let download_dir = match custom_output_dir {
            Some(d) => d,
            None => validate_download_dir(&prefs.download_dir)?,
        };
        std::fs::create_dir_all(&download_dir).map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;
        let download_dir = ensure_safe_canonical_dir(&download_dir)?;
        let stage_dir = download_dir.join(".tmp").join(&job.id);
        std::fs::create_dir_all(&stage_dir).map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;

        let result = self
            .execute_in_stage(job, cached, &download_dir, &stage_dir, cancel_rx)
            .await;

        let _ = std::fs::remove_dir_all(&stage_dir);
        result
    }

    async fn stream_direct_file(
        &self,
        job_id: &str,
        platform: &str,
        url_str: &str,
        dest: &Path,
        cancel_rx: &mut watch::Receiver<bool>,
    ) -> Result<i64, AppError> {
        let safe_url =
            validate_video_cdn_url(url_str, platform).ok_or_else(|| AppError::new("FORBIDDEN"))?;
        ensure_public_host(&safe_url).await?;
        let mut resp = SHARED_HTTP
            .get(&safe_url)
            .timeout(Duration::from_secs(300))
            .send()
            .await
            .map_err(|_| AppError::new("NETWORK_ERROR"))?;
        if !resp.status().is_success() {
            return Err(classify_extractor_error(&format!(
                "HTTP error {}",
                resp.status().as_u16()
            )));
        }
        let total = resp.content_length().map(|n| n as i64);
        if let Some(t) = total {
            if t > MAX_DIRECT_VIDEO_BYTES {
                return Err(AppError::new("FILE_TOO_LARGE"));
            }
        }
        let mut file =
            std::fs::File::create(dest).map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;
        let mut downloaded: i64 = 0;
        let started = Instant::now();
        let mut last_emit = Instant::now();

        loop {
            let chunk = tokio::select! {
                c = resp.chunk() => c.map_err(|_| AppError::new("NETWORK_ERROR"))?,
                _ = cancel_rx.changed() => {
                    drop(file);
                    let _ = std::fs::remove_file(dest);
                    return Ok(0);
                }
            };
            let Some(bytes) = chunk else { break };
            downloaded = downloaded.saturating_add(bytes.len() as i64);
            if downloaded > MAX_DIRECT_VIDEO_BYTES {
                drop(file);
                let _ = std::fs::remove_file(dest);
                return Err(AppError::new("FILE_TOO_LARGE"));
            }
            if file.write_all(&bytes).is_err() {
                drop(file);
                let _ = std::fs::remove_file(dest);
                return Err(AppError::new("STORAGE_UNAVAILABLE"));
            }
            if last_emit.elapsed() >= Duration::from_millis(250) {
                last_emit = Instant::now();
                let elapsed = started.elapsed().as_secs_f64().max(0.1);
                let speed = downloaded as f64 / elapsed;
                let pct = total.filter(|&t| t > 0).map(|t| ((downloaded as f64 / t as f64) * 95.0).min(95.0));
                let eta = total
                    .filter(|&t| t > downloaded && speed > 1.0)
                    .map(|t| ((t - downloaded) as f64 / speed) as i64);
                self.db
                    .update_progress(job_id, "downloading", pct, Some(downloaded), total, Some(speed), eta);
            }
        }
        Ok(downloaded)
    }

    async fn execute_in_stage(
        &self,
        job: &JobRecord,
        cached: Option<CachedAnalysis>,
        download_dir: &Path,
        stage_dir: &Path,
        cancel_rx: &mut watch::Receiver<bool>,
    ) -> Result<(), AppError> {
        let tools = self.current_tools();
        let resolved = match cached.as_ref().and_then(|c| c.resolved_url.clone()) {
            Some(r) => r,
            None => resolve_alias(&job.url, &job.platform).await?,
        };
        self.db
            .update_progress(&job.id, "downloading", Some(1.0), None, None, None, None);

        if job.kind == "image" {
            let cached_photos = cached
                .as_ref()
                .filter(|c| !c.meta.images.is_empty())
                .map(|c| images::ExtractedPhotoSet {
                    title: c.meta.title.clone(),
                    uploader: c.meta.uploader.clone(),
                    media_id: c.meta.media_id.clone(),
                    images: c.meta.images.clone(),
                });
            let photo_set = match cached_photos {
                Some(set) => set,
                None => images::extract_images(&resolved, &job.platform, &tools)
                    .await
                    .ok_or_else(|| AppError::new("FORMAT_UNAVAILABLE"))?,
            };
            let stage_file = stage_dir.join(format!("media.{}", job.container));
            let db = Arc::clone(&self.db);
            let jid = job.id.clone();
            let size = images::download_photo_set(
                &photo_set,
                &job.container,
                job.image_index,
                &stage_file,
                |pct, bytes| {
                    db.update_progress(&jid, "downloading", Some(pct), Some(bytes), None, None, None);
                },
            )
            .await?;
            if *cancel_rx.borrow() {
                return Ok(());
            }
            let title = photo_set.title.or_else(|| job.title.clone());
            let thumb = photo_set
                .images
                .first()
                .map(|i| i.url.clone())
                .or_else(|| job.thumbnail_url.clone());
            let desired = sanitize_filename(title.as_deref(), &job.platform, &job.container);
            let dest = unique_dest_path(download_dir, &desired);
            std::fs::rename(&stage_file, &dest)
                .or_else(|_| std::fs::copy(&stage_file, &dest).map(|_| ()))
                .map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;
            let fname = dest
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(&desired)
                .to_string();
            self.db.mark_completed(
                &job.id,
                title.as_deref(),
                thumb.as_deref(),
                &fname,
                &dest.display().to_string(),
                size,
            );
            return Ok(());
        }

        // Fast direct MP4 download path for X / Twitter (including quote-tweet videos!)
        let mut direct_variants = cached
            .as_ref()
            .map(|c| c.direct_videos.clone())
            .unwrap_or_default();
        let mut target_url = cached
            .as_ref()
            .and_then(|c| c.download_target_url.clone())
            .unwrap_or_else(|| resolved.clone());

        if job.platform == "x" && direct_variants.is_empty() {
            if let Some(syn) = images::fetch_twitter_syndication(&resolved).await {
                direct_variants = syn.video_variants;
                if let Some(qu) = syn.quoted_tweet_url {
                    target_url = qu;
                }
            }
        }

        if job.platform == "threads" && direct_variants.is_empty() {
            if let Some(th) = images::fetch_threads_media(&resolved).await {
                direct_variants = th.video_variants;
            }
        }

        if job.platform == "pinterest" && direct_variants.is_empty() {
            if let Some(pin) = images::fetch_pinterest_media(&resolved).await {
                direct_variants = pin.video_variants;
            }
        }

        if job.platform == "facebook" && direct_variants.is_empty() {
            if let Some(fb) = images::fetch_facebook_media(&resolved).await {
                direct_variants = fb.video_variants;
            }
        }

        if job.platform == "instagram" && direct_variants.is_empty() {
            if let Some(ig) = images::fetch_instagram_polaris(&resolved).await {
                direct_variants = ig.video_variants;
            }
        }

        if !direct_variants.is_empty() {
            if let Some(chosen) =
                pick_direct_variant(&direct_variants, &job.quality, job.allow_quality_fallback)
            {
                let raw_mp4 = stage_dir.join("media.mp4");
                let dl_size = self
                    .stream_direct_file(&job.id, &job.platform, &chosen.url, &raw_mp4, cancel_rx)
                    .await?;
                if *cancel_rx.borrow() {
                    return Ok(());
                }
                let final_stage = if job.kind == "audio" {
                    let ffmpeg = tools
                        .ffmpeg
                        .as_ref()
                        .ok_or_else(|| AppError::new("FFMPEG_MISSING"))?;
                    self.db
                        .update_progress(&job.id, "processing", Some(96.0), None, None, None, None);
                    let out_audio = stage_dir.join(format!("audio.{}", job.container));
                    let bitrate = match job.audio_bitrate_kbps.unwrap_or(192) {
                        128 => 128,
                        256 => 256,
                        320 => 320,
                        _ => 192,
                    };
                    let mut ff_args = vec![
                        "-nostdin".into(),
                        "-hide_banner".into(),
                        "-loglevel".into(),
                        "error".into(),
                        "-y".into(),
                        "-protocol_whitelist".into(),
                        "file".into(),
                        "-f".into(),
                        "mp4".into(),
                        "-i".into(),
                        raw_mp4.display().to_string(),
                        "-vn".into(),
                    ];
                    if job.container == "mp3" {
                        ff_args.extend([
                            "-c:a".into(),
                            "libmp3lame".into(),
                            "-b:a".into(),
                            format!("{bitrate}k"),
                        ]);
                    } else {
                        ff_args.extend(["-c:a".into(), "aac".into()]);
                    }
                    ff_args.push(out_audio.display().to_string());
                    let mut ff_child = background_cmd(ffmpeg)
                        .args(&ff_args)
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .spawn()
                        .map_err(|_| AppError::new("FFMPEG_MISSING"))?;
                    let ff_status = tokio::select! {
                        st = ff_child.wait() => st.map_err(|_| AppError::new("EXTRACTOR_FAILED"))?,
                        _ = cancel_rx.changed() => {
                            let _ = ff_child.kill().await;
                            let _ = ff_child.wait().await;
                            return Ok(());
                        }
                        _ = tokio::time::sleep(Duration::from_secs(FFMPEG_TIMEOUT_SECS)) => {
                            let _ = ff_child.kill().await;
                            let _ = ff_child.wait().await;
                            return Err(AppError::new("TIMEOUT"));
                        }
                    };
                    if !ff_status.success() {
                        return Err(AppError::new("EXTRACTOR_FAILED"));
                    }
                    out_audio
                } else {
                    raw_mp4
                };

                let size = std::fs::metadata(&final_stage)
                    .map(|m| m.len() as i64)
                    .unwrap_or(dl_size);
                let desired = sanitize_filename(job.title.as_deref(), &job.platform, &job.container);
                let dest = unique_dest_path(download_dir, &desired);
                std::fs::rename(&final_stage, &dest)
                    .or_else(|_| std::fs::copy(&final_stage, &dest).map(|_| ()))
                    .map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;
                let fname = dest
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(&desired)
                    .to_string();
                self.db.mark_completed(
                    &job.id,
                    job.title.as_deref(),
                    job.thumbnail_url.as_deref(),
                    &fname,
                    &dest.display().to_string(),
                    size,
                );
                return Ok(());
            }
        }

        let yt_dlp = tools
            .yt_dlp
            .as_ref()
            .ok_or_else(|| AppError::new("DEPENDENCY_MISSING"))?;
        if job.kind == "audio" && (tools.ffmpeg.is_none() || tools.ffprobe.is_none()) {
            return Err(AppError::new("FFMPEG_MISSING"));
        }

        let mut args = build_ytdlp_base_args(&tools);
        args.extend([
            "--newline".into(),
            "--no-overwrites".into(),
            "--no-continue".into(),
            "--concurrent-fragments".into(),
            "8".into(),
            "--progress-delta".into(),
            "0.25".into(),
            "--progress-template".into(),
            "download:MVPROG|%(progress.downloaded_bytes)s|%(progress.total_bytes)s|%(progress.total_bytes_estimate)s|%(progress.speed)s|%(progress.eta)s|%(progress.fragment_index)s|%(progress.fragment_count)s".into(),
            "-o".into(),
            stage_dir.join("media.%(ext)s").display().to_string(),
        ]);

        if job.kind == "audio" {
            args.extend([
                "-f".into(),
                "bestaudio/best".into(),
                "-x".into(),
                "--audio-format".into(),
                job.container.clone(),
            ]);
            if job.container == "mp3" {
                args.push("--audio-quality".into());
                args.push(format!("{}K", job.audio_bitrate_kbps.unwrap_or(192)));
            }
        } else {
            let fmt_spec = build_ytdlp_video_format_spec(&job.quality, job.allow_quality_fallback);
            args.extend([
                "-S".into(),
                "res,fps,br,vcodec:h264,ext:mp4:m4a".into(),
                "-f".into(),
                fmt_spec,
                "--merge-output-format".into(),
                "mp4".into(),
            ]);
        }

        args.push("--".into());
        args.push(target_url);

        let mut child = background_cmd(yt_dlp)
            .env("PYTHONUNBUFFERED", "1")
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|_| AppError::new("DEPENDENCY_MISSING"))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AppError::new("EXTRACTOR_FAILED"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| AppError::new("EXTRACTOR_FAILED"))?;
        let db_prog = Arc::clone(&self.db);
        let jid = job.id.clone();

        let out_task = tauri::async_runtime::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if let Some(rest) = line.strip_prefix("MVPROG|") {
                    let parts: Vec<&str> = rest.split('|').collect();
                    if parts.len() >= 5 {
                        let dl = parts[0].parse::<i64>().ok();
                        let tot = parts[1]
                            .parse::<i64>()
                            .ok()
                            .or_else(|| parts[2].parse::<i64>().ok());
                        let spd = parts[3].parse::<f64>().ok().filter(|v| v.is_finite());
                        let eta = parts[4].parse::<i64>().ok();
                        let frag_i = parts.get(5).and_then(|s| s.parse::<f64>().ok());
                        let frag_n = parts.get(6).and_then(|s| s.parse::<f64>().ok());
                        let pct = match (dl, tot) {
                            (Some(d), Some(t)) if t > 0 => {
                                Some(((d as f64 / t as f64) * 95.0).min(95.0))
                            }
                            _ => match (frag_i, frag_n) {
                                (Some(fi), Some(fn_tot)) if fn_tot > 0.0 => {
                                    Some(((fi / fn_tot) * 95.0).clamp(1.0, 95.0))
                                }
                                _ => None,
                            },
                        };
                        db_prog.update_progress(&jid, "downloading", pct, dl, tot, spd, eta);
                    }
                } else if line.contains("[Merger]") || line.contains("[ExtractAudio]") {
                    db_prog.update_progress(&jid, "processing", Some(97.0), None, None, None, None);
                }
            }
        });

        let err_task = tauri::async_runtime::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            let mut acc = String::new();
            while let Ok(Some(line)) = reader.next_line().await {
                if acc.len() < 4096 {
                    acc.push_str(&line);
                    acc.push('\n');
                }
            }
            acc
        });

        let status = tokio::select! {
            st = child.wait() => st.map_err(|_| AppError::new("EXTRACTOR_FAILED"))?,
            _ = cancel_rx.changed() => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Ok(());
            }
            _ = tokio::time::sleep(Duration::from_secs(DOWNLOAD_TIMEOUT_SECS)) => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(AppError::new("TIMEOUT"));
            }
        };

        let _ = out_task.await;
        let err_text = err_task.await.unwrap_or_default();

        if *cancel_rx.borrow() {
            return Ok(());
        }
        if !status.success() {
            return Err(classify_extractor_error(&err_text));
        }

        let target_ext = &job.container;
        let mut produced = stage_dir.join(format!("media.{target_ext}"));
        if !produced.is_file() {
            if let Ok(entries) = std::fs::read_dir(stage_dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_file() {
                        if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                            if name.starts_with("media.") && !name.ends_with(".part") {
                                produced = p;
                                break;
                            }
                        }
                    }
                }
            }
        }
        if !produced.is_file() {
            return Err(AppError::new("FORMAT_UNAVAILABLE"));
        }

        let size = std::fs::metadata(&produced)
            .map(|m| m.len() as i64)
            .unwrap_or(0);
        let raw_ext = produced
            .extension()
            .and_then(|s| s.to_str())
            .map(str::to_ascii_lowercase);
        let final_ext = match raw_ext.as_deref() {
            Some(ext @ ("mp4" | "mkv" | "webm" | "mp3" | "m4a" | "jpg" | "jpeg" | "png" | "webp" | "zip")) => ext,
            _ => target_ext.as_str(),
        };
        let desired = sanitize_filename(job.title.as_deref(), &job.platform, final_ext);
        let dest = unique_dest_path(download_dir, &desired);
        std::fs::rename(&produced, &dest)
            .or_else(|_| std::fs::copy(&produced, &dest).map(|_| ()))
            .map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;
        let fname = dest
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&desired)
            .to_string();
        self.db.mark_completed(
            &job.id,
            job.title.as_deref(),
            job.thumbnail_url.as_deref(),
            &fname,
            &dest.display().to_string(),
            size,
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tiktok_oembed_video_and_ignores_deleted() {
        let valid: Value = serde_json::json!({
            "embed_type": "video",
            "title": "Demo TikTok",
            "author_name": "Creator",
            "thumbnail_url": "https://p16-sign-sg.tiktokcdn.com/obj/thumb.jpeg",
            "embed_product_id": "7123456789012345678"
        });
        let meta = parse_tiktok_oembed(
            &valid,
            "https://www.tiktok.com/@creator/video/7123456789012345678",
            "https://vt.tiktok.com/Zabc/",
            true,
        )
        .unwrap();
        assert!(meta.supports_mp4);
        assert!(!meta.needs_enrichment);
        assert_eq!(meta.available_qualities, vec!["best"]);
        assert_eq!(meta.available_audio_formats, vec!["mp3", "m4a"]);

        // 2026 minimal oEmbed response (no thumbnail/title -> instant open + background enrichment)
        let minimal: Value = serde_json::json!({
            "embed_type": "video",
            "embed_product_id": "7106594312292453675"
        });
        let meta2 = parse_tiktok_oembed(
            &minimal,
            "https://www.tiktok.com/@tiktok/video/7106594312292453675",
            "https://vt.tiktok.com/Zmin/",
            true,
        )
        .unwrap();
        assert!(meta2.needs_enrichment);
        assert_eq!(meta2.uploader.as_deref(), Some("@tiktok"));

        let bad: Value = serde_json::json!({"embed_type": "video"});
        assert!(parse_tiktok_oembed(&bad, "https://www.tiktok.com/@x/video/1", "https://vt.tiktok.com/Zbad/", true).is_none());
    }

    #[test]
    fn enqueue_works_from_sync_thread_without_tokio_runtime() {
        let dir = std::env::temp_dir().join(format!("mv_sync_enqueue_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Arc::new(Database::open(&dir.join("app.db"), dir.join("dl")).unwrap());
        let previews = Arc::new(PreviewCache::new());
        let engine = Arc::new(AppEngine::new(db, previews));

        // Regression test: previously calling enqueue() outside a Tokio runtime
        // (such as from a sync Tauri command on the WebView UI thread) panicked and
        // force-closed the app.
        let job = engine
            .enqueue(
                CreateDownloadRequest {
                    url: "https://youtu.be/dQw4w9WgXcQ".into(),
                    kind: "video".into(),
                    quality: "best".into(),
                    container: "mp4".into(),
                    audio_bitrate_kbps: None,
                    allow_quality_fallback: false,
                    image_index: None,
                    output_dir: Some(dir.join("custom").display().to_string()),
                },
                None,
            )
            .expect("enqueue from sync thread must not fail or panic");
        assert_eq!(job.status, "queued");
        let _ = engine.cancel(&job.id);
        std::thread::sleep(Duration::from_millis(80));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prefers_universal_h264_aac_format_spec() {
        let best = build_ytdlp_video_format_spec("best", false);
        assert!(best.starts_with("bestvideo[vcodec^=avc1][ext=mp4]+bestaudio[acodec^=mp4a][ext=m4a]/"));
        let p1080 = build_ytdlp_video_format_spec("1080", false);
        assert!(p1080.starts_with("bestvideo[height=1080][vcodec^=avc1][ext=mp4]+bestaudio[acodec^=mp4a][ext=m4a]/"));
        let p720_fb = build_ytdlp_video_format_spec("720", true);
        assert!(p720_fb.starts_with("bestvideo[height<=720][vcodec^=avc1][ext=mp4]+bestaudio[acodec^=mp4a][ext=m4a]/"));
    }

    #[tokio::test]
    #[ignore]
    async fn live_smoke_analyze_speed_youtube_x_quote_and_instagram() {
        let dir = std::env::temp_dir().join(format!("mv_smoke_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Arc::new(Database::open(&dir.join("app.db"), dir.join("dl")).unwrap());
        let previews = Arc::new(PreviewCache::new());
        let engine = AppEngine::new(db, previews);

        let t0 = Instant::now();
        let yt = engine
            .analyze("https://youtu.be/Z8-mdLbCa4k")
            .await
            .expect("YouTube analyze failed");
        println!(
            "FAST_YT ({:.2}s): title={:?} qualities={:?} audio={:?}",
            t0.elapsed().as_secs_f64(),
            yt.title,
            yt.available_qualities,
            yt.available_audio_formats
        );
        assert!(yt.supports_mp4);
        assert!(yt.available_qualities.contains(&"1080".to_string()));
        assert!(yt.available_audio_formats.contains(&"mp3".to_string()));

        let t1 = Instant::now();
        let xq = engine
            .analyze("https://x.com/orpheuskaze/status/2125937180795338764?a=0v1dbvl08UWsxZ2mkdA&s=20")
            .await
            .expect("X quote-tweet analyze failed");
        println!(
            "FAST_X_QUOTE ({:.2}s): title={:?} qualities={:?} images={} audio={:?}",
            t1.elapsed().as_secs_f64(),
            xq.title,
            xq.available_qualities,
            xq.images.len(),
            xq.available_audio_formats
        );
        assert!(xq.supports_mp4);
        assert!(!xq.available_qualities.is_empty());

        let t2 = Instant::now();
        let ig = engine
            .analyze("https://www.instagram.com/p/DgLJAyrIBzH/")
            .await
            .expect("Instagram photo analyze failed");
        println!(
            "FAST_IG_PHOTO ({:.2}s): title={:?} images={} formats={:?}",
            t2.elapsed().as_secs_f64(),
            ig.title,
            ig.images.len(),
            ig.available_image_formats
        );
        assert_eq!(ig.media_type.as_deref(), Some("image"));
        assert!(!ig.images.is_empty());

        let t3 = Instant::now();
        let reel = engine
            .analyze("https://www.instagram.com/reel/DgKrEuFSXL7/")
            .await
            .expect("Instagram Reel analyze failed");
        println!(
            "FAST_IG_REEL ({:.2}s): title={:?} qualities={:?} audio={:?}",
            t3.elapsed().as_secs_f64(),
            reel.title,
            reel.available_qualities,
            reel.available_audio_formats
        );
        assert!(reel.supports_mp4);
        assert!(reel.available_qualities.contains(&"1080".to_string()));

        let t4 = Instant::now();
        let th = engine
            .analyze("https://www.threads.com/share/MqtvI-pQu")
            .await
            .expect("Threads share link analyze failed");
        println!(
            "FAST_THREADS_SHARE ({:.2}s): title={:?} images={} formats={:?}",
            t4.elapsed().as_secs_f64(),
            th.title,
            th.images.len(),
            th.available_image_formats
        );
        assert_eq!(th.platform, "threads");
        assert_eq!(th.media_type.as_deref(), Some("image"));
        assert_eq!(th.images.len(), 4);
        assert!(th.available_image_formats.contains(&"zip".to_string()));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    #[ignore]
    async fn live_smoke_untested_platforms() {
        let dir = std::env::temp_dir().join(format!("mv_smoke_all_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Arc::new(Database::open(&dir.join("app.db"), dir.join("dl")).unwrap());
        let previews = Arc::new(PreviewCache::new());
        let engine = AppEngine::new(db, previews);

        let cases = [
            ("THREADS_VIDEO", "https://www.threads.com/@nasa/post/Df-IxZ8AgOO"),
            ("PINTEREST_PHOTO", "https://www.pinterest.com/pin/99360735500167749/"),
            ("FACEBOOK_VIDEO", "https://www.facebook.com/watch/?v=2470894823717065"),
            ("FACEBOOK_PHOTO", "https://www.facebook.com/photo/?fbid=1697460148416021"),
            ("FACEBOOK_POST", "https://www.facebook.com/NASA/posts/pfbid02ssCrhKWFtcvNpi8X7G3tHqPXiCFtStChH3fZv2aYR3rnR7Hh4Zt5UCj2gWevjav7l"),
        ];

        for (label, url) in cases {
            let t = Instant::now();
            match engine.analyze(url).await {
                Ok(m) => {
                    println!(
                        "{label} OK ({:.2}s): type={:?} title={:?} qualities={:?} audio={:?} images={} img_fmts={:?}",
                        t.elapsed().as_secs_f64(),
                        m.media_type,
                        m.title,
                        m.available_qualities,
                        m.available_audio_formats,
                        m.images.len(),
                        m.available_image_formats
                    );
                }
                Err(e) => {
                    println!(
                        "{label} ERR ({:.2}s): code={} msg={}",
                        t.elapsed().as_secs_f64(),
                        e.code,
                        e.message
                    );
                }
            }
        }

        let _ = std::fs::remove_dir_all(dir);
    }
}
