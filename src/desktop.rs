//! Host desktop integration, for AppImage runs only.
//!
//! A taskbar picks the icon for a window by matching the window's app_id to an
//! installed desktop entry and reading that entry's `Icon=` key. A Wayland
//! client can instead hand its icon straight to the compositor over
//! xdg-toplevel-icon, but GTK only implements that from 4.20 and the AppImage
//! bundles Ubuntu 24.04's GTK 4.14, so that route is closed to it. Running an
//! AppImage installs nothing, and the compositor cannot read the icon inside
//! the AppDir - it is a different process with no idea the mount exists - so
//! without the steps below an AppImage window gets a generic placeholder.
//!
//! Measured on KDE Plasma 6.7 (KWin 6.7.4), with the client's own icon
//! suppressed so only the host state could matter:
//!
//! - nothing installed: placeholder.
//! - icon installed, no desktop entry: still a placeholder. Plasma does not
//!   look for a themed icon named after the app_id; the entry is what it
//!   matches on.
//! - entry and icon installed: the real icon, with the client sending no icon
//!   of its own. This is the case an AppImage has to reach.
//!
//! X11 sessions do not need any of this - GTK 4.14 sets `_NET_WM_ICON` from the
//! icon theme, which resolves inside the AppImage because AppRun puts its share
//! tree on `XDG_DATA_DIRS` - and neither does the flatpak, which exports the
//! entry and the icon to the host itself. Everything here is gated on
//! `$APPIMAGE`, so no other way of running the app is touched.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub const APP_ID: &str = "io.github.zbcoding.Gifkino";

/// Absolute path of the running `.AppImage`, exported by its runtime. Absent
/// for a flatpak, a distro package or a plain `cargo run`, and that absence is
/// what keeps this module out of their way.
pub fn appimage() -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var_os("APPIMAGE")?);
    path.is_absolute().then_some(path)
}

/// Where the AppImage runtime mounted the AppDir. The icons and the desktop
/// entry are copied out of here, because the mount is gone after the run.
fn appdir() -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var_os("APPDIR")?);
    path.is_absolute().then_some(path)
}

fn data_home() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
}

fn entry_path(data_home: &Path) -> PathBuf {
    data_home
        .join("applications")
        .join(format!("{APP_ID}.desktop"))
}

/// Install the entry and the icons, reporting what was written. Idempotent: a
/// second call overwrites the same paths.
pub fn install(appdir: &Path, appimage: &Path, data_home: &Path) -> Result<Vec<PathBuf>> {
    let template_path = appdir
        .join("usr/share/applications")
        .join(format!("{APP_ID}.desktop"));
    let template = std::fs::read_to_string(&template_path)
        .with_context(|| format!("reading {}", template_path.display()))?;

    let entry = entry_path(data_home);
    let dir = entry.parent().context("desktop entry has no parent")?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    std::fs::write(&entry, rewrite_entry(&template, appimage))
        .with_context(|| format!("writing {}", entry.display()))?;

    let mut written = vec![entry];
    written.extend(copy_icons(appdir, data_home)?);
    refresh(data_home);
    Ok(written)
}

/// Remove everything `install` wrote. Missing files are not an error: the point
/// is to end with none of them present.
pub fn uninstall(data_home: &Path) -> Result<Vec<PathBuf>> {
    let mut removed = Vec::new();
    let entry = entry_path(data_home);
    if std::fs::remove_file(&entry).is_ok() {
        removed.push(entry);
    }
    for (_, dest) in icon_pairs(&data_home.join("icons/hicolor"), data_home) {
        if std::fs::remove_file(&dest).is_ok() {
            removed.push(dest);
        }
    }
    refresh(data_home);
    Ok(removed)
}

/// Install on an AppImage run when the entry is missing, or when it still points
/// at an AppImage path that is no longer this one - the user renamed the file or
/// moved it to another directory, and a stale `Exec` is an entry that launches
/// nothing. Best effort by design: a read-only or full home is not a reason to
/// refuse to open a GIF, so a failure is reported and the app carries on.
///
/// Doing this in the application at all is a deviation from the AppImage
/// convention, where the image only carries the entry and an integrator -
/// AppImageLauncher, appimaged, a software centre - copies it onto the host.
/// The convention leaves a plain double-click with a placeholder icon, which is
/// the whole problem, so the first run covers for a missing integrator and then
/// gets out of the way of a present one: an entry an integrator already wrote
/// means this app has nothing to add, and adding one anyway would put the
/// application in the menu twice.
pub fn integrate(appimage: &Path) {
    if std::env::var_os("GIFKINO_NO_DESKTOP_INTEGRATION").is_some() {
        return;
    }
    let (Some(appdir), Some(data_home)) = (appdir(), data_home()) else {
        return;
    };
    let applications = data_home.join("applications");
    if integrator_owns_it(&applications) {
        return;
    }
    let entry = entry_path(&data_home);
    if let Ok(existing) = std::fs::read_to_string(&entry)
        && existing.contains(&appimage.to_string_lossy().to_string())
    {
        return;
    }
    match install(&appdir, appimage, &data_home) {
        Ok(_) => eprintln!(
            "gifkino: added {} to your applications, so the desktop can show \
             its name and icon. Undo with: {} --uninstall-desktop",
            entry.display(),
            appimage.display()
        ),
        Err(err) => eprintln!("gifkino: could not install the desktop entry: {err:#}"),
    }
}

/// Whether an AppImage integrator has already installed an entry for this
/// application. Both AppImageLauncher and appimaged name what they write
/// `appimagekit_<md5 of the path>-<entry name>`, and the md5 is why the name
/// cannot simply be matched in full: it changes with the AppImage's location.
fn integrator_owns_it(applications: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(applications) else {
        return false;
    };
    entries.flatten().any(|entry| {
        entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with("appimagekit") && name.contains(APP_ID))
    })
}

/// Point the entry at the AppImage. The packaged `Exec` reads
/// `gifkino @@ %f @@`, where the `@@` markers are flatpak's file-forwarding
/// syntax and mean nothing outside a flatpak - left in place they would reach
/// the app as literal arguments.
///
/// `Exec` and `TryExec` are not the same kind of value and must not be written
/// the same way. `Exec` is a command line, so a path containing a space has to
/// be quoted to stay one argument. `TryExec` is a bare path that the launcher
/// looks up as-is: quote it and the lookup fails, which makes the whole entry
/// invalid - GLib's `g_desktop_app_info_new` returns NULL for it - and an
/// invalid entry is one no taskbar will match, losing the icon that all of
/// this exists to fix. Measured, not guessed: the first version of this
/// function quoted both.
fn rewrite_entry(template: &str, appimage: &Path) -> String {
    let mut out = String::with_capacity(template.len() + 64);
    for line in template.lines() {
        if line.starts_with("Exec=") {
            out.push_str(&format!("Exec={} %f", quote(appimage)));
        } else if line.starts_with("TryExec=") {
            out.push_str(&format!("TryExec={}", appimage.display()));
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

/// Desktop-entry quoting: the reserved characters keep their literal meaning
/// only when escaped inside the quotes.
fn quote(path: &Path) -> String {
    let mut out = String::from("\"");
    for ch in path.to_string_lossy().chars() {
        if matches!(ch, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('"');
    out
}

fn copy_icons(appdir: &Path, data_home: &Path) -> Result<Vec<PathBuf>> {
    let source = appdir.join("usr/share/icons/hicolor");
    let mut written = Vec::new();
    for (src, dest) in icon_pairs(&source, data_home) {
        let dir = dest.parent().context("icon has no parent")?;
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        std::fs::copy(&src, &dest)
            .with_context(|| format!("copying {} to {}", src.display(), dest.display()))?;
        written.push(dest);
    }
    Ok(written)
}

/// Every `<size>/apps/<app id>.<ext>` under a hicolor tree, paired with where it
/// belongs in the target data directory. Both the rasterized sizes and the
/// scalable SVG travel: a desktop asking for 32px should not have to rasterize,
/// and one asking for a size nobody rasterized should still have the SVG.
fn icon_pairs(hicolor: &Path, data_home: &Path) -> Vec<(PathBuf, PathBuf)> {
    let Ok(sizes) = std::fs::read_dir(hicolor) else {
        return Vec::new();
    };
    let mut pairs = Vec::new();
    for size in sizes.flatten() {
        let apps = size.path().join("apps");
        let Ok(icons) = std::fs::read_dir(&apps) else {
            continue;
        };
        for icon in icons.flatten() {
            let name = icon.file_name();
            let Some(name) = name.to_str() else { continue };
            if !name.starts_with(APP_ID) || !icon.path().is_file() {
                continue;
            }
            let dest = data_home
                .join("icons/hicolor")
                .join(size.file_name())
                .join("apps")
                .join(name);
            pairs.push((icon.path(), dest));
        }
    }
    pairs.sort();
    pairs
}

/// GLib reads the mimeinfo/desktop cache rather than scanning the directory, so
/// a new entry is invisible to some launchers until this runs. KDE rebuilds its
/// own cache on a directory change and needs no prompting. Absent tools are
/// fine - the entry itself is already on disk.
fn refresh(data_home: &Path) {
    let _ = std::process::Command::new("update-desktop-database")
        .arg(data_home.join("applications"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// `--install-desktop` / `--uninstall-desktop`, for a user who would rather run
/// it themselves than have the first run do it.
pub fn run_command(install_it: bool) -> Result<()> {
    let data_home = data_home().context("neither XDG_DATA_HOME nor HOME is set")?;
    if !install_it {
        let removed = uninstall(&data_home)?;
        for path in &removed {
            println!("removed {}", path.display());
        }
        if removed.is_empty() {
            println!("nothing to remove under {}", data_home.display());
        }
        return Ok(());
    }

    let appimage = appimage().context(
        "not running from an AppImage: the flatpak and distro packages install \
         the desktop entry and icon themselves, and a development build is \
         covered by scripts/install-user.sh",
    )?;
    let appdir = appdir().context("APPDIR is not set, so there is nothing to copy from")?;
    for path in install(&appdir, &appimage, &data_home)? {
        println!("installed {}", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEMPLATE: &str = "[Desktop Entry]\n\
                            Type=Application\n\
                            Name=Gifkino\n\
                            Exec=gifkino @@ %f @@\n\
                            TryExec=gifkino\n\
                            Icon=io.github.zbcoding.Gifkino\n\
                            Categories=Graphics;\n";

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gifkino-desktop-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_entry_points_at_the_appimage_and_drops_the_flatpak_markers() {
        let entry = rewrite_entry(TEMPLATE, Path::new("/opt/My Apps/Gifkino.AppImage"));

        assert!(
            entry.contains("Exec=\"/opt/My Apps/Gifkino.AppImage\" %f"),
            "a path with a space has to survive as one argument: {entry}"
        );
        assert!(
            entry.contains("TryExec=/opt/My Apps/Gifkino.AppImage\n"),
            "TryExec is a bare path looked up as-is; quoting it makes the whole \
             entry invalid and the taskbar stops matching it: {entry}"
        );
        assert!(
            !entry.contains("@@"),
            "flatpak's file-forwarding markers mean nothing here: {entry}"
        );
        // Everything else is the packaged entry, verbatim.
        for line in [
            "Name=Gifkino",
            "Icon=io.github.zbcoding.Gifkino",
            "Categories=Graphics;",
        ] {
            assert!(entry.contains(line), "{line} went missing: {entry}");
        }
    }

    #[test]
    fn reserved_characters_are_escaped_in_exec_but_not_in_tryexec() {
        let entry = rewrite_entry(TEMPLATE, Path::new("/home/u/$(x)/a\"b/Gifkino.AppImage"));
        assert!(
            entry.contains("Exec=\"/home/u/\\$(x)/a\\\"b/Gifkino.AppImage\" %f"),
            "a command line has to escape what the launcher would interpret: {entry}"
        );
        assert!(
            entry.contains("TryExec=/home/u/$(x)/a\"b/Gifkino.AppImage\n"),
            "a bare path is compared literally, so escaping it breaks the lookup: {entry}"
        );
    }

    #[test]
    fn install_then_uninstall_leaves_nothing_behind() {
        let root = scratch("roundtrip");
        let appdir = root.join("AppDir");
        let data = root.join("data");

        std::fs::create_dir_all(appdir.join("usr/share/applications")).unwrap();
        std::fs::write(
            appdir
                .join("usr/share/applications")
                .join(format!("{APP_ID}.desktop")),
            TEMPLATE,
        )
        .unwrap();
        for (size, ext) in [("scalable", "svg"), ("128x128", "png"), ("48x48", "png")] {
            let dir = appdir
                .join("usr/share/icons/hicolor")
                .join(size)
                .join("apps");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(format!("{APP_ID}.{ext}")), b"icon").unwrap();
        }

        let appimage = root.join("Gifkino-x86_64.AppImage");
        let written = install(&appdir, &appimage, &data).unwrap();

        let entry = data.join("applications").join(format!("{APP_ID}.desktop"));
        assert!(entry.is_file(), "the entry is what the taskbar matches on");
        assert!(
            std::fs::read_to_string(&entry)
                .unwrap()
                .contains(&appimage.display().to_string())
        );
        for (size, ext) in [("scalable", "svg"), ("128x128", "png"), ("48x48", "png")] {
            let icon = data
                .join("icons/hicolor")
                .join(size)
                .join("apps")
                .join(format!("{APP_ID}.{ext}"));
            assert!(icon.is_file(), "{} was not installed", icon.display());
        }
        assert_eq!(written.len(), 4, "one entry and three icons: {written:?}");

        // A second install is an overwrite, not a duplicate or an error.
        assert_eq!(install(&appdir, &appimage, &data).unwrap().len(), 4);

        let removed = uninstall(&data).unwrap();
        assert_eq!(removed.len(), 4, "{removed:?}");
        assert!(!entry.exists());
        assert!(icon_pairs(&data.join("icons/hicolor"), &data).is_empty());

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn icons_belonging_to_other_apps_are_left_alone() {
        let root = scratch("foreign");
        let hicolor = root.join("icons/hicolor");
        let apps = hicolor.join("128x128/apps");
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::write(apps.join(format!("{APP_ID}.png")), b"ours").unwrap();
        std::fs::write(apps.join("org.example.Other.png"), b"theirs").unwrap();

        let pairs = icon_pairs(&hicolor, &root);
        assert_eq!(pairs.len(), 1, "{pairs:?}");
        assert!(pairs[0].0.ends_with(format!("{APP_ID}.png")));

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn an_entry_written_by_an_integrator_is_left_to_it() {
        let apps = scratch("integrator");
        assert!(
            !integrator_owns_it(&apps),
            "an empty applications directory owns nothing"
        );

        // Another AppImage's integrated entry says nothing about this one.
        std::fs::write(
            apps.join("appimagekit_abc123-org.example.Other.desktop"),
            "",
        )
        .unwrap();
        assert!(!integrator_owns_it(&apps));

        // The md5 in the middle varies with the AppImage's path, so only the
        // prefix and the app id can be matched.
        std::fs::write(
            apps.join(format!("appimagekit_deadbeef-{APP_ID}.desktop")),
            "",
        )
        .unwrap();
        assert!(integrator_owns_it(&apps));

        std::fs::remove_dir_all(&apps).unwrap();
    }
}
