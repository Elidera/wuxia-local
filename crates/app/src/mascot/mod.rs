//! Енотик — the raccoon girl of the panel (art: assets/mascot/girl.png,
//! prepared by tools/mascot/prepare.py into raw RGBA so no image decoder is needed).
//!
//! One picture, brought to life with small animations that follow the program
//! state: bouncing while waiting, swaying while reading, dimmed with "z z Z"
//! while asleep, a sweat drop when something is wrong.

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use std::sync::{Arc, OnceLock};

const GIRL: &[u8] = include_bytes!("../../res/girl.rgba");
const GIRL_W: usize = 305;
const GIRL_H: usize = 400;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mood {
    /// Everything is fine, waiting for subtitles.
    Idle,
    /// A translation or the model download is in progress.
    Working,
    /// Dialogue translation is off or the game is not in front.
    Sleeping,
    /// Something needs the user's attention.
    Error,
}

impl Mood {
    pub fn phrase(self) -> &'static str {
        match self {
            Mood::Idle => "Жду субтитров! Переведу всё, что появится в игре.",
            Mood::Working => "Перевожу, перевожу…",
            Mood::Sleeping => "Сплю, пока игра не активна. Разбудите меня клавишей F7 или вернитесь в игру.",
            Mood::Error => "Ой, что-то не так. Загляните в «Подготовку» на главной.",
        }
    }
}

fn texture(ctx: &egui::Context) -> egui::TextureHandle {
    let id = egui::Id::new("wx_mascot_tex");
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return t;
    }
    let img = egui::ColorImage::from_rgba_unmultiplied([GIRL_W, GIRL_H], GIRL);
    let t = ctx.load_texture("wx_mascot", img, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, t.clone()));
    t
}

/// Draw the mascot into `rect` (aspect kept, standing on the bottom edge).
pub fn paint(ui: &Ui, rect: Rect, mood: Mood, time: f64) {
    let tex = texture(ui.ctx());
    let aspect = GIRL_W as f32 / GIRL_H as f32;
    let mut size = Vec2::new(rect.width(), rect.width() / aspect);
    if size.y > rect.height() {
        size = Vec2::new(rect.height() * aspect, rect.height());
    }
    let t = time as f32;
    // Mood animations: (vertical offset, squash, rotation, tint).
    let (dy, squash, angle, tint) = match mood {
        Mood::Idle => {
            let b = (t * 3.0).sin().abs();
            (-b * 4.0, 1.0 + (1.0 - b) * 0.015, 0.0, Color32::WHITE)
        }
        Mood::Working => (-(t * 6.0).sin().abs() * 2.0, 1.0, (t * 2.2).sin() * 0.05, Color32::WHITE),
        Mood::Sleeping => (0.0, 1.0 + (t * 1.3).sin() * 0.012, 0.0, Color32::from_gray(150)),
        Mood::Error => ((t * 18.0).sin() * 0.6, 1.0, (t * 18.0).sin() * 0.02, Color32::WHITE),
    };
    let h = size.y * squash;
    let w = size.x / squash.sqrt();
    let bottom = Pos2::new(rect.center().x, rect.bottom() + dy);
    let img_rect = Rect::from_min_max(Pos2::new(bottom.x - w / 2.0, bottom.y - h), Pos2::new(bottom.x + w / 2.0, bottom.y));
    egui::Image::from_texture(egui::load::SizedTexture::new(tex.id(), img_rect.size()))
        .tint(tint)
        .rotate(angle, Vec2::new(0.5, 1.0))
        .paint_at(ui, img_rect);

    // Overlays, positioned relative to the picture (head is at the top right).
    let at = |fx: f32, fy: f32| Pos2::new(img_rect.left() + fx * img_rect.width(), img_rect.top() + fy * img_rect.height());
    let painter = ui.painter();
    match mood {
        Mood::Sleeping => {
            let k0 = (time * 0.5).fract() as f32;
            for i in 0..3 {
                let k = (k0 + i as f32 / 3.0).fract();
                let a = (255.0 * (1.0 - k)) as u8;
                painter.text(
                    at(0.86 + k * 0.12, 0.12 - k * 0.12),
                    Align2::CENTER_CENTER,
                    "z",
                    FontId::proportional(img_rect.width() * (0.07 + 0.03 * i as f32)),
                    Color32::from_rgba_unmultiplied(235, 228, 214, a),
                );
            }
        }
        Mood::Working => {
            // "…" bubble.
            let c = at(0.93, 0.08);
            let r = img_rect.width() * 0.09;
            painter.circle(c, r, Color32::from_rgb(252, 244, 236), Stroke::new(1.0, Color32::from_rgb(150, 112, 80)));
            let n = ((time * 3.0) as usize) % 4;
            painter.text(c, Align2::CENTER_CENTER, &"•••"[..n * '•'.len_utf8()], FontId::proportional(r * 0.9), Color32::from_rgb(150, 112, 80));
        }
        Mood::Error => {
            let c = at(0.36, 0.17);
            let r = img_rect.width() * 0.035;
            painter.circle_filled(c, r, Color32::from_rgb(150, 205, 245));
            painter.add(egui::Shape::convex_polygon(
                vec![c + Vec2::new(-r * 0.95, -r * 0.3), c + Vec2::new(0.0, -r * 2.4), c + Vec2::new(r * 0.95, -r * 0.3)],
                Color32::from_rgb(150, 205, 245),
                Stroke::NONE,
            ));
        }
        Mood::Idle => {}
    }
}

/// The mascot as a widget with a speech-bubble tooltip. Keeps animating.
pub fn widget(ui: &mut Ui, size: Vec2, mood: Mood) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let time = ui.input(|i| i.time);
    paint(ui, rect, mood, time);
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(40));
    resp.on_hover_text(mood.phrase())
}

/// Window and tray icon (her face), 64×64 RGBA.
pub const ICON_64: &[u8] = include_bytes!("../../res/icon_64.rgba");
/// Tray icon, 32×32 RGBA.
pub const ICON_32: &[u8] = include_bytes!("../../res/icon_32.rgba");

pub fn icon_data() -> Arc<egui::IconData> {
    static ICON: OnceLock<Arc<egui::IconData>> = OnceLock::new();
    ICON.get_or_init(|| Arc::new(egui::IconData { rgba: ICON_64.to_vec(), width: 64, height: 64 })).clone()
}
