//! Control panel: dark "ink & gold" theme, sidebar navigation, cards.
//!
//! Pages: Главная (status, checklist, big actions, model) · Настройки ·
//! Словари · Проверка · Справка.

use crate::state::{Command, Shared};
use eframe::egui::{
    self, vec2, Align, Color32, FontFamily, FontId, Layout, Margin, RichText, Rounding, Sense, Stroke, TextStyle, Ui, Vec2,
};
use std::collections::HashMap;
use std::sync::Arc;

pub const BG: Color32 = Color32::from_rgb(20, 22, 27);
pub const CARD: Color32 = Color32::from_rgb(29, 32, 39);
pub const CARD_HI: Color32 = Color32::from_rgb(38, 42, 51);
pub const GOLD: Color32 = Color32::from_rgb(212, 165, 90);
pub const RED: Color32 = Color32::from_rgb(196, 92, 70);
pub const GREEN: Color32 = Color32::from_rgb(112, 184, 124);
pub const YELLOW: Color32 = Color32::from_rgb(226, 182, 84);
pub const TEXT: Color32 = Color32::from_rgb(234, 228, 216);
pub const WEAK: Color32 = Color32::from_rgb(150, 146, 138);

/// Donation page. Empty = the «Поддержать» button is hidden.
pub const DONATE_URL: &str = "";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Home,
    Settings,
    Dictionaries,
    Check,
    Help,
}

#[derive(Default)]
pub struct PanelState {
    pub tab: Tab,
    pub edits: HashMap<String, String>,
    pub test_input: String,
    pub settings_query: String,
}

pub struct OcrView {
    pub tex: egui::TextureId,
    pub size: Vec2,
    pub text: String,
    pub ms: u128,
}

/// Global look for all egui widgets (the overlay paints with explicit colours,
/// so it is not affected).
pub fn apply_theme(ctx: &egui::Context) {
    // Always dark, regardless of the Windows light/dark setting.
    ctx.set_theme(egui::ThemePreference::Dark);
    let mut style = (*ctx.style()).clone();
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = BG;
    v.extreme_bg_color = Color32::from_rgb(14, 15, 19);
    v.faint_bg_color = CARD;
    v.override_text_color = Some(TEXT);
    v.hyperlink_color = GOLD;
    v.selection.bg_fill = Color32::from_rgb(110, 84, 44);
    v.selection.stroke = Stroke::new(1.0, GOLD);
    v.window_rounding = Rounding::same(10.0);
    let r = Rounding::same(7.0);
    for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.rounding = r;
    }
    v.widgets.noninteractive.bg_fill = CARD;
    v.widgets.noninteractive.weak_bg_fill = CARD;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(44, 48, 58));
    v.widgets.inactive.bg_fill = CARD_HI;
    v.widgets.inactive.weak_bg_fill = CARD_HI;
    v.widgets.hovered.bg_fill = Color32::from_rgb(52, 57, 69);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(52, 57, 69);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, GOLD);
    v.widgets.active.bg_fill = Color32::from_rgb(120, 92, 48);
    v.widgets.active.weak_bg_fill = Color32::from_rgb(120, 92, 48);
    style.visuals = v;
    style.spacing.item_spacing = vec2(8.0, 8.0);
    style.spacing.button_padding = vec2(12.0, 6.0);
    style.spacing.slider_width = 220.0;
    style.spacing.interact_size.y = 26.0;
    style.text_styles = [
        (TextStyle::Heading, FontId::new(22.0, FontFamily::Name("bold".into()))),
        (TextStyle::Body, FontId::new(15.0, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(15.0, FontFamily::Proportional)),
        (TextStyle::Small, FontId::new(12.5, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(13.5, FontFamily::Monospace)),
    ]
    .into();
    ctx.set_style(style);
}

// ---------- small widgets ----------

fn card<R>(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::none()
        .fill(CARD)
        .rounding(Rounding::same(10.0))
        .stroke(Stroke::new(1.0, Color32::from_rgb(42, 46, 56)))
        .inner_margin(Margin::same(14.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if !title.is_empty() {
                ui.label(RichText::new(title).size(16.0).family(FontFamily::Name("bold".into())).color(GOLD));
                ui.add_space(2.0);
            }
            add(ui)
        })
        .inner
}

fn hint(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).small().color(WEAK));
}

fn key_chip(ui: &mut Ui, key: &str) {
    if key.is_empty() {
        return;
    }
    egui::Frame::none()
        .fill(Color32::from_rgb(14, 15, 19))
        .rounding(Rounding::same(4.0))
        .stroke(Stroke::new(1.0, Color32::from_rgb(70, 74, 84)))
        .inner_margin(Margin::symmetric(6.0, 1.0))
        .show(ui, |ui| {
            ui.label(RichText::new(key).monospace().size(12.0).color(WEAK));
        });
}

/// iOS-style switch (from the egui demo).
fn toggle(ui: &mut Ui, on: &mut bool) -> egui::Response {
    let size = vec2(2.2, 1.15) * ui.spacing().interact_size.y;
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool(resp.id, *on);
        let r = rect.height() / 2.0;
        let bg = if *on { Color32::from_rgb(150, 112, 56) } else { Color32::from_rgb(60, 64, 74) };
        ui.painter().rect_filled(rect, Rounding::same(r), bg);
        let cx = egui::lerp((rect.left() + r)..=(rect.right() - r), t);
        ui.painter().circle_filled(egui::pos2(cx, rect.center().y), r * 0.78, if *on { GOLD } else { Color32::from_gray(190) });
    }
    resp
}

// ---------- settings search ----------

#[derive(Default)]
struct Search {
    words: Vec<String>,
    /// Title of the card being drawn: a query matching the title shows the whole card.
    title: String,
}

thread_local! {
    static SEARCH: std::cell::RefCell<Search> = std::cell::RefCell::new(Search::default());
}

fn fold(s: &str) -> String {
    s.to_lowercase().replace('ё', "е")
}

fn search_set(query: &str) {
    SEARCH.with(|s| {
        let mut s = s.borrow_mut();
        s.words = fold(query).split_whitespace().map(str::to_string).collect();
        s.title.clear();
    });
}

/// Does `text` (plus the current card title) contain every word of the query?
fn search_hit(text: &str) -> bool {
    SEARCH.with(|s| {
        let s = s.borrow();
        if s.words.is_empty() {
            return true;
        }
        let hay = fold(&format!("{} {text}", s.title));
        s.words.iter().all(|w| hay.contains(w.as_str()))
    })
}

/// A settings card that hides itself when nothing in it matches the search.
/// `keys` lists the words of all its rows (labels and hints).
fn search_card(ui: &mut Ui, title: &str, keys: &str, add: impl FnOnce(&mut Ui)) -> bool {
    SEARCH.with(|s| s.borrow_mut().title = title.to_string());
    let shown = search_hit(keys);
    if shown {
        card(ui, title, add);
        ui.add_space(8.0);
    }
    SEARCH.with(|s| s.borrow_mut().title.clear());
    shown
}

fn setting_row(ui: &mut Ui, label: &str, help: &str, add: impl FnOnce(&mut Ui) -> bool) -> bool {
    if !search_hit(&format!("{label} {help}")) {
        return false;
    }
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_width((ui.available_width() * 0.5).max(180.0));
            ui.label(label);
            if !help.is_empty() {
                hint(ui, help);
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            changed = add(ui);
        });
    });
    changed
}

fn segmented(ui: &mut Ui, value: &mut String, options: &[(&str, &str)]) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for (v, label) in options {
            let selected = value == v;
            let text = RichText::new(*label).color(if selected { Color32::BLACK } else { TEXT });
            let b = egui::Button::new(text).fill(if selected { GOLD } else { CARD_HI }).rounding(Rounding::same(6.0));
            if ui.add(b).clicked() && !selected {
                *value = v.to_string();
                changed = true;
            }
        }
    });
    changed
}

fn big_action(ui: &mut Ui, title: &str, subtitle: &str, key: &str, active: bool, width: f32) -> bool {
    let fill = if active { Color32::from_rgb(64, 52, 30) } else { CARD_HI };
    let stroke = if active { Stroke::new(1.5, GOLD) } else { Stroke::new(1.0, Color32::from_rgb(52, 56, 66)) };
    let resp = egui::Frame::none()
        .fill(fill)
        .stroke(stroke)
        .rounding(Rounding::same(10.0))
        .inner_margin(Margin::same(12.0))
        .show(ui, |ui| {
            // The caller lays buttons out in a row: without an explicit vertical layout the
            // subtitle would be placed to the right of the title and widen the button.
            let w = (width - 26.0).max(120.0);
            ui.set_width(w);
            ui.set_max_width(w);
            ui.vertical(|ui| {
                ui.set_min_height(58.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(title).size(16.5).family(FontFamily::Name("bold".into())).color(if active { GOLD } else { TEXT }));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| key_chip(ui, key));
                });
                hint(ui, subtitle);
            });
        })
        .response
        .interact(Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.clicked()
}

fn status_pill(ui: &mut Ui, text: &str, color: Color32) {
    egui::Frame::none()
        .fill(color.gamma_multiply(0.18))
        .stroke(Stroke::new(1.0, color))
        .rounding(Rounding::same(12.0))
        .inner_margin(Margin::symmetric(10.0, 3.0))
        .show(ui, |ui| {
            ui.label(RichText::new(text).color(color).strong());
        });
}

fn check_item(ui: &mut Ui, ok: Option<bool>, title: &str, detail: &str) {
    ui.horizontal(|ui| {
        let (sym, col) = match ok {
            Some(true) => ("✔", GREEN),
            Some(false) => ("✖", RED),
            None => ("…", YELLOW),
        };
        ui.label(RichText::new(sym).size(17.0).color(col));
        ui.vertical(|ui| {
            ui.label(title);
            if !detail.is_empty() {
                hint(ui, detail);
            }
        });
    });
}

// ---------- the panel ----------

/// Returns true if the config was edited (debounced save).
pub fn panel_ui(ctx: &egui::Context, shared: &Arc<Shared>, st: &mut PanelState, ocr_view: Option<&OcrView>) -> bool {
    let mut dirty = false;

    egui::TopBottomPanel::top("wx_header").frame(egui::Frame::none().fill(Color32::from_rgb(16, 17, 21)).inner_margin(Margin::symmetric(16.0, 10.0))).show(ctx, |ui| {
        header(ui, shared);
    });

    egui::SidePanel::left("wx_nav")
        .exact_width(150.0)
        .resizable(false)
        .frame(egui::Frame::none().fill(Color32::from_rgb(16, 17, 21)).inner_margin(Margin::symmetric(10.0, 14.0)))
        .show(ctx, |ui| {
            for (tab, icon, label) in [
                (Tab::Home, "⌂", "Главная"),
                (Tab::Settings, "⚙", "Настройки"),
                (Tab::Dictionaries, "📖", "Словари"),
                (Tab::Check, "🔍", "Проверка"),
                (Tab::Help, "❓", "Справка"),
            ] {
                let selected = st.tab == tab;
                let text = RichText::new(format!("{icon}  {label}")).size(15.5).color(if selected { GOLD } else { TEXT });
                let b = egui::Button::new(text)
                    .fill(if selected { Color32::from_rgb(40, 36, 28) } else { Color32::TRANSPARENT })
                    .stroke(if selected { Stroke::new(1.0, Color32::from_rgb(110, 86, 48)) } else { Stroke::NONE })
                    .rounding(Rounding::same(8.0))
                    .min_size(vec2(130.0, 34.0));
                if ui.add(b).clicked() {
                    st.tab = tab;
                }
            }
            ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
                ui.label(RichText::new(crate::build_info()).small().color(WEAK));
                ui.add_space(2.0);
                let quit = egui::Button::new(RichText::new("⏻  Выход").color(WEAK)).fill(Color32::TRANSPARENT).min_size(vec2(130.0, 26.0));
                if ui.add(quit).on_hover_text("Полностью закрыть программу. Крестик окна только прячет панель в трей.").clicked() {
                    shared.send(Command::Quit);
                }
                if !DONATE_URL.is_empty() {
                    let b = egui::Button::new(RichText::new("♥  Поддержать").color(Color32::BLACK))
                        .fill(GOLD)
                        .rounding(Rounding::same(8.0))
                        .min_size(vec2(130.0, 30.0));
                    if ui.add(b).on_hover_text("Программа бесплатная. Если она вам помогает — можно угостить енотика печенькой.").clicked() {
                        shared.send(Command::OpenUrl(DONATE_URL.to_string()));
                    }
                }
                ui.add_space(4.0);
                let space = ui.available_height();
                if space > 90.0 {
                    let h = space.min(170.0) - 6.0;
                    let r = crate::mascot::widget(ui, vec2(130.0, h), mascot_mood(shared));
                    if r.clicked() && !DONATE_URL.is_empty() {
                        shared.send(Command::OpenUrl(DONATE_URL.to_string()));
                    }
                }
            });
        });

    egui::CentralPanel::default().frame(egui::Frame::none().fill(BG).inner_margin(Margin::same(16.0))).show(ctx, |ui| {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.set_max_width(ui.available_width());
            dirty |= match st.tab {
                Tab::Home => page_home(ui, shared),
                Tab::Settings => page_settings(ui, shared, &mut st.settings_query),
                Tab::Dictionaries => page_dictionaries(ui, shared, &mut st.edits),
                Tab::Check => page_check(ui, shared, &mut st.test_input, ocr_view),
                Tab::Help => page_help(ui, shared),
            };
        });
    });
    dirty
}

fn mascot_mood(shared: &Arc<Shared>) -> crate::mascot::Mood {
    use crate::mascot::Mood;
    use crate::state::ServerStatus;
    let u = shared.ui.lock().unwrap();
    let error = matches!(u.server, Some(ServerStatus::Failed(_))) || matches!(u.ocr_status, Some(Err(_)));
    let working = u.dialogue.as_ref().map_or(false, |d| d.pending)
        || u.interface.as_ref().map_or(false, |i| i.pending > 0)
        || !matches!(u.server, Some(ServerStatus::Ready { .. }) | Some(ServerStatus::Failed(_)));
    if error {
        Mood::Error
    } else if working {
        Mood::Working
    } else if !u.dialogue_on || u.paused_unfocused {
        Mood::Sleeping
    } else {
        Mood::Idle
    }
}

fn header(ui: &mut Ui, shared: &Arc<Shared>) {
    let (game, lang) = {
        let c = shared.cfg();
        (crate::config::profile(&c.general.profile).name.to_string(), c.source_lang())
    };
    let (server, dialogue_on, paused) = {
        let u = shared.ui.lock().unwrap();
        (u.server.clone(), u.dialogue_on, u.paused_unfocused)
    };
    ui.horizontal(|ui| {
        // Seal-like logo.
        let (rect, _) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::hover());
        ui.painter().rect_filled(rect, Rounding::same(6.0), RED);
        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "译", FontId::proportional(21.0), Color32::from_rgb(250, 240, 225));
        ui.add_space(6.0);
        ui.vertical(|ui| {
            ui.label(RichText::new("Wuxia Local").size(19.0).family(FontFamily::Name("bold".into())));
            ui.label(RichText::new(format!("{game} · {} → русский", lang.name_ru())).small().color(WEAK));
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let mut on = dialogue_on;
            if toggle(ui, &mut on).on_hover_text("Перевод субтитров (F7)").clicked() {
                shared.send(Command::ToggleDialogue);
            }
            ui.label(RichText::new("Диалоги").color(if dialogue_on { GOLD } else { WEAK }));
            ui.add_space(12.0);
            match server {
                Some(s) if s.is_ready() => {
                    if dialogue_on && paused {
                        status_pill(ui, "Ждёт окно игры", YELLOW)
                    } else {
                        status_pill(ui, "Готов", GREEN)
                    }
                }
                Some(crate::state::ServerStatus::Failed(_)) => status_pill(ui, "Ошибка модели", RED),
                Some(crate::state::ServerStatus::Starting(_)) => status_pill(ui, "Загрузка модели…", YELLOW),
                _ => status_pill(ui, "Запуск…", YELLOW),
            }
        });
    });
}

fn page_home(ui: &mut Ui, shared: &Arc<Shared>) -> bool {
    let cfg = shared.cfg();
    let hk = cfg.hotkeys.clone();
    let (server, dialogue_on, show_original, stats, notices, ocr_status, has_interface) = {
        let u = shared.ui.lock().unwrap();
        (
            u.server.clone(),
            u.dialogue_on,
            u.show_original,
            u.stats.clone(),
            u.notices.iter().cloned().collect::<Vec<_>>(),
            u.ocr_status.clone(),
            u.interface.is_some(),
        )
    };

    // Checklist until everything is ready.
    let model_ok = server.as_ref().map(|s| s.is_ready());
    let ocr_ok = ocr_status.as_ref().map(|r| r.is_ok());
    let auto_mode = cfg.dialogue.mode == "auto";
    let region_ok = auto_mode || cfg.dialogue.region_set;
    if model_ok != Some(true) || ocr_ok != Some(true) || !region_ok {
        card(ui, "Подготовка", |ui| {
            let model_detail = match &server {
                Some(crate::state::ServerStatus::Starting(s)) => s.clone(),
                Some(crate::state::ServerStatus::Failed(e)) => e.clone(),
                Some(crate::state::ServerStatus::Ready { model }) => model.clone(),
                _ => "запускается…".into(),
            };
            check_item(
                ui,
                match &server {
                    Some(s) if s.is_ready() => Some(true),
                    Some(crate::state::ServerStatus::Failed(_)) => Some(false),
                    _ => None,
                },
                "Нейросеть загружена",
                &model_detail,
            );
            let ocr_detail = match &ocr_status {
                Some(Err(e)) => e.clone(),
                _ => String::new(),
            };
            let ocr_title = match cfg.source_lang() {
                wuxia_core::lang::Lang::En => "Распознавание английского текста (Windows OCR)",
                wuxia_core::lang::Lang::Zh => "Распознавание китайского текста (Windows OCR)",
            };
            check_item(ui, ocr_ok, ocr_title, &ocr_detail);
            ui.horizontal(|ui| {
                let (title, detail) = if auto_mode {
                    ("Текст ищется по всему экрану (авто)", "")
                } else if region_ok {
                    ("Область субтитров выбрана", "")
                } else {
                    ("Область субтитров выбрана", "Откройте игру с субтитрами и выберите область")
                };
                check_item(ui, Some(region_ok), title, detail);
                if !region_ok {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Выбрать…").clicked() {
                            shared.send(Command::SelectRegion);
                        }
                    });
                }
            });
            check_item(ui, None, "Игра в режиме «Оконный без рамки»", "В полноэкранном режиме Windows не показывает перевод поверх игры");
        });
        ui.add_space(8.0);
    }

    // Big actions.
    let w = ((ui.available_width() - ui.spacing().item_spacing.x) / 2.0).floor();
    ui.horizontal(|ui| {
        let (title, sub) = if dialogue_on { ("Диалоги включены", "Субтитры переводятся автоматически") } else { ("Диалоги выключены", "Нажмите, чтобы переводить субтитры") };
        if big_action(ui, title, sub, &hk.toggle_dialogue, dialogue_on, w) {
            shared.send(Command::ToggleDialogue);
        }
        if big_action(ui, "Перевести экран", "Меню, задания, предметы — по нажатию", &hk.screen_translate, has_interface, w) {
            // ScreenTranslate hides the panel itself and waits until it is off the screen.
            shared.send(Command::ScreenTranslate);
        }
    });
    ui.horizontal(|ui| {
        let sub = if show_original { "Сейчас показан оригинал" } else { "Сейчас показан перевод" };
        if big_action(ui, "Оригинал / перевод", sub, &hk.show_original, show_original, w) {
            shared.send(Command::ToggleOriginal);
        }
        if big_action(ui, "Область субтитров", "Необязательно: следить только за полосой субтитров", &hk.select_region, false, w) {
            shared.send(Command::SelectRegion);
        }
    });
    ui.add_space(8.0);

    // Model.
    card(ui, "Нейросеть-переводчик", |ui| {
        let mut active = cfg.llm.active_model.clone();
        let current = cfg.active_model();
        egui::ComboBox::from_label("").selected_text(&current.name).width(ui.available_width() - 4.0).show_ui(ui, |ui| {
            for m in &cfg.models {
                let downloaded = !m.path.is_empty() && std::path::Path::new(&m.path).exists();
                let label = if downloaded { m.name.clone() } else { format!("{}  · скачается {:.1} ГБ", m.name, m.size_gb) };
                ui.selectable_value(&mut active, m.id.clone(), label);
            }
        });
        if active != cfg.llm.active_model {
            shared.send(Command::SwitchModel(active));
        }
        hint(
            ui,
            match current.id.as_str() {
                "hymt2-1.8b" => "Быстрая специализированная модель перевода. Подходит большинству ПК.",
                "hymt2-7b" => "Самый качественный и литературный перевод. Нужна видеокарта с 6+ ГБ памяти.",
                id if id.starts_with("qwen") => "Универсальная модель. В тестах переводит хуже Hy-MT2.",
                _ => "",
            },
        );
        ui.horizontal(|ui| {
            if let Some(s) = &server {
                ui.label(RichText::new(s.label()).small().color(WEAK));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("Перезапустить").clicked() {
                    shared.send(Command::RestartServer);
                }
                if ui.small_button("Свой файл модели…").on_hover_text("Если модель не скачивается: скачайте файл .gguf сами и укажите его здесь").clicked() {
                    let sh = shared.clone();
                    std::thread::spawn(move || {
                        if let Some(p) = crate::filepick::pick_gguf() {
                            sh.send(Command::UseModelFile(p));
                        }
                    });
                }
            });
        });
        // The first download is slow or fails (HuggingFace unreachable): offer the manual way.
        let downloading = !std::path::Path::new(&current.path).exists();
        let stuck = match &server {
            Some(crate::state::ServerStatus::Failed(_)) => true,
            Some(crate::state::ServerStatus::Starting(m)) => m.contains("первая загрузка"),
            _ => false,
        };
        if downloading && stuck {
            hint(ui, "Не скачивается? Скачайте файл модели в браузере (файл .gguf с «Q4_K_M» в названии) и нажмите «Свой файл модели…».");
            let repo = current.hf.split(':').next().unwrap_or("");
            if !repo.is_empty() {
                ui.hyperlink_to(format!("huggingface.co/{repo}"), format!("https://huggingface.co/{repo}/tree/main"));
            }
        }
    });
    ui.add_space(8.0);

    // Stats.
    card(ui, "", |ui| {
        ui.columns(4, |c| {
            stat(&mut c[0], &format!("{:.0} мс", stats.ocr_ms_avg), "распознавание");
            stat(&mut c[1], &format!("{:.1} с", stats.llm_ms_avg / 1000.0), "перевод");
            stat(&mut c[2], &stats.translated.to_string(), "переведено");
            stat(&mut c[3], &stats.cache_hits.to_string(), "из памяти");
        });
    });

    if !notices.is_empty() {
        ui.add_space(8.0);
        card(ui, "Сообщения", |ui| {
            for n in notices.iter().rev().take(6) {
                ui.label(RichText::new(format!("• {n}")).color(Color32::from_rgb(230, 200, 170)));
            }
            if ui.small_button("Очистить").clicked() {
                shared.ui(|u| u.notices.clear());
            }
        });
    }
    false
}

fn stat(ui: &mut Ui, value: &str, label: &str) {
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(value).size(19.0).family(FontFamily::Name("bold".into())).color(GOLD));
        ui.label(RichText::new(label).small().color(WEAK));
    });
}

fn page_settings(ui: &mut Ui, shared: &Arc<Shared>, query: &mut String) -> bool {
    let mut dirty = false;
    ui.heading("Настройки");
    hint(ui, "Изменения применяются сразу и сохраняются автоматически.");
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(query).hint_text("🔍  Поиск по настройкам: «шрифт», «скрин», «F7»…").desired_width((ui.available_width() - 40.0).max(160.0)));
        if !query.is_empty() && ui.small_button("✕").on_hover_text("Очистить поиск").clicked() {
            query.clear();
        }
    });
    ui.add_space(8.0);
    search_set(query);
    let mut c = shared.cfg.write().unwrap();
    let mut game_changed = false;
    let mut any = false;

    any |= search_card(ui, "Игра", "игра wuthering waves where winds meet другая словарь терминов окно язык текста английский китайский самой заголовок окна название нейросети", |ui| {
        let g = &mut c.general;
        let options: Vec<(&str, &str)> = crate::config::PROFILES.iter().map(|p| (p.id, p.name)).collect();
        let mut prof = g.profile.clone();
        if setting_row(ui, "Игра", "Wuthering Waves, Where Winds Meet или другая: подставляет словарь терминов и узнаёт окно игры", |ui| segmented(ui, &mut prof, &options)) {
            g.profile = prof;
            g.source_lang = crate::config::profile(&g.profile).langs[0].into();
            game_changed = true;
        }
        if setting_row(ui, "Язык текста в игре", "Английский или китайский — как выбрано в самой игре", |ui| {
            segmented(ui, &mut g.source_lang, &[("zh", "Китайский"), ("en", "Английский")])
        }) {
            game_changed = true;
        }
        if g.profile == "custom" {
            dirty |= setting_row(ui, "Заголовок окна игры", "Часть названия окна; несколько вариантов — через |", |ui| {
                ui.add(egui::TextEdit::singleline(&mut g.window_match).desired_width(220.0)).changed()
            });
            dirty |= setting_row(ui, "Название игры", "Для подсказки нейросети", |ui| {
                ui.add(egui::TextEdit::singleline(&mut g.game_title).desired_width(220.0)).changed()
            });
        }
    });
    if game_changed {
        c.apply_profile();
    }

    any |= search_card(ui, "Перевод субтитров", "своё окно перевода переместить изменить размер двигать где искать текст весь экран авто только область субтитров частота чтения экрана нагрузка процессор размер шрифта затемнение подложки где показывать перевод поверх над под субтитрами помнить реплик контекста связности диалога отделять имя говорящего реплика пожелание к стилю qwen", |ui| {
        let d = &mut c.dialogue;
        dirty |= setting_row(ui, "Где искать текст", "Весь экран (авто) — без выбора области; область — только выделенная полоса субтитров", |ui| {
            segmented(ui, &mut d.mode, &[("auto", "Весь экран (авто)"), ("region", "Только область")])
        });
        if d.mode == "auto" {
            dirty |= setting_row(ui, "Частота чтения экрана", "Реже — меньше нагрузка на процессор", |ui| {
                ui.add(egui::Slider::new(&mut d.auto_interval_ms, 250..=2000).suffix(" мс")).changed()
            });
        }
        dirty |= setting_row(ui, "Размер шрифта", "", |ui| ui.add(egui::Slider::new(&mut d.font_size, 14.0..=56.0).suffix(" пт")).changed());
        dirty |= setting_row(ui, "Затемнение подложки", "0 — без подложки", |ui| {
            ui.add(egui::Slider::new(&mut d.backdrop_opacity, 0.0..=1.0).custom_formatter(|v, _| format!("{:.0} %", v * 100.0))).changed()
        });
        dirty |= setting_row(ui, "Где показывать перевод", "Поверх, над, под субтитрами игры — или в своём окне, которое можно двигать", |ui| {
            segmented(ui, &mut d.position, &[("box", "Своё окно"), ("over", "Поверх"), ("above", "Над"), ("below", "Под")])
        });
        let editing = shared.ui.lock().unwrap().edit_box;
        if setting_row(ui, "Окно перевода", "Перетащите мышью, тяните за край — размер, колесо — шрифт. Правый клик или кнопка — готово", |ui| {
            let label = if editing { "Готово" } else { "Переместить / изменить размер" };
            ui.add(egui::Button::new(RichText::new(label).color(Color32::BLACK)).fill(GOLD)).clicked()
        }) {
            shared.send(Command::EditBox);
        }
        dirty |= setting_row(ui, "Помнить реплик для контекста", "Помогает связности диалога", |ui| ui.add(egui::Slider::new(&mut d.context_lines, 0..=6)).changed());
        dirty |= setting_row(ui, "Отделять имя говорящего", "«Имя：реплика» — имя переводится отдельно", |ui| toggle(ui, &mut d.split_speaker).changed());
        dirty |= setting_row(ui, "Пожелание к стилю", "Только для моделей Qwen", |ui| {
            ui.add(egui::TextEdit::singleline(&mut d.style_hint).hint_text("например: торжественнее").desired_width(220.0)).changed()
        });
    });

    any |= search_card(ui, "Распознавание субтитров", "цвет текста субтитров тёмный светлый порог яркости бледный текст мусора повышать контраст пёстром фоне частота проверки нагрузка процессор", |ui| {
        let d = &mut c.dialogue;
        dirty |= setting_row(ui, "Цвет текста субтитров", "Тёмный или светлый", |ui| segmented(ui, &mut d.text_color, &[("dark", "Тёмный"), ("light", "Светлый")]));
        dirty |= setting_row(ui, "Порог яркости текста", "Ниже — ловит бледный текст, выше — меньше мусора", |ui| {
            ui.add(egui::Slider::new(&mut d.ink_threshold, 120..=250)).changed()
        });
        let mut contrast = d.preprocess == "contrast";
        if setting_row(ui, "Повышать контраст", "Если текст на пёстром фоне плохо читается", |ui| toggle(ui, &mut contrast).changed()) {
            d.preprocess = if contrast { "contrast".into() } else { "none".into() };
            dirty = true;
        }
        dirty |= setting_row(ui, "Частота проверки", "Реже — меньше нагрузка на процессор", |ui| {
            ui.add(egui::Slider::new(&mut d.interval_ms, 80..=500).suffix(" мс")).changed()
        });
    });

    any |= search_card(ui, "Перевод экрана", "как показывать списком подложкой заменой текста размер шрифта высоты исходной надписи скрывать при смене экрана меню закрылось", |ui| {
        let i = &mut c.interface;
        dirty |= setting_row(ui, "Как показывать", "Списком, подложкой или заменой текста", |ui| segmented(ui, &mut i.mode, &[("list", "Списком"), ("backdrop", "Подложка"), ("replace", "Замена")]));
        dirty |= setting_row(ui, "Размер шрифта", "Относительно высоты исходной надписи", |ui| {
            ui.add(egui::Slider::new(&mut i.font_scale, 0.5..=1.2).custom_formatter(|v, _| format!("{:.0} %", v * 100.0))).changed()
        });
        dirty |= setting_row(ui, "Скрывать при смене экрана", "Перевод исчезает, когда меню закрылось", |ui| toggle(ui, &mut i.auto_hide).changed());
    });

    any |= search_card(ui, "Прочее", "видно на скриншотах скриншот перевод использовать видеокарту тормозит модель процессор", |ui| {
        let g = &mut c.general;
        dirty |= setting_row(ui, "Видно на скриншотах", "Включайте, чтобы сделать скриншот с переводом", |ui| toggle(ui, &mut g.visible_in_screenshots).changed());
        let l = &mut c.llm;
        let mut gpu = l.gpu_layers > 0;
        if setting_row(ui, "Использовать видеокарту", "Выключите, если игра тормозит — модель уйдёт на процессор", |ui| toggle(ui, &mut gpu).changed()) {
            l.gpu_layers = if gpu { 99 } else { 0 };
            dirty = true;
            shared.send(Command::RestartServer);
        }
    });
    ui.add_space(8.0);

    let hk = c.hotkeys.clone();
    let keys = format!(
        "горячие клавиши {} {} {} {} {} перевод диалогов экран оригинал область субтитров панель config.toml",
        hk.toggle_dialogue, hk.screen_translate, hk.show_original, hk.select_region, hk.panel
    );
    any |= search_card(ui, "Горячие клавиши", &keys, |ui| {
        for (k, what) in [
            (&hk.toggle_dialogue, "Включить / выключить перевод диалогов"),
            (&hk.screen_translate, "Перевести экран / убрать перевод"),
            (&hk.show_original, "Оригинал / перевод"),
            (&hk.select_region, "Выбрать область субтитров"),
            (&hk.panel, "Открыть / закрыть эту панель"),
        ] {
            if !search_hit(&format!("{k} {what}")) {
                continue;
            }
            ui.horizontal(|ui| {
                key_chip(ui, k);
                ui.label(what);
            });
        }
        hint(ui, "Изменить клавиши можно в config.toml (кнопка ниже), затем перезапустить программу.");
        if ui.button("Открыть config.toml").clicked() {
            shared.send(Command::Open(shared.paths.config()));
        }
    });
    drop(c);
    search_set("");
    if !any {
        hint(ui, "Ничего не найдено. Попробуйте другое слово.");
    }
    if game_changed {
        shared.send(Command::GameChanged);
    }
    dirty
}

fn page_dictionaries(ui: &mut Ui, shared: &Arc<Shared>, edits: &mut HashMap<String, String>) -> bool {
    let (recent, counts) = {
        let u = shared.ui.lock().unwrap();
        (u.recent.iter().cloned().collect::<Vec<_>>(), u.dict_counts)
    };
    ui.heading("Словари");
    hint(ui, "Так переводчик узнаёт имена, термины и ваш стиль. Файлы открываются в Блокноте; после правки нажмите «Перечитать».");
    ui.add_space(6.0);

    let w = (ui.available_width() - 16.0) / 3.0;
    ui.horizontal(|ui| {
        for (title, sub, path) in [
            ("Глоссарий", format!("{} терминов", counts.0), shared.paths.user_glossary()),
            ("Исправления OCR", format!("{} правил", counts.1), shared.paths.fixes()),
            ("Мои переводы", "исправленные реплики".to_string(), shared.paths.corrections()),
        ] {
            ui.allocate_ui(vec2(w, 90.0), |ui| {
                card(ui, title, |ui| {
                    hint(ui, &sub);
                    if ui.button("Открыть").clicked() {
                        shared.send(Command::Open(path));
                    }
                });
            });
        }
    });
    if ui.button("⟳  Перечитать словари").clicked() {
        shared.send(Command::ReloadGlossary);
    }
    ui.add_space(8.0);

    card(ui, "Последние реплики", |ui| {
        hint(ui, "Исправьте перевод и сохраните — эта реплика всегда будет переводиться так.");
        if recent.is_empty() {
            ui.label(RichText::new("Пока пусто.").color(WEAK));
        }
        for r in recent.iter().take(30) {
            egui::Frame::none().fill(CARD_HI).rounding(Rounding::same(8.0)).inner_margin(Margin::same(10.0)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(&r.src).size(16.0).color(Color32::from_rgb(200, 190, 170)));
                let e = edits.entry(r.src.clone()).or_insert_with(|| r.dst.clone());
                ui.add(egui::TextEdit::multiline(e).desired_rows(1).desired_width(f32::INFINITY));
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{} · {} мс{}", r.model, r.ms, if r.cached { " · из памяти" } else { "" })).small().color(WEAK));
                    if *e != r.dst {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.add(egui::Button::new(RichText::new("Сохранить").color(Color32::BLACK)).fill(GOLD)).clicked() {
                                shared.send(Command::SaveCorrection { src: r.src.clone(), dst: e.clone() });
                            }
                        });
                    }
                });
            });
        }
    });
    false
}

fn page_check(ui: &mut Ui, shared: &Arc<Shared>, test_input: &mut String, ocr_view: Option<&OcrView>) -> bool {
    let (test_result, bench) = {
        let u = shared.ui.lock().unwrap();
        (u.test_result.clone(), u.bench_progress.clone())
    };
    ui.heading("Проверка");
    ui.add_space(6.0);
    card(ui, "Видно ли перевод поверх игры", |ui| {
        hint(ui, "Нажмите кнопку и сразу переключитесь в игру (Alt+Tab). 10 секунд в центре экрана должна гореть надпись. Если её не видно — перевод тоже не будет виден: напишите об этом.");
        if ui.add(egui::Button::new(RichText::new("Показать надпись поверх игры").color(Color32::BLACK)).fill(GOLD)).clicked() {
            shared.send(Command::TestOverlay);
        }
    });
    ui.add_space(8.0);
    card(ui, "Перевести текст", |ui| {
        hint(ui, "Вставьте английский или китайский текст — например, из видео или с сайта — и нажмите Enter.");
        let resp = ui.add(egui::TextEdit::multiline(test_input).hint_text("少侠，此去开封，路途遥远……").desired_rows(2).desired_width(f32::INFINITY));
        let enter = resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
        if (ui.add(egui::Button::new(RichText::new("Перевести").color(Color32::BLACK)).fill(GOLD)).clicked() || enter) && !test_input.trim().is_empty() {
            shared.send(Command::TestTranslate(test_input.trim().to_string()));
        }
        if let Some(r) = &test_result {
            egui::Frame::none().fill(CARD_HI).rounding(Rounding::same(8.0)).inner_margin(Margin::same(10.0)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(r).size(17.0));
            });
        }
    });
    ui.add_space(8.0);
    card(ui, "Что видит распознавание", |ui| match ocr_view {
        Some(v) => {
            let w = ui.available_width().min(v.size.x);
            let h = v.size.y * w / v.size.x.max(1.0);
            ui.image(egui::load::SizedTexture::new(v.tex, vec2(w, h)));
            ui.label(RichText::new(format!("Распознано за {} мс:", v.ms)).small().color(WEAK));
            ui.label(RichText::new(if v.text.is_empty() { "(ничего)" } else { v.text.as_str() }).monospace());
            hint(ui, "Видите мусор? В «Настройках» подвиньте порог яркости или включите повышение контраста.");
        }
        None => {
            hint(ui, "Включите диалоги и дождитесь субтитров в игре, держа эту страницу открытой.");
        }
    });
    ui.add_space(8.0);
    card(ui, "Сравнить модели", |ui| {
        hint(ui, "Переводит 25 тестовых реплик всеми моделями и открывает отчёт. Модели, которых нет, будут скачаны.");
        match bench {
            Some(b) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(b);
                });
            }
            None => {
                if ui.button("Запустить сравнение").clicked() {
                    shared.send(Command::RunBench);
                }
            }
        }
    });
    false
}

fn page_help(ui: &mut Ui, shared: &Arc<Shared>) -> bool {
    let hk = shared.cfg().hotkeys;
    ui.heading("Справка");
    ui.add_space(6.0);
    card(ui, "Быстрый старт", |ui| {
        for (i, s) in [
            "В игре включите режим экрана «Оконный без рамки» (Borderless).".to_string(),
            format!("Дождитесь субтитров и нажмите {} — обведите область, где они появляются.", hk.select_region),
            format!("Перевод диалогов включается и выключается клавишей {}.", hk.toggle_dialogue),
            format!("Чтобы перевести меню или описание предмета, откройте его и нажмите {}. Повторное нажатие убирает перевод.", hk.screen_translate),
            "Плохой перевод? Исправьте его в «Словари → Последние реплики» — он запомнится.".to_string(),
        ]
        .iter()
        .enumerate()
        {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format!("{}.", i + 1)).color(GOLD).strong());
                ui.label(s);
            });
        }
    });
    ui.add_space(8.0);
    card(ui, "Если что-то не работает", |ui| {
        for (q, a) in [
            ("Перевод не появляется", "Проверьте, что диалоги включены и область выбрана. На странице «Проверка» видно, что распознаётся."),
            ("Распознаётся мусор", "Обведите область плотнее вокруг субтитров; в «Настройках» подвиньте порог яркости."),
            ("Игра тормозит", "В «Настройках → Прочее» выключите «Использовать видеокарту» или выберите модель 1.8B."),
            ("Перевод поверх игры не виден", "Игра должна быть в режиме «Оконный без рамки», не «Полный экран»."),
            ("Клавиши не работают", "Их может занимать другая программа — смотрите «Сообщения» на главной."),
        ] {
            ui.label(RichText::new(q).strong());
            hint(ui, a);
            ui.add_space(2.0);
        }
    });
    ui.add_space(8.0);
    card(ui, "Файлы", |ui| {
        ui.horizontal_wrapped(|ui| {
            if ui.button("Папка данных").clicked() {
                shared.send(Command::Open(shared.paths.data.clone()));
            }
            if ui.button("Логи").clicked() {
                shared.send(Command::Open(shared.paths.logs()));
            }
            if ui.button("config.toml").clicked() {
                shared.send(Command::Open(shared.paths.config()));
            }
        });
        hint(ui, &format!("Данные: {}", shared.paths.data.display()));
    });
    false
}
