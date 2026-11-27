#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio_player;
mod db;
mod engine;
mod errors;
mod extractors;
mod mp4_engine;
mod ui;
mod ui_kit;
mod url_policy;
mod video_player;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::db::Database;
use crate::engine::AppEngine;
use crate::extractors::PreviewCache;

const MAX_CRASH_LOG_BYTES: u64 = 256 * 1024;

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

fn load_app_icon() -> Option<egui::IconData> {
    let png_bytes = include_bytes!("../assets/icon-128.png");
    let img = image::load_from_memory(png_bytes).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    Some(egui::IconData {
        rgba: img.into_raw(),
        width: w,
        height: h,
    })
}

fn main() -> eframe::Result<()> {
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

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("failed to initialize Tokio runtime");

    let db = Arc::new(
        Database::open(&db_path, download_dir).expect("failed to initialize MediaVault SQLite"),
    );
    let previews = Arc::new(PreviewCache::new());
    let engine = Arc::new(AppEngine::new(
        Arc::clone(&db),
        Arc::clone(&previews),
        rt.handle().clone(),
    ));

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("MediaVault")
        .with_inner_size([1180.0, 760.0])
        .with_min_inner_size([920.0, 640.0]);
    if let Some(icon) = load_app_icon() {
        viewport = viewport.with_icon(Arc::new(icon));
    }

    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    let rt_guard = Arc::new(rt);
    let rt_for_app = Arc::clone(&rt_guard);

    eframe::run_native(
        "MediaVault",
        native_options,
        Box::new(move |cc| {
            Ok(Box::new(ui::MediaVaultApp::new(
                cc,
                db,
                previews,
                engine,
                db_path,
                rt_for_app,
            )))
        }),
    )
}
