//! The window. Chrome follows the architecture: the frame strip is the
//! scrubber, transport lives in the footer beside it, and the scope control
//! sits directly above the strip because in Range mode the strip selection is
//! its operand.

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use adw::prelude::*;
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::pango;
use gtk4 as gtk;
use libadwaita as adw;
use relm4::{Component, ComponentParts, ComponentSender, RelmWidgetExt};

use crate::core::fit::{self, Splice};
use crate::core::model::Rgba8;
use crate::core::render::{self, TextRasterizer};
use crate::core::snap::{self, Guides};
use crate::core::{
    Axis, Change, Document, Editor, Fade, FitMode, Frame, OverlayId, OverlayKind, Scope, Shape,
    ShapeOverlay, TextAlign, TextOverlay, Transform,
};
use crate::i18n::{fill, lookup, n, t, tn};
use crate::keymap::{Action, Chord, Keymap, MODALS, Modal, Mods};
use crate::pipeline::caps::Caps;
use crate::pipeline::gif::{Encodable, ExportSettings};
use crate::pipeline::video::{self, ImportOptions, ImportPlan, Trim, VideoInfo};
use crate::pipeline::{self as pipeline, gif as gif_pipeline, import_any};
use crate::settings::Settings;
use crate::ui::color::ColorPick;
use crate::ui::text::rasterize;

mod actions;
mod build;
mod canvas;
mod controls;
mod dialogs;
mod imports;
mod shortcuts;
mod strip;
#[cfg(test)]
mod tests;
mod update;
mod view;

use actions::*;
use build::*;
use canvas::*;
use controls::*;
use dialogs::*;
use imports::*;
use shortcuts::*;
use strip::*;

/// The box a thumbnail is fitted into (`core::model::THUMB_BOX`) — the widest
/// and tallest a strip cell can be at 1x, and the fallback cell width while
/// there is no document to read a real thumbnail size off.
const THUMB_BOX: i32 = crate::core::model::THUMB_BOX as i32;
const THUMB_SPACING: i32 = 4;
/// Timeline-strip thumbnail zoom bounds and the per-step multiplier shared by
/// the Ctrl+wheel handler and the Ctrl+Up/Down shortcuts.
const STRIP_ZOOM_MIN: f64 = 0.2;
const STRIP_ZOOM_MAX: f64 = 3.0;
const STRIP_ZOOM_STEP: f64 = 1.2;
/// How long the settings have to sit still before a size measurement starts. A
/// measurement spawns ffmpeg and encodes for real, so it waits for the user to
/// stop moving rather than chasing every keystroke.
const MEASURE_DEBOUNCE: std::time::Duration = std::time::Duration::from_secs(5);
/// The ceiling an import runs under when the user switches the memory limit
/// off for one file: an exabyte, which no decode can reach, chosen over
/// `usize::MAX` so the arithmetic that turns a budget into a frame cap has
/// room to multiply without wrapping.
const NO_IMPORT_LIMIT: usize = 1 << 60;
/// Smallest the window may be dragged to. The editor's own chrome needs
/// 465x210 — the header's buttons beside the import row — and the window with
/// the import dialog over it needs 458x104, both measured on the assembled
/// tree; libadwaita warns once per frame while a window is narrower than its
/// content and paints the toolbar clipped, so the floor is stated here and
/// handed to the compositor as a size request.
const MIN_WINDOW_W: i32 = 480;
const MIN_WINDOW_H: i32 = 400;
/// Below this width the header's menu bar folds into the ☰ menu: undo, the
/// five menus, the title, Export and ☰ side by side need it.
const MENUBAR_MIN_WINDOW_W: f64 = 720.0;
const BAND_H: f64 = 18.0;
/// "Splice at the end of the timeline", clamped to the frame count when the
/// import lands. The file menu's Add frames has no frame under the pointer to
/// aim at, unlike a frame's own context menu.
const AT_END: usize = usize::MAX;

pub const CSS: &str = "
.canvas-frame { border: 1px solid alpha(currentColor, 0.15); }
.checkerboard {
    background-color: @view_bg_color;
    background-image:
        linear-gradient(45deg, alpha(currentColor, .08) 25%, transparent 25%, transparent 75%, alpha(currentColor, .08) 75%),
        linear-gradient(45deg, alpha(currentColor, .08) 25%, transparent 25%, transparent 75%, alpha(currentColor, .08) 75%);
    background-size: 16px 16px;
    background-position: 0 0, 8px 8px;
}
.thumb { border-top: 3px solid transparent; padding-top: 2px; }
.thumb.in-scope { border-top-color: alpha(@accent_bg_color, 0.55); }
.thumb.playhead { border-top-color: @accent_bg_color; }
.thumb.selected { background: alpha(@accent_bg_color, 0.18); }
/* The drag-reorder divider: same accent as the playhead border, drawn as an
   Overlay child of the thumbnail so it paints on top of the picture. */
.drop-divider { background: @accent_bg_color; }
.tnum { font-feature-settings: 'tnum'; }
.bind-conflict { color: @error_color; }
/* A quick colour in the picker (`ui::color`) is its patch and nothing else:
   the theme's button padding would make a palette of sixteen twice as wide
   as the wheel above it. */
.color-swatch { padding: 1px; min-width: 0; min-height: 0; }
";

/// How many overlay rows the band area shows before it starts scrolling. Past
const BANDS_COLLAPSED_ROWS: usize = 5;
/// How many layers the sidebar's list shows before it starts scrolling, and
/// how few it keeps showing when the panel runs out of room. Past the first
/// the editor for the layer that is picked would be off the panel; below the
/// second the list is too small to pick anything out of, and the panel
/// around it scrolls instead.
const LAYER_ROWS_SHOWN: usize = 6;
const LAYER_ROWS_KEPT: usize = 3;
/// Grab radius for a resize handle, in widget pixels. Impasto inflates its
/// 4.5px grip by a 5px tolerance for the same reason: the dot is smaller than
/// anyone can reliably hit.
const HANDLE_PX: f64 = 9.5;
/// Impasto's grip: a blue dot with a white ring, drawn at a constant widget
/// size whatever the zoom (`Pinta.Tools/Handles/MoveHandle.cs`).
const HANDLE_R: f64 = 4.5;
const HANDLE_FILL: (f64, f64, f64) = (0.10, 0.30, 1.0);
/// Rotation constrains to this many steps of a full turn, as Impasto's
/// transform tool does.
const ROTATE_STEPS: f32 = 32.0;
/// How near, in widget pixels, a dragged overlay must come to a canvas edge
/// or centre line to snap onto it. Impasto's `CANVAS_GUIDE_TOLERANCE`.
const SNAP_PX: f64 = 8.0;
/// Impasto's rotate glyph (`resources/README.md`). GTK has no CSS cursor name
/// for rotation, so it travels as a texture rather than a name.
const ROTATE_CURSOR: &[u8] = include_bytes!("../../../resources/rotate-handle.png");
/// Left-rail tool icons (`resources/README.md`), compiled ahead of time with
/// `glib-compile-resources` rather than in a build script.
const TOOL_ICONS_GRESOURCE: &[u8] = include_bytes!("../../../resources/icons/icons.gresource");

/// Registers the bundled tool-icon gresource with the default display's icon
/// theme, so `gtk::Button::from_icon_name("tool-text-symbolic")` etc. resolve
/// like a stock Adwaita icon (recolored for theme/hover/insensitive state).
/// Called once from `App::init`, before any tool button is built.
fn register_tool_icons() {
    let bytes = glib::Bytes::from_static(TOOL_ICONS_GRESOURCE);
    let resource = gio::Resource::from_data(&bytes).expect("icons.gresource is well-formed");
    gio::resources_register(&resource);
    let display = gdk::Display::default().expect("a default display");
    gtk::IconTheme::for_display(&display).add_resource_path("/io/github/zbcoding/Gifkino/icons");
}

#[derive(Debug)]
pub enum Msg {
    Open,
    /// Probe first, then import; the user gets a say if the file is too big to
    /// come in whole.
    Load(PathBuf),
    /// Import at the settings the dialog came back with, under the memory
    /// budget it was allowed: the dialog can raise the configured one for a
    /// single import.
    LoadConfirmed(PathBuf, Box<ImportPlan>, usize),
    /// The user pressed the X beside the import progress bar.
    CancelImport,
    /// Debounced: measure the export size for this document revision, unless
    /// the document has moved on since.
    Estimate(u64),
    Export,
    /// The save dialog came back with a path: the snapshot is in hand, so
    /// claim the bar and encode off the main thread. Separate from `Export`
    /// so a cancelled dialog leaves no bar behind.
    BeginExport {
        path: PathBuf,
        frames: Vec<(image::RgbaImage, u16)>,
        settings: ExportSettings,
    },
    Undo,
    Redo,
    TogglePlay,
    Tick,
    Seek(usize),
    /// Right-click on a frame already inside the active selection: navigate
    /// to it without throwing the selection away, which a plain `Seek`
    /// (navigation *is* picking one frame) would do.
    SeekKeepSelection(usize),
    ExtendSelection(usize),
    SetScope(ScopeChoice),
    AddOverlay(Tool),
    SelectOverlay(Option<OverlayId>),
    /// Left click on a band in the strip: the overlay and its whole range
    /// (`App::select_band`); `None` is a click on empty band space.
    SelectBand(Option<OverlayId>),
    FrameOp(FrameOp),
    /// Put the frames in scope on the app's frame clipboard, changing
    /// nothing. Cutting is `FrameOp::Cut`, which copies and then deletes.
    FrameCopy,
    /// Splice the frame clipboard in after the frame on screen.
    FramePaste,
    /// Nudge every frame in scope one slot toward the start or the end,
    /// swapping the block with the one unselected frame beside it.
    MoveSelection {
        earlier: bool,
    },
    /// A drag-and-drop landing: `from` is the frame the drag started on, and
    /// `gap` is the divider position it was released at. The model widens
    /// `from` to the whole selection when it was dragged from inside one.
    MoveSelectionTo {
        from: usize,
        gap: usize,
    },
    /// The sidebar's delay spin button and the frame context menu's
    /// "Set delay…": applies to every frame the current scope names (just
    /// the playhead, for `ThisFrame`).
    SetScopeDelay(u16),
    /// Open the "set delay for all frames" dialog. Needs the live frame count
    /// and a default value, which only the model knows.
    DelayAllDialog,
    SetAllFramesDelay(u16),
    EditText(String),
    /// Every overlay property the sidebar can change, in one message: they all
    /// do the same thing to history and the view.
    SetOverlayProp(OverlayProp),
    /// Delete one named overlay — the X on its row in the layer list, which
    /// acts on the row it sits on rather than on whatever is selected.
    DeleteOverlay(OverlayId),
    /// The band menu's copy: put the selected overlay on frames it does not
    /// cover yet. `all` is every frame in the document, which is what the
    /// menu offers when no multi-frame selection is up; otherwise the frames
    /// in scope.
    CopyOverlayToFrames {
        all: bool,
    },
    /// Move one overlay a step up or down the z-order, past the overlay
    /// shown next to it in the layer list. `up` is toward the top of that
    /// list, which is the overlay painted last.
    RestackOverlay {
        id: OverlayId,
        up: bool,
    },
    DeleteSelection,
    SelectAllFrames,
    /// Ctrl+click: add or remove this one frame, wherever it is.
    ToggleSelection(usize),
    ToggleBandsExpanded,
    /// Canvas pointer, in image pixels. `scale` is how many widget pixels one
    /// image pixel currently occupies, which is what turns a grab radius into
    /// a distance the model can compare against.
    CanvasPress {
        x: f32,
        y: f32,
        scale: f32,
        state: gdk::ModifierType,
    },
    CanvasDrag {
        x: f32,
        y: f32,
        state: gdk::ModifierType,
    },
    /// Turn canvas-guide snapping for overlay drags on or off.
    ToggleSnap,
    CanvasRelease,
    ToggleCropTool,
    /// Esc: leave the crop tool if it is on, otherwise drop the overlay
    /// selection so the sidebar falls back to the plain frame view.
    Escape,
    ApplyCrop,
    /// Fill the canvas from the crop box on the frame on screen only.
    ApplyZoom,
    /// Fill the canvas from the crop box on every frame in the document.
    ApplyZoomAll,
    /// Open the crop-all dialog. The dialog needs the live canvas size, which
    /// only the model knows — action closures do not.
    CropAllDialog,
    /// Crop every frame to this box, the four dialog fields in pixels.
    CropAll(u32, u32, u32, u32),
    /// Keep the crop box and blank everything outside it, on every frame in
    /// scope. Unlike `ApplyZoom` the kept region is not scaled back up.
    ApplyShrink,
    DropEveryNth(usize),
    SmartDrop(usize),
    /// Open the resize dialog. Seeded from the live canvas size like
    /// `CropAllDialog` — action closures do not know it.
    ResizeDialog,
    Resize(u32, u32),
    /// Flip the frames in scope along `Axis`.
    Mirror(Axis),
    /// Ask which colour to fade the frames in scope from or to.
    FadeDialog(Fade),
    /// The fade dialog answered: ramp the frames in scope to `colour`.
    ApplyFade {
        fade: Fade,
        colour: Rgba8,
    },
    /// Ask how many frames the loop cross-fade should blend.
    LoopFadeDialog,
    /// Dissolve the last `.0` frames into the first `.0`, then drop those.
    ApplyLoopFade(usize),
    /// Pick a file to splice in at this index. The frame context menu and the
    /// file menu are the same operation at two different indices.
    ImportInto(usize),
    /// Like `Load`, but the decoded frames splice in at this index instead of
    /// replacing the document — `import_at` carries the target across the
    /// async decode.
    LoadInto(PathBuf, usize),
    /// The fit chooser answered. Carries the decoded frames rather than the
    /// path: they are already in memory, and re-decoding the file to answer a
    /// dialog would decode it twice.
    SpliceFrames {
        at: usize,
        frames: Vec<Frame>,
        fit: FitMode,
    },
    /// Reorder: pull the frame at `.0` out and reinsert it at `.1`.
    MoveFrame(usize, usize),
    /// A frame's own context menu: ask where to move it.
    MoveFrameDialog(usize),
    SetKeymap(Box<Keymap>),
    /// An edit landed: the toast offers Undo.
    Toast(String),
    /// Feedback for something that changed nothing undoable, so its toast
    /// carries no Undo button. See `update_with_view`.
    Notice(String),
    /// Scale the timeline strip's thumbnails. `.0` multiplies the current zoom;
    /// `0.0` is the reset sentinel back to 1×.
    StripZoom(f64),
}

impl Msg {
    /// Whether acting on this would reorder or remove frames. Frame work runs
    /// against the snapshot taken when it started and lands its results by
    /// frame index, so anything that moves an index has to wait until the
    /// worker is done. One-frame field edits are in here too: a delay changed
    /// mid-work would ride in the produced frame and be swapped back out.
    fn changes_frames(&self) -> bool {
        matches!(
            self,
            Msg::Load(_)
                | Msg::LoadConfirmed(..)
                | Msg::LoadInto(_, _)
                | Msg::Undo
                | Msg::Redo
                | Msg::DeleteSelection
                | Msg::FrameOp(_)
                | Msg::FramePaste
                | Msg::MoveSelection { .. }
                | Msg::MoveSelectionTo { .. }
                | Msg::SetScopeDelay(_)
                | Msg::SetAllFramesDelay(_)
                | Msg::ApplyCrop
                | Msg::CropAll(_, _, _, _)
                | Msg::ApplyZoom
                | Msg::ApplyZoomAll
                | Msg::ApplyShrink
                | Msg::DropEveryNth(_)
                | Msg::SmartDrop(_)
                | Msg::Resize(_, _)
                | Msg::Mirror(_)
                | Msg::ApplyFade { .. }
                | Msg::ApplyLoopFade(_)
                | Msg::SpliceFrames { .. }
                | Msg::MoveFrame(_, _)
        )
    }

    /// Operations whose result would be stale or whose follow-up message would
    /// be discarded while an import, resize, or zoom owns the document.
    fn requires_idle(&self) -> bool {
        self.changes_frames()
            || matches!(
                self,
                Msg::Open
                    | Msg::Export
                    | Msg::BeginExport { .. }
                    | Msg::CropAllDialog
                    | Msg::ResizeDialog
                    | Msg::DelayAllDialog
                    | Msg::ImportInto(_)
                    | Msg::FadeDialog(_)
                    | Msg::LoopFadeDialog
            )
    }

    fn edits_document(&self) -> bool {
        matches!(
            self,
            Msg::Undo
                | Msg::Redo
                | Msg::AddOverlay(_)
                | Msg::FrameOp(_)
                | Msg::FramePaste
                | Msg::DeleteOverlay(_)
                | Msg::CopyOverlayToFrames { .. }
                | Msg::RestackOverlay { .. }
                | Msg::MoveSelection { .. }
                | Msg::MoveSelectionTo { .. }
                | Msg::SetScopeDelay(_)
                | Msg::SetAllFramesDelay(_)
                | Msg::EditText(_)
                | Msg::SetOverlayProp(_)
                | Msg::DeleteSelection
                | Msg::CanvasPress { .. }
                | Msg::CanvasDrag { .. }
                | Msg::CanvasRelease
                | Msg::ToggleCropTool
                | Msg::ApplyCrop
                | Msg::ApplyZoom
                | Msg::ApplyZoomAll
                | Msg::ApplyShrink
                | Msg::CropAll(_, _, _, _)
                | Msg::DropEveryNth(_)
                | Msg::SmartDrop(_)
                | Msg::Resize(_, _)
                | Msg::Mirror(_)
                | Msg::ApplyFade { .. }
                | Msg::ApplyLoopFade(_)
                | Msg::SpliceFrames { .. }
                | Msg::MoveFrame(_, _)
        )
    }
}

/// One field of the selected overlay. Grouped so the update arm is one branch
/// rather than a dozen near-identical ones.
#[derive(Debug, Clone)]
pub enum OverlayProp {
    Font(String),
    TextSize(f32),
    Fill(Option<crate::core::model::Rgba8>),
    Stroke(Option<(crate::core::model::Rgba8, f32)>),
    Outline(Option<(crate::core::model::Rgba8, f32)>),
    Align(TextAlign),
    Antialias(bool),
}

impl OverlayProp {
    /// Whether applying this would actually change anything. Setting a sidebar
    /// widget fires its own notify handler, so selecting an overlay echoes
    /// every one of its properties straight back — and a no-op edit still costs
    /// an undo step.
    fn changes(&self, kind: &OverlayKind) -> bool {
        match (kind, self) {
            (OverlayKind::Text(t), OverlayProp::Font(f)) => &t.font != f,
            (OverlayKind::Text(t), OverlayProp::TextSize(px)) => (t.size_px - px).abs() > 0.01,
            (OverlayKind::Text(t), OverlayProp::Fill(Some(c))) => &t.color != c,
            (OverlayKind::Text(t), OverlayProp::Outline(o)) => &t.outline != o,
            (OverlayKind::Text(t), OverlayProp::Align(a)) => &t.align != a,
            (OverlayKind::Text(t), OverlayProp::Antialias(on)) => &t.antialias != on,
            (OverlayKind::Shape(s), OverlayProp::Fill(c)) => &s.fill != c,
            (OverlayKind::Shape(s), OverlayProp::Stroke(v)) => &s.stroke != v,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeChoice {
    ThisFrame,
    AllFrames,
    Range,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameOp {
    Delete,
    Duplicate,
    Reverse,
    /// Delete, but onto the frame clipboard first.
    Cut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Text,
    Rect,
    Ellipse,
    Arrow,
}

/// What a canvas drag is doing. Corner order is TL, TR, BL, BR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DragMode {
    Move,
    Resize(usize),
    Rotate,
    CropRect,
}

#[derive(Debug, Clone)]
struct Drag {
    mode: DragMode,
    /// Where the press landed, in image pixels.
    from: (f32, f32),
    /// How close, in image pixels, a dragged edge must come to a canvas guide
    /// to snap: a constant distance on screen, whatever the zoom.
    snap_reach: f32,
    /// The transform the drag started from, so every motion is computed against
    /// the original rather than accumulating rounding.
    origin: Transform,
    current: Transform,
    moved: bool,
}

impl Drag {
    /// A drag pressed at `from`, not yet moved off `origin`.
    fn start(mode: DragMode, from: (f32, f32), origin: Transform, snap_reach: f32) -> Self {
        Self {
            mode,
            from,
            snap_reach,
            origin,
            current: origin,
            moved: false,
        }
    }
}

/// Keep canvas gestures and a pending crop in the same image coordinate space
/// when an asynchronous resize lands in the middle of an interaction: scaled
/// by `(fx, fy)` and moved by `(dx, dy)`, the same as `Document::land` does
/// to the overlays.
fn scale_in_flight_canvas(
    drag: &mut Option<Drag>,
    crop_rect: &mut Option<(f32, f32, f32, f32)>,
    fx: f32,
    fy: f32,
    dx: f32,
    dy: f32,
) {
    if (fx, fy) == (1.0, 1.0) && (dx, dy) == (0.0, 0.0) {
        return;
    }
    if let Some((x, y, w, h)) = crop_rect {
        (*x, *y, *w, *h) = (*x * fx + dx, *y * fy + dy, *w * fx, *h * fy);
    }
    if let Some(drag) = drag {
        drag.from = (drag.from.0 * fx + dx, drag.from.1 * fy + dy);
        // Resampled pixels are a different size on screen; the reach is a
        // screen distance, so it follows them.
        drag.snap_reach *= fx.min(fy);
        for transform in [&mut drag.origin, &mut drag.current] {
            transform.x = transform.x * fx + dx;
            transform.y = transform.y * fy + dy;
            transform.w *= fx;
            transform.h *= fy;
        }
    }
}

/// What the one progress bar is showing, whichever long job holds the app.
/// The bar sits in the toolbar, so it is visible with a document open and not
/// only on the welcome page.
#[derive(Debug)]
struct Busy {
    kind: BusyKind,
    done: usize,
    /// Known when the job is countable; the import's is not until the
    /// container says how long it is.
    total: Option<usize>,
    /// Stop flag shared with the import worker, so the X next to the bar can
    /// end a decode between progress ticks. Only an import can be cancelled.
    cancel: Option<Arc<AtomicBool>>,
}

impl Busy {
    /// A job claiming the bar, with nothing done yet.
    fn new(kind: BusyKind, total: Option<usize>, cancel: Option<Arc<AtomicBool>>) -> Self {
        Self {
            kind,
            done: 0,
            total,
            cancel,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BusyKind {
    Import,
    Resize,
    Zoom,
    Shrink,
    Crop,
    /// Resampling a splice onto one canvas — the incoming frames, and every
    /// frame the document already had if the canvas grew to meet them.
    Fit,
    /// Writing the document out as a GIF. The worker encodes a snapshot, so
    /// document edits stay allowed while it runs — only a second job
    /// competing for the one bar waits.
    Export,
    /// A mirror, a fade or a loop cross-fade: pixel passes over the frames
    /// in scope, the canvas untouched.
    Effect,
}

/// A frame-heavy operation the worker thread runs against the document as it
/// stood when it was launched.
#[derive(Debug)]
enum FrameWork {
    Resize(u32, u32),
    Zoom {
        frames: Vec<usize>,
        rect: (u32, u32, u32, u32),
    },
    Shrink {
        frames: Vec<usize>,
        rect: (u32, u32, u32, u32),
    },
    Crop {
        rect: (u32, u32, u32, u32),
    },
    Mirror {
        frames: Vec<usize>,
        axis: Axis,
    },
    Fade {
        frames: Vec<usize>,
        fade: Fade,
        colour: Rgba8,
    },
    LoopFade {
        count: usize,
    },
}

impl FrameWork {
    /// The history label, marked: the toast translates it when it is shown.
    fn label(&self) -> &'static str {
        match self {
            FrameWork::Resize(..) => n("Resized"),
            FrameWork::Zoom { .. } => n("Zoomed"),
            // Translators: Past-tense edit name for cropping a frame in
            // place, without scaling the kept region back up to the canvas
            // size. "{change} on {count} frames" is appended when the scope
            // is more than one frame.
            FrameWork::Shrink { .. } => n("Cropped"),
            FrameWork::Crop { .. } => n("Cropped"),
            // Translators: History label after flipping frames.
            FrameWork::Mirror { .. } => n("Frames mirrored"),
            FrameWork::Fade { fade, .. } => match fade {
                // Translators: History label after fading frames in from a solid color.
                Fade::In => n("Faded in"),
                // Translators: History label after fading frames out to a solid color.
                Fade::Out => n("Faded out"),
            },
            // Translators: History label after blending the animation's end into its start.
            FrameWork::LoopFade { .. } => n("Loop cross-faded"),
        }
    }

    /// The progress bar's kind: the canvas operations each have their own
    /// wording, the effects share one.
    fn busy_kind(&self) -> BusyKind {
        match self {
            FrameWork::Resize(..) => BusyKind::Resize,
            FrameWork::Zoom { .. } => BusyKind::Zoom,
            FrameWork::Shrink { .. } => BusyKind::Shrink,
            FrameWork::Crop { .. } => BusyKind::Crop,
            FrameWork::Mirror { .. } | FrameWork::Fade { .. } | FrameWork::LoopFade { .. } => {
                BusyKind::Effect
            }
        }
    }

    /// How many frames the edit will claim.
    fn touched(&self, doc: &Document) -> usize {
        match self {
            FrameWork::Resize(..) | FrameWork::Crop { .. } => doc.frames.len(),
            FrameWork::Zoom { frames, .. }
            | FrameWork::Shrink { frames, .. }
            | FrameWork::Mirror { frames, .. }
            | FrameWork::Fade { frames, .. } => frames.len(),
            FrameWork::LoopFade { count } => (*count).min(doc.loop_fade_limit()),
        }
    }

    /// How overlays scale when the work lands. A zoom keeps the canvas, so it
    /// is `(1.0, 1.0)` and nothing moves.
    fn scale(&self, doc: &Document) -> (f32, f32) {
        match self {
            FrameWork::Resize(w, h) => {
                let (cw, ch) = doc.size();
                (*w as f32 / cw.max(1) as f32, *h as f32 / ch.max(1) as f32)
            }
            _ => (1.0, 1.0),
        }
    }

    /// How overlays move when the work lands, on top of any scale. Only a
    /// crop moves the origin — the frame it kept starts somewhere other than
    /// `(0, 0)`, so every overlay has to follow it there.
    fn offset(&self) -> (f32, f32) {
        match self {
            FrameWork::Crop { rect: (x, y, _, _) } => (-(*x as f32), -(*y as f32)),
            _ => (0.0, 0.0),
        }
    }

    /// How the produced frames go into the document. Most work lands them
    /// with the overlays following the canvas; a loop cross-fade keeps the
    /// canvas and also drops the head it dissolved into.
    fn landing(&self) -> Landing {
        match self {
            FrameWork::LoopFade { .. } => |doc, tail, _, _| doc.apply_loop_fade(tail),
            _ => Document::land,
        }
    }
}

/// Puts a finished frame job's frames into the document, given the scale
/// and offset its overlays follow.
type Landing = fn(&mut Document, Vec<(usize, Frame)>, (f32, f32), (f32, f32));

/// What a finished frame job hands back: the produced frames as
/// `(index, frame)` pairs, plus what the edit needs to say and to scale.
#[derive(Debug)]
pub struct WorkDone {
    label: &'static str,
    frames_touched: usize,
    scale: (f32, f32),
    offset: (f32, f32),
    frames: Vec<(usize, Frame)>,
    land: Landing,
}

/// How an import ended. A cancelled one is not a failure: the user asked for
/// it, so nothing is toasted and whatever decoded is dropped.
#[derive(Debug)]
pub enum ImportOutcome {
    Loaded(PathBuf, Vec<Frame>),
    Cancelled,
    Failed(String),
}

#[derive(Debug)]
pub enum Cmd {
    /// Frames decoded so far, and the estimate to expect.
    ImportProgress(usize, Option<usize>),
    /// Pre-flight probe: what importing this file would cost.
    Planned(PathBuf, Box<Result<ImportPlan, String>>),
    /// Frame work done so far, and the total it is working through.
    WorkProgress(usize, usize),
    /// Measured export size, and the revision it was measured at.
    Estimated(u64, Result<usize, String>),
    /// A finished resize or zoom, or a worker panic reported back to the UI.
    Worked(Result<Box<WorkDone>, &'static str>),
    /// A finished splice: incoming frames and, when the canvas grew, the
    /// document's own, all resampled onto one canvas.
    Spliced(Result<Box<Splice>, &'static str>),
    Imported(Box<ImportOutcome>),
    Exported(Result<(PathBuf, u64), String>),
}

pub struct App {
    editor: Editor,
    caps: Caps,
    settings: Settings,
    path: Option<PathBuf>,
    playhead: usize,
    playing: bool,
    scope: ScopeChoice,
    /// The frames Ctrl+ and Shift+click have picked. A set, not a range: Ctrl
    /// takes one frame at a time and they need not touch.
    selection: Vec<usize>,
    /// Where the next Shift+click measures its run from.
    anchor: Option<usize>,
    selected_overlay: Option<OverlayId>,
    /// What the one progress bar is showing, if anything: an import, or frame
    /// work (a resize, a zoom) running off the main thread. The work is keyed
    /// to frame indices, so frame-moving messages wait while it runs.
    busy: Option<Busy>,
    /// Measured export size and the revision it describes. Kept on show while a
    /// newer one is being measured, because a number that blinks out on every
    /// keystroke is worse than one that is a moment stale.
    estimate: Option<(u64, usize)>,
    /// Revision an estimate is already queued for.
    estimate_pending: Option<u64>,
    /// Shared with the key controller, which cannot borrow the model.
    keymap: Rc<RefCell<Keymap>>,
    /// In-flight canvas drag, if any.
    drag: Option<Drag>,
    /// Whether overlay drags pull onto the canvas edges and centre lines.
    snap: bool,
    /// The guides the drag in flight is resting on, drawn while it lasts.
    guides: Guides,
    /// Crop tool armed, and the rect it has been given, in image pixels.
    crop_tool: bool,
    crop_rect: Option<(f32, f32, f32, f32)>,
    bands_expanded: bool,
    /// Frame keys the strip was last built for. Keyed on the frames rather than
    /// on `rev` because `rev` also moves for overlay edits, and rebuilding a
    /// few hundred thumbnails on every keystroke in the text entry is a freeze.
    strip_keys: RefCell<Vec<(u64, bool)>>,
    /// Timeline-strip thumbnail zoom.
    strip_zoom: Rc<Cell<f64>>,
    /// Distance from one strip cell to the next at the current zoom and
    /// thumbnail size (`cell_pitch`), which is where the bands under the
    /// strip put their per-frame columns. Shared with the bands
    /// `DrawingArea`'s draw and click closures, which cannot borrow the
    /// model; `update_view` keeps it current.
    strip_pitch: Rc<Cell<f64>>,
    /// Zoom the strip's pictures were last sized to, so `update_view` only walks
    /// the thumbnails when it actually changed.
    strip_zoom_shown: Cell<f64>,
    /// `canvas_key` the canvas picture was last composited for, so
    /// `update_view` only recomposites when the picture would change.
    canvas_shown: Cell<Option<(u64, usize, bool)>>,
    rev: u64,
    /// Set right before a `Load`/`LoadInto` starts and consumed when
    /// `Cmd::Imported` lands: where the decoded frames go. `None` replaces the
    /// document; `Some(i)` splices them in at `i`.
    import_at: Option<usize>,
    /// A memory budget the import dialog raised for one import, replacing
    /// `settings.max_import_bytes` until that import lands. The frames it
    /// decodes have to survive the splice too, so the landing check reads the
    /// same number the plan was measured against.
    import_budget: Option<usize>,
    /// Cut or copied frames, waiting for a paste. App-local rather than the
    /// system clipboard: what a paste needs is the frame with its delay and
    /// its cached thumbnail, and a GIF frame has no interchange format that
    /// carries either.
    clipboard: Vec<Frame>,
}

/// One row of the strip's layer list.
struct Band {
    id: OverlayId,
    name: String,
    range: Range<usize>,
    selected: bool,
    /// Packed row, not the overlay's index: see `pack_rows`.
    row: usize,
}

/// What the canvas overlay draws on top of the picture. Shared with the draw
/// function, which runs outside the update cycle.
#[derive(Default)]
struct CanvasState {
    image: (f32, f32),
    /// Canvas guides the drag in flight has snapped to.
    guides: Guides,
    selected: Option<Transform>,
    /// Every overlay on the current frame, so hovering one can promise a drag
    /// before it is the selected one.
    movable: Vec<Transform>,
    crop: Option<(f32, f32, f32, f32)>,
    /// Impasto's grip hint, built from the keymap so a rebind moves it.
    hint: String,
    /// The same for the body of an overlay, where a drag moves rather than
    /// resizes.
    move_hint: String,
    /// What "rotate" is bound to, so the hover cursor can say so before the
    /// drag starts.
    rotate: Mods,
}

pub struct Widgets {
    title: adw::WindowTitle,
    stack: gtk::Stack,
    import_bar: gtk::ProgressBar,
    /// The circle X beside the bar; visible while an import runs, because a
    /// resize or zoom cannot be stopped part-way.
    import_cancel: gtk::Button,
    toasts: adw::ToastOverlay,
    canvas: gtk::Picture,
    canvas_frame: gtk::Frame,
    strip: gtk::Box,
    bands: gtk::DrawingArea,
    play: gtk::Button,
    time: gtk::Label,
    undo: gtk::Button,
    redo: gtk::Button,
    export: gtk::Button,
    actions: gio::SimpleActionGroup,
    scope_buttons: [gtk::ToggleButton; 3],
    properties: gtk::Box,
    text_entry: gtk::Entry,
    text_row: adw::ActionRow,
    /// The "Properties" group holding `text_row`. Past one frame in scope it
    /// shows only for a picked text overlay — see `frame_group`.
    text_group: adw::PreferencesGroup,
    /// Titled "Frame" for a single frame in scope, or a "N frames selected"
    /// summary once the scope names more than one.
    frame_group: adw::PreferencesGroup,
    frame_delay: gtk::SpinButton,
    /// Carries the same summary as `frame_group`'s title, since a spin
    /// button has no room for a heading of its own.
    delay_row: adw::ActionRow,
    overlay_list: gtk::ListBox,
    /// Caps the layer list's height at `LAYER_ROWS_SHOWN` rows, so a
    /// document with dozens of overlays scrolls the list rather than the
    /// whole panel. See `update_view`.
    overlay_list_scroll: gtk::ScrolledWindow,
    overlay_list_group: adw::PreferencesGroup,
    /// Overlay id for each `overlay_list` row, by position.
    overlay_list_ids: Rc<RefCell<Vec<OverlayId>>>,
    /// The frames the current scope names, mirrored for the strip's
    /// right-click handler: it has to pick the single-frame or the
    /// selection popover synchronously, before the model could answer.
    scope_mirror: Rc<RefCell<Vec<usize>>>,
    /// Each thumbnail's drag-reorder divider, in strip order, so the drop
    /// target can show and hide them as the pointer moves. Rebuilt with the
    /// strip; see `rebuild_strip`.
    drop_dividers: Rc<RefCell<Vec<gtk::Widget>>>,
    text_rows: Vec<gtk::Widget>,
    shape_rows: Vec<gtk::Widget>,
    overlay_group: adw::PreferencesGroup,
    font_button: gtk::FontDialogButton,
    text_size: gtk::SpinButton,
    fill_button: ColorPick,
    fill_on: gtk::Switch,
    outline_button: ColorPick,
    outline_width: gtk::SpinButton,
    align_buttons: Vec<(TextAlign, gtk::ToggleButton)>,
    antialias: gtk::Switch,
    stroke_button: ColorPick,
    stroke_width: gtk::SpinButton,
    crop_group: adw::PreferencesGroup,
    crop_label: gtk::Label,
    crop_button: gtk::ToggleButton,
    crop_apply: gtk::Button,
    zoom_apply: gtk::Button,
    shrink_apply: gtk::Button,
    snap_button: gtk::ToggleButton,
    tool_buttons: Vec<(Tool, gtk::Button)>,
    shape_button: adw::SplitButton,
    shape_tool: Rc<Cell<Tool>>,
    bands_scroll: gtk::ScrolledWindow,
    bands_expander: gtk::Button,
    canvas_overlay: gtk::DrawingArea,
    canvas_state: Rc<RefCell<CanvasState>>,
    doc_info: gtk::Label,
    /// Overlay geometry the band drawing reads, kept in sync with the model.
    bands_model: Rc<RefCell<Vec<Band>>>,
    /// Strip thumbnail textures, reused across strip rebuilds.
    thumb_textures: ThumbTextures,
    /// Set while `update_view` writes the sidebar from the model. See
    /// `connect_pair`.
    sync: Rc<Cell<bool>>,
}

impl App {
    fn text_fn(&self) -> TextRasterizer<'static> {
        &rasterize
    }

    /// The memory budget an import runs under: what the settings allow, or
    /// what the dialog raised it to for the import in flight.
    fn import_budget(&self) -> usize {
        self.import_budget.unwrap_or(self.settings.max_import_bytes)
    }

    /// Import options carrying that budget.
    fn import_options(&self) -> ImportOptions {
        ImportOptions {
            max_bytes: self.import_budget(),
            ..Default::default()
        }
    }

    fn frame_count(&self) -> usize {
        self.editor.doc.frames.len()
    }

    /// Frame index `i`, pulled back onto the last frame (0 with none).
    fn clamp_frame(&self, i: usize) -> usize {
        i.min(self.frame_count().saturating_sub(1))
    }

    fn scope(&self) -> Scope {
        match self.scope {
            ScopeChoice::ThisFrame => Scope::ThisFrame,
            ScopeChoice::AllFrames => Scope::AllFrames,
            ScopeChoice::Range if !self.selection.is_empty() => {
                Scope::Frames(self.selection.clone())
            }
            ScopeChoice::Range => Scope::ThisFrame,
        }
    }

    fn scope_frames(&self) -> Vec<usize> {
        self.scope().resolve(self.playhead, self.frame_count())
    }

    fn set_scope_delay(&mut self, cs: u16, sender: &ComponentSender<Self>) {
        let frames = self.scope_frames();
        if frames.is_empty() {
            return;
        }
        let total = self.frame_count();
        let touched = frames.len();
        let (change, _) = self
            .editor
            // Translators: Past-tense edit name, used inside "{change} on {count} frames".
            .edit(n("Delay set"), touched, |d| {
                d.set_delay_at(&frames, cs.max(1))
            });
        self.after_edit();
        self.toast_if_document_wide(sender, &change, total);
    }

    /// Overlays sitting on `frame`, in document order.
    fn overlays_on(&self, frame: usize) -> Vec<OverlayId> {
        self.editor
            .doc
            .overlays
            .iter()
            .filter(|o| o.range.contains(&frame))
            .map(|o| o.id)
            .collect()
    }

    /// The sidebar's layer list for the frame on screen: topmost overlay
    /// first. `doc.overlays` is bottom-to-top — list order *is* z-order —
    /// but a layer list reads the way the layers stack, so the row above
    /// another names the overlay painted over it.
    fn stacked_overlays(&self) -> Vec<(OverlayId, String)> {
        self.editor
            .doc
            .overlays
            .iter()
            .rev()
            .filter(|o| o.range.contains(&self.playhead))
            .map(|o| (o.id, o.name.clone()))
            .collect()
    }

    /// The overlay the sidebar edits: the selection, but only while it is on
    /// the frame on screen. Off its frame the sidebar drops to the plain frame
    /// view; returning to that frame brings the editor back.
    fn editing_overlay(&self) -> Option<OverlayId> {
        self.selected_overlay
            .filter(|id| self.overlays_on(self.playhead).contains(id))
    }

    /// Every frame when `all`, else the frames in scope. The panel's "Zoom
    /// and resize" and the band menu's "Copy overlay to selected frames"
    /// follow the scope; the Image menu's "Zoom and resize all frames" and
    /// "…to all frames" ignore it. Regression: the panel zoom used to
    /// hardcode `vec![self.playhead]`.
    fn frames_or_scope(&self, all: bool) -> Vec<usize> {
        if all {
            (0..self.frame_count()).collect()
        } else {
            self.scope_frames()
        }
    }

    /// The crop-and-keep-size work for a given box, or `None` if the box is
    /// off canvas. Which frames it touches follows the scope exactly, the
    /// same as `FrameOp` does: the frame on screen, a selection, or every
    /// frame - never hardcoded to just the frame on screen the way `ApplyZoom`
    /// intentionally is. Split out from the `Msg` handler so that contract has
    /// a regression test that does not need a live sender.
    fn shrink_work(&self, rect: (f32, f32, f32, f32)) -> Option<FrameWork> {
        let rect = normalize_canvas_rect(self.editor.doc.size(), rect)?;
        Some(FrameWork::Shrink {
            frames: self.scope_frames(),
            rect,
        })
    }

    /// The contiguous range an overlay would take. A gappy selection widens.
    fn scope_span(&self) -> Range<usize> {
        self.scope().span(self.playhead, self.frame_count())
    }

    /// The frames an edit to overlay `id` lands on now; see `overlay_edit_span`.
    fn edit_span(&self, id: OverlayId) -> Range<usize> {
        let range = self
            .editor
            .doc
            .overlay(id)
            .map_or(0..0, |o| o.range.clone());
        overlay_edit_span(range, self.scope_span(), self.playhead)
    }

    /// A band click: pick the overlay together with every frame it covers, so
    /// the canvas and the sidebar edit it whole instead of splitting off the
    /// frame on screen. The frames show as a strip selection, which is what
    /// says how far the edit will reach.
    fn select_band(&mut self, id: OverlayId) {
        let Some(range) = self.editor.doc.overlay(id).map(|o| o.range.clone()) else {
            return;
        };
        let count = self.frame_count();
        self.leave_crop();
        self.selected_overlay = Some(id);
        self.selection = range.clone().filter(|i| *i < count).collect();
        self.anchor = self.selection.first().copied();
        self.scope = ScopeChoice::Range;
        self.seek_to_overlay(id);
    }

    /// The canvas only shows the playhead frame, so an overlay being edited
    /// must sit on it: seek to its first frame rather than edit something
    /// unseen. Unlike a strip seek this is bookkeeping for the panel, not
    /// navigation — the selection and the scope stay as they are.
    fn seek_to_overlay(&mut self, id: OverlayId) {
        if let Some(o) = self.editor.doc.overlay(id)
            && !o.range.contains(&self.playhead)
        {
            self.playhead = self.clamp_frame(o.range.start);
        }
    }

    fn composite_playhead(&self) -> Option<gdk::Texture> {
        let img = render::composite(&self.editor.doc, self.playhead, self.text_fn())?;
        Some(texture_from(&img))
    }

    fn elapsed_cs(&self) -> u32 {
        self.editor.doc.frames[..self.playhead.min(self.frame_count())]
            .iter()
            .map(|f| f.delay_cs as u32)
            .sum()
    }

    fn schedule_tick(&self, sender: &ComponentSender<Self>) {
        let Some(frame) = self.editor.doc.frames.get(self.playhead) else {
            return;
        };
        let ms = (frame.delay_cs.max(1) as u64) * 10;
        let sender = sender.clone();
        // Rescheduled per frame using that frame's delay: a fixed tick with an
        // accumulator is more code and less correct.
        glib::timeout_add_local_once(std::time::Duration::from_millis(ms), move || {
            sender.input(Msg::Tick);
        });
    }
}
