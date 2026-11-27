//! Modern Pure-Rust Design System, Vector Icons, Smooth Animations & Windows 11 DWM Integration.
//! Zero external icon fonts, zero missing-glyph boxes, silky 60fps+ spring/ease transitions.

use egui::{
    epaint::Shadow, Align2, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId,
    Frame, Id, Margin, Painter, Pos2, Rect, Response, RichText, Sense, Shape, Stroke, StrokeKind,
    Ui, Vec2,
};

// ============================================================================
// Windows 11 Seamless DWM Titlebar Integration (No Extra Crates)
// ============================================================================

#[cfg(windows)]
mod win_dwm {
    use egui::Color32;
    use std::ffi::c_void;

    const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
    const DWMWA_BORDER_COLOR: u32 = 34;
    const DWMWA_CAPTION_COLOR: u32 = 35;
    const DWMWA_TEXT_COLOR: u32 = 36;

    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: *mut c_void,
            dw_attribute: u32,
            pv_attribute: *const c_void,
            cb_attribute: u32,
        ) -> i32;
    }

    #[link(name = "user32")]
    extern "system" {
        fn GetActiveWindow() -> *mut c_void;
        fn FindWindowW(lp_class_name: *const u16, lp_window_name: *const u16) -> *mut c_void;
    }

    fn colorref(c: Color32) -> u32 {
        (c.r() as u32) | ((c.g() as u32) << 8) | ((c.b() as u32) << 16)
    }

    pub fn sync_titlebar(dark: bool, caption: Color32, text: Color32, border: Color32) {
        unsafe {
            let mut hwnd = GetActiveWindow();
            if hwnd.is_null() {
                let title: Vec<u16> = "MediaVault\0".encode_utf16().collect();
                hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
            }
            if hwnd.is_null() {
                return;
            }
            let dark_val: u32 = if dark { 1 } else { 0 };
            let cap_val = colorref(caption);
            let txt_val = colorref(text);
            let brd_val = colorref(border);

            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                (&dark_val as *const u32).cast(),
                std::mem::size_of::<u32>() as u32,
            );
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_CAPTION_COLOR,
                (&cap_val as *const u32).cast(),
                std::mem::size_of::<u32>() as u32,
            );
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_TEXT_COLOR,
                (&txt_val as *const u32).cast(),
                std::mem::size_of::<u32>() as u32,
            );
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_BORDER_COLOR,
                (&brd_val as *const u32).cast(),
                std::mem::size_of::<u32>() as u32,
            );
        }
    }
}

pub fn sync_window_chrome(dark: bool, p: &Palette) {
    #[cfg(windows)]
    win_dwm::sync_titlebar(dark, p.header_bg, p.text, p.border);
    #[cfg(not(windows))]
    let _ = (dark, p);
}

// ============================================================================
// Crisp Native System Typography (Segoe UI Variable / Segoe UI)
// ============================================================================

pub fn install_modern_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    #[cfg(windows)]
    {
        let regular_candidates = [
            r"C:\Windows\Fonts\segoeui.ttf",
            r"C:\Windows\Fonts\SegUIVar.ttf",
        ];
        for path in regular_candidates {
            if let Ok(bytes) = std::fs::read(path) {
                fonts
                    .font_data
                    .insert("ui_regular".into(), FontData::from_owned(bytes).into());
                if let Some(fam) = fonts.families.get_mut(&FontFamily::Proportional) {
                    fam.insert(0, "ui_regular".into());
                }
                break;
            }
        }

        let semibold_candidates = [
            r"C:\Windows\Fonts\seguisb.ttf",
            r"C:\Windows\Fonts\segoeuib.ttf",
        ];
        for path in semibold_candidates {
            if let Ok(bytes) = std::fs::read(path) {
                fonts
                    .font_data
                    .insert("ui_semibold".into(), FontData::from_owned(bytes).into());
                fonts.families.insert(
                    FontFamily::Name("semibold".into()),
                    vec!["ui_semibold".into(), "ui_regular".into()],
                );
                break;
            }
        }
    }

    if !fonts.families.contains_key(&FontFamily::Name("semibold".into())) {
        let fallback = fonts
            .families
            .get(&FontFamily::Proportional)
            .cloned()
            .unwrap_or_default();
        fonts
            .families
            .insert(FontFamily::Name("semibold".into()), fallback);
    }

    ctx.set_fonts(fonts);
}

pub fn font_semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

pub fn font_regular(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

// ============================================================================
// Modern Color Palette (Obsidian SaaS Dark + Crisp Daylight)
// ============================================================================

#[derive(Clone, Copy)]
pub struct Palette {
    pub dark: bool,
    pub bg: Color32,
    pub header_bg: Color32,
    pub surface: Color32,
    pub surface_hover: Color32,
    pub surface_elevated: Color32,
    pub input_bg: Color32,
    pub inset: Color32,
    pub border: Color32,
    pub border_hover: Color32,
    pub text: Color32,
    pub text_secondary: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_soft: Color32,
    pub accent_border: Color32,
    pub good: Color32,
    pub good_soft: Color32,
    pub warn: Color32,
    pub warn_soft: Color32,
    pub error: Color32,
    pub error_soft: Color32,
    pub scrim: Color32,
}

impl Palette {
    pub fn for_mode(dark: bool) -> Self {
        if dark {
            Self {
                dark: true,
                bg: Color32::from_rgb(9, 11, 17),
                header_bg: Color32::from_rgb(13, 16, 25),
                surface: Color32::from_rgb(17, 21, 33),
                surface_hover: Color32::from_rgb(23, 29, 45),
                surface_elevated: Color32::from_rgb(22, 27, 42),
                input_bg: Color32::from_rgb(12, 15, 24),
                inset: Color32::from_rgb(26, 32, 50),
                border: Color32::from_rgba_unmultiplied(148, 163, 198, 28),
                border_hover: Color32::from_rgba_unmultiplied(148, 163, 198, 62),
                text: Color32::from_rgb(241, 245, 252),
                text_secondary: Color32::from_rgb(203, 213, 232),
                muted: Color32::from_rgb(136, 149, 176),
                accent: Color32::from_rgb(99, 102, 241),
                accent_hover: Color32::from_rgb(120, 124, 250),
                accent_soft: Color32::from_rgba_unmultiplied(99, 102, 241, 34),
                accent_border: Color32::from_rgba_unmultiplied(129, 140, 248, 85),
                good: Color32::from_rgb(52, 211, 153),
                good_soft: Color32::from_rgba_unmultiplied(16, 185, 129, 32),
                warn: Color32::from_rgb(251, 191, 36),
                warn_soft: Color32::from_rgba_unmultiplied(245, 158, 11, 32),
                error: Color32::from_rgb(248, 113, 113),
                error_soft: Color32::from_rgba_unmultiplied(239, 68, 68, 34),
                scrim: Color32::from_rgba_unmultiplied(4, 6, 12, 175),
            }
        } else {
            Self {
                dark: false,
                bg: Color32::from_rgb(244, 246, 251),
                header_bg: Color32::from_rgb(255, 255, 255),
                surface: Color32::from_rgb(255, 255, 255),
                surface_hover: Color32::from_rgb(247, 249, 254),
                surface_elevated: Color32::from_rgb(255, 255, 255),
                input_bg: Color32::from_rgb(245, 247, 252),
                inset: Color32::from_rgb(238, 242, 249),
                border: Color32::from_rgba_unmultiplied(15, 23, 42, 24),
                border_hover: Color32::from_rgba_unmultiplied(15, 23, 42, 52),
                text: Color32::from_rgb(15, 23, 42),
                text_secondary: Color32::from_rgb(51, 65, 85),
                muted: Color32::from_rgb(100, 116, 139),
                accent: Color32::from_rgb(79, 70, 229),
                accent_hover: Color32::from_rgb(67, 56, 202),
                accent_soft: Color32::from_rgba_unmultiplied(79, 70, 229, 24),
                accent_border: Color32::from_rgba_unmultiplied(79, 70, 229, 68),
                good: Color32::from_rgb(5, 150, 105),
                good_soft: Color32::from_rgba_unmultiplied(16, 185, 129, 26),
                warn: Color32::from_rgb(217, 119, 6),
                warn_soft: Color32::from_rgba_unmultiplied(245, 158, 11, 28),
                error: Color32::from_rgb(220, 38, 38),
                error_soft: Color32::from_rgba_unmultiplied(239, 68, 68, 24),
                scrim: Color32::from_rgba_unmultiplied(15, 23, 42, 120),
            }
        }
    }

    pub fn platform_color(&self, platform: &str) -> Color32 {
        match platform {
            "youtube" => Color32::from_rgb(239, 68, 68),
            "instagram" => Color32::from_rgb(236, 72, 153),
            "tiktok" => {
                if self.dark {
                    Color32::from_rgb(45, 212, 191)
                } else {
                    Color32::from_rgb(13, 148, 136)
                }
            }
            "x" => {
                if self.dark {
                    Color32::from_rgb(203, 213, 225)
                } else {
                    Color32::from_rgb(30, 41, 59)
                }
            }
            "threads" => {
                if self.dark {
                    Color32::from_rgb(192, 132, 252)
                } else {
                    Color32::from_rgb(126, 34, 206)
                }
            }
            "facebook" => Color32::from_rgb(59, 130, 246),
            "pinterest" => Color32::from_rgb(225, 29, 72),
            _ => self.muted,
        }
    }
}

pub fn apply_modern_theme(ctx: &egui::Context, theme: &str) {
    let is_light = theme == "light";
    let mut visuals = if is_light {
        egui::Visuals::light()
    } else {
        egui::Visuals::dark()
    };
    let p = Palette::for_mode(!is_light);

    visuals.panel_fill = p.bg;
    visuals.window_fill = p.surface_elevated;
    visuals.extreme_bg_color = p.input_bg;
    visuals.faint_bg_color = p.inset;
    visuals.window_corner_radius = CornerRadius::same(14);
    visuals.window_stroke = Stroke::new(1.0, p.border_hover);
    visuals.window_shadow = Shadow {
        offset: [0, 14],
        blur: 36,
        spread: 0,
        color: Color32::from_rgba_unmultiplied(0, 0, 0, if p.dark { 150 } else { 42 }),
    };
    visuals.popup_shadow = Shadow {
        offset: [0, 8],
        blur: 20,
        spread: 0,
        color: Color32::from_rgba_unmultiplied(0, 0, 0, if p.dark { 120 } else { 32 }),
    };
    visuals.menu_corner_radius = CornerRadius::same(10);

    visuals.widgets.noninteractive.bg_fill = p.surface;
    visuals.widgets.noninteractive.weak_bg_fill = p.surface;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text_secondary);
    visuals.widgets.noninteractive.corner_radius = CornerRadius::same(8);

    visuals.widgets.inactive.bg_fill = p.inset;
    visuals.widgets.inactive.weak_bg_fill = p.inset;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, p.border);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, p.text);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(8);

    visuals.widgets.hovered.bg_fill = p.surface_hover;
    visuals.widgets.hovered.weak_bg_fill = p.surface_hover;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, p.border_hover);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, p.text);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(8);
    visuals.widgets.hovered.expansion = 0.0;

    visuals.widgets.active.bg_fill = p.accent_soft;
    visuals.widgets.active.weak_bg_fill = p.accent_soft;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, p.accent);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, p.text);
    visuals.widgets.active.corner_radius = CornerRadius::same(8);
    visuals.widgets.active.expansion = 0.0;

    visuals.widgets.open.bg_fill = p.surface_hover;
    visuals.widgets.open.weak_bg_fill = p.surface_hover;
    visuals.widgets.open.bg_stroke = Stroke::new(1.0, p.accent_border);
    visuals.widgets.open.fg_stroke = Stroke::new(1.0, p.text);
    visuals.widgets.open.corner_radius = CornerRadius::same(8);

    visuals.selection.bg_fill = p.accent_soft;
    visuals.selection.stroke = Stroke::new(1.0, p.accent);

    let mut style = (*ctx.style()).clone();
    style.visuals = visuals;
    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.button_padding = Vec2::new(12.0, 6.0);
    style.spacing.combo_width = 148.0;
    style.spacing.scroll.bar_width = 6.0;
    style.spacing.scroll.floating = true;
    style.animation_time = 0.16;
    ctx.set_style(style);

    sync_window_chrome(!is_light, &p);
}

// ============================================================================
// Color & Easing Math
// ============================================================================

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| -> u8 { (x as f32 + (y as f32 - x as f32) * t).round() as u8 };
    Color32::from_rgba_unmultiplied(
        mix(a.r(), b.r()),
        mix(a.g(), b.g()),
        mix(a.b(), b.b()),
        mix(a.a(), b.a()),
    )
}

pub fn with_alpha(c: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha)
}

pub fn ease_out_cubic(t: f32) -> f32 {
    let u = 1.0 - t.clamp(0.0, 1.0);
    1.0 - u * u * u
}

// ============================================================================
// Pure-Rust Anti-Aliased Vector Icon System (Zero Unicode Glyph Boxes!)
// ============================================================================

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Check,
    Folder,
    Settings,
    Paste,
    Search,
    Sparkle,
    Play,
    Pause,
    Stop,
    Eye,
    Move,
    Retry,
    Trash,
    Close,
    Download,
    Video,
    Audio,
    Photo,
    Warning,
    Sun,
    Moon,
    ChevronLeft,
    ChevronRight,
    DoubleLeft,
    DoubleRight,
    External,
    Volume,
    VolumeMute,
}

pub fn draw_icon(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let c = rect.center();
    let s = rect.width().min(rect.height()) * 0.5;
    let sw = (s * 0.24).clamp(1.25, 2.0);
    let stroke = Stroke::new(sw, color);

    match icon {
        Icon::Check => {
            let pts = vec![
                Pos2::new(c.x - s * 0.62, c.y + s * 0.04),
                Pos2::new(c.x - s * 0.16, c.y + s * 0.48),
                Pos2::new(c.x + s * 0.65, c.y - s * 0.44),
            ];
            painter.add(Shape::line(pts, stroke));
        }
        Icon::Folder => {
            let body = Rect::from_min_max(
                Pos2::new(c.x - s * 0.78, c.y - s * 0.36),
                Pos2::new(c.x + s * 0.78, c.y + s * 0.62),
            );
            let tab = Rect::from_min_max(
                Pos2::new(c.x - s * 0.78, c.y - s * 0.62),
                Pos2::new(c.x - s * 0.08, c.y - s * 0.26),
            );
            painter.rect_stroke(tab, CornerRadius::same(2), stroke, StrokeKind::Inside);
            painter.rect_stroke(body, CornerRadius::same(2), stroke, StrokeKind::Inside);
        }
        Icon::Settings => {
            // Sleek 3-line equalizer sliders icon (Linear/Raycast style)
            for (dy, knob_x) in [(-0.46, 0.28), (0.0, -0.26), (0.46, 0.18)] {
                let y = c.y + s * dy;
                painter.line_segment(
                    [Pos2::new(c.x - s * 0.75, y), Pos2::new(c.x + s * 0.75, y)],
                    Stroke::new(sw * 0.9, with_alpha(color, 185)),
                );
                painter.circle_filled(Pos2::new(c.x + s * knob_x, y), s * 0.22, color);
            }
        }
        Icon::Paste => {
            let board = Rect::from_min_max(
                Pos2::new(c.x - s * 0.60, c.y - s * 0.52),
                Pos2::new(c.x + s * 0.60, c.y + s * 0.76),
            );
            let clip = Rect::from_min_max(
                Pos2::new(c.x - s * 0.30, c.y - s * 0.76),
                Pos2::new(c.x + s * 0.30, c.y - s * 0.36),
            );
            painter.rect_stroke(board, CornerRadius::same(2), stroke, StrokeKind::Inside);
            painter.rect_filled(clip, CornerRadius::same(2), color);
        }
        Icon::Search => {
            let center = Pos2::new(c.x - s * 0.14, c.y - s * 0.14);
            let r = s * 0.50;
            painter.circle_stroke(center, r, stroke);
            painter.line_segment(
                [
                    Pos2::new(center.x + r * 0.72, center.y + r * 0.72),
                    Pos2::new(c.x + s * 0.74, c.y + s * 0.74),
                ],
                Stroke::new(sw * 1.15, color),
            );
        }
        Icon::Sparkle => {
            // 4-point diamond star
            let pts = vec![
                Pos2::new(c.x, c.y - s * 0.80),
                Pos2::new(c.x + s * 0.24, c.y - s * 0.24),
                Pos2::new(c.x + s * 0.80, c.y),
                Pos2::new(c.x + s * 0.24, c.y + s * 0.24),
                Pos2::new(c.x, c.y + s * 0.80),
                Pos2::new(c.x - s * 0.24, c.y + s * 0.24),
                Pos2::new(c.x - s * 0.80, c.y),
                Pos2::new(c.x - s * 0.24, c.y - s * 0.24),
            ];
            painter.add(Shape::convex_polygon(pts, color, Stroke::NONE));
        }
        Icon::Play => {
            let pts = vec![
                Pos2::new(c.x - s * 0.42, c.y - s * 0.62),
                Pos2::new(c.x + s * 0.65, c.y),
                Pos2::new(c.x - s * 0.42, c.y + s * 0.62),
            ];
            painter.add(Shape::convex_polygon(pts, color, Stroke::NONE));
        }
        Icon::Pause => {
            let w = s * 0.28;
            let h = s * 0.62;
            painter.rect_filled(
                Rect::from_min_max(Pos2::new(c.x - s * 0.46, c.y - h), Pos2::new(c.x - s * 0.46 + w, c.y + h)),
                CornerRadius::same(1),
                color,
            );
            painter.rect_filled(
                Rect::from_min_max(Pos2::new(c.x + s * 0.46 - w, c.y - h), Pos2::new(c.x + s * 0.46, c.y + h)),
                CornerRadius::same(1),
                color,
            );
        }
        Icon::Stop => {
            let r = Rect::from_center_size(c, Vec2::splat(s * 1.1));
            painter.rect_filled(r, CornerRadius::same(2), color);
        }
        Icon::Eye => {
            let top = vec![
                Pos2::new(c.x - s * 0.82, c.y),
                Pos2::new(c.x - s * 0.35, c.y - s * 0.48),
                Pos2::new(c.x + s * 0.35, c.y - s * 0.48),
                Pos2::new(c.x + s * 0.82, c.y),
            ];
            let bot = vec![
                Pos2::new(c.x - s * 0.82, c.y),
                Pos2::new(c.x - s * 0.35, c.y + s * 0.48),
                Pos2::new(c.x + s * 0.35, c.y + s * 0.48),
                Pos2::new(c.x + s * 0.82, c.y),
            ];
            painter.add(Shape::line(top, stroke));
            painter.add(Shape::line(bot, stroke));
            painter.circle_filled(c, s * 0.26, color);
        }
        Icon::Move => {
            painter.line_segment(
                [Pos2::new(c.x - s * 0.65, c.y), Pos2::new(c.x + s * 0.62, c.y)],
                stroke,
            );
            painter.add(Shape::line(
                vec![
                    Pos2::new(c.x + s * 0.16, c.y - s * 0.46),
                    Pos2::new(c.x + s * 0.65, c.y),
                    Pos2::new(c.x + s * 0.16, c.y + s * 0.46),
                ],
                stroke,
            ));
            painter.line_segment(
                [
                    Pos2::new(c.x - s * 0.65, c.y - s * 0.45),
                    Pos2::new(c.x - s * 0.65, c.y + s * 0.45),
                ],
                stroke,
            );
        }
        Icon::Retry => {
            let r = s * 0.58;
            let segs = 14;
            let mut pts = Vec::with_capacity(segs + 1);
            for i in 0..=segs {
                let a = -0.35 + (i as f32 / segs as f32) * std::f32::consts::TAU * 0.78;
                pts.push(Pos2::new(c.x + a.cos() * r, c.y + a.sin() * r));
            }
            painter.add(Shape::line(pts, stroke));
            let tip = Pos2::new(c.x + (-0.35f32).cos() * r, c.y + (-0.35f32).sin() * r);
            painter.add(Shape::line(
                vec![
                    Pos2::new(tip.x - s * 0.38, tip.y - s * 0.08),
                    tip,
                    Pos2::new(tip.x - s * 0.04, tip.y - s * 0.42),
                ],
                stroke,
            ));
        }
        Icon::Trash => {
            painter.line_segment(
                [
                    Pos2::new(c.x - s * 0.68, c.y - s * 0.44),
                    Pos2::new(c.x + s * 0.68, c.y - s * 0.44),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(c.x - s * 0.24, c.y - s * 0.68),
                    Pos2::new(c.x + s * 0.24, c.y - s * 0.68),
                ],
                stroke,
            );
            let body = Rect::from_min_max(
                Pos2::new(c.x - s * 0.50, c.y - s * 0.42),
                Pos2::new(c.x + s * 0.50, c.y + s * 0.70),
            );
            painter.rect_stroke(body, CornerRadius::same(2), stroke, StrokeKind::Inside);
        }
        Icon::Close => {
            let d = s * 0.52;
            painter.line_segment([Pos2::new(c.x - d, c.y - d), Pos2::new(c.x + d, c.y + d)], stroke);
            painter.line_segment([Pos2::new(c.x + d, c.y - d), Pos2::new(c.x - d, c.y + d)], stroke);
        }
        Icon::Download => {
            painter.line_segment(
                [Pos2::new(c.x, c.y - s * 0.72), Pos2::new(c.x, c.y + s * 0.32)],
                stroke,
            );
            painter.add(Shape::line(
                vec![
                    Pos2::new(c.x - s * 0.44, c.y - s * 0.08),
                    Pos2::new(c.x, c.y + s * 0.38),
                    Pos2::new(c.x + s * 0.44, c.y - s * 0.08),
                ],
                stroke,
            ));
            painter.line_segment(
                [
                    Pos2::new(c.x - s * 0.68, c.y + s * 0.68),
                    Pos2::new(c.x + s * 0.68, c.y + s * 0.68),
                ],
                stroke,
            );
        }
        Icon::Video => {
            let box_r = Rect::from_center_size(c, Vec2::new(s * 1.56, s * 1.18));
            painter.rect_stroke(box_r, CornerRadius::same(3), stroke, StrokeKind::Inside);
            let pts = vec![
                Pos2::new(c.x - s * 0.22, c.y - s * 0.30),
                Pos2::new(c.x + s * 0.34, c.y),
                Pos2::new(c.x - s * 0.22, c.y + s * 0.30),
            ];
            painter.add(Shape::convex_polygon(pts, color, Stroke::NONE));
        }
        Icon::Audio => {
            for (dx, h) in [(-0.55, 0.36), (-0.18, 0.72), (0.18, 0.52), (0.55, 0.30)] {
                painter.line_segment(
                    [
                        Pos2::new(c.x + s * dx, c.y - s * h),
                        Pos2::new(c.x + s * dx, c.y + s * h),
                    ],
                    Stroke::new(sw * 1.2, color),
                );
            }
        }
        Icon::Photo => {
            let box_r = Rect::from_center_size(c, Vec2::new(s * 1.52, s * 1.24));
            painter.rect_stroke(box_r, CornerRadius::same(3), stroke, StrokeKind::Inside);
            painter.circle_filled(Pos2::new(c.x - s * 0.32, c.y - s * 0.24), s * 0.16, color);
            painter.add(Shape::line(
                vec![
                    Pos2::new(c.x - s * 0.56, c.y + s * 0.42),
                    Pos2::new(c.x - s * 0.08, c.y - s * 0.02),
                    Pos2::new(c.x + s * 0.20, c.y + s * 0.22),
                    Pos2::new(c.x + s * 0.42, c.y + s * 0.04),
                    Pos2::new(c.x + s * 0.62, c.y + s * 0.42),
                ],
                Stroke::new(sw * 0.9, color),
            ));
        }
        Icon::Warning => {
            let tri = vec![
                Pos2::new(c.x, c.y - s * 0.72),
                Pos2::new(c.x + s * 0.76, c.y + s * 0.62),
                Pos2::new(c.x - s * 0.76, c.y + s * 0.62),
                Pos2::new(c.x, c.y - s * 0.72),
            ];
            painter.add(Shape::line(tri, stroke));
            painter.line_segment(
                [Pos2::new(c.x, c.y - s * 0.22), Pos2::new(c.x, c.y + s * 0.16)],
                stroke,
            );
            painter.circle_filled(Pos2::new(c.x, c.y + s * 0.40), sw * 0.55, color);
        }
        Icon::Sun => {
            painter.circle_stroke(c, s * 0.38, stroke);
            for i in 0..8 {
                let a = (i as f32) * std::f32::consts::FRAC_PI_4;
                painter.line_segment(
                    [
                        Pos2::new(c.x + a.cos() * s * 0.58, c.y + a.sin() * s * 0.58),
                        Pos2::new(c.x + a.cos() * s * 0.80, c.y + a.sin() * s * 0.80),
                    ],
                    stroke,
                );
            }
        }
        Icon::Moon => {
            let r = s * 0.60;
            let segs = 16;
            let mut pts = Vec::with_capacity(segs + 1);
            for i in 0..=segs {
                let a = 0.55 + (i as f32 / segs as f32) * std::f32::consts::TAU * 0.68;
                pts.push(Pos2::new(c.x + a.cos() * r, c.y + a.sin() * r));
            }
            painter.add(Shape::line(pts, stroke));
            painter.add(Shape::line(
                vec![
                    Pos2::new(c.x + 0.55f32.cos() * r, c.y + 0.55f32.sin() * r),
                    Pos2::new(c.x - s * 0.04, c.y - s * 0.04),
                    Pos2::new(c.x + (0.55 + std::f32::consts::TAU * 0.68).cos() * r, c.y + (0.55 + std::f32::consts::TAU * 0.68).sin() * r),
                ],
                stroke,
            ));
        }
        Icon::ChevronLeft => {
            painter.add(Shape::line(
                vec![
                    Pos2::new(c.x + s * 0.25, c.y - s * 0.52),
                    Pos2::new(c.x - s * 0.28, c.y),
                    Pos2::new(c.x + s * 0.25, c.y + s * 0.52),
                ],
                stroke,
            ));
        }
        Icon::ChevronRight => {
            painter.add(Shape::line(
                vec![
                    Pos2::new(c.x - s * 0.25, c.y - s * 0.52),
                    Pos2::new(c.x + s * 0.28, c.y),
                    Pos2::new(c.x - s * 0.25, c.y + s * 0.52),
                ],
                stroke,
            ));
        }
        Icon::DoubleLeft => {
            for dx in [-0.24, 0.24] {
                painter.add(Shape::line(
                    vec![
                        Pos2::new(c.x + s * (dx + 0.26), c.y - s * 0.46),
                        Pos2::new(c.x + s * (dx - 0.22), c.y),
                        Pos2::new(c.x + s * (dx + 0.26), c.y + s * 0.46),
                    ],
                    stroke,
                ));
            }
        }
        Icon::DoubleRight => {
            for dx in [-0.24, 0.24] {
                painter.add(Shape::line(
                    vec![
                        Pos2::new(c.x + s * (dx - 0.26), c.y - s * 0.46),
                        Pos2::new(c.x + s * (dx + 0.22), c.y),
                        Pos2::new(c.x + s * (dx - 0.26), c.y + s * 0.46),
                    ],
                    stroke,
                ));
            }
        }
        Icon::External => {
            painter.add(Shape::line(
                vec![
                    Pos2::new(c.x - s * 0.12, c.y - s * 0.56),
                    Pos2::new(c.x - s * 0.62, c.y - s * 0.56),
                    Pos2::new(c.x - s * 0.62, c.y + s * 0.62),
                    Pos2::new(c.x + s * 0.56, c.y + s * 0.62),
                    Pos2::new(c.x + s * 0.56, c.y + s * 0.12),
                ],
                stroke,
            ));
            painter.line_segment(
                [Pos2::new(c.x - s * 0.08, c.y + s * 0.08), Pos2::new(c.x + s * 0.64, c.y - s * 0.64)],
                stroke,
            );
            painter.add(Shape::line(
                vec![
                    Pos2::new(c.x + s * 0.16, c.y - s * 0.64),
                    Pos2::new(c.x + s * 0.64, c.y - s * 0.64),
                    Pos2::new(c.x + s * 0.64, c.y - s * 0.16),
                ],
                stroke,
            ));
        }
        Icon::Volume => {
            let body = vec![
                Pos2::new(c.x - s * 0.72, c.y - s * 0.28),
                Pos2::new(c.x - s * 0.36, c.y - s * 0.28),
                Pos2::new(c.x + s * 0.04, c.y - s * 0.62),
                Pos2::new(c.x + s * 0.04, c.y + s * 0.62),
                Pos2::new(c.x - s * 0.36, c.y + s * 0.28),
                Pos2::new(c.x - s * 0.72, c.y + s * 0.28),
            ];
            painter.add(Shape::convex_polygon(body, color, Stroke::NONE));
            for (r, alpha) in [(s * 0.42, 220u8), (s * 0.68, 175u8)] {
                let mut pts = Vec::with_capacity(9);
                for i in 0..=8 {
                    let a = -0.78 + (i as f32 / 8.0) * 1.56;
                    pts.push(Pos2::new(c.x + s * 0.08 + a.cos() * r, c.y + a.sin() * r));
                }
                painter.add(Shape::line(pts, Stroke::new(sw * 0.9, with_alpha(color, alpha))));
            }
        }
        Icon::VolumeMute => {
            let body = vec![
                Pos2::new(c.x - s * 0.72, c.y - s * 0.28),
                Pos2::new(c.x - s * 0.36, c.y - s * 0.28),
                Pos2::new(c.x + s * 0.02, c.y - s * 0.62),
                Pos2::new(c.x + s * 0.02, c.y + s * 0.62),
                Pos2::new(c.x - s * 0.36, c.y + s * 0.28),
                Pos2::new(c.x - s * 0.72, c.y + s * 0.28),
            ];
            painter.add(Shape::convex_polygon(body, with_alpha(color, 185), Stroke::NONE));
            let x0 = c.x + s * 0.24;
            let x1 = c.x + s * 0.74;
            let y0 = c.y - s * 0.26;
            let y1 = c.y + s * 0.26;
            painter.line_segment([Pos2::new(x0, y0), Pos2::new(x1, y1)], stroke);
            painter.line_segment([Pos2::new(x1, y0), Pos2::new(x0, y1)], stroke);
        }
    }
}

pub fn draw_brand_mark(ui: &mut Ui, p: &Palette) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
    let painter = ui.painter();
    // Subtle outer glow
    painter.rect_filled(
        rect.expand(2.0),
        CornerRadius::same(10),
        with_alpha(p.accent, 34),
    );
    painter.rect_filled(rect, CornerRadius::same(8), p.accent);
    // Crisp white inner play/vault triangle
    let c = rect.center();
    let pts = vec![
        Pos2::new(c.x - 4.0, c.y - 6.0),
        Pos2::new(c.x + 6.5, c.y),
        Pos2::new(c.x - 4.0, c.y + 6.0),
    ];
    painter.add(Shape::convex_polygon(pts, Color32::WHITE, Stroke::NONE));
}

// ============================================================================
// Smooth Animated Components
// ============================================================================

pub fn draw_spinner(painter: &Painter, center: Pos2, radius: f32, color: Color32, time: f64) {
    let base = (time as f32 * 5.2) % std::f32::consts::TAU;
    let segs = 18;
    let arc = std::f32::consts::PI * 1.35;
    let mut pts = Vec::with_capacity(segs + 1);
    for i in 0..=segs {
        let a = base + (i as f32 / segs as f32) * arc;
        pts.push(Pos2::new(center.x + a.cos() * radius, center.y + a.sin() * radius));
    }
    painter.circle_stroke(center, radius, Stroke::new(2.0, with_alpha(color, 45)));
    painter.add(Shape::line(pts, Stroke::new(2.2, color)));
}

pub fn draw_pulsing_dot(painter: &Painter, center: Pos2, color: Color32, time: f64) {
    let pulse = ((time * 2.6).sin() * 0.5 + 0.5) as f32;
    let halo_r = 3.5 + pulse * 3.5;
    let halo_a = ((1.0 - pulse) * 95.0) as u8;
    painter.circle_filled(center, halo_r, with_alpha(color, halo_a));
    painter.circle_filled(center, 3.2, color);
}

pub fn draw_equalizer_bars(ui: &mut Ui, active: bool, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(42.0, 26.0), Sense::hover());
    let t = ui.input(|i| i.time) as f32;
    let painter = ui.painter();
    let bar_w = 4.5;
    let gap = 4.5;
    let start_x = rect.center().x - (4.0 * bar_w + 3.0 * gap) * 0.5;
    for idx in 0..4 {
        let h_frac = if active {
            0.28 + 0.72 * ((t * (5.2 + idx as f32 * 1.1) + idx as f32 * 1.3).sin() * 0.5 + 0.5)
        } else {
            0.22
        };
        let bh = (rect.height() * h_frac).max(4.0);
        let x = start_x + idx as f32 * (bar_w + gap);
        let bar = Rect::from_min_max(
            Pos2::new(x, rect.bottom() - bh),
            Pos2::new(x + bar_w, rect.bottom()),
        );
        painter.rect_filled(bar, CornerRadius::same(2), color);
    }
    if active {
        ui.ctx().request_repaint();
    }
}

/// Subtle radial ambient gradient glow on the top canvas (Linear / Raycast style)
pub fn draw_ambient_glow(painter: &Painter, rect: Rect, p: &Palette) {
    if !p.dark {
        return;
    }
    let center = Pos2::new(rect.center().x, rect.top() + 30.0);
    for (r, a) in [(420.0, 7u8), (280.0, 10u8), (170.0, 12u8)] {
        painter.circle_filled(center, r, with_alpha(p.accent, a));
    }
}

#[derive(Clone, Copy)]
pub enum BtnVariant {
    Primary,
    Secondary,
    Ghost,
    AccentGhost,
    GoodGhost,
    DangerGhost,
    DangerSolid,
}

pub fn anim_button(
    ui: &mut Ui,
    id_salt: impl std::hash::Hash,
    label: &str,
    icon: Option<Icon>,
    variant: BtnVariant,
    height: f32,
    min_width: Option<f32>,
    enabled: bool,
    loading: bool,
    p: &Palette,
) -> Response {
    let font = font_semibold(12.5);
    let text_galley = if label.is_empty() {
        None
    } else {
        Some(ui.painter().layout_no_wrap(label.to_string(), font.clone(), Color32::WHITE))
    };
    let text_w = text_galley.as_ref().map(|g| g.size().x).unwrap_or(0.0);
    let icon_w = if icon.is_some() || loading { 15.0 } else { 0.0 };
    let gap = if text_w > 0.0 && icon_w > 0.0 { 7.0 } else { 0.0 };
    let pad_x = if label.is_empty() { 10.0 } else { 13.0 };
    let width = (pad_x * 2.0 + icon_w + gap + text_w).max(min_width.unwrap_or(0.0));

    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, height), sense);
    if enabled && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    let anim_id = ui.make_persistent_id(id_salt);
    let hover_t = ui
        .ctx()
        .animate_bool_with_time(anim_id.with("h"), enabled && resp.hovered(), 0.14);
    let press_t = ui
        .ctx()
        .animate_bool_with_time(anim_id.with("p"), enabled && resp.is_pointer_button_down_on(), 0.08);

    let (bg_base, bg_hov, fg_base, fg_hov, brd_base, brd_hov) = match variant {
        BtnVariant::Primary => (
            p.accent,
            p.accent_hover,
            Color32::WHITE,
            Color32::WHITE,
            Color32::TRANSPARENT,
            with_alpha(Color32::WHITE, 45),
        ),
        BtnVariant::Secondary => (
            p.inset,
            p.surface_hover,
            p.text_secondary,
            p.text,
            p.border,
            p.border_hover,
        ),
        BtnVariant::Ghost => (
            Color32::TRANSPARENT,
            p.inset,
            p.muted,
            p.text,
            Color32::TRANSPARENT,
            p.border,
        ),
        BtnVariant::AccentGhost => (
            with_alpha(p.accent, if p.dark { 22 } else { 16 }),
            with_alpha(p.accent, if p.dark { 46 } else { 32 }),
            if p.dark { Color32::from_rgb(165, 180, 252) } else { p.accent },
            if p.dark { Color32::WHITE } else { p.accent_hover },
            with_alpha(p.accent, 45),
            with_alpha(p.accent, 110),
        ),
        BtnVariant::GoodGhost => (
            with_alpha(p.good, if p.dark { 20 } else { 16 }),
            with_alpha(p.good, if p.dark { 42 } else { 30 }),
            p.good,
            p.good,
            with_alpha(p.good, 40),
            with_alpha(p.good, 95),
        ),
        BtnVariant::DangerGhost => (
            Color32::TRANSPARENT,
            p.error_soft,
            p.muted,
            p.error,
            Color32::TRANSPARENT,
            with_alpha(p.error, 85),
        ),
        BtnVariant::DangerSolid => (
            p.error,
            Color32::from_rgb(239, 68, 68),
            Color32::WHITE,
            Color32::WHITE,
            Color32::TRANSPARENT,
            with_alpha(Color32::WHITE, 40),
        ),
    };

    let mut bg = lerp_color(bg_base, bg_hov, hover_t);
    let mut fg = lerp_color(fg_base, fg_hov, hover_t);
    let brd = lerp_color(brd_base, brd_hov, hover_t);
    if !enabled {
        bg = with_alpha(bg, (bg.a() as f32 * 0.55) as u8);
        fg = with_alpha(fg, (fg.a() as f32 * 0.45) as u8);
    }

    let draw_rect = rect.shrink(press_t * 0.7);
    let painter = ui.painter();

    // Subtle glow behind primary button on hover
    if matches!(variant, BtnVariant::Primary) && enabled && hover_t > 0.01 {
        painter.rect_filled(
            draw_rect.expand(2.5),
            CornerRadius::same(10),
            with_alpha(p.accent, (hover_t * 52.0) as u8),
        );
    }

    painter.rect_filled(draw_rect, CornerRadius::same(8), bg);
    if brd.a() > 0 {
        painter.rect_stroke(draw_rect, CornerRadius::same(8), Stroke::new(1.0, brd), StrokeKind::Inside);
    }

    let content_w = icon_w + gap + text_w;
    let mut cursor_x = draw_rect.center().x - content_w * 0.5;
    if loading {
        let t = ui.input(|i| i.time);
        draw_spinner(painter, Pos2::new(cursor_x + 7.0, draw_rect.center().y), 5.8, fg, t);
        ui.ctx().request_repaint();
        cursor_x += icon_w + gap;
    } else if let Some(ic) = icon {
        let ir = Rect::from_center_size(Pos2::new(cursor_x + 7.5, draw_rect.center().y), Vec2::splat(13.5));
        draw_icon(painter, ir, ic, fg);
        cursor_x += icon_w + gap;
    }

    if !label.is_empty() {
        painter.text(
            Pos2::new(cursor_x, draw_rect.center().y),
            Align2::LEFT_CENTER,
            label,
            font,
            fg,
        );
    }

    resp
}

/// Linear/Vercel-style segmented tab bar with a smoothly gliding active indicator pill.
pub fn animated_tab_bar(
    ui: &mut Ui,
    id_salt: &str,
    items: &[(&str, String)],
    active_key: &str,
    p: &Palette,
) -> Option<String> {
    let mut clicked_key: Option<String> = None;
    let bar_id = Id::new(id_salt);
    let font = font_semibold(12.5);
    let height = 32.0;

    let widths: Vec<f32> = items
        .iter()
        .map(|(_, lbl)| {
            let g = ui.painter().layout_no_wrap(lbl.clone(), font.clone(), p.text);
            g.size().x + 24.0
        })
        .collect();
    let total_w: f32 = widths.iter().sum::<f32>() + 8.0;

    let (outer_rect, _) = ui.allocate_exact_size(Vec2::new(total_w, height), Sense::hover());
    let painter = ui.painter();

    // Container track
    painter.rect_filled(outer_rect, CornerRadius::same(9), p.input_bg);
    painter.rect_stroke(
        outer_rect,
        CornerRadius::same(9),
        Stroke::new(1.0, p.border),
        StrokeKind::Inside,
    );

    // Compute per-tab rects
    let mut rects = Vec::with_capacity(items.len());
    let mut x = outer_rect.left() + 4.0;
    let mut active_idx = 0usize;
    for (idx, ((key, _), w)) in items.iter().zip(widths.iter()).enumerate() {
        let r = Rect::from_min_size(Pos2::new(x, outer_rect.top() + 3.0), Vec2::new(*w, height - 6.0));
        if *key == active_key {
            active_idx = idx;
        }
        rects.push(r);
        x += *w;
    }

    // Smoothly animate pill X & width across tabs!
    let target = rects[active_idx];
    let anim_x = ui
        .ctx()
        .animate_value_with_time(bar_id.with("pill_x"), target.left() - outer_rect.left(), 0.20);
    let anim_w = ui
        .ctx()
        .animate_value_with_time(bar_id.with("pill_w"), target.width(), 0.20);
    let pill_rect = Rect::from_min_size(
        Pos2::new(outer_rect.left() + anim_x, outer_rect.top() + 3.0),
        Vec2::new(anim_w, height - 6.0),
    );
    painter.rect_filled(pill_rect, CornerRadius::same(6), p.accent);

    // Interactive hit-tests & labels
    for (idx, ((key, lbl), r)) in items.iter().zip(rects.iter()).enumerate() {
        let resp = ui.interact(*r, bar_id.with(idx), Sense::click());
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let is_active = idx == active_idx;
        let hov = ui
            .ctx()
            .animate_bool_with_time(bar_id.with(("h", idx)), !is_active && resp.hovered(), 0.12);
        let fg = if is_active {
            Color32::WHITE
        } else {
            lerp_color(p.muted, p.text, hov)
        };
        painter.text(r.center(), Align2::CENTER_CENTER, lbl, font.clone(), fg);
        if resp.clicked() && !is_active {
            clicked_key = Some((*key).to_string());
        }
    }

    clicked_key
}

/// Smoothly eased animated progress bar with luminous shimmer sweep.
pub fn animated_progress_bar(ui: &mut Ui, id_salt: impl std::hash::Hash, target_frac: f32, width: f32, p: &Palette) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 7.0), Sense::hover());
    let smooth_frac = ui
        .ctx()
        .animate_value_with_time(Id::new(id_salt), target_frac.clamp(0.02, 1.0), 0.22);
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(4), p.inset);

    let fill_w = (rect.width() * smooth_frac).max(6.0);
    let fill_rect = Rect::from_min_size(rect.min, Vec2::new(fill_w, rect.height()));
    painter.rect_filled(fill_rect, CornerRadius::same(4), p.accent);

    // Continuous smooth shimmer highlight moving across the active fill
    let t = ui.input(|i| i.time) as f32;
    let phase = (t * 0.95) % 1.4 - 0.2;
    let sx = fill_rect.left() + phase * fill_rect.width();
    let shimmer = Rect::from_min_max(
        Pos2::new((sx - 28.0).max(fill_rect.left()), fill_rect.top()),
        Pos2::new((sx + 28.0).min(fill_rect.right()), fill_rect.bottom()),
    );
    if shimmer.width() > 2.0 {
        painter.rect_filled(shimmer, CornerRadius::same(4), with_alpha(Color32::WHITE, 62));
    }
    ui.ctx().request_repaint();
}

/// Clean elevated section card container
pub fn section_card(p: &Palette) -> Frame {
    Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::same(16))
        .shadow(Shadow {
            offset: [0, 4],
            blur: 18,
            spread: 0,
            color: Color32::from_rgba_unmultiplied(0, 0, 0, if p.dark { 60 } else { 14 }),
        })
}

/// Pill badge with optional vector icon or pulsing status dot
pub fn status_pill(
    ui: &mut Ui,
    text: &str,
    fg: Color32,
    bg: Color32,
    icon: Option<Icon>,
    pulsing: bool,
) {
    let font = font_semibold(11.0);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font.clone(), fg);
    let lead_w = if pulsing {
        11.0
    } else if icon.is_some() {
        13.0
    } else {
        0.0
    };
    let gap = if lead_w > 0.0 { 5.0 } else { 0.0 };
    let w = 16.0 + lead_w + gap + galley.size().x;
    let h = 22.0;

    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(11), bg);
    painter.rect_stroke(
        rect,
        CornerRadius::same(11),
        Stroke::new(1.0, with_alpha(fg, 45)),
        StrokeKind::Inside,
    );

    let mut x = rect.left() + 8.0;
    if pulsing {
        let t = ui.input(|i| i.time);
        draw_pulsing_dot(painter, Pos2::new(x + 4.5, rect.center().y), fg, t);
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(40));
        x += lead_w + gap;
    } else if let Some(ic) = icon {
        let ir = Rect::from_center_size(Pos2::new(x + 5.5, rect.center().y), Vec2::splat(11.0));
        draw_icon(painter, ir, ic, fg);
        x += lead_w + gap;
    }
    painter.text(Pos2::new(x, rect.center().y), Align2::LEFT_CENTER, text, font, fg);
}

pub fn platform_chip(ui: &mut Ui, name: &str, key: &str, p: &Palette) {
    let dot_col = p.platform_color(key);
    let font = font_semibold(11.0);
    let galley = ui.painter().layout_no_wrap(name.to_string(), font.clone(), p.text_secondary);
    let w = 14.0 + 6.0 + 5.0 + galley.size().x;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 22.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(11), p.inset);
    painter.rect_stroke(
        rect,
        CornerRadius::same(11),
        Stroke::new(1.0, p.border),
        StrokeKind::Inside,
    );
    painter.circle_filled(Pos2::new(rect.left() + 10.0, rect.center().y), 3.0, dot_col);
    painter.text(
        Pos2::new(rect.left() + 17.5, rect.center().y),
        Align2::LEFT_CENTER,
        name,
        font,
        p.text_secondary,
    );
}

pub fn subtle_tag(ui: &mut Ui, text: &str, p: &Palette) {
    let font = font_regular(11.0);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font.clone(), p.muted);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(galley.size().x + 14.0, 21.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(6), p.inset);
    painter.text(rect.center(), Align2::CENTER_CENTER, text, font, p.muted);
}

/// Smoothly animated modal backdrop + elevated centered dialog card with slide-up easing.
pub fn show_animated_modal(
    ctx: &egui::Context,
    id_salt: &str,
    title: &str,
    subtitle: Option<&str>,
    size: Vec2,
    p: &Palette,
    open: &mut bool,
    body: impl FnOnce(&mut Ui),
) {
    let modal_id = Id::new(id_salt);
    let open_t = ctx.animate_bool_with_time(modal_id.with("open"), *open, 0.18);
    if open_t <= 0.01 && !*open {
        return;
    }

    if *open && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        *open = false;
    }

    let eased = ease_out_cubic(open_t);
    let screen = ctx.screen_rect();

    // Backdrop scrim (click outside to dismiss)
    egui::Area::new(modal_id.with("scrim"))
        .fixed_pos(screen.min)
        .order(egui::Order::Middle)
        .show(ctx, |ui| {
            let (rect, resp) = ui.allocate_exact_size(screen.size(), Sense::click());
            let scrim_a = (p.scrim.a() as f32 * eased) as u8;
            ui.painter().rect_filled(rect, CornerRadius::ZERO, with_alpha(p.scrim, scrim_a));
            if resp.clicked() {
                *open = false;
            }
        });

    // Slide-up offset (14px -> 0px)
    let y_offset = (1.0 - eased) * 14.0;
    let clamped_w = size.x.min(screen.width() - 48.0);
    let clamped_h = size.y.min(screen.height() - 48.0);
    let top_left = Pos2::new(
        screen.center().x - clamped_w * 0.5,
        screen.center().y - clamped_h * 0.5 + y_offset,
    );

    egui::Area::new(modal_id.with("card"))
        .fixed_pos(top_left)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            ui.set_min_size(Vec2::new(clamped_w, clamped_h));
            ui.set_max_size(Vec2::new(clamped_w, clamped_h));

            let card_frame = Frame::new()
                .fill(p.surface_elevated)
                .stroke(Stroke::new(1.0, p.border_hover))
                .corner_radius(CornerRadius::same(16))
                .inner_margin(Margin::same(20))
                .shadow(Shadow {
                    offset: [0, 18],
                    blur: 44,
                    spread: 0,
                    color: Color32::from_rgba_unmultiplied(
                        0,
                        0,
                        0,
                        ((if p.dark { 175.0 } else { 55.0 }) * eased) as u8,
                    ),
                });

            card_frame.show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new(title).font(font_semibold(16.0)).color(p.text));
                        if let Some(sub) = subtitle {
                            ui.label(RichText::new(sub).font(font_regular(12.0)).color(p.muted));
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if anim_button(
                            ui,
                            modal_id.with("close_x"),
                            "",
                            Some(Icon::Close),
                            BtnVariant::Ghost,
                            28.0,
                            Some(28.0),
                            true,
                            false,
                            p,
                        )
                        .clicked()
                        {
                            *open = false;
                        }
                    });
                });

                ui.add_space(10.0);
                let (sep_rect, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
                ui.painter().rect_filled(sep_rect, CornerRadius::ZERO, p.border);
                ui.add_space(12.0);

                body(ui);
            });
        });
}
