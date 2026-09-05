# Release checklist

## Version numbers

Bump all three, in one commit:

1. `Cargo.toml` — `version`. `scripts/build-appimage.sh` and the CI smoke
   test read it from here; `Cargo.lock` follows on the next build.
2. `CHANGELOG.md` — rename the top `## Unreleased` section to the version.
3. `xdg/io.github.zbcoding.Gifkino.metainfo.xml` — prepend a
   `<release version="X.Y.Z" date="YYYY-MM-DD">` entry under `<releases>`.

## Ship it

```bash
git push origin main        # CI runs the test job
git tag vX.Y.Z
git push origin vX.Y.Z      # CI builds, smoke-tests, publishes
```

A `v*` tag runs `.github/workflows/release.yml`: AppImage on Ubuntu 24.04,
Flatpak through flatpak-builder, an xvfb launch test of the image, then a
GitHub release with both files attached and download notes naming which
distro each one runs on.

## Local builds are verification, not artifacts

`scripts/build-appimage.sh` bundles a release AppImage from the local tree,
but the glibc floor follows the build host: an image built on a rolling
distro only runs on distros at least that new. Build locally to prove the
packaging works; ship only what CI built on 24.04. Host expectations (cargo,
GTK4 + libadwaita dev packages, ffmpeg, gifsicle, schemas, icon theme,
pixbuf loaders, rsvg, wget) are listed in the script header.
