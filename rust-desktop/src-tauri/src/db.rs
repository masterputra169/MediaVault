use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::errors::AppError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preferences {
    pub default_video_quality: String,
    pub default_audio_format: String,
    pub theme: String,
    pub download_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRecord {
    pub id: String,
    pub url: String,
    pub title: Option<String>,
    pub platform: String,
    pub kind: String,
    pub quality: String,
    pub container: String,
    pub audio_bitrate_kbps: Option<i64>,
    pub allow_quality_fallback: bool,
    pub image_index: Option<i64>,
    pub status: String,
    pub progress_percent: Option<f64>,
    pub downloaded_bytes: Option<i64>,
    pub total_bytes: Option<i64>,
    pub speed_bytes_per_second: Option<f64>,
    pub eta_seconds: Option<i64>,
    pub file_size_bytes: Option<i64>,
    pub output_filename: Option<String>,
    pub output_path: Option<String>,
    pub thumbnail_url: Option<String>,
    pub thumbnail_preview: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub file_available: bool,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
    pub retry_of: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobCounts {
    pub all: i64,
    pub active: i64,
    pub video: i64,
    pub audio: i64,
    pub image: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobPage {
    pub items: Vec<JobRecord>,
    pub limit: i64,
    pub offset: i64,
    pub total: i64,
    pub active_count: i64,
    pub counts: JobCounts,
}

pub struct Database {
    conn: Mutex<Connection>,
    default_download_dir: PathBuf,
}

impl Database {
    pub fn open(db_path: &Path, default_download_dir: PathBuf) -> Result<Self, AppError> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;
        }
        let conn = Connection::open(db_path).map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;
            CREATE TABLE IF NOT EXISTS jobs (
                id TEXT PRIMARY KEY,
                source_url TEXT NOT NULL,
                title TEXT,
                platform TEXT NOT NULL,
                kind TEXT NOT NULL,
                quality TEXT NOT NULL,
                container TEXT NOT NULL,
                audio_bitrate_kbps INTEGER,
                allow_quality_fallback INTEGER NOT NULL DEFAULT 0,
                image_index INTEGER,
                status TEXT NOT NULL,
                progress_percent REAL,
                downloaded_bytes INTEGER,
                total_bytes INTEGER,
                speed_bytes_per_second REAL,
                eta_seconds INTEGER,
                file_size_bytes INTEGER,
                output_filename TEXT,
                output_path TEXT,
                thumbnail_url TEXT,
                error_code TEXT,
                error_message TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                completed_at TEXT,
                retry_of TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_jobs_created_at ON jobs(created_at DESC);
            CREATE TABLE IF NOT EXISTS preferences (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            ",
        )
        .map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;

        // Crash recovery: mark unfinished jobs from previous launch as interrupted
        let now = Utc::now().to_rfc3339();
        let _ = conn.execute(
            "UPDATE jobs SET status = 'interrupted', error_code = 'INTERRUPTED', error_message = 'Pekerjaan terhenti saat aplikasi ditutup.', updated_at = ?1 WHERE status IN ('queued', 'downloading', 'processing')",
            params![now],
        );

        let saved_dir: Option<String> = conn
            .query_row(
                "SELECT value FROM preferences WHERE key = 'download_dir'",
                [],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten();

        // Clean leftover .tmp staging subdirectories from interrupted/crashed sessions
        for base in [
            Some(default_download_dir.clone()),
            saved_dir.as_deref().and_then(|s| validate_download_dir(s).ok()),
        ]
        .into_iter()
        .flatten()
        {
            let tmp_root = base.join(".tmp");
            if std::fs::symlink_metadata(&tmp_root).is_ok_and(|m| m.file_type().is_dir()) {
                let _ = std::fs::remove_dir_all(&tmp_root);
            }
        }

        Ok(Self {
            conn: Mutex::new(conn),
            default_download_dir,
        })
    }

    pub fn get_preferences(&self) -> Result<Preferences, AppError> {
        let conn = self.conn.lock().unwrap();
        let get = |key: &str, default: &str| -> String {
            conn.query_row(
                "SELECT value FROM preferences WHERE key = ?1",
                params![key],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .ok()
            .flatten()
            .unwrap_or_else(|| default.to_string())
        };
        Ok(Preferences {
            default_video_quality: get("default_video_quality", "best"),
            default_audio_format: get("default_audio_format", "mp3"),
            theme: get("theme", "dark"),
            download_dir: get(
                "download_dir",
                &self.default_download_dir.display().to_string(),
            ),
        })
    }

    pub fn save_preferences(&self, prefs: &Preferences) -> Result<Preferences, AppError> {
        if !matches!(
            prefs.default_video_quality.as_str(),
            "best" | "1080" | "720" | "480" | "360"
        ) || !matches!(prefs.default_audio_format.as_str(), "mp3" | "m4a")
            || !matches!(prefs.theme.as_str(), "dark" | "light" | "system")
        {
            return Err(AppError::new("INVALID_REQUEST"));
        }
        let dir = validate_download_dir(&prefs.download_dir)?;
        std::fs::create_dir_all(&dir).map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;
        let canonical = ensure_safe_canonical_dir(&dir)?;
        let normalized_str = strip_win_verbatim(&canonical);
        let conn = self.conn.lock().unwrap();
        for (k, v) in [
            ("default_video_quality", prefs.default_video_quality.as_str()),
            ("default_audio_format", prefs.default_audio_format.as_str()),
            ("theme", prefs.theme.as_str()),
            ("download_dir", normalized_str.as_str()),
        ] {
            conn.execute(
                "INSERT INTO preferences(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![k, v],
            )
            .map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;
        }
        drop(conn);
        self.get_preferences()
    }

    pub fn insert_job(&self, job: &JobRecord, queue_limit: i64) -> Result<(), AppError> {
        let conn = self.conn.lock().unwrap();
        let active: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM jobs WHERE status IN ('queued', 'downloading', 'processing')",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if active >= queue_limit {
            return Err(AppError::new("QUEUE_FULL"));
        }
        conn.execute(
            "INSERT INTO jobs (
                id, source_url, title, platform, kind, quality, container,
                audio_bitrate_kbps, allow_quality_fallback, image_index,
                status, progress_percent, downloaded_bytes, total_bytes,
                speed_bytes_per_second, eta_seconds, file_size_bytes,
                output_filename, output_path, thumbnail_url,
                error_code, error_message, created_at, updated_at, completed_at, retry_of
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7,
                ?8, ?9, ?10,
                ?11, ?12, ?13, ?14,
                ?15, ?16, ?17,
                ?18, ?19, ?20,
                ?21, ?22, ?23, ?24, ?25, ?26
            )",
            params![
                job.id,
                job.url,
                job.title,
                job.platform,
                job.kind,
                job.quality,
                job.container,
                job.audio_bitrate_kbps,
                if job.allow_quality_fallback { 1 } else { 0 },
                job.image_index,
                job.status,
                job.progress_percent,
                job.downloaded_bytes,
                job.total_bytes,
                job.speed_bytes_per_second,
                job.eta_seconds,
                job.file_size_bytes,
                job.output_filename,
                job.output_path,
                job.thumbnail_url,
                job.error_code,
                job.error_message,
                job.created_at,
                job.updated_at,
                job.completed_at,
                job.retry_of,
            ],
        )
        .map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;
        Ok(())
    }

    pub fn update_progress(
        &self,
        id: &str,
        status: &str,
        progress: Option<f64>,
        downloaded: Option<i64>,
        total: Option<i64>,
        speed: Option<f64>,
        eta: Option<i64>,
    ) {
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            "UPDATE jobs SET status = ?2, progress_percent = COALESCE(?3, progress_percent), downloaded_bytes = COALESCE(?4, downloaded_bytes), total_bytes = COALESCE(?5, total_bytes), speed_bytes_per_second = ?6, eta_seconds = ?7, updated_at = ?8 WHERE id = ?1 AND status NOT IN ('cancelled', 'completed', 'failed')",
            params![id, status, progress, downloaded, total, speed, eta, now],
        );
    }

    pub fn mark_completed(
        &self,
        id: &str,
        title: Option<&str>,
        thumbnail_url: Option<&str>,
        output_filename: &str,
        output_path: &str,
        file_size_bytes: i64,
    ) {
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            "UPDATE jobs SET status = 'completed', progress_percent = 100.0, title = COALESCE(?2, title), thumbnail_url = COALESCE(?3, thumbnail_url), output_filename = ?4, output_path = ?5, file_size_bytes = ?6, error_code = NULL, error_message = NULL, updated_at = ?7, completed_at = ?7 WHERE id = ?1",
            params![id, title, thumbnail_url, output_filename, output_path, file_size_bytes, now],
        );
    }

    pub fn update_output_location(
        &self,
        id: &str,
        output_filename: &str,
        output_path: &str,
    ) -> Result<(), AppError> {
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().unwrap();
        let changed = conn
            .execute(
                "UPDATE jobs SET output_filename = ?2, output_path = ?3, updated_at = ?4 WHERE id = ?1",
                params![id, output_filename, output_path, now],
            )
            .map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;
        if changed == 0 {
            return Err(AppError::new("JOB_NOT_FOUND"));
        }
        Ok(())
    }

    pub fn mark_failed(&self, id: &str, code: &str, message: &str) {
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            "UPDATE jobs SET status = 'failed', error_code = ?2, error_message = ?3, speed_bytes_per_second = NULL, eta_seconds = NULL, updated_at = ?4 WHERE id = ?1 AND status != 'cancelled'",
            params![id, code, message, now],
        );
    }

    pub fn mark_cancelled(&self, id: &str) -> Result<(), AppError> {
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().unwrap();
        let changed = conn
            .execute(
                "UPDATE jobs SET status = 'cancelled', speed_bytes_per_second = NULL, eta_seconds = NULL, updated_at = ?2 WHERE id = ?1 AND status IN ('queued', 'downloading', 'processing')",
                params![id, now],
            )
            .map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;
        if changed == 0 {
            return Err(AppError::new("JOB_STATE"));
        }
        Ok(())
    }

    pub fn get_job(&self, id: &str) -> Result<JobRecord, AppError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT * FROM jobs WHERE id = ?1", params![id], row_to_job)
            .optional()
            .map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?
            .ok_or_else(|| AppError::new("JOB_NOT_FOUND"))
    }

    pub fn delete_job(&self, id: &str) -> Result<JobRecord, AppError> {
        let job = self.get_job(id)?;
        if matches!(job.status.as_str(), "queued" | "downloading" | "processing") {
            return Err(AppError::new("JOB_STATE"));
        }
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM jobs WHERE id = ?1", params![id])
            .map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;
        Ok(job)
    }

    pub fn list_jobs(
        &self,
        limit: i64,
        offset: i64,
        search: Option<&str>,
        platform: Option<&str>,
        kind: Option<&str>,
        active: bool,
    ) -> Result<JobPage, AppError> {
        let conn = self.conn.lock().unwrap();
        let mut clauses: Vec<String> = Vec::new();
        let mut values: Vec<rusqlite::types::Value> = Vec::new();

        if active {
            clauses.push("status IN ('queued', 'downloading', 'processing')".into());
        }
        if let Some(p) = platform.filter(|s| !s.is_empty()) {
            clauses.push("platform = ?".into());
            values.push(p.to_string().into());
        }
        if let Some(k) = kind.filter(|s| !s.is_empty()) {
            clauses.push("kind = ?".into());
            values.push(k.to_string().into());
        }
        if let Some(q) = search.map(str::trim).filter(|s| !s.is_empty()) {
            let escaped = q
                .to_lowercase()
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            let pattern = format!("%{escaped}%");
            clauses.push("(LOWER(COALESCE(title, '')) LIKE ? ESCAPE '\\' OR LOWER(COALESCE(output_filename, '')) LIKE ? ESCAPE '\\' OR LOWER(platform) LIKE ? ESCAPE '\\')".into());
            values.push(pattern.clone().into());
            values.push(pattern.clone().into());
            values.push(pattern.into());
        }

        let where_sql = if clauses.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", clauses.join(" AND "))
        };

        let total: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM jobs{where_sql}"),
                rusqlite::params_from_iter(values.iter()),
                |r| r.get(0),
            )
            .map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;

        let mut page_values = values;
        page_values.push(limit.into());
        page_values.push(offset.into());

        let mut stmt = conn
            .prepare(&format!(
                "SELECT * FROM jobs{where_sql} ORDER BY created_at DESC LIMIT ? OFFSET ?"
            ))
            .map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;

        let items: Vec<JobRecord> = stmt
            .query_map(rusqlite::params_from_iter(page_values.iter()), row_to_job)
            .map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?
            .filter_map(Result::ok)
            .collect();

        let counts = conn
            .query_row(
                "SELECT
                    COUNT(*),
                    SUM(CASE WHEN status IN ('queued','downloading','processing') THEN 1 ELSE 0 END),
                    SUM(CASE WHEN kind = 'video' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN kind = 'audio' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN kind = 'image' THEN 1 ELSE 0 END)
                 FROM jobs",
                [],
                |r| {
                    Ok(JobCounts {
                        all: r.get::<_, i64>(0)?,
                        active: r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                        video: r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                        audio: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                        image: r.get::<_, Option<i64>>(4)?.unwrap_or(0),
                    })
                },
            )
            .map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;

        Ok(JobPage {
            items,
            limit,
            offset,
            total,
            active_count: counts.active,
            counts,
        })
    }
}

fn row_to_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<JobRecord> {
    let output_path: Option<String> = row.get("output_path")?;
    let file_available = output_path
        .as_deref()
        .is_some_and(|p| Path::new(p).is_file());
    Ok(JobRecord {
        id: row.get("id")?,
        url: row.get("source_url")?,
        title: row.get("title")?,
        platform: row.get("platform")?,
        kind: row.get("kind")?,
        quality: row.get("quality")?,
        container: row.get("container")?,
        audio_bitrate_kbps: row.get("audio_bitrate_kbps")?,
        allow_quality_fallback: row.get::<_, i64>("allow_quality_fallback")? != 0,
        image_index: row.get("image_index")?,
        status: row.get("status")?,
        progress_percent: row.get("progress_percent")?,
        downloaded_bytes: row.get("downloaded_bytes")?,
        total_bytes: row.get("total_bytes")?,
        speed_bytes_per_second: row.get("speed_bytes_per_second")?,
        eta_seconds: row.get("eta_seconds")?,
        file_size_bytes: row.get("file_size_bytes")?,
        output_filename: row.get("output_filename")?,
        output_path,
        thumbnail_url: row.get("thumbnail_url")?,
        thumbnail_preview: None,
        error_code: row.get("error_code")?,
        error_message: row.get("error_message")?,
        file_available,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        completed_at: row.get("completed_at")?,
        retry_of: row.get("retry_of")?,
    })
}

pub fn strip_win_verbatim(p: &Path) -> String {
    let raw = p.display().to_string();
    #[cfg(windows)]
    {
        if let Some(rest) = raw.strip_prefix(r"\\?\") {
            return rest.to_string();
        }
    }
    raw
}

fn is_forbidden_download_dir(path: &Path) -> bool {
    let norm = strip_win_verbatim(path)
        .replace('/', r"\")
        .trim_end_matches('\\')
        .to_lowercase();
    if norm.is_empty() {
        return true;
    }

    let mut blocked: Vec<PathBuf> = Vec::new();
    if let Some(local) = dirs::data_local_dir() {
        blocked.push(local.join("MediaVault").join("bin"));
    }
    #[cfg(windows)]
    {
        for env_key in ["SystemRoot", "WINDIR", "ProgramFiles", "ProgramFiles(x86)", "ProgramData"] {
            if let Some(v) = std::env::var_os(env_key) {
                blocked.push(PathBuf::from(v));
            }
        }
        if let Some(roaming) = dirs::config_dir() {
            blocked.push(
                roaming
                    .join("Microsoft")
                    .join("Windows")
                    .join("Start Menu")
                    .join("Programs")
                    .join("Startup"),
            );
        }
    }

    for b in blocked {
        let bn = strip_win_verbatim(&b)
            .replace('/', r"\")
            .trim_end_matches('\\')
            .to_lowercase();
        if !bn.is_empty() && (norm == bn || norm.starts_with(&format!(r"{bn}\"))) {
            return true;
        }
    }
    false
}

pub fn validate_download_dir(raw: &str) -> Result<PathBuf, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.len() > 260
        || trimmed.contains('"')
        || trimmed.chars().any(char::is_control)
    {
        return Err(AppError::new("INVALID_REQUEST"));
    }
    // Block UNC shares (`\\server\share`), extended device paths (`\\?\`, `\\.\`), and `//host/share`
    // to prevent Windows SMB/Net-NTLMv2 credential leaks and device namespace access.
    if trimmed.starts_with(r"\\") || trimmed.starts_with("//") {
        return Err(AppError::new("FORBIDDEN"));
    }

    let path = PathBuf::from(trimmed);
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        let mut comps = path.components();
        match comps.next() {
            Some(Component::Prefix(pref)) => match pref.kind() {
                Prefix::Disk(_) => {}
                _ => return Err(AppError::new("FORBIDDEN")),
            },
            _ => return Err(AppError::new("INVALID_REQUEST")),
        }
        if !matches!(comps.next(), Some(Component::RootDir)) {
            return Err(AppError::new("INVALID_REQUEST"));
        }
        for comp in comps {
            match comp {
                Component::Normal(part) => {
                    let s = part.to_string_lossy();
                    if s.contains(':') {
                        return Err(AppError::new("FORBIDDEN"));
                    }
                }
                Component::ParentDir => return Err(AppError::new("FORBIDDEN")),
                Component::CurDir => {}
                _ => return Err(AppError::new("FORBIDDEN")),
            }
        }
    }
    #[cfg(not(windows))]
    {
        use std::path::Component;
        if !path.is_absolute() {
            return Err(AppError::new("INVALID_REQUEST"));
        }
        for comp in path.components() {
            if matches!(comp, Component::ParentDir) {
                return Err(AppError::new("FORBIDDEN"));
            }
        }
    }

    if is_forbidden_download_dir(&path) {
        return Err(AppError::new("FORBIDDEN"));
    }

    if let Ok(meta) = std::fs::symlink_metadata(&path) {
        if meta.file_type().is_symlink() || !meta.file_type().is_dir() {
            return Err(AppError::new("FORBIDDEN"));
        }
    }
    Ok(path)
}

pub fn ensure_safe_canonical_dir(dir: &Path) -> Result<PathBuf, AppError> {
    let meta = std::fs::symlink_metadata(dir).map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;
    if meta.file_type().is_symlink() || !meta.file_type().is_dir() {
        return Err(AppError::new("FORBIDDEN"));
    }
    let canon = std::fs::canonicalize(dir).map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;
    let clean = strip_win_verbatim(&canon);
    if clean.starts_with(r"\\") || clean.starts_with("//") || is_forbidden_download_dir(&canon) {
        return Err(AppError::new("FORBIDDEN"));
    }
    Ok(canon)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unc_traversal_and_sensitive_download_dirs() {
        assert!(validate_download_dir("").is_err());
        assert!(validate_download_dir(r"\\attacker.example\share").is_err());
        assert!(validate_download_dir("//attacker.example/share").is_err());
        assert!(validate_download_dir(r"\\?\C:\Users\Public").is_err());
        assert!(validate_download_dir("relative/downloads").is_err());
        #[cfg(windows)]
        {
            assert!(validate_download_dir(r"C:\Users\Public\..\Windows").is_err());
            assert!(validate_download_dir(r"C:\Windows\System32").is_err());
            assert!(validate_download_dir(r"C:\Users\Public\ads:stream").is_err());
            if let Some(local) = dirs::data_local_dir() {
                let bin_dir = local.join("MediaVault").join("bin");
                assert!(validate_download_dir(&bin_dir.display().to_string()).is_err());
            }
        }
    }

    #[test]
    fn db_crud_counts_and_pagination() {
        let dir = std::env::temp_dir().join(format!("mv_rust_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Database::open(&dir.join("app.db"), dir.join("dl")).unwrap();
        let now = Utc::now().to_rfc3339();
        let make = |id: &str, kind: &str, platform: &str| JobRecord {
            id: id.into(),
            url: "https://youtu.be/demo".into(),
            title: Some(format!("Demo {id}")),
            platform: platform.into(),
            kind: kind.into(),
            quality: "best".into(),
            container: if kind == "image" { "jpg".into() } else { "mp4".into() },
            audio_bitrate_kbps: None,
            allow_quality_fallback: false,
            image_index: None,
            status: "completed".into(),
            progress_percent: Some(100.0),
            downloaded_bytes: None,
            total_bytes: None,
            speed_bytes_per_second: None,
            eta_seconds: None,
            file_size_bytes: Some(1024),
            output_filename: Some("demo.mp4".into()),
            output_path: None,
            thumbnail_url: None,
            thumbnail_preview: None,
            error_code: None,
            error_message: None,
            file_available: false,
            created_at: now.clone(),
            updated_at: now.clone(),
            completed_at: Some(now.clone()),
            retry_of: None,
        };
        db.insert_job(&make("1", "video", "youtube"), 20).unwrap();
        db.insert_job(&make("2", "image", "instagram"), 20).unwrap();

        let page = db.list_jobs(5, 0, None, None, Some("image"), false).unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.counts.all, 2);
        assert_eq!(page.counts.video, 1);
        assert_eq!(page.counts.image, 1);

        let moved_path = dir.join("moved.mp4");
        std::fs::write(&moved_path, b"ok").unwrap();
        db.update_output_location("1", "moved.mp4", &moved_path.display().to_string())
            .unwrap();
        let updated = db.get_job("1").unwrap();
        assert_eq!(updated.output_filename.as_deref(), Some("moved.mp4"));
        assert_eq!(
            updated.output_path.as_deref(),
            Some(moved_path.to_string_lossy().as_ref())
        );
        assert!(updated.file_available);

        let _ = std::fs::remove_dir_all(dir);
    }
}
