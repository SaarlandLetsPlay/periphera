# periphera

Zentrale Oberfläche zur Verwaltung von Peripheriegeräten unter Linux:
DPI, Beleuchtung (statisch, Atmen, Farbwechsel, Knight Rider, Welle, Regenbogen, Feuer)
und Tastenmakros, synchron über alle beleuchteten Geräte.

Die Kommunikation mit den Geräten erfolgt über das HID++-Protokoll.
Das Projekt steht in keiner Verbindung zum Hersteller der unterstützten Geräte.

## Befehle

```
periphera gui                     Oberfläche öffnen
periphera list                    Erkannte Geräte
periphera dpi get|list|set <DPI>  DPI lesen, Werte anzeigen, setzen
periphera light set <RRGGBB> [--hell N]
periphera light anim <name> ...   Animation (Strg+C beendet)
periphera light stop
periphera keys "<Gerät>"          Tastendrücke live anzeigen
periphera macros check|run       Makro-Konfiguration prüfen bzw. starten
```

## Makros

Konfiguration unter `~/.config/periphera/macros.toml`. Die Oberfläche erstellt und
bearbeitet sie; Tasten werden über "Taste lernen" erfasst.

## Bauen

```
cargo build --release
```

Für den Zugriff ohne root installiert das Paket die udev-Regel
`packaging/70-periphera.rules`.

## Lizenz

MIT, siehe LICENSE.
