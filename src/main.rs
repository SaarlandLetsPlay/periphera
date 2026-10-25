//! periphera: zentrale Steuerung von Peripheriegeräten über HID++.

mod animate;
mod dpi;
mod gui;
mod hidpp;
mod lighting;
mod input;
mod macros;

use hidapi::HidApi;
use hidpp::{Hidpp, HIDPP_USAGE_PAGES, VENDOR_ID};
use std::collections::HashSet;
use std::error::Error;

const USAGE: &str = "\
Verwendung:
  periphera gui             Oberfläche zur Geräteverwaltung öffnen
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
fn find_device(api: &HidApi, want_dpi: bool) -> Result<Option<(String, Hidpp)>, Box<dyn Error>> {
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
        return Ok(Some((name, hp)));
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
    let api = HidApi::new()?;

    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["list"] => list(&api)?,
        ["gui"] => gui::run()?,
        ["light", "set", color, rest @ ..] => {
            let hell = brightness_arg(rest)?;
            light_set(&api, lighting::Effect::Static { rgb: animate::scale(lighting::parse_hex(color)?, hell) })?
        }
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
            let (name, mut hp) = find_device(&api, true)?.ok_or("kein Gerät mit DPI-Feature gefunden")?;
            let list = dpi::supported_list(&mut hp)?;
            println!(
                "{name}: {} Werte, {}..={}",
                list.len(),
                list.first().unwrap_or(&0),
                list.last().unwrap_or(&0)
            );
        }
        ["dpi", "get"] => {
            let (name, mut hp) = find_device(&api, true)?.ok_or("kein Gerät mit DPI-Feature gefunden")?;
            println!("{name}: {} DPI", dpi::get(&mut hp)?);
        }
        ["dpi", "set", value] => {
            let value: u32 = value.parse().map_err(|_| "DPI muss eine Zahl sein")?;
            let (name, mut hp) = find_device(&api, true)?.ok_or("kein Gerät mit DPI-Feature gefunden")?;
            let now = dpi::set(&mut hp, value)?;
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
    let (name, mut hp) = find_device(api, false)?.ok_or("kein HID++-Gerät gefunden")?;
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
    for info in api.device_list().filter(|d| d.vendor_id() == VENDOR_ID) {
        if !HIDPP_USAGE_PAGES.contains(&info.usage_page()) || !seen.insert(info.path().to_owned()) {
            continue;
        }
        let Some(mut hp) = Hidpp::open(api, info.path())? else { continue };
        if lighting::supported(&mut hp)? {
            let name = info.product_string().unwrap_or("Gerät").to_string();
            println!("{name}: Animation {anim:?}");
            hps.push((name, hp));
        }
    }
    if hps.is_empty() {
        return Err("kein Gerät mit Beleuchtung gefunden".into());
    }
    animate::run_all(hps, anim, speed, hell)
}

fn features(api: &HidApi) -> Result<(), Box<dyn Error>> {
    let (name, mut hp) = find_device(api, false)?.ok_or("kein HID++-Gerät gefunden")?;
    println!("{name}:");
    for (id, idx) in hp.all_features()? {
        println!("  {id:#06x}  Index {idx:#04x}");
    }
    Ok(())
}
