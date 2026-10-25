//! Beleuchtung über das Feature ColorLEDEffects (0x8070).
//! Statische Farbe pro Zone; alle Zonen eines Geräts werden gemeinsam gesetzt.

use crate::hidpp::Hidpp;
use std::error::Error;

pub const FEATURE_COLOR_LED: u16 = 0x8070;

const FUNC_GET_INFO: u8 = 0;
const FUNC_SET_ZONE_EFFECT: u8 = 3;
const EFFECT_STATIC: u8 = 0x01;
const EFFECT_BREATHING: u8 = 0x02;
const EFFECT_CYCLE: u8 = 0x03;

/// Beleuchtungseffekt, gilt für alle Zonen eines Geräts.
#[derive(Clone, Copy, PartialEq)]
pub enum Effect {
    Static { rgb: [u8; 3] },
    /// Pulsieren in einer Farbe; `period_ms` ist die Dauer eines Atemzugs.
    Breathing { rgb: [u8; 3], period_ms: u16 },
    /// Durchläuft alle Farben; `period_ms` ist die Dauer eines vollen Zyklus.
    Cycle { period_ms: u16 },
}

/// Prüft, ob das Gerät Farb-LEDs hat.
pub fn supported(hp: &mut Hidpp) -> Result<bool, Box<dyn Error>> {
    Ok(hp.feature_index(FEATURE_COLOR_LED)?.is_some())
}

/// Anzahl der Beleuchtungszonen des Geräts.
pub fn zone_count(hp: &mut Hidpp) -> Result<u8, Box<dyn Error>> {
    let r = hp
        .call(FEATURE_COLOR_LED, FUNC_GET_INFO, &[])?
        .ok_or("keine Antwort auf ColorLEDEffects.getInfo")?;
    Ok(r[4])
}

/// Setzt alle Zonen des Geräts auf einen Effekt (flüchtig, nicht im Gerät gespeichert).
pub fn set_effect(hp: &mut Hidpp, effect: Effect) -> Result<u8, Box<dyn Error>> {
    let zones = zone_count(hp)?;
    for zone in 0..zones {
        let params = match effect {
            Effect::Static { rgb } => vec![zone, EFFECT_STATIC, rgb[0], rgb[1], rgb[2], 0, 0],
            Effect::Breathing { rgb, period_ms } => {
                let [ph, pl] = period_ms.to_be_bytes();
                vec![zone, EFFECT_BREATHING, rgb[0], rgb[1], rgb[2], ph, pl, 0, 0]
            }
            Effect::Cycle { period_ms } => {
                let [ph, pl] = period_ms.to_be_bytes();
                vec![zone, EFFECT_CYCLE, ph, pl, 0, 0, 0]
            }
        };
        hp.call(FEATURE_COLOR_LED, FUNC_SET_ZONE_EFFECT, &params)?
            .ok_or_else(|| format!("keine Antwort auf Zone {zone}"))?;
    }
    Ok(zones)
}


/// Setzt eine einzelne Zone auf einen Effekt (für Animationen).
pub fn set_zone(hp: &mut Hidpp, zone: u8, effect: Effect) -> Result<(), Box<dyn Error>> {
    let params = match effect {
        Effect::Static { rgb } => vec![zone, EFFECT_STATIC, rgb[0], rgb[1], rgb[2], 0, 0],
        _ => return Err("nur statische Farben pro Zone".into()),
    };
    hp.send(FEATURE_COLOR_LED, FUNC_SET_ZONE_EFFECT, &params)
}

/// Parst eine Farbe im Format RRGGBB oder #RRGGBB.
pub fn parse_hex(s: &str) -> Result<[u8; 3], Box<dyn Error>> {
    let h = s.trim_start_matches('#');
    if h.len() != 6 {
        return Err(format!("Farbe '{s}' muss 6 Hex-Ziffern haben (z. B. ff8800)").into());
    }
    let v = u32::from_str_radix(h, 16).map_err(|_| format!("'{s}' ist keine gültige Hex-Farbe"))?;
    Ok([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}
