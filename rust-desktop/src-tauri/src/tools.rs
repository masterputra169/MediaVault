use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use tokio::process::Command;

#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const BLOCKED_CHILD_ENV: &[&str] = &[
    "NODE_OPTIONS",
    "PYTHONSTARTUP",
    "PYTHONPATH",
    "PYTHONHOME",
    "PYTHONUSERBASE",
    "LD_PRELOAD",
    "DYLD_INSERT_LIBRARIES",
];

pub fn background_cmd(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.stdin(Stdio::null());
    cmd.kill_on_drop(true);
    for key in BLOCKED_CHILD_ENV {
        cmd.env_remove(key);
    }
    cmd.env("YTDLP_NO_PLUGINS", "1");
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

#[derive(Debug, Clone, Serialize)]
pub struct DependencyStatus {
    pub available: bool,
    pub version: Option<String>,
    pub raw_version: Option<String>,
    pub path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ToolPaths {
    pub yt_dlp: Option<PathBuf>,
    pub ffmpeg: Option<PathBuf>,
    pub ffprobe: Option<PathBuf>,
    pub gallery_dl: Option<PathBuf>,
    pub node: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HealthReport {
    pub status: String,
    pub app_version: String,
    pub yt_dlp: DependencyStatus,
    pub ffmpeg: DependencyStatus,
    pub ffprobe: DependencyStatus,
    pub gallery_dl: DependencyStatus,
    pub node: DependencyStatus,
    pub download_dir: String,
    pub db_path: String,
    pub storage_writable: bool,
}

fn search_in_path(exe: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    let cwd = std::env::current_dir().ok().and_then(|p| std::fs::canonicalize(p).ok());
    for dir in std::env::split_paths(&path_var) {
        if dir.as_os_str().is_empty() || !dir.is_absolute() {
            continue;
        }
        if let Some(ref cur) = cwd {
            if std::fs::canonicalize(&dir).ok().as_ref() == Some(cur) {
                continue;
            }
        }
        let candidate = dir.join(exe);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn find_in_winget(exe: &str) -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    let base = PathBuf::from(local).join("Microsoft").join("WinGet");
    let pkgs = base.join("Packages");
    let canon_pkgs = std::fs::canonicalize(&pkgs).ok();
    let link = base.join("Links").join(exe);
    if link.is_file() {
        if let (Ok(canon_link), Some(ref pkgs_root)) = (std::fs::canonicalize(&link), &canon_pkgs) {
            if canon_link.starts_with(pkgs_root) && canon_link.is_file() {
                return Some(canon_link);
            }
        }
    }
    if let Ok(entries) = std::fs::read_dir(&pkgs) {
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_symlink() || !ft.is_dir() {
                continue;
            }
            let p = entry.path();
            if let Some(found) = walk_for_file(&p, exe, 3) {
                return Some(found);
            }
        }
    }
    None
}

fn walk_for_file(dir: &Path, target: &str, depth: usize) -> Option<PathBuf> {
    if depth == 0 {
        return None;
    }
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if ft.is_symlink() {
            continue;
        }
        let p = entry.path();
        if ft.is_file() {
            if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                if name.eq_ignore_ascii_case(target) {
                    return Some(p);
                }
            }
        } else if ft.is_dir() {
            if let Some(found) = walk_for_file(&p, target, depth - 1) {
                return Some(found);
            }
        }
    }
    None
}

#[cfg(debug_assertions)]
fn find_in_workspace_venv(exe: &str) -> Option<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        roots.push(cwd.clone());
        if let Some(p1) = cwd.parent() {
            roots.push(p1.to_path_buf());
            if let Some(p2) = p1.parent() {
                roots.push(p2.to_path_buf());
            }
        }
    }
    for root in roots {
        let candidate = root
            .join("backend")
            .join(".venv")
            .join("Scripts")
            .join(exe);
        if std::fs::symlink_metadata(&candidate).is_ok_and(|m| m.file_type().is_file()) {
            return Some(candidate);
        }
    }
    None
}

#[cfg(not(debug_assertions))]
fn find_in_workspace_venv(_exe: &str) -> Option<PathBuf> {
    None
}

fn find_in_app_bin(base_name: &str, exe: &str) -> Option<PathBuf> {
    let base = dirs::data_local_dir()?.join("MediaVault").join("bin");
    // Prefer unpacked onedir bundle (e.g. bin\yt-dlp\yt-dlp.exe) for instant startup
    let onedir = base.join(base_name).join(exe);
    if std::fs::symlink_metadata(&onedir).is_ok_and(|m| m.file_type().is_file()) {
        return Some(onedir);
    }
    let flat = base.join(exe);
    std::fs::symlink_metadata(&flat)
        .is_ok_and(|m| m.file_type().is_file())
        .then_some(flat)
}

fn locate(base_name: &str) -> Option<PathBuf> {
    #[cfg(windows)]
    let exe = format!("{base_name}.exe");
    #[cfg(not(windows))]
    let exe = base_name.to_string();

    if let Some(p) = find_in_app_bin(base_name, &exe) {
        return Some(p);
    }
    if let Some(p) = search_in_path(&exe) {
        return Some(p);
    }
    if let Some(p) = find_in_workspace_venv(&exe) {
        return Some(p);
    }
    #[cfg(windows)]
    if let Some(p) = find_in_winget(&exe) {
        return Some(p);
    }
    #[cfg(windows)]
    if base_name == "node" {
        let std_node = PathBuf::from(r"C:\Program Files\nodejs\node.exe");
        if std_node.is_file() {
            return Some(std_node);
        }
    }
    None
}

pub fn discover_tools() -> ToolPaths {
    ToolPaths {
        yt_dlp: locate("yt-dlp"),
        ffmpeg: locate("ffmpeg"),
        ffprobe: locate("ffprobe"),
        gallery_dl: locate("gallery-dl"),
        node: locate("node"),
    }
}

fn parse_version_token(line: &str) -> Option<String> {
    for tok in line.split_whitespace() {
        let bytes = tok.as_bytes();
        let starts_digit = bytes.first().is_some_and(u8::is_ascii_digit);
        let starts_v_digit = bytes.len() >= 2
            && (bytes[0] == b'v' || bytes[0] == b'V')
            && bytes[1].is_ascii_digit();
        if starts_digit || starts_v_digit {
            let trimmed = tok.trim_start_matches(['v', 'V']);
            let clean = trimmed.split('-').next().unwrap_or(trimmed);
            if !clean.is_empty() {
                return Some(clean.to_string());
            }
        }
    }
    let t = line.trim();
    (!t.is_empty()).then(|| t.to_string())
}

async fn probe_version(path: Option<&PathBuf>, arg: &str) -> DependencyStatus {
    let Some(bin) = path else {
        return DependencyStatus {
            available: false,
            version: None,
            raw_version: None,
            path: None,
        };
    };
    let out = tokio::time::timeout(
        Duration::from_secs(5),
        background_cmd(bin).arg(arg).output(),
    )
    .await
    .ok()
    .and_then(Result::ok);
    match out {
        Some(o) if o.status.success() => {
            let line = String::from_utf8_lossy(&o.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            let version = parse_version_token(&line);
            let raw_version = (!line.is_empty()).then_some(line);
            DependencyStatus {
                available: true,
                version,
                raw_version,
                path: Some(bin.display().to_string()),
            }
        }
        _ => DependencyStatus {
            available: false,
            version: None,
            raw_version: None,
            path: Some(bin.display().to_string()),
        },
    }
}

pub async fn build_health(download_dir: &Path, db_path: &Path) -> (ToolPaths, HealthReport) {
    let tools = discover_tools();
    let (yt_dlp, ffmpeg, ffprobe, gallery_dl, node) = tokio::join!(
        probe_version(tools.yt_dlp.as_ref(), "--version"),
        probe_version(tools.ffmpeg.as_ref(), "-version"),
        probe_version(tools.ffprobe.as_ref(), "-version"),
        probe_version(tools.gallery_dl.as_ref(), "--version"),
        probe_version(tools.node.as_ref(), "--version"),
    );
    let writable = std::fs::create_dir_all(download_dir).is_ok();
    let ok = yt_dlp.available && ffmpeg.available && ffprobe.available && writable;
    let report = HealthReport {
        status: if ok { "ok".into() } else { "degraded".into() },
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        yt_dlp,
        ffmpeg,
        ffprobe,
        gallery_dl,
        node,
        download_dir: download_dir.display().to_string(),
        db_path: db_path.display().to_string(),
        storage_writable: writable,
    };
    (tools, report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tool_version_tokens_cleanly() {
        assert_eq!(
            parse_version_token("ffmpeg version 9.1-full_build-www.gyan.dev Copyright"),
            Some("9.1".into())
        );
        assert_eq!(parse_version_token("v25.1.0"), Some("25.1.0".into()));
        assert_eq!(parse_version_token("2026.11.23"), Some("2026.11.23".into()));
        assert_eq!(parse_version_token("1.32.21"), Some("1.32.21".into()));
    }
}
