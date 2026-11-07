//! egui front-end.
//!
//! The *root* viewport is the overlay: a transparent, borderless, always-on-top,
//! click-through window covering the primary screen and excluded from capture.
//! The control panel is a normal window opened on demand (F10 by default).

use crate::config::Config;
use crate::state::{Command, InterfaceView, Shared};
use crate::win;
use eframe::egui::{
    self, text::LayoutJob, Color32, FontData, FontDefinitions, FontFamily, FontId, Id, LayerId, Order, Pos2, Rect,
    Rounding, Stroke, TextFormat, Vec2, ViewportBuilder, ViewportCommand, ViewportId,
};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const OVERLAY_TITLE: &str = "Wuxia Local Overlay";
pub const PANEL_TITLE: &str = "Wuxia Local — панель";

pub struct WuxiaApp {
    shared: Arc<Shared>,
    overlay_hwnd: Option<isize>,
    panel_hwnd: Option<isize>,
    /// Capture visibility currently applied to (overlay, panel).
    applied_visibility: (Option<bool>, Option<bool>),
    /// Frames rendered while hidden; the overlay is shown only once it is transparent.
    frames: u32,
    focus_frames: u32,
    exclude_attempts: u32,
    region_tex: Option<egui::TextureHandle>,
    ocr_tex: Option<(u64, egui::TextureHandle)>,
    drag_start: Option<Pos2>,
    panel_state: crate::ui_panel::PanelState,
    cfg_dirty_since: Option<Instant>,
    sized: bool,
}

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let candidates = [
        ("segoe", "segoeui.ttf", true),
        ("segoe_b", "segoeuib.ttf", false),
        ("yahei", "msyh.ttc", true),
        ("simsun", "simsun.ttc", true),
    ];
    let mut order = Vec::new();
    for (name, file, use_it) in candidates {
        let path = std::path::Path::new(&windir).join("Fonts").join(file);
        if let Ok(bytes) = std::fs::read(&path) {
            fonts.font_data.insert(name.to_string(), FontData::from_owned(bytes));
            if use_it {
                order.push(name.to_string());
            }
        }
    }
    if let Some(prop) = fonts.families.get_mut(&FontFamily::Proportional) {
        for (i, n) in order.iter().enumerate() {
            prop.insert(i, n.clone());
        }
    }
    if fonts.font_data.contains_key("segoe_b") {
        fonts.families.insert(FontFamily::Name("bold".into()), vec!["segoe_b".into(), "yahei".into()]);
    } else {
        let p = fonts.families[&FontFamily::Proportional].clone();
        fonts.families.insert(FontFamily::Name("bold".into()), p);
    }
    ctx.set_fonts(fonts);
}

impl WuxiaApp {
    pub fn new(cc: &eframe::CreationContext<'_>, shared: Arc<Shared>) -> Self {
        install_fonts(&cc.egui_ctx);
        crate::ui_panel::apply_theme(&cc.egui_ctx);
        let _ = shared.egui.set(cc.egui_ctx.clone());
        Self {
            shared,
            overlay_hwnd: None,
            panel_hwnd: None,
            applied_visibility: (None, None),
            frames: 0,
            focus_frames: 0,
            exclude_attempts: 0,
            region_tex: None,
            ocr_tex: None,
            drag_start: None,
            panel_state: crate::ui_panel::PanelState::default(),
            cfg_dirty_since: None,
            sized: false,
        }
    }

    fn region_rect(ctx: &egui::Context, cfg: &Config) -> Rect {
        let (sw, sh) = win::screen_size();
        let r = cfg.dialogue.region;
        let k = 1.0 / ctx.pixels_per_point();
        Rect::from_min_size(
            Pos2::new(r[0] * sw as f32 * k, r[1] * sh as f32 * k),
            Vec2::new(r[2] * sw as f32 * k, r[3] * sh as f32 * k),
        )
    }
}

fn shadowed_text(painter: &egui::Painter, pos: Pos2, galley: Arc<egui::Galley>, color: Color32) {
    let sh = Color32::from_black_alpha(200);
    for (dx, dy) in [(-1.5, 0.0), (1.5, 0.0), (0.0, -1.5), (0.0, 1.5), (1.5, 1.5)] {
        painter.galley_with_override_text_color(pos + Vec2::new(dx, dy), galley.clone(), sh);
    }
    painter.galley(pos, galley, color);
}

impl eframe::App for WuxiaApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.overlay_hwnd.is_none() {
            self.overlay_hwnd = win::find_window(OVERLAY_TITLE);
            if let Some(h) = self.overlay_hwnd {
                win::make_tool_window(h);
            }
        }
        // The window starts at 1×1 px: a freshly created GL window is black until the
        // compositor picks up transparency. Expand to full screen after a few clear frames.
        if !self.sized {
            self.frames += 1;
            if self.frames >= 4 && self.overlay_hwnd.is_some() {
                let (sw, sh) = win::screen_size();
                let ppp = ctx.pixels_per_point();
                ctx.send_viewport_cmd(ViewportCommand::OuterPosition(Pos2::ZERO));
                ctx.send_viewport_cmd(ViewportCommand::InnerSize(Vec2::new(sw as f32 / ppp, sh as f32 / ppp)));
                self.sized = true;
            }
            ctx.request_repaint_after(Duration::from_millis(50));
        }

        let cfg = self.shared.cfg();
        let want_visible = cfg.general.visible_in_screenshots;
        if let Some(h) = self.overlay_hwnd {
            // Wait until the window is full-size; retry a few times (the call can fail
            // while the window is still being set up).
            if self.sized && self.applied_visibility.0 != Some(want_visible) {
                match win::set_capture_visible(h, want_visible) {
                    Ok(()) => {
                        tracing::info!("оверлей: видимость на скриншотах = {want_visible}");
                        self.applied_visibility.0 = Some(want_visible);
                        self.exclude_attempts = 0;
                    }
                    Err(e) => {
                        self.exclude_attempts += 1;
                        if self.exclude_attempts >= 20 {
                            self.shared.ui(|u| u.notice(format!("Не удалось изменить видимость оверлея для захвата: {e}")));
                            self.applied_visibility.0 = Some(want_visible);
                        } else {
                            ctx.request_repaint_after(Duration::from_millis(250));
                        }
                    }
                }
            }
        }
        let (dialogue, interface, show_original, selecting, panel_open, server, paused, dialogue_on) = {
            let u = self.shared.ui.lock().unwrap();
            (
                u.dialogue.clone(),
                u.interface.clone(),
                u.show_original,
                u.selecting_region,
                u.panel_open,
                u.server.clone(),
                u.paused_unfocused,
                u.dialogue_on,
            )
        };

        let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("wuxia_overlay")));
        let screen = ctx.screen_rect();

        let _ = screen;
        if selecting {
            self.region_window(ctx, &cfg);
        } else {
            self.region_tex = None;
            if let Some(view) = &interface {
                draw_interface(ctx, &painter, view, &cfg, show_original);
            }
            if let Some(d) = &dialogue {
                if interface.is_none() {
                    draw_dialogue(ctx, &painter, d, &cfg, show_original);
                }
            }
            // Tiny status badge while the model is not ready.
            if dialogue_on && !paused {
                if let Some(s) = server.as_ref().filter(|s| !s.is_ready()) {
                    let g = ctx.fonts(|f| {
                        f.layout_no_wrap(format!("Wuxia Local: {}", s.label()), FontId::proportional(14.0), Color32::from_rgb(255, 225, 160))
                    });
                    let r = Rect::from_min_size(Pos2::new(12.0, 12.0), g.size() + Vec2::splat(12.0));
                    painter.rect_filled(r, Rounding::same(6.0), Color32::from_black_alpha(170));
                    painter.galley(r.min + Vec2::splat(6.0), g, Color32::WHITE);
                }
            }
        }

        if panel_open {
            self.panel(ctx);
            // Bring it to the front a few frames after (re)opening.
            if self.shared.ui.lock().unwrap().focus_panel {
                self.focus_frames = 6;
                self.shared.ui.lock().unwrap().focus_panel = false;
            }
            if self.focus_frames > 0 {
                self.focus_frames -= 1;
                let id = ViewportId::from_hash_of("wuxia_panel");
                ctx.send_viewport_cmd_to(id, ViewportCommand::Focus);
                if self.focus_frames == 3 {
                    if let Some(h) = win::find_window(PANEL_TITLE) {
                        win::bring_to_front(h);
                    }
                }
                ctx.request_repaint_after(Duration::from_millis(60));
            }
        }

        // Debounced config save after edits in the panel.
        if let Some(t) = self.cfg_dirty_since {
            if t.elapsed() > Duration::from_millis(800) {
                self.shared.send(Command::SaveConfig);
                self.cfg_dirty_since = None;
            } else {
                ctx.request_repaint_after(Duration::from_millis(300));
            }
        }
    }
}

fn draw_dialogue(ctx: &egui::Context, painter: &egui::Painter, d: &crate::state::DialogueView, cfg: &Config, show_original: bool) {
    let region = WuxiaApp::region_rect(ctx, cfg);
    let size = cfg.dialogue.font_size;
    let body = if show_original {
        d.source.clone()
    } else if !d.translation.is_empty() {
        d.translation.clone()
    } else if d.pending {
        "…".into()
    } else if let Some(e) = &d.error {
        format!("{}  [ошибка: {}]", d.source, e.chars().take(60).collect::<String>())
    } else {
        return;
    };
    let wrap = (region.width() * 1.1).max(300.0);
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap;
    job.halign = egui::Align::Center;
    if let (Some(sp), false) = (&d.speaker, show_original) {
        job.append(
            &format!("{sp}: "),
            0.0,
            TextFormat { font_id: FontId::new(size, FontFamily::Name("bold".into())), color: Color32::from_rgb(240, 200, 120), ..Default::default() },
        );
    }
    let color = if d.pending && !show_original { Color32::from_rgb(225, 225, 225) } else { Color32::from_rgb(250, 246, 235) };
    job.append(&body, 0.0, TextFormat { font_id: FontId::proportional(size), color, ..Default::default() });
    let galley = ctx.fonts(|f| f.layout_job(job));
    let gsize = galley.size();
    let pad = Vec2::new(14.0, 8.0);
    let plate_w = gsize.x + pad.x * 2.0;
    let plate_h = gsize.y + pad.y * 2.0;
    let cx = region.center().x;
    let top = match cfg.dialogue.position.as_str() {
        "above" => region.top() - plate_h - 6.0,
        "over" => region.center().y - plate_h / 2.0,
        _ => region.bottom() + 6.0,
    };
    let screen = ctx.screen_rect();
    let top = top.clamp(0.0, (screen.bottom() - plate_h).max(0.0));
    let plate = Rect::from_min_size(Pos2::new(cx - plate_w / 2.0, top), Vec2::new(plate_w, plate_h));
    let alpha = if cfg.dialogue.position == "over" { 0.92f32.max(cfg.dialogue.backdrop_opacity) } else { cfg.dialogue.backdrop_opacity };
    painter.rect_filled(plate, Rounding::same(8.0), Color32::from_black_alpha((alpha.clamp(0.0, 1.0) * 255.0) as u8));
    // The galley is centre-aligned around x=0 when halign=Center.
    let pos = Pos2::new(plate.center().x - gsize.x / 2.0 - galley.rect.min.x, plate.top() + pad.y);
    shadowed_text(painter, pos, galley, Color32::WHITE);
}

fn draw_interface(ctx: &egui::Context, painter: &egui::Painter, view: &InterfaceView, cfg: &Config, show_original: bool) {
    let ppp = ctx.pixels_per_point();
    let mode = cfg.interface.mode.as_str();
    if show_original {
        return;
    }
    if mode == "list" {
        let mut job = LayoutJob::default();
        job.wrap.max_width = 420.0;
        for it in &view.items {
            let dst = it.dst.as_deref().unwrap_or("…");
            job.append(&format!("{}  ", it.src), 0.0, TextFormat { font_id: FontId::proportional(13.0), color: Color32::from_gray(170), ..Default::default() });
            job.append(&format!("{dst}\n"), 0.0, TextFormat { font_id: FontId::proportional(16.0), color: Color32::WHITE, ..Default::default() });
        }
        let g = ctx.fonts(|f| f.layout_job(job));
        let screen = ctx.screen_rect();
        let r = Rect::from_min_size(Pos2::new(screen.right() - g.size().x - 36.0, 60.0), g.size() + Vec2::splat(24.0));
        painter.rect_filled(r, Rounding::same(8.0), Color32::from_black_alpha(215));
        painter.galley(r.min + Vec2::splat(12.0), g, Color32::WHITE);
    } else {
        for it in &view.items {
            let Some(dst) = &it.dst else { continue };
            let r = Rect::from_min_size(Pos2::new(it.rect.x / ppp, it.rect.y / ppp), Vec2::new(it.rect.w / ppp, it.rect.h / ppp));
            let mut size = (r.height() * cfg.interface.font_scale).clamp(9.0, 64.0);
            let mut g = ctx.fonts(|f| f.layout_no_wrap(dst.clone(), FontId::proportional(size), Color32::WHITE));
            // Russian is longer: shrink to fit up to 55 %, then allow overflow.
            let max_w = r.width() * 1.15 + 8.0;
            if g.size().x > max_w {
                size = (size * max_w / g.size().x).max(size * 0.55);
                g = ctx.fonts(|f| f.layout_no_wrap(dst.clone(), FontId::proportional(size), Color32::WHITE));
            }
            let cover = Rect::from_min_size(r.min - Vec2::splat(2.0), Vec2::new(r.width().max(g.size().x) + 4.0, r.height() + 4.0));
            let (fill, fg) = if it.busy || mode == "backdrop" {
                (Color32::from_rgba_unmultiplied(12, 12, 14, 230), Color32::from_rgb(245, 240, 225))
            } else {
                (Color32::from_rgb(it.bg[0], it.bg[1], it.bg[2]), Color32::from_rgb(it.fg[0], it.fg[1], it.fg[2]))
            };
            painter.rect_filled(cover, Rounding::same(if it.busy || mode == "backdrop" { 4.0 } else { 1.0 }), fill);
            let pos = Pos2::new(r.left(), r.center().y - g.size().y / 2.0);
            painter.galley(pos, g, fg);
        }
    }
    if view.pending > 0 {
        let g = ctx.fonts(|f| f.layout_no_wrap(format!("Перевод экрана… осталось {}", view.pending), FontId::proportional(14.0), Color32::WHITE));
        let screen = ctx.screen_rect();
        let r = Rect::from_min_size(Pos2::new(screen.right() - g.size().x - 24.0, 12.0), g.size() + Vec2::splat(12.0));
        painter.rect_filled(r, Rounding::same(6.0), Color32::from_black_alpha(180));
        painter.galley(r.min + Vec2::splat(6.0), g, Color32::WHITE);
    }
}

impl WuxiaApp {
    /// Region picker: a normal (opaque, focusable) full-screen window showing a
    /// screenshot taken just before. The transparent overlay never stops being
    /// click-through, so it can neither turn black nor trap the mouse.
    fn region_window(&mut self, ctx: &egui::Context, cfg: &Config) {
        let shot = self.shared.ui.lock().unwrap().region_shot.clone();
        let Some(shot) = shot else { return };
        if self.region_tex.is_none() {
            let mut rgba = Vec::with_capacity(shot.bgra.len());
            for p in shot.bgra.chunks_exact(4) {
                rgba.extend_from_slice(&[p[2], p[1], p[0], 255]);
            }
            let img = egui::ColorImage::from_rgba_unmultiplied([shot.width as usize, shot.height as usize], &rgba);
            self.region_tex = Some(ctx.load_texture("region_shot", img, egui::TextureOptions::LINEAR));
        }
        let Some(tex) = self.region_tex.clone() else { return };
        let shared = self.shared.clone();
        let region = cfg.dialogue.region;
        let drag = &mut self.drag_start;
        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("wuxia_region"),
            ViewportBuilder::default()
                .with_title("Wuxia Local — выбор области")
                .with_fullscreen(true)
                .with_decorations(false)
                .with_always_on_top(),
            |ctx, _class| {
                if ctx.input(|i| i.viewport().close_requested()) {
                    shared.send(Command::CancelRegion);
                }
                egui::CentralPanel::default().frame(egui::Frame::none()).show(ctx, |ui| {
                    let screen = ctx.screen_rect();
                    let s = screen.size();
                    let painter = ui.painter();
                    painter.image(tex.id(), screen, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                    painter.rect_filled(screen, Rounding::ZERO, Color32::from_black_alpha(70));
                    let cur = Rect::from_min_size(
                        Pos2::new(region[0] * s.x, region[1] * s.y),
                        Vec2::new(region[2] * s.x, region[3] * s.y),
                    );
                    painter.rect_stroke(cur, Rounding::same(2.0), Stroke::new(2.0, Color32::from_rgb(120, 180, 255)));
                    let help = ctx.fonts(|f| {
                        f.layout(
                            "Обведите мышью область, где появляются субтитры (с запасом по краям).\n\
                             Esc или правая кнопка мыши — отмена. Синяя рамка — текущая область."
                                .to_string(),
                            FontId::proportional(20.0),
                            Color32::WHITE,
                            screen.width() * 0.8,
                        )
                    });
                    let hr = Rect::from_center_size(
                        Pos2::new(screen.center().x, 40.0 + help.size().y / 2.0),
                        help.size() + Vec2::splat(20.0),
                    );
                    painter.rect_filled(hr, Rounding::same(8.0), Color32::from_black_alpha(200));
                    painter.galley(hr.min + Vec2::splat(10.0), help, Color32::WHITE);

                    let (pressed, down, released, pos, cancel) = ctx.input(|i| {
                        (
                            i.pointer.primary_pressed(),
                            i.pointer.primary_down(),
                            i.pointer.primary_released(),
                            i.pointer.interact_pos(),
                            i.key_pressed(egui::Key::Escape) || i.pointer.secondary_clicked(),
                        )
                    });
                    if cancel {
                        *drag = None;
                        shared.send(Command::CancelRegion);
                        return;
                    }
                    if pressed {
                        *drag = pos;
                    }
                    if let (Some(a), Some(b)) = (*drag, pos) {
                        let r = Rect::from_two_pos(a, b);
                        if down || released {
                            painter.rect_filled(r, Rounding::ZERO, Color32::from_rgba_unmultiplied(255, 220, 120, 40));
                            painter.rect_stroke(r, Rounding::ZERO, Stroke::new(2.0, Color32::from_rgb(255, 210, 90)));
                        }
                        if released {
                            *drag = None;
                            if r.width() > 20.0 && r.height() > 10.0 {
                                let frac = [r.min.x / s.x, r.min.y / s.y, r.width() / s.x, r.height() / s.y];
                                shared.send(Command::RegionSelected(frac));
                            }
                        }
                    }
                });
                ctx.request_repaint();
            },
        );
    }

    fn panel(&mut self, ctx: &egui::Context) {
        let shared = self.shared.clone();
        // Texture of the last OCR'd subtitle strip, refreshed when a new one arrives.
        let dbg = self.shared.ui.lock().unwrap().ocr_debug.clone();
        if let Some(d) = &dbg {
            if self.ocr_tex.as_ref().map_or(true, |(id, _)| *id != d.id) {
                let mut rgba = Vec::with_capacity(d.frame.bgra.len());
                for p in d.frame.bgra.chunks_exact(4) {
                    rgba.extend_from_slice(&[p[2], p[1], p[0], 255]);
                }
                let img = egui::ColorImage::from_rgba_unmultiplied([d.frame.width as usize, d.frame.height as usize], &rgba);
                self.ocr_tex = Some((d.id, ctx.load_texture("ocr_debug", img, egui::TextureOptions::LINEAR)));
            }
        }
        let ocr_view = match (&dbg, &self.ocr_tex) {
            (Some(d), Some((_, tex))) => Some(crate::ui_panel::OcrView {
                tex: tex.id(),
                size: Vec2::new(d.frame.width as f32, d.frame.height as f32),
                text: d.text.clone(),
                ms: d.ms,
            }),
            _ => None,
        };
        let st = &mut self.panel_state;
        let mut dirty = false;
        let mut protect_now = false;
        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("wuxia_panel"),
            ViewportBuilder::default()
                .with_title(PANEL_TITLE)
                .with_inner_size([880.0, 720.0])
                .with_min_inner_size([720.0, 480.0])
                .with_active(true),
            |ctx, _class| {
                protect_now = true;
                if ctx.input(|i| i.viewport().close_requested()) {
                    shared.ui(|u| u.panel_open = false);
                }
                dirty |= crate::ui_panel::panel_ui(ctx, &shared, st, ocr_view.as_ref());
            },
        );
        if protect_now && self.panel_hwnd.is_none() {
            self.panel_hwnd = win::find_window(PANEL_TITLE);
        }
        if let Some(h) = self.panel_hwnd {
            let want = self.shared.cfg().general.visible_in_screenshots;
            if self.applied_visibility.1 != Some(want) {
                if let Err(e) = win::set_capture_visible(h, want) {
                    tracing::warn!("панель: видимость на скриншотах: {e}");
                }
                self.applied_visibility.1 = Some(want);
            }
        }
        if !self.shared.ui.lock().unwrap().panel_open {
            self.panel_hwnd = None;
            self.applied_visibility.1 = None;
        }
        if dirty {
            self.cfg_dirty_since = Some(Instant::now());
        }
    }
}
