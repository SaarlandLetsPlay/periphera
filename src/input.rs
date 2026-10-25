//! Eingabe-Ebene (evdev): Geräte finden, Ereignisse mitlesen.

use evdev::{Device, EventType, KeyCode};
use std::error::Error;
use std::path::PathBuf;

/// Findet das erste evdev-Gerät, dessen Name `name_part` enthält.
pub fn find(name_part: &str) -> Option<(PathBuf, Device)> {
    evdev::enumerate().find(|(_, dev)| {
        dev.name()
            .map(|n| n.contains(name_part))
            .unwrap_or(false)
    })
}

/// Lesbarer Name eines Tasten- oder Button-Codes, z. B. "KEY_A" oder "BTN_SIDE".
pub fn key_name(code: u16) -> String {
    format!("{:?}", KeyCode(code))
}

/// Wartet im Hintergrund auf den nächsten Tastendruck am Tastatur-Eingang des Geräts.
/// Liefert den Tastennamen über den Kanal. Mit `cancel` lässt sich das Warten abbrechen.
pub fn learn(
    name_part: String,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> std::sync::mpsc::Receiver<Result<String, String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| -> Result<Option<String>, Box<dyn Error>> {
            let (_, mut dev) = find(&name_part).ok_or_else(|| format!("kein Eingabegerät mit '{name_part}'"))?;
            dev.set_nonblocking(true)?;
            while !cancel.load(std::sync::atomic::Ordering::SeqCst) {
                match dev.fetch_events() {
                    Ok(events) => {
                        for ev in events {
                            if ev.event_type() == EventType::KEY && ev.value() == 1 {
                                return Ok(Some(key_name(ev.code())));
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(e.into()),
                }
                std::thread::sleep(std::time::Duration::from_millis(15));
            }
            Ok(None)
        })();
        let msg = match result {
            Ok(Some(key)) => Ok(key),
            Ok(None) => Err("abgebrochen".to_string()),
            Err(e) => Err(e.to_string()),
        };
        let _ = tx.send(msg);
    });
    rx
}

/// Zeigt alle Tasten- und Button-Ereignisse eines Geräts live an (zum Ermitteln von Codes).
pub fn watch(name_part: &str) -> Result<(), Box<dyn Error>> {
    let (path, mut dev) = find(name_part).ok_or_else(|| format!("kein evdev-Gerät mit '{name_part}'"))?;
    println!("Lausche auf {} ({})", dev.name().unwrap_or("?"), path.display());
    println!("Taste drücken, Strg+C beendet.");
    loop {
        for ev in dev.fetch_events()? {
            if ev.event_type() == EventType::KEY {
                let state = match ev.value() {
                    0 => "losgelassen",
                    1 => "gedrückt",
                    2 => "wiederholt",
                    _ => "?",
                };
                println!("  {:<14} {:>3}  {}", key_name(ev.code()), ev.code(), state);
            }
        }
    }
}
