pub mod db;
pub mod engine;
pub mod errors;
pub mod images;
pub mod installer;
pub mod tools;
pub mod url_policy;

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::db::{
    ensure_safe_canonical_dir, strip_win_verbatim, validate_download_dir, Database, JobPage,
    JobRecord, Preferences,
};
use crate::engine::{AppEngine, CreateDownloadRequest, MediaMetadata};
use crate::errors::AppError;
use crate::images::PreviewCache;
use crate::installer::{ToolInstallProgress, ToolInstaller};
use crate::tools::{build_health, HealthReport};

const MAX_RANGE_CHUNK_BYTES: u64 = 2 * 1024 * 1024;
const MAX_FULL_MEDIA_BYTES: u64 = 32 * 1024 * 1024;
const MAX_ZIP_SLIDE_BYTES: u64 = 25 * 1024 * 1024;
const MAX_ALBUM_SLIDES: usize = 100;
const MAX_CRASH_LOG_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct DownloadedMediaSlide {
    pub index: usize,
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloadedMediaPreview {
    pub job_id: String,
    pub kind: String,
    pub title: String,
    pub filename: String,
    pub container: String,
    pub file_size_bytes: Option<i64>,
    pub stream_url: Option<String>,
    pub slides: Vec<DownloadedMediaSlide>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InstallDependencyResponse {
    pub health: Option<HealthReport>,
    pub installed: Vec<String>,
    pub up_to_date: Vec<String>,
    pub message: String,
}

pub struct AppState {
    pub db: Arc<Database>,
    pub previews: Arc<PreviewCache>,
    pub engine: Arc<AppEngine>,
    pub installer: Arc<ToolInstaller>,
    pub db_path: PathBuf,
}

fn default_paths() -> (PathBuf, PathBuf) {
    let data_base = dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("MediaVault");
    let db_path = data_base.join("app.db");
    let download_dir = dirs::download_dir()
        .unwrap_or_else(|| data_base.clone())
        .join("MediaVault");
    (db_path, download_dir)
}

#[tauri::command]
async fn get_health(state: State<'_, AppState>) -> Result<HealthReport, AppError> {
    let prefs = state.db.get_preferences()?;
    let dl = PathBuf::from(&prefs.download_dir);
    let (tools, report) = build_health(&dl, &state.db_path).await;
    state.engine.update_tools(tools);
    Ok(report)
}

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Result<Preferences, AppError> {
    state.db.get_preferences()
}

#[tauri::command]
fn update_settings(
    prefs: Preferences,
    state: State<'_, AppState>,
) -> Result<Preferences, AppError> {
    state.db.save_preferences(&prefs)
}

#[tauri::command]
async fn analyze_media(url: String, state: State<'_, AppState>) -> Result<MediaMetadata, AppError> {
    state.engine.analyze(&url).await
}

#[tauri::command]
async fn enrich_media(url: String, state: State<'_, AppState>) -> Result<MediaMetadata, AppError> {
    state.engine.enrich(&url).await
}

#[tauri::command]
async fn get_preview(token: String, state: State<'_, AppState>) -> Result<String, AppError> {
    state.previews.fetch_data_url(&token).await
}

#[tauri::command]
async fn create_download(
    request: CreateDownloadRequest,
    state: State<'_, AppState>,
) -> Result<JobRecord, AppError> {
    state.engine.enqueue(request, None)
}

#[derive(Debug, Deserialize)]
pub struct ListParams {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub search: Option<String>,
    pub platform: Option<String>,
    pub kind: Option<String>,
    pub active: Option<bool>,
}

#[tauri::command]
fn list_downloads(params: ListParams, state: State<'_, AppState>) -> Result<JobPage, AppError> {
    let limit = params.limit.unwrap_or(5).clamp(1, 50);
    let offset = params.offset.unwrap_or(0).max(0);
    let mut page = state.db.list_jobs(
        limit,
        offset,
        params.search.as_deref(),
        params.platform.as_deref(),
        params.kind.as_deref(),
        params.active.unwrap_or(false),
    )?;
    for job in &mut page.items {
        job.thumbnail_preview = state.previews.register(job.thumbnail_url.as_deref());
        // Never expose raw external thumbnail URLs to WebView2; UI renders strictly via PreviewCache tokens.
        job.thumbnail_url = None;
    }
    Ok(page)
}

#[tauri::command]
fn cancel_download(job_id: String, state: State<'_, AppState>) -> Result<(), AppError> {
    state.engine.cancel(&job_id)
}

#[tauri::command]
async fn retry_download(
    job_id: String,
    state: State<'_, AppState>,
) -> Result<JobRecord, AppError> {
    let old = state.db.get_job(&job_id)?;
    if matches!(old.status.as_str(), "queued" | "downloading" | "processing") {
        return Err(AppError::new("JOB_STATE"));
    }
    let req = CreateDownloadRequest {
        url: old.url,
        kind: old.kind,
        quality: old.quality,
        container: old.container,
        audio_bitrate_kbps: old.audio_bitrate_kbps,
        allow_quality_fallback: old.allow_quality_fallback,
        image_index: old.image_index,
        output_dir: old
            .output_path
            .as_deref()
            .and_then(|p| Path::new(p).parent())
            .filter(|d| d.is_dir())
            .map(|d| d.display().to_string()),
    };
    state.engine.enqueue(req, Some(old.id))
}

#[tauri::command]
fn delete_download(
    job_id: String,
    delete_file: bool,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    let job = state.db.delete_job(&job_id)?;
    if delete_file {
        if let Some(path_str) = job.output_path {
            let p = Path::new(&path_str);
            if let Ok(meta) = std::fs::symlink_metadata(p) {
                if meta.file_type().is_file() {
                    let _ = std::fs::remove_file(p);
                }
            }
        }
    }
    Ok(())
}

#[tauri::command]
async fn move_download(
    job_id: String,
    state: State<'_, AppState>,
) -> Result<Option<String>, AppError> {
    let job = state.db.get_job(&job_id)?;
    if matches!(job.status.as_str(), "queued" | "downloading" | "processing") {
        return Err(AppError::new("JOB_STATE"));
    }
    let src_str = job
        .output_path
        .ok_or_else(|| AppError::new("FILE_NOT_FOUND"))?;
    let src_path = PathBuf::from(&src_str);
    let meta = std::fs::symlink_metadata(&src_path).map_err(|_| AppError::new("FILE_NOT_FOUND"))?;
    if !meta.file_type().is_file() {
        return Err(AppError::new("FILE_NOT_FOUND"));
    }

    let start_dir = src_path.parent().map(Path::to_path_buf);
    let picked = tokio::task::spawn_blocking(move || {
        let mut dialog = rfd::FileDialog::new().set_title("Pilih folder tujuan baru");
        if let Some(dir) = start_dir.filter(|d| d.is_dir()) {
            dialog = dialog.set_directory(dir);
        }
        dialog.pick_folder()
    })
    .await
    .map_err(|_| AppError::new("INTERNAL_ERROR"))?;

    let Some(target_dir) = picked else {
        return Ok(None);
    };

    let target_dir = validate_download_dir(&target_dir.display().to_string())?;
    std::fs::create_dir_all(&target_dir).map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;
    let canon_target = ensure_safe_canonical_dir(&target_dir)?;

    if let Some(src_parent) = src_path.parent() {
        if let Ok(canon_parent) = std::fs::canonicalize(src_parent) {
            if canon_parent == canon_target {
                return Ok(Some(src_str));
            }
        }
    }

    let desired_name = job
        .output_filename
        .as_deref()
        .or_else(|| src_path.file_name().and_then(|s| s.to_str()))
        .unwrap_or("media.bin");
    let dest_path = crate::engine::unique_dest_path(&canon_target, desired_name);

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
    let final_path_str = strip_win_verbatim(&dest_path);
    state
        .db
        .update_output_location(&job.id, &final_name, &final_path_str)?;
    Ok(Some(final_path_str))
}

#[cfg(windows)]
fn windows_explorer_exe() -> PathBuf {
    std::env::var_os("SystemRoot")
        .or_else(|| std::env::var_os("WINDIR"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("explorer.exe")
}

#[cfg(windows)]
fn normalize_windows_explorer_path(p: &Path) -> Result<String, AppError> {
    let meta = std::fs::symlink_metadata(p).map_err(|_| AppError::new("FILE_NOT_FOUND"))?;
    if meta.file_type().is_symlink() {
        return Err(AppError::new("FORBIDDEN"));
    }
    let resolved = std::fs::canonicalize(p).map_err(|_| AppError::new("FILE_NOT_FOUND"))?;
    let raw = resolved.display().to_string();
    // Explicitly reject UNC shares (`\\?\UNC\` or `\\server\share`) to prevent NTLM leaks.
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

#[tauri::command]
fn open_in_explorer(job_id: String, state: State<'_, AppState>) -> Result<(), AppError> {
    let job = state.db.get_job(&job_id)?;
    let path_str = job
        .output_path
        .ok_or_else(|| AppError::new("FILE_NOT_FOUND"))?;
    let p = Path::new(&path_str);
    let meta = std::fs::symlink_metadata(p).map_err(|_| AppError::new("FILE_NOT_FOUND"))?;
    if !meta.file_type().is_file() {
        return Err(AppError::new("FILE_NOT_FOUND"));
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let clean = normalize_windows_explorer_path(p)?;
        // Windows explorer.exe requires `/select,"C:\path with spaces\file.mp4"` —
        // quoting before `/select,` (which Command::arg does when the path contains spaces)
        // causes explorer.exe to ignore /select and open the default Home/Documents folder.
        std::process::Command::new(windows_explorer_exe())
            .raw_arg(format!("/select,\"{clean}\""))
            .spawn()
            .map_err(|_| AppError::new("INTERNAL_ERROR"))?;
    }
    Ok(())
}

#[tauri::command]
fn open_downloads_folder(state: State<'_, AppState>) -> Result<(), AppError> {
    let prefs = state.db.get_preferences()?;
    let dl = validate_download_dir(&prefs.download_dir)?;
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

#[tauri::command]
async fn install_dependency(
    tool: String,
    current_version: Option<String>,
    state: State<'_, AppState>,
) -> Result<InstallDependencyResponse, AppError> {
    let known = state.engine.current_tools();
    let outcome = state
        .installer
        .install(&tool, Some(&known), current_version.as_deref())
        .await?;
    let health = if outcome.installed.is_empty() {
        None
    } else {
        let prefs = state.db.get_preferences()?;
        let dl = PathBuf::from(&prefs.download_dir);
        let (tools, report) = build_health(&dl, &state.db_path).await;
        state.engine.update_tools(tools);
        Some(report)
    };
    Ok(InstallDependencyResponse {
        health,
        installed: outcome.installed,
        up_to_date: outcome.up_to_date,
        message: outcome.message,
    })
}

#[tauri::command]
fn get_install_status(state: State<'_, AppState>) -> ToolInstallProgress {
    state.installer.snapshot()
}

#[tauri::command]
async fn pick_download_folder(
    initial_dir: Option<String>,
    state: State<'_, AppState>,
) -> Result<Option<String>, AppError> {
    let current_dir = initial_dir
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .or_else(|| {
            state
                .db
                .get_preferences()
                .ok()
                .map(|p| PathBuf::from(p.download_dir))
                .filter(|p| p.is_dir())
        });

    let picked = tokio::task::spawn_blocking(move || {
        let mut dialog = rfd::FileDialog::new().set_title("Pilih folder simpan unduhan");
        if let Some(dir) = current_dir {
            dialog = dialog.set_directory(dir);
        }
        dialog.pick_folder()
    })
    .await
    .map_err(|_| AppError::new("INTERNAL_ERROR"))?;

    match picked {
        Some(dir) => {
            let valid = validate_download_dir(&dir.display().to_string())?;
            Ok(Some(strip_win_verbatim(&valid)))
        }
        None => Ok(None),
    }
}

fn mvmedia_base_url() -> &'static str {
    #[cfg(windows)]
    {
        "http://mvmedia.localhost"
    }
    #[cfg(not(windows))]
    {
        "mvmedia://localhost"
    }
}

fn webview_origin() -> &'static str {
    #[cfg(windows)]
    {
        "http://tauri.localhost"
    }
    #[cfg(not(windows))]
    {
        "tauri://localhost"
    }
}

fn mime_for_media_ext(ext: &str) -> Option<&'static str> {
    match ext {
        "mp4" => Some("video/mp4"),
        "webm" => Some("video/webm"),
        "mkv" => Some("video/x-matroska"),
        "mp3" => Some("audio/mpeg"),
        "m4a" => Some("audio/mp4"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

fn resolve_completed_job_file(db: &Database, job_id: &str) -> Result<(JobRecord, PathBuf, String), AppError> {
    if uuid::Uuid::parse_str(job_id).is_err() {
        return Err(AppError::new("NOT_FOUND"));
    }
    let job = db.get_job(job_id)?;
    if job.status != "completed" {
        return Err(AppError::new("FILE_NOT_FOUND"));
    }
    let path_str = job
        .output_path
        .as_deref()
        .ok_or_else(|| AppError::new("FILE_NOT_FOUND"))?;
    let path = PathBuf::from(path_str);
    let meta = std::fs::symlink_metadata(&path).map_err(|_| AppError::new("FILE_NOT_FOUND"))?;
    if !meta.file_type().is_file() {
        return Err(AppError::new("FILE_NOT_FOUND"));
    }
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if !matches!(
        ext.as_str(),
        "mp4" | "webm" | "mkv" | "mp3" | "m4a" | "jpg" | "jpeg" | "png" | "webp" | "zip"
    ) {
        return Err(AppError::new("FORMAT_UNAVAILABLE"));
    }
    Ok((job, path, ext))
}

fn list_zip_image_entries(zip_path: &Path) -> Result<Vec<String>, AppError> {
    let file = std::fs::File::open(zip_path).map_err(|_| AppError::new("FILE_NOT_FOUND"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|_| AppError::new("FORMAT_UNAVAILABLE"))?;
    let mut names = Vec::new();
    for i in 0..archive.len().min(2000) {
        let Ok(entry) = archive.by_index(i) else {
            continue;
        };
        if entry.is_dir() || entry.is_symlink() {
            continue;
        }
        let Some(enclosed) = entry.enclosed_name() else {
            continue;
        };
        let ext = enclosed
            .extension()
            .and_then(|s| s.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        if matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp") {
            names.push(entry.name().to_string());
        }
    }
    names.sort();
    names.truncate(MAX_ALBUM_SLIDES);
    Ok(names)
}

#[tauri::command]
async fn get_downloaded_media_preview(
    job_id: String,
    state: State<'_, AppState>,
) -> Result<DownloadedMediaPreview, AppError> {
    let (job, path, ext) = resolve_completed_job_file(&state.db, &job_id)?;
    let filename = job
        .output_filename
        .clone()
        .or_else(|| path.file_name().and_then(|s| s.to_str()).map(String::from))
        .unwrap_or_else(|| format!("media.{ext}"));
    let title = job.title.clone().unwrap_or_else(|| filename.clone());
    let base = mvmedia_base_url();

    if ext == "zip" {
        let zip_path = path.clone();
        let entry_names = tokio::task::spawn_blocking(move || list_zip_image_entries(&zip_path))
            .await
            .map_err(|_| AppError::new("INTERNAL_ERROR"))??;
        if entry_names.is_empty() {
            return Err(AppError::new("FORMAT_UNAVAILABLE"));
        }
        let slides = entry_names
            .into_iter()
            .enumerate()
            .map(|(idx, name)| DownloadedMediaSlide {
                index: idx + 1,
                name,
                url: format!("{base}/job/{job_id}/slide/{idx}"),
            })
            .collect();
        return Ok(DownloadedMediaPreview {
            job_id,
            kind: "album".into(),
            title,
            filename,
            container: ext,
            file_size_bytes: job.file_size_bytes,
            stream_url: None,
            slides,
        });
    }

    let kind = match ext.as_str() {
        "mp4" | "webm" | "mkv" => "video",
        "mp3" | "m4a" => "audio",
        _ => "image",
    };

    Ok(DownloadedMediaPreview {
        job_id: job_id.clone(),
        kind: kind.into(),
        title,
        filename,
        container: ext,
        file_size_bytes: job.file_size_bytes,
        stream_url: Some(format!("{base}/job/{job_id}")),
        slides: Vec::new(),
    })
}

fn mvmedia_error_response(status: u16) -> tauri::http::Response<Vec<u8>> {
    tauri::http::Response::builder()
        .status(status)
        .header("Cache-Control", "no-store")
        .header("X-Content-Type-Options", "nosniff")
        .header("Access-Control-Allow-Origin", webview_origin())
        .header("Vary", "Origin")
        .body(Vec::new())
        .unwrap()
}

fn parse_byte_range(header: &str, file_len: u64) -> Option<(u64, u64)> {
    let spec = header.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (start_s, end_s) = spec.split_once('-')?;
    if start_s.is_empty() {
        let suffix: u64 = end_s.trim().parse().ok()?;
        if suffix == 0 || file_len == 0 {
            return None;
        }
        let start = file_len.saturating_sub(suffix);
        return Some((start, file_len - 1));
    }
    let start: u64 = start_s.trim().parse().ok()?;
    if start >= file_len {
        return None;
    }
    let end = if end_s.trim().is_empty() {
        file_len - 1
    } else {
        end_s.trim().parse::<u64>().ok()?.min(file_len - 1)
    };
    if end < start {
        return None;
    }
    Some((start, end))
}

fn serve_mvmedia_request(
    db: &Database,
    request: &tauri::http::Request<Vec<u8>>,
) -> tauri::http::Response<Vec<u8>> {
    let method = request.method().as_str();
    if method != "GET" && method != "HEAD" {
        return mvmedia_error_response(405);
    }
    let raw_path = request.uri().path();
    let segs: Vec<&str> = raw_path.split('/').filter(|s| !s.is_empty()).collect();

    match segs.as_slice() {
        ["job", job_id] => {
            let Ok((_, path, ext)) = resolve_completed_job_file(db, job_id) else {
                return mvmedia_error_response(404);
            };
            let Some(mime) = mime_for_media_ext(&ext) else {
                return mvmedia_error_response(415);
            };
            let Ok(mut file) = std::fs::File::open(&path) else {
                return mvmedia_error_response(404);
            };
            let Ok(meta) = file.metadata() else {
                return mvmedia_error_response(404);
            };
            let file_len = meta.len();
            if method == "HEAD" {
                return tauri::http::Response::builder()
                    .status(200)
                    .header("Content-Type", mime)
                    .header("Accept-Ranges", "bytes")
                    .header("Content-Length", file_len.to_string())
                    .header("Cache-Control", "no-store")
                    .header("X-Content-Type-Options", "nosniff")
                    .header("Access-Control-Allow-Origin", webview_origin())
                    .header("Vary", "Origin")
                    .body(Vec::new())
                    .unwrap();
            }

            if let Some(range_val) = request
                .headers()
                .get(tauri::http::header::RANGE)
                .and_then(|v| v.to_str().ok())
            {
                let Some((start, req_end)) = parse_byte_range(range_val, file_len) else {
                    return tauri::http::Response::builder()
                        .status(416)
                        .header("Content-Range", format!("bytes */{file_len}"))
                        .header("Cache-Control", "no-store")
                        .header("X-Content-Type-Options", "nosniff")
                        .header("Access-Control-Allow-Origin", webview_origin())
                        .header("Vary", "Origin")
                        .body(Vec::new())
                        .unwrap();
                };
                let capped_end = req_end.min(start.saturating_add(MAX_RANGE_CHUNK_BYTES - 1));
                let chunk_len = (capped_end - start + 1) as usize;
                let mut buf = vec![0u8; chunk_len];
                if file.seek(SeekFrom::Start(start)).is_err() || file.read_exact(&mut buf).is_err() {
                    return mvmedia_error_response(500);
                }
                return tauri::http::Response::builder()
                    .status(206)
                    .header("Content-Type", mime)
                    .header("Accept-Ranges", "bytes")
                    .header("Content-Range", format!("bytes {start}-{capped_end}/{file_len}"))
                    .header("Content-Length", chunk_len.to_string())
                    .header("Cache-Control", "no-store")
                    .header("X-Content-Type-Options", "nosniff")
                    .header("Access-Control-Allow-Origin", webview_origin())
                    .header("Vary", "Origin")
                    .body(buf)
                    .unwrap();
            }

            if file_len > MAX_FULL_MEDIA_BYTES {
                // Force ranged streaming for very large files
                let end = (MAX_RANGE_CHUNK_BYTES - 1).min(file_len.saturating_sub(1));
                let chunk_len = (end + 1) as usize;
                let mut buf = vec![0u8; chunk_len];
                if file.read_exact(&mut buf).is_err() {
                    return mvmedia_error_response(500);
                }
                return tauri::http::Response::builder()
                    .status(206)
                    .header("Content-Type", mime)
                    .header("Accept-Ranges", "bytes")
                    .header("Content-Range", format!("bytes 0-{end}/{file_len}"))
                    .header("Content-Length", chunk_len.to_string())
                    .header("Cache-Control", "no-store")
                    .header("X-Content-Type-Options", "nosniff")
                    .header("Access-Control-Allow-Origin", webview_origin())
                    .header("Vary", "Origin")
                    .body(buf)
                    .unwrap();
            }

            let mut buf = Vec::with_capacity(file_len as usize);
            if file.read_to_end(&mut buf).is_err() {
                return mvmedia_error_response(500);
            }
            tauri::http::Response::builder()
                .status(200)
                .header("Content-Type", mime)
                .header("Accept-Ranges", "bytes")
                .header("Content-Length", buf.len().to_string())
                .header("Cache-Control", "no-store")
                .header("X-Content-Type-Options", "nosniff")
                .header("Access-Control-Allow-Origin", webview_origin())
                .header("Vary", "Origin")
                .body(buf)
                .unwrap()
        }
        ["job", job_id, "slide", slide_idx_str] => {
            let Ok(slide_idx) = slide_idx_str.parse::<usize>() else {
                return mvmedia_error_response(400);
            };
            let Ok((_, path, ext)) = resolve_completed_job_file(db, job_id) else {
                return mvmedia_error_response(404);
            };
            if ext != "zip" {
                return mvmedia_error_response(404);
            }
            let Ok(names) = list_zip_image_entries(&path) else {
                return mvmedia_error_response(404);
            };
            let Some(target_name) = names.get(slide_idx) else {
                return mvmedia_error_response(404);
            };
            let slide_ext = Path::new(target_name)
                .extension()
                .and_then(|s| s.to_str())
                .map(str::to_ascii_lowercase)
                .unwrap_or_default();
            let Some(mime) = mime_for_media_ext(&slide_ext) else {
                return mvmedia_error_response(415);
            };
            let Ok(file) = std::fs::File::open(&path) else {
                return mvmedia_error_response(404);
            };
            let Ok(mut archive) = zip::ZipArchive::new(file) else {
                return mvmedia_error_response(500);
            };
            let Ok(entry) = archive.by_name(target_name) else {
                return mvmedia_error_response(404);
            };
            if entry.is_symlink() {
                return mvmedia_error_response(403);
            }
            let mut buf = Vec::new();
            if entry
                .take(MAX_ZIP_SLIDE_BYTES + 1)
                .read_to_end(&mut buf)
                .is_err()
            {
                return mvmedia_error_response(500);
            }
            if buf.len() as u64 > MAX_ZIP_SLIDE_BYTES {
                return mvmedia_error_response(413);
            }
            tauri::http::Response::builder()
                .status(200)
                .header("Content-Type", mime)
                .header("Content-Length", buf.len().to_string())
                .header("Cache-Control", "no-store")
                .header("X-Content-Type-Options", "nosniff")
                .header("Access-Control-Allow-Origin", webview_origin())
                .header("Vary", "Origin")
                .body(if method == "HEAD" { Vec::new() } else { buf })
                .unwrap()
        }
        _ => mvmedia_error_response(404),
    }
}

pub fn run() {
    let (db_path, download_dir) = default_paths();
    let _ = std::fs::create_dir_all(&download_dir);
    if let Some(log_dir) = db_path.parent().map(Path::to_path_buf) {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let log_path = log_dir.join("crash.log");
            let too_big = std::fs::metadata(&log_path)
                .map(|m| m.len() >= MAX_CRASH_LOG_BYTES)
                .unwrap_or(false);
            let loc = info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
                .unwrap_or_else(|| "unknown".into());
            let msg = format!("[{}] PANIC at {loc}\n", chrono::Utc::now().to_rfc3339());
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(too_big)
                .append(!too_big)
                .open(log_path)
                .and_then(|mut f| std::io::Write::write_all(&mut f, msg.as_bytes()));
            prev(info);
        }));
    }
    let db = Arc::new(
        Database::open(&db_path, download_dir).expect("failed to initialize MediaVault SQLite"),
    );
    let db_for_protocol = Arc::clone(&db);
    let previews = Arc::new(PreviewCache::new());
    let engine = Arc::new(AppEngine::new(Arc::clone(&db), Arc::clone(&previews)));
    let installer = Arc::new(ToolInstaller::new());
    let state = AppState {
        db,
        previews,
        engine,
        installer,
        db_path,
    };

    tauri::Builder::default()
        .manage(state)
        .register_uri_scheme_protocol("mvmedia", move |_ctx, request| {
            serve_mvmedia_request(&db_for_protocol, &request)
        })
        .invoke_handler(tauri::generate_handler![
            get_health,
            get_settings,
            update_settings,
            analyze_media,
            enrich_media,
            get_preview,
            get_downloaded_media_preview,
            create_download,
            list_downloads,
            cancel_download,
            retry_download,
            delete_download,
            move_download,
            open_in_explorer,
            open_downloads_folder,
            install_dependency,
            get_install_status,
            pick_download_folder,
        ])
        .run(tauri::generate_context!())
        .expect("error while running MediaVault desktop");
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn normalizes_windows_explorer_paths_without_extended_prefix() {
        let dir = std::env::temp_dir().join(format!("mv exp test {}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("Semua Tentang Uang.mp4");
        std::fs::write(&file, b"demo").unwrap();

        let normalized = normalize_windows_explorer_path(&file).unwrap();
        assert!(!normalized.starts_with(r"\\?\"));
        assert!(!normalized.contains('/'));
        assert!(normalized.ends_with(r"\Semua Tentang Uang.mp4"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mvmedia_protocol_serves_range_video_zip_slide_and_blocks_invalid_ids() {
        use std::io::Write as _;
        let dir = std::env::temp_dir().join(format!("mv_proto_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Database::open(&dir.join("app.db"), dir.join("dl")).unwrap();
        let now = chrono::Utc::now().to_rfc3339();

        // 1. Video job with Range request
        let vid_id = uuid::Uuid::new_v4().to_string();
        let vid_path = dir.join("clip.mp4");
        std::fs::write(&vid_path, b"0123456789ABCDEF").unwrap();
        db.insert_job(
            &JobRecord {
                id: vid_id.clone(),
                url: "https://youtu.be/demo".into(),
                title: Some("Clip".into()),
                platform: "youtube".into(),
                kind: "video".into(),
                quality: "best".into(),
                container: "mp4".into(),
                audio_bitrate_kbps: None,
                allow_quality_fallback: false,
                image_index: None,
                status: "completed".into(),
                progress_percent: Some(100.0),
                downloaded_bytes: None,
                total_bytes: None,
                speed_bytes_per_second: None,
                eta_seconds: None,
                file_size_bytes: Some(16),
                output_filename: Some("clip.mp4".into()),
                output_path: Some(vid_path.display().to_string()),
                thumbnail_url: None,
                thumbnail_preview: None,
                error_code: None,
                error_message: None,
                file_available: true,
                created_at: now.clone(),
                updated_at: now.clone(),
                completed_at: Some(now.clone()),
                retry_of: None,
            },
            20,
        )
        .unwrap();

        let req_range = tauri::http::Request::builder()
            .method("GET")
            .uri(format!("http://mvmedia.localhost/job/{vid_id}"))
            .header("Range", "bytes=4-9")
            .body(Vec::new())
            .unwrap();
        let res_range = serve_mvmedia_request(&db, &req_range);
        assert_eq!(res_range.status().as_u16(), 206);
        assert_eq!(res_range.body().as_slice(), b"456789");
        assert_eq!(
            res_range.headers().get("Content-Range").unwrap().to_str().unwrap(),
            "bytes 4-9/16"
        );

        // 2. ZIP photo album slide request
        let zip_id = uuid::Uuid::new_v4().to_string();
        let zip_path = dir.join("album.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zw.start_file("photo_01.jpg", opts).unwrap();
            zw.write_all(b"JPEG_ONE").unwrap();
            zw.start_file("photo_02.jpg", opts).unwrap();
            zw.write_all(b"JPEG_TWO").unwrap();
            zw.finish().unwrap();
        }
        db.insert_job(
            &JobRecord {
                id: zip_id.clone(),
                url: "https://www.instagram.com/p/demo/".into(),
                title: Some("Album".into()),
                platform: "instagram".into(),
                kind: "image".into(),
                quality: "best".into(),
                container: "zip".into(),
                audio_bitrate_kbps: None,
                allow_quality_fallback: false,
                image_index: None,
                status: "completed".into(),
                progress_percent: Some(100.0),
                downloaded_bytes: None,
                total_bytes: None,
                speed_bytes_per_second: None,
                eta_seconds: None,
                file_size_bytes: Some(100),
                output_filename: Some("album.zip".into()),
                output_path: Some(zip_path.display().to_string()),
                thumbnail_url: None,
                thumbnail_preview: None,
                error_code: None,
                error_message: None,
                file_available: true,
                created_at: now.clone(),
                updated_at: now.clone(),
                completed_at: Some(now.clone()),
                retry_of: None,
            },
            20,
        )
        .unwrap();

        let req_slide = tauri::http::Request::builder()
            .method("GET")
            .uri(format!("http://mvmedia.localhost/job/{zip_id}/slide/1"))
            .body(Vec::new())
            .unwrap();
        let res_slide = serve_mvmedia_request(&db, &req_slide);
        assert_eq!(res_slide.status().as_u16(), 200);
        assert_eq!(res_slide.body().as_slice(), b"JPEG_TWO");

        // 3. Path traversal / non-UUID job id must return 404
        let req_bad = tauri::http::Request::builder()
            .method("GET")
            .uri("http://mvmedia.localhost/job/..%2F..%2FWindows%2Fwin.ini")
            .body(Vec::new())
            .unwrap();
        assert_eq!(serve_mvmedia_request(&db, &req_bad).status().as_u16(), 404);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
