#!/usr/bin/env bash
# Build GifEditor-x86_64.AppImage from a release cargo build.
#
# Build host decides the floor. Cargo.toml gates the bindings at GTK 4.14 /
# libadwaita 1.5, which is exactly what Ubuntu 24.04 ships, so building there
# gives an image that runs on 24.04 and everything newer. Building on a rolling
# distro raises the glibc requirement with nothing gained - use a noble
# container if your machine is newer.
#
# Host expectations: cargo, the GTK4 + libadwaita development packages, ffmpeg,
# gifsicle, gsettings-desktop-schemas, adwaita-icon-theme, the gdk-pixbuf
# loaders and gdk-pixbuf-query-loaders, librsvg2-bin, wget.
#
# ponytail: fetches linuxdeploy and appimagetool from their rolling "continuous"
# builds because neither ships tagged releases. Pin to a mirrored copy if a
# reproducible build matters more than tracking upstream fixes.
set -euo pipefail

app_id=io.github.zbcoding.GifEditor
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
work=${1:-$repo/appimage-build}
tools=$work/tools
appdir=$work/AppDir

version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$repo/Cargo.toml" | head -1)
[ -n "$version" ] || { echo "could not read version from Cargo.toml" >&2; exit 1; }

rm -rf "$appdir"
mkdir -p "$appdir/usr/bin" "$appdir/usr/share/applications" \
         "$appdir/usr/share/metainfo" "$appdir/usr/share/glib-2.0/schemas" \
         "$appdir/usr/share/icons" "$appdir/usr/share/gif-editor" "$tools"

# --- 1. Build ---------------------------------------------------------------
cargo build --release --manifest-path "$repo/Cargo.toml"

# --- 2. Desktop entry, metainfo, translations -------------------------------
# The @@ markers in Exec are flatpak's file-forwarding syntax; outside a
# sandbox they would reach the binary as literal arguments.
sed 's/^Exec=.*/Exec=gif-editor %f/' "$repo/xdg/$app_id.desktop" \
  > "$appdir/usr/share/applications/$app_id.desktop"
cp "$repo/xdg/$app_id.metainfo.xml" "$appdir/usr/share/metainfo/"
# i18n.rs reads .po files directly; AppRun points GIF_EDITOR_PO_DIR here. The
# rest of po/ is translator tooling and has no business in the image.
install -Dm644 "$repo"/po/*.po -t "$appdir/usr/share/gif-editor/po/"

# --- 3. Icons ---------------------------------------------------------------
# The app icon is an SVG; rasterize the sizes a desktop actually indexes, and
# add Adwaita on top because GTK widget chrome draws from it.
for size in 512 256 128 64 48; do
  dir=$appdir/usr/share/icons/hicolor/${size}x${size}/apps
  mkdir -p "$dir"
  rsvg-convert -w "$size" -h "$size" "$repo/xdg/$app_id.svg" -o "$dir/$app_id.png"
done
mkdir -p "$appdir/usr/share/icons/hicolor/scalable/apps"
cp "$repo/xdg/$app_id.svg" "$appdir/usr/share/icons/hicolor/scalable/apps/"
[ -d /usr/share/icons/Adwaita ] && cp -r /usr/share/icons/Adwaita "$appdir/usr/share/icons/"
icon=$appdir/usr/share/icons/hicolor/256x256/apps/$app_id.png

# --- 4. GSettings schemas ---------------------------------------------------
# libadwaita reads org.gnome.desktop.* at startup; ship the host's compiled set.
cp /usr/share/glib-2.0/schemas/gschemas.compiled \
  "$appdir/usr/share/glib-2.0/schemas/"

# --- 5. gdk-pixbuf loaders --------------------------------------------------
# SVG icon rendering and file-chooser thumbnails go through these plugins. On
# Debian/Ubuntu each is a separate dlopen'd .so - bundle the set and repoint the
# cache at the AppDir copy. Distros that compile the common loaders into
# libgdk_pixbuf itself (e.g. Arch) have nothing to copy; GTK uses those
# built-ins and the AppRun leaves the pixbuf env vars unset.
dest_moduledir=$appdir/usr/lib/gdk-pixbuf-2.0/2.10.0/loaders
png_loader=$(find /usr/lib -name 'libpixbufloader-png.so' -print -quit 2>/dev/null || true)
bundled_loaders=0
if [ -n "$png_loader" ]; then
  src_moduledir=$(dirname "$png_loader")
  mkdir -p "$dest_moduledir"
  cp "$src_moduledir"/*.so "$dest_moduledir/"
  GDK_PIXBUF_MODULEDIR=$dest_moduledir gdk-pixbuf-query-loaders \
    > "$appdir/usr/lib/gdk-pixbuf-2.0/2.10.0/loaders.cache"
  sed -i "s|$dest_moduledir/|loaders/|" \
    "$appdir/usr/lib/gdk-pixbuf-2.0/2.10.0/loaders.cache"
  bundled_loaders=1
fi

# --- 6. Bundle native libraries ---------------------------------------------
wget -qO "$tools/linuxdeploy" \
  https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage
wget -qO "$tools/appimagetool" \
  https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
chmod +x "$tools/linuxdeploy" "$tools/appimagetool"
export APPIMAGE_EXTRACT_AND_RUN=1   # CI runners have no FUSE

# The binary links GTK and libadwaita through ELF DT_NEEDED entries, so
# linuxdeploy walks the dependency graph on its own - unlike a project that
# dlopen's the stack by soname. ffmpeg, ffprobe and gifsicle are extra
# executables, not libraries: src/pipeline drives them over pipes, and each is
# passed here so its own library closure comes along. Their absence would only
# grey out import and the -O3 export pass, but an image that cannot open a
# video is not worth shipping.
libs=()
if [ "$bundled_loaders" = 1 ]; then
  for so in "$dest_moduledir"/*.so; do libs+=(--library "$so"); done
fi
helpers=()
for tool in ffmpeg ffprobe gifsicle; do
  path=$(command -v "$tool" || true)
  if [ -n "$path" ]; then
    helpers+=(--executable "$path")
  else
    echo "warning: $tool not found on the host; the AppImage will ship without it" >&2
  fi
done

"$tools/linuxdeploy" --appdir "$appdir" \
  --executable "$repo/target/release/gif-editor" \
  --desktop-file "$appdir/usr/share/applications/$app_id.desktop" \
  --icon-file "$icon" \
  "${helpers[@]}" "${libs[@]}"

# --- 7. Third-party licenses ------------------------------------------------
# The GTK/GLib/Pango/gdk-pixbuf/librsvg stack is LGPL and the bundled ffmpeg is
# GPL: shipping the binaries is fine as long as the notices travel with them and
# the user can get the corresponding source. Copy each bundled library's distro
# copyright file when the host is Debian-family; always leave a pointer to
# unmodified upstream sources.
docdir=$appdir/usr/share/doc/gif-editor
mkdir -p "$docdir/third-party"
cp "$repo/LICENSE" "$docdir/"
if command -v dpkg-query >/dev/null 2>&1; then
  { find "$appdir/usr/lib" -maxdepth 1 -name '*.so*' -type f -printf '%f\n'
    printf 'ffmpeg\nffprobe\ngifsicle\n'; } \
  | while read -r name; do
      pkg=$(dpkg-query -S "*/$name" 2>/dev/null | awk -F: 'NR==1 {print $1}') || continue
      src=/usr/share/doc/$pkg/copyright
      [ -n "$pkg" ] && [ -f "$src" ] && cp "$src" "$docdir/third-party/$pkg.copyright"
    done
fi
cat > "$docdir/THIRD-PARTY.AppImage.md" <<'EOF'
# Bundled software

GIF Editor itself is MIT (see ./LICENSE). This AppImage additionally carries
the GTK 4 / libadwaita runtime it links against and the supporting stack
(GLib, Pango, Cairo, gdk-pixbuf, Graphene, HarfBuzz, librsvg, FreeType,
Fontconfig, pixman and the usual image codecs), plus the ffmpeg, ffprobe and
gifsicle programs the app runs as subprocesses.

Every bundled binary is an unmodified build taken from the Ubuntu 24.04
archive. Per-package license texts are in ./third-party/. Corresponding source
is the matching `deb-src` entry for Ubuntu 24.04 (noble).

The GTK stack is LGPL and ffmpeg as Ubuntu builds it is GPL. Both are separate
files inside the image rather than linked into the editor. To use your own
build of one: extract the AppImage
(`./GifEditor-x86_64.AppImage --appimage-extract`), replace the file under
`squashfs-root/usr/lib/` or `squashfs-root/usr/bin/`, and repack with
`appimagetool squashfs-root`.
EOF

# --- 8. AppRun + pack -------------------------------------------------------
# linuxdeploy left AppRun as a symlink to usr/bin/gif-editor; drop it so the
# heredoc writes a real file instead of overwriting the binary through it.
rm -f "$appdir/AppRun"
cat > "$appdir/AppRun" <<'EOF'
#!/bin/sh
here=$(dirname "$(readlink -f "$0")")
export LD_LIBRARY_PATH="$here/usr/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export GSETTINGS_SCHEMA_DIR="$here/usr/share/glib-2.0/schemas"
export XDG_DATA_DIRS="$here/usr/share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
export GIF_EDITOR_PO_DIR="$here/usr/share/gif-editor/po"
# The bundled ffmpeg/ffprobe/gifsicle win over the host's, which may be absent.
export PATH="$here/usr/bin:$PATH"
loader_cache="$here/usr/lib/gdk-pixbuf-2.0/2.10.0/loaders.cache"
if [ -f "$loader_cache" ]; then
  export GDK_PIXBUF_MODULEDIR="$here/usr/lib/gdk-pixbuf-2.0/2.10.0/loaders"
  export GDK_PIXBUF_MODULE_FILE="$loader_cache"
fi
exec "$here/usr/bin/gif-editor" "$@"
EOF
chmod +x "$appdir/AppRun"

# Guard against the binary being clobbered (e.g. a heredoc following the AppRun
# symlink linuxdeploy leaves) - the AppImage would still build and only fail
# when a user runs it.
file "$appdir/usr/bin/gif-editor" | grep -q ELF \
  || { echo "usr/bin/gif-editor is not an ELF binary" >&2; exit 1; }
file "$appdir/AppRun" | grep -q 'shell script' \
  || { echo "AppRun is not a script" >&2; exit 1; }

ARCH=x86_64 "$tools/appimagetool" "$appdir" "$repo/GifEditor-x86_64.AppImage"
echo "built: $repo/GifEditor-x86_64.AppImage  (version $version)"
