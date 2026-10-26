//! periphera: zentrale Steuerung von Peripheriegeräten über HID++.

mod animate;
mod dpi;
mod gui;
mod hidpp;
mod lighting;
mod input;
mod macros;
mod settings;

use hidapi::HidApi;
use hidpp::{Hidpp, HIDPP_USAGE_PAGES, VENDOR_ID};
use std::collections::HashSet;
use std::process::{Command, Stdio};
use std::error::Error;

const USAGE: &str = "\
Verwendung:
  periphera gui             Oberfläche zur Geräteverwaltung öffnen
  periphera restore         Gespeicherte DPI und Beleuchtung wiederherstellen (beim Anmelden)
  periphera light set <RRGGBB>        Statische Farbe auf alle Geräte
  periphera light breathe <RRGGBB> <ms>  Atmen-Effekt (ms = Dauer eines Atemzugs)
  periphera light cycle <ms>          Farbwechsel (ms = Dauer eines Zyklus)
  periphera light stop                Laufende Animation beenden
  periphera light anim <knight|welle|regenbogen|feuer> [RRGGBB] [Tempo]  Animation (Strg+C beendet)
  periphera list            Alle HID++-Geräte anzeigen
  periphera dpi get         Aktuelle DPI lesen
  periphera dpi list        Unterstützte DPI-Werte des Geräts
  periphera dpi set <DPI>   DPI setzen (nur Werte aus der Geräteliste)
  periphera keys <Gerät>    Tasten- und Button-Ereignisse live anzeigen
  periphera macros check [Datei]  Makro-Konfiguration prüfen
  periphera macros run [Datei]    Makros starten (Standard: ~/.config/periphera/macros.toml)";

fn default_macro_path() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    format!("{home}/.config/periphera/macros.toml")
}

/// Öffnet das erste Gerät, das auf HID++ antwortet und das gewünschte Feature hat.
/// `want_dpi` filtert auf Geräte mit DPI-Feature.
fn find_device(api: &HidApi, want_dpi: bool) -> Result<Option<(String, String, Hidpp)>, Box<dyn Error>> {
    let mut seen = HashSet::new();
    for info in api.device_list().filter(|d| d.vendor_id() == VENDOR_ID) {
        if !HIDPP_USAGE_PAGES.contains(&info.usage_page()) || !seen.insert(info.path().to_owned()) {
            continue;
        }
        let Some(mut hp) = Hidpp::open(api, info.path())? else {
            continue;
        };
        if want_dpi && !dpi::supported(&mut hp)? {
            continue;
        }
        let name = info.product_string().unwrap_or("Unbekanntes Gerät").to_string();
        if let Ok(filter) = std::env::var("PERIPHERA_DEVICE") {
            if !name.contains(&filter) {
                continue;
            }
        }
        let id = format!("{:04x}:{:04x}", info.vendor_id(), info.product_id());
        return Ok(Some((name, id, hp)));
    }
    Ok(None)
}

fn list(api: &HidApi) -> Result<(), Box<dyn Error>> {
    let mut seen = HashSet::new();
    let mut count = 0;
    for info in api.device_list().filter(|d| d.vendor_id() == VENDOR_ID) {
        if !HIDPP_USAGE_PAGES.contains(&info.usage_page()) || !seen.insert(info.path().to_owned()) {
            continue;
        }
        let Some(mut hp) = Hidpp::open(api, info.path())? else {
            continue;
        };
        let name = info.product_string().unwrap_or("Unbekanntes Gerät");
        let dpi_ok = dpi::supported(&mut hp)?;
        println!(
            "{} ({:04x}:{:04x}) - DPI: {}",
            name,
            info.vendor_id(),
            info.product_id(),
            if dpi_ok { "ja" } else { "nein" }
        );
        count += 1;
    }
    if count == 0 {
        println!("Keine HID++-Geräte gefunden.");
    }
    Ok(())
}

fn run() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut api = HidApi::new()?;

    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["list"] => list(&api)?,
        ["gui"] => gui::run()?,
        ["light", "set", color, rest @ ..] => {
            let hell = brightness_arg(rest)?;
            let rgb = lighting::parse_hex(color)?;
            light_set(&api, lighting::Effect::Static { rgb: animate::scale(rgb, hell) })?;
            remember_light("statisch", rgb, 3000, (hell * 100.0).round() as u32)?
        }
        ["restore"] => restore(&mut api)?,
        ["light", "breathe", color, ms] => {
            let speed = 3000.0 / ms.parse::<f32>()?;
            light_anim(&api, "atmen", &[color, &speed.to_string()])?
        }
        ["light", "anim", name, rest @ ..] => light_anim(&api, name, rest)?,
        ["light", "stop"] => animate::stop_running(),
        ["light", "cycle", ms] => {
            let speed = 3000.0 / ms.parse::<f32>()?;
            light_anim(&api, "farbwechsel", &[&speed.to_string()])?
        }
        ["features"] => features(&api)?,
        ["keys", name] => input::watch(name)?,
        ["hidpp", "listen"] => hidpp::listen_raw(&api, 60)?,
        ["learn", name] => {
            let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let rx = input::learn(name.to_string(), cancel);
            println!("Warte auf Tastendruck an '{name}' …");
            match rx.recv()? {
                Ok(key) => println!("Erkannt: {key}"),
                Err(e) => println!("Fehler: {e}"),
            }
        }
        ["macros", "check"] => macros::check(&default_macro_path())?,
        ["macros", "check", path] => macros::check(path)?,
        ["macros", "run"] => macros::run(&default_macro_path())?,
        ["macros", "run", path] => macros::run(path)?,
        ["raw", id, func, params @ ..] => {
            let id = u16::from_str_radix(id.trim_start_matches("0x"), 16)?;
            let func: u8 = func.parse()?;
            let params = params
                .iter()
                .map(|p| u8::from_str_radix(p.trim_start_matches("0x"), 16))
                .collect::<Result<Vec<_>, _>>()?;
            raw(&api, id, func, &params)?
        }
        ["dpi", "list"] => {
            let (name, _, mut hp) = find_device(&api, true)?.ok_or("kein Gerät mit DPI-Feature gefunden")?;
            let list = dpi::supported_list(&mut hp)?;
            println!(
                "{name}: {} Werte, {}..={}",
                list.len(),
                list.first().unwrap_or(&0),
                list.last().unwrap_or(&0)
            );
        }
        ["dpi", "get"] => {
            let (name, _, mut hp) = find_device(&api, true)?.ok_or("kein Gerät mit DPI-Feature gefunden")?;
            println!("{name}: {} DPI", dpi::get(&mut hp)?);
        }
        ["dpi", "set", value] => {
            let value: u32 = value.parse().map_err(|_| "DPI muss eine Zahl sein")?;
            let (name, id, mut hp) = find_device(&api, true)?.ok_or("kein Gerät mit DPI-Feature gefunden")?;
            let now = dpi::set(&mut hp, value)?;
            remember_dpi(&id, now)?;
            if u32::from(now) == value {
                println!("{name}: DPI auf {now} gesetzt");
            } else {
                println!("{name}: Gerät meldet {now} DPI statt {value}");
            }
        }
        _ => println!("{USAGE}"),
    }
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("Fehler: {e}");
        std::process::exit(1);
    }
}

/// Debug-Befehl: schickt einen Rohbefehl und gibt die Antwort als Hex aus.
fn raw(api: &HidApi, feature_id: u16, func: u8, params: &[u8]) -> Result<(), Box<dyn Error>> {
    let (name, _, mut hp) = find_device(api, false)?.ok_or("kein HID++-Gerät gefunden")?;
    match hp.call(feature_id, func, params)? {
        Some(r) => println!("{name}: {:02x?}", &r[..]),
        None => println!("{name}: keine Antwort"),
    }
    Ok(())
}

/// Setzt die Farbe auf allen Geräten mit Beleuchtung gleichzeitig (Synchronisation).
fn light_set(api: &HidApi, effect: lighting::Effect) -> Result<(), Box<dyn Error>> {
    animate::stop_running();
    let mut seen = HashSet::new();
    let mut done = 0;
    for info in api.device_list().filter(|d| d.vendor_id() == VENDOR_ID) {
        if !HIDPP_USAGE_PAGES.contains(&info.usage_page()) || !seen.insert(info.path().to_owned()) {
            continue;
        }
        let Some(mut hp) = Hidpp::open(api, info.path())? else { continue };
        if !lighting::supported(&mut hp)? {
            continue;
        }
        let name = info.product_string().unwrap_or("Gerät").to_string();
        let zones = lighting::set_effect(&mut hp, effect)?;
        println!("{name}: {zones} Zone(n), Effekt gesetzt");
        done += 1;
    }
    if done == 0 {
        println!("Kein Gerät mit Beleuchtung gefunden.");
    }
    Ok(())
}

/// Startet eine Software-Animation auf allen beleuchteten Geräten (läuft bis Strg+C).
/// Liest optional `--hell N` (Prozent, 0-100) aus den Argumenten. Ohne Angabe 100 %.
fn brightness_arg(args: &[&str]) -> Result<f32, Box<dyn Error>> {
    match args.iter().position(|a| *a == "--hell") {
        Some(i) => {
            let pct: f32 = args.get(i + 1).ok_or("--hell braucht einen Wert (0-100)")?.parse()?;
            Ok((pct / 100.0).clamp(0.0, 1.0))
        }
        None => Ok(1.0),
    }
}

fn light_anim(api: &HidApi, name: &str, rest_all: &[&str]) -> Result<(), Box<dyn Error>> {
    let hell = brightness_arg(rest_all)?;
    let cut = rest_all.iter().position(|a| *a == "--hell").unwrap_or(rest_all.len());
    let rest = &rest_all[..cut];
    let anim = match name {
        "knight" => animate::Anim::Knight { rgb: lighting::parse_hex(rest.first().unwrap_or(&"ff0000"))? },
        "welle" => animate::Anim::Welle { rgb: lighting::parse_hex(rest.first().unwrap_or(&"00b4ff"))? },
        "regenbogen" => animate::Anim::Regenbogen,
        "feuer" => animate::Anim::Feuer,
        "atmen" => animate::Anim::Atmen { rgb: lighting::parse_hex(rest.first().unwrap_or(&"00b4ff"))? },
        "farbwechsel" => animate::Anim::Farbwechsel,
        _ => return Err("unbekannte Animation (knight, welle, regenbogen, feuer, atmen, farbwechsel)".into()),
    };
    let speed: f32 = rest.last().and_then(|s| s.parse().ok()).unwrap_or(1.0);
    let mut hps: Vec<(String, Hidpp)> = Vec::new();
    let mut seen = HashSet::new();
    let only = std::env::var("PERIPHERA_DEVICE").ok();
    for info in api.device_list().filter(|d| d.vendor_id() == VENDOR_ID) {
        if !HIDPP_USAGE_PAGES.contains(&info.usage_page()) || !seen.insert(info.path().to_owned()) {
            continue;
        }
        let name = info.product_string().unwrap_or("Gerät").to_string();
        if only.as_ref().is_some_and(|f| !name.contains(f.as_str())) {
            continue;
        }
        let Some(mut hp) = Hidpp::open(api, info.path())? else { continue };
        if lighting::supported(&mut hp)? {
            println!("{name}: Animation {anim:?}");
            hps.push((name, hp));
        }
    }
    if hps.is_empty() {
        return Err("kein Gerät mit Beleuchtung gefunden".into());
    }
    animate::run_all(hps, anim, speed, hell)
}

/// Merkt sich die DPI eines Geräts in der Einstellungsdatei.
fn remember_dpi(id: &str, value: u16) -> Result<(), Box<dyn Error>> {
    let mut s = settings::load();
    s.dpi.insert(id.to_string(), value);
    settings::save(&s)
}

/// Merkt sich die zuletzt gesetzte Beleuchtung (gilt für alle Geräte).
fn remember_light(mode: &str, color: [u8; 3], period_ms: u16, hell: u32) -> Result<(), Box<dyn Error>> {
    let mut s = settings::load();
    s.light = Some(settings::Light { mode: mode.to_string(), color, period_ms, hell });
    settings::save(&s)
}

/// Startargumente für die Animation, wie sie die Oberfläche selbst verwendet.
fn light_anim_args(light: &settings::Light) -> Vec<String> {
    let [r, g, b] = light.color;
    let mut args = vec!["light".to_string(), "anim".into(), light.mode.clone()];
    if matches!(light.mode.as_str(), "knight" | "welle" | "atmen") {
        args.push(format!("{r:02x}{g:02x}{b:02x}"));
    }
    args.push((light.period_ms as f32 / 3000.0).recip().to_string());
    args.push("--hell".into());
    args.push(light.hell.to_string());
    args
}

/// Stellt gespeicherte DPI und Beleuchtung wieder her. Beim Anmelden sind die Geräte oft
/// noch nicht bereit, daher wird bis zu zehnmal im Abstand von drei Sekunden versucht.
fn restore(api: &mut HidApi) -> Result<(), Box<dyn Error>> {
    let s = settings::load();
    if s.dpi.is_empty() && s.light.is_none() {
        println!("Keine gespeicherten Einstellungen.");
        return Ok(());
    }
    let mut dpi_done: HashSet<String> = HashSet::new();
    let mut light_ids: HashSet<String> = HashSet::new();
    let mut anim_started = false;
    for attempt in 1..=10 {
        api.refresh_devices()?;
        let infos: Vec<_> = api
            .device_list()
            .filter(|d| d.vendor_id() == VENDOR_ID && HIDPP_USAGE_PAGES.contains(&d.usage_page()))
            .cloned()
            .collect();
        let mut seen = HashSet::new();
        for info in infos {
            if !seen.insert(info.path().to_owned()) {
                continue;
            }
            let Some(mut hp) = Hidpp::open(api, info.path())? else { continue };
            let id = format!("{:04x}:{:04x}", info.vendor_id(), info.product_id());
            let name = info.product_string().unwrap_or("Gerät").to_string();
            if let Some(&value) = s.dpi.get(&id) {
                if !dpi_done.contains(&id) && dpi::supported(&mut hp)? {
                    match dpi::set(&mut hp, u32::from(value)) {
                        Ok(now) => {
                            println!("{name}: DPI auf {now} wiederhergestellt");
                            dpi_done.insert(id.clone());
                        }
                        Err(e) => eprintln!("{name}: DPI {value} fehlgeschlagen: {e}"),
                    }
                }
            }
            if let Some(light) = &s.light {
                if !light_ids.contains(&id) && !anim_started && lighting::supported(&mut hp)? {
                    let is_static = light.mode == "statisch";
                    let is_cycle_full = light.mode == "farbwechsel" && light.hell >= 100;
                    if is_static || is_cycle_full {
                        let effect = if is_static {
                            lighting::Effect::Static { rgb: animate::scale(light.color, light.hell as f32 / 100.0) }
                        } else {
                            lighting::Effect::Cycle { period_ms: light.period_ms }
                        };
                        match lighting::set_effect(&mut hp, effect) {
                            Ok(_) => {
                                println!("{name}: Beleuchtung \"{}\" wiederhergestellt", light.mode);
                                light_ids.insert(id.clone());
                            }
                            Err(e) => eprintln!("{name}: Beleuchtung fehlgeschlagen: {e}"),
                        }
                    } else {
                        // Animationen laufen als eigener Prozess über alle beleuchteten Geräte.
                        let child = Command::new(std::env::current_exe()?)
                            .args(light_anim_args(light))
                            .stdin(Stdio::null())
                            .stdout(Stdio::null())
                            .stderr(Stdio::null())
                            .spawn()?;
                        println!("Animation \"{}\" gestartet (PID {})", light.mode, child.id());
                        anim_started = true;
                        light_ids.insert(id.clone());
                    }
                }
            }
        }
        let dpi_missing = s.dpi.keys().any(|k| !dpi_done.contains(k));
        let light_missing = s.light.is_some() && light_ids.is_empty();
        if !dpi_missing && !light_missing {
            return Ok(());
        }
        if attempt < 10 {
            std::thread::sleep(std::time::Duration::from_secs(3));
        }
    }
    println!("Nicht alle Einstellungen konnten wiederhergestellt werden (Geräte nicht erreichbar).");
    Ok(())
}

fn features(api: &HidApi) -> Result<(), Box<dyn Error>> {
    let (name, _, mut hp) = find_device(api, false)?.ok_or("kein HID++-Gerät gefunden")?;
    println!("{name}:");
    for (id, idx) in hp.all_features()? {
        println!("  {id:#06x}  Index {idx:#04x}");
    }
    Ok(())
}
