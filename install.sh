#!/usr/bin/env bash
set -Eeuo pipefail

# Build and register SlateAmp for the current GNOME user (no sudo).
project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
cd "$project_dir"

for cmd in cargo rustc pkg-config install; do
    if ! command -v "$cmd" >/dev/null 2>&1; then
        printf 'Missing %s. Arch: sudo pacman -S --needed base-devel rust pkgconf\n' "$cmd" >&2
        exit 1
    fi
done
for pc in gtk4 libadwaita-1 gstreamer-1.0; do
    if ! pkg-config --exists "$pc"; then
        printf 'Missing %s. Arch: sudo pacman -S --needed gtk4 libadwaita gstreamer\n' "$pc" >&2
        exit 1
    fi
done

cargo build --release

binary_path="$HOME/.local/bin/slateamp"
data_home="${XDG_DATA_HOME:-$HOME/.local/share}"
applications_dir="$data_home/applications"
icon_dir="$data_home/icons/hicolor"
app_id='io.github.eugenek32.SlateAmp'
desktop_path="$applications_dir/$app_id.desktop"
mkdir -p "$applications_dir"

install -Dm755 target/release/slateamp "$binary_path"
install -Dm755 scripts/slateampctl "$HOME/.local/bin/slateampctl"
for size in 128 256 512; do
    install -Dm644 "assets/$app_id-$size.png" "$icon_dir/${size}x${size}/apps/$app_id.png"
done
# Remove the old SVG alias; the new icon uses the same name as the app ID.
rm -f "$icon_dir/scalable/apps/slateamp.svg"
# Use an absolute PNG path: GNOME may not resolve the per-user icon theme
# immediately, and users previously needed to patch this manually.
icon_path="$icon_dir/256x256/apps/$app_id.png"

# Use an absolute Exec: GNOME's launch environment may lack ~/.local/bin in PATH.
# Desktop Entry quoting is different from shell quoting. This handles paths with
# ordinary spaces or shell metacharacters without relying on a login shell.
desktop_exec=${binary_path//\\/\\\\}
desktop_exec=${desktop_exec//\"/\\\"}
desktop_exec=${desktop_exec//\$/\\\$}
desktop_exec=${desktop_exec//\`/\\\`}
cat > "$desktop_path" <<DESKTOP
[Desktop Entry]
Type=Application
Name=SlateAmp
GenericName=Music Player
GenericName[ru]=Музыкальный проигрыватель
Comment=Music and internet radio
Comment[ru]=Музыка и интернет-радио
Exec="$desktop_exec"
Icon=$icon_path
Terminal=false
Categories=AudioVideo;Audio;Player;GTK;
Keywords=music;radio;playlist;audio;winamp;
Keywords[ru]=музыка;радио;плейлист;аудио;
StartupNotify=true
DESKTOP
chmod 644 "$desktop_path"

if command -v desktop-file-validate >/dev/null 2>&1; then
    desktop-file-validate "$desktop_path"
fi
# The desktop database contains MIME associations; icon cache is optional.
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$applications_dir" || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1 && [[ -e "$icon_dir/index.theme" ]]; then
    gtk-update-icon-cache -f -t "$icon_dir" >/dev/null 2>&1 || true
fi

printf '\nSlateAmp installed.\n'
printf '  Binary:  %s\n' "$binary_path"
printf '  Launcher: %s\n' "$desktop_path"
printf '  Icon:    %s\n' "$icon_dir/256x256/apps/$app_id.png"
printf '  Media controls: %s\n' "$HOME/.local/bin/slateampctl"
printf '\nCheck launcher: gio launch "%s"\n' "$desktop_path"
printf 'If GNOME search does not refresh immediately, log out and back in.\n'
