//! Gespeicherte Geräteeinstellungen (DPI, Beleuchtung). Der Sensor und die LEDs behalten
//! ihre Werte nach dem Abschalten nicht, deshalb stellt `periphera restore` sie beim Anmelden wieder her.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::error::Error;

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Settings {
    /// DPI je Gerät, Schlüssel "vid:pid" (z. B. "046d:c08b").
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dpi: BTreeMap<String, u16>,
    /// Zuletzt gesetzte Beleuchtung, gilt für alle Geräte gemeinsam.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub light: Option<Light>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Light {
    /// Effektname wie in der CLI: "statisch", "atmen", "farbwechsel", "knight", "welle", "regenbogen", "feuer".
    pub mode: String,
    pub color: [u8; 3],
    pub period_ms: u16,
    /// Helligkeit in Prozent.
    pub hell: u32,
}

pub fn path() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    format!("{home}/.config/periphera/settings.toml")
}

/// Liest die Einstellungen. Fehlt die Datei oder ist sie kaputt, gibt es Standardwerte zurück.
pub fn load() -> Settings {
    let p = path();
    match std::fs::read_to_string(&p) {
        Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
            eprintln!("Warnung: {p} kann nicht gelesen werden: {e}");
            Settings::default()
        }),
        Err(_) => Settings::default(),
    }
}

/// Schreibt die Einstellungen atomar (erst temporäre Datei, dann umbenennen).
pub fn save(s: &Settings) -> Result<(), Box<dyn Error>> {
    let p = path();
    if let Some(dir) = std::path::Path::new(&p).parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = format!("{p}.new");
    std::fs::write(&tmp, toml::to_string_pretty(s)?)?;
    std::fs::rename(&tmp, &p)?;
    Ok(())
}
