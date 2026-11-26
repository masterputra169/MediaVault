use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use url::Url;
use zip::ZipArchive;

use crate::errors::AppError;
use crate::url_policy::{ensure_public_host, SHARED_HTTP};

const ALLOWED_INSTALL_HOSTS: &[&str] = &[
    "github.com",
    "api.github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

const YT_DLP_LATEST_PAGE: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest";
const YT_DLP_LATEST_API: &str = "https://api.github.com/repos/yt-dlp/yt-dlp/releases/latest";
#[cfg_attr(not(test), allow(dead_code))]
const YT_DLP_ZIP_URL: &str =
    "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_win.zip";
const GALLERY_DL_LATEST_PAGE: &str = "https://github.com/gdl-org/builds/releases/latest";
const GALLERY_DL_LATEST_API: &str = "https://api.github.com/repos/gdl-org/builds/releases/latest";
#[cfg_attr(not(test), allow(dead_code))]
const GALLERY_DL_EXE_URL: &str =
    "https://github.com/gdl-org/builds/releases/latest/download/gallery-dl_windows.exe";
const FFMPEG_LATEST_API: &str = "https://api.github.com/repos/GyanD/codexffmpeg/releases/latest";
const FFMPEG_FALLBACK_API: &str =
    "https://api.github.com/repos/BtbN/FFmpeg-Builds/releases/tags/latest";

const MAX_YTDLP_BYTES: u64 = 70 * 1024 * 1024;
const MAX_GALLERY_DL_BYTES: u64 = 60 * 1024 * 1024;
const MAX_FFMPEG_BYTES: u64 = 280 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: u64 = 450 * 1024 * 1024;
const MAX_ZIP_ENTRIES: usize = 2000;
const MAX_RELEASE_JSON_BYTES: usize = 512 * 1024;

struct ResolvedReleaseAsset {
    download_url: String,
    expected_sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct InstallOutcome {
    pub installed: Vec<String>,
    pub up_to_date: Vec<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolInstallProgress {
    pub active: bool,
    pub tool: Option<String>,
    pub stage: String,
    pub percent: Option<f64>,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub message: String,
}

impl Default for ToolInstallProgress {
    fn default() -> Self {
        Self {
            active: false,
            tool: None,
            stage: "idle".into(),
            percent: None,
            downloaded_bytes: 0,
            total_bytes: None,
            message: String::new(),
        }
    }
}

pub struct ToolInstaller {
    progress: Mutex<ToolInstallProgress>,
    run_lock: tokio::sync::Mutex<()>,
}

impl ToolInstaller {
    pub fn new() -> Self {
        Self {
            progress: Mutex::new(ToolInstallProgress::default()),
            run_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub fn snapshot(&self) -> ToolInstallProgress {
        self.progress.lock().unwrap().clone()
    }

    fn set_progress(
        &self,
        active: bool,
        tool: Option<&str>,
        stage: &str,
        percent: Option<f64>,
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
        message: impl Into<String>,
    ) {
        let mut p = self.progress.lock().unwrap();
        p.active = active;
        p.tool = tool.map(String::from);
        p.stage = stage.to_string();
        p.percent = percent;
        p.downloaded_bytes = downloaded_bytes;
        p.total_bytes = total_bytes;
        p.message = message.into();
    }

    pub async fn install(
        self: &Arc<Self>,
        target: &str,
        known_tools: Option<&crate::tools::ToolPaths>,
        known_raw_version: Option<&str>,
    ) -> Result<InstallOutcome, AppError> {
        let targets: Vec<&str> = match target {
            "yt_dlp" => vec!["yt_dlp"],
            "ffmpeg" => vec!["ffmpeg"],
            "gallery_dl" => vec!["gallery_dl"],
            "all_missing" => vec!["yt_dlp", "ffmpeg", "gallery_dl"],
            _ => return Err(AppError::new("INVALID_REQUEST")),
        };

        let Ok(_guard) = self.run_lock.try_lock() else {
            return Err(AppError::new("INSTALL_IN_PROGRESS"));
        };

        let bin_dir = app_bin_dir()?;
        std::fs::create_dir_all(&bin_dir).map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;

        let mut installed = Vec::new();
        let mut up_to_date = Vec::new();
        let mut last_up_to_date_ver: Option<(String, String)> = None;
        let existing = match known_tools {
            Some(t) => t.clone(),
            None => crate::tools::discover_tools(),
        };

        for &item in &targets {
            if target == "all_missing" {
                let already_present = match item {
                    "yt_dlp" => existing.yt_dlp.is_some(),
                    "ffmpeg" => existing.ffmpeg.is_some() && existing.ffprobe.is_some(),
                    "gallery_dl" => existing.gallery_dl.is_some(),
                    _ => false,
                };
                if already_present {
                    continue;
                }
            } else if matches!(item, "yt_dlp" | "gallery_dl") {
                // Before re-downloading an already-installed tool, verify if a newer release exists.
                let bin_opt = if item == "yt_dlp" {
                    existing.yt_dlp.as_ref()
                } else {
                    existing.gallery_dl.as_ref()
                };
                if let Some(bin) = bin_opt {
                    let display = if item == "yt_dlp" { "yt-dlp" } else { "gallery-dl" };
                    self.set_progress(
                        true,
                        Some(item),
                        "checking",
                        Some(20.0),
                        0,
                        None,
                        format!("Memeriksa versi terbaru {display}…"),
                    );
                    let latest_page = if item == "yt_dlp" {
                        YT_DLP_LATEST_PAGE
                    } else {
                        GALLERY_DL_LATEST_PAGE
                    };
                    let cached_ver = known_raw_version
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(String::from);
                    let (current_raw, latest_tag) = if let Some(cv) = cached_ver {
                        (Some(cv), fetch_latest_github_tag(latest_page).await)
                    } else {
                        tokio::join!(probe_raw_version(bin), fetch_latest_github_tag(latest_page))
                    };
                    if let (Some(cur), Some(tag)) = (current_raw, latest_tag) {
                        if is_version_up_to_date(&cur, &tag) {
                            up_to_date.push(item.to_string());
                            last_up_to_date_ver = Some((display.to_string(), normalize_tag(&tag)));
                            continue;
                        }
                    }
                }
            }

            let res = match item {
                "yt_dlp" => self.install_yt_dlp(&bin_dir).await,
                "ffmpeg" => self.install_ffmpeg(&bin_dir).await,
                "gallery_dl" => self.install_gallery_dl(&bin_dir).await,
                _ => Err(AppError::new("INVALID_REQUEST")),
            };

            if let Err(err) = res {
                self.set_progress(false, Some(item), "error", None, 0, None, err.message);
                return Err(err);
            }
            installed.push(item.to_string());
        }

        let summary = if !installed.is_empty() {
            "Komponen berhasil dipasang dan siap dipakai.".to_string()
        } else if let Some((name, ver)) = last_up_to_date_ver {
            format!("{name} sudah versi terbaru ({ver}).")
        } else {
            "Semua komponen sudah terpasang.".to_string()
        };

        self.set_progress(
            false,
            None,
            "done",
            Some(100.0),
            0,
            None,
            summary.clone(),
        );
        Ok(InstallOutcome {
            installed,
            up_to_date,
            message: summary,
        })
    }

    async fn install_yt_dlp(&self, bin_dir: &Path) -> Result<(), AppError> {
        let asset = resolve_github_release_asset(YT_DLP_LATEST_API, |name| name == "yt-dlp_win.zip")
            .await
            .ok_or_else(|| AppError::new("NETWORK_ERROR"))?;

        let stage_zip = bin_dir.join(".tmp-yt-dlp.zip");
        let stage_dir = bin_dir.join(".tmp-yt-dlp-dir");
        let final_dir = bin_dir.join("yt-dlp");
        let _ = std::fs::remove_file(&stage_zip);
        let _ = std::fs::remove_dir_all(&stage_dir);

        let (_, actual_sha) = self
            .download_allowlisted(
                "yt_dlp",
                "Mengunduh yt-dlp…",
                &asset.download_url,
                &stage_zip,
                MAX_YTDLP_BYTES,
            )
            .await?;

        if !verify_sha256_hex(&actual_sha, &asset.expected_sha256) {
            let _ = std::fs::remove_file(&stage_zip);
            return Err(AppError::new("INSTALL_FAILED"));
        }

        self.set_progress(
            true,
            Some("yt_dlp"),
            "extracting",
            Some(92.0),
            0,
            None,
            "Mengekstrak paket yt-dlp…",
        );

        let zip_path = stage_zip.clone();
        let out_dir = stage_dir.clone();
        let extract_res = tokio::task::spawn_blocking(move || extract_zip_all(&zip_path, &out_dir))
            .await
            .map_err(|_| AppError::new("INTERNAL_ERROR"))?;
        let _ = std::fs::remove_file(&stage_zip);
        if let Err(err) = extract_res {
            let _ = std::fs::remove_dir_all(&stage_dir);
            return Err(err);
        }

        if !std::fs::symlink_metadata(stage_dir.join("yt-dlp.exe"))
            .is_ok_and(|m| m.file_type().is_file())
        {
            let _ = std::fs::remove_dir_all(&stage_dir);
            return Err(AppError::new("INSTALL_FAILED"));
        }

        let backup_dir = bin_dir.join(".old-yt-dlp");
        let _ = std::fs::remove_dir_all(&backup_dir);
        if final_dir.exists() {
            let _ = std::fs::rename(&final_dir, &backup_dir);
        }
        if std::fs::rename(&stage_dir, &final_dir).is_err() {
            if backup_dir.exists() && !final_dir.exists() {
                let _ = std::fs::rename(&backup_dir, &final_dir);
            }
            let _ = std::fs::remove_dir_all(&stage_dir);
            return Err(AppError::new("STORAGE_UNAVAILABLE"));
        }
        let _ = std::fs::remove_dir_all(&backup_dir);
        Ok(())
    }

    async fn install_gallery_dl(&self, bin_dir: &Path) -> Result<(), AppError> {
        let asset = resolve_github_release_asset(GALLERY_DL_LATEST_API, |name| {
            name == "gallery-dl_windows.exe"
        })
        .await
        .ok_or_else(|| AppError::new("NETWORK_ERROR"))?;

        let stage_exe = bin_dir.join(".tmp-gallery-dl.exe");
        let final_exe = bin_dir.join("gallery-dl.exe");
        let backup_exe = bin_dir.join(".old-gallery-dl.exe");
        let _ = std::fs::remove_file(&stage_exe);

        let (_, actual_sha) = self
            .download_allowlisted(
                "gallery_dl",
                "Mengunduh gallery-dl…",
                &asset.download_url,
                &stage_exe,
                MAX_GALLERY_DL_BYTES,
            )
            .await?;

        if !verify_sha256_hex(&actual_sha, &asset.expected_sha256) {
            let _ = std::fs::remove_file(&stage_exe);
            return Err(AppError::new("INSTALL_FAILED"));
        }

        self.set_progress(
            true,
            Some("gallery_dl"),
            "extracting",
            Some(95.0),
            0,
            None,
            "Menyimpan gallery-dl.exe…",
        );

        atomic_replace_file(&stage_exe, &final_exe, &backup_exe)?;
        Ok(())
    }

    async fn install_ffmpeg(&self, bin_dir: &Path) -> Result<(), AppError> {
        self.set_progress(
            true,
            Some("ffmpeg"),
            "resolving",
            Some(2.0),
            0,
            None,
            "Mencari rilis resmi FFmpeg Windows…",
        );

        let asset = match resolve_github_release_asset(FFMPEG_LATEST_API, |name| {
            name.ends_with("-essentials_build.zip")
        })
        .await
        {
            Some(a) => a,
            None => resolve_github_release_asset(FFMPEG_FALLBACK_API, |name| {
                name == "ffmpeg-master-latest-win64-gpl.zip"
            })
            .await
            .ok_or_else(|| AppError::new("NETWORK_ERROR"))?,
        };

        let stage_zip = bin_dir.join(".tmp-ffmpeg.zip");
        let stage_ff = bin_dir.join(".tmp-ffmpeg.exe");
        let stage_fp = bin_dir.join(".tmp-ffprobe.exe");
        let _ = std::fs::remove_file(&stage_zip);
        let _ = std::fs::remove_file(&stage_ff);
        let _ = std::fs::remove_file(&stage_fp);

        let (_, actual_sha) = self
            .download_allowlisted(
                "ffmpeg",
                "Mengunduh FFmpeg + ffprobe…",
                &asset.download_url,
                &stage_zip,
                MAX_FFMPEG_BYTES,
            )
            .await?;

        if !verify_sha256_hex(&actual_sha, &asset.expected_sha256) {
            let _ = std::fs::remove_file(&stage_zip);
            return Err(AppError::new("INSTALL_FAILED"));
        }

        self.set_progress(
            true,
            Some("ffmpeg"),
            "extracting",
            Some(92.0),
            0,
            None,
            "Mengekstrak ffmpeg.exe & ffprobe.exe…",
        );

        let z = stage_zip.clone();
        let ff_out = stage_ff.clone();
        let fp_out = stage_fp.clone();
        let extract_res = tokio::task::spawn_blocking(move || {
            extract_named_bins(&z, &[("ffmpeg.exe", &ff_out), ("ffprobe.exe", &fp_out)])
        })
        .await
        .map_err(|_| AppError::new("INTERNAL_ERROR"))?;
        let _ = std::fs::remove_file(&stage_zip);
        if let Err(err) = extract_res {
            let _ = std::fs::remove_file(&stage_ff);
            let _ = std::fs::remove_file(&stage_fp);
            return Err(err);
        }

        if !stage_ff.is_file() || !stage_fp.is_file() {
            let _ = std::fs::remove_file(&stage_ff);
            let _ = std::fs::remove_file(&stage_fp);
            return Err(AppError::new("INSTALL_FAILED"));
        }

        let final_ff = bin_dir.join("ffmpeg.exe");
        let final_fp = bin_dir.join("ffprobe.exe");
        let backup_ff = bin_dir.join(".old-ffmpeg.exe");
        let backup_fp = bin_dir.join(".old-ffprobe.exe");
        if let Err(err) = atomic_replace_file(&stage_ff, &final_ff, &backup_ff) {
            let _ = std::fs::remove_file(&stage_fp);
            return Err(err);
        }
        if let Err(err) = atomic_replace_file(&stage_fp, &final_fp, &backup_fp) {
            return Err(err);
        }
        Ok(())
    }

    async fn download_allowlisted(
        &self,
        tool: &str,
        label: &str,
        initial_url: &str,
        dest: &Path,
        max_bytes: u64,
    ) -> Result<(u64, String), AppError> {
        let mut current = initial_url.to_string();
        let mut resp = None;

        for _ in 0..6 {
            validate_install_url(&current)?;
            ensure_public_host(&current).await?;
            let r = SHARED_HTTP
                .get(&current)
                .header("Accept", "application/octet-stream, */*")
                .timeout(Duration::from_secs(300))
                .send()
                .await
                .map_err(|_| AppError::new("NETWORK_ERROR"))?;
            let st = r.status().as_u16();
            if matches!(st, 301 | 302 | 303 | 307 | 308) {
                let loc = r
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| AppError::new("NETWORK_ERROR"))?;
                let base = Url::parse(&current).map_err(|_| AppError::new("INVALID_URL"))?;
                let next = base.join(loc).map_err(|_| AppError::new("INVALID_URL"))?;
                current = next.to_string();
                continue;
            }
            if !r.status().is_success() {
                return Err(AppError::new("NETWORK_ERROR"));
            }
            resp = Some(r);
            break;
        }

        let mut resp = resp.ok_or_else(|| AppError::new("NETWORK_ERROR"))?;
        let total = resp.content_length();
        if let Some(len) = total {
            if len > max_bytes {
                return Err(AppError::new("FILE_TOO_LARGE"));
            }
        }

        let mut file = File::create(dest).map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;
        let mut hasher = Sha256::new();
        let mut downloaded: u64 = 0;
        let mut last_emit = Instant::now();
        self.set_progress(true, Some(tool), "downloading", Some(3.0), 0, total, label);

        while let Some(chunk) = resp.chunk().await.map_err(|_| {
            let _ = std::fs::remove_file(dest);
            AppError::new("NETWORK_ERROR")
        })? {
            downloaded = downloaded.saturating_add(chunk.len() as u64);
            if downloaded > max_bytes {
                drop(file);
                let _ = std::fs::remove_file(dest);
                return Err(AppError::new("FILE_TOO_LARGE"));
            }
            hasher.update(&chunk);
            if file.write_all(&chunk).is_err() {
                drop(file);
                let _ = std::fs::remove_file(dest);
                return Err(AppError::new("STORAGE_UNAVAILABLE"));
            }
            if last_emit.elapsed() >= Duration::from_millis(180) {
                last_emit = Instant::now();
                let pct = total
                    .filter(|&t| t > 0)
                    .map(|t| ((downloaded as f64 / t as f64) * 88.0 + 3.0).min(91.0));
                self.set_progress(true, Some(tool), "downloading", pct, downloaded, total, label);
            }
        }
        if file.flush().is_err() {
            drop(file);
            let _ = std::fs::remove_file(dest);
            return Err(AppError::new("STORAGE_UNAVAILABLE"));
        }
        drop(file);

        if let Some(expected_len) = total {
            if downloaded != expected_len {
                let _ = std::fs::remove_file(dest);
                return Err(AppError::new("NETWORK_ERROR"));
            }
        }

        let digest = hasher.finalize();
        let mut hex = String::with_capacity(64);
        for b in digest {
            use std::fmt::Write as _;
            let _ = write!(&mut hex, "{b:02x}");
        }
        Ok((downloaded, hex))
    }
}

fn atomic_replace_file(stage: &Path, dest: &Path, backup: &Path) -> Result<(), AppError> {
    let _ = std::fs::remove_file(backup);
    let had_existing = dest.exists();
    if had_existing {
        if std::fs::rename(dest, backup).is_err() {
            let _ = std::fs::remove_file(stage);
            return Err(AppError::new("STORAGE_UNAVAILABLE"));
        }
    }
    if std::fs::rename(stage, dest).is_err() {
        if had_existing && backup.exists() && !dest.exists() {
            let _ = std::fs::rename(backup, dest);
        }
        let _ = std::fs::remove_file(stage);
        return Err(AppError::new("STORAGE_UNAVAILABLE"));
    }
    let _ = std::fs::remove_file(backup);
    Ok(())
}

pub fn verify_sha256_hex(actual: &str, expected: &str) -> bool {
    let a = actual.trim().to_ascii_lowercase();
    let e = expected
        .trim()
        .strip_prefix("sha256:")
        .unwrap_or(expected.trim())
        .to_ascii_lowercase();
    if a.len() != 64 || e.len() != 64 || !e.bytes().all(|b| b.is_ascii_hexdigit()) {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.bytes().zip(e.bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}

pub fn app_bin_dir() -> Result<PathBuf, AppError> {
    let base = dirs::data_local_dir().ok_or_else(|| AppError::new("STORAGE_UNAVAILABLE"))?;
    Ok(base.join("MediaVault").join("bin"))
}

pub fn validate_install_url(raw: &str) -> Result<(), AppError> {
    let parsed = Url::parse(raw).map_err(|_| AppError::new("FORBIDDEN"))?;
    if parsed.scheme() != "https" || !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(AppError::new("FORBIDDEN"));
    }
    if let Some(port) = parsed.port() {
        if port != 443 {
            return Err(AppError::new("FORBIDDEN"));
        }
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| AppError::new("FORBIDDEN"))?
        .to_lowercase();
    if !ALLOWED_INSTALL_HOSTS.contains(&host.as_str()) {
        return Err(AppError::new("FORBIDDEN"));
    }
    Ok(())
}

async fn probe_raw_version(bin: &Path) -> Option<String> {
    let out = tokio::time::timeout(
        Duration::from_secs(5),
        crate::tools::background_cmd(bin).arg("--version").output(),
    )
    .await
    .ok()?
    .ok()?;
    if !out.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    (!line.is_empty()).then_some(line)
}

async fn fetch_latest_github_tag(latest_url: &str) -> Option<String> {
    validate_install_url(latest_url).ok()?;
    ensure_public_host(latest_url).await.ok()?;
    let resp = SHARED_HTTP
        .head(latest_url)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .ok()?;
    let loc = resp.headers().get("location")?.to_str().ok()?;
    let base = Url::parse(latest_url).ok()?;
    let resolved = base.join(loc).ok()?;
    let resolved_str = resolved.as_str();
    validate_install_url(resolved_str).ok()?;
    if resolved.host_str() != Some("github.com") {
        return None;
    }
    let tag = resolved.path().rsplit("/tag/").next()?.trim();
    if tag.is_empty() || tag == resolved.path() {
        return None;
    }
    Some(tag.to_string())
}

fn normalize_tag(raw: &str) -> String {
    raw.trim().trim_start_matches(['v', 'V']).to_string()
}

fn parse_numeric_parts(s: &str) -> Option<Vec<u64>> {
    let clean = normalize_tag(s);
    let mut parts = Vec::new();
    for seg in clean.split('.') {
        let digits: String = seg.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            break;
        }
        parts.push(digits.parse::<u64>().ok()?);
    }
    (!parts.is_empty()).then_some(parts)
}

/// Returns true when the locally installed raw `--version` string is already at or newer
/// than `latest_tag`. Handles both date-versioned tools (`2026.11.23`) and `gallery-dl`
/// `gdl-org/builds` strings like `1.32.22-dev:2026.11.27` compared against tag `2026.11.27`.
pub fn is_version_up_to_date(installed_raw: &str, latest_tag: &str) -> bool {
    let latest_norm = normalize_tag(latest_tag);
    if latest_norm.is_empty() {
        return false;
    }
    let inst_trim = installed_raw.trim();
    // Direct candidate extraction: if installed contains `:YYYY.MM.DD` (gdl-org/builds), compare that date.
    let candidate = if let Some((_, after_colon)) = inst_trim.rsplit_once(':') {
        after_colon.trim()
    } else {
        inst_trim.split_whitespace().next().unwrap_or(inst_trim)
    };
    let cand_norm = normalize_tag(candidate);
    if cand_norm.eq_ignore_ascii_case(&latest_norm) {
        return true;
    }
    if let (Some(c_parts), Some(l_parts)) = (
        parse_numeric_parts(&cand_norm),
        parse_numeric_parts(&latest_norm),
    ) {
        // Guard against comparing mismatched numbering schemes (e.g. semver `1.32.21` vs date `2026.11.27`):
        // only compare numerically when both start in the same scale (both year >= 2000, or both < 100).
        let same_scheme = (c_parts[0] >= 2000 && l_parts[0] >= 2000)
            || (c_parts[0] < 100 && l_parts[0] < 100);
        if same_scheme {
            let max_len = c_parts.len().max(l_parts.len());
            for i in 0..max_len {
                let cv = *c_parts.get(i).unwrap_or(&0);
                let lv = *l_parts.get(i).unwrap_or(&0);
                if cv != lv {
                    return cv > lv;
                }
            }
            return true;
        }
    }
    false
}

async fn fetch_bounded_json(api_url: &str) -> Option<Value> {
    validate_install_url(api_url).ok()?;
    ensure_public_host(api_url).await.ok()?;
    let mut resp = SHARED_HTTP
        .get(api_url)
        .header("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(12))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    if let Some(len) = resp.content_length() {
        if len as usize > MAX_RELEASE_JSON_BYTES {
            return None;
        }
    }
    let mut buf = Vec::new();
    while let Some(chunk) = resp.chunk().await.ok()? {
        if buf.len().saturating_add(chunk.len()) > MAX_RELEASE_JSON_BYTES {
            return None;
        }
        buf.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&buf).ok()
}

async fn resolve_github_release_asset<F>(api_url: &str, matcher: F) -> Option<ResolvedReleaseAsset>
where
    F: Fn(&str) -> bool,
{
    let data = fetch_bounded_json(api_url).await?;
    let assets = data.get("assets")?.as_array()?;
    for a in assets {
        let name = a.get("name").and_then(Value::as_str).unwrap_or("");
        if !matcher(name) {
            continue;
        }
        let url = a.get("browser_download_url").and_then(Value::as_str)?;
        if validate_install_url(url).is_err() {
            continue;
        }
        let digest_raw = a.get("digest").and_then(Value::as_str)?;
        let expected_sha256 = digest_raw.strip_prefix("sha256:")?.trim().to_ascii_lowercase();
        if expected_sha256.len() != 64 || !expected_sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        return Some(ResolvedReleaseAsset {
            download_url: url.to_string(),
            expected_sha256,
        });
    }
    None
}

fn is_windows_reserved_name(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or(name)
        .trim_end_matches([' ', '.'])
        .to_ascii_uppercase();
    matches!(
        stem.as_str(),
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

fn safe_relative_path(raw: &Path) -> Option<PathBuf> {
    let mut clean = PathBuf::new();
    for comp in raw.components() {
        match comp {
            Component::Normal(part) => {
                let s = part.to_str()?;
                if s.is_empty()
                    || s.contains(':')
                    || s.chars().any(char::is_control)
                    || s.ends_with(['.', ' '])
                    || is_windows_reserved_name(s)
                {
                    return None;
                }
                clean.push(part);
            }
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!clean.as_os_str().is_empty()).then_some(clean)
}

fn copy_bounded_entry<R: Read>(
    entry: &mut R,
    out_file: &mut File,
    total_written: &mut u64,
    limit: u64,
) -> Result<(), AppError> {
    let remaining = limit.saturating_sub(*total_written);
    let mut limited = entry.take(remaining.saturating_add(1));
    let copied =
        io::copy(&mut limited, out_file).map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;
    *total_written = total_written.saturating_add(copied);
    if *total_written > limit {
        return Err(AppError::new("FILE_TOO_LARGE"));
    }
    Ok(())
}

fn extract_zip_all(zip_path: &Path, dest_dir: &Path) -> Result<(), AppError> {
    extract_zip_all_with_limit(zip_path, dest_dir, MAX_EXTRACTED_BYTES)
}

fn extract_zip_all_with_limit(
    zip_path: &Path,
    dest_dir: &Path,
    max_bytes: u64,
) -> Result<(), AppError> {
    std::fs::create_dir_all(dest_dir).map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;
    let file = File::open(zip_path).map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;
    let mut archive = ZipArchive::new(file).map_err(|_| AppError::new("EXTRACTOR_FAILED"))?;
    if archive.len() > MAX_ZIP_ENTRIES {
        return Err(AppError::new("FILE_TOO_LARGE"));
    }
    let mut total_written: u64 = 0;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|_| AppError::new("EXTRACTOR_FAILED"))?;
        if entry.is_symlink() {
            return Err(AppError::new("FORBIDDEN"));
        }
        let Some(enclosed) = entry.enclosed_name().map(PathBuf::from) else {
            return Err(AppError::new("FORBIDDEN"));
        };
        let Some(rel) = safe_relative_path(&enclosed) else {
            return Err(AppError::new("FORBIDDEN"));
        };
        let out_path = dest_dir.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out_path).map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;
        }
        let mut out_file =
            File::create(&out_path).map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;
        if let Err(err) = copy_bounded_entry(&mut entry, &mut out_file, &mut total_written, max_bytes)
        {
            drop(out_file);
            let _ = std::fs::remove_file(&out_path);
            return Err(err);
        }
    }
    Ok(())
}

fn extract_named_bins(zip_path: &Path, wanted: &[(&str, &Path)]) -> Result<(), AppError> {
    let file = File::open(zip_path).map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;
    let mut archive = ZipArchive::new(file).map_err(|_| AppError::new("EXTRACTOR_FAILED"))?;
    if archive.len() > MAX_ZIP_ENTRIES {
        return Err(AppError::new("FILE_TOO_LARGE"));
    }
    let mut total_written: u64 = 0;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|_| AppError::new("EXTRACTOR_FAILED"))?;
        if entry.is_symlink() {
            return Err(AppError::new("FORBIDDEN"));
        }
        if entry.is_dir() {
            continue;
        }
        let Some(enclosed) = entry.enclosed_name().map(PathBuf::from) else {
            continue;
        };
        if safe_relative_path(&enclosed).is_none() {
            continue;
        }
        let Some(fname) = enclosed.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if let Some((_, dest_path)) = wanted
            .iter()
            .find(|(target, _)| target.eq_ignore_ascii_case(fname))
        {
            let mut out_file =
                File::create(dest_path).map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;
            if let Err(err) = copy_bounded_entry(
                &mut entry,
                &mut out_file,
                &mut total_written,
                MAX_EXTRACTED_BYTES,
            ) {
                drop(out_file);
                let _ = std::fs::remove_file(dest_path);
                return Err(err);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    #[test]
    fn enforces_official_install_host_allowlist() {
        assert!(validate_install_url(YT_DLP_ZIP_URL).is_ok());
        assert!(validate_install_url(GALLERY_DL_EXE_URL).is_ok());
        assert!(validate_install_url(
            "https://release-assets.githubusercontent.com/github-production-release-asset/1/yt-dlp_win.zip"
        )
        .is_ok());
        assert!(validate_install_url("http://github.com/yt-dlp/yt-dlp").is_err());
        assert!(validate_install_url("https://evil.example/yt-dlp_win.zip").is_err());
        assert!(validate_install_url("https://github.com.evil.tld/a.zip").is_err());
        assert!(validate_install_url("https://127.0.0.1/a.zip").is_err());
    }

    #[test]
    fn compares_installed_and_latest_versions_accurately() {
        // yt-dlp date-based versions
        assert!(is_version_up_to_date("2026.11.23", "2026.11.23"));
        assert!(is_version_up_to_date("2026.11.25", "2026.11.23"));
        assert!(!is_version_up_to_date("2026.10.01", "2026.11.23"));

        // gallery-dl gdl-org/builds format (`1.32.22-dev:2026.11.27` vs tag `2026.11.27`)
        assert!(is_version_up_to_date("1.32.22-dev:2026.11.27", "2026.11.27"));
        assert!(!is_version_up_to_date("1.32.22-dev:2026.11.20", "2026.11.27"));

        // Old semver `1.32.21` vs date-based `gdl-org/builds` tag `2026.11.27` must trigger update once
        assert!(!is_version_up_to_date("1.32.21", "2026.11.27"));
        // Semver vs semver
        assert!(is_version_up_to_date("1.32.21", "v1.32.21"));
        assert!(!is_version_up_to_date("1.32.20", "v1.32.21"));
    }

    #[test]
    fn extracts_zip_safely_and_blocks_zip_slip() {
        let tmp = std::env::temp_dir().join(format!("mv_zip_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();

        // Good zip containing nested ffmpeg.exe and ffprobe.exe
        let good_zip = tmp.join("good.zip");
        {
            let f = File::create(&good_zip).unwrap();
            let mut zw = ZipWriter::new(f);
            let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            zw.start_file("ffmpeg-9.1/bin/ffmpeg.exe", opts).unwrap();
            zw.write_all(b"FFMPEG_BIN").unwrap();
            zw.start_file("ffmpeg-9.1/bin/ffprobe.exe", opts).unwrap();
            zw.write_all(b"FFPROBE_BIN").unwrap();
            zw.finish().unwrap();
        }
        let ff = tmp.join("ffmpeg.exe");
        let fp = tmp.join("ffprobe.exe");
        extract_named_bins(&good_zip, &[("ffmpeg.exe", &ff), ("ffprobe.exe", &fp)]).unwrap();
        assert_eq!(std::fs::read(&ff).unwrap(), b"FFMPEG_BIN");
        assert_eq!(std::fs::read(&fp).unwrap(), b"FFPROBE_BIN");

        // Malicious zip-slip path must be rejected
        let bad_zip = tmp.join("bad.zip");
        {
            let f = File::create(&bad_zip).unwrap();
            let mut zw = ZipWriter::new(f);
            let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            zw.start_file("../escape.exe", opts).unwrap();
            zw.write_all(b"BAD").unwrap();
            zw.finish().unwrap();
        }
        let out_dir = tmp.join("out");
        assert!(extract_zip_all(&bad_zip, &out_dir).is_err());
        assert!(!tmp.join("escape.exe").exists());

        // Zip bomb must abort immediately when exceeding byte limit (before full inflation)
        let bomb_zip = tmp.join("bomb.zip");
        {
            let f = File::create(&bomb_zip).unwrap();
            let mut zw = ZipWriter::new(f);
            let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            zw.start_file("big.bin", opts).unwrap();
            zw.write_all(&vec![0u8; 64 * 1024]).unwrap();
            zw.finish().unwrap();
        }
        let bomb_out = tmp.join("bomb_out");
        assert_eq!(
            extract_zip_all_with_limit(&bomb_zip, &bomb_out, 1024)
                .unwrap_err()
                .code,
            "FILE_TOO_LARGE"
        );
        assert!(!bomb_out.join("big.bin").exists());

        // NTFS ADS and Windows reserved device names must be blocked
        assert!(safe_relative_path(Path::new("sub/file:ads.txt")).is_none());
        assert!(safe_relative_path(Path::new("CON.txt")).is_none());
        assert!(safe_relative_path(Path::new("sub/NUL")).is_none());

        // SHA-256 verifier checks exact 64-hex match
        let h = "a".repeat(64);
        assert!(verify_sha256_hex(&h, &format!("sha256:{h}")));
        assert!(!verify_sha256_hex(&h, &"b".repeat(64)));
        assert!(!verify_sha256_hex("short", "short"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    #[ignore]
    async fn live_smoke_install_yt_dlp_and_gallery_dl_to_temp_dir() {
        let tmp = std::env::temp_dir().join(format!("mv_inst_live_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        let installer = Arc::new(ToolInstaller::new());

        let t0 = Instant::now();
        installer
            .install_yt_dlp(&tmp)
            .await
            .expect("live yt-dlp install failed");
        let yt_bin = tmp.join("yt-dlp").join("yt-dlp.exe");
        assert!(yt_bin.is_file());
        let yt_out = crate::tools::background_cmd(&yt_bin)
            .arg("--version")
            .output()
            .await
            .unwrap();
        assert!(yt_out.status.success());
        println!(
            "LIVE_INSTALL_YTDLP OK ({:.2}s): {}",
            t0.elapsed().as_secs_f64(),
            String::from_utf8_lossy(&yt_out.stdout).trim()
        );

        let t1 = Instant::now();
        installer
            .install_gallery_dl(&tmp)
            .await
            .expect("live gallery-dl install failed");
        let gdl_bin = tmp.join("gallery-dl.exe");
        assert!(gdl_bin.is_file());
        let gdl_out = crate::tools::background_cmd(&gdl_bin)
            .arg("--version")
            .output()
            .await
            .unwrap();
        assert!(gdl_out.status.success());
        println!(
            "LIVE_INSTALL_GALLERY_DL OK ({:.2}s): {}",
            t1.elapsed().as_secs_f64(),
            String::from_utf8_lossy(&gdl_out.stdout).trim()
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
