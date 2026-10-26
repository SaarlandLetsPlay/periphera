#!/usr/bin/env bash
# Baut periphera im Release-Modus und ersetzt die lokal installierte Kopie.
#
# Aufruf:  scripts/deploy.sh            # bauen + ersetzen, laufende GUI bleibt unberührt
#          scripts/deploy.sh --restart  # zusätzlich laufende GUI/Makro-Engine neu starten
#
# Die Datei wird atomar ersetzt (Kopie nach .new, dann mv). Dadurch klappt das Ersetzen
# auch dann, wenn das Programm gerade läuft (cp würde mit "busy" abbrechen).

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="${HOME}/.local/bin/periphera"
RESTART=0
[[ "${1:-}" == "--restart" ]] && RESTART=1

cd "$ROOT"
cargo build --release

install -m 0755 target/release/periphera "${DEST}.new"
mv -f "${DEST}.new" "$DEST"
echo "Installiert: $DEST ($(stat -c %y "$DEST" | cut -d. -f1))"

if pgrep -f "${DEST} " >/dev/null || pgrep -x periphera >/dev/null; then
    if (( RESTART )); then
        pkill -f "^${DEST} gui" 2>/dev/null || true
        pkill -f "^${DEST} macros run" 2>/dev/null || true
        sleep 1
        nohup "$DEST" gui >"${HOME}/.local/state/periphera/gui.log" 2>&1 &
        echo "GUI neu gestartet."
    else
        echo "Hinweis: periphera läuft noch mit der alten Version. Neu starten oder mit --restart ausführen."
    fi
fi
