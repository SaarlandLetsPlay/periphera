//! Softwareseitige Animationen: pro Bild wird jeder Zone eine Farbe zugewiesen.
//! Alle Geräte laufen mit einer gemeinsamen Zeitbasis und bleiben so synchron.

use crate::hidpp::Hidpp;
use crate::lighting::{self, Effect};
use std::error::Error;
use std::time::{Duration, Instant};

/// Eine Animation, wählbar über den Namen auf der Kommandozeile.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Anim {
    /// Leuchtender Punkt läuft hin und her (Knight Rider).
    Knight { rgb: [u8; 3] },
    /// Helligkeitswelle läuft über alle Zonen.
    Welle { rgb: [u8; 3] },
    /// Regenbogen fließt über die Zonen.
    Regenbogen,
    /// Flackerndes Feuer in Orange-Gelb.
    Feuer,
    /// Pulsieren in einer Farbe (Software, gleich auf allen Geräten).
    Atmen { rgb: [u8; 3] },
    /// Alle Zonen durchlaufen gemeinsam den Farbkreis.
    Farbwechsel,
}

impl Anim {
    /// Liefert die Farbe einer Zone zum Zeitpunkt `t` (Sekunden).
    pub fn color(self, t: f32, zone: usize, zones: usize, speed: f32) -> [u8; 3] {
        let x = if zones <= 1 { 0.0 } else { zone as f32 / (zones - 1) as f32 };
        match self {
            Anim::Knight { rgb } => {
                // Eine Hin- und Rückbewegung dauert 2 / speed Sekunden.
                let phase = (t * speed / 2.0).fract();
                let tri = if phase < 0.5 { phase * 2.0 } else { 2.0 - phase * 2.0 };
                let pos = tri * (zones.saturating_sub(1)) as f32;
                let dist = (zone as f32 - pos).abs();
                let b = (1.0 - dist / 1.5).clamp(0.0, 1.0);
                scale(rgb, b)
            }
            Anim::Welle { rgb } => {
                let b = 0.5 + 0.5 * (std::f32::consts::TAU * (t * speed * 0.5 - x)).sin();
                scale(rgb, b)
            }
            Anim::Regenbogen => {
                let hue = (t * speed * 0.15 + x).fract();
                hsv(hue)
            }
            Anim::Atmen { rgb } => {
                // Ein Atemzug dauert 3 / speed Sekunden.
                let b = 0.5 - 0.5 * (std::f32::consts::TAU * t * speed / 3.0).cos();
                scale(rgb, b)
            }
            Anim::Farbwechsel => hsv((t * speed / 3.0).fract()),
            Anim::Feuer => {
                let b = 0.35 + 0.65 * noise(zone as u32, (t * 9.0 * speed) as u32);
                scale([255, 90, 0], b)
            }
        }
    }
}

pub fn scale(rgb: [u8; 3], b: f32) -> [u8; 3] {
    [
        (rgb[0] as f32 * b) as u8,
        (rgb[1] as f32 * b) as u8,
        (rgb[2] as f32 * b) as u8,
    ]
}

/// Vollgesättigte Farbe für einen Farbkreis-Wert 0..1.
fn hsv(h: f32) -> [u8; 3] {
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let q = 1.0 - f;
    let (r, g, b) = match (i as i32).rem_euclid(6) {
        0 => (1.0, f, 0.0),
        1 => (q, 1.0, 0.0),
        2 => (0.0, 1.0, f),
        3 => (0.0, q, 1.0),
        4 => (f, 0.0, 1.0),
        _ => (1.0, 0.0, q),
    };
    [(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8]
}

/// Einfache Hash-Funktion für flackernde Zufallswerte in 0..1.
fn noise(a: u32, b: u32) -> f32 {
    let mut h = a.wrapping_mul(374_761_393).wrapping_add(b.wrapping_mul(668_265_263));
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    ((h ^ (h >> 16)) & 0xFFFF) as f32 / 65535.0
}

/// PID-Datei: sorgt dafür, dass immer nur eine Animation läuft.
fn pid_file() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    std::path::Path::new(&home).join(".local/state/periphera/anim.pid")
}

/// Beendet eine eventuell laufende Animation (über die PID-Datei).
pub fn stop_running() {
    let path = pid_file();
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(pid) = text.trim().parse::<u32>() {
            let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
            if comm.trim() == "periphera" && pid != std::process::id() {
                let _ = std::process::Command::new("kill").arg(pid.to_string()).status();
                std::thread::sleep(Duration::from_millis(150));
            }
        }
    }
    let _ = std::fs::remove_file(&path);
}

/// Merkt sich diesen Prozess als laufende Animation.
fn claim() -> Result<(), Box<dyn Error>> {
    stop_running();
    let path = pid_file();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, std::process::id().to_string())?;
    Ok(())
}

/// Läuft auf einem Gerät, bis der Prozess beendet wird. Fehler einzelner Zonen werden
/// übersprungen, damit die Animation nicht stehen bleibt.
fn run_device(mut hp: Hidpp, name: String, anim: Anim, speed: f32, brightness: f32, start: Instant) {
    let zones = loop {
        match lighting::zone_count(&mut hp) {
            Ok(z) if z > 0 => break z as usize,
            _ => std::thread::sleep(Duration::from_millis(500)),
        }
    };
    let frame = Duration::from_millis(40);
    let debug = std::env::var("PERIPHERA_DEBUG").is_ok();
    let mut next = Instant::now();
    let mut errors = 0u32;
    let mut frames = 0u32;
    let mut frame_time = Duration::ZERO;
    loop {
        let frame_start = Instant::now();
        let t = start.elapsed().as_secs_f32();
        for zone in 0..zones {
            let rgb = scale(anim.color(t, zone, zones, speed), brightness);
            if let Err(e) = lighting::set_zone(&mut hp, zone as u8, Effect::Static { rgb }) {
                errors += 1;
                if errors == 1 || errors % 200 == 0 {
                    eprintln!("{name}: Zone {zone}: {e} ({errors} Fehler bisher)");
                }
            }
        }
        frame_time += frame_start.elapsed();
        frames += 1;
        if debug && frames % 100 == 0 {
            eprintln!("{name}: {:.1} ms pro Bild", frame_time.as_secs_f32() * 1000.0 / 100.0);
            frame_time = Duration::ZERO;
        }
        // Fester Bildtakt: Restzeit schlafen, bei Verzug nicht aufholen.
        next += frame;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            next = now;
        }
    }
}

/// Startet die Animation auf allen beleuchteten Geräten gleichzeitig.
/// Die Geräte teilen sich eine Zeitbasis, dadurch bleiben sie synchron.
pub fn run_all(hps: Vec<(String, Hidpp)>, anim: Anim, speed: f32, brightness: f32) -> Result<(), Box<dyn Error>> {
    claim()?;
    let start = Instant::now();
    let handles: Vec<_> = hps
        .into_iter()
        .map(|(name, hp)| std::thread::spawn(move || run_device(hp, name, anim, speed, brightness, start)))
        .collect();
    for h in handles {
        let _ = h.join();
    }
    Ok(())
}
