//! 100% Pure-Rust Download, Media Analysis, and Processing Engine.
//! Zero yt-dlp, zero FFmpeg, zero ffprobe, zero gallery-dl, zero Node.js!

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::sync::{watch, Semaphore};

use crate::db::{
    ensure_safe_canonical_dir, strip_win_verbatim, validate_download_dir, Database, JobRecord,
};
use crate::errors::{classify_extractor_error, AppError};
use crate::extractors::{
    self, DirectVideoVariant, ExtractedPhotoSet, ImageItem, PreviewCache,
};
use crate::mp4_engine;
use crate::url_policy::{
    ensure_public_host, resolve_alias, validate_url, validate_video_cdn_url, SHARED_HTTP,
};

const QUALITIES: &[&str] = &["1080", "720", "480", "360"];
const QUEUE_LIMIT: i64 = 20;
const MAX_DIRECT_VIDEO_BYTES: i64 = 2 * 1024 * 1024 * 1024;
const DOWNLOAD_TIMEOUT_SECS: u64 = 3600;

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
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
struct CachedAnalysis {
    meta: MediaMetadata,
    resolved_url: String,
    direct_videos: Vec<DirectVideoVariant>,
    direct_audio_url: Option<String>,
    stream_ua: Option<String>,
    stream_cookie: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CreateDownloadRequest {
    pub url: String,
    pub kind: String,
    pub quality: String,
    pub container: String,
    pub allow_quality_fallback: bool,
    pub image_index: Option<i64>,
    pub output_dir: Option<String>,
}

fn build_quality_list(variants: &[DirectVideoVariant]) -> Vec<String> {
    if variants.is_empty() {
        return Vec::new();
    }
    let mut out = vec!["best".to_string()];
    let max_h = variants.iter().map(|v| v.height).max().unwrap_or(0);
    for &q in QUALITIES {
        if let Ok(qh) = q.parse::<u32>() {
            if variants.iter().any(|v| v.height == qh) || (max_h >= qh && qh <= 1080) {
                out.push(q.to_string());
            }
        }
    }
    out
}

fn build_image_formats(count: usize) -> Vec<String> {
    if count == 0 {
        return Vec::new();
    }
    let mut out = vec!["jpg".into(), "png".into(), "webp".into()];
    if count > 1 {
        out.push("zip".into());
    }
    out
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
    let clean_ext = ext.trim().trim_start_matches('.').to_ascii_lowercase();
    let safe_ext = match clean_ext.as_str() {
        "mp4" | "m4a" | "wav" | "jpg" | "jpeg" | "png" | "webp" | "zip" => clean_ext.as_str(),
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
    cancels: Arc<Mutex<HashMap<String, watch::Sender<bool>>>>,
    analyze_cache: Arc<Mutex<HashMap<String, (Instant, CachedAnalysis)>>>,
    rt_handle: tokio::runtime::Handle,
}

impl AppEngine {
    pub fn new(
        db: Arc<Database>,
        previews: Arc<PreviewCache>,
        rt_handle: tokio::runtime::Handle,
    ) -> Self {
        Self {
            db,
            previews,
            semaphore: Arc::new(Semaphore::new(2)),
            cancels: Arc::new(Mutex::new(HashMap::new())),
            analyze_cache: Arc::new(Mutex::new(HashMap::new())),
            rt_handle,
        }
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

    async fn extract_platform_analysis(
        &self,
        original_url: &str,
        resolved_url: &str,
        platform: &str,
    ) -> Result<CachedAnalysis, AppError> {
        match platform {
            "youtube" => {
                let yt = extractors::fetch_youtube_innertube(resolved_url).await?;
                let has_video = !yt.video_variants.is_empty();
                let meta = MediaMetadata {
                    url: original_url.to_string(),
                    platform: "youtube".into(),
                    title: yt.title,
                    uploader: yt.uploader,
                    duration_seconds: yt.duration_seconds,
                    thumbnail_url: yt.thumbnail_url,
                    preview_token: None,
                    media_id: Some(yt.video_id),
                    media_type: Some("video".into()),
                    format_count: yt.video_variants.len(),
                    available_qualities: build_quality_list(&yt.video_variants),
                    available_audio_formats: if yt.has_audio {
                        vec!["m4a".into(), "wav".into()]
                    } else {
                        Vec::new()
                    },
                    available_image_formats: Vec::new(),
                    images: Vec::new(),
                    has_audio: yt.has_audio,
                    supports_mp4: has_video,
                    warnings: Vec::new(),
                };
                Ok(CachedAnalysis {
                    meta,
                    resolved_url: resolved_url.to_string(),
                    direct_videos: yt.video_variants,
                    direct_audio_url: yt.audio_url,
                    stream_ua: Some(yt.client_ua),
                    stream_cookie: None,
                })
            }
            "x" => {
                let syn = extractors::fetch_twitter_syndication(resolved_url)
                    .await
                    .ok_or_else(|| AppError::new("UNAVAILABLE_MEDIA"))?;
                let has_video = !syn.video_variants.is_empty();
                let has_images = !syn.images.is_empty();
                let meta = MediaMetadata {
                    url: original_url.to_string(),
                    platform: "x".into(),
                    title: syn.title,
                    uploader: syn.uploader,
                    duration_seconds: syn.duration_seconds,
                    thumbnail_url: syn.thumbnail_url,
                    preview_token: None,
                    media_id: Some(syn.media_id),
                    media_type: Some(if has_video { "video" } else { "image" }.into()),
                    format_count: syn.video_variants.len() + syn.images.len(),
                    available_qualities: build_quality_list(&syn.video_variants),
                    available_audio_formats: if has_video {
                        vec!["m4a".into(), "wav".into()]
                    } else {
                        Vec::new()
                    },
                    available_image_formats: build_image_formats(syn.images.len()),
                    images: syn.images,
                    has_audio: has_video,
                    supports_mp4: has_video,
                    warnings: if !has_video && !has_images {
                        vec!["No downloadable media found.".into()]
                    } else {
                        Vec::new()
                    },
                };
                Ok(CachedAnalysis {
                    meta,
                    resolved_url: resolved_url.to_string(),
                    direct_videos: syn.video_variants,
                    direct_audio_url: None,
                    stream_ua: None,
                    stream_cookie: None,
                })
            }
            "instagram" => {
                let ig = extractors::fetch_instagram_polaris(resolved_url)
                    .await
                    .ok_or_else(|| AppError::new("UNAVAILABLE_MEDIA"))?;
                let has_video = !ig.video_variants.is_empty();
                let meta = MediaMetadata {
                    url: original_url.to_string(),
                    platform: "instagram".into(),
                    title: ig.title,
                    uploader: ig.uploader,
                    duration_seconds: ig.duration_seconds,
                    thumbnail_url: ig.thumbnail_url,
                    preview_token: None,
                    media_id: Some(ig.media_id),
                    media_type: Some(if has_video { "video" } else { "image" }.into()),
                    format_count: ig.video_variants.len() + ig.images.len(),
                    available_qualities: build_quality_list(&ig.video_variants),
                    available_audio_formats: if has_video {
                        vec!["m4a".into(), "wav".into()]
                    } else {
                        Vec::new()
                    },
                    available_image_formats: build_image_formats(ig.images.len()),
                    images: ig.images,
                    has_audio: has_video,
                    supports_mp4: has_video,
                    warnings: Vec::new(),
                };
                Ok(CachedAnalysis {
                    meta,
                    resolved_url: resolved_url.to_string(),
                    direct_videos: ig.video_variants,
                    direct_audio_url: None,
                    stream_ua: None,
                    stream_cookie: None,
                })
            }
            "threads" => {
                let th = extractors::fetch_threads_media(resolved_url)
                    .await
                    .ok_or_else(|| AppError::new("UNAVAILABLE_MEDIA"))?;
                let has_video = !th.video_variants.is_empty();
                let meta = MediaMetadata {
                    url: original_url.to_string(),
                    platform: "threads".into(),
                    title: th.title,
                    uploader: th.uploader,
                    duration_seconds: th.duration_seconds,
                    thumbnail_url: th.thumbnail_url,
                    preview_token: None,
                    media_id: Some(th.media_id),
                    media_type: Some(if has_video { "video" } else { "image" }.into()),
                    format_count: th.video_variants.len() + th.images.len(),
                    available_qualities: build_quality_list(&th.video_variants),
                    available_audio_formats: if has_video && th.has_audio {
                        vec!["m4a".into(), "wav".into()]
                    } else {
                        Vec::new()
                    },
                    available_image_formats: build_image_formats(th.images.len()),
                    images: th.images,
                    has_audio: has_video && th.has_audio,
                    supports_mp4: has_video,
                    warnings: Vec::new(),
                };
                Ok(CachedAnalysis {
                    meta,
                    resolved_url: resolved_url.to_string(),
                    direct_videos: th.video_variants,
                    direct_audio_url: None,
                    stream_ua: None,
                    stream_cookie: None,
                })
            }
            "facebook" => {
                let fb = extractors::fetch_facebook_media(resolved_url)
                    .await
                    .ok_or_else(|| AppError::new("UNAVAILABLE_MEDIA"))?;
                let has_video = !fb.video_variants.is_empty();
                let meta = MediaMetadata {
                    url: original_url.to_string(),
                    platform: "facebook".into(),
                    title: fb.title,
                    uploader: fb.uploader,
                    duration_seconds: fb.duration_seconds,
                    thumbnail_url: fb.thumbnail_url,
                    preview_token: None,
                    media_id: fb.media_id,
                    media_type: Some(if has_video { "video" } else { "image" }.into()),
                    format_count: fb.video_variants.len() + fb.images.len(),
                    available_qualities: build_quality_list(&fb.video_variants),
                    available_audio_formats: if has_video {
                        vec!["m4a".into(), "wav".into()]
                    } else {
                        Vec::new()
                    },
                    available_image_formats: build_image_formats(fb.images.len()),
                    images: fb.images,
                    has_audio: has_video,
                    supports_mp4: has_video,
                    warnings: Vec::new(),
                };
                Ok(CachedAnalysis {
                    meta,
                    resolved_url: resolved_url.to_string(),
                    direct_videos: fb.video_variants,
                    direct_audio_url: None,
                    stream_ua: None,
                    stream_cookie: None,
                })
            }
            "pinterest" => {
                let pin = extractors::fetch_pinterest_media(resolved_url)
                    .await
                    .ok_or_else(|| AppError::new("UNAVAILABLE_MEDIA"))?;
                let has_video = !pin.video_variants.is_empty();
                let meta = MediaMetadata {
                    url: original_url.to_string(),
                    platform: "pinterest".into(),
                    title: pin.title,
                    uploader: None,
                    duration_seconds: None,
                    thumbnail_url: pin.thumbnail_url,
                    preview_token: None,
                    media_id: Some(pin.media_id),
                    media_type: Some(if has_video { "video" } else { "image" }.into()),
                    format_count: pin.video_variants.len() + pin.images.len(),
                    available_qualities: build_quality_list(&pin.video_variants),
                    available_audio_formats: if has_video {
                        vec!["m4a".into(), "wav".into()]
                    } else {
                        Vec::new()
                    },
                    available_image_formats: build_image_formats(pin.images.len()),
                    images: pin.images,
                    has_audio: has_video,
                    supports_mp4: has_video,
                    warnings: Vec::new(),
                };
                Ok(CachedAnalysis {
                    meta,
                    resolved_url: resolved_url.to_string(),
                    direct_videos: pin.video_variants,
                    direct_audio_url: None,
                    stream_ua: None,
                    stream_cookie: None,
                })
            }
            "tiktok" => {
                let tt = extractors::fetch_tiktok_media(resolved_url).await?;
                let has_video = !tt.video_variants.is_empty();
                let meta = MediaMetadata {
                    url: original_url.to_string(),
                    platform: "tiktok".into(),
                    title: tt.title,
                    uploader: tt.uploader,
                    duration_seconds: tt.duration_seconds,
                    thumbnail_url: tt.thumbnail_url,
                    preview_token: None,
                    media_id: Some(tt.media_id),
                    media_type: Some(if has_video { "video" } else { "image" }.into()),
                    format_count: tt.video_variants.len() + tt.images.len(),
                    available_qualities: build_quality_list(&tt.video_variants),
                    available_audio_formats: if has_video {
                        vec!["m4a".into(), "wav".into()]
                    } else {
                        Vec::new()
                    },
                    available_image_formats: build_image_formats(tt.images.len()),
                    images: tt.images,
                    has_audio: has_video,
                    supports_mp4: has_video,
                    warnings: Vec::new(),
                };
                Ok(CachedAnalysis {
                    meta,
                    resolved_url: resolved_url.to_string(),
                    direct_videos: tt.video_variants,
                    direct_audio_url: None,
                    stream_ua: None,
                    stream_cookie: tt.cookies,
                })
            }
            _ => Err(AppError::new("UNSUPPORTED_PLATFORM")),
        }
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
        let resolved_url = resolve_alias(&url, &platform).await?;
        let entry = self
            .extract_platform_analysis(&url, &resolved_url, &platform)
            .await?;
        self.store_cache(&url, &entry);
        Ok(self.annotate_previews(entry.meta))
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
                if !matches!(req.container.as_str(), "m4a" | "wav") {
                    return Err(AppError::new("INVALID_REQUEST"));
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
            kind: req.kind,
            quality: req.quality,
            container: req.container,
            audio_bitrate_kbps: None,
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
        self.rt_handle.spawn(async move {
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

        let res = tokio::time::timeout(
            Duration::from_secs(DOWNLOAD_TIMEOUT_SECS),
            self.execute_job(&job, cached, custom_output_dir, &mut cancel_rx),
        )
        .await;
        self.cancels.lock().unwrap().remove(&job.id);

        if *cancel_rx.borrow() {
            return;
        }
        match res {
            Ok(Ok(())) => {}
            Ok(Err(err)) => self.db.mark_failed(&job.id, err.code, err.message),
            Err(_) => {
                let err = AppError::new("TIMEOUT");
                self.db.mark_failed(&job.id, err.code, err.message);
            }
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
        ua: Option<&str>,
        cookie: Option<&str>,
        progress_range: (f64, f64),
        cancel_rx: &mut watch::Receiver<bool>,
    ) -> Result<i64, AppError> {
        let mut current_url =
            validate_video_cdn_url(url_str, platform).ok_or_else(|| AppError::new("FORBIDDEN"))?;
        ensure_public_host(&current_url).await?;

        // YouTube googlevideo.com throttles single un-ranged GETs to ~1x playback speed;
        // requesting 4 MiB HTTP byte ranges unlocks full line speed without any external tool.
        if platform == "youtube" {
            return self
                .stream_youtube_ranged(job_id, &current_url, dest, ua, progress_range, cancel_rx)
                .await;
        }

        let referer = match platform {
            "facebook" => Some("https://www.facebook.com/"),
            "instagram" => Some("https://www.instagram.com/"),
            "threads" => Some("https://www.threads.com/"),
            "tiktok" => Some("https://www.tiktok.com/"),
            "x" => Some("https://x.com/"),
            "pinterest" => Some("https://www.pinterest.com/"),
            _ => None,
        };

        // Follow up to 5 validated same-platform CDN redirects (e.g. Facebook video.*.fna.fbcdn.net -> video.xx.fbcdn.net)
        let mut active_cookie = cookie;
        let mut resp = None;
        for _hop in 0..=5 {
            let mut req = SHARED_HTTP
                .get(&current_url)
                .header("Accept", "*/*")
                .timeout(Duration::from_secs(600));
            if let Some(r) = referer {
                req = req.header("Referer", r);
            }
            if let Some(u) = ua {
                req = req.header("User-Agent", u);
            }
            if let Some(c) = active_cookie {
                req = req.header("Cookie", c);
            }
            let r = req.send().await.map_err(|_| AppError::new("NETWORK_ERROR"))?;
            let status = r.status().as_u16();
            if matches!(status, 301 | 302 | 303 | 307 | 308) {
                let loc = r
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| AppError::new("NETWORK_ERROR"))?;
                let base = url::Url::parse(&current_url).map_err(|_| AppError::new("INVALID_URL"))?;
                let joined = base.join(loc).map_err(|_| AppError::new("INVALID_URL"))?;
                let next_safe = validate_video_cdn_url(joined.as_str(), platform)
                    .ok_or_else(|| AppError::new("FORBIDDEN"))?;
                ensure_public_host(&next_safe).await?;
                if joined.host_str() != base.host_str() {
                    active_cookie = None;
                }
                current_url = next_safe;
                continue;
            }
            if !r.status().is_success() {
                return Err(classify_extractor_error(&format!("HTTP error {status}")));
            }
            resp = Some(r);
            break;
        }
        let mut resp = resp.ok_or_else(|| AppError::new("NETWORK_ERROR"))?;
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
        let (p_start, p_end) = progress_range;

        loop {
            let chunk = tokio::select! {
                c = resp.chunk() => c.map_err(|_| {
                    let _ = std::fs::remove_file(dest);
                    AppError::new("NETWORK_ERROR")
                })?,
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
            if last_emit.elapsed() >= Duration::from_millis(200) {
                last_emit = Instant::now();
                let elapsed = started.elapsed().as_secs_f64().max(0.1);
                let speed = downloaded as f64 / elapsed;
                let pct = total.filter(|&t| t > 0).map(|t| {
                    let frac = (downloaded as f64 / t as f64).clamp(0.0, 1.0);
                    p_start + frac * (p_end - p_start)
                });
                let eta = total
                    .filter(|&t| t > downloaded && speed > 1.0)
                    .map(|t| ((t - downloaded) as f64 / speed) as i64);
                self.db
                    .update_progress(job_id, "downloading", pct, Some(downloaded), total, Some(speed), eta);
            }
        }
        file.flush().map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;
        Ok(downloaded)
    }

    async fn stream_youtube_ranged(
        &self,
        job_id: &str,
        safe_url: &str,
        dest: &Path,
        ua: Option<&str>,
        progress_range: (f64, f64),
        cancel_rx: &mut watch::Receiver<bool>,
    ) -> Result<i64, AppError> {
        const CHUNK_SIZE: i64 = 4 * 1024 * 1024;
        let mut file =
            std::fs::File::create(dest).map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;
        let mut offset: i64 = 0;
        let mut total: Option<i64> = None;
        let started = Instant::now();
        let mut last_emit = Instant::now();
        let (p_start, p_end) = progress_range;

        let mut active_url = safe_url.to_string();
        loop {
            if *cancel_rx.borrow() {
                drop(file);
                let _ = std::fs::remove_file(dest);
                return Ok(0);
            }
            let end = match total {
                Some(t) => (offset + CHUNK_SIZE - 1).min(t - 1),
                None => offset + CHUNK_SIZE - 1,
            };
            let range_hdr = format!("bytes={offset}-{end}");
            let mut resp = None;
            for _hop in 0..=4 {
                let mut req = SHARED_HTTP
                    .get(&active_url)
                    .header("Range", &range_hdr)
                    .timeout(Duration::from_secs(120));
                if let Some(u) = ua {
                    req = req.header("User-Agent", u);
                }
                let r = req.send().await.map_err(|_| {
                    let _ = std::fs::remove_file(dest);
                    AppError::new("NETWORK_ERROR")
                })?;
                let st = r.status().as_u16();
                if matches!(st, 301 | 302 | 303 | 307 | 308) {
                    let Some(loc) = r.headers().get("location").and_then(|v| v.to_str().ok()) else {
                        drop(file);
                        let _ = std::fs::remove_file(dest);
                        return Err(AppError::new("NETWORK_ERROR"));
                    };
                    let base = url::Url::parse(&active_url).map_err(|_| AppError::new("INVALID_URL"))?;
                    let joined = base.join(loc).map_err(|_| AppError::new("INVALID_URL"))?;
                    let next_safe = validate_video_cdn_url(joined.as_str(), "youtube")
                        .ok_or_else(|| AppError::new("FORBIDDEN"))?;
                    ensure_public_host(&next_safe).await?;
                    active_url = next_safe;
                    continue;
                }
                resp = Some(r);
                break;
            }
            let mut resp = match resp {
                Some(r) => r,
                None => {
                    drop(file);
                    let _ = std::fs::remove_file(dest);
                    return Err(AppError::new("NETWORK_ERROR"));
                }
            };
            let status = resp.status().as_u16();
            if status == 416 && offset > 0 {
                break;
            }
            if !resp.status().is_success() {
                drop(file);
                let _ = std::fs::remove_file(dest);
                return Err(classify_extractor_error(&format!("HTTP error {status}")));
            }
            if total.is_none() {
                if let Some(cr) = resp
                    .headers()
                    .get("content-range")
                    .and_then(|v| v.to_str().ok())
                {
                    if let Some(t) = cr.rsplit('/').next().and_then(|s| s.parse::<i64>().ok()) {
                        if t > MAX_DIRECT_VIDEO_BYTES {
                            drop(file);
                            let _ = std::fs::remove_file(dest);
                            return Err(AppError::new("FILE_TOO_LARGE"));
                        }
                        total = Some(t);
                    }
                } else if status == 200 {
                    if let Some(cl) = resp.content_length().map(|n| n as i64) {
                        if cl > MAX_DIRECT_VIDEO_BYTES {
                            drop(file);
                            let _ = std::fs::remove_file(dest);
                            return Err(AppError::new("FILE_TOO_LARGE"));
                        }
                        total = Some(cl);
                    }
                }
            }

            let mut received_this_req: i64 = 0;
            loop {
                let chunk = tokio::select! {
                    c = resp.chunk() => c.map_err(|_| {
                        let _ = std::fs::remove_file(dest);
                        AppError::new("NETWORK_ERROR")
                    })?,
                    _ = cancel_rx.changed() => {
                        drop(file);
                        let _ = std::fs::remove_file(dest);
                        return Ok(0);
                    }
                };
                let Some(bytes) = chunk else { break };
                offset = offset.saturating_add(bytes.len() as i64);
                received_this_req = received_this_req.saturating_add(bytes.len() as i64);
                if offset > MAX_DIRECT_VIDEO_BYTES {
                    drop(file);
                    let _ = std::fs::remove_file(dest);
                    return Err(AppError::new("FILE_TOO_LARGE"));
                }
                if file.write_all(&bytes).is_err() {
                    drop(file);
                    let _ = std::fs::remove_file(dest);
                    return Err(AppError::new("STORAGE_UNAVAILABLE"));
                }
                if last_emit.elapsed() >= Duration::from_millis(200) {
                    last_emit = Instant::now();
                    let elapsed = started.elapsed().as_secs_f64().max(0.1);
                    let speed = offset as f64 / elapsed;
                    let pct = total.filter(|&t| t > 0).map(|t| {
                        let frac = (offset as f64 / t as f64).clamp(0.0, 1.0);
                        p_start + frac * (p_end - p_start)
                    });
                    let eta = total
                        .filter(|&t| t > offset && speed > 1.0)
                        .map(|t| ((t - offset) as f64 / speed) as i64);
                    self.db.update_progress(
                        job_id,
                        "downloading",
                        pct,
                        Some(offset),
                        total,
                        Some(speed),
                        eta,
                    );
                }
            }

            if status == 200 || received_this_req == 0 {
                break;
            }
            if let Some(t) = total {
                if offset >= t {
                    break;
                }
            } else if received_this_req < CHUNK_SIZE {
                break;
            }
        }
        file.flush().map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;
        Ok(offset)
    }

    async fn execute_in_stage(
        &self,
        job: &JobRecord,
        cached: Option<CachedAnalysis>,
        download_dir: &Path,
        stage_dir: &Path,
        cancel_rx: &mut watch::Receiver<bool>,
    ) -> Result<(), AppError> {
        self.db
            .update_progress(&job.id, "downloading", Some(1.0), None, None, None, None);

        let analysis = match cached {
            Some(c) => c,
            None => {
                let resolved = resolve_alias(&job.url, &job.platform).await?;
                self.extract_platform_analysis(&job.url, &resolved, &job.platform)
                    .await?
            }
        };

        if job.kind == "image" {
            let photo_set = if !analysis.meta.images.is_empty() {
                ExtractedPhotoSet {
                    title: analysis.meta.title.clone(),
                    uploader: analysis.meta.uploader.clone(),
                    media_id: analysis.meta.media_id.clone(),
                    images: analysis.meta.images.clone(),
                }
            } else {
                extractors::extract_images(&analysis.resolved_url, &job.platform)
                    .await
                    .ok_or_else(|| AppError::new("FORMAT_UNAVAILABLE"))?
            };
            let stage_file = stage_dir.join(format!("media.{}", job.container));
            let db = Arc::clone(&self.db);
            let jid = job.id.clone();
            let size = extractors::download_photo_set(
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
                &strip_win_verbatim(&dest),
                size,
            );
            return Ok(());
        }

        let ua = analysis.stream_ua.as_deref();
        let cookie = analysis.stream_cookie.as_deref();

        if job.kind == "audio" {
            let src_file = stage_dir.join("raw_audio_src.mp4");
            let audio_source_url = analysis
                .direct_audio_url
                .as_deref()
                .or_else(|| analysis.direct_videos.last().map(|v| v.url.as_str()))
                .ok_or_else(|| AppError::new("FORMAT_UNAVAILABLE"))?;
            self.stream_direct_file(
                &job.id,
                &job.platform,
                audio_source_url,
                &src_file,
                ua,
                cookie,
                (2.0, 90.0),
                cancel_rx,
            )
            .await?;
            if *cancel_rx.borrow() {
                return Ok(());
            }
            self.db
                .update_progress(&job.id, "processing", Some(94.0), None, None, None, None);
            let out_audio = stage_dir.join(format!("audio.{}", job.container));
            let src_clone = src_file.clone();
            let out_clone = out_audio.clone();
            let container = job.container.clone();
            tokio::task::spawn_blocking(move || {
                if container == "wav" {
                    mp4_engine::transcode_audio_to_wav(&src_clone, &out_clone)
                } else {
                    mp4_engine::extract_audio_m4a(&src_clone, &out_clone)
                }
            })
            .await
            .map_err(|_| AppError::new("INTERNAL_ERROR"))??;

            let size = std::fs::metadata(&out_audio).map(|m| m.len() as i64).unwrap_or(0);
            let title = analysis.meta.title.or_else(|| job.title.clone());
            let thumb = analysis.meta.thumbnail_url.or_else(|| job.thumbnail_url.clone());
            let desired = sanitize_filename(title.as_deref(), &job.platform, &job.container);
            let dest = unique_dest_path(download_dir, &desired);
            std::fs::rename(&out_audio, &dest)
                .or_else(|_| std::fs::copy(&out_audio, &dest).map(|_| ()))
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
                &strip_win_verbatim(&dest),
                size,
            );
            return Ok(());
        }

        // Video (MP4)
        let chosen = pick_direct_variant(
            &analysis.direct_videos,
            &job.quality,
            job.allow_quality_fallback,
        )
        .ok_or_else(|| AppError::new("FORMAT_UNAVAILABLE"))?;

        let final_stage = if let Some(aud_url) = analysis.direct_audio_url.as_deref() {
            // Adaptive DASH stream (e.g. YouTube 1080p/720p H.264 + AAC): download video & audio then mux in pure Rust!
            let raw_v = stage_dir.join("video_only.mp4");
            let raw_a = stage_dir.join("audio_only.m4a");
            self.stream_direct_file(
                &job.id,
                &job.platform,
                &chosen.url,
                &raw_v,
                ua,
                cookie,
                (2.0, 80.0),
                cancel_rx,
            )
            .await?;
            if *cancel_rx.borrow() {
                return Ok(());
            }
            self.stream_direct_file(
                &job.id,
                &job.platform,
                aud_url,
                &raw_a,
                ua,
                cookie,
                (80.0, 93.0),
                cancel_rx,
            )
            .await?;
            if *cancel_rx.borrow() {
                return Ok(());
            }
            self.db
                .update_progress(&job.id, "processing", Some(96.0), None, None, None, None);
            let muxed = stage_dir.join("muxed.mp4");
            let rv = raw_v.clone();
            let ra = raw_a.clone();
            let mx = muxed.clone();
            tokio::task::spawn_blocking(move || mp4_engine::mux_video_and_audio_mp4(&rv, &ra, &mx))
                .await
                .map_err(|_| AppError::new("INTERNAL_ERROR"))??;
            muxed
        } else {
            let raw_mp4 = stage_dir.join("media.mp4");
            self.stream_direct_file(
                &job.id,
                &job.platform,
                &chosen.url,
                &raw_mp4,
                ua,
                cookie,
                (2.0, 96.0),
                cancel_rx,
            )
            .await?;
            if *cancel_rx.borrow() {
                return Ok(());
            }
            raw_mp4
        };

        let size = std::fs::metadata(&final_stage)
            .map(|m| m.len() as i64)
            .unwrap_or(0);
        let title = analysis.meta.title.or_else(|| job.title.clone());
        let thumb = analysis.meta.thumbnail_url.or_else(|| job.thumbnail_url.clone());
        let desired = sanitize_filename(title.as_deref(), &job.platform, "mp4");
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
            title.as_deref(),
            thumb.as_deref(),
            &fname,
            &strip_win_verbatim(&dest),
            size,
        );
        Ok(())
    }
}

// ============================================================================
// Safe Local Filesystem & Windows Explorer Helpers
// ============================================================================

#[cfg(windows)]
fn windows_explorer_exe() -> PathBuf {
    std::env::var_os("SystemRoot")
        .or_else(|| std::env::var_os("WINDIR"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("explorer.exe")
}

#[cfg(windows)]
pub fn normalize_windows_explorer_path(p: &Path) -> Result<String, AppError> {
    let meta = std::fs::symlink_metadata(p).map_err(|_| AppError::new("FILE_NOT_FOUND"))?;
    if meta.file_type().is_symlink() {
        return Err(AppError::new("FORBIDDEN"));
    }
    let resolved = std::fs::canonicalize(p).map_err(|_| AppError::new("FILE_NOT_FOUND"))?;
    let raw = resolved.display().to_string();
    if raw.starts_with(r"\\?\UNC\") || raw.starts_with(r"\\?\unc\") {
        return Err(AppError::new("FORBIDDEN"));
    }
    let stripped = raw
        .strip_prefix(r"\\?\")
        .unwrap_or(&raw)
        .replace('/', r"\");
    let trimmed = if stripped.len() > 3 {
        stripped.trim_end_matches('\\').to_string()
    } else {
        stripped
    };
    if trimmed.is_empty()
        || trimmed.starts_with(r"\\")
        || trimmed.ends_with('\\')
        || trimmed.contains('"')
        || trimmed.chars().any(char::is_control)
    {
        return Err(AppError::new("INVALID_REQUEST"));
    }
    Ok(trimmed)
}

pub fn open_file_in_explorer(path_str: &str) -> Result<(), AppError> {
    let p = Path::new(path_str);
    let meta = std::fs::symlink_metadata(p).map_err(|_| AppError::new("FILE_NOT_FOUND"))?;
    if !meta.file_type().is_file() {
        return Err(AppError::new("FILE_NOT_FOUND"));
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let clean = normalize_windows_explorer_path(p)?;
        std::process::Command::new(windows_explorer_exe())
            .raw_arg(format!("/select,\"{clean}\""))
            .spawn()
            .map_err(|_| AppError::new("INTERNAL_ERROR"))?;
    }
    Ok(())
}

pub fn open_folder_in_explorer(dir_str: &str) -> Result<(), AppError> {
    let dl = validate_download_dir(dir_str)?;
    std::fs::create_dir_all(&dl).map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;
    let canon = ensure_safe_canonical_dir(&dl)?;
    #[cfg(windows)]
    {
        let clean = normalize_windows_explorer_path(&canon)?;
        std::process::Command::new(windows_explorer_exe())
            .arg(&clean)
            .spawn()
            .map_err(|_| AppError::new("INTERNAL_ERROR"))?;
    }
    #[cfg(not(windows))]
    let _ = canon;
    Ok(())
}

pub fn open_media_in_default_player(path_str: &str) -> Result<(), AppError> {
    let p = Path::new(path_str);
    let meta = std::fs::symlink_metadata(p).map_err(|_| AppError::new("FILE_NOT_FOUND"))?;
    if !meta.file_type().is_file() {
        return Err(AppError::new("FILE_NOT_FOUND"));
    }
    let ext = p
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if !matches!(ext.as_str(), "mp4" | "m4a" | "wav" | "jpg" | "jpeg" | "png" | "webp") {
        return Err(AppError::new("FORBIDDEN"));
    }
    #[cfg(windows)]
    {
        let clean = normalize_windows_explorer_path(p)?;
        // Launching explorer.exe with a verified local media file path opens it in Windows default media player safely without cmd.exe.
        std::process::Command::new(windows_explorer_exe())
            .arg(&clean)
            .spawn()
            .map_err(|_| AppError::new("INTERNAL_ERROR"))?;
    }
    Ok(())
}

pub fn move_job_file(db: &Database, job_id: &str, target_dir: &Path) -> Result<String, AppError> {
    let job = db.get_job(job_id)?;
    if matches!(job.status.as_str(), "queued" | "downloading" | "processing") {
        return Err(AppError::new("JOB_STATE"));
    }
    let src_str = job.output_path.ok_or_else(|| AppError::new("FILE_NOT_FOUND"))?;
    let src_path = PathBuf::from(&src_str);
    let meta = std::fs::symlink_metadata(&src_path).map_err(|_| AppError::new("FILE_NOT_FOUND"))?;
    if !meta.file_type().is_file() {
        return Err(AppError::new("FILE_NOT_FOUND"));
    }
    let valid_dir = validate_download_dir(&target_dir.display().to_string())?;
    std::fs::create_dir_all(&valid_dir).map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;
    let canon_target = ensure_safe_canonical_dir(&valid_dir)?;

    if let Some(src_parent) = src_path.parent() {
        if let Ok(canon_parent) = std::fs::canonicalize(src_parent) {
            if canon_parent == canon_target {
                return Ok(src_str);
            }
        }
    }

    let desired_name = job
        .output_filename
        .as_deref()
        .or_else(|| src_path.file_name().and_then(|s| s.to_str()))
        .unwrap_or("media.bin");
    let dest_path = unique_dest_path(&canon_target, desired_name);

    let move_res = std::fs::rename(&src_path, &dest_path).or_else(|_| {
        std::fs::copy(&src_path, &dest_path)?;
        std::fs::remove_file(&src_path)
    });
    if move_res.is_err() {
        let _ = std::fs::remove_file(&dest_path);
        return Err(AppError::new("STORAGE_UNAVAILABLE"));
    }
    let final_name = dest_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(desired_name)
        .to_string();
    let final_path = strip_win_verbatim(&dest_path);
    db.update_output_location(&job.id, &final_name, &final_path)?;
    Ok(final_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_reserved_dos_filenames_and_extensions() {
        assert_eq!(sanitize_filename(Some("CON"), "youtube", "mp4"), "_CON.mp4");
        assert_eq!(sanitize_filename(Some("NUL.txt"), "x", "exe"), "_NUL.txt.bin");
        assert_eq!(
            sanitize_filename(Some("Demo: Video / Test?"), "youtube", "mp4"),
            "Demo Video Test.mp4"
        );
    }

    #[test]
    fn parses_youtube_innertube_avc1_and_mp4a_streams() {
        let payload = serde_json::json!({
            "playabilityStatus": { "status": "OK" },
            "videoDetails": {
                "videoId": "dQw4w9WgXcQ",
                "title": "Test Video 1080p",
                "author": "Test Channel",
                "lengthSeconds": "212",
                "thumbnail": { "thumbnails": [{ "url": "https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg" }] }
            },
            "streamingData": {
                "adaptiveFormats": [
                    { "mimeType": "video/mp4; codecs=\"avc1.640028\"", "width": 1920, "height": 1080, "bitrate": 2400000, "url": "https://rr1---sn-demo.googlevideo.com/videoplayback?itag=137" },
                    { "mimeType": "video/mp4; codecs=\"avc1.4d401f\"", "width": 1280, "height": 720, "bitrate": 1200000, "url": "https://rr1---sn-demo.googlevideo.com/videoplayback?itag=136" },
                    { "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"", "bitrate": 128000, "url": "https://rr1---sn-demo.googlevideo.com/videoplayback?itag=140" }
                ]
            }
        });
        let res = extractors::parse_youtube_innertube_response(&payload, "dQw4w9WgXcQ", "UA").unwrap();
        assert_eq!(res.title.as_deref(), Some("Test Video 1080p"));
        assert_eq!(res.video_variants.len(), 2);
        assert_eq!(res.video_variants[0].height, 1080);
        assert!(res.audio_url.is_some());
    }

    #[test]
    fn enqueue_works_from_sync_ui_thread_with_runtime_handle() {
        let dir = std::env::temp_dir().join(format!("mv_pure_eng_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let db = Arc::new(Database::open(&dir.join("app.db"), dir.join("dl")).unwrap());
        let previews = Arc::new(PreviewCache::new());
        let engine = Arc::new(AppEngine::new(Arc::clone(&db), previews, rt.handle().clone()));

        let job = engine
            .enqueue(
                CreateDownloadRequest {
                    url: "https://youtu.be/dQw4w9WgXcQ".into(),
                    kind: "video".into(),
                    quality: "best".into(),
                    container: "mp4".into(),
                    allow_quality_fallback: true,
                    image_index: None,
                    output_dir: None,
                },
                None,
            )
            .unwrap();
        assert_eq!(job.platform, "youtube");
        engine.cancel(&job.id).unwrap();
        drop(rt);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    #[ignore]
    async fn live_smoke_pure_rust_youtube_download_and_mux() {
        let dir = std::env::temp_dir().join(format!("mv_yt_live_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Arc::new(Database::open(&dir.join("app.db"), dir.join("dl")).unwrap());
        let previews = Arc::new(PreviewCache::new());
        let engine = Arc::new(AppEngine::new(
            Arc::clone(&db),
            previews,
            tokio::runtime::Handle::current(),
        ));

        // Short classic public YouTube video ("Me at the zoo", 19s) — fast live download + pure-Rust mux test!
        let t0 = Instant::now();
        let meta = engine
            .analyze("https://www.youtube.com/watch?v=jNQXAC9IVRw")
            .await
            .expect("live YouTube InnerTube analyze failed");
        println!(
            "PURE_RUST_YT_ANALYZE ({:.2}s): title={:?} qualities={:?} audio={:?}",
            t0.elapsed().as_secs_f64(),
            meta.title,
            meta.available_qualities,
            meta.available_audio_formats
        );
        assert!(!meta.available_qualities.is_empty());

        let t1 = Instant::now();
        let job = engine
            .enqueue(
                CreateDownloadRequest {
                    url: "https://www.youtube.com/watch?v=jNQXAC9IVRw".into(),
                    kind: "video".into(),
                    quality: "best".into(),
                    container: "mp4".into(),
                    allow_quality_fallback: true,
                    image_index: None,
                    output_dir: Some(dir.join("dl").display().to_string()),
                },
                None,
            )
            .unwrap();

        for _ in 0..120 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let cur = db.get_job(&job.id).unwrap();
            if cur.status == "completed" {
                let out_path = PathBuf::from(cur.output_path.as_deref().unwrap());
                let probe = mp4_engine::probe_mp4(&out_path).expect("probe_mp4 failed on live muxed video");
                println!(
                    "PURE_RUST_YT_DOWNLOAD+MUX OK ({:.2}s): {} bytes | {}x{} | v={:?} a={:?} dur={:.1}s",
                    t1.elapsed().as_secs_f64(),
                    probe.file_size_bytes,
                    probe.width.unwrap_or(0),
                    probe.height.unwrap_or(0),
                    probe.video_codec,
                    probe.audio_codec,
                    probe.duration_seconds.unwrap_or(0.0),
                );
                assert!(probe.video_codec.is_some());
                assert!(probe.audio_codec.is_some());
                assert!(probe.file_size_bytes > 50_000);
                // Also verify our native in-app VideoPlayer decodes real RGBA frames!
                let mut vp = crate::video_player::VideoPlayer::new();
                vp.open(&out_path, || {});
                let mut decoded_frame = None;
                for _ in 0..80 {
                    if let Some(f) = vp.take_new_frame(0) {
                        decoded_frame = Some(f);
                        break;
                    }
                    let snap = vp.snapshot();
                    if let Some(err) = snap.error {
                        panic!("VideoPlayer error on YouTube MP4: {err}");
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                let f = decoded_frame.expect("VideoPlayer did not decode a video frame");
                println!(
                    "VIDEO_PLAYER_YT OK: decoded {}x{} frame ({} bytes RGBA) dur={:.1}s has_audio={}",
                    f.width,
                    f.height,
                    f.rgba.len(),
                    vp.snapshot().duration_secs,
                    vp.snapshot().has_audio,
                );
                assert!(f.width > 0 && f.height > 0 && f.rgba.len() == (f.width * f.height * 4) as usize);
                vp.stop();
                let _ = std::fs::remove_dir_all(&dir);
                return;
            }
            if cur.status == "failed" {
                panic!("Live YouTube job failed: {:?} {:?}", cur.error_code, cur.error_message);
            }
        }
        panic!("Live YouTube job timed out");
    }

    #[tokio::test]
    #[ignore]
    async fn live_smoke_pure_rust_facebook_video_download() {
        let dir = std::env::temp_dir().join(format!("mv_fb_live_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Arc::new(Database::open(&dir.join("app.db"), dir.join("dl")).unwrap());
        let previews = Arc::new(PreviewCache::new());
        let engine = Arc::new(AppEngine::new(
            Arc::clone(&db),
            previews,
            tokio::runtime::Handle::current(),
        ));

        let url = "https://www.facebook.com/share/r/1CtbTq5jzq/";
        let t0 = Instant::now();
        let meta = engine
            .analyze(url)
            .await
            .expect("live Facebook analyze failed");
        println!(
            "PURE_RUST_FB_ANALYZE ({:.2}s): title={:?} qualities={:?}",
            t0.elapsed().as_secs_f64(),
            meta.title,
            meta.available_qualities
        );
        assert!(!meta.available_qualities.is_empty());

        let t1 = Instant::now();
        let job = engine
            .enqueue(
                CreateDownloadRequest {
                    url: url.into(),
                    kind: "video".into(),
                    quality: "best".into(),
                    container: "mp4".into(),
                    allow_quality_fallback: true,
                    image_index: None,
                    output_dir: Some(dir.join("dl").display().to_string()),
                },
                None,
            )
            .unwrap();

        for _ in 0..120 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let cur = db.get_job(&job.id).unwrap();
            if cur.status == "completed" {
                let out_path = PathBuf::from(cur.output_path.as_deref().unwrap());
                let probe = mp4_engine::probe_mp4(&out_path).expect("probe_mp4 failed on Facebook video");
                println!(
                    "PURE_RUST_FB_DOWNLOAD OK ({:.2}s): {} bytes | {}x{} | v={:?} a={:?} dur={:.1}s",
                    t1.elapsed().as_secs_f64(),
                    probe.file_size_bytes,
                    probe.width.unwrap_or(0),
                    probe.height.unwrap_or(0),
                    probe.video_codec,
                    probe.audio_codec,
                    probe.duration_seconds.unwrap_or(0.0),
                );
                assert!(probe.video_codec.is_some());
                assert!(probe.file_size_bytes > 50_000);
                let mut vp = crate::video_player::VideoPlayer::new();
                vp.open(&out_path, || {});
                let mut decoded_frame = None;
                for _ in 0..80 {
                    if let Some(f) = vp.take_new_frame(0) {
                        decoded_frame = Some(f);
                        break;
                    }
                    let snap = vp.snapshot();
                    if let Some(err) = snap.error {
                        panic!("VideoPlayer error on Facebook MP4: {err}");
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                let f = decoded_frame.expect("VideoPlayer did not decode a Facebook video frame");
                println!(
                    "VIDEO_PLAYER_FB OK: decoded {}x{} frame ({} bytes RGBA) dur={:.1}s has_audio={}",
                    f.width,
                    f.height,
                    f.rgba.len(),
                    vp.snapshot().duration_secs,
                    vp.snapshot().has_audio,
                );
                assert!(f.width > 0 && f.height > 0 && f.rgba.len() == (f.width * f.height * 4) as usize);
                vp.stop();
                let _ = std::fs::remove_dir_all(&dir);
                return;
            }
            if cur.status == "failed" {
                panic!("Live Facebook job failed: {:?} {:?}", cur.error_code, cur.error_message);
            }
        }
        panic!("Live Facebook job timed out");
    }
}
