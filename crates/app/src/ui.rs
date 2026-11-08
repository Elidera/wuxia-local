//! egui front-end.
//!
//! The *root* viewport is a hidden 1×1 px transparent window that only hosts
//! the control panel (F10) and the region picker. Translations over the game are
//! drawn by the native layered window in `overlay.rs`.

use crate::config::Config;
use crate::state::{Command, Shared};
use crate::win;
use eframe::egui::{
    self, Color32, FontData, FontDefinitions, FontFamily, FontId, Pos2, Rect, Rounding, Stroke, Vec2, ViewportBuilder,
    ViewportCommand, ViewportId,
};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Title of the hidden 1×1 root window (the overlay itself is `overlay::TITLE`).
pub const OVERLAY_TITLE: &str = "Wuxia Local Host";
pub const PANEL_TITLE: &str = "Wuxia Local — панель";

pub struct WuxiaApp {
    shared: Arc<Shared>,
    overlay_hwnd: Option<isize>,
    panel_hwnd: Option<isize>,
    /// Capture visibility currently applied to the panel.
    applied_visibility: Option<bool>,
    focus_frames: u32,
    region_tex: Option<egui::TextureHandle>,
    ocr_tex: Option<(u64, egui::TextureHandle)>,
    drag_start: Option<Pos2>,
    panel_state: crate::ui_panel::PanelState,
    cfg_dirty_since: Option<Instant>,
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
            applied_visibility: None,
            focus_frames: 0,
            region_tex: None,
            ocr_tex: None,
            drag_start: None,
            panel_state: crate::ui_panel::PanelState::default(),
            cfg_dirty_since: None,
        }
    }
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
        // Translations are drawn by the native overlay window (overlay.rs);
        // this 1×1 root window only hosts the panel and the region picker.
        let cfg = self.shared.cfg();
        let (selecting, panel_open) = {
            let u = self.shared.ui.lock().unwrap();
            (u.selecting_region, u.panel_open)
        };
        if selecting {
            self.region_window(ctx, &cfg);
        } else {
            self.region_tex = None;
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
                .with_icon(crate::mascot::icon_data())
                .with_active(true),
            |ctx, _class| {
                protect_now = true;
                if ctx.input(|i| i.viewport().close_requested()) {
                    // ✕ hides the panel; the program keeps running in the tray.
                    shared.ui(|u| u.panel_open = false);
                    shared.send(Command::PanelClosedToTray);
                }
                dirty |= crate::ui_panel::panel_ui(ctx, &shared, st, ocr_view.as_ref());
            },
        );
        if protect_now && self.panel_hwnd.is_none() {
            self.panel_hwnd = win::find_window(PANEL_TITLE);
        }
        if let Some(h) = self.panel_hwnd {
            let want = self.shared.cfg().general.visible_in_screenshots;
            if self.applied_visibility != Some(want) {
                if let Err(e) = win::set_capture_visible(h, want) {
                    tracing::warn!("панель: видимость на скриншотах: {e}");
                }
                self.applied_visibility = Some(want);
            }
        }
        if !self.shared.ui.lock().unwrap().panel_open {
            self.panel_hwnd = None;
            self.applied_visibility = None;
        }
        if dirty {
            self.cfg_dirty_since = Some(Instant::now());
        }
    }
}
