#!/usr/bin/env bash
set -Eeuo pipefail
app_id='io.github.eugenek32.SlateAmp'
data_home="${XDG_DATA_HOME:-$HOME/.local/share}"
rm -f "$HOME/.local/bin/slateamp" "$HOME/.local/bin/slateampctl" \
    "$data_home/applications/$app_id.desktop" \
    "$data_home/icons/hicolor/scalable/apps/slateamp.svg"
for size in 128 256 512; do
    rm -f "$data_home/icons/hicolor/${size}x${size}/apps/$app_id.png"
done
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$data_home/applications" || true
fi
printf 'SlateAmp removed. Settings and library data were preserved.\n'
