//! Gifkino's document core: the model, its history, the frame-list and
//! overlay operations, compositing, and the GIF codec. Pure Rust over
//! `image::RgbaImage` with no file, process or thread access, so the desktop
//! app and the browser editor (through WebAssembly) run the same code.

pub mod fit;
pub mod gif;
pub mod handles;
pub mod history;
pub mod model;
pub mod ops;
pub mod render;
pub mod snap;

pub use fit::FitMode;
pub use history::{Change, Editor};
pub use model::{
    Document, Frame, OverlayId, OverlayKind, Scope, Shape, ShapeOverlay, TextAlign, TextOverlay,
    Transform,
};
pub use ops::{Axis, Fade};
