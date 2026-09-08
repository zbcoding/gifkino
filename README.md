# Gifkino

Edit animated GIFs. Import mp4 videos as resized gif animations, insert frames, add text, add arrows and shapes, then optimize gifs with an updating size.

## Features

- **Open videos, gifs, and images:** GIFs, still images, and videos (with auto resizing on import for large files).
- **Overlays scoped to frame ranges:** text, shapes (rect, ellipse, arrow)
  and images, placed on the frames you pick. Drag, rotate
  (Alt), keep aspect (Shift), or resize from the center (Ctrl) on the canvas.
- **Frame list editing:** reorder, duplicate, delete, retime, cut/copy/paste,
  drop one frame in every N, and smart-drop the frames that move the least.
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

Rust + GTK4 + libadwaita.

<details>
<summary>Dependencies and build steps</summary>

You need the GTK development packages, ffmpeg and gifsicle:

```bash
# Ubuntu 24.04 / Debian 13
sudo apt-get install libgtk-4-dev libadwaita-1-dev ffmpeg gifsicle
```

### Build and run

```bash
cargo run                  # welcome state
cargo run -- path/to.gif   # open a GIF or video directly
cargo test                 # whole suite, well under a minute
```

</details>

## License

MIT — see [LICENSE](LICENSE).

## Contributions
The best way to contribute is to make a comment in [Issues](https://github.com/zbcoding/gifkino/issues) with screenshots and context. You can also write or generate a pull request. Contributions may be added to free or paid versions of this software.