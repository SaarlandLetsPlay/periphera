//! Minimale HID++ 2.0 Schicht: Ping, Feature-Discovery und Kurzbefehle.

use hidapi::{HidApi, HidDevice};
use std::collections::HashMap;
use std::error::Error;
use std::ffi::CStr;

/// USB-Vendor-ID der unterstützten Geräte (HID++).
pub const VENDOR_ID: u16 = 0x046D;
/// Vendor-Usage-Pages, auf denen HID++ läuft (Maus: 0xFF00, G213: 0xFF43).
pub const HIDPP_USAGE_PAGES: [u16; 2] = [0xFF00, 0xFF43];

const REPORT_SHORT: u8 = 0x10;
const REPORT_LONG: u8 = 0x11;
const REPORT_SHORT_LEN: usize = 7;
const REPORT_LONG_LEN: usize = 20;
const SW_ID: u8 = 0x01;
const IROOT_INDEX: u8 = 0x00;

pub const FEATURE_ADJUSTABLE_DPI: u16 = 0x2201;

pub struct Hidpp {
    dev: HidDevice,
    /// Geräteindex: 0xFF = direkt verbunden, 1..=6 = Empfänger-Slot.
    dev_idx: u8,
    features: HashMap<u16, u8>,
}

impl Hidpp {
    /// Öffnet eine HID++-Schnittstelle und sucht das erste Gerät, das antwortet.
    /// Gibt `Ok(None)` zurück, wenn auf dieser Schnittstelle kein HID++-Gerät antwortet.
    pub fn open(api: &HidApi, path: &CStr) -> Result<Option<Self>, Box<dyn Error>> {
        let dev = api.open_path(path)?;
        for dev_idx in std::iter::once(0xFF).chain(1..=6u8) {
            if ping(&dev, dev_idx)?.is_some() {
                return Ok(Some(Hidpp { dev, dev_idx, features: HashMap::new() }));
            }
        }
        Ok(None)
    }

    /// Liefert den Feature-Index und merkt ihn sich. `None` heißt: Gerät kennt das Feature nicht.
    pub fn feature_index(&mut self, feature_id: u16) -> Result<Option<u8>, Box<dyn Error>> {
        if let Some(&idx) = self.features.get(&feature_id) {
            return Ok(Some(idx));
        }
        let r = self.request(IROOT_INDEX, 0, &feature_id.to_be_bytes())?;
        match r {
            Some(report) if report[4] != 0 => {
                self.features.insert(feature_id, report[4]);
                Ok(Some(report[4]))
            }
            _ => Ok(None),
        }
    }

    /// Sendet einen Kurzbefehl an ein Feature (Feature-Index und Funktion) und gibt die Antwort zurück.
    pub fn call(
        &mut self,
        feature_id: u16,
        func: u8,
        params: &[u8],
    ) -> Result<Option<[u8; 20]>, Box<dyn Error>> {
        let Some(idx) = self.feature_index(feature_id)? else {
            return Ok(None);
        };
        self.request(idx, func, params)
    }

    /// Sendet einen Befehl ohne auf die Antwort zu warten (für Animationen mit vielen Schreibvorgängen).
    /// Vorhandene Antworten werden verworfen, damit der Eingabepuffer nicht überläuft.
    pub fn send(&mut self, feature_id: u16, func: u8, params: &[u8]) -> Result<(), Box<dyn Error>> {
        let Some(idx) = self.feature_index(feature_id)? else {
            return Err("Feature nicht vorhanden".into());
        };
        self.dev.write(&build_report(self.dev_idx, idx, func, params))?;
        let mut buf = [0u8; 20];
        while self.dev.read_timeout(&mut buf, 0)? > 0 {}
        Ok(())
    }

    fn request(&self, feature_idx: u8, func: u8, params: &[u8]) -> Result<Option<[u8; 20]>, Box<dyn Error>> {
        let report = build_report(self.dev_idx, feature_idx, func, params);
        self.dev.write(&report)?;
        read_matching(&self.dev, self.dev_idx, feature_idx, func)
    }
}

/// Baut einen HID++-Befehl: Kurzbefehl (7 Byte) bis 3 Parameter, sonst Langbefehl (20 Byte).
fn build_report(dev_idx: u8, feature_idx: u8, func: u8, params: &[u8]) -> Vec<u8> {
    let long = params.len() > 3;
    let mut out = vec![0u8; if long { REPORT_LONG_LEN } else { REPORT_SHORT_LEN }];
    out[0] = if long { REPORT_LONG } else { REPORT_SHORT };
    out[1] = dev_idx;
    out[2] = feature_idx;
    out[3] = (func << 4) | SW_ID;
    out[4..4 + params.len()].copy_from_slice(params);
    out
}

/// Wartet auf die Antwort zu genau diesem Befehl. Notifications anderer Features werden übersprungen.
fn read_matching(
    dev: &HidDevice,
    dev_idx: u8,
    feature_idx: u8,
    func: u8,
) -> Result<Option<[u8; 20]>, Box<dyn Error>> {
    let mut buf = [0u8; 20];
    for _ in 0..16 {
        let n = dev.read_timeout(&mut buf, 300)?;
        if n == 0 {
            return Ok(None);
        }
        if buf[1] != dev_idx {
            continue;
        }
        // HID++ 2.0 Fehler: Feature-Index 0xFF, danach der ursprüngliche Feature-Index.
        if buf[2] == 0xFF && buf[3] == feature_idx {
            return Err(format!("HID++ Fehlercode {:#04x}", buf[5]).into());
        }
        if buf[2] == feature_idx && (buf[3] >> 4) == func {
            return Ok(Some(buf));
        }
    }
    Ok(None)
}

/// IRoot.ping: prüft, ob unter diesem Geräteindex ein HID++-Gerät antwortet.
fn ping(dev: &HidDevice, dev_idx: u8) -> Result<Option<[u8; 20]>, Box<dyn Error>> {
    dev.write(&build_report(dev_idx, IROOT_INDEX, 1, &[]))?;
    // Fehler beim Ping (z. B. unbekannter Slot) bedeuten einfach: kein Gerät hier.
    Ok(read_matching(dev, dev_idx, IROOT_INDEX, 1).unwrap_or(None))
}

impl Hidpp {
    /// Liest alle Features des Geräts über IFeatureSet (0x0001): (Feature-ID, Feature-Index).
    pub fn all_features(&mut self) -> Result<Vec<(u16, u8)>, Box<dyn Error>> {
        let count = self
            .request_by_id(0x0001, 0, &[])?
            .ok_or("keine Antwort auf IFeatureSet.getCount")?[4];
        let mut out = Vec::new();
        for i in 0..count {
            let r = self
                .request_by_id(0x0001, 1, &[i])?
                .ok_or("keine Antwort auf IFeatureSet.getFeatureID")?;
            out.push((u16::from_be_bytes([r[4], r[5]]), i));
        }
        Ok(out)
    }

    /// Befehl per Feature-ID statt per Feature-Index. Der Index wird über IRoot aufgelöst.
    pub fn request_by_id(
        &mut self,
        feature_id: u16,
        func: u8,
        params: &[u8],
    ) -> Result<Option<[u8; 20]>, Box<dyn Error>> {
        self.call(feature_id, func, params)
    }
}

/// Nur-Lese-Probe: liest alle HID++-Meldungen der Logitech-Geräte für `seconds` Sekunden mit
/// und gibt sie roh aus. Schreibt nichts an die Geräte.
pub fn listen_raw(api: &HidApi, seconds: u64) -> Result<(), Box<dyn Error>> {
    let paths: Vec<_> = api
        .device_list()
        .filter(|d| d.vendor_id() == VENDOR_ID && HIDPP_USAGE_PAGES.contains(&d.usage_page()))
        .map(|d| (d.product_string().unwrap_or("?").to_string(), d.path().to_owned()))
        .collect();
    let mut devs = Vec::new();
    for (name, path) in paths {
        match api.open_path(&path) {
            Ok(dev) => {
                dev.set_blocking_mode(false)?;
                println!("Lese: {name}");
                devs.push((name, dev));
            }
            Err(e) => println!("Konnte {name} nicht öffnen: {e}"),
        }
    }
    println!("Jetzt die Extratasten drücken ({seconds} s) …");
    let start = std::time::Instant::now();
    let end = start + std::time::Duration::from_secs(seconds);
    let mut buf = [0u8; 64];
    while std::time::Instant::now() < end {
        for (name, dev) in &devs {
            let n = dev.read(&mut buf)?;
            if n > 0 {
                let hex: Vec<String> = buf[..n].iter().map(|b| format!("{b:02x}")).collect();
                println!("{:>7} ms  {name}: {}", start.elapsed().as_millis(), hex.join(" "));
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    println!("Ende.");
    Ok(())
}
