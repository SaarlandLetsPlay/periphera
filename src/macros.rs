//! Makro-Engine: fängt Auslösetasten per evdev ab, leitet alle anderen Ereignisse
//! über ein virtuelles Eingabegerät weiter und führt bei Auslösung eine Schrittfolge aus.

use crate::input;
use evdev::{AttributeSet, EventType, InputEvent, KeyCode, uinput::VirtualDevice};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Serialize, Deserialize)]
struct Config {
    #[serde(rename = "macro", default)]
    macros: Vec<MacroDef>,
}

/// Eine Makro-Definition aus der TOML-Datei.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct MacroDef {
    /// Teil des evdev-Gerätenamens, z. B. "G502 HERO Gaming Mouse Keyboard".
    pub device: String,
    /// Auslösende Taste oder Maustaste, z. B. "BTN_SIDE" oder "KEY_F13".
    pub trigger: String,
    /// Schritte: "tap KEY_C", "down KEY_LEFTCTRL", "up KEY_LEFTCTRL", "wait 100" (ms).
    pub steps: Vec<String>,
}

/// Liest alle Makro-Definitionen aus der Datei (leere Liste, wenn sie fehlt).
pub fn read_defs(path: &str) -> Result<Vec<MacroDef>, Box<dyn Error>> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let cfg: Config = toml::from_str(&text).map_err(|e| format!("{path}: {e}"))?;
            Ok(cfg.macros)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("{path}: {e}").into()),
    }
}

/// Schreibt alle Makro-Definitionen in die Datei.
pub fn write_defs(path: &str, defs: &[MacroDef]) -> Result<(), Box<dyn Error>> {
    let cfg = Config { macros: defs.to_vec() };
    let text = toml::to_string_pretty(&cfg)?;
    if let Some(dir) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, text)?;
    Ok(())
}

/// Alle Tasten- und Button-Namen, die sich für Makros nutzen lassen (sortiert).
pub fn key_names() -> Vec<String> {
    let mut v: Vec<String> = key_table().into_keys().collect();
    v.sort();
    v
}

enum Step {
    Tap(u16),
    Down(u16),
    Up(u16),
    Wait(u64),
}

/// Ein gebundenes, geprüftes Makro.
struct Bound {
    trigger: u16,
    steps: Vec<Step>,
    busy: AtomicBool,
}

/// Alle Tasten- und Button-Codes, die das virtuelle Gerät erhält.
fn key_table() -> HashMap<String, u16> {
    (0..0x300u16)
        .filter_map(|c| {
            let name = input::key_name(c);
            (name.starts_with("KEY_") || name.starts_with("BTN_")).then_some((name, c))
        })
        .collect()
}

fn parse_step(line: &str, keys: &HashMap<String, u16>) -> Result<Step, String> {
    let mut parts = line.split_whitespace();
    let op = parts.next().ok_or("leerer Schritt")?;
    let arg = parts.next().ok_or_else(|| format!("'{line}': Argument fehlt"))?;
    let code = |name: &str| {
        keys.get(name)
            .copied()
            .ok_or_else(|| format!("unbekannte Taste '{name}'"))
    };
    match op {
        "tap" => Ok(Step::Tap(code(arg)?)),
        "down" => Ok(Step::Down(code(arg)?)),
        "up" => Ok(Step::Up(code(arg)?)),
        "wait" => Ok(Step::Wait(arg.parse().map_err(|_| format!("'{arg}' ist keine Zahl"))?)),
        _ => Err(format!("unbekannter Schritt '{op}'")),
    }
}

fn send(virt: &Mutex<VirtualDevice>, code: u16, value: i32) -> Result<(), Box<dyn Error>> {
    let events = [
        InputEvent::new(EventType::KEY.0, code, value),
        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
    ];
    virt.lock().unwrap().emit(&events)?;
    Ok(())
}

fn run_steps(virt: &Mutex<VirtualDevice>, steps: &[Step]) -> Result<(), Box<dyn Error>> {
    for step in steps {
        match *step {
            Step::Tap(c) => {
                send(virt, c, 1)?;
                send(virt, c, 0)?;
            }
            Step::Down(c) => send(virt, c, 1)?,
            Step::Up(c) => send(virt, c, 0)?,
            Step::Wait(ms) => thread::sleep(Duration::from_millis(ms)),
        }
    }
    Ok(())
}

/// Bedient ein Gerät: grabt es, leitet Ereignisse weiter und startet Makros bei Auslösung.
fn serve(device: String, bindings: Vec<Arc<Bound>>) -> Result<(), Box<dyn Error>> {
    let (path, mut dev) = input::find(&device).ok_or_else(|| format!("kein evdev-Gerät mit '{device}'"))?;
    dev.grab()?;

    let mut keys = AttributeSet::<KeyCode>::new();
    for c in key_table().values() {
        keys.insert(KeyCode(*c));
    }
    let virt = Arc::new(Mutex::new(
        VirtualDevice::builder()?.name("periphera virtual").with_keys(&keys)?.build()?,
    ));
    println!("Aktiv: {} ({}), {} Makro(s)", dev.name().unwrap_or("?"), path.display(), bindings.len());

    loop {
        for ev in dev.fetch_events()? {
            if ev.event_type() == EventType::KEY {
                if let Some(b) = bindings.iter().find(|b| b.trigger == ev.code()) {
                    // Auslöser komplett schlucken (Druck, Wiederholung, Loslassen).
                    if ev.value() == 1 && !b.busy.swap(true, Ordering::SeqCst) {
                        let b = Arc::clone(b);
                        let virt = Arc::clone(&virt);
                        thread::spawn(move || {
                            if let Err(e) = run_steps(&virt, &b.steps) {
                                eprintln!("Makro-Fehler: {e}");
                            }
                            b.busy.store(false, Ordering::SeqCst);
                        });
                    }
                    continue;
                }
            }
            virt.lock().unwrap().emit(&[ev])?;
        }
    }
}

/// Liest und prüft die Konfiguration. Gibt Gerätename -> Makros zurück, ohne Geräte zu öffnen.
fn load(path: &str) -> Result<HashMap<String, Vec<Arc<Bound>>>, Box<dyn Error>> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let cfg: Config = toml::from_str(&text).map_err(|e| format!("{path}: {e}"))?;
    if cfg.macros.is_empty() {
        return Err(format!("{path}: keine [[macro]]-Einträge").into());
    }

    let keys = key_table();
    let mut groups: HashMap<String, Vec<Arc<Bound>>> = HashMap::new();
    for m in cfg.macros {
        let trigger = *keys
            .get(&m.trigger)
            .ok_or_else(|| format!("unbekannte Auslösetaste '{}'", m.trigger))?;
        let steps = m
            .steps
            .iter()
            .map(|s| parse_step(s, &keys))
            .collect::<Result<Vec<_>, _>>()?;
        groups
            .entry(m.device)
            .or_default()
            .push(Arc::new(Bound { trigger, steps, busy: AtomicBool::new(false) }));
    }
    Ok(groups)
}

/// Prüft die Konfiguration und zeigt die Bindungen an, ohne Eingabegeräte zu greifen.
pub fn check(path: &str) -> Result<(), Box<dyn Error>> {
    for (device, bindings) in load(path)? {
        println!("Gerät '{device}': {} Makro(s)", bindings.len());
        for b in &bindings {
            println!("  Auslöser {} -> {} Schritt(e)", input::key_name(b.trigger), b.steps.len());
        }
    }
    println!("Konfiguration OK");
    Ok(())
}

/// Lädt alle Makros und bedient die Geräte, bis das Programm beendet wird.
pub fn run(path: &str) -> Result<(), Box<dyn Error>> {
    let groups = load(path)?;

    let handles: Vec<_> = groups
        .into_iter()
        .map(|(device, bindings)| {
            thread::spawn(move || serve(device, bindings).map_err(|e| e.to_string()))
        })
        .collect();
    for h in handles {
        match h.join() {
            Ok(Err(e)) => return Err(e.into()),
            Err(_) => return Err("Gerät-Thread abgestürzt".into()),
            Ok(Ok(())) => {}
        }
    }
    Ok(())
}
