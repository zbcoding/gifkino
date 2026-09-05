// The model and pipeline are built ahead of the chrome that drives them: the
// risky parts land first as tested logic (design.md, Build order).
#![allow(dead_code)]

mod core;
mod desktop;
mod i18n;
mod keymap;
mod pipeline;
mod settings;
mod ui;

use std::path::PathBuf;

use relm4::RelmApp;

const USAGE: &str = "\
Usage: gifkino [FILE]
       gifkino --install-desktop
       gifkino --uninstall-desktop

  FILE                 GIF or video to open.
  --install-desktop    Install the desktop entry and icons into your home, so
                       the desktop shows this application's name and icon. Only
                       for AppImage runs; an AppImage does this on first run
                       anyway, unless GIFKINO_NO_DESKTOP_INTEGRATION is set.
  --uninstall-desktop  Remove what --install-desktop wrote.
";

fn main() {
    let first = std::env::args_os().nth(1);
    match first.as_deref().and_then(|arg| arg.to_str()) {
        Some(flag @ ("--install-desktop" | "--uninstall-desktop")) => {
            if let Err(err) = desktop::run_command(flag == "--install-desktop") {
                eprintln!("gifkino: {err:#}");
                std::process::exit(1);
            }
            return;
        }
        Some("--help" | "-h") => {
            print!("{USAGE}");
            return;
        }
        _ => {}
    }

    // An AppImage installs nothing on the host, and without a desktop entry the
    // window has no name or icon as far as the desktop is concerned. See
    // desktop.rs for why the app cannot just hand the compositor its icon.
    if let Some(appimage) = desktop::appimage() {
        desktop::integrate(&appimage);
    }

    let path = first.map(PathBuf::from);
    // The path is ours to open, not GApplication's.
    let app = RelmApp::new(desktop::APP_ID).with_args(Vec::new());
    app.run::<ui::window::App>(path);
}
