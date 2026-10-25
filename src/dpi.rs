//! DPI-Steuerung über das Feature AdjustableDPI (0x2201).

use crate::hidpp::{Hidpp, FEATURE_ADJUSTABLE_DPI};
use std::error::Error;

const FUNC_GET_SENSOR_DPI: u8 = 2;
const FUNC_SET_SENSOR_DPI: u8 = 3;
const SENSOR: u8 = 0;

/// Prüft, ob das Gerät das DPI-Feature hat.
pub fn supported(hp: &mut Hidpp) -> Result<bool, Box<dyn Error>> {
    Ok(hp.feature_index(FEATURE_ADJUSTABLE_DPI)?.is_some())
}

/// Liest die aktuelle DPI des Sensors.
pub fn get(hp: &mut Hidpp) -> Result<u16, Box<dyn Error>> {
    let r = hp
        .call(FEATURE_ADJUSTABLE_DPI, FUNC_GET_SENSOR_DPI, &[SENSOR])?
        .ok_or("keine Antwort auf getSensorDpi")?;
    Ok(u16::from_be_bytes([r[5], r[6]]))
}

/// Setzt die DPI und liest sie zur Kontrolle zurück.
pub fn set(hp: &mut Hidpp, dpi: u32) -> Result<u16, Box<dyn Error>> {
    let list = supported_list(hp)?;
    let valid = u16::try_from(dpi).ok().filter(|v| list.contains(v));
    let Some(dpi) = valid else {
        let min = list.first().copied().unwrap_or(0);
        let max = list.last().copied().unwrap_or(0);
        return Err(format!("DPI {dpi} wird nicht unterstützt (Gerät: {min}..={max}, Schritte laut Liste)").into());
    };
    let [hi, lo] = dpi.to_be_bytes();
    hp.call(FEATURE_ADJUSTABLE_DPI, FUNC_SET_SENSOR_DPI, &[SENSOR, hi, lo])?
        .ok_or("keine Antwort auf setSensorDpi")?;
    get(hp)
}

const FUNC_GET_SENSOR_DPI_LIST: u8 = 1;

/// Liest alle vom Sensor unterstützten DPI-Werte, aufgelöst zu einzelnen Werten.
/// Die Geräteliste kennt Einzelwerte und Bereiche: ein Eintrag mit gesetzten Bits 0xE000
/// gibt die Schrittweite (Low-Bits) an, der folgende Eintrag ist das Ende des Bereichs.
pub fn supported_list(hp: &mut Hidpp) -> Result<Vec<u16>, Box<dyn Error>> {
    let r = hp
        .call(FEATURE_ADJUSTABLE_DPI, FUNC_GET_SENSOR_DPI_LIST, &[SENSOR])?
        .ok_or("keine Antwort auf getSensorDpiList")?;

    let mut values: Vec<u16> = Vec::new();
    let mut step: Option<u16> = None;
    // Byte 4 = Sensor, danach Paare aus je zwei Byte, Ende bei 0.
    for pair in r[5..].chunks_exact(2) {
        let v = u16::from_be_bytes([pair[0], pair[1]]);
        if v == 0 {
            break;
        }
        if v & 0xE000 == 0xE000 {
            step = Some(v & 0x1FFF);
            continue;
        }
        match (step.take(), values.last().copied()) {
            (Some(s), Some(from)) => {
                values.extend((from + s..=v).step_by(usize::from(s)));
            }
            _ => values.push(v),
        }
    }
    Ok(values)
}
