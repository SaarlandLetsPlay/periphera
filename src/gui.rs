//! Oberfläche: alle angeschlossenen Geräte zentral verwalten.

use crate::animate;
use crate::dpi;
use crate::hidpp::{Hidpp, HIDPP_USAGE_PAGES, VENDOR_ID};
use crate::lighting::{self, Effect};
use crate::input;
use crate::macros::{self, MacroDef};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use hidapi::HidApi;
use std::collections::HashSet;
use std::ffi::CString;
use std::process::{Child, Command};

// Farbschema: dunkle Oberfläche mit türkisem Akzent und violetter Zweitfarbe.
const BG: Color32 = Color32::from_rgb(12, 15, 21);
const PANEL: Color32 = Color32::from_rgb(18, 22, 30);
const CARD: Color32 = Color32::from_rgb(26, 32, 43);
const CARD_HOVER: Color32 = Color32::from_rgb(34, 42, 56);
const ACCENT: Color32 = Color32::from_rgb(45, 212, 191);
const VIOLET: Color32 = Color32::from_rgb(124, 58, 237);
const TEXT: Color32 = Color32::from_rgb(230, 237, 243);
const MUTED: Color32 = Color32::from_rgb(139, 152, 169);
const OK: Color32 = Color32::from_rgb(74, 222, 128);
const OFF: Color32 = Color32::from_rgb(100, 110, 125);

const RADIUS: u8 = 12;

/// Fenster-Icon: 256x256 RGBA, aus assets/logo.svg erzeugt.
static WINDOW_ICON: &[u8] = include_bytes!("../assets/icon-256.rgba");

struct DpiState {
    current: u16,
    values: Vec<u16>,
    /// Index in `values` für den Schieberegler.
    index: usize,
}

struct DeviceEntry {
    name: String,
    kind: Kind,
    id: String,
    path: CString,
    dpi: Option<DpiState>,
    light: bool,
    /// Name des Tastatur-Eingangs in evdev (für Makros und Tastenerfassung).
    evdev: String,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Mouse,
    Keyboard,
    Other,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Mouse => "Maus",
            Kind::Keyboard => "Tastatur",
            Kind::Other => "Gerät",
        }
    }
}

pub struct App {
    api: HidApi,
    devices: Vec<DeviceEntry>,
    selected: Option<usize>,
    status: String,
    engine: Option<Child>,
    macro_path: String,
    macro_text: String,
    light_color: [u8; 3],
    light_mode: LightMode,
    light_period_ms: u16,
    light_hell: u32,
    anim: Option<Child>,
    defs: Vec<MacroDef>,
    sel_macro: Option<usize>,
    key_filter: String,
    /// Laufende Tastenerfassung: (Gerät, Empfänger, Abbruch-Schalter)
    learn: Option<(String, Receiver<Result<String, String>>, Arc<AtomicBool>)>,
    engine_before_learn: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum LightMode {
    Static,
    Breathing,
    Cycle,
    Knight,
    Welle,
    Regenbogen,
    Feuer,
}

impl LightMode {
    fn is_animation(self) -> bool {
        !matches!(self, LightMode::Static)
    }
    fn cli_name(self) -> &'static str {
        match self {
            LightMode::Knight => "knight",
            LightMode::Welle => "welle",
            LightMode::Regenbogen => "regenbogen",
            LightMode::Feuer => "feuer",
            LightMode::Breathing => "atmen",
            LightMode::Cycle => "farbwechsel",
            LightMode::Static => "",
        }
    }
}

fn kind_of(name: &str) -> Kind {
    let n = name.to_lowercase();
    if n.contains("mouse") {
        Kind::Mouse
    } else if n.contains("keyboard") {
        Kind::Keyboard
    } else {
        Kind::Other
    }
}

fn apply_theme(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = CARD;
    // Farbe der Reglerleiste (Slider-Spur): heller als die Karte, damit sie sichtbar bleibt.
    v.extreme_bg_color = Color32::from_rgb(58, 70, 90);
    v.selection.bg_fill = ACCENT.gamma_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active] {
        w.corner_radius = CornerRadius::same(RADIUS);
    }
    // bg_fill steuert die Spur von Reglern; etwas heller als die Karte, damit sie sichtbar ist.
    v.widgets.inactive.bg_fill = Color32::from_rgb(58, 70, 90);
    v.widgets.inactive.weak_bg_fill = CARD;
    v.widgets.hovered.bg_fill = CARD_HOVER;
    v.widgets.hovered.weak_bg_fill = CARD_HOVER;
    v.widgets.active.bg_fill = ACCENT;
    v.widgets.active.weak_bg_fill = ACCENT;
    v.override_text_color = Some(TEXT);
    ctx.set_visuals(v);
}

/// Zeichnet das Logo (abgerundetes Quadrat, Signalbögen, Mittelpunkt) in das gegebene Rechteck.
fn draw_logo(painter: &egui::Painter, rect: Rect) {
    let s = rect.width();
    painter.rect_filled(rect, s * 0.23, ACCENT);
    painter.rect_filled(
        Rect::from_min_max(Pos2::new(rect.center().x, rect.center().y), rect.max),
        s * 0.23,
        VIOLET.gamma_multiply(0.35),
    );
    let c = Pos2::new(rect.center().x, rect.min.y + s * 0.62);
    for (radius, alpha) in [(s * 0.33, 150u8), (s * 0.48, 80u8)] {
        let pts: Vec<Pos2> = (0..=24)
            .map(|i| {
                let t = (-150.0_f32 + 120.0 * i as f32 / 24.0).to_radians();
                Pos2::new(c.x + radius * t.cos(), c.y + radius * t.sin())
            })
            .collect();
        painter.add(egui::Shape::line(
            pts,
            Stroke::new(s * 0.075, Color32::from_white_alpha(alpha)),
        ));
    }
    painter.circle_filled(c, s * 0.11, Color32::WHITE);
}

/// Zeichnet ein Geräte-Symbol: Maus (schmale Kapsel) oder Tastatur (breites Rechteck mit Tasten).
fn draw_kind_icon(painter: &egui::Painter, rect: Rect, kind: Kind) {
    let stroke = Stroke::new(1.5, MUTED);
    match kind {
        Kind::Mouse => {
            let r = Rect::from_center_size(rect.center(), Vec2::new(rect.width() * 0.55, rect.height() * 0.85));
            painter.rect_stroke(r, CornerRadius::same(12), stroke, egui::StrokeKind::Inside);
            painter.line_segment(
                [Pos2::new(r.center().x, r.min.y + 4.0), Pos2::new(r.center().x, r.center().y)],
                stroke,
            );
        }
        Kind::Keyboard => {
            let r = Rect::from_center_size(rect.center(), Vec2::new(rect.width() * 0.95, rect.height() * 0.6));
            painter.rect_stroke(r, CornerRadius::same(5), stroke, egui::StrokeKind::Inside);
            for row in 0..2 {
                for col in 0..5 {
                    let x = r.min.x + 7.0 + col as f32 * (r.width() - 14.0) / 4.0;
                    let y = r.min.y + 7.0 + row as f32 * (r.height() - 14.0);
                    painter.circle_filled(Pos2::new(x, y), 1.8, MUTED);
                }
            }
        }
        Kind::Other => {
            painter.circle_stroke(rect.center(), rect.width() * 0.35, stroke);
        }
    }
}

/// Pille mit Statuspunkt, z. B. für die Makro-Engine.
fn status_pill(ui: &mut egui::Ui, text: &str, color: Color32) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.18))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.6)))
        .corner_radius(CornerRadius::same(255))
        .inner_margin(egui::Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (dot, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                ui.painter().circle_filled(dot.center(), 4.0, color);
                ui.label(RichText::new(text).color(color).strong());
            });
        });
}

/// Karten-Rahmen für einen Bereich.
fn card(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(CARD)
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).size(15.0).strong().color(TEXT));
            ui.add_space(8.0);
            add(ui);
        });
    ui.add_space(12.0);
}

impl App {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        let macro_path = format!("{home}/.config/periphera/macros.toml");
        let macro_text = std::fs::read_to_string(&macro_path).unwrap_or_default();
        let mut app = App {
            api: HidApi::new()?,
            devices: Vec::new(),
            selected: None,
            status: String::new(),
            engine: None,
            macro_path,
            macro_text,
            light_color: [0, 180, 255],
            light_mode: LightMode::Static,
            light_period_ms: 3000,
            light_hell: 100,
            anim: None,
            defs: Vec::new(),
            sel_macro: None,
            key_filter: String::new(),
            learn: None,
            engine_before_learn: false,
        };
        app.reload_defs();
        app.refresh();
        Ok(app)
    }

    fn reload_defs(&mut self) {
        match macros::read_defs(&self.macro_path) {
            Ok(defs) => self.defs = defs,
            Err(e) => self.status = format!("Makros laden fehlgeschlagen: {e}"),
        }
        self.sel_macro = None;
    }

    fn save_defs(&mut self) {
        match macros::write_defs(&self.macro_path, &self.defs) {
            Ok(()) => {
                self.macro_text = std::fs::read_to_string(&self.macro_path).unwrap_or_default();
                self.status = "Makros gespeichert".into();
                if self.engine.is_some() {
                    // Die Engine übernimmt Änderungen nur beim Start.
                    self.toggle_engine();
                    self.toggle_engine();
                    self.status = "Makros gespeichert, Engine neu gestartet".into();
                }
            }
            Err(e) => self.status = format!("Speichern fehlgeschlagen: {e}"),
        }
    }

    /// Sucht alle HID++-Geräte neu und liest ihre DPI-Daten.
    fn refresh(&mut self) {
        let _ = self.api.refresh_devices();
        let mut devices = Vec::new();
        let mut seen = HashSet::new();
        let infos: Vec<_> = self
            .api
            .device_list()
            .filter(|d| d.vendor_id() == VENDOR_ID && HIDPP_USAGE_PAGES.contains(&d.usage_page()))
            .cloned()
            .collect();
        for info in infos {
            if !seen.insert(info.path().to_owned()) {
                continue;
            }
            let name = info.product_string().unwrap_or("Unbekanntes Gerät").to_string();
            let Ok(Some(mut hp)) = Hidpp::open(&self.api, info.path()) else {
                continue;
            };
            let dpi = read_dpi(&mut hp).ok().flatten();
            let light = lighting::supported(&mut hp).unwrap_or(false);
            let kind = kind_of(&name);
            // Maus-Zusatztasten kommen über den Tastatur-Teil des Mausgeräts ("... Keyboard").
            let evdev = if kind == Kind::Mouse { format!("{name} Keyboard") } else { name.clone() };
            devices.push(DeviceEntry {
                kind,
                name,
                id: format!("{:04x}:{:04x}", info.vendor_id(), info.product_id()),
                path: info.path().to_owned(),
                dpi,
                light,
                evdev,
            });
        }
        self.devices = devices;
        self.selected = self.selected.filter(|&i| i < self.devices.len());
        self.status = format!("{} Gerät(e) gefunden", self.devices.len());
    }

    fn set_dpi(&mut self, idx: usize) {
        let Some(state) = self.devices[idx].dpi.as_ref() else { return };
        let target = state.values[state.index];
        let result = Hidpp::open(&self.api, &self.devices[idx].path)
            .map_err(|e| e.to_string())
            .and_then(|hp| hp.ok_or_else(|| "Gerät antwortet nicht mehr".to_string()))
            .and_then(|mut hp| dpi::set(&mut hp, u32::from(target)).map_err(|e| e.to_string()));
        match result {
            Ok(now) => {
                self.status = format!("{}: DPI auf {now} gesetzt", self.devices[idx].name);
                if let Some(s) = self.devices[idx].dpi.as_mut() {
                    s.current = now;
                }
            }
            Err(e) => self.status = format!("DPI setzen fehlgeschlagen: {e}"),
        }
    }

    /// Setzt die gewählte Farbe auf allen Geräten mit Beleuchtung (synchron).
    fn apply_light(&mut self) {
        if self.light_mode.is_animation() {
            self.start_animation();
            return;
        }
        self.stop_animation();
        let effect = match self.light_mode {
            LightMode::Static => Effect::Static { rgb: animate::scale(self.light_color, self.light_hell as f32 / 100.0) },
            LightMode::Breathing => Effect::Breathing { rgb: self.light_color, period_ms: self.light_period_ms },
            LightMode::Cycle => Effect::Cycle { period_ms: self.light_period_ms },
            _ => return,
        };
        let mut done = Vec::new();
        let mut errors = Vec::new();
        for d in self.devices.iter().filter(|d| d.light) {
            let result = Hidpp::open(&self.api, &d.path)
                .map_err(|e| e.to_string())
                .and_then(|hp| hp.ok_or_else(|| "Gerät antwortet nicht".to_string()))
                .and_then(|mut hp| lighting::set_effect(&mut hp, effect).map_err(|e| e.to_string()));
            match result {
                Ok(_) => done.push(d.name.clone()),
                Err(e) => errors.push(format!("{}: {e}", d.name)),
            }
        }
        self.status = if errors.is_empty() {
            format!("Farbe auf {} Gerät(en) gesetzt", done.len())
        } else {
            format!("Fehler: {}", errors.join("; "))
        };
    }

    fn stop_animation(&mut self) {
        if let Some(mut child) = self.anim.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        crate::animate::stop_running();
    }

    /// Startet die Software-Animation als eigenen Prozess (läuft, bis sie gestoppt wird).
    fn start_animation(&mut self) {
        self.stop_animation();
        let [r, g, b] = self.light_color;
        let color = format!("{r:02x}{g:02x}{b:02x}");
        let speed = (self.light_period_ms as f32 / 3000.0).recip().to_string();
        let mut args = vec!["light".to_string(), "anim".into(), self.light_mode.cli_name().into()];
        if matches!(self.light_mode, LightMode::Knight | LightMode::Welle | LightMode::Breathing) {
            args.push(color);
        }
        args.push(speed);
        args.push("--hell".into());
        args.push(self.light_hell.to_string());
        match std::env::current_exe().and_then(|exe| Command::new(exe).args(&args).spawn()) {
            Ok(child) => {
                self.anim = Some(child);
                self.status = "Animation läuft".into();
            }
            Err(e) => self.status = format!("Animation konnte nicht starten: {e}"),
        }
    }

    fn toggle_engine(&mut self) {
        if let Some(mut child) = self.engine.take() {
            let _ = child.kill();
            let _ = child.wait();
            self.status = "Makro-Engine gestoppt".into();
            return;
        }
        match std::env::current_exe().and_then(|exe| {
            Command::new(exe).args(["macros", "run", &self.macro_path]).spawn()
        }) {
            Ok(child) => {
                self.engine = Some(child);
                self.status = "Makro-Engine gestartet".into();
            }
            Err(e) => self.status = format!("Makro-Engine konnte nicht starten: {e}"),
        }
    }

}

fn read_dpi(hp: &mut Hidpp) -> Result<Option<DpiState>, Box<dyn std::error::Error>> {
    if !dpi::supported(hp)? {
        return Ok(None);
    }
    let current = dpi::get(hp)?;
    let values = dpi::supported_list(hp)?;
    let index = values.iter().position(|&v| v == current).unwrap_or(0);
    Ok(Some(DpiState { current, values, index }))
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_learn();
        if self.learn.is_some() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
        // Beendete Engine-Prozesse aufräumen.
        if let Some(child) = self.engine.as_mut() {
            if let Ok(Some(_)) = child.try_wait() {
                self.engine = None;
                self.status = "Makro-Engine wurde beendet".into();
            }
        }

        egui::Panel::top("top")
            .frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin::symmetric(18, 12)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (logo_rect, _) = ui.allocate_exact_size(Vec2::splat(44.0), Sense::hover());
                    draw_logo(ui.painter(), logo_rect);
                    ui.vertical(|ui| {
                        ui.label(RichText::new("periphera").size(22.0).strong().color(TEXT));
                        ui.label(RichText::new("Peripherie zentral verwalten").size(12.0).color(MUTED));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let engine_on = self.engine.is_some();
                        let engine_label = if engine_on { "Makro-Engine stoppen" } else { "Makro-Engine starten" };
                        if ui.add(egui::Button::new(engine_label).min_size(Vec2::new(0.0, 30.0))).clicked() {
                            self.toggle_engine();
                        }
                        status_pill(
                            ui,
                            if engine_on { "Makros aktiv" } else { "Makros aus" },
                            if engine_on { OK } else { OFF },
                        );
                        if ui.add(egui::Button::new("Aktualisieren").min_size(Vec2::new(0.0, 30.0))).clicked() {
                            self.refresh();
                        }
                    });
                });
            });

        egui::Panel::bottom("status")
            .frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin::symmetric(18, 8)))
            .show(ui, |ui| {
                ui.label(RichText::new(&self.status).size(12.0).color(MUTED));
            });

        egui::Panel::left("devices")
            .resizable(true)
            .default_size(300.0)
            .frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::same(14)))
            .show(ui, |ui| {
                ui.label(RichText::new("GERÄTE").size(12.0).strong().color(MUTED));
                ui.add_space(8.0);
                if self.devices.is_empty() {
                    ui.label(RichText::new("Keine kompatiblen Geräte gefunden.").color(MUTED));
                }
                for i in 0..self.devices.len() {
                    self.device_row(ui, i);
                    ui.add_space(8.0);
                }
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin::same(20)))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| match self.selected {
                    None => self.welcome(ui),
                    Some(idx) => {
                        self.device_view(ui, idx);
                        self.keys_view(ui, idx);
                    }
                });
            });
    }
}

impl App {
    fn device_row(&mut self, ui: &mut egui::Ui, i: usize) {
        let selected = self.selected == Some(i);
        let (name, kind, id) = {
            let d = &self.devices[i];
            (d.name.clone(), d.kind, d.id.clone())
        };
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 62.0), Sense::click());
        let fill = if selected {
            ACCENT.gamma_multiply(0.22)
        } else if resp.hovered() {
            CARD_HOVER
        } else {
            CARD
        };
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::same(RADIUS), fill);
        if selected {
            let bar = Rect::from_min_max(rect.min, Pos2::new(rect.min.x + 4.0, rect.max.y));
            painter.rect_filled(bar, CornerRadius::same(2), ACCENT);
        }
        let icon_rect = Rect::from_min_size(rect.min + Vec2::new(14.0, 12.0), Vec2::splat(38.0));
        painter.rect_filled(icon_rect, CornerRadius::same(10), BG);
        draw_kind_icon(&painter, icon_rect.shrink(8.0), kind);
        let text_x = rect.min.x + 64.0;
        painter.text(
            Pos2::new(text_x, rect.min.y + 12.0),
            Align2::LEFT_TOP,
            name,
            FontId::proportional(14.5),
            TEXT,
        );
        painter.text(
            Pos2::new(text_x, rect.min.y + 34.0),
            Align2::LEFT_TOP,
            format!("{} · {id}", kind.label()),
            FontId::proportional(12.0),
            MUTED,
        );
        if resp.clicked() {
            self.selected = Some(i);
        }
    }

    fn welcome(&self, ui: &mut egui::Ui) {
        ui.add_space(60.0);
        ui.vertical_centered(|ui| {
            let (logo_rect, _) = ui.allocate_exact_size(Vec2::splat(96.0), Sense::hover());
            draw_logo(ui.painter(), logo_rect);
            ui.add_space(16.0);
            ui.label(RichText::new("Willkommen bei periphera").size(22.0).strong().color(TEXT));
            ui.label(RichText::new("Wähle links ein Gerät, um DPI und Makros einzustellen.").color(MUTED));
        });
    }

    fn device_view(&mut self, ui: &mut egui::Ui, idx: usize) {
        let (name, kind, id) = {
            let d = &self.devices[idx];
            (d.name.clone(), d.kind, d.id.clone())
        };
        ui.label(RichText::new(name).size(24.0).strong().color(TEXT));
        ui.label(RichText::new(format!("{} · {id}", kind.label())).color(MUTED));
        ui.add_space(14.0);

        card(ui, "DPI", |ui| {
            let mut apply = false;
            match self.devices[idx].dpi.as_mut() {
                None => {
                    ui.label(RichText::new("Dieses Gerät hat keine DPI-Einstellung.").color(MUTED));
                }
                Some(state) => {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(state.current.to_string()).size(44.0).strong().color(ACCENT));
                        ui.label(RichText::new("DPI aktuell").color(MUTED));
                    });
                    ui.add_space(6.0);
                    let max = state.values.len().saturating_sub(1);
                    let values = state.values.clone();
                    let mut i = state.index;
                    let min_dpi = values.first().copied().unwrap_or(0);
                    let max_dpi = values.last().copied().unwrap_or(0);
                    // Grenzen oben links und rechts, darunter der Regler über die volle Breite.
                    ui.horizontal(|ui| {
                        dpi_pill(ui, min_dpi);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            dpi_pill(ui, max_dpi);
                        });
                    });
                    ui.add_space(4.0);
                    ui.spacing_mut().slider_width = (ui.available_width() - 8.0).max(120.0);
                    ui.add(egui::Slider::new(&mut i, 0..=max).show_value(false).trailing_fill(true));
                    state.index = i;
                    ui.add_space(8.0);
                    let btn = egui::Button::new(RichText::new("DPI übernehmen").strong().color(BG))
                        .fill(ACCENT)
                        .min_size(Vec2::new(180.0, 34.0));
                    if ui.add(btn).clicked() {
                        apply = true;
                    }
                }
            }
            if apply {
                self.set_dpi(idx);
            }
        });

        card(ui, "Beleuchtung", |ui| {
            if !self.devices[idx].light {
                ui.label(RichText::new("Dieses Gerät hat keine Beleuchtung.").color(MUTED));
                return;
            }
            ui.label(RichText::new("Wird auf alle beleuchteten Geräte gleichzeitig angewendet.").color(MUTED));
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(&mut self.light_mode, LightMode::Static, "Statisch");
                ui.selectable_value(&mut self.light_mode, LightMode::Breathing, "Atmen");
                ui.selectable_value(&mut self.light_mode, LightMode::Cycle, "Farbwechsel");
                ui.selectable_value(&mut self.light_mode, LightMode::Knight, "Knight Rider");
                ui.selectable_value(&mut self.light_mode, LightMode::Welle, "Welle");
                ui.selectable_value(&mut self.light_mode, LightMode::Regenbogen, "Regenbogen");
                ui.selectable_value(&mut self.light_mode, LightMode::Feuer, "Feuer");
            });
            ui.add_space(6.0);
            let before_hell = self.light_hell;
            ui.horizontal(|ui| {
                ui.label(RichText::new("Helligkeit").color(MUTED));
                let old_width = ui.spacing().slider_width;
                ui.spacing_mut().slider_width = 260.0;
                ui.add(egui::Slider::new(&mut self.light_hell, 0..=100).suffix(" %"));
                ui.spacing_mut().slider_width = old_width;
            });
            if self.light_hell != before_hell && self.anim.is_some() {
                self.start_animation();
            }
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if matches!(self.light_mode, LightMode::Static | LightMode::Breathing | LightMode::Knight | LightMode::Welle) {
                    ui.color_edit_button_srgb(&mut self.light_color);
                }
                if matches!(self.light_mode, LightMode::Breathing | LightMode::Cycle) {
                    ui.add(
                        egui::Slider::new(&mut self.light_period_ms, 500..=10000)
                            .suffix(" ms")
                            .text("Periode"),
                    );
                }
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let btn = egui::Button::new(RichText::new("Auf alle Geräte anwenden").strong().color(BG))
                    .fill(ACCENT)
                    .min_size(Vec2::new(200.0, 34.0));
                if ui.add(btn).clicked() {
                    self.apply_light();
                }
            });
        });
    }

    /// Bereich "Tasten & Makros" für das gewählte Gerät.
    fn keys_view(&mut self, ui: &mut egui::Ui, idx: usize) {
        let evdev = self.devices[idx].evdev.clone();
        let learning = self.learn.is_some();
        card(ui, "Tasten & Makros", |ui| {
            ui.horizontal(|ui| {
                if learning {
                    ui.label(RichText::new("Drücke jetzt die Taste am Gerät …").color(ACCENT).strong());
                    if ui.button("Abbrechen").clicked() {
                        if let Some((_, _, cancel)) = self.learn.as_ref() {
                            cancel.store(true, Ordering::SeqCst);
                        }
                    }
                } else {
                    let btn = egui::Button::new(RichText::new("Taste lernen").strong().color(BG))
                        .fill(ACCENT)
                        .min_size(Vec2::new(160.0, 32.0));
                    if ui.add(btn).clicked() {
                        self.start_learn(evdev.clone());
                    }
                    ui.label(RichText::new("Taste am Gerät einmal drücken, dann Befehle festlegen.").color(MUTED));
                }
            });
            ui.add_space(10.0);

            // Liste der Makros dieses Geräts als Tasten-Chips
            let mine: Vec<usize> = (0..self.defs.len()).filter(|&i| self.defs[i].device == evdev).collect();
            if mine.is_empty() {
                ui.label(RichText::new("Noch keine Taste belegt.").color(MUTED));
            }
            ui.horizontal_wrapped(|ui| {
                for i in mine.iter().copied() {
                    let d = &self.defs[i];
                    let selected = self.sel_macro == Some(i);
                    let label = format!("{}  ·  {} Befehl(e)", display_key(&d.trigger), d.steps.len());
                    let color = if selected { BG } else { TEXT };
                    let fill = if selected { ACCENT } else { CARD_HOVER };
                    let btn = egui::Button::new(RichText::new(label).color(color)).fill(fill);
                    if ui.add(btn).clicked() {
                        self.sel_macro = Some(i);
                    }
                }
            });

            if let Some(i) = self.sel_macro.filter(|&i| i < self.defs.len()) {
                ui.add_space(12.0);
                self.macro_editor(ui, i);
            }
        });
        ui.add_space(4.0);
        egui::CollapsingHeader::new(RichText::new("Erweitert: Rohdatei bearbeiten").color(MUTED))
            .show(ui, |ui| {
                ui.label(RichText::new(&self.macro_path).size(12.0).color(MUTED));
                ui.add(
                    egui::TextEdit::multiline(&mut self.macro_text)
                        .font(egui::TextStyle::Monospace)
                        .desired_rows(10)
                        .desired_width(f32::INFINITY),
                );
                if ui.button("Rohdatei speichern").clicked() {
                    match std::fs::write(&self.macro_path, &self.macro_text) {
                        Ok(()) => {
                            self.reload_defs();
                            self.status = "Rohdatei gespeichert".into();
                        }
                        Err(e) => self.status = format!("Speichern fehlgeschlagen: {e}"),
                    }
                }
            });
    }

    fn macro_editor(&mut self, ui: &mut egui::Ui, i: usize) {
        let keys = macros::key_names();
        ui.label(RichText::new(format!("Befehle für {}", display_key(&self.defs[i].trigger))).strong().color(TEXT));
        ui.add_space(4.0);
        let mut remove_step: Option<usize> = None;
        let mut swap: Option<(usize, usize)> = None;
        let count = self.defs[i].steps.len();
        for s in 0..count {
            let (mut op, mut key, mut ms) = parse_step(&self.defs[i].steps[s]);
            let before = (op, key.clone(), ms);
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{}.", s + 1)).color(MUTED));
                egui::ComboBox::from_id_salt(("op", i, s))
                    .selected_text(OPS[op])
                    .width(110.0)
                    .show_ui(ui, |ui| {
                        for (k, name) in OPS.iter().enumerate() {
                            ui.selectable_value(&mut op, k, *name);
                        }
                    });
                if op == 3 {
                    ui.add(egui::DragValue::new(&mut ms).range(0..=10_000).suffix(" ms"));
                } else {
                    egui::ComboBox::from_id_salt(("key", i, s))
                        .selected_text(display_key(&key))
                        .width(200.0)
                        .show_ui(ui, |ui| {
                            ui.set_max_height(300.0);
                            for k in keys.iter().filter(|k| self.key_filter.is_empty() || k.contains(&self.key_filter.to_uppercase())) {
                                ui.selectable_value(&mut key, k.clone(), display_key(k));
                            }
                        });
                }
                if ui.small_button("▲").clicked() && s > 0 {
                    swap = Some((s - 1, s));
                }
                if ui.small_button("▼").clicked() && s + 1 < count {
                    swap = Some((s, s + 1));
                }
                if ui.small_button("✕").clicked() {
                    remove_step = Some(s);
                }
            });
            if before != (op, key.clone(), ms) {
                self.defs[i].steps[s] = build_step(op, &key, ms);
            }
        }
        if let Some((a, b)) = swap {
            self.defs[i].steps.swap(a, b);
        }
        if let Some(s) = remove_step {
            self.defs[i].steps.remove(s);
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("+ Taste / Befehl hinzufügen").clicked() {
                self.defs[i].steps.push("tap KEY_A".into());
            }
            ui.add(egui::TextEdit::singleline(&mut self.key_filter).hint_text("Taste suchen …").desired_width(160.0));
        });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let save = egui::Button::new(RichText::new("Speichern").strong().color(BG)).fill(ACCENT);
            if ui.add(save).clicked() {
                self.save_defs();
            }
            if ui.button("Makro löschen").clicked() {
                self.defs.remove(i);
                self.sel_macro = None;
                self.save_defs();
            }
        });
    }

    fn start_learn(&mut self, evdev: String) {
        self.engine_before_learn = self.engine.is_some();
        if self.engine.is_some() {
            self.toggle_engine();
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let rx = input::learn(evdev.clone(), Arc::clone(&cancel));
        self.learn = Some((evdev, rx, cancel));
        self.status = "Warte auf Tastendruck …".into();
    }

    /// Prüft, ob eine Taste erfasst wurde, und legt dann ein neues Makro an.
    fn poll_learn(&mut self) {
        let Some((evdev, rx, _)) = self.learn.as_ref() else { return };
        let result = match rx.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("Erfassung unterbrochen".into()),
        };
        let evdev = evdev.clone();
        self.learn = None;
        match result {
            Ok(key) => {
                let already = self.defs.iter().position(|d| d.device == evdev && d.trigger == key);
                let i = match already {
                    Some(i) => i,
                    None => {
                        self.defs.push(MacroDef { device: evdev, trigger: key.clone(), steps: Vec::new() });
                        self.defs.len() - 1
                    }
                };
                self.sel_macro = Some(i);
                self.status = format!("Taste {} erfasst", display_key(&key));
            }
            Err(e) => self.status = format!("Tastenerfassung: {e}"),
        }
        if self.engine_before_learn {
            self.engine_before_learn = false;
            self.toggle_engine();
        }
    }
}

/// Kleine Beschriftung am Ende des DPI-Reglers.
fn dpi_pill(ui: &mut egui::Ui, value: u16) {
    egui::Frame::new()
        .fill(CARD_HOVER)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.label(RichText::new(format!("{value} DPI")).strong().color(TEXT));
        });
}

/// Anzeigename einer Taste: "KEY_F13" -> "F13", "BTN_SIDE" -> "BTN_SIDE" (Mausbutton).
fn display_key(name: &str) -> String {
    name.strip_prefix("KEY_").unwrap_or(name).to_string()
}

const OPS: [&str; 4] = ["Taste tippen", "Taste halten", "Taste loslassen", "Warten"];

/// Zerlegt einen Schritt "tap KEY_A" in (Operation, Taste, Millisekunden).
fn parse_step(step: &str) -> (usize, String, u32) {
    let mut parts = step.split_whitespace();
    let op = parts.next().unwrap_or("");
    let arg = parts.next().unwrap_or("");
    match op {
        "down" => (1, arg.to_string(), 0),
        "up" => (2, arg.to_string(), 0),
        "wait" => (3, String::new(), arg.parse().unwrap_or(100)),
        _ => (0, if arg.is_empty() { "KEY_A".into() } else { arg.to_string() }, 0),
    }
}

fn build_step(op: usize, key: &str, ms: u32) -> String {
    match op {
        1 => format!("down {key}"),
        2 => format!("up {key}"),
        3 => format!("wait {ms}"),
        _ => format!("tap {key}"),
    }
}

impl Drop for App {
    /// Beim Schließen des Fensters laufende Animation und Makro-Engine beenden.
    fn drop(&mut self) {
        self.stop_animation();
        if let Some(mut child) = self.engine.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let app = App::new()?;
    let icon = egui::IconData {
        rgba: WINDOW_ICON.to_vec(),
        width: 256,
        height: 256,
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1040.0, 720.0])
            .with_min_inner_size([760.0, 520.0])
            .with_title("periphera")
            .with_app_id("periphera")
            .with_icon(Arc::new(icon)),
        ..Default::default()
    };
    eframe::run_native(
        "periphera",
        options,
        Box::new(move |cc| {
            apply_theme(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )?;
    Ok(())
}
