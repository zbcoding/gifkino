#!/usr/bin/env bash
# Install Gifkino's desktop entry, metainfo and icon into the user prefix, and
# point a `gifkino` on PATH at the local build. `--uninstall` removes all of it.
#
# This is what makes a development build look like an installed application: a
# taskbar has no idea what a binary in target/ is. The chain it walks is the
# window's Wayland app_id -> a .desktop file whose name matches it -> that
# file's Icon= key -> an icon of that name in a theme directory. The app
# announces `io.github.zbcoding.Gifkino` (RelmApp::new in src/main.rs sets the
# GApplication id, and GTK hands it to xdg_toplevel.set_app_id), so with none of
# these files installed there is nothing to match and the window gets a generic
# placeholder icon. Neither the flatpak nor the AppImage needs this - they carry
# the same three files themselves.
#
# The binary symlink is not cosmetic: the desktop entry's Exec and TryExec name
# `gifkino`, and an entry whose TryExec does not resolve is treated as invalid,
# which loses the icon match again. The symlink points into target/release, so a
# rebuild is picked up with no reinstall.
set -euo pipefail

app_id=io.github.zbcoding.Gifkino
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
data=${XDG_DATA_HOME:-$HOME/.local/share}
bin=$HOME/.local/bin

desktop=$data/applications/$app_id.desktop
metainfo=$data/metainfo/$app_id.metainfo.xml
icon=$data/icons/hicolor/scalable/apps/$app_id.svg

# Both caches are best-effort: the desktop database is what GLib reads, and
# kbuildsycoca6 is KDE's, absent on a GNOME-only machine.
refresh() {
  command -v update-desktop-database >/dev/null && \
    update-desktop-database "$data/applications" || true
  command -v gtk4-update-icon-cache >/dev/null && \
    gtk4-update-icon-cache -q -t -f --ignore-theme-index "$data/icons/hicolor" || true
  command -v kbuildsycoca6 >/dev/null && kbuildsycoca6 --noincremental >/dev/null 2>&1 || true
}

if [ "${1:-}" = "--uninstall" ]; then
  rm -f "$desktop" "$metainfo" "$icon" "$bin/gifkino"
  refresh
  echo "removed $app_id from $data"
  exit 0
fi

[ -x "$repo/target/release/gifkino" ] || {
  echo "no release build at target/release/gifkino - run: cargo build --release" >&2
  exit 1
}

install -Dm644 "$repo/xdg/$app_id.desktop" "$desktop"
install -Dm644 "$repo/xdg/$app_id.metainfo.xml" "$metainfo"
install -Dm644 "$repo/xdg/$app_id.svg" "$icon"
mkdir -p "$bin"
ln -sf "$repo/target/release/gifkino" "$bin/gifkino"
refresh

echo "installed $app_id into $data"
case ":$PATH:" in
  *":$bin:"*) ;;
  *) echo "warning: $bin is not on PATH, so the desktop entry's TryExec will fail" >&2 ;;
esac
