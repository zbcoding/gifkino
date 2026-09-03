// The model and pipeline are built ahead of the chrome that drives them: the
// risky parts land first as tested logic (design.md, Build order).
#![allow(dead_code)]

mod core;
mod i18n;
mod keymap;
mod pipeline;
mod settings;
mod ui;

use std::path::PathBuf;

use relm4::RelmApp;

fn main() {
    let path = std::env::args_os().nth(1).map(PathBuf::from);
    // The path is ours to open, not GApplication's.
    let app = RelmApp::new("io.github.zbcoding.GifEditor").with_args(Vec::new());
    app.run::<ui::window::App>(path);
}
