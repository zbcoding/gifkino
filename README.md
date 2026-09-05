# Gifkino

Edit animated GIFs along the time axis. Every edit knows which frames it
applies to: add a caption once and it spans the range you chose, instead of
being retyped on every frame it should cover.

## Features

- **Open anything:** GIFs, still images, and videos (decoded through ffmpeg,
  with a frame budget you set before a byte is decoded).
- **Overlays scoped to frame ranges:** text, shapes (rect, ellipse, arrow)
  and images, each living on exactly the frames you pick. Drag, rotate
  (Alt), keep aspect (Shift), or resize from the center (Ctrl) on the canvas.
- **Whole-animation transforms:** crop, resize (with aspect-ratio lock and a
  live memory estimate), zoom, rotate and flip, with overlays moving along.
- **Frame list editing:** reorder, duplicate, delete, retime, cut/copy/paste,
  drop one frame in every N, and smart-drop the frames that move the least.
- **Timeline that zooms** (Ctrl+wheel), with overlay bands showing what covers
  what.
- **Undo and redo** over every edit, including scoped overlay changes.
- **Rebindable shortcuts** throughout (`Ctrl+?` opens the editor); tooltips
  and menus follow your bindings.
- **Memory budgets** for imports, operations and total usage, tunable in
  `~/.config/gifkino/settings.conf`.
- **Translated UI:** English, German and Japanese.
- **Optimized GIF export.**

## Download and install

[**Download the latest release**](https://github.com/zbcoding/gifkino/releases/latest)
— grab `Gifkino.flatpak` or `Gifkino-x86_64.AppImage`. Both builds are
self-contained: each carries the ffmpeg, ffprobe and gifsicle programs the
editor drives as subprocesses, so import and the optimized export work on a
machine that has none of them installed.

- **Flatpak** also brings its own GTK, so it runs anywhere flatpak does,
  Ubuntu 22.04 included: `flatpak install --bundle Gifkino.flatpak`.
- **AppImage** uses the GTK inside the image and is built against glibc 2.39,
  so it needs Ubuntu 24.04, Debian 13, Fedora 40 or newer. `chmod +x` it and
  run it.

## Build from source

Rust + GTK4 + libadwaita. You need the GTK development packages, ffmpeg and
gifsicle:

```bash
# Ubuntu 24.04 / Debian 13
sudo apt-get install libgtk-4-dev libadwaita-1-dev ffmpeg gifsicle
```

```bash
cargo run                  # welcome state
cargo run -- path/to.gif   # open a GIF or video directly
cargo test                 # whole suite, well under a minute
```

## Status

0.1.0 is the first release.

## License

MIT — see [LICENSE](LICENSE).
