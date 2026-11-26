use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use egui::{
    Align, Color32, ColorImage, CornerRadius, FontId, Frame, Layout, Margin, RichText, Stroke,
    TextureHandle, TextureOptions, Vec2,
};

use crate::audio_player::AudioPlayer;
use crate::db::{validate_download_dir, Database, JobCounts, JobRecord, Preferences};
use crate::engine::{
    move_job_file, open_file_in_explorer, open_folder_in_explorer, open_media_in_default_player,
    AppEngine, CreateDownloadRequest, MediaMetadata,
};
use crate::errors::AppError;
use crate::extractors::{decode_preview_rgba, PreviewCache, PreviewRgba};
use crate::mp4_engine::{probe_mp4, Mp4ProbeInfo};

const PLATFORMS: &[(&str, &str)] = &[
    ("", "Semua platform"),
    ("youtube", "YouTube"),
    ("instagram", "Instagram"),
    ("tiktok", "TikTok"),
    ("x", "X"),
    ("threads", "Threads"),
    ("facebook", "Facebook"),
    ("pinterest", "Pinterest"),
];

const MAX_ZIP_SLIDE_BYTES: u64 = 25 * 1024 * 1024;
const MAX_ALBUM_SLIDES: usize = 100;

enum AsyncMsg {
    AnalyzeDone(Result<MediaMetadata, AppError>),
    ThumbLoaded(String, Result<Arc<PreviewRgba>, ()>),
    ViewerReady(ViewerContent),
}

enum ViewerContent {
    Image {
        title: String,
        meta: String,
        path: String,
        rgba: PreviewRgba,
    },
    Album {
        title: String,
        meta: String,
        path: String,
        slide_names: Vec<String>,
        current_idx: usize,
        current_rgba: Option<PreviewRgba>,
    },
    Audio {
        title: String,
        meta: String,
        path: String,
    },
    Video {
        title: String,
        meta: String,
        path: String,
        probe: Option<Mp4ProbeInfo>,
        thumb_token: Option<String>,
    },
    Error(String),
}

struct ToastState {
    message: String,
    is_error: bool,
    expires_at: Instant,
}

pub struct MediaVaultApp {
    db: Arc<Database>,
    previews: Arc<PreviewCache>,
    engine: Arc<AppEngine>,
    db_path: PathBuf,
    rt: Arc<tokio::runtime::Runtime>,

    tx: mpsc::Sender<AsyncMsg>,
    rx: mpsc::Receiver<AsyncMsg>,

    prefs: Preferences,
    url_input: String,
    url_error: Option<String>,
    analyzing: bool,

    // Downloads list state
    tab: String,
    search: String,
    platform_filter: String,
    page: usize,
    page_size: usize,
    jobs: Vec<JobRecord>,
    total_jobs: i64,
    counts: JobCounts,
    last_refresh: Instant,

    // Analyze & download options modal
    analyze_modal: Option<AnalyzeModalState>,

    // Settings modal
    settings_open: bool,
    settings_draft: Preferences,

    // Delete confirmation modal
    delete_target: Option<JobRecord>,
    delete_file_checked: bool,

    // Completed media viewer modal
    viewer_open: bool,
    viewer_loading: bool,
    viewer_content: Option<ViewerContent>,
    viewer_texture: Option<TextureHandle>,
    audio_player: AudioPlayer,

    // GPU Texture cache for thumbnails
    textures: HashMap<String, TextureHandle>,
    loading_tokens: HashSet<String>,

    toast: Option<ToastState>,
    applied_theme: String,
}

struct AnalyzeModalState {
    meta: MediaMetadata,
    mode: String, // "video" | "audio" | "image"
    quality: String,
    allow_fallback: bool,
    audio_format: String,
    image_format: String,
    selected_slide: usize,
    save_dir: String,
}

impl MediaVaultApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        db: Arc<Database>,
        previews: Arc<PreviewCache>,
        engine: Arc<AppEngine>,
        db_path: PathBuf,
        rt: Arc<tokio::runtime::Runtime>,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let prefs = db.get_preferences().unwrap_or(Preferences {
            default_video_quality: "best".into(),
            default_audio_format: "m4a".into(),
            theme: "dark".into(),
            download_dir: dirs::download_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("MediaVault")
                .display()
                .to_string(),
        });

        apply_theme(&cc.egui_ctx, &prefs.theme);

        let mut app = Self {
            db,
            previews,
            engine,
            db_path,
            rt,
            tx,
            rx,
            settings_draft: prefs.clone(),
            applied_theme: prefs.theme.clone(),
            prefs,
            url_input: String::new(),
            url_error: None,
            analyzing: false,
            tab: "all".into(),
            search: String::new(),
            platform_filter: String::new(),
            page: 0,
            page_size: 5,
            jobs: Vec::new(),
            total_jobs: 0,
            counts: JobCounts {
                all: 0,
                active: 0,
                video: 0,
                audio: 0,
                image: 0,
            },
            last_refresh: Instant::now() - Duration::from_secs(10),
            analyze_modal: None,
            settings_open: false,
            delete_target: None,
            delete_file_checked: false,
            viewer_open: false,
            viewer_loading: false,
            viewer_content: None,
            viewer_texture: None,
            audio_player: AudioPlayer::new(),
            textures: HashMap::new(),
            loading_tokens: HashSet::new(),
            toast: None,
        };
        app.refresh_jobs();
        app
    }

    fn show_toast(&mut self, msg: impl Into<String>, is_error: bool) {
        self.toast = Some(ToastState {
            message: msg.into(),
            is_error,
            expires_at: Instant::now() + Duration::from_secs(4),
        });
    }

    fn refresh_jobs(&mut self) {
        let kind = match self.tab.as_str() {
            "video" | "audio" | "image" => Some(self.tab.as_str()),
            _ => None,
        };
        let active_only = self.tab == "active";
        let search = self.search.trim();
        let plat = self.platform_filter.as_str();
        if let Ok(mut page) = self.db.list_jobs(
            self.page_size as i64,
            (self.page * self.page_size) as i64,
            (!search.is_empty()).then_some(search),
            (!plat.is_empty()).then_some(plat),
            kind,
            active_only,
        ) {
            for job in &mut page.items {
                job.thumbnail_preview = self.previews.register(job.thumbnail_url.as_deref());
            }
            let total_pages = ((page.total.max(1) as usize) + self.page_size - 1) / self.page_size;
            if self.page >= total_pages && total_pages > 0 {
                self.page = total_pages - 1;
            }
            self.jobs = page.items;
            self.total_jobs = page.total;
            self.counts = page.counts;
        }
        self.last_refresh = Instant::now();
    }

    fn ensure_thumbnail(&mut self, ctx: &egui::Context, token: Option<&str>) {
        let Some(tok) = token else {
            return;
        };
        if self.textures.contains_key(tok) || self.loading_tokens.contains(tok) {
            return;
        }
        if let Some(cached) = self.previews.get_cached_rgba(tok) {
            self.upload_texture(ctx, tok, &cached);
            return;
        }
        self.loading_tokens.insert(tok.to_string());
        let previews = Arc::clone(&self.previews);
        let tx = self.tx.clone();
        let tok_owned = tok.to_string();
        let ctx_clone = ctx.clone();
        self.rt.spawn(async move {
            let res = previews.fetch_rgba(&tok_owned).await.map_err(|_| ());
            let _ = tx.send(AsyncMsg::ThumbLoaded(tok_owned, res));
            ctx_clone.request_repaint();
        });
    }

    fn upload_texture(&mut self, ctx: &egui::Context, key: &str, rgba: &PreviewRgba) {
        if self.textures.len() > 96 {
            self.textures.clear();
        }
        let img = ColorImage::from_rgba_unmultiplied(
            [rgba.width as usize, rgba.height as usize],
            &rgba.pixels,
        );
        let handle = ctx.load_texture(key, img, TextureOptions::LINEAR);
        self.textures.insert(key.to_string(), handle);
    }

    fn start_analyze(&mut self, ctx: &egui::Context) {
        let raw = self.url_input.trim().to_string();
        if raw.is_empty() {
            self.url_error = Some("Tempel tautan media terlebih dahulu.".into());
            return;
        }
        self.url_error = None;
        self.analyzing = true;
        let engine = Arc::clone(&self.engine);
        let tx = self.tx.clone();
        let ctx_clone = ctx.clone();
        self.rt.spawn(async move {
            let res = engine.analyze(&raw).await;
            let _ = tx.send(AsyncMsg::AnalyzeDone(res));
            ctx_clone.request_repaint();
        });
    }

    fn open_completed_preview(&mut self, ctx: &egui::Context, job: &JobRecord) {
        self.audio_player.stop();
        self.viewer_texture = None;
        self.viewer_content = None;
        self.viewer_open = true;
        self.viewer_loading = true;

        let Some(path_str) = job.output_path.clone() else {
            self.viewer_loading = false;
            self.viewer_content = Some(ViewerContent::Error("File tidak ditemukan di disk.".into()));
            return;
        };
        let title = job
            .title
            .clone()
            .or_else(|| job.output_filename.clone())
            .unwrap_or_else(|| "Pratinjau Media".into());
        let filename = job
            .output_filename
            .clone()
            .unwrap_or_else(|| "media".into());
        let container = job.container.to_uppercase();
        let size_txt = job.file_size_bytes.map(nice_bytes).unwrap_or_default();
        let meta = if size_txt.is_empty() {
            format!("{filename} · {container}")
        } else {
            format!("{filename} · {container} · {size_txt}")
        };
        let thumb_token = job.thumbnail_preview.clone();
        let tx = self.tx.clone();
        let ctx_clone = ctx.clone();

        self.rt.spawn_blocking(move || {
            let p = Path::new(&path_str);
            let Ok(fs_meta) = std::fs::symlink_metadata(p) else {
                let _ = tx.send(AsyncMsg::ViewerReady(ViewerContent::Error(
                    "File tidak ditemukan di disk.".into(),
                )));
                ctx_clone.request_repaint();
                return;
            };
            if !fs_meta.file_type().is_file() {
                let _ = tx.send(AsyncMsg::ViewerReady(ViewerContent::Error(
                    "File tidak valid.".into(),
                )));
                ctx_clone.request_repaint();
                return;
            }
            let ext = p
                .extension()
                .and_then(|s| s.to_str())
                .map(str::to_ascii_lowercase)
                .unwrap_or_default();

            let content = match ext.as_str() {
                "jpg" | "jpeg" | "png" | "webp" => match std::fs::read(p)
                    .ok()
                    .and_then(|b| decode_preview_rgba(&b, 1440).ok())
                {
                    Some(rgba) => ViewerContent::Image {
                        title,
                        meta,
                        path: path_str,
                        rgba,
                    },
                    None => ViewerContent::Error("Gambar tidak dapat didekode.".into()),
                },
                "zip" => match load_zip_slide_names(p) {
                    Ok(names) if !names.is_empty() => {
                        let first_rgba = load_zip_slide_rgba(p, &names[0]).ok();
                        ViewerContent::Album {
                            title,
                            meta: format!("{meta} · {} foto", names.len()),
                            path: path_str,
                            slide_names: names,
                            current_idx: 0,
                            current_rgba: first_rgba,
                        }
                    }
                    _ => ViewerContent::Error("Arsip ZIP tidak berisi foto yang valid.".into()),
                },
                "m4a" | "wav" | "mp3" => ViewerContent::Audio {
                    title,
                    meta,
                    path: path_str,
                },
                "mp4" => {
                    let probe = probe_mp4(p).ok();
                    ViewerContent::Video {
                        title,
                        meta,
                        path: path_str,
                        probe,
                        thumb_token,
                    }
                }
                _ => ViewerContent::Error("Format file ini belum mendukung pratinjau.".into()),
            };
            let _ = tx.send(AsyncMsg::ViewerReady(content));
            ctx_clone.request_repaint();
        });
    }

    fn switch_album_slide(&mut self, ctx: &egui::Context, delta: isize) {
        let Some(ViewerContent::Album {
            path,
            slide_names,
            current_idx,
            current_rgba,
            ..
        }) = self.viewer_content.as_mut()
        else {
            return;
        };
        if slide_names.is_empty() {
            return;
        }
        let next = (*current_idx as isize + delta).clamp(0, (slide_names.len() - 1) as isize) as usize;
        if next == *current_idx {
            return;
        }
        *current_idx = next;
        if let Ok(rgba) = load_zip_slide_rgba(Path::new(path), &slide_names[next]) {
            let img = ColorImage::from_rgba_unmultiplied(
                [rgba.width as usize, rgba.height as usize],
                &rgba.pixels,
            );
            self.viewer_texture = Some(ctx.load_texture("viewer_active", img, TextureOptions::LINEAR));
            *current_rgba = Some(rgba);
        }
    }

    fn poll_async(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                AsyncMsg::AnalyzeDone(res) => {
                    self.analyzing = false;
                    match res {
                        Ok(meta) => {
                            let mode = if !meta.available_qualities.is_empty() {
                                "video"
                            } else if !meta.available_image_formats.is_empty() {
                                "image"
                            } else {
                                "audio"
                            }
                            .to_string();
                            let quality = if meta
                                .available_qualities
                                .contains(&self.prefs.default_video_quality)
                            {
                                self.prefs.default_video_quality.clone()
                            } else {
                                meta.available_qualities
                                    .first()
                                    .cloned()
                                    .unwrap_or_else(|| "best".into())
                            };
                            let audio_format = if meta
                                .available_audio_formats
                                .contains(&self.prefs.default_audio_format)
                            {
                                self.prefs.default_audio_format.clone()
                            } else {
                                meta.available_audio_formats
                                    .first()
                                    .cloned()
                                    .unwrap_or_else(|| "m4a".into())
                            };
                            let image_format = meta
                                .available_image_formats
                                .first()
                                .cloned()
                                .unwrap_or_else(|| "jpg".into());
                            self.analyze_modal = Some(AnalyzeModalState {
                                meta,
                                mode,
                                quality,
                                allow_fallback: true,
                                audio_format,
                                image_format,
                                selected_slide: 1,
                                save_dir: self.prefs.download_dir.clone(),
                            });
                        }
                        Err(err) => {
                            self.url_error = Some(err.to_string());
                        }
                    }
                }
                AsyncMsg::ThumbLoaded(tok, res) => {
                    self.loading_tokens.remove(&tok);
                    if let Ok(rgba) = res {
                        self.upload_texture(ctx, &tok, &rgba);
                    }
                }
                AsyncMsg::ViewerReady(content) => {
                    self.viewer_loading = false;
                    match &content {
                        ViewerContent::Image { rgba, .. } => {
                            let img = ColorImage::from_rgba_unmultiplied(
                                [rgba.width as usize, rgba.height as usize],
                                &rgba.pixels,
                            );
                            self.viewer_texture =
                                Some(ctx.load_texture("viewer_active", img, TextureOptions::LINEAR));
                        }
                        ViewerContent::Album {
                            current_rgba: Some(rgba),
                            ..
                        } => {
                            let img = ColorImage::from_rgba_unmultiplied(
                                [rgba.width as usize, rgba.height as usize],
                                &rgba.pixels,
                            );
                            self.viewer_texture =
                                Some(ctx.load_texture("viewer_active", img, TextureOptions::LINEAR));
                        }
                        _ => {}
                    }
                    self.viewer_content = Some(content);
                }
            }
        }
    }
}

impl eframe::App for MediaVaultApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_async(ctx);

        if self.applied_theme != self.prefs.theme {
            apply_theme(ctx, &self.prefs.theme);
            self.applied_theme = self.prefs.theme.clone();
        }

        // Poll DB smoothly when active downloads exist (350ms), gently when idle (5s)
        let poll_interval = if self.counts.active > 0 {
            Duration::from_millis(350)
        } else {
            Duration::from_secs(5)
        };
        if self.last_refresh.elapsed() >= poll_interval {
            self.refresh_jobs();
        }
        if self.counts.active > 0 || self.analyzing || self.viewer_loading || self.audio_player.is_active() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }

        if let Some(t) = &self.toast {
            if Instant::now() >= t.expires_at {
                self.toast = None;
            }
        }

        let dark = ctx.style().visuals.dark_mode;
        let palette = Palette::for_mode(dark);

        // === TOP HEADER BAR ===
        egui::TopBottomPanel::top("header")
            .frame(
                Frame::new()
                    .fill(palette.surface)
                    .inner_margin(Margin::symmetric(18, 12))
                    .stroke(Stroke::new(1.0, palette.border)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("MediaVault.")
                            .size(18.0)
                            .strong()
                            .color(palette.text),
                    );
                    pill_badge(ui, "100% Pure Rust", palette.accent, palette.accent_bg);

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("⚙ Pengaturan").clicked() {
                            self.settings_draft = self.prefs.clone();
                            self.settings_open = true;
                        }
                        if ui.button("📁 Folder Unduhan").clicked() {
                            if let Err(e) = open_folder_in_explorer(&self.prefs.download_dir) {
                                self.show_toast(e.to_string(), true);
                            }
                        }
                        pill_badge(ui, "✓ Siap · Zero External Deps", palette.good, palette.good_bg);
                    });
                });
            });

        // === BOTTOM FOOTER ===
        egui::TopBottomPanel::bottom("footer")
            .frame(
                Frame::new()
                    .fill(palette.surface)
                    .inner_margin(Margin::symmetric(18, 7))
                    .stroke(Stroke::new(1.0, palette.border)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("MediaVault Desktop v0.2.0 · Native Rust + SQLite · Lokal & Privat")
                            .size(11.5)
                            .color(palette.muted),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("Folder: {}", self.prefs.download_dir))
                                .size(11.5)
                                .color(palette.muted),
                        );
                    });
                });
            });

        // === MAIN BODY ===
        egui::CentralPanel::default()
            .frame(Frame::new().fill(palette.bg).inner_margin(Margin::same(16)))
            .show(ctx, |ui| {
                // 1. URL ANALYZER PANEL
                card_frame(&palette).show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new("Unduh Video, Audio & Foto")
                                .size(14.5)
                                .strong()
                                .color(palette.text),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            for name in [
                                "Pinterest",
                                "Facebook",
                                "Threads",
                                "X",
                                "TikTok",
                                "Instagram",
                                "YouTube",
                            ] {
                                pill_badge(ui, name, palette.muted, palette.inset);
                            }
                        });
                    });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let avail = (ui.available_width() - 255.0).max(200.0);
                        let resp = ui.add_sized(
                            [avail, 34.0],
                            egui::TextEdit::singleline(&mut self.url_input)
                                .hint_text("Tempel tautan video, reel, foto, atau album publik di sini…")
                                .margin(Margin::symmetric(10, 8)),
                        );
                        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !self.analyzing {
                            self.start_analyze(ctx);
                        }
                        if !self.url_input.is_empty() && ui.add_sized([28.0, 34.0], egui::Button::new("×")).clicked() {
                            self.url_input.clear();
                            self.url_error = None;
                        }
                        if ui.add_sized([74.0, 34.0], egui::Button::new("Tempel")).clicked() {
                            if let Ok(mut cb) = arboard::Clipboard::new() {
                                if let Ok(text) = cb.get_text() {
                                    self.url_input = text.trim().to_string();
                                    self.url_error = None;
                                }
                            }
                        }
                        let btn_label = if self.analyzing {
                            "Menganalisis…"
                        } else {
                            "Analisis URL"
                        };
                        let analyze_btn = egui::Button::new(
                            RichText::new(btn_label).strong().color(Color32::WHITE),
                        )
                        .fill(palette.accent)
                        .corner_radius(CornerRadius::same(6));
                        if ui
                            .add_enabled(!self.analyzing, |ui: &mut egui::Ui| {
                                ui.add_sized([124.0, 34.0], analyze_btn)
                            })
                            .clicked()
                        {
                            self.start_analyze(ctx);
                        }
                    });

                    if let Some(err) = &self.url_error {
                        ui.add_space(6.0);
                        ui.label(RichText::new(format!("⚠ {err}")).color(palette.error).size(12.5));
                    }
                });

                ui.add_space(12.0);

                // 2. DOWNLOADS LIST PANEL
                card_frame(&palette).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("Daftar unduhan")
                                .size(15.0)
                                .strong()
                                .color(palette.text),
                        );
                        if self.counts.active > 0 {
                            pill_badge(
                                ui,
                                &format!("● {} berjalan", self.counts.active),
                                palette.accent,
                                palette.accent_bg,
                            );
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let prev_plat = self.platform_filter.clone();
                            egui::ComboBox::from_id_salt("platform_filter")
                                .selected_text(platform_label(&self.platform_filter))
                                .width(135.0)
                                .show_ui(ui, |ui| {
                                    for &(k, v) in PLATFORMS {
                                        ui.selectable_value(&mut self.platform_filter, k.into(), v);
                                    }
                                });
                            if self.platform_filter != prev_plat {
                                self.page = 0;
                                self.refresh_jobs();
                            }

                            let prev_search = self.search.clone();
                            ui.add_sized(
                                [180.0, 26.0],
                                egui::TextEdit::singleline(&mut self.search)
                                    .hint_text("Cari judul media…"),
                            );
                            if self.search != prev_search {
                                self.page = 0;
                                self.refresh_jobs();
                            }
                        });
                    });

                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        let tabs = [
                            ("all", format!("Semua ({})", self.counts.all)),
                            ("active", format!("Aktif ({})", self.counts.active)),
                            ("video", format!("Video ({})", self.counts.video)),
                            ("audio", format!("Audio ({})", self.counts.audio)),
                            ("image", format!("Foto ({})", self.counts.image)),
                        ];
                        for (key, label) in tabs {
                            let selected = self.tab == key;
                            if ui.selectable_label(selected, label).clicked() && !selected {
                                self.tab = key.into();
                                self.page = 0;
                                self.refresh_jobs();
                            }
                        }
                    });

                    ui.separator();

                    // Preload visible row thumbnails
                    let tokens_to_load: Vec<Option<String>> =
                        self.jobs.iter().map(|j| j.thumbnail_preview.clone()).collect();
                    for tok in tokens_to_load {
                        self.ensure_thumbnail(ctx, tok.as_deref());
                    }

                    let list_height = (ui.available_height() - 40.0).max(160.0);
                    egui::ScrollArea::vertical()
                        .max_height(list_height)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if self.jobs.is_empty() {
                                ui.add_space(40.0);
                                ui.vertical_centered(|ui| {
                                    ui.label(
                                        RichText::new("Belum ada unduhan")
                                            .size(15.0)
                                            .strong()
                                            .color(palette.text),
                                    );
                                    ui.label(
                                        RichText::new("Tempel tautan video, audio, atau foto di atas lalu klik Analisis URL.")
                                            .size(12.5)
                                            .color(palette.muted),
                                    );
                                });
                            } else {
                                let jobs_snapshot = self.jobs.clone();
                                for job in jobs_snapshot {
                                    self.render_job_row(ui, ctx, &job, &palette);
                                    ui.separator();
                                }
                            }
                        });

                    // Pagination footer
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        let total_pages =
                            ((self.total_jobs.max(1) as usize) + self.page_size - 1) / self.page_size;
                        let start = if self.total_jobs == 0 {
                            0
                        } else {
                            self.page * self.page_size + 1
                        };
                        let end = (self.total_jobs as usize).min((self.page + 1) * self.page_size);
                        ui.label(
                            RichText::new(format!(
                                "Menampilkan {start} - {end} dari {} unduhan",
                                self.total_jobs
                            ))
                            .size(12.0)
                            .color(palette.muted),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui
                                .add_enabled(self.page + 1 < total_pages, egui::Button::new("»"))
                                .clicked()
                            {
                                self.page = total_pages.saturating_sub(1);
                                self.refresh_jobs();
                            }
                            if ui
                                .add_enabled(self.page + 1 < total_pages, egui::Button::new("›"))
                                .clicked()
                            {
                                self.page += 1;
                                self.refresh_jobs();
                            }
                            ui.label(format!("{} / {}", self.page + 1, total_pages));
                            if ui.add_enabled(self.page > 0, egui::Button::new("‹")).clicked() {
                                self.page = self.page.saturating_sub(1);
                                self.refresh_jobs();
                            }
                            if ui.add_enabled(self.page > 0, egui::Button::new("«")).clicked() {
                                self.page = 0;
                                self.refresh_jobs();
                            }
                            let prev_ps = self.page_size;
                            egui::ComboBox::from_id_salt("page_size")
                                .selected_text(format!("{} data", self.page_size))
                                .width(78.0)
                                .show_ui(ui, |ui| {
                                    for n in [4, 5, 10, 20] {
                                        ui.selectable_value(&mut self.page_size, n, format!("{n} data"));
                                    }
                                });
                            if self.page_size != prev_ps {
                                self.page = 0;
                                self.refresh_jobs();
                            }
                        });
                    });
                });
            });

        // === MODALS ===
        self.render_analyze_modal(ctx, &palette);
        self.render_viewer_modal(ctx, &palette);
        self.render_settings_modal(ctx, &palette);
        self.render_delete_modal(ctx, &palette);
        self.render_toast(ctx, &palette);
    }
}

impl MediaVaultApp {
    fn render_job_row(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        job: &JobRecord,
        palette: &Palette,
    ) {
        let is_active = matches!(job.status.as_str(), "queued" | "downloading" | "processing");
        let can_preview = job.status == "completed" && job.file_available;

        ui.horizontal(|ui| {
            // Thumbnail (clickable when completed)
            let thumb_size = Vec2::new(60.0, 40.0);
            let tex = job
                .thumbnail_preview
                .as_deref()
                .and_then(|t| self.textures.get(t));
            let thumb_resp = if let Some(handle) = tex {
                ui.add(
                    egui::Image::new((handle.id(), thumb_size))
                        .corner_radius(CornerRadius::same(5))
                        .sense(if can_preview {
                            egui::Sense::click()
                        } else {
                            egui::Sense::hover()
                        }),
                )
            } else {
                let (rect, resp) = ui.allocate_exact_size(
                    thumb_size,
                    if can_preview {
                        egui::Sense::click()
                    } else {
                        egui::Sense::hover()
                    },
                );
                ui.painter().rect_filled(rect, CornerRadius::same(5), palette.inset);
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    if job.kind == "image" { "🖼" } else if job.kind == "audio" { "♪" } else { "▶" },
                    FontId::proportional(14.0),
                    palette.muted,
                );
                resp
            };
            if can_preview && thumb_resp.clicked() {
                self.open_completed_preview(ctx, job);
            }

            // Middle info
            ui.vertical(|ui| {
                ui.horizontal_wrapped(|ui| {
                    let title = job
                        .title
                        .as_deref()
                        .or(job.output_filename.as_deref())
                        .unwrap_or(&job.platform);
                    let short_title: String = title.chars().take(54).collect();
                    let display = if title.chars().count() > 54 {
                        format!("{short_title}…")
                    } else {
                        short_title
                    };
                    ui.label(RichText::new(display).strong().color(palette.text));

                    let (st_text, st_col, st_bg) = status_style(&job.status, palette);
                    pill_badge(ui, st_text, st_col, st_bg);
                    pill_badge(ui, &kind_label(job), palette.muted, palette.inset);
                });

                if is_active {
                    let frac = (job.progress_percent.unwrap_or(0.0) / 100.0).clamp(0.0, 1.0) as f32;
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [200.0, 14.0],
                            egui::ProgressBar::new(frac).animate(true),
                        );
                        let mut parts = vec![format!("{}%", job.progress_percent.unwrap_or(0.0).round() as i64)];
                        if let Some(dl) = job.downloaded_bytes {
                            if let Some(tot) = job.total_bytes {
                                parts.push(format!("{} / {}", nice_bytes(dl), nice_bytes(tot)));
                            } else {
                                parts.push(nice_bytes(dl));
                            }
                        }
                        if let Some(spd) = job.speed_bytes_per_second.filter(|&s| s > 0.0) {
                            parts.push(format!("{}/s", nice_bytes(spd as i64)));
                        }
                        if let Some(eta) = job.eta_seconds {
                            parts.push(nice_duration(eta as f64));
                        }
                        ui.label(RichText::new(parts.join(" · ")).size(11.5).color(palette.muted));
                    });
                } else {
                    let mut meta = vec![platform_label(&job.platform).to_string()];
                    if let Some(sz) = job.file_size_bytes {
                        meta.push(nice_bytes(sz));
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(meta.join(" · ")).size(11.5).color(palette.muted));
                        if let Some(err) = &job.error_message {
                            ui.label(RichText::new(format!("· {err}")).size(11.5).color(palette.error));
                        } else if job.status == "completed" && !job.file_available {
                            ui.label(
                                RichText::new("· File tidak ada di disk")
                                    .size(11.5)
                                    .color(palette.error),
                            );
                        }
                    });
                }
            });

            // Right action buttons
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if is_active {
                    if ui.button("Batalkan").clicked() {
                        match self.engine.cancel(&job.id) {
                            Ok(()) => {
                                self.show_toast("Unduhan dibatalkan.", false);
                                self.refresh_jobs();
                            }
                            Err(e) => self.show_toast(e.to_string(), true),
                        }
                    }
                } else {
                    if ui
                        .button(RichText::new("Hapus").color(palette.error))
                        .clicked()
                    {
                        self.delete_target = Some(job.clone());
                        self.delete_file_checked = false;
                    }
                    if ui.button("Ulangi").clicked() {
                        let req = CreateDownloadRequest {
                            url: job.url.clone(),
                            kind: job.kind.clone(),
                            quality: job.quality.clone(),
                            container: if job.kind == "audio" && job.container == "mp3" {
                                "m4a".into()
                            } else {
                                job.container.clone()
                            },
                            allow_quality_fallback: job.allow_quality_fallback,
                            image_index: job.image_index,
                            output_dir: job
                                .output_path
                                .as_deref()
                                .and_then(|p| Path::new(p).parent())
                                .filter(|d| d.is_dir())
                                .map(|d| d.display().to_string()),
                        };
                        match self.engine.enqueue(req, Some(job.id.clone())) {
                            Ok(_) => {
                                self.show_toast("Unduhan diulang.", false);
                                self.page = 0;
                                self.refresh_jobs();
                            }
                            Err(e) => self.show_toast(e.to_string(), true),
                        }
                    }
                    if job.file_available {
                        if ui.button("Pindahkan").clicked() {
                            let start = job
                                .output_path
                                .as_deref()
                                .and_then(|p| Path::new(p).parent())
                                .map(Path::to_path_buf);
                            let mut dlg = rfd::FileDialog::new().set_title("Pilih folder tujuan baru");
                            if let Some(d) = start.filter(|d| d.is_dir()) {
                                dlg = dlg.set_directory(d);
                            }
                            if let Some(dest_dir) = dlg.pick_folder() {
                                match move_job_file(&self.db, &job.id, &dest_dir) {
                                    Ok(_) => {
                                        self.show_toast("File berhasil dipindahkan.", false);
                                        self.refresh_jobs();
                                    }
                                    Err(e) => self.show_toast(e.to_string(), true),
                                }
                            }
                        }
                        if ui
                            .button(RichText::new("Buka folder").color(palette.good))
                            .clicked()
                        {
                            if let Some(p) = job.output_path.as_deref() {
                                if let Err(e) = open_file_in_explorer(p) {
                                    self.show_toast(e.to_string(), true);
                                }
                            }
                        }
                        if can_preview {
                            if ui
                                .button(RichText::new("Pratinjau").strong().color(palette.accent))
                                .clicked()
                            {
                                self.open_completed_preview(ctx, job);
                            }
                        }
                    }
                }
            });
        });
    }

    fn render_analyze_modal(&mut self, ctx: &egui::Context, palette: &Palette) {
        let Some(mut modal) = self.analyze_modal.take() else {
            return;
        };

        // Preload modal preview thumbnail
        let preview_tok = if modal.mode == "image" && !modal.meta.images.is_empty() {
            modal
                .meta
                .images
                .get(modal.selected_slide.saturating_sub(1))
                .and_then(|i| i.preview_token.clone())
        } else {
            modal.meta.preview_token.clone()
        };
        self.ensure_thumbnail(ctx, preview_tok.as_deref());

        let mut keep_open = true;
        let mut start_download = false;

        egui::Window::new("Pratinjau & Opsi Unduhan")
            .open(&mut keep_open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .fixed_size(Vec2::new(720.0, 440.0))
            .show(ctx, |ui| {
                ui.columns(2, |cols| {
                    // Left column: preview thumbnail + metadata
                    cols[0].vertical(|ui| {
                        let box_size = Vec2::new(320.0, 195.0);
                        if let Some(tex) = preview_tok.as_deref().and_then(|t| self.textures.get(t)) {
                            let sz = fit_aspect(tex.size_vec2(), box_size);
                            ui.centered_and_justified(|ui| {
                                ui.add(
                                    egui::Image::new((tex.id(), sz))
                                        .corner_radius(CornerRadius::same(8)),
                                );
                            });
                        } else {
                            let (rect, _) = ui.allocate_exact_size(box_size, egui::Sense::hover());
                            ui.painter().rect_filled(rect, CornerRadius::same(8), palette.inset);
                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "Memuat pratinjau…",
                                FontId::proportional(13.0),
                                palette.muted,
                            );
                        }
                        ui.add_space(8.0);
                        pill_badge(
                            ui,
                            platform_label(&modal.meta.platform),
                            palette.accent,
                            palette.accent_bg,
                        );
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(
                                modal
                                    .meta
                                    .title
                                    .as_deref()
                                    .unwrap_or("Judul tidak tersedia"),
                            )
                            .size(14.0)
                            .strong(),
                        );
                        if let Some(up) = &modal.meta.uploader {
                            ui.label(RichText::new(up).size(12.0).color(palette.muted));
                        }
                        if let Some(dur) = modal.meta.duration_seconds {
                            ui.label(
                                RichText::new(format!("Durasi: {}", nice_duration(dur)))
                                    .size(12.0)
                                    .color(palette.muted),
                            );
                        }
                    });

                    // Right column: options
                    cols[1].vertical(|ui| {
                        ui.label(RichText::new("Jenis keluaran").strong());
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(
                                    !modal.meta.available_qualities.is_empty(),
                                    egui::SelectableLabel::new(modal.mode == "video", "Video"),
                                )
                                .clicked()
                            {
                                modal.mode = "video".into();
                            }
                            if ui
                                .add_enabled(
                                    !modal.meta.available_audio_formats.is_empty(),
                                    egui::SelectableLabel::new(modal.mode == "audio", "Audio"),
                                )
                                .clicked()
                            {
                                modal.mode = "audio".into();
                            }
                            if ui
                                .add_enabled(
                                    !modal.meta.available_image_formats.is_empty(),
                                    egui::SelectableLabel::new(modal.mode == "image", "Foto"),
                                )
                                .clicked()
                            {
                                modal.mode = "image".into();
                            }
                        });

                        ui.add_space(10.0);
                        match modal.mode.as_str() {
                            "video" => {
                                ui.horizontal(|ui| {
                                    ui.label("Kualitas:");
                                    egui::ComboBox::from_id_salt("dl_quality")
                                        .selected_text(quality_label(&modal.quality))
                                        .show_ui(ui, |ui| {
                                            for q in &modal.meta.available_qualities {
                                                ui.selectable_value(
                                                    &mut modal.quality,
                                                    q.clone(),
                                                    quality_label(q),
                                                );
                                            }
                                        });
                                });
                                ui.label(
                                    RichText::new("Format: MP4 (H.264 + AAC universal · Pure-Rust Muxer)")
                                        .size(11.5)
                                        .color(palette.muted),
                                );
                                ui.checkbox(
                                    &mut modal.allow_fallback,
                                    "Izinkan resolusi lebih rendah jika pilihan tidak ada",
                                );
                            }
                            "audio" => {
                                ui.horizontal(|ui| {
                                    ui.label("Format audio:");
                                    egui::ComboBox::from_id_salt("dl_audio_fmt")
                                        .selected_text(audio_format_label(&modal.audio_format))
                                        .show_ui(ui, |ui| {
                                            for f in &modal.meta.available_audio_formats {
                                                ui.selectable_value(
                                                    &mut modal.audio_format,
                                                    f.clone(),
                                                    audio_format_label(f),
                                                );
                                            }
                                        });
                                });
                            }
                            "image" => {
                                ui.horizontal(|ui| {
                                    ui.label("Format foto:");
                                    let count = modal.meta.images.len();
                                    egui::ComboBox::from_id_salt("dl_img_fmt")
                                        .selected_text(image_format_label(&modal.image_format, count))
                                        .show_ui(ui, |ui| {
                                            for f in &modal.meta.available_image_formats {
                                                ui.selectable_value(
                                                    &mut modal.image_format,
                                                    f.clone(),
                                                    image_format_label(f, count),
                                                );
                                            }
                                        });
                                });
                                if modal.image_format != "zip" && modal.meta.images.len() > 1 {
                                    ui.add_space(6.0);
                                    ui.horizontal(|ui| {
                                        ui.label(format!(
                                            "Slide foto ({} / {}):",
                                            modal.selected_slide,
                                            modal.meta.images.len()
                                        ));
                                        if ui
                                            .add_enabled(modal.selected_slide > 1, egui::Button::new("‹"))
                                            .clicked()
                                        {
                                            modal.selected_slide -= 1;
                                        }
                                        if ui
                                            .add_enabled(
                                                modal.selected_slide < modal.meta.images.len(),
                                                egui::Button::new("›"),
                                            )
                                            .clicked()
                                        {
                                            modal.selected_slide += 1;
                                        }
                                    });
                                }
                            }
                            _ => {}
                        }

                        ui.add_space(12.0);
                        ui.label(RichText::new("Simpan ke folder:").size(12.0));
                        ui.horizontal(|ui| {
                            ui.add_sized(
                                [230.0, 26.0],
                                egui::TextEdit::singleline(&mut modal.save_dir).interactive(false),
                            );
                            if ui.button("Ubah…").clicked() {
                                let mut dlg =
                                    rfd::FileDialog::new().set_title("Pilih folder simpan unduhan");
                                let cur = PathBuf::from(&modal.save_dir);
                                if cur.is_dir() {
                                    dlg = dlg.set_directory(cur);
                                }
                                if let Some(picked) = dlg.pick_folder() {
                                    match validate_download_dir(&picked.display().to_string()) {
                                        Ok(valid) => modal.save_dir = valid.display().to_string(),
                                        Err(e) => self.show_toast(e.to_string(), true),
                                    }
                                }
                            }
                        });

                        ui.add_space(16.0);
                        let dl_btn = egui::Button::new(
                            RichText::new("⬇ Unduh sekarang").strong().color(Color32::WHITE),
                        )
                        .fill(palette.accent)
                        .corner_radius(CornerRadius::same(6));
                        if ui.add_sized([ui.available_width(), 36.0], dl_btn).clicked() {
                            start_download = true;
                        }
                    });
                });
            });

        if start_download {
            let container = match modal.mode.as_str() {
                "video" => "mp4".into(),
                "audio" => modal.audio_format.clone(),
                _ => modal.image_format.clone(),
            };
            let req = CreateDownloadRequest {
                url: modal.meta.url.clone(),
                kind: modal.mode.clone(),
                quality: if modal.mode == "video" {
                    modal.quality.clone()
                } else {
                    "best".into()
                },
                container,
                allow_quality_fallback: modal.allow_fallback,
                image_index: if modal.mode == "image" && modal.image_format != "zip" {
                    Some(modal.selected_slide as i64)
                } else {
                    None
                },
                output_dir: Some(modal.save_dir.clone()),
            };
            match self.engine.enqueue(req, None) {
                Ok(_) => {
                    self.url_input.clear();
                    self.page = 0;
                    self.refresh_jobs();
                    self.show_toast("Unduhan ditambahkan ke antrean.", false);
                }
                Err(e) => self.show_toast(e.to_string(), true),
            }
        } else if keep_open {
            self.analyze_modal = Some(modal);
        }
    }

    fn render_viewer_modal(&mut self, ctx: &egui::Context, palette: &Palette) {
        if !self.viewer_open {
            return;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
            self.switch_album_slide(ctx, -1);
        } else if ctx.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
            self.switch_album_slide(ctx, 1);
        }

        let mut open = self.viewer_open;
        let mut delta_slide = 0isize;
        let mut open_explorer_path: Option<String> = None;
        let mut open_player_path: Option<String> = None;
        let mut play_audio_path: Option<String> = None;
        let mut toggle_audio = false;
        let mut stop_audio = false;

        let video_thumb_tok = match &self.viewer_content {
            Some(ViewerContent::Video { thumb_token, .. }) => thumb_token.clone(),
            _ => None,
        };
        self.ensure_thumbnail(ctx, video_thumb_tok.as_deref());

        egui::Window::new("Pratinjau Media Terunduh")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .default_size(Vec2::new(720.0, 520.0))
            .show(ctx, |ui| {
                if self.viewer_loading {
                    ui.centered_and_justified(|ui| {
                        ui.spinner();
                    });
                    return;
                }
                match &self.viewer_content {
                    Some(ViewerContent::Image { title, meta, path, .. }) => {
                        ui.label(RichText::new(title).strong().size(14.5));
                        ui.label(RichText::new(meta).size(11.5).color(palette.muted));
                        ui.add_space(8.0);
                        if let Some(tex) = &self.viewer_texture {
                            let sz = fit_aspect(tex.size_vec2(), Vec2::new(680.0, 390.0));
                            ui.vertical_centered(|ui| {
                                ui.add(
                                    egui::Image::new((tex.id(), sz))
                                        .corner_radius(CornerRadius::same(6)),
                                );
                            });
                        }
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.button("📁 Buka folder").clicked() {
                                open_explorer_path = Some(path.clone());
                            }
                            if ui.button("🖼 Buka di Foto Windows").clicked() {
                                open_player_path = Some(path.clone());
                            }
                        });
                    }
                    Some(ViewerContent::Album {
                        title,
                        meta,
                        path,
                        slide_names,
                        current_idx,
                        ..
                    }) => {
                        ui.label(RichText::new(title).strong().size(14.5));
                        ui.label(RichText::new(meta).size(11.5).color(palette.muted));
                        ui.add_space(8.0);
                        if let Some(tex) = &self.viewer_texture {
                            let sz = fit_aspect(tex.size_vec2(), Vec2::new(680.0, 360.0));
                            ui.vertical_centered(|ui| {
                                ui.add(
                                    egui::Image::new((tex.id(), sz))
                                        .corner_radius(CornerRadius::same(6)),
                                );
                            });
                        }
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(*current_idx > 0, egui::Button::new("‹ Sebelumnya"))
                                .clicked()
                            {
                                delta_slide = -1;
                            }
                            ui.label(format!("{} / {}", current_idx + 1, slide_names.len()));
                            if ui
                                .add_enabled(
                                    *current_idx + 1 < slide_names.len(),
                                    egui::Button::new("Berikutnya ›"),
                                )
                                .clicked()
                            {
                                delta_slide = 1;
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button("📁 Buka folder").clicked() {
                                    open_explorer_path = Some(path.clone());
                                }
                            });
                        });
                    }
                    Some(ViewerContent::Audio { title, meta, path }) => {
                        ui.label(RichText::new(title).strong().size(15.0));
                        ui.label(RichText::new(meta).size(12.0).color(palette.muted));
                        ui.add_space(24.0);
                        ui.vertical_centered(|ui| {
                            ui.label(RichText::new("♪ Pemutar Audio Native Rust (Symphonia + Rodio)").size(14.0).color(palette.accent));
                            ui.add_space(12.0);
                            let playing_this = self.audio_player.loaded_path() == Some(path.as_str())
                                && self.audio_player.is_active();
                            ui.horizontal(|ui| {
                                if !playing_this {
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                RichText::new("▶ Putar Audio").strong().color(Color32::WHITE),
                                            )
                                            .fill(palette.accent),
                                        )
                                        .clicked()
                                    {
                                        play_audio_path = Some(path.clone());
                                    }
                                } else {
                                    let lbl = if self.audio_player.is_paused() {
                                        "▶ Lanjutkan"
                                    } else {
                                        "⏸ Jeda"
                                    };
                                    if ui.button(lbl).clicked() {
                                        toggle_audio = true;
                                    }
                                    if ui.button("⏹ Berhenti").clicked() {
                                        stop_audio = true;
                                    }
                                }
                                if ui.button("📁 Buka folder").clicked() {
                                    open_explorer_path = Some(path.clone());
                                }
                            });
                        });
                    }
                    Some(ViewerContent::Video {
                        title,
                        meta,
                        path,
                        probe,
                        thumb_token,
                    }) => {
                        ui.label(RichText::new(title).strong().size(15.0));
                        ui.label(RichText::new(meta).size(12.0).color(palette.muted));
                        ui.add_space(10.0);
                        if let Some(tex) = thumb_token.as_deref().and_then(|t| self.textures.get(t)) {
                            let sz = fit_aspect(tex.size_vec2(), Vec2::new(640.0, 290.0));
                            ui.vertical_centered(|ui| {
                                ui.add(
                                    egui::Image::new((tex.id(), sz))
                                        .corner_radius(CornerRadius::same(8)),
                                );
                            });
                        }
                        if let Some(info) = probe {
                            ui.add_space(8.0);
                            let mut specs = Vec::new();
                            if let (Some(w), Some(h)) = (info.width, info.height) {
                                specs.push(format!("Resolusi: {w}×{h}"));
                            }
                            if let Some(d) = info.duration_seconds {
                                specs.push(format!("Durasi: {}", nice_duration(d)));
                            }
                            if let Some(vc) = &info.video_codec {
                                specs.push(format!("Video: {vc}"));
                            }
                            if let Some(ac) = &info.audio_codec {
                                specs.push(format!("Audio: {ac}"));
                            }
                            ui.label(RichText::new(specs.join(" · ")).size(12.5).color(palette.good));
                        }
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new("▶ Putar di Pemutar Video").strong().color(Color32::WHITE),
                                    )
                                    .fill(palette.accent),
                                )
                                .clicked()
                            {
                                open_player_path = Some(path.clone());
                            }
                            if ui.button("📁 Buka folder").clicked() {
                                open_explorer_path = Some(path.clone());
                            }
                        });
                    }
                    Some(ViewerContent::Error(msg)) => {
                        ui.label(RichText::new(format!("⚠ {msg}")).color(palette.error));
                    }
                    None => {}
                }
            });

        if delta_slide != 0 {
            self.switch_album_slide(ctx, delta_slide);
        }
        if let Some(p) = open_explorer_path {
            if let Err(e) = open_file_in_explorer(&p) {
                self.show_toast(e.to_string(), true);
            }
        }
        if let Some(p) = open_player_path {
            if let Err(e) = open_media_in_default_player(&p) {
                self.show_toast(e.to_string(), true);
            }
        }
        if let Some(p) = play_audio_path {
            if let Err(e) = self.audio_player.play_file(Path::new(&p)) {
                self.show_toast(e, true);
            }
        }
        if toggle_audio {
            self.audio_player.toggle_pause();
        }
        if stop_audio {
            self.audio_player.stop();
        }
        if !open {
            self.viewer_open = false;
            self.viewer_content = None;
            self.viewer_texture = None;
            self.audio_player.stop();
        }
    }

    fn render_settings_modal(&mut self, ctx: &egui::Context, palette: &Palette) {
        if !self.settings_open {
            return;
        }
        let mut open = self.settings_open;
        let mut save_clicked = false;

        egui::Window::new("Pengaturan & Engine Pure-Rust")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .fixed_size(Vec2::new(660.0, 340.0))
            .show(ctx, |ui| {
                ui.columns(2, |cols| {
                    cols[0].vertical(|ui| {
                        ui.label(RichText::new("Preferensi").strong().size(14.0));
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.label("Kualitas video:");
                            egui::ComboBox::from_id_salt("pref_q")
                                .selected_text(quality_label(&self.settings_draft.default_video_quality))
                                .show_ui(ui, |ui| {
                                    for q in ["best", "1080", "720", "480", "360"] {
                                        ui.selectable_value(
                                            &mut self.settings_draft.default_video_quality,
                                            q.into(),
                                            quality_label(q),
                                        );
                                    }
                                });
                        });
                        ui.horizontal(|ui| {
                            ui.label("Format audio:");
                            egui::ComboBox::from_id_salt("pref_a")
                                .selected_text(audio_format_label(&self.settings_draft.default_audio_format))
                                .show_ui(ui, |ui| {
                                    for a in ["m4a", "wav"] {
                                        ui.selectable_value(
                                            &mut self.settings_draft.default_audio_format,
                                            a.into(),
                                            audio_format_label(a),
                                        );
                                    }
                                });
                        });
                        ui.horizontal(|ui| {
                            ui.label("Tema:");
                            egui::ComboBox::from_id_salt("pref_t")
                                .selected_text(theme_label(&self.settings_draft.theme))
                                .show_ui(ui, |ui| {
                                    for t in ["dark", "light", "system"] {
                                        ui.selectable_value(
                                            &mut self.settings_draft.theme,
                                            t.into(),
                                            theme_label(t),
                                        );
                                    }
                                });
                        });
                        ui.add_space(6.0);
                        ui.label("Folder simpan unduhan:");
                        ui.horizontal(|ui| {
                            ui.add_sized(
                                [220.0, 26.0],
                                egui::TextEdit::singleline(&mut self.settings_draft.download_dir),
                            );
                            if ui.button("Pilih…").clicked() {
                                let mut dlg = rfd::FileDialog::new()
                                    .set_title("Pilih folder simpan default");
                                let cur = PathBuf::from(&self.settings_draft.download_dir);
                                if cur.is_dir() {
                                    dlg = dlg.set_directory(cur);
                                }
                                if let Some(picked) = dlg.pick_folder() {
                                    self.settings_draft.download_dir = picked.display().to_string();
                                }
                            }
                        });
                        ui.add_space(12.0);
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("Simpan preferensi").strong().color(Color32::WHITE),
                                )
                                .fill(palette.accent),
                            )
                            .clicked()
                        {
                            save_clicked = true;
                        }
                    });

                    cols[1].vertical(|ui| {
                        ui.label(RichText::new("Status Engine Native Rust").strong().size(14.0));
                        ui.add_space(8.0);
                        for (k, v) in [
                            ("Core & GUI", "✓ 100% Rust (eframe/egui)"),
                            ("Ekstraktor 7 Platform", "✓ Built-in Pure Rust"),
                            ("MP4/fMP4 Muxer & Probe", "✓ Built-in Pure Rust"),
                            ("Audio M4A/WAV & Player", "✓ Symphonia + Rodio"),
                            ("Dependensi Eksternal", "✓ 0 (Tanpa yt-dlp/FFmpeg/Node)"),
                        ] {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(k).color(palette.muted).size(12.0));
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.label(RichText::new(v).color(palette.good).size(12.0).strong());
                                });
                            });
                        }
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new(format!("Database SQLite:\n{}", self.db_path.display()))
                                .size(11.0)
                                .color(palette.muted),
                        );
                    });
                });
            });

        if save_clicked {
            match self.db.save_preferences(&self.settings_draft) {
                Ok(saved) => {
                    self.prefs = saved;
                    open = false;
                    self.show_toast("Preferensi disimpan.", false);
                }
                Err(e) => self.show_toast(e.to_string(), true),
            }
        }
        self.settings_open = open;
    }

    fn render_delete_modal(&mut self, ctx: &egui::Context, palette: &Palette) {
        let Some(target) = self.delete_target.clone() else {
            return;
        };
        let mut open = true;
        let mut confirm = false;
        let mut cancel = false;

        egui::Window::new("Hapus riwayat unduhan")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .fixed_size(Vec2::new(400.0, 150.0))
            .show(ctx, |ui| {
                let name = target
                    .title
                    .as_deref()
                    .or(target.output_filename.as_deref())
                    .unwrap_or(&target.platform);
                ui.label(RichText::new(name).color(palette.muted));
                if target.file_available {
                    ui.add_space(6.0);
                    ui.checkbox(
                        &mut self.delete_file_checked,
                        "Hapus juga file di komputer secara permanen",
                    );
                }
                ui.add_space(12.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add(
                            egui::Button::new(RichText::new("Hapus").color(Color32::WHITE))
                                .fill(palette.error),
                        )
                        .clicked()
                    {
                        confirm = true;
                    }
                    if ui.button("Kembali").clicked() {
                        cancel = true;
                    }
                });
            });

        if confirm {
            match self.db.delete_job(&target.id) {
                Ok(deleted) => {
                    if self.delete_file_checked {
                        if let Some(path_str) = deleted.output_path {
                            let p = Path::new(&path_str);
                            if std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_file()) {
                                let _ = std::fs::remove_file(p);
                            }
                        }
                    }
                    self.delete_target = None;
                    self.refresh_jobs();
                    self.show_toast("Catatan unduhan dihapus.", false);
                }
                Err(e) => self.show_toast(e.to_string(), true),
            }
        } else if cancel || !open {
            self.delete_target = None;
        }
    }

    fn render_toast(&mut self, ctx: &egui::Context, palette: &Palette) {
        let Some(t) = &self.toast else {
            return;
        };
        let msg = t.message.clone();
        let is_err = t.is_error;
        let mut dismiss = false;

        egui::Area::new(egui::Id::new("toast_overlay"))
            .anchor(egui::Align2::RIGHT_BOTTOM, Vec2::new(-18.0, -40.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                Frame::new()
                    .fill(palette.surface)
                    .stroke(Stroke::new(
                        1.0,
                        if is_err { palette.error } else { palette.good },
                    ))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(if is_err { "!" } else { "✓" })
                                    .strong()
                                    .color(if is_err { palette.error } else { palette.good }),
                            );
                            ui.label(RichText::new(&msg).size(12.5).color(palette.text));
                            if ui.small_button("×").clicked() {
                                dismiss = true;
                            }
                        });
                    });
            });
        if dismiss {
            self.toast = None;
        }
    }
}

// ============================================================================
// Helpers & Styling
// ============================================================================

struct Palette {
    bg: Color32,
    surface: Color32,
    inset: Color32,
    border: Color32,
    text: Color32,
    muted: Color32,
    accent: Color32,
    accent_bg: Color32,
    good: Color32,
    good_bg: Color32,
    warn: Color32,
    warn_bg: Color32,
    error: Color32,
    error_bg: Color32,
}

impl Palette {
    fn for_mode(dark: bool) -> Self {
        if dark {
            Self {
                bg: Color32::from_rgb(15, 20, 32),
                surface: Color32::from_rgb(22, 29, 46),
                inset: Color32::from_rgb(30, 39, 60),
                border: Color32::from_rgb(46, 58, 86),
                text: Color32::from_rgb(236, 241, 250),
                muted: Color32::from_rgb(148, 163, 190),
                accent: Color32::from_rgb(99, 102, 241),
                accent_bg: Color32::from_rgb(35, 38, 78),
                good: Color32::from_rgb(52, 211, 153),
                good_bg: Color32::from_rgb(20, 56, 48),
                warn: Color32::from_rgb(251, 191, 36),
                warn_bg: Color32::from_rgb(62, 50, 24),
                error: Color32::from_rgb(248, 113, 113),
                error_bg: Color32::from_rgb(66, 28, 34),
            }
        } else {
            Self {
                bg: Color32::from_rgb(243, 245, 250),
                surface: Color32::from_rgb(255, 255, 255),
                inset: Color32::from_rgb(236, 240, 248),
                border: Color32::from_rgb(212, 220, 234),
                text: Color32::from_rgb(22, 29, 44),
                muted: Color32::from_rgb(95, 108, 132),
                accent: Color32::from_rgb(79, 70, 229),
                accent_bg: Color32::from_rgb(232, 234, 255),
                good: Color32::from_rgb(16, 148, 98),
                good_bg: Color32::from_rgb(224, 248, 237),
                warn: Color32::from_rgb(180, 112, 12),
                warn_bg: Color32::from_rgb(254, 243, 214),
                error: Color32::from_rgb(208, 42, 52),
                error_bg: Color32::from_rgb(254, 232, 234),
            }
        }
    }
}

fn apply_theme(ctx: &egui::Context, theme: &str) {
    let mut visuals = match theme {
        "light" => egui::Visuals::light(),
        "dark" => egui::Visuals::dark(),
        _ => egui::Visuals::dark(),
    };
    let p = Palette::for_mode(visuals.dark_mode);
    visuals.panel_fill = p.bg;
    visuals.window_fill = p.surface;
    visuals.widgets.noninteractive.bg_fill = p.surface;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    visuals.widgets.inactive.bg_fill = p.inset;
    visuals.selection.bg_fill = p.accent;
    ctx.set_visuals(visuals);
}

fn card_frame(p: &Palette) -> Frame {
    Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(14))
}

fn pill_badge(ui: &mut egui::Ui, text: &str, fg: Color32, bg: Color32) {
    Frame::new()
        .fill(bg)
        .corner_radius(CornerRadius::same(5))
        .inner_margin(Margin::symmetric(7, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(11.0).strong().color(fg));
        });
}

fn status_style<'a>(status: &'a str, p: &Palette) -> (&'a str, Color32, Color32) {
    match status {
        "queued" => ("◷ Antrean", p.accent, p.accent_bg),
        "downloading" => ("↓ Mengunduh", p.accent, p.accent_bg),
        "processing" => ("⚙ Memproses", p.warn, p.warn_bg),
        "completed" => ("✓ Selesai", p.good, p.good_bg),
        "failed" => ("! Gagal", p.error, p.error_bg),
        "cancelled" => ("× Batal", p.muted, p.inset),
        "interrupted" => ("! Terhenti", p.warn, p.warn_bg),
        _ => (status, p.muted, p.inset),
    }
}

fn platform_label(p: &str) -> &'static str {
    PLATFORMS
        .iter()
        .find(|(k, _)| *k == p)
        .map(|(_, v)| *v)
        .unwrap_or("Platform")
}

fn quality_label(q: &str) -> String {
    if q == "best" {
        "Terbaik".into()
    } else {
        format!("{q}p")
    }
}

fn audio_format_label(a: &str) -> &'static str {
    match a {
        "wav" => "WAV (PCM 16-bit)",
        _ => "M4A (Lossless AAC)",
    }
}

fn image_format_label(f: &str, count: usize) -> String {
    if f == "zip" {
        format!("Semua foto ({count} slide · ZIP)")
    } else {
        f.to_uppercase()
    }
}

fn theme_label(t: &str) -> &'static str {
    match t {
        "light" => "Terang",
        "system" => "Sistem",
        _ => "Gelap",
    }
}

fn kind_label(job: &JobRecord) -> String {
    if job.kind == "video" {
        format!("Video · {} {}", job.container.to_uppercase(), quality_label(&job.quality))
    } else if job.kind == "audio" {
        format!("Audio · {}", job.container.to_uppercase())
    } else if job.container == "zip" {
        "Foto · ZIP Album".into()
    } else {
        format!("Foto · {}", job.container.to_uppercase())
    }
}

fn nice_bytes(b: i64) -> String {
    let v = b.max(0) as f64;
    if v >= 1_073_741_824.0 {
        format!("{:.2} GB", v / 1_073_741_824.0)
    } else if v >= 1_048_576.0 {
        format!("{:.1} MB", v / 1_048_576.0)
    } else if v >= 1024.0 {
        format!("{:.0} KB", v / 1024.0)
    } else {
        format!("{b} B")
    }
}

fn nice_duration(sec: f64) -> String {
    let s = sec.max(0.0).round() as u64;
    let h = s / 3600;
    let m = (s % 3600) / 60;
    let r = s % 60;
    if h > 0 {
        format!("{h}:{m:02}:{r:02}")
    } else {
        format!("{m}:{r:02}")
    }
}

fn fit_aspect(src: Vec2, max: Vec2) -> Vec2 {
    if src.x <= 0.0 || src.y <= 0.0 {
        return max;
    }
    let scale = (max.x / src.x).min(max.y / src.y).min(1.0);
    Vec2::new((src.x * scale).max(1.0), (src.y * scale).max(1.0))
}

fn load_zip_slide_names(zip_path: &Path) -> Result<Vec<String>, AppError> {
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

fn load_zip_slide_rgba(zip_path: &Path, entry_name: &str) -> Result<PreviewRgba, AppError> {
    let file = std::fs::File::open(zip_path).map_err(|_| AppError::new("FILE_NOT_FOUND"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|_| AppError::new("FORMAT_UNAVAILABLE"))?;
    let entry = archive
        .by_name(entry_name)
        .map_err(|_| AppError::new("FILE_NOT_FOUND"))?;
    if entry.is_symlink() {
        return Err(AppError::new("FORBIDDEN"));
    }
    let mut buf = Vec::new();
    entry
        .take(MAX_ZIP_SLIDE_BYTES + 1)
        .read_to_end(&mut buf)
        .map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;
    if buf.len() as u64 > MAX_ZIP_SLIDE_BYTES {
        return Err(AppError::new("FILE_TOO_LARGE"));
    }
    decode_preview_rgba(&buf, 1440)
}
