//! MediaVault Desktop — 100% Pure-Rust Native GUI (Full English, Smooth Animations).

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use egui::{
    Align, Align2, Color32, ColorImage, CornerRadius, Frame, Id, Layout, Margin, Pos2, Rect,
    RichText, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Vec2,
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
use crate::ui_kit::{
    anim_button, animated_progress_bar, animated_tab_bar, apply_modern_theme, draw_ambient_glow,
    draw_brand_mark, draw_equalizer_bars, draw_icon, draw_spinner, ease_out_cubic, font_regular,
    font_semibold, install_modern_fonts, lerp_color, platform_chip, section_card,
    show_animated_modal, status_pill, subtle_tag, sync_window_chrome, with_alpha, BtnVariant, Icon,
    Palette,
};
use crate::video_player::VideoPlayer;

const PLATFORMS: &[(&str, &str)] = &[
    ("", "All platforms"),
    ("youtube", "YouTube"),
    ("instagram", "Instagram"),
    ("tiktok", "TikTok"),
    ("x", "X"),
    ("threads", "Threads"),
    ("facebook", "Facebook"),
    ("pinterest", "Pinterest"),
];

const SUPPORTED_BADGES: &[(&str, &str)] = &[
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
    created_at: Instant,
    duration: Duration,
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
    video_frame_seq: u64,
    audio_player: AudioPlayer,
    video_player: VideoPlayer,

    // GPU Texture cache for thumbnails
    textures: HashMap<String, TextureHandle>,
    loading_tokens: HashSet<String>,

    toast: Option<ToastState>,
    applied_theme: String,
    chrome_synced_frames: u8,
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
        install_modern_fonts(&cc.egui_ctx);

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

        apply_modern_theme(&cc.egui_ctx, &prefs.theme);

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
            video_frame_seq: 0,
            audio_player: AudioPlayer::new(),
            video_player: VideoPlayer::new(),
            textures: HashMap::new(),
            loading_tokens: HashSet::new(),
            toast: None,
            chrome_synced_frames: 0,
        };
        app.refresh_jobs();
        app
    }

    fn show_toast(&mut self, msg: impl Into<String>, is_error: bool) {
        self.toast = Some(ToastState {
            message: msg.into(),
            is_error,
            created_at: Instant::now(),
            duration: Duration::from_millis(3800),
        });
    }

    fn toggle_quick_theme(&mut self, ctx: &egui::Context) {
        let next = if self.prefs.theme == "light" {
            "dark"
        } else {
            "light"
        };
        let mut updated = self.prefs.clone();
        updated.theme = next.into();
        if let Ok(saved) = self.db.save_preferences(&updated) {
            self.prefs = saved;
            self.settings_draft = self.prefs.clone();
        } else {
            self.prefs.theme = next.into();
        }
        apply_modern_theme(ctx, &self.prefs.theme);
        self.applied_theme = self.prefs.theme.clone();
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
            self.url_error = Some("Paste a public media link first.".into());
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
        self.video_player.stop();
        self.viewer_texture = None;
        self.video_frame_seq = 0;
        self.viewer_content = None;
        self.viewer_open = true;
        self.viewer_loading = true;

        let Some(path_str) = job.output_path.clone() else {
            self.viewer_loading = false;
            self.viewer_content = Some(ViewerContent::Error("File was not found on disk.".into()));
            return;
        };
        let title = job
            .title
            .clone()
            .or_else(|| job.output_filename.clone())
            .unwrap_or_else(|| "Media Preview".into());
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
                    "File was not found on disk.".into(),
                )));
                ctx_clone.request_repaint();
                return;
            };
            if !fs_meta.file_type().is_file() {
                let _ = tx.send(AsyncMsg::ViewerReady(ViewerContent::Error(
                    "Invalid file path.".into(),
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
                    None => ViewerContent::Error("Unable to decode image file.".into()),
                },
                "zip" => match load_zip_slide_names(p) {
                    Ok(names) if !names.is_empty() => {
                        let first_rgba = load_zip_slide_rgba(p, &names[0]).ok();
                        ViewerContent::Album {
                            title,
                            meta: format!("{meta} · {} photos", names.len()),
                            path: path_str,
                            slide_names: names,
                            current_idx: 0,
                            current_rgba: first_rgba,
                        }
                    }
                    _ => ViewerContent::Error("ZIP archive does not contain valid photos.".into()),
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
                _ => ViewerContent::Error("Preview is not supported for this file format.".into()),
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
        let next =
            (*current_idx as isize + delta).clamp(0, (slide_names.len() - 1) as isize) as usize;
        if next == *current_idx {
            return;
        }
        *current_idx = next;
        if let Ok(rgba) = load_zip_slide_rgba(Path::new(path), &slide_names[next]) {
            let img = ColorImage::from_rgba_unmultiplied(
                [rgba.width as usize, rgba.height as usize],
                &rgba.pixels,
            );
            self.viewer_texture =
                Some(ctx.load_texture("viewer_active", img, TextureOptions::LINEAR));
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
                        ViewerContent::Video { path, .. } => {
                            self.video_frame_seq = 0;
                            self.viewer_texture = None;
                            let ctx_repaint = ctx.clone();
                            self.video_player.open(Path::new(path), move || {
                                ctx_repaint.request_repaint();
                            });
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
            apply_modern_theme(ctx, &self.prefs.theme);
            self.applied_theme = self.prefs.theme.clone();
            self.chrome_synced_frames = 0;
        }

        let dark = ctx.style().visuals.dark_mode;
        let palette = Palette::for_mode(dark);

        // Sync Windows 11 DWM titlebar color once window handle is active
        if self.chrome_synced_frames < 3 {
            sync_window_chrome(dark, &palette);
            self.chrome_synced_frames += 1;
        }

        // Smooth live DB polling while active jobs run
        let poll_interval = if self.counts.active > 0 {
            Duration::from_millis(300)
        } else {
            Duration::from_secs(4)
        };
        if self.last_refresh.elapsed() >= poll_interval {
            self.refresh_jobs();
        }
        if self.counts.active > 0
            || self.analyzing
            || self.viewer_loading
            || self.audio_player.is_active()
            || self.video_player.is_open()
        {
            ctx.request_repaint_after(Duration::from_millis(16));
        }

        if let Some(t) = &self.toast {
            if t.created_at.elapsed() >= t.duration {
                self.toast = None;
            }
        }

        // === TOP HEADER BAR ===
        egui::TopBottomPanel::top("header")
            .frame(
                Frame::new()
                    .fill(palette.header_bg)
                    .inner_margin(Margin::symmetric(22, 12))
                    .stroke(Stroke::new(1.0, palette.border)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    draw_brand_mark(ui, &palette);
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("MediaVault")
                            .font(font_semibold(18.5))
                            .color(palette.text),
                    );
                    ui.add_space(2.0);
                    status_pill(
                        ui,
                        "Pure Rust",
                        palette.accent,
                        palette.accent_soft,
                        None,
                        false,
                    );

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if anim_button(
                            ui,
                            "hdr_settings",
                            "Settings",
                            Some(Icon::Settings),
                            BtnVariant::Secondary,
                            32.0,
                            None,
                            true,
                            false,
                            &palette,
                        )
                        .clicked()
                        {
                            self.settings_draft = self.prefs.clone();
                            self.settings_open = true;
                        }

                        if anim_button(
                            ui,
                            "hdr_folder",
                            "Downloads",
                            Some(Icon::Folder),
                            BtnVariant::Secondary,
                            32.0,
                            None,
                            true,
                            false,
                            &palette,
                        )
                        .clicked()
                        {
                            if let Err(e) = open_folder_in_explorer(&self.prefs.download_dir) {
                                self.show_toast(e.to_string(), true);
                            }
                        }

                        let theme_icon = if dark { Icon::Sun } else { Icon::Moon };
                        let theme_resp = anim_button(
                            ui,
                            "hdr_theme",
                            "",
                            Some(theme_icon),
                            BtnVariant::Secondary,
                            32.0,
                            Some(32.0),
                            true,
                            false,
                            &palette,
                        );
                        if theme_resp.clicked() {
                            self.toggle_quick_theme(ctx);
                        }
                        theme_resp.on_hover_text(if dark {
                            "Switch to Light theme"
                        } else {
                            "Switch to Dark theme"
                        });

                        ui.add_space(4.0);
                        status_pill(
                            ui,
                            "Ready · Zero External Deps",
                            palette.good,
                            palette.good_soft,
                            None,
                            true,
                        );
                    });
                });
            });

        // === BOTTOM STATUS BAR ===
        egui::TopBottomPanel::bottom("footer")
            .frame(
                Frame::new()
                    .fill(palette.header_bg)
                    .inner_margin(Margin::symmetric(22, 8))
                    .stroke(Stroke::new(1.0, palette.border)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("MediaVault v0.4.0 · 100% Native Rust + SQLite · Local & Private")
                            .font(font_regular(11.5))
                            .color(palette.muted),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("Folder: {}", self.prefs.download_dir))
                                .font(font_regular(11.5))
                                .color(palette.muted),
                        );
                    });
                });
            });

        // === MAIN WORKSPACE ===
        egui::CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(palette.bg)
                    .inner_margin(Margin::symmetric(22, 16)),
            )
            .show(ctx, |ui| {
                // Subtle ambient top radial glow (Linear / Raycast aesthetic)
                draw_ambient_glow(ui.painter(), ui.max_rect(), &palette);

                // 1. HERO URL ANALYZER CARD
                section_card(&palette).show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new("Download Video, Audio & Photos")
                                .font(font_semibold(15.0))
                                .color(palette.text),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            for &(key, name) in SUPPORTED_BADGES.iter().rev() {
                                platform_chip(ui, name, key, &palette);
                            }
                        });
                    });

                    ui.add_space(12.0);

                    // Unified modern URL bar with animated focus ring
                    let url_box_id = Id::new("hero_url_input");
                    let has_focus = ctx.memory(|m| m.has_focus(url_box_id));
                    let focus_t = ctx.animate_bool_with_time(url_box_id.with("ring"), has_focus, 0.16);
                    let ring_color = lerp_color(palette.border, palette.accent, focus_t);

                    ui.horizontal(|ui| {
                        let paste_w = 88.0;
                        let analyze_w = 136.0;
                        let input_w = (ui.available_width() - paste_w - analyze_w - 20.0).max(240.0);

                        let input_frame = Frame::new()
                            .fill(palette.input_bg)
                            .stroke(Stroke::new(if focus_t > 0.05 { 1.4 } else { 1.0 }, ring_color))
                            .corner_radius(CornerRadius::same(10))
                            .inner_margin(Margin::symmetric(12, 0));

                        input_frame.show(ui, |ui| {
                            ui.set_width(input_w - 24.0);
                            ui.set_height(38.0);
                            ui.horizontal_centered(|ui| {
                                let clear_space = if self.url_input.is_empty() { 0.0 } else { 28.0 };
                                let te_w = (ui.available_width() - clear_space).max(120.0);
                                let resp = ui.add_sized(
                                    [te_w, 34.0],
                                    egui::TextEdit::singleline(&mut self.url_input)
                                        .id(url_box_id)
                                        .frame(false)
                                        .font(font_regular(13.5))
                                        .hint_text(
                                            RichText::new("Paste a public video, reel, photo, or album URL here...")
                                                .color(palette.muted),
                                        ),
                                );
                                if resp.lost_focus()
                                    && ui.input(|i| i.key_pressed(egui::Key::Enter))
                                    && !self.analyzing
                                {
                                    self.start_analyze(ctx);
                                }
                                if !self.url_input.is_empty()
                                    && anim_button(
                                        ui,
                                        "url_clear_btn",
                                        "",
                                        Some(Icon::Close),
                                        BtnVariant::Ghost,
                                        24.0,
                                        Some(24.0),
                                        true,
                                        false,
                                        &palette,
                                    )
                                    .clicked()
                                {
                                    self.url_input.clear();
                                    self.url_error = None;
                                }
                            });
                        });

                        if anim_button(
                            ui,
                            "paste_btn",
                            "Paste",
                            Some(Icon::Paste),
                            BtnVariant::Secondary,
                            40.0,
                            Some(paste_w),
                            true,
                            false,
                            &palette,
                        )
                        .clicked()
                        {
                            if let Ok(mut cb) = arboard::Clipboard::new() {
                                if let Ok(text) = cb.get_text() {
                                    self.url_input = text.trim().to_string();
                                    self.url_error = None;
                                }
                            }
                        }

                        if anim_button(
                            ui,
                            "analyze_btn",
                            if self.analyzing { "Analyzing..." } else { "Analyze URL" },
                            Some(Icon::Sparkle),
                            BtnVariant::Primary,
                            40.0,
                            Some(analyze_w),
                            !self.analyzing,
                            self.analyzing,
                            &palette,
                        )
                        .clicked()
                        {
                            self.start_analyze(ctx);
                        }
                    });

                    if let Some(err) = &self.url_error {
                        ui.add_space(10.0);
                        Frame::new()
                            .fill(palette.error_soft)
                            .stroke(Stroke::new(1.0, with_alpha(palette.error, 85)))
                            .corner_radius(CornerRadius::same(8))
                            .inner_margin(Margin::symmetric(12, 7))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    let (ir, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                                    draw_icon(ui.painter(), ir, Icon::Warning, palette.error);
                                    ui.label(
                                        RichText::new(err)
                                            .font(font_regular(12.5))
                                            .color(palette.error),
                                    );
                                });
                            });
                    }
                });

                ui.add_space(14.0);

                // 2. DOWNLOADS LIBRARY CARD
                section_card(&palette).show(ui, |ui| {
                    // Header + search + platform filter
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("Downloads")
                                .font(font_semibold(15.5))
                                .color(palette.text),
                        );
                        if self.counts.active > 0 {
                            status_pill(
                                ui,
                                &format!("{} active", self.counts.active),
                                palette.accent,
                                palette.accent_soft,
                                None,
                                true,
                            );
                        }

                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let prev_plat = self.platform_filter.clone();
                            egui::ComboBox::from_id_salt("platform_filter")
                                .selected_text(platform_label(&self.platform_filter))
                                .width(138.0)
                                .show_ui(ui, |ui| {
                                    for &(k, v) in PLATFORMS {
                                        ui.selectable_value(&mut self.platform_filter, k.into(), v);
                                    }
                                });
                            if self.platform_filter != prev_plat {
                                self.page = 0;
                                self.refresh_jobs();
                            }

                            // Modern search field with inline vector magnifying glass
                            let prev_search = self.search.clone();
                            Frame::new()
                                .fill(palette.input_bg)
                                .stroke(Stroke::new(1.0, palette.border))
                                .corner_radius(CornerRadius::same(8))
                                .inner_margin(Margin::symmetric(9, 0))
                                .show(ui, |ui| {
                                    ui.set_height(28.0);
                                    ui.set_width(196.0);
                                    ui.horizontal_centered(|ui| {
                                        let (ir, _) = ui.allocate_exact_size(Vec2::splat(13.0), Sense::hover());
                                        draw_icon(ui.painter(), ir, Icon::Search, palette.muted);
                                        ui.add_sized(
                                            [168.0, 24.0],
                                            egui::TextEdit::singleline(&mut self.search)
                                                .frame(false)
                                                .font(font_regular(12.5))
                                                .hint_text(
                                                    RichText::new("Search titles...").color(palette.muted),
                                                ),
                                        );
                                    });
                                });
                            if self.search != prev_search {
                                self.page = 0;
                                self.refresh_jobs();
                            }
                        });
                    });

                    ui.add_space(10.0);

                    // Animated Sliding Pill Tab Bar!
                    let tabs = [
                        ("all", format!("All ({})", self.counts.all)),
                        ("active", format!("Active ({})", self.counts.active)),
                        ("video", format!("Video ({})", self.counts.video)),
                        ("audio", format!("Audio ({})", self.counts.audio)),
                        ("image", format!("Photos ({})", self.counts.image)),
                    ];
                    if let Some(new_tab) =
                        animated_tab_bar(ui, "downloads_tabs", &tabs, &self.tab, &palette)
                    {
                        self.tab = new_tab;
                        self.page = 0;
                        self.refresh_jobs();
                    }

                    ui.add_space(12.0);

                    // Preload visible row thumbnails
                    let tokens_to_load: Vec<Option<String>> =
                        self.jobs.iter().map(|j| j.thumbnail_preview.clone()).collect();
                    for tok in tokens_to_load {
                        self.ensure_thumbnail(ctx, tok.as_deref());
                    }

                    let list_height = (ui.available_height() - 44.0).max(160.0);
                    egui::ScrollArea::vertical()
                        .max_height(list_height)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if self.jobs.is_empty() {
                                ui.add_space(52.0);
                                ui.vertical_centered(|ui| {
                                    let (ir, _) =
                                        ui.allocate_exact_size(Vec2::splat(38.0), Sense::hover());
                                    ui.painter().rect_filled(ir, CornerRadius::same(12), palette.inset);
                                    draw_icon(
                                        ui.painter(),
                                        ir.shrink(9.0),
                                        Icon::Download,
                                        palette.muted,
                                    );
                                    ui.add_space(10.0);
                                    ui.label(
                                        RichText::new("No downloads yet")
                                            .font(font_semibold(15.0))
                                            .color(palette.text),
                                    );
                                    ui.add_space(3.0);
                                    ui.label(
                                        RichText::new(
                                            "Paste a public video, audio, or photo link above and click Analyze URL.",
                                        )
                                        .font(font_regular(12.5))
                                        .color(palette.muted),
                                    );
                                });
                            } else {
                                let jobs_snapshot = self.jobs.clone();
                                for (idx, job) in jobs_snapshot.iter().enumerate() {
                                    self.render_job_card(ui, ctx, job, &palette);
                                    if idx + 1 < jobs_snapshot.len() {
                                        ui.add_space(7.0);
                                    }
                                }
                            }
                        });

                    // Pagination footer
                    ui.add_space(10.0);
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
                                "Showing {start}-{end} of {} downloads",
                                self.total_jobs
                            ))
                            .font(font_regular(12.0))
                            .color(palette.muted),
                        );

                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if anim_button(
                                ui,
                                "pg_last",
                                "",
                                Some(Icon::DoubleRight),
                                BtnVariant::Secondary,
                                28.0,
                                Some(28.0),
                                self.page + 1 < total_pages,
                                false,
                                &palette,
                            )
                            .clicked()
                            {
                                self.page = total_pages.saturating_sub(1);
                                self.refresh_jobs();
                            }
                            if anim_button(
                                ui,
                                "pg_next",
                                "",
                                Some(Icon::ChevronRight),
                                BtnVariant::Secondary,
                                28.0,
                                Some(28.0),
                                self.page + 1 < total_pages,
                                false,
                                &palette,
                            )
                            .clicked()
                            {
                                self.page += 1;
                                self.refresh_jobs();
                            }

                            ui.label(
                                RichText::new(format!("{} / {}", self.page + 1, total_pages))
                                    .font(font_semibold(12.0))
                                    .color(palette.text_secondary),
                            );

                            if anim_button(
                                ui,
                                "pg_prev",
                                "",
                                Some(Icon::ChevronLeft),
                                BtnVariant::Secondary,
                                28.0,
                                Some(28.0),
                                self.page > 0,
                                false,
                                &palette,
                            )
                            .clicked()
                            {
                                self.page = self.page.saturating_sub(1);
                                self.refresh_jobs();
                            }
                            if anim_button(
                                ui,
                                "pg_first",
                                "",
                                Some(Icon::DoubleLeft),
                                BtnVariant::Secondary,
                                28.0,
                                Some(28.0),
                                self.page > 0,
                                false,
                                &palette,
                            )
                            .clicked()
                            {
                                self.page = 0;
                                self.refresh_jobs();
                            }

                            ui.add_space(4.0);
                            let prev_ps = self.page_size;
                            egui::ComboBox::from_id_salt("page_size")
                                .selected_text(format!("{} / page", self.page_size))
                                .width(88.0)
                                .show_ui(ui, |ui| {
                                    for n in [4, 5, 10, 20] {
                                        ui.selectable_value(&mut self.page_size, n, format!("{n} / page"));
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

        // === ANIMATED MODALS & TOAST ===
        self.render_analyze_modal(ctx, &palette);
        self.render_viewer_modal(ctx, &palette);
        self.render_settings_modal(ctx, &palette);
        self.render_delete_modal(ctx, &palette);
        self.render_toast(ctx, &palette);
    }
}

impl MediaVaultApp {
    fn render_job_card(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        job: &JobRecord,
        palette: &Palette,
    ) {
        let is_active = matches!(job.status.as_str(), "queued" | "downloading" | "processing");
        let can_preview = job.status == "completed" && job.file_available;
        let row_id = Id::new(("job_row", &job.id));

        // Smooth hover highlight across the entire card
        let prev_hov = ctx.data(|d| d.get_temp::<bool>(row_id.with("hov_state")).unwrap_or(false));
        let hover_t = ctx.animate_bool_with_time(row_id.with("hov_anim"), prev_hov, 0.14);

        let card_bg = lerp_color(
            if palette.dark {
                Color32::from_rgb(20, 25, 39)
            } else {
                Color32::from_rgb(249, 250, 253)
            },
            palette.surface_hover,
            hover_t,
        );
        let card_border = lerp_color(palette.border, palette.border_hover, hover_t);

        let frame_resp = Frame::new()
            .fill(card_bg)
            .stroke(Stroke::new(1.0, card_border))
            .corner_radius(CornerRadius::same(11))
            .inner_margin(Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    // Rounded thumbnail with subtle hover eye overlay
                    let thumb_size = Vec2::new(72.0, 44.0);
                    let tex = job
                        .thumbnail_preview
                        .as_deref()
                        .and_then(|t| self.textures.get(t));

                    let (thumb_rect, thumb_resp) = ui.allocate_exact_size(
                        thumb_size,
                        if can_preview { Sense::click() } else { Sense::hover() },
                    );
                    if can_preview && thumb_resp.hovered() {
                        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                    }

                    if let Some(handle) = tex {
                        egui::Image::new((handle.id(), thumb_size))
                            .corner_radius(CornerRadius::same(7))
                            .paint_at(ui, thumb_rect);
                        ui.painter().rect_stroke(
                            thumb_rect,
                            CornerRadius::same(7),
                            Stroke::new(1.0, palette.border),
                            StrokeKind::Inside,
                        );
                    } else {
                        ui.painter()
                            .rect_filled(thumb_rect, CornerRadius::same(7), palette.inset);
                        let icon = match job.kind.as_str() {
                            "image" => Icon::Photo,
                            "audio" => Icon::Audio,
                            _ => Icon::Video,
                        };
                        draw_icon(
                            ui.painter(),
                            Rect::from_center_size(thumb_rect.center(), Vec2::splat(18.0)),
                            icon,
                            palette.muted,
                        );
                    }

                    // Hover preview overlay on thumbnail
                    let thumb_hov = ctx.animate_bool_with_time(
                        row_id.with("th_hov"),
                        can_preview && thumb_resp.hovered(),
                        0.12,
                    );
                    if thumb_hov > 0.01 {
                        ui.painter().rect_filled(
                            thumb_rect,
                            CornerRadius::same(7),
                            Color32::from_rgba_unmultiplied(9, 11, 17, (thumb_hov * 155.0) as u8),
                        );
                        draw_icon(
                            ui.painter(),
                            Rect::from_center_size(thumb_rect.center(), Vec2::splat(16.0)),
                            Icon::Eye,
                            with_alpha(Color32::WHITE, (thumb_hov * 255.0) as u8),
                        );
                    }
                    if can_preview && thumb_resp.clicked() {
                        self.open_completed_preview(ctx, job);
                    }

                    ui.add_space(4.0);

                    // Middle metadata column
                    ui.vertical(|ui| {
                        ui.horizontal_wrapped(|ui| {
                            let title = job
                                .title
                                .as_deref()
                                .or(job.output_filename.as_deref())
                                .unwrap_or(&job.platform);
                            let short_title: String = title.chars().take(52).collect();
                            let display = if title.chars().count() > 52 {
                                format!("{short_title}...")
                            } else {
                                short_title
                            };
                            ui.label(
                                RichText::new(display)
                                    .font(font_semibold(13.5))
                                    .color(palette.text),
                            );

                            let (st_text, st_col, st_bg, st_icon, st_pulse) =
                                status_style(&job.status, palette);
                            status_pill(ui, st_text, st_col, st_bg, st_icon, st_pulse);
                            subtle_tag(ui, &kind_label(job), palette);
                        });

                        ui.add_space(4.0);

                        if is_active {
                            let frac =
                                (job.progress_percent.unwrap_or(0.0) / 100.0).clamp(0.0, 1.0) as f32;
                            ui.horizontal(|ui| {
                                animated_progress_bar(ui, ("prog", &job.id), frac, 210.0, palette);
                                let mut parts = vec![format!(
                                    "{}%",
                                    job.progress_percent.unwrap_or(0.0).round() as i64
                                )];
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
                                ui.label(
                                    RichText::new(parts.join(" · "))
                                        .font(font_regular(11.5))
                                        .color(palette.muted),
                                );
                            });
                        } else {
                            ui.horizontal_wrapped(|ui| {
                                // Colored platform dot
                                let (dot_r, _) =
                                    ui.allocate_exact_size(Vec2::new(8.0, 14.0), Sense::hover());
                                ui.painter().circle_filled(
                                    dot_r.center(),
                                    3.2,
                                    palette.platform_color(&job.platform),
                                );
                                let mut meta = vec![platform_label(&job.platform).to_string()];
                                if let Some(sz) = job.file_size_bytes {
                                    meta.push(nice_bytes(sz));
                                }
                                ui.label(
                                    RichText::new(meta.join(" · "))
                                        .font(font_regular(11.5))
                                        .color(palette.muted),
                                );
                                if let Some(err) = &job.error_message {
                                    ui.label(
                                        RichText::new(format!("· {err}"))
                                            .font(font_regular(11.5))
                                            .color(palette.error),
                                    );
                                } else if job.status == "completed" && !job.file_available {
                                    ui.label(
                                        RichText::new("· File missing on disk")
                                            .font(font_regular(11.5))
                                            .color(palette.error),
                                    );
                                }
                            });
                        }
                    });

                    // Right action pills
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if is_active {
                            if anim_button(
                                ui,
                                ("cancel", &job.id),
                                "Cancel",
                                Some(Icon::Close),
                                BtnVariant::DangerGhost,
                                28.0,
                                None,
                                true,
                                false,
                                palette,
                            )
                            .clicked()
                            {
                                match self.engine.cancel(&job.id) {
                                    Ok(()) => {
                                        self.show_toast("Download cancelled.", false);
                                        self.refresh_jobs();
                                    }
                                    Err(e) => self.show_toast(e.to_string(), true),
                                }
                            }
                        } else {
                            let del_resp = anim_button(
                                ui,
                                ("del", &job.id),
                                "",
                                Some(Icon::Trash),
                                BtnVariant::DangerGhost,
                                28.0,
                                Some(28.0),
                                true,
                                false,
                                palette,
                            );
                            if del_resp.clicked() {
                                self.delete_target = Some(job.clone());
                                self.delete_file_checked = false;
                            }
                            del_resp.on_hover_text("Delete entry");

                            let retry_variant = if job.status == "failed" {
                                BtnVariant::AccentGhost
                            } else {
                                BtnVariant::Secondary
                            };
                            let retry_label = if job.status == "failed" { "Retry" } else { "" };
                            let retry_resp = anim_button(
                                ui,
                                ("retry", &job.id),
                                retry_label,
                                Some(Icon::Retry),
                                retry_variant,
                                28.0,
                                if retry_label.is_empty() { Some(28.0) } else { None },
                                true,
                                false,
                                palette,
                            );
                            if retry_resp.clicked() {
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
                                        self.show_toast("Download re-queued.", false);
                                        self.page = 0;
                                        self.refresh_jobs();
                                    }
                                    Err(e) => self.show_toast(e.to_string(), true),
                                }
                            }
                            if retry_label.is_empty() {
                                retry_resp.on_hover_text("Download again");
                            }

                            if job.file_available {
                                let move_resp = anim_button(
                                    ui,
                                    ("move", &job.id),
                                    "",
                                    Some(Icon::Move),
                                    BtnVariant::Secondary,
                                    28.0,
                                    Some(28.0),
                                    true,
                                    false,
                                    palette,
                                );
                                if move_resp.clicked() {
                                    let start = job
                                        .output_path
                                        .as_deref()
                                        .and_then(|p| Path::new(p).parent())
                                        .map(Path::to_path_buf);
                                    let mut dlg =
                                        rfd::FileDialog::new().set_title("Choose destination folder");
                                    if let Some(d) = start.filter(|d| d.is_dir()) {
                                        dlg = dlg.set_directory(d);
                                    }
                                    if let Some(dest_dir) = dlg.pick_folder() {
                                        match move_job_file(&self.db, &job.id, &dest_dir) {
                                            Ok(_) => {
                                                self.show_toast("File moved successfully.", false);
                                                self.refresh_jobs();
                                            }
                                            Err(e) => self.show_toast(e.to_string(), true),
                                        }
                                    }
                                }
                                move_resp.on_hover_text("Move file to another folder");

                                if anim_button(
                                    ui,
                                    ("folder", &job.id),
                                    "Folder",
                                    Some(Icon::Folder),
                                    BtnVariant::GoodGhost,
                                    28.0,
                                    None,
                                    true,
                                    false,
                                    palette,
                                )
                                .clicked()
                                {
                                    if let Some(p) = job.output_path.as_deref() {
                                        if let Err(e) = open_file_in_explorer(p) {
                                            self.show_toast(e.to_string(), true);
                                        }
                                    }
                                }

                                if can_preview {
                                    if anim_button(
                                        ui,
                                        ("preview", &job.id),
                                        "Preview",
                                        Some(Icon::Eye),
                                        BtnVariant::AccentGhost,
                                        28.0,
                                        None,
                                        true,
                                        false,
                                        palette,
                                    )
                                    .clicked()
                                    {
                                        self.open_completed_preview(ctx, job);
                                    }
                                }
                            }
                        }
                    });
                });
            });

        let now_hov = ui.rect_contains_pointer(frame_resp.response.rect);
        ctx.data_mut(|d| d.insert_temp(row_id.with("hov_state"), now_hov));
    }

    fn render_analyze_modal(&mut self, ctx: &egui::Context, palette: &Palette) {
        let Some(mut modal) = self.analyze_modal.take() else {
            // Keep exit animation alive cleanly
            let mut closed = false;
            show_animated_modal(
                ctx,
                "modal_analyze",
                "Download Options",
                None,
                Vec2::new(760.0, 465.0),
                palette,
                &mut closed,
                |_| {},
            );
            return;
        };

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

        show_animated_modal(
            ctx,
            "modal_analyze",
            "Media Preview & Download Options",
            Some("Choose output type, quality, and destination folder"),
            Vec2::new(760.0, 465.0),
            palette,
            &mut keep_open,
            |ui| {
                ui.columns(2, |cols| {
                    // Left column: preview + metadata
                    cols[0].vertical(|ui| {
                        let box_size = Vec2::new(336.0, 200.0);
                        let (rect, _) = ui.allocate_exact_size(box_size, Sense::hover());
                        ui.painter()
                            .rect_filled(rect, CornerRadius::same(10), palette.input_bg);
                        if let Some(tex) = preview_tok.as_deref().and_then(|t| self.textures.get(t))
                        {
                            let sz = fit_aspect(tex.size_vec2(), box_size - Vec2::splat(6.0));
                            let img_rect = Rect::from_center_size(rect.center(), sz);
                            egui::Image::new((tex.id(), sz))
                                .corner_radius(CornerRadius::same(8))
                                .paint_at(ui, img_rect);
                        } else {
                            let t = ui.input(|i| i.time);
                            draw_spinner(ui.painter(), rect.center(), 14.0, palette.accent, t);
                            ui.ctx().request_repaint();
                        }
                        ui.painter().rect_stroke(
                            rect,
                            CornerRadius::same(10),
                            Stroke::new(1.0, palette.border),
                            StrokeKind::Inside,
                        );

                        ui.add_space(10.0);
                        platform_chip(
                            ui,
                            platform_label(&modal.meta.platform),
                            &modal.meta.platform,
                            palette,
                        );
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(
                                modal
                                    .meta
                                    .title
                                    .as_deref()
                                    .unwrap_or("Untitled media"),
                            )
                            .font(font_semibold(14.0))
                            .color(palette.text),
                        );
                        if let Some(up) = &modal.meta.uploader {
                            ui.add_space(2.0);
                            ui.label(
                                RichText::new(up)
                                    .font(font_regular(12.0))
                                    .color(palette.muted),
                            );
                        }
                        if let Some(dur) = modal.meta.duration_seconds {
                            ui.label(
                                RichText::new(format!("Duration: {}", nice_duration(dur)))
                                    .font(font_regular(12.0))
                                    .color(palette.muted),
                            );
                        }
                    });

                    // Right column: options
                    cols[1].vertical(|ui| {
                        ui.label(
                            RichText::new("Output format")
                                .font(font_semibold(12.5))
                                .color(palette.text_secondary),
                        );
                        ui.add_space(6.0);

                        let mut modes: Vec<(&str, String)> = Vec::new();
                        if !modal.meta.available_qualities.is_empty() {
                            modes.push(("video", "Video".into()));
                        }
                        if !modal.meta.available_audio_formats.is_empty() {
                            modes.push(("audio", "Audio".into()));
                        }
                        if !modal.meta.available_image_formats.is_empty() {
                            modes.push(("image", "Photo".into()));
                        }
                        if !modes.is_empty() {
                            if let Some(m) =
                                animated_tab_bar(ui, "analyze_mode_tabs", &modes, &modal.mode, palette)
                            {
                                modal.mode = m;
                            }
                        }

                        ui.add_space(12.0);

                        match modal.mode.as_str() {
                            "video" => {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new("Resolution")
                                            .font(font_regular(12.5))
                                            .color(palette.muted),
                                    );
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        egui::ComboBox::from_id_salt("dl_quality")
                                            .selected_text(quality_label(&modal.quality))
                                            .width(175.0)
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
                                });
                                ui.add_space(6.0);
                                ui.label(
                                    RichText::new("Container: Universal MP4 (H.264 + AAC · Pure-Rust Muxer)")
                                        .font(font_regular(11.5))
                                        .color(palette.muted),
                                );
                                ui.add_space(6.0);
                                ui.checkbox(
                                    &mut modal.allow_fallback,
                                    "Fall back to closest available resolution if exact match is unavailable",
                                );
                            }
                            "audio" => {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new("Audio codec")
                                            .font(font_regular(12.5))
                                            .color(palette.muted),
                                    );
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        egui::ComboBox::from_id_salt("dl_audio_fmt")
                                            .selected_text(audio_format_label(&modal.audio_format))
                                            .width(185.0)
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
                                });
                            }
                            "image" => {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new("Photo format")
                                            .font(font_regular(12.5))
                                            .color(palette.muted),
                                    );
                                    let count = modal.meta.images.len();
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        egui::ComboBox::from_id_salt("dl_img_fmt")
                                            .selected_text(image_format_label(&modal.image_format, count))
                                            .width(195.0)
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
                                });
                                if modal.image_format != "zip" && modal.meta.images.len() > 1 {
                                    ui.add_space(8.0);
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(format!(
                                                "Photo slide ({} / {})",
                                                modal.selected_slide,
                                                modal.meta.images.len()
                                            ))
                                            .font(font_regular(12.5))
                                            .color(palette.muted),
                                        );
                                        if anim_button(
                                            ui,
                                            "slide_prev",
                                            "",
                                            Some(Icon::ChevronLeft),
                                            BtnVariant::Secondary,
                                            26.0,
                                            Some(26.0),
                                            modal.selected_slide > 1,
                                            false,
                                            palette,
                                        )
                                        .clicked()
                                        {
                                            modal.selected_slide -= 1;
                                        }
                                        if anim_button(
                                            ui,
                                            "slide_next",
                                            "",
                                            Some(Icon::ChevronRight),
                                            BtnVariant::Secondary,
                                            26.0,
                                            Some(26.0),
                                            modal.selected_slide < modal.meta.images.len(),
                                            false,
                                            palette,
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

                        ui.add_space(16.0);
                        ui.label(
                            RichText::new("Save to folder")
                                .font(font_semibold(12.0))
                                .color(palette.text_secondary),
                        );
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            Frame::new()
                                .fill(palette.input_bg)
                                .stroke(Stroke::new(1.0, palette.border))
                                .corner_radius(CornerRadius::same(8))
                                .inner_margin(Margin::symmetric(10, 6))
                                .show(ui, |ui| {
                                    ui.set_width(220.0);
                                    ui.add(
                                        egui::TextEdit::singleline(&mut modal.save_dir)
                                            .frame(false)
                                            .interactive(false)
                                            .font(font_regular(12.0)),
                                    );
                                });
                            if anim_button(
                                ui,
                                "modal_browse_dir",
                                "Browse",
                                Some(Icon::Folder),
                                BtnVariant::Secondary,
                                32.0,
                                None,
                                true,
                                false,
                                palette,
                            )
                            .clicked()
                            {
                                let mut dlg =
                                    rfd::FileDialog::new().set_title("Choose download folder");
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

                        ui.add_space(20.0);
                        if anim_button(
                            ui,
                            "modal_download_now",
                            "Download Now",
                            Some(Icon::Download),
                            BtnVariant::Primary,
                            40.0,
                            Some(ui.available_width()),
                            true,
                            false,
                            palette,
                        )
                        .clicked()
                        {
                            start_download = true;
                        }
                    });
                });
            },
        );

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
                    self.show_toast("Added to download queue.", false);
                }
                Err(e) => self.show_toast(e.to_string(), true),
            }
        } else if keep_open {
            self.analyze_modal = Some(modal);
        }
    }

    fn render_viewer_modal(&mut self, ctx: &egui::Context, palette: &Palette) {
        let is_video_viewer = matches!(self.viewer_content, Some(ViewerContent::Video { .. }));
        let mut video_seek_to: Option<f64> = None;
        let mut video_toggle_play = false;
        let mut video_set_volume: Option<f32> = None;
        let mut video_set_muted: Option<bool> = None;

        // Upload newest decoded video frame directly to GPU texture in-place
        if self.viewer_open && is_video_viewer {
            if let Some(frame) = self.video_player.take_new_frame(self.video_frame_seq) {
                self.video_frame_seq = frame.seq;
                let img = ColorImage::from_rgba_unmultiplied(
                    [frame.width as usize, frame.height as usize],
                    &frame.rgba,
                );
                if let Some(tex) = &mut self.viewer_texture {
                    tex.set(img, TextureOptions::LINEAR);
                } else {
                    self.viewer_texture =
                        Some(ctx.load_texture("viewer_video_frame", img, TextureOptions::LINEAR));
                }
            }
        }

        let video_snap = if is_video_viewer {
            Some(self.video_player.snapshot())
        } else {
            None
        };

        if self.viewer_open {
            if is_video_viewer {
                if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
                    video_toggle_play = true;
                } else if ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
                    if let Some(s) = &video_snap {
                        video_seek_to = Some((s.position_secs - 5.0).max(0.0));
                    }
                } else if ctx.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
                    if let Some(s) = &video_snap {
                        let max_t = if s.duration_secs > 0.0 {
                            s.duration_secs
                        } else {
                            s.position_secs + 5.0
                        };
                        video_seek_to = Some((s.position_secs + 5.0).min(max_t));
                    }
                } else if ctx.input(|i| i.key_pressed(egui::Key::M)) {
                    if let Some(s) = &video_snap {
                        video_set_muted = Some(!s.muted);
                    }
                }
            } else if ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
                self.switch_album_slide(ctx, -1);
            } else if ctx.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
                self.switch_album_slide(ctx, 1);
            }
        }

        let video_thumb_tok = match &self.viewer_content {
            Some(ViewerContent::Video { thumb_token, .. }) => thumb_token.clone(),
            _ => None,
        };
        if self.viewer_open {
            self.ensure_thumbnail(ctx, video_thumb_tok.as_deref());
        }

        let mut open = self.viewer_open;
        let mut delta_slide = 0isize;
        let mut open_explorer_path: Option<String> = None;
        let mut open_player_path: Option<String> = None;
        let mut play_audio_path: Option<String> = None;
        let mut toggle_audio = false;
        let mut stop_audio = false;

        let (modal_title, modal_sub) = match &self.viewer_content {
            Some(ViewerContent::Image { title, meta, .. })
            | Some(ViewerContent::Album { title, meta, .. })
            | Some(ViewerContent::Audio { title, meta, .. })
            | Some(ViewerContent::Video { title, meta, .. }) => {
                (title.as_str(), Some(meta.as_str()))
            }
            _ => ("Media Preview", None),
        };

        let title_owned = modal_title.to_string();
        let sub_owned = modal_sub.map(String::from);

        // Generous responsive cinema modal so both 16:9 landscape and 9:16 portrait videos shine!
        let screen = ctx.screen_rect();
        let modal_w = (screen.width() - 80.0).clamp(760.0, 960.0);
        let modal_h = (screen.height() - 72.0).clamp(540.0, 670.0);
        let stage_h = (modal_h - 210.0).clamp(320.0, 470.0);

        show_animated_modal(
            ctx,
            "modal_viewer",
            &title_owned,
            sub_owned.as_deref(),
            Vec2::new(modal_w, modal_h),
            palette,
            &mut open,
            |ui| {
                if self.viewer_loading {
                    let (r, _) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width(), stage_h),
                        Sense::hover(),
                    );
                    let t = ui.input(|i| i.time);
                    draw_spinner(ui.painter(), r.center(), 18.0, palette.accent, t);
                    ui.ctx().request_repaint();
                    return;
                }

                match &self.viewer_content {
                    Some(ViewerContent::Image { path, .. }) => {
                        let box_sz = Vec2::new(ui.available_width(), stage_h + 36.0);
                        let (rect, _) = ui.allocate_exact_size(box_sz, Sense::hover());
                        ui.painter()
                            .rect_filled(rect, CornerRadius::same(10), palette.input_bg);
                        if let Some(tex) = &self.viewer_texture {
                            let sz = fit_aspect_cinema(tex.size_vec2(), box_sz - Vec2::splat(12.0));
                            egui::Image::new((tex.id(), sz))
                                .corner_radius(CornerRadius::same(8))
                                .paint_at(ui, Rect::from_center_size(rect.center(), sz));
                        }
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            if anim_button(
                                ui,
                                "v_img_open",
                                "Open in Photos",
                                Some(Icon::External),
                                BtnVariant::Primary,
                                34.0,
                                None,
                                true,
                                false,
                                palette,
                            )
                            .clicked()
                            {
                                open_player_path = Some(path.clone());
                            }
                            if anim_button(
                                ui,
                                "v_img_folder",
                                "Show in Folder",
                                Some(Icon::Folder),
                                BtnVariant::Secondary,
                                34.0,
                                None,
                                true,
                                false,
                                palette,
                            )
                            .clicked()
                            {
                                open_explorer_path = Some(path.clone());
                            }
                        });
                    }
                    Some(ViewerContent::Album {
                        path,
                        slide_names,
                        current_idx,
                        ..
                    }) => {
                        let box_sz = Vec2::new(ui.available_width(), stage_h + 28.0);
                        let (rect, _) = ui.allocate_exact_size(box_sz, Sense::hover());
                        ui.painter()
                            .rect_filled(rect, CornerRadius::same(10), palette.input_bg);
                        if let Some(tex) = &self.viewer_texture {
                            let sz = fit_aspect_cinema(tex.size_vec2(), box_sz - Vec2::splat(12.0));
                            egui::Image::new((tex.id(), sz))
                                .corner_radius(CornerRadius::same(8))
                                .paint_at(ui, Rect::from_center_size(rect.center(), sz));
                        }
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            if anim_button(
                                ui,
                                "alb_prev",
                                "Previous",
                                Some(Icon::ChevronLeft),
                                BtnVariant::Secondary,
                                34.0,
                                None,
                                *current_idx > 0,
                                false,
                                palette,
                            )
                            .clicked()
                            {
                                delta_slide = -1;
                            }
                            ui.label(
                                RichText::new(format!("{} / {}", current_idx + 1, slide_names.len()))
                                    .font(font_semibold(13.0))
                                    .color(palette.text),
                            );
                            if anim_button(
                                ui,
                                "alb_next",
                                "Next",
                                Some(Icon::ChevronRight),
                                BtnVariant::Secondary,
                                34.0,
                                None,
                                *current_idx + 1 < slide_names.len(),
                                false,
                                palette,
                            )
                            .clicked()
                            {
                                delta_slide = 1;
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if anim_button(
                                    ui,
                                    "alb_folder",
                                    "Show in Folder",
                                    Some(Icon::Folder),
                                    BtnVariant::Secondary,
                                    34.0,
                                    None,
                                    true,
                                    false,
                                    palette,
                                )
                                .clicked()
                                {
                                    open_explorer_path = Some(path.clone());
                                }
                            });
                        });
                    }
                    Some(ViewerContent::Audio { path, .. }) => {
                        let playing_this = self.audio_player.loaded_path() == Some(path.as_str())
                            && self.audio_player.is_active();
                        let is_playing_now = playing_this && !self.audio_player.is_paused();

                        Frame::new()
                            .fill(palette.input_bg)
                            .stroke(Stroke::new(1.0, palette.border))
                            .corner_radius(CornerRadius::same(12))
                            .inner_margin(Margin::symmetric(24, 52))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.vertical_centered(|ui| {
                                    draw_equalizer_bars(ui, is_playing_now, palette.accent);
                                    ui.add_space(12.0);
                                    ui.label(
                                        RichText::new("Native Rust Audio Player (Symphonia + Rodio)")
                                            .font(font_semibold(13.5))
                                            .color(palette.text),
                                    );
                                    ui.add_space(4.0);
                                    ui.label(
                                        RichText::new(if is_playing_now {
                                            "Playing locally in-app"
                                        } else if playing_this {
                                            "Paused"
                                        } else {
                                            "Ready to play"
                                        })
                                        .font(font_regular(12.0))
                                        .color(palette.muted),
                                    );
                                });
                            });

                        ui.add_space(16.0);
                        ui.horizontal(|ui| {
                            if !playing_this {
                                if anim_button(
                                    ui,
                                    "aud_play",
                                    "Play Audio",
                                    Some(Icon::Play),
                                    BtnVariant::Primary,
                                    36.0,
                                    None,
                                    true,
                                    false,
                                    palette,
                                )
                                .clicked()
                                {
                                    play_audio_path = Some(path.clone());
                                }
                            } else {
                                let (lbl, ic) = if self.audio_player.is_paused() {
                                    ("Resume", Icon::Play)
                                } else {
                                    ("Pause", Icon::Pause)
                                };
                                if anim_button(
                                    ui,
                                    "aud_toggle",
                                    lbl,
                                    Some(ic),
                                    BtnVariant::Primary,
                                    36.0,
                                    None,
                                    true,
                                    false,
                                    palette,
                                )
                                .clicked()
                                {
                                    toggle_audio = true;
                                }
                                if anim_button(
                                    ui,
                                    "aud_stop",
                                    "Stop",
                                    Some(Icon::Stop),
                                    BtnVariant::Secondary,
                                    36.0,
                                    None,
                                    true,
                                    false,
                                    palette,
                                )
                                .clicked()
                                {
                                    stop_audio = true;
                                }
                            }
                            if anim_button(
                                ui,
                                "aud_folder",
                                "Show in Folder",
                                Some(Icon::Folder),
                                BtnVariant::Secondary,
                                36.0,
                                None,
                                true,
                                false,
                                palette,
                            )
                            .clicked()
                            {
                                open_explorer_path = Some(path.clone());
                            }
                        });
                    }
                    Some(ViewerContent::Video {
                        path,
                        probe,
                        thumb_token,
                        ..
                    }) => {
                        let snap = video_snap.clone().unwrap_or_default();
                        let duration = if snap.duration_secs > 0.0 {
                            snap.duration_secs
                        } else {
                            probe
                                .as_ref()
                                .and_then(|p| p.duration_seconds)
                                .unwrap_or(0.0)
                        };

                        // Cinema Video Stage — fits both Landscape 16:9 and Portrait 9:16 cleanly
                        let box_sz = Vec2::new(ui.available_width(), stage_h);
                        let (stage_rect, stage_resp) =
                            ui.allocate_exact_size(box_sz, Sense::click());
                        if stage_resp.hovered() {
                            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if stage_resp.clicked() && snap.error.is_none() {
                            video_toggle_play = true;
                        }

                        let cinema_bg = Color32::from_rgb(5, 7, 12);
                        ui.painter()
                            .rect_filled(stage_rect, CornerRadius::same(12), cinema_bg);

                        if let Some(tex) = &self.viewer_texture {
                            let sz = fit_aspect_cinema(tex.size_vec2(), box_sz - Vec2::splat(6.0));
                            egui::Image::new((tex.id(), sz))
                                .corner_radius(CornerRadius::same(8))
                                .paint_at(ui, Rect::from_center_size(stage_rect.center(), sz));
                        } else if let Some(tex) =
                            thumb_token.as_deref().and_then(|t| self.textures.get(t))
                        {
                            let sz = fit_aspect_cinema(tex.size_vec2(), box_sz - Vec2::splat(6.0));
                            egui::Image::new((tex.id(), sz))
                                .corner_radius(CornerRadius::same(8))
                                .paint_at(ui, Rect::from_center_size(stage_rect.center(), sz));
                            if snap.error.is_none() {
                                let t = ui.input(|i| i.time);
                                draw_spinner(
                                    ui.painter(),
                                    stage_rect.center(),
                                    18.0,
                                    palette.accent,
                                    t,
                                );
                            }
                        } else if snap.error.is_none() {
                            let t = ui.input(|i| i.time);
                            draw_spinner(
                                ui.painter(),
                                stage_rect.center(),
                                18.0,
                                palette.accent,
                                t,
                            );
                        }

                        // Translucent center Play button overlay when paused or ended
                        let show_play_badge =
                            snap.error.is_none() && self.viewer_texture.is_some() && !snap.playing;
                        let badge_alpha = ctx.animate_bool_with_time(
                            Id::new("vid_pause_overlay"),
                            show_play_badge,
                            0.14,
                        );
                        if badge_alpha > 0.02 {
                            ui.painter().circle_filled(
                                stage_rect.center(),
                                30.0,
                                Color32::from_rgba_unmultiplied(
                                    9,
                                    11,
                                    17,
                                    (badge_alpha * 185.0) as u8,
                                ),
                            );
                            ui.painter().circle_stroke(
                                stage_rect.center(),
                                30.0,
                                Stroke::new(
                                    1.4,
                                    with_alpha(Color32::WHITE, (badge_alpha * 130.0) as u8),
                                ),
                            );
                            draw_icon(
                                ui.painter(),
                                Rect::from_center_size(
                                    stage_rect.center() + Vec2::new(2.0, 0.0),
                                    Vec2::splat(22.0),
                                ),
                                Icon::Play,
                                with_alpha(Color32::WHITE, (badge_alpha * 255.0) as u8),
                            );
                        }

                        ui.painter().rect_stroke(
                            stage_rect,
                            CornerRadius::same(12),
                            Stroke::new(1.0, palette.border),
                            StrokeKind::Inside,
                        );

                        if let Some(err) = &snap.error {
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                let (ir, _) =
                                    ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                                draw_icon(ui.painter(), ir, Icon::Warning, palette.warn);
                                ui.label(
                                    RichText::new(err)
                                        .font(font_regular(12.0))
                                        .color(palette.warn),
                                );
                            });
                        } else {
                            // Interactive Seek Timeline Scrubber
                            ui.add_space(10.0);
                            let scrub_w = ui.available_width();
                            let (scrub_rect, scrub_resp) =
                                ui.allocate_exact_size(Vec2::new(scrub_w, 16.0), Sense::click_and_drag());
                            if scrub_resp.hovered() || scrub_resp.dragged() {
                                ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                            }
                            let frac = if duration > 0.0 {
                                (snap.position_secs / duration).clamp(0.0, 1.0) as f32
                            } else {
                                0.0
                            };
                            let track_h = 5.0;
                            let track_rect = Rect::from_center_size(
                                scrub_rect.center(),
                                Vec2::new(scrub_w, track_h),
                            );
                            ui.painter()
                                .rect_filled(track_rect, CornerRadius::same(3), palette.inset);
                            let fill_w = (scrub_w * frac).max(4.0);
                            let fill_rect =
                                Rect::from_min_size(track_rect.min, Vec2::new(fill_w, track_h));
                            ui.painter()
                                .rect_filled(fill_rect, CornerRadius::same(3), palette.accent);

                            let thumb_hov = ctx.animate_bool_with_time(
                                Id::new("vid_scrub_hov"),
                                scrub_resp.hovered() || scrub_resp.dragged(),
                                0.12,
                            );
                            let thumb_x = track_rect.left() + scrub_w * frac;
                            let thumb_r = 5.5 + thumb_hov * 2.5;
                            ui.painter().circle_filled(
                                Pos2::new(thumb_x, track_rect.center().y),
                                thumb_r + 2.5,
                                with_alpha(palette.accent, (thumb_hov * 75.0) as u8),
                            );
                            ui.painter().circle_filled(
                                Pos2::new(thumb_x, track_rect.center().y),
                                thumb_r,
                                Color32::WHITE,
                            );

                            if (scrub_resp.clicked() || scrub_resp.dragged()) && duration > 0.0 {
                                if let Some(ptr) = scrub_resp.interact_pointer_pos() {
                                    let next_frac =
                                        ((ptr.x - track_rect.left()) / scrub_w).clamp(0.0, 1.0);
                                    video_seek_to = Some(next_frac as f64 * duration);
                                }
                            }
                        }

                        // Bottom Controls Bar
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            if snap.error.is_none() {
                                let (play_lbl, play_ic) = if snap.playing {
                                    ("Pause", Icon::Pause)
                                } else if snap.ended {
                                    ("Replay", Icon::Retry)
                                } else {
                                    ("Play", Icon::Play)
                                };
                                if anim_button(
                                    ui,
                                    "vid_play_toggle",
                                    play_lbl,
                                    Some(play_ic),
                                    BtnVariant::Primary,
                                    34.0,
                                    Some(92.0),
                                    true,
                                    false,
                                    palette,
                                )
                                .clicked()
                                {
                                    video_toggle_play = true;
                                }

                                ui.label(
                                    RichText::new(format!(
                                        "{} / {}",
                                        nice_duration(snap.position_secs),
                                        nice_duration(duration)
                                    ))
                                    .font(font_semibold(12.0))
                                    .color(palette.text_secondary),
                                );

                                if snap.has_audio {
                                    let vol_icon = if snap.muted || snap.volume <= 0.01 {
                                        Icon::VolumeMute
                                    } else {
                                        Icon::Volume
                                    };
                                    if anim_button(
                                        ui,
                                        "vid_mute_btn",
                                        "",
                                        Some(vol_icon),
                                        BtnVariant::Secondary,
                                        32.0,
                                        Some(32.0),
                                        true,
                                        false,
                                        palette,
                                    )
                                    .clicked()
                                    {
                                        video_set_muted = Some(!snap.muted);
                                    }

                                    // Compact sleek volume bar
                                    let vol_w = 72.0;
                                    let (vol_rect, vol_resp) = ui.allocate_exact_size(
                                        Vec2::new(vol_w, 16.0),
                                        Sense::click_and_drag(),
                                    );
                                    if vol_resp.hovered() || vol_resp.dragged() {
                                        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                                    }
                                    let eff_vol = if snap.muted { 0.0 } else { snap.volume };
                                    let vt = Rect::from_center_size(
                                        vol_rect.center(),
                                        Vec2::new(vol_w, 4.0),
                                    );
                                    ui.painter().rect_filled(vt, CornerRadius::same(2), palette.inset);
                                    let vf = Rect::from_min_size(
                                        vt.min,
                                        Vec2::new((vol_w * eff_vol).max(2.0), 4.0),
                                    );
                                    ui.painter().rect_filled(vf, CornerRadius::same(2), palette.accent);
                                    ui.painter().circle_filled(
                                        Pos2::new(vt.left() + vol_w * eff_vol, vt.center().y),
                                        4.5,
                                        Color32::WHITE,
                                    );
                                    if vol_resp.clicked() || vol_resp.dragged() {
                                        if let Some(ptr) = vol_resp.interact_pointer_pos() {
                                            let v = ((ptr.x - vt.left()) / vol_w).clamp(0.0, 1.0);
                                            video_set_volume = Some(v);
                                            if snap.muted && v > 0.02 {
                                                video_set_muted = Some(false);
                                            }
                                        }
                                    }
                                }
                            }

                            if let Some(info) = probe {
                                if let (Some(w), Some(h)) = (info.width, info.height) {
                                    status_pill(
                                        ui,
                                        &format!("{w}×{h}"),
                                        palette.accent,
                                        palette.accent_soft,
                                        None,
                                        false,
                                    );
                                }
                                if let Some(vc) = &info.video_codec {
                                    subtle_tag(ui, vc, palette);
                                }
                            }

                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if anim_button(
                                    ui,
                                    "vid_folder",
                                    "Folder",
                                    Some(Icon::Folder),
                                    BtnVariant::Secondary,
                                    34.0,
                                    None,
                                    true,
                                    false,
                                    palette,
                                )
                                .clicked()
                                {
                                    open_explorer_path = Some(path.clone());
                                }
                                if anim_button(
                                    ui,
                                    "vid_play_ext",
                                    "External",
                                    Some(Icon::External),
                                    BtnVariant::Secondary,
                                    34.0,
                                    None,
                                    true,
                                    false,
                                    palette,
                                )
                                .clicked()
                                {
                                    open_player_path = Some(path.clone());
                                }
                            });
                        });
                    }
                    Some(ViewerContent::Error(msg)) => {
                        ui.horizontal(|ui| {
                            let (ir, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                            draw_icon(ui.painter(), ir, Icon::Warning, palette.error);
                            ui.label(RichText::new(msg).font(font_regular(13.0)).color(palette.error));
                        });
                    }
                    None => {}
                }
            },
        );

        if video_toggle_play {
            self.video_player.toggle_play();
        }
        if let Some(t) = video_seek_to {
            self.video_player.seek(t);
        }
        if let Some(v) = video_set_volume {
            self.video_player.set_volume(v);
        }
        if let Some(m) = video_set_muted {
            self.video_player.set_muted(m);
        }
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
        if !open && self.viewer_open {
            self.viewer_open = false;
            self.viewer_content = None;
            self.viewer_texture = None;
            self.video_frame_seq = 0;
            self.audio_player.stop();
            self.video_player.stop();
        }
    }

    fn render_settings_modal(&mut self, ctx: &egui::Context, palette: &Palette) {
        let mut open = self.settings_open;
        let mut save_clicked = false;

        show_animated_modal(
            ctx,
            "modal_settings",
            "Settings & Engine",
            Some("Configure default download preferences and inspect the native Rust engine"),
            Vec2::new(710.0, 395.0),
            palette,
            &mut open,
            |ui| {
                ui.columns(2, |cols| {
                    cols[0].vertical(|ui| {
                        ui.label(
                            RichText::new("Preferences")
                                .font(font_semibold(14.0))
                                .color(palette.text),
                        );
                        ui.add_space(10.0);

                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Default video quality").color(palette.muted));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                egui::ComboBox::from_id_salt("pref_q")
                                    .selected_text(quality_label(&self.settings_draft.default_video_quality))
                                    .width(136.0)
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
                        });
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Default audio format").color(palette.muted));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                egui::ComboBox::from_id_salt("pref_a")
                                    .selected_text(audio_format_label(&self.settings_draft.default_audio_format))
                                    .width(136.0)
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
                        });
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Appearance").color(palette.muted));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                egui::ComboBox::from_id_salt("pref_t")
                                    .selected_text(theme_label(&self.settings_draft.theme))
                                    .width(136.0)
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
                        });

                        ui.add_space(10.0);
                        ui.label(
                            RichText::new("Default download folder")
                                .font(font_semibold(12.0))
                                .color(palette.text_secondary),
                        );
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            Frame::new()
                                .fill(palette.input_bg)
                                .stroke(Stroke::new(1.0, palette.border))
                                .corner_radius(CornerRadius::same(8))
                                .inner_margin(Margin::symmetric(9, 5))
                                .show(ui, |ui| {
                                    ui.set_width(214.0);
                                    ui.add(
                                        egui::TextEdit::singleline(&mut self.settings_draft.download_dir)
                                            .frame(false)
                                            .font(font_regular(12.0)),
                                    );
                                });
                            if anim_button(
                                ui,
                                "pref_browse",
                                "Browse",
                                Some(Icon::Folder),
                                BtnVariant::Secondary,
                                30.0,
                                None,
                                true,
                                false,
                                palette,
                            )
                            .clicked()
                            {
                                let mut dlg = rfd::FileDialog::new()
                                    .set_title("Choose default download folder");
                                let cur = PathBuf::from(&self.settings_draft.download_dir);
                                if cur.is_dir() {
                                    dlg = dlg.set_directory(cur);
                                }
                                if let Some(picked) = dlg.pick_folder() {
                                    self.settings_draft.download_dir = picked.display().to_string();
                                }
                            }
                        });

                        ui.add_space(16.0);
                        if anim_button(
                            ui,
                            "pref_save",
                            "Save Preferences",
                            Some(Icon::Check),
                            BtnVariant::Primary,
                            36.0,
                            Some(160.0),
                            true,
                            false,
                            palette,
                        )
                        .clicked()
                        {
                            save_clicked = true;
                        }
                    });

                    cols[1].vertical(|ui| {
                        ui.label(
                            RichText::new("Native Rust Engine")
                                .font(font_semibold(14.0))
                                .color(palette.text),
                        );
                        ui.add_space(10.0);
                        Frame::new()
                            .fill(palette.input_bg)
                            .stroke(Stroke::new(1.0, palette.border))
                            .corner_radius(CornerRadius::same(10))
                            .inner_margin(Margin::same(12))
                            .show(ui, |ui| {
                                for (k, v) in [
                                    ("Core & GUI", "100% Pure Rust (eframe/egui)"),
                                    ("7-Platform Extractors", "Built-in Native Rust"),
                                    ("MP4 / fMP4 Muxer", "Built-in ISO-BMFF Engine"),
                                    ("Audio Engine", "Symphonia + Rodio"),
                                    ("External Tools", "0 (No yt-dlp / FFmpeg / Node)"),
                                ] {
                                    ui.horizontal(|ui| {
                                        let (ir, _) =
                                            ui.allocate_exact_size(Vec2::splat(13.0), Sense::hover());
                                        draw_icon(ui.painter(), ir, Icon::Check, palette.good);
                                        ui.label(
                                            RichText::new(k)
                                                .font(font_regular(12.0))
                                                .color(palette.muted),
                                        );
                                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                            ui.label(
                                                RichText::new(v)
                                                    .font(font_semibold(11.5))
                                                    .color(palette.good),
                                            );
                                        });
                                    });
                                    ui.add_space(4.0);
                                }
                            });

                        ui.add_space(10.0);
                        ui.label(
                            RichText::new(format!("SQLite Database:\n{}", self.db_path.display()))
                                .font(font_regular(11.0))
                                .color(palette.muted),
                        );
                    });
                });
            },
        );

        if save_clicked {
            match self.db.save_preferences(&self.settings_draft) {
                Ok(saved) => {
                    self.prefs = saved;
                    open = false;
                    self.show_toast("Preferences saved.", false);
                }
                Err(e) => self.show_toast(e.to_string(), true),
            }
        }
        self.settings_open = open;
    }

    fn render_delete_modal(&mut self, ctx: &egui::Context, palette: &Palette) {
        let Some(target) = self.delete_target.clone() else {
            let mut closed = false;
            show_animated_modal(
                ctx,
                "modal_delete",
                "Delete Download",
                None,
                Vec2::new(430.0, 195.0),
                palette,
                &mut closed,
                |_| {},
            );
            return;
        };
        let mut open = true;
        let mut confirm = false;
        let mut cancel = false;

        show_animated_modal(
            ctx,
            "modal_delete",
            "Delete Download Entry",
            Some("Remove this item from your local history"),
            Vec2::new(430.0, 200.0),
            palette,
            &mut open,
            |ui| {
                let name = target
                    .title
                    .as_deref()
                    .or(target.output_filename.as_deref())
                    .unwrap_or(&target.platform);
                let short: String = name.chars().take(65).collect();
                ui.label(
                    RichText::new(short)
                        .font(font_semibold(13.0))
                        .color(palette.text_secondary),
                );
                if target.file_available {
                    ui.add_space(10.0);
                    ui.checkbox(
                        &mut self.delete_file_checked,
                        "Also permanently delete the file from disk",
                    );
                }
                ui.add_space(18.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if anim_button(
                        ui,
                        "del_confirm",
                        "Delete",
                        Some(Icon::Trash),
                        BtnVariant::DangerSolid,
                        34.0,
                        None,
                        true,
                        false,
                        palette,
                    )
                    .clicked()
                    {
                        confirm = true;
                    }
                    if anim_button(
                        ui,
                        "del_cancel",
                        "Cancel",
                        None,
                        BtnVariant::Secondary,
                        34.0,
                        None,
                        true,
                        false,
                        palette,
                    )
                    .clicked()
                    {
                        cancel = true;
                    }
                });
            },
        );

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
                    self.show_toast("Download entry deleted.", false);
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
        let elapsed = t.created_at.elapsed().as_secs_f32();
        let total = t.duration.as_secs_f32().max(0.5);
        let enter_t = ease_out_cubic((elapsed / 0.22).min(1.0));
        let remain_frac = (1.0 - elapsed / total).clamp(0.0, 1.0);
        let slide_y = (1.0 - enter_t) * 18.0;

        let msg = t.message.clone();
        let is_err = t.is_error;
        let accent = if is_err { palette.error } else { palette.good };
        let mut dismiss = false;

        ctx.request_repaint();

        egui::Area::new(Id::new("toast_overlay"))
            .anchor(Align2::RIGHT_BOTTOM, Vec2::new(-22.0, -44.0 + slide_y))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let frame_resp = Frame::new()
                    .fill(palette.surface_elevated)
                    .stroke(Stroke::new(1.0, with_alpha(accent, 110)))
                    .corner_radius(CornerRadius::same(11))
                    .inner_margin(Margin::symmetric(14, 10))
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 10],
                        blur: 28,
                        spread: 0,
                        color: Color32::from_rgba_unmultiplied(0, 0, 0, if palette.dark { 140 } else { 36 }),
                    })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (ir, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                            draw_icon(
                                ui.painter(),
                                ir,
                                if is_err { Icon::Warning } else { Icon::Check },
                                accent,
                            );
                            ui.label(
                                RichText::new(&msg)
                                    .font(font_semibold(12.5))
                                    .color(palette.text),
                            );
                            ui.add_space(4.0);
                            if anim_button(
                                ui,
                                "toast_close",
                                "",
                                Some(Icon::Close),
                                BtnVariant::Ghost,
                                22.0,
                                Some(22.0),
                                true,
                                false,
                                palette,
                            )
                            .clicked()
                            {
                                dismiss = true;
                            }
                        });
                    });

                // Subtle bottom countdown bar
                let r = frame_resp.response.rect;
                let bar_w = (r.width() - 20.0).max(0.0) * remain_frac;
                let bar = Rect::from_min_size(
                    Pos2::new(r.left() + 10.0, r.bottom() - 2.5),
                    Vec2::new(bar_w, 2.5),
                );
                ui.painter()
                    .rect_filled(bar, CornerRadius::same(2), with_alpha(accent, 190));
            });

        if dismiss {
            self.toast = None;
        }
    }
}

// ============================================================================
// Formatting & Media Helpers (100% English)
// ============================================================================

fn status_style<'a>(status: &'a str, p: &Palette) -> (&'a str, Color32, Color32, Option<Icon>, bool) {
    match status {
        "queued" => ("Queued", p.accent, p.accent_soft, None, true),
        "downloading" => ("Downloading", p.accent, p.accent_soft, None, true),
        "processing" => ("Processing", p.warn, p.warn_soft, None, true),
        "completed" => ("Completed", p.good, p.good_soft, Some(Icon::Check), false),
        "failed" => ("Failed", p.error, p.error_soft, Some(Icon::Warning), false),
        "cancelled" => ("Cancelled", p.muted, p.inset, Some(Icon::Close), false),
        "interrupted" => ("Interrupted", p.warn, p.warn_soft, Some(Icon::Warning), false),
        _ => (status, p.muted, p.inset, None, false),
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
        "Best available".into()
    } else {
        format!("{q}p")
    }
}

fn audio_format_label(a: &str) -> &'static str {
    match a {
        "wav" => "WAV (16-bit PCM)",
        _ => "M4A (Lossless AAC)",
    }
}

fn image_format_label(f: &str, count: usize) -> String {
    if f == "zip" {
        format!("All photos ({count} slides · ZIP)")
    } else {
        f.to_uppercase()
    }
}

fn theme_label(t: &str) -> &'static str {
    match t {
        "light" => "Light",
        "system" => "System",
        _ => "Dark",
    }
}

fn kind_label(job: &JobRecord) -> String {
    if job.kind == "video" {
        let q = if job.quality == "best" {
            "Best".into()
        } else {
            format!("{}p", job.quality)
        };
        format!("Video · {} {q}", job.container.to_uppercase())
    } else if job.kind == "audio" {
        format!("Audio · {}", job.container.to_uppercase())
    } else if job.container == "zip" {
        "Photos · ZIP Album".into()
    } else {
        format!("Photo · {}", job.container.to_uppercase())
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

/// Scales media cleanly to fill the cinema preview stage while preserving exact aspect ratio
/// (so both widescreen 16:9 landscape and tall 9:16 portrait videos fill the stage crisply).
fn fit_aspect_cinema(src: Vec2, max: Vec2) -> Vec2 {
    if src.x <= 0.0 || src.y <= 0.0 {
        return max;
    }
    let scale = (max.x / src.x).min(max.y / src.y).min(2.5);
    Vec2::new((src.x * scale).max(1.0), (src.y * scale).max(1.0))
}

fn load_zip_slide_names(zip_path: &Path) -> Result<Vec<String>, AppError> {
    let file = std::fs::File::open(zip_path).map_err(|_| AppError::new("FILE_NOT_FOUND"))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|_| AppError::new("FORMAT_UNAVAILABLE"))?;
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
    let mut archive =
        zip::ZipArchive::new(file).map_err(|_| AppError::new("FORMAT_UNAVAILABLE"))?;
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
