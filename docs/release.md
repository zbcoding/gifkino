# Release checklist

## Changes checklist
Before making a release, make sure that these are current:
- Update changelog.md
- Add translations
- Add new features' keybindings and key shortcuts

## Version numbers

Bump all three, in one commit:

1. `Cargo.toml` — `version`. `scripts/build-appimage.sh` and the CI smoke
   test read it from here; `Cargo.lock` follows on the next build.
2. `CHANGELOG.md` — rename the top `## Unreleased` section to the version.
3. `xdg/io.github.zbcoding.Gifkino.metainfo.xml` — prepend a
   `<release version="X.Y.Z" date="YYYY-MM-DD">` entry under `<releases>`.

## Before you tag

A green push to `main` says nothing about either package. `test` is the only
job a branch push runs; `appimage` and `flatpak` are gated on a `v*` tag or a
manual dispatch, and they both `needs: test`, so anything the test job catches
skips them entirely and the failure looks worse than it is.

1. `cargo fmt --check`, then `cargo test`. CI runs fmt as a gate ahead of the
   tests. The suite includes the packaging guards described below, which cost
   nothing here and twenty minutes of CI each.
2. `gh workflow run release.yml --ref main` — builds and smoke-tests both
   packages without publishing anything, since the release job is tag-only.
   Wait for it to go green before tagging. Budget ~5 minutes for the AppImage
   and ~20 for the Flatpak from a cold module cache.
3. Only if step 2 fails on the Flatpak, or you would rather not wait on it:
   build the bundle locally. It needs `org.gnome.Platform//50`,
   `org.gnome.Sdk//50` and `org.freedesktop.Sdk.Extension.rust-stable//25.08`,
   and the manifest's `type: dir` source means it builds the working tree,
   uncommitted changes included.

   ```bash
   flatpak-builder --user --state-dir=/tmp/gk-fb --repo=/tmp/gk-repo \
     --force-clean /tmp/gk-build io.github.zbcoding.Gifkino.yml
   flatpak build-bundle /tmp/gk-repo /tmp/Gifkino.flatpak \
     io.github.zbcoding.Gifkino \
     --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo
   flatpak install --user -y --noninteractive --bundle /tmp/Gifkino.flatpak
   flatpak run --command=ffmpeg io.github.zbcoding.Gifkino -version
   flatpak uninstall --user -y io.github.zbcoding.Gifkino
   ```

   The install is the half that matters: flatpak validates and rewrites the
   desktop entry there, not at build time, so a bundle can build and still
   refuse to install.

## The FlatPark asset

Each release also carries `Gifkino-flatpark-x86_64.tar.xz`: the `/app` tree
from the same `flatpak-builder` run that produces `Gifkino.flatpak`, packed
straight out of `build-dir/files`. It exists because
[FlatPark](https://flatpark.org) is a signed flatpak remote whose packages are
all **extra-data** — it never rebuilds an app and never rehosts one, so it can
take neither the bundle (a repo commit, installable only by `flatpak install
--bundle`) nor, sensibly, the AppImage, which would drag a second noble-built
GTK stack into a GNOME runtime and drift from this bundle release by release.
Handing FlatPark the bundle's own contents keeps one lineage: one `gifkino`
binary, one ffmpeg, one gifsicle, one set of catalogs, whichever channel a
user installs from.

What the asset promises, and what breaks if a change here stops being true:

1. **It is the `/app` tree, contents at the archive root** — `bin/`, `lib/`,
   `share/`. No top-level directory to strip.
2. **It is relocatable.** FlatPark unpacks it at `/app/extra/gifkino`, not
   `/app`, so nothing in it may depend on its own absolute path. Two things
   currently would, and its wrapper sets both: `GIFKINO_PO_DIR` at
   `share/gifkino/po` (`i18n.rs` walks four levels up from the executable and
   then falls back to `/app/share/gifkino/po`, neither of which lands there),
   and `PATH` at `bin/` (`pipeline/caps.rs` probes `ffmpeg`, `ffprobe` and
   `gifsicle` by name). **Anything new that resolves a path at runtime must
   either be found relative to the executable or get an env var, and the
   FlatPark wrapper has to learn about it** — a new helper binary is fine
   because `PATH` already covers `bin/`.
3. **It carries the helper programs and the catalogs.** The workflow asserts
   `bin/gifkino`, `bin/ffmpeg`, `bin/ffprobe`, `bin/gifsicle`,
   `share/gifkino/po` and `share/applications/<app-id>.desktop` are present
   and fails the release if one is missing.
4. **It targets `org.gnome.Platform//50`**, because that is what the manifest
   built it against. Bumping `RUNTIME_VERSION` in the workflow, or the
   `runtime-version` in the manifest, is a change FlatPark has to follow in
   [its own manifest](https://github.com/flatpark/flatpark/tree/main/registry/io.github.zbcoding.Gifkino)
   — the tree's `libgtk` comes from the runtime, so a mismatch is an install
   that starts against the wrong GTK or does not start at all. Say so in the
   release notes when it moves.
5. **The desktop entry, icon and metainfo in the tree are not what FlatPark
   exports.** Extra-data is fetched on the user's machine, long after the
   flatpak is built, so FlatPark ships its own copies of all three from its
   registry directory. Changing ours (a new MIME type, a renamed icon) means
   opening a PR there too.

The `xdg/` files and this asset are the only two places a packaging change has
to be mirrored outside this repository. Nothing about the AppImage feeds
FlatPark any more.

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

## What has failed a release here

All three of these failed the first attempt at v0.1.0, one behind the other —
the fmt gate hid the other two, and both of those surface only in the flatpak
job, ten to twenty minutes in.

- `cargo fmt --check` exiting 1, with `appimage`, `flatpak` and `release` all
  skipped. A formatting diff in a test module, nothing more.
- `ERROR: appstreamcli compose failed` during the flatpak build, reported as
  `file-read-error` on `xdg/io.github.zbcoding.Gifkino.svg`. gdk-pixbuf
  identifies an image by sniffing the first 256 bytes for the opening tag, so a
  comment block in front of `<svg` makes a valid icon an "unrecognized image
  file format". Only the flatpak noticed: `rsvg-convert`, which the AppImage
  build uses, goes through librsvg directly and never sniffs.
  `the_app_icon_announces_itself_before_the_sniffers_give_up` locks it down.
- `Failed to install bundle: Invalid Exec argument @@` when the bundle is
  installed. flatpak's exporter writes the `@@ %f @@` file-forwarding markers
  itself, out of a plain `%f`, and rejects an app that ships them.
  `the_packaged_exec_leaves_file_forwarding_to_flatpak` locks that down.

## A failed tag run publishes nothing

`release` needs both packaging jobs, so a failure anywhere leaves no GitHub
release and no assets — the tag is the only thing that moved. Fix it on main
and move the tag rather than spending a version number on a run that shipped
nothing:

```bash
gh release view vX.Y.Z       # confirm there is nothing to break: expect a miss
git push origin main
git tag -f vX.Y.Z
git push --force origin vX.Y.Z
```

Once a release does exist, that tag is other people's; ship the fix as the next
version instead.

## Local builds are verification, not artifacts

`scripts/build-appimage.sh` bundles a release AppImage from the local tree,
but the glibc floor follows the build host: an image built on a rolling
distro only runs on distros at least that new. Build locally to prove the
packaging works; ship only what CI built on 24.04. Host expectations (cargo,
GTK4 + libadwaita dev packages, ffmpeg, gifsicle, schemas, icon theme,
pixbuf loaders, rsvg, wget) are listed in the script header.
