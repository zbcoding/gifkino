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

use crate::core::render::{self, TextRasterizer};
use crate::core::{
    Change, Document, Editor, Frame, OverlayId, OverlayKind, Scope, Shape, ShapeOverlay, TextAlign,
    TextOverlay, Transform,
};
use crate::i18n::{fill, lookup, n, t, tn};
use crate::keymap::{Action, Chord, Keymap, MODALS, Modal, Mods};
use crate::pipeline::caps::Caps;
use crate::pipeline::gif::{Encodable, ExportSettings};
use crate::pipeline::video::{self, ImportOptions, ImportPlan};
use crate::pipeline::{gif as gif_pipeline, import_any};
use crate::settings::Settings;
use crate::ui::text::rasterize;

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
const BAND_H: f64 = 18.0;

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
/// Impasto's rotate glyph (`resources/README.md`). GTK has no CSS cursor name
/// for rotation, so it travels as a texture rather than a name.
const ROTATE_CURSOR: &[u8] = include_bytes!("../../resources/rotate-handle.png");
/// Left-rail tool icons (`resources/README.md`), compiled ahead of time with
/// `glib-compile-resources` rather than in a build script.
const TOOL_ICONS_GRESOURCE: &[u8] = include_bytes!("../../resources/icons/icons.gresource");

/// Registers the bundled tool-icon gresource with the default display's icon
/// theme, so `gtk::Button::from_icon_name("tool-text-symbolic")` etc. resolve
/// like a stock Adwaita icon (recolored for theme/hover/insensitive state).
/// Called once from `App::init`, before any tool button is built.
fn register_tool_icons() {
    let bytes = glib::Bytes::from_static(TOOL_ICONS_GRESOURCE);
    let resource = gio::Resource::from_data(&bytes).expect("icons.gresource is well-formed");
    gio::resources_register(&resource);
    let display = gdk::Display::default().expect("a default display");
    gtk::IconTheme::for_display(&display).add_resource_path("/io/github/zbcoding/GifEditor/icons");
}

#[derive(Debug)]
pub enum Msg {
    Open,
    /// Probe first, then import; the user gets a say if the file is too big to
    /// come in whole.
    Load(PathBuf),
    /// Import at the settings the dialog came back with.
    LoadConfirmed(PathBuf, Box<ImportPlan>),
    /// The user pressed the X beside the import progress bar.
    CancelImport,
    /// Debounced: measure the export size for this document revision, unless
    /// the document has moved on since.
    Estimate(u64),
    Export,
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
    Resize(u32, u32),
    /// A frame's own context menu: splice a decoded image in right after it.
    InsertImageFrame(usize, PathBuf),
    /// A still image chosen from "Add frames from file": decode it and append
    /// one frame, fitted to the canvas. Videos and animations take the async
    /// `LoadAppend` path instead.
    AppendImageFrame(PathBuf),
    /// Pick another clip or image from "Add frames from file".
    ImportMore,
    /// Like `Load`, but appends the decoded frames instead of replacing the
    /// document — `import_append` carries the mode across the async decode.
    LoadAppend(PathBuf),
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
                | Msg::LoadConfirmed(_, _)
                | Msg::LoadAppend(_)
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
                | Msg::InsertImageFrame(_, _)
                | Msg::AppendImageFrame(_)
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
                    | Msg::CropAllDialog
                    | Msg::DelayAllDialog
                    | Msg::ImportMore
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
                | Msg::InsertImageFrame(_, _)
                | Msg::AppendImageFrame(_)
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
    /// The transform the drag started from, so every motion is computed against
    /// the original rather than accumulating rounding.
    origin: Transform,
    current: Transform,
    moved: bool,
}

/// Keep canvas gestures and a pending crop in the same image coordinate space
/// when an asynchronous resize lands in the middle of an interaction.
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
        (*x, *y, *w, *h) = (*x * fx - dx, *y * fy - dy, *w * fx, *h * fy);
    }
    if let Some(drag) = drag {
        drag.from = (drag.from.0 * fx - dx, drag.from.1 * fy - dy);
        for transform in [&mut drag.origin, &mut drag.current] {
            transform.x = transform.x * fx - dx;
            transform.y = transform.y * fy - dy;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BusyKind {
    Import,
    Resize,
    Zoom,
    Shrink,
    Crop,
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
        }
    }

    /// How many frames the edit will claim.
    fn touched(&self, doc: &Document) -> usize {
        match self {
            FrameWork::Resize(..) | FrameWork::Crop { .. } => doc.frames.len(),
            FrameWork::Zoom { frames, .. } | FrameWork::Shrink { frames, .. } => frames.len(),
        }
    }

    /// How overlays scale when the work lands. A zoom keeps the canvas, so it
    /// is `(1.0, 1.0)` and `scale_overlays` moves nothing.
    fn scale(&self, doc: &Document) -> (f32, f32) {
        match self {
            FrameWork::Resize(w, h) => {
                let (cw, ch) = doc.size();
                (*w as f32 / cw.max(1) as f32, *h as f32 / ch.max(1) as f32)
            }
            FrameWork::Zoom { .. } | FrameWork::Shrink { .. } | FrameWork::Crop { .. } => {
                (1.0, 1.0)
            }
        }
    }

    /// How overlays move when the work lands, on top of any scale. Only a
    /// crop moves the origin — the frame it kept starts somewhere other than
    /// `(0, 0)`, so every overlay has to follow it there.
    fn shift(&self) -> (f32, f32) {
        match self {
            FrameWork::Crop { rect: (x, y, _, _) } => (*x as f32, *y as f32),
            _ => (0.0, 0.0),
        }
    }
}

/// What a finished frame job hands back: the produced frames as
/// `(index, frame)` pairs, plus what the edit needs to say and to scale.
#[derive(Debug)]
pub struct WorkDone {
    label: &'static str,
    frames_touched: usize,
    scale: (f32, f32),
    shift: (f32, f32),
    frames: Vec<(usize, Frame)>,
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
    rev: u64,
    /// Set right before a `Load`/`LoadAppend` starts and consumed when
    /// `Cmd::Imported` lands: whether the decode should replace the document
    /// or append its frames to the end of the current one.
    import_append: bool,
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
    /// The "Properties" group holding `text_row`, hidden while the scope
    /// names more than one frame — see `frame_group`.
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
    fill_button: gtk::ColorDialogButton,
    fill_on: gtk::Switch,
    outline_button: gtk::ColorDialogButton,
    outline_width: gtk::SpinButton,
    align_buttons: Vec<(TextAlign, gtk::ToggleButton)>,
    antialias: gtk::Switch,
    stroke_button: gtk::ColorDialogButton,
    stroke_width: gtk::SpinButton,
    crop_group: adw::PreferencesGroup,
    crop_label: gtk::Label,
    crop_button: gtk::ToggleButton,
    crop_apply: gtk::Button,
    zoom_apply: gtk::Button,
    shrink_apply: gtk::Button,
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
    /// Set while `update_view` writes the sidebar from the model. See
    /// `connect_pair`.
    sync: Rc<Cell<bool>>,
}

impl App {
    fn text_fn(&self) -> TextRasterizer<'static> {
        &rasterize
    }

    /// Import options carrying the user's memory budget.
    fn import_options(&self) -> ImportOptions {
        ImportOptions {
            max_bytes: self.settings.max_import_bytes,
            ..Default::default()
        }
    }

    fn frame_count(&self) -> usize {
        self.editor.doc.frames.len()
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

    /// Frames a zoom touches. The panel's "Zoom and resize" follows the frame
    /// scope (`all` false); the Image menu's "Zoom and resize all frames"
    /// ignores it and takes the whole document (`all` true). Regression: the
    /// panel button used to hardcode `vec![self.playhead]`.
    fn zoom_frames(&self, all: bool) -> Vec<usize> {
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

    /// The canvas only shows the playhead frame, so an overlay being edited
    /// must sit on it: seek to its first frame rather than edit something
    /// unseen. Unlike a strip seek this is bookkeeping for the panel, not
    /// navigation — the selection and the scope stay as they are.
    fn seek_to_overlay(&mut self, id: OverlayId) {
        if let Some(o) = self.editor.doc.overlay(id)
            && !o.range.contains(&self.playhead)
        {
            self.playhead = o.range.start.min(self.frame_count().saturating_sub(1));
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

impl Component for App {
    type Init = Option<PathBuf>;
    type Input = Msg;
    type Output = ();
    type CommandOutput = Cmd;
    type Root = adw::ApplicationWindow;
    type Widgets = Widgets;

    fn init_root() -> Self::Root {
        adw::ApplicationWindow::builder()
            .title(t("GIF Editor"))
            .default_width(1100)
            .default_height(760)
            .build()
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        relm4::set_global_css(CSS);
        register_tool_icons();

        let model = App {
            editor: Editor::new(Document::default()),
            caps: Caps::probe(),
            settings: Settings::load(),
            path: None,
            playhead: 0,
            playing: false,
            scope: ScopeChoice::ThisFrame,
            selection: Vec::new(),
            anchor: None,
            selected_overlay: None,
            busy: None,
            estimate: None,
            estimate_pending: None,
            keymap: Rc::new(RefCell::new(Keymap::load())),
            drag: None,
            crop_tool: false,
            crop_rect: None,
            bands_expanded: false,
            strip_keys: RefCell::new(Vec::new()),
            strip_zoom: Rc::new(Cell::new(1.0)),
            strip_pitch: Rc::new(Cell::new(cell_pitch(THUMB_BOX, 1.0))),
            strip_zoom_shown: Cell::new(1.0),
            rev: 0,
            import_append: false,
            clipboard: Vec::new(),
        };

        let widgets = build(&root, &model, &sender);
        install_shortcuts(&root, &sender, model.keymap.clone());

        if let Some(path) = init {
            sender.input(Msg::Load(path));
        }

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Msg, sender: ComponentSender<Self>, root: &Self::Root) {
        let blocked = self.busy.as_ref().is_some_and(|busy| match busy.kind {
            BusyKind::Import => msg.requires_idle() || msg.edits_document(),
            BusyKind::Resize | BusyKind::Zoom | BusyKind::Shrink | BusyKind::Crop => {
                msg.requires_idle()
                    && !(matches!(msg, Msg::DeleteSelection) && self.selected_overlay.is_some())
            }
        });
        if blocked {
            return;
        }
        match msg {
            Msg::Open => {
                let dialog = gtk::FileDialog::builder().title(t("Open")).build();
                let filter = gtk::FileFilter::new();
                filter.set_name(Some(t("Videos and GIFs")));
                filter.add_mime_type("video/*");
                filter.add_mime_type("image/gif");
                let filters = gio::ListStore::new::<gtk::FileFilter>();
                filters.append(&filter);
                dialog.set_filters(Some(&filters));

                let sender = sender.clone();
                dialog.open(Some(root), gio::Cancellable::NONE, move |res| {
                    if let Some(path) = res.ok().and_then(|f| f.path()) {
                        sender.input(Msg::Load(path));
                    }
                });
            }
            Msg::Load(path) => {
                self.import_append = false;
                let cancel = Arc::new(AtomicBool::new(false));
                self.busy = Some(Busy {
                    kind: BusyKind::Import,
                    done: 0,
                    total: None,
                    cancel: Some(cancel.clone()),
                });
                plan_import(path, self.import_options(), cancel, &sender);
            }
            Msg::LoadAppend(path) => {
                if self.frame_count() == 0 {
                    // Nothing to append to: behave like a normal open.
                    sender.input(Msg::Load(path));
                    return;
                }
                self.import_append = true;
                let cancel = Arc::new(AtomicBool::new(false));
                self.busy = Some(Busy {
                    kind: BusyKind::Import,
                    done: 0,
                    total: None,
                    cancel: Some(cancel.clone()),
                });
                plan_import(path, self.import_options(), cancel, &sender);
            }
            Msg::ImportMore => {
                let dialog = gtk::FileDialog::builder()
                    .title(t("Add frames from a file"))
                    .build();
                let filter = gtk::FileFilter::new();
                filter.set_name(Some(t("Images, videos and GIFs")));
                filter.add_mime_type("image/*");
                filter.add_mime_type("video/*");
                let filters = gio::ListStore::new::<gtk::FileFilter>();
                filters.append(&filter);
                dialog.set_filters(Some(&filters));

                let sender = sender.clone();
                dialog.open(Some(root), gio::Cancellable::NONE, move |res| {
                    if let Some(path) = res.ok().and_then(|f| f.path()) {
                        // Still images splice in directly; anything that might
                        // carry motion goes through the async import pipeline.
                        sender.input(if is_still_image(&path) {
                            Msg::AppendImageFrame(path)
                        } else {
                            Msg::LoadAppend(path)
                        });
                    }
                });
            }
            Msg::LoadConfirmed(path, plan) => {
                let cancel = Arc::new(AtomicBool::new(false));
                self.busy = Some(Busy {
                    kind: BusyKind::Import,
                    done: 0,
                    total: None,
                    cancel: Some(cancel.clone()),
                });
                load(path, Some(*plan), self.import_options(), cancel, &sender);
            }
            Msg::CancelImport => {
                // The flag is all the UI does; the worker reports back through
                // `Cmd::Imported` and is the only thing that clears the bar.
                if let Some(cancel) = self.busy.as_ref().and_then(|busy| busy.cancel.as_ref()) {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
            Msg::Estimate(rev) => {
                if rev != self.rev || self.frame_count() == 0 {
                    return;
                }
                let settings = ExportSettings::default();
                // Composite on the main thread (Pango is not Send), encode off
                // it, exactly as the real export does.
                let sample = Encodable::sample_document(
                    &self.editor.doc,
                    &rasterize,
                    &settings,
                    gif_pipeline::ESTIMATE_SAMPLES,
                );
                let total = self.frame_count();
                sender.spawn_oneshot_command(move || {
                    let out = gif_pipeline::estimate_size(&sample, total, &settings)
                        .map_err(|e| e.to_string());
                    Cmd::Estimated(rev, out)
                });
            }
            Msg::Export => {
                if self.frame_count() == 0 {
                    return;
                }
                let dialog = gtk::FileDialog::builder().title(t("Export GIF")).build();
                dialog.set_initial_name(Some(
                    self.path
                        .as_ref()
                        .and_then(|p| p.file_stem())
                        .map(|s| format!("{}.gif", s.to_string_lossy()))
                        .unwrap_or_else(|| "export.gif".into())
                        .as_str(),
                ));
                // Composite on the main thread (Pango is not Send), encode off it.
                let settings = ExportSettings::default();
                let enc = Encodable::from_document(&self.editor.doc, &rasterize, &settings);
                let frames = enc.frames;
                let sender = sender.clone();
                dialog.save(Some(root), gio::Cancellable::NONE, move |res| {
                    let Some(path) = res.ok().and_then(|f| f.path()) else {
                        return;
                    };
                    let enc = Encodable { frames };
                    sender.spawn_oneshot_command(move || {
                        // bind so the closure captures the PathBuf, not `*path`
                        let path = path;
                        let out = gif_pipeline::export_path(&path, &enc, &settings)
                            .map(|size| (path, size))
                            .map_err(|e| e.to_string());
                        Cmd::Exported(out)
                    });
                });
            }
            Msg::Undo => {
                if self.editor.undo() {
                    self.after_edit();
                }
            }
            Msg::Redo => {
                if self.editor.redo() {
                    self.after_edit();
                }
            }
            Msg::TogglePlay => {
                self.playing = !self.playing && self.frame_count() > 1;
                if self.playing {
                    self.schedule_tick(&sender);
                }
            }
            Msg::Tick => {
                if self.playing && self.frame_count() > 0 {
                    self.playhead = (self.playhead + 1) % self.frame_count();
                    self.schedule_tick(&sender);
                }
            }
            Msg::Seek(i) => {
                self.playhead = i.min(self.frame_count().saturating_sub(1));
                self.selection.clear();
                self.anchor = Some(self.playhead);
                // A seek is navigation to one frame, which is also choosing
                // one frame to work on: the scope follows it whatever it was.
                // Sticky All frames past a click is how a one-frame drag
                // became a hundred-frame edit.
                self.scope = ScopeChoice::ThisFrame;
            }
            Msg::ExtendSelection(i) => {
                // Shift picks a run between the anchor and here, in either
                // direction, and re-picks from the same anchor if it is
                // shift-clicked again — so the anchor survives the message.
                let i = i.min(self.frame_count().saturating_sub(1));
                let anchor = self
                    .anchor
                    .unwrap_or(self.playhead)
                    .min(self.frame_count().saturating_sub(1));
                self.selection = run_between(anchor, i);
                self.anchor = Some(anchor);
                self.playhead = i;
                self.scope = ScopeChoice::Range;
            }
            Msg::ToggleSelection(i) => {
                // Ctrl adds or removes one frame anywhere, which is the whole
                // difference from Shift: the result need not be a run.
                let i = i.min(self.frame_count().saturating_sub(1));
                if self.selection.is_empty() {
                    self.selection.push(self.playhead);
                }
                toggle_frame(&mut self.selection, i);
                self.anchor = Some(i);
                self.playhead = i;
                self.scope = if self.selection.is_empty() {
                    ScopeChoice::ThisFrame
                } else {
                    ScopeChoice::Range
                };
            }
            Msg::SetScope(choice) => {
                if choice != ScopeChoice::Range || !self.selection.is_empty() {
                    self.scope = choice;
                }
            }
            Msg::AddOverlay(tool) => {
                if self.frame_count() == 0 {
                    return;
                }
                let frames = self.scope_frames();
                if frames.is_empty() {
                    return;
                }
                let (w, h) = self.editor.doc.size();
                let (kind, name, transform) = match tool {
                    Tool::Text => (
                        OverlayKind::Text(TextOverlay {
                            text: "Text".into(),
                            size_px: (h as f32 / 8.0).max(12.0),
                            ..Default::default()
                        }),
                        "Text",
                        Transform::at(
                            w as f32 * 0.1,
                            h as f32 * 0.72,
                            w as f32 * 0.8,
                            h as f32 * 0.2,
                        ),
                    ),
                    other => (
                        OverlayKind::Shape(ShapeOverlay {
                            shape: match other {
                                Tool::Rect => Shape::Rect,
                                Tool::Ellipse => Shape::Ellipse,
                                _ => Shape::Arrow,
                            },
                            fill: matches!(other, Tool::Arrow).then_some([255, 60, 60, 255]),
                            stroke: (!matches!(other, Tool::Arrow))
                                .then_some(([255, 60, 60, 255], 3.0)),
                        }),
                        match other {
                            Tool::Rect => "Rectangle",
                            Tool::Ellipse => "Ellipse",
                            _ => "Arrow",
                        },
                        Transform::at(
                            w as f32 * 0.25,
                            h as f32 * 0.25,
                            w as f32 * 0.5,
                            h as f32 * 0.5,
                        ),
                    ),
                };
                let touched = frames.len();
                let added = match tool {
                    Tool::Text => n("Text added"),
                    Tool::Rect => n("Rectangle added"),
                    Tool::Ellipse => n("Ellipse added"),
                    Tool::Arrow => n("Arrow added"),
                };
                let (change, ids) = self.editor.edit(added, touched, |d| {
                    d.add_overlay_over(name, kind, transform, &frames)
                });
                self.selected_overlay = ids
                    .iter()
                    .copied()
                    .find(|&id| {
                        self.editor
                            .doc
                            .overlay(id)
                            .is_some_and(|o| o.range.contains(&self.playhead))
                    })
                    .or(ids.first().copied());
                self.after_edit();
                self.toast_if_document_wide(&sender, &change, self.frame_count());
            }
            Msg::SelectOverlay(id) => {
                self.selected_overlay = id;
                if let Some(id) = id {
                    self.seek_to_overlay(id);
                }
            }
            Msg::EditText(text) => {
                let Some(id) = self.selected_overlay else {
                    return;
                };
                // The scope decides which frames the text change lands on; a
                // narrow scope splits the overlay so the rest keeps its text.
                let span = overlay_edit_span(
                    self.editor
                        .doc
                        .overlay(id)
                        .map_or(0..0, |o| o.range.clone()),
                    self.scope_span(),
                    self.playhead,
                );
                if span.is_empty() {
                    return;
                }
                let touched = span.len();
                let (_, (edited, _)) = self.editor.edit(n("Text edited"), touched, |d| {
                    d.edit_on_frames(id, span, |o| {
                        if let OverlayKind::Text(t) = &mut o.kind {
                            t.text = text;
                        }
                    })
                });
                self.selected_overlay = Some(edited);
                self.after_edit();
            }
            Msg::DeleteSelection => {
                if let Some(id) = self.selected_overlay.take() {
                    self.delete_overlay(id, &sender);
                } else if !self.selection.is_empty() {
                    let frames = std::mem::take(&mut self.selection);
                    let touched = frames.len();
                    let total = self.frame_count();
                    let (change, _) = self.editor.edit(n("Frames deleted"), touched, |d| {
                        d.delete_frames_at(&frames)
                    });
                    self.playhead = self.playhead.min(self.frame_count().saturating_sub(1));
                    self.scope = ScopeChoice::ThisFrame;
                    self.after_edit();
                    self.toast_if_document_wide(&sender, &change, total);
                }
            }
            Msg::DeleteOverlay(id) => {
                self.delete_overlay(id, &sender);
            }
            Msg::RestackOverlay { id, up } => self.restack_overlay(id, up),
            Msg::FrameCopy => {
                let frames = self.scope_frames();
                if frames.is_empty() {
                    return;
                }
                self.copy_frames(&frames);
                let copied = self.clipboard.len();
                sender.input(Msg::Notice(fill(
                    tn("{count} frame copied", "{count} frames copied", copied),
                    &[("count", &copied.to_string())],
                )));
            }
            Msg::FramePaste => {
                let total = self.frame_count();
                if let Some(change) = self.paste_frames() {
                    self.toast_if_document_wide(&sender, &change, total);
                }
            }
            Msg::FrameOp(op) => {
                let frames = self.scope_frames();
                self.run_frame_op(op, frames, &sender);
            }
            Msg::SeekKeepSelection(i) => {
                self.playhead = i.min(self.frame_count().saturating_sub(1));
            }
            Msg::MoveSelection { earlier } => {
                let picked = self.scope_frames();
                let Some(gap) = selection_nudge_gap(&picked, earlier, self.frame_count()) else {
                    return;
                };
                let to = move_target_for_set(&picked, gap);
                if let Some(change) = self.move_picked(&picked, to) {
                    self.toast_if_document_wide(&sender, &change, self.frame_count());
                }
            }
            Msg::MoveSelectionTo { from, gap } => {
                // A drag that started on a frame inside the active selection
                // moves the whole selection; anywhere else it is a plain
                // one-frame drag.
                let multi = self.selection.len() > 1 && self.selection.contains(&from);
                let picked: Vec<usize> = if multi {
                    self.selection.clone()
                } else {
                    vec![from]
                };
                let to = move_target_for_set(&picked, gap);
                if let Some(change) = self.move_picked(&picked, to) {
                    self.toast_if_document_wide(&sender, &change, self.frame_count());
                }
            }
            Msg::InsertImageFrame(index, path) => {
                let delay = self.editor.doc.frames.get(index).map_or(10, |f| f.delay_cs);
                let Some(frame) = self.decode_image_frame(&path, delay) else {
                    sender.input(Msg::Toast(t("Could not read that image.").into()));
                    return;
                };
                self.splice_frame((index + 1).min(self.frame_count()), frame, &sender);
            }
            Msg::AppendImageFrame(path) => {
                let delay = self.editor.doc.frames.last().map_or(10, |f| f.delay_cs);
                let Some(frame) = self.decode_image_frame(&path, delay) else {
                    sender.input(Msg::Toast(t("Could not read that image.").into()));
                    return;
                };
                self.splice_frame(self.frame_count(), frame, &sender);
            }
            Msg::MoveFrame(from, to) => {
                let total = self.frame_count();
                if from >= total || to >= total || from == to {
                    return;
                }
                let (change, _) = self
                    .editor
                    .edit(n("Frame moved"), 1, |d| d.move_frame(from, to));
                self.playhead = to;
                self.after_edit();
                self.toast_if_document_wide(&sender, &change, total);
            }
            Msg::MoveFrameDialog(i) => {
                if i >= self.frame_count() {
                    return;
                }
                move_frame_dialog(root, i, self.frame_count(), &sender);
            }
            Msg::SetScopeDelay(cs) => {
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
                self.toast_if_document_wide(&sender, &change, total);
            }
            Msg::DelayAllDialog => {
                if self.frame_count() == 0 {
                    return;
                }
                let default = self.editor.doc.frames[0].delay_cs;
                delay_all_dialog(root, self.frame_count(), default, &sender);
            }
            Msg::SetAllFramesDelay(cs) => {
                let total = self.frame_count();
                if total == 0 {
                    return;
                }
                let (change, _) = self
                    .editor
                    .edit(n("Delay set"), total, |d| d.set_delay(0..total, cs.max(1)));
                self.after_edit();
                self.toast_if_document_wide(&sender, &change, total);
            }
            Msg::SelectAllFrames => {
                if self.frame_count() > 0 {
                    self.selection.clear();
                    self.scope = ScopeChoice::AllFrames;
                }
            }
            Msg::ToggleBandsExpanded => self.bands_expanded = !self.bands_expanded,
            Msg::SetOverlayProp(prop) => {
                let Some(id) = self.selected_overlay else {
                    return;
                };
                let Some(overlay) = self.editor.doc.overlay(id) else {
                    return;
                };
                if !prop.changes(&overlay.kind) {
                    return;
                }
                self.seek_to_overlay(id);
                // The scope decides which frames the restyle lands on; a
                // narrow scope splits the overlay so the rest keeps its style.
                let span = overlay_edit_span(
                    self.editor
                        .doc
                        .overlay(id)
                        .map_or(0..0, |o| o.range.clone()),
                    self.scope_span(),
                    self.playhead,
                );
                if span.is_empty() {
                    return;
                }
                let touched = span.len();
                let (_, (edited, _)) = self.editor.edit(n("Overlay restyled"), touched, |d| {
                    d.edit_on_frames(id, span, |o| match (&mut o.kind, prop) {
                        (OverlayKind::Text(t), OverlayProp::Font(f)) => t.font = f,
                        (OverlayKind::Text(t), OverlayProp::TextSize(px)) => t.size_px = px,
                        (OverlayKind::Text(t), OverlayProp::Fill(Some(c))) => t.color = c,
                        (OverlayKind::Text(t), OverlayProp::Outline(o)) => t.outline = o,
                        (OverlayKind::Text(t), OverlayProp::Align(a)) => t.align = a,
                        (OverlayKind::Text(t), OverlayProp::Antialias(on)) => t.antialias = on,
                        (OverlayKind::Shape(s), OverlayProp::Fill(c)) => s.fill = c,
                        (OverlayKind::Shape(s), OverlayProp::Stroke(v)) => s.stroke = v,
                        _ => {}
                    })
                });
                self.selected_overlay = Some(edited);
                self.after_edit();
            }
            Msg::ToggleCropTool => {
                self.crop_tool = !self.crop_tool;
                self.crop_rect = None;
                if self.crop_tool {
                    self.selected_overlay = None;
                }
            }
            Msg::Escape => {
                if self.crop_tool {
                    self.crop_tool = false;
                    self.crop_rect = None;
                } else {
                    self.selected_overlay = None;
                }
            }
            Msg::CanvasPress { x, y, scale, state } => {
                if self.crop_tool {
                    self.crop_rect = Some((x, y, 0.0, 0.0));
                    self.drag = Some(Drag {
                        mode: DragMode::CropRect,
                        from: (x, y),
                        origin: Transform::at(x, y, 0.0, 0.0),
                        current: Transform::at(x, y, 0.0, 0.0),
                        moved: false,
                    });
                    return;
                }
                // A handle of the current selection wins over what is under the
                // pointer, or a small overlay could never be resized. The
                // rotate modifier outranks both, as it does in Impasto.
                let grab = (HANDLE_PX / scale.max(0.01) as f64) as f32;
                let rotating = self.keymap.borrow().mods(Modal::Rotate).held(state);
                let selected = self
                    .selected_overlay
                    .and_then(|id| self.editor.doc.overlay(id))
                    .filter(|o| !o.hidden && o.range.contains(&self.playhead))
                    .map(|o| o.transform);
                if let Some(transform) = selected {
                    let on_overlay =
                        contains(transform, x, y) || handle_at(transform, x, y, grab).is_some();
                    if rotating && on_overlay {
                        self.drag = Some(Drag {
                            mode: DragMode::Rotate,
                            from: (x, y),
                            origin: transform,
                            current: transform,
                            moved: false,
                        });
                        return;
                    }
                    if let Some(corner) = handle_at(transform, x, y, grab) {
                        self.drag = Some(Drag {
                            mode: DragMode::Resize(corner),
                            from: (x, y),
                            origin: transform,
                            current: transform,
                            moved: false,
                        });
                        return;
                    }
                }
                let hit = self
                    .editor
                    .doc
                    .overlays_on(self.playhead)
                    .filter(|o| contains(o.transform, x, y))
                    // Topmost: `overlays_on` yields bottom-to-top.
                    .last()
                    .map(|o| (o.id, o.transform));
                match hit {
                    Some((id, transform)) => {
                        self.selected_overlay = Some(id);
                        self.drag = Some(Drag {
                            mode: if rotating {
                                DragMode::Rotate
                            } else {
                                DragMode::Move
                            },
                            from: (x, y),
                            origin: transform,
                            current: transform,
                            moved: false,
                        });
                    }
                    None => self.selected_overlay = None,
                }
            }
            Msg::CanvasDrag { x, y, state } => {
                let keys = self.keymap.borrow();
                let (keep_aspect, from_center) = (
                    keys.mods(Modal::KeepAspect).held(state),
                    keys.mods(Modal::FromCenter).held(state),
                );
                drop(keys);
                let Some(drag) = &mut self.drag else { return };
                let (dx, dy) = (x - drag.from.0, y - drag.from.1);
                if dx.abs() < 0.5 && dy.abs() < 0.5 && !drag.moved {
                    return;
                }
                drag.moved = true;
                match drag.mode {
                    DragMode::CropRect => {
                        let (x0, y0) = drag.from;
                        self.crop_rect =
                            Some((x0.min(x), y0.min(y), (x - x0).abs(), (y - y0).abs()));
                        return;
                    }
                    DragMode::Move => {
                        drag.current = Transform {
                            x: drag.origin.x + dx,
                            y: drag.origin.y + dy,
                            ..drag.origin
                        };
                    }
                    DragMode::Resize(corner) => {
                        // Corner drags are simple only in the box's own frame,
                        // so both ends of the drag go through it first.
                        let (fx, fy) = drag.origin.to_local(drag.from.0, drag.from.1);
                        let (tx, ty) = drag.origin.to_local(x, y);
                        let resized = resize_corner(
                            drag.origin,
                            corner,
                            tx - fx,
                            ty - fy,
                            keep_aspect,
                            from_center,
                        );
                        drag.current = pin_anchor(drag.origin, resized, corner, from_center);
                    }
                    DragMode::Rotate => {
                        let (cx, cy) = drag.origin.center();
                        let from = (drag.from.1 - cy).atan2(drag.from.0 - cx);
                        let now = (y - cy).atan2(x - cx);
                        let mut angle = drag.origin.angle + (now - from);
                        if keep_aspect {
                            let step = std::f32::consts::TAU / ROTATE_STEPS;
                            angle = (angle / step).round() * step;
                        }
                        drag.current = Transform {
                            angle,
                            ..drag.origin
                        };
                    }
                }
                // Live, and deliberately outside the history: one drag is one
                // undo step, committed when the button comes up.
                let (id, current) = (self.selected_overlay, drag.current);
                if let Some(o) = id.and_then(|id| self.editor.doc.overlay_mut(id)) {
                    o.transform = current;
                }
                self.rev += 1;
            }
            Msg::CanvasRelease => {
                let Some(drag) = self.drag.take() else { return };
                if !drag.moved || drag.mode == DragMode::CropRect {
                    return;
                }
                let Some(id) = self.selected_overlay else {
                    return;
                };
                let (origin, final_t) = (drag.origin, drag.current);
                // Put the transform back before recording, so undo returns to
                // where the drag started rather than to one frame before it.
                if let Some(o) = self.editor.doc.overlay_mut(id) {
                    o.transform = origin;
                }
                // The scope decides which frames the drag commits to; a scope
                // narrower than the overlay's range splits it, so the rest of
                // the frames keep where it was.
                let span = overlay_edit_span(
                    self.editor
                        .doc
                        .overlay(id)
                        .map_or(0..0, |o| o.range.clone()),
                    self.scope_span(),
                    self.playhead,
                );
                if span.is_empty() {
                    return;
                }
                let label = match drag.mode {
                    DragMode::Move => n("Overlay moved"),
                    DragMode::Rotate => n("Overlay rotated"),
                    _ => n("Overlay resized"),
                };
                let (change, (edited, _)) = self.editor.edit(label, span.len(), |d| {
                    d.edit_on_frames(id, span, |o| o.transform = final_t)
                });
                self.selected_overlay = Some(edited);
                self.after_edit();
                self.toast_if_document_wide(&sender, &change, self.frame_count());
            }
            Msg::ApplyCrop => {
                let Some((x, y, w, h)) = self.crop_rect.take() else {
                    return;
                };
                let Some(rect) = normalize_canvas_rect(self.editor.doc.size(), (x, y, w, h)) else {
                    return;
                };
                self.start_crop(rect, &sender);
            }
            Msg::ApplyZoom => self.start_zoom(self.zoom_frames(false), &sender),
            Msg::ApplyZoomAll => self.start_zoom(self.zoom_frames(true), &sender),
            Msg::ApplyShrink => {
                let Some(rect) = self.crop_rect.take() else {
                    return;
                };
                let Some(work) = self.shrink_work(rect) else {
                    return;
                };
                self.crop_tool = false;
                self.busy = Some(Busy {
                    kind: BusyKind::Shrink,
                    done: 0,
                    total: Some(work.touched(&self.editor.doc)),
                    cancel: None,
                });
                run_frame_work(&self.editor.doc, work, &sender);
            }
            Msg::CropAllDialog => {
                let (cw, ch) = self.editor.doc.size();
                if cw == 0 || ch == 0 {
                    return;
                }
                crop_dialog(root, cw, ch, &sender);
            }
            Msg::CropAll(x, y, w, h) => {
                self.start_crop((x, y, w, h), &sender);
            }
            Msg::DropEveryNth(every) => {
                if self.frame_count() == 0 || every < 2 {
                    return;
                }
                let total = self.frame_count();
                let touched = total / every;
                let (change, _) = self
                    .editor
                    .edit(n("Frames removed"), touched, |d| d.drop_every_nth(every));
                self.playhead = 0;
                self.after_edit();
                self.toast_if_document_wide(&sender, &change, total);
            }
            Msg::SmartDrop(percent) => {
                let total = self.frame_count();
                let count = total * percent.min(95) / 100;
                if count == 0 {
                    return;
                }
                let (change, _) = self
                    .editor
                    .edit(n("Frames removed"), count, |d| d.drop_low_motion(count));
                self.playhead = 0;
                self.after_edit();
                self.toast_if_document_wide(&sender, &change, total);
            }
            Msg::Resize(w, h) => {
                let (w, h) = (w.max(1), h.max(1));
                if self.frame_count() == 0 || (w, h) == self.editor.doc.size() {
                    return;
                }
                // The import budget doubles as the resize-output budget: the
                // old frames stay resident in undo history either way, so the
                // peak equals the post-landing steady state.
                if !resize_fits_budget(w, h, self.frame_count(), self.settings.max_import_bytes) {
                    // Translators: Shown when resized RGBA frames would exceed the configured memory limit.
                    sender.input(Msg::Toast(
                        t("That resize would use more memory than the configured limit.").into(),
                    ));
                    return;
                }
                let work = FrameWork::Resize(w, h);
                self.busy = Some(Busy {
                    kind: BusyKind::Resize,
                    done: 0,
                    total: Some(self.frame_count()),
                    cancel: None,
                });
                run_frame_work(&self.editor.doc, work, &sender);
            }
            Msg::SetKeymap(map) => {
                map.save();
                *self.keymap.borrow_mut() = *map;
            }
            Msg::Toast(_) | Msg::Notice(_) => {}
            Msg::StripZoom(factor) => {
                self.strip_zoom
                    .set(next_strip_zoom(self.strip_zoom.get(), factor));
            }
        }
    }

    fn update_cmd(&mut self, msg: Cmd, sender: ComponentSender<Self>, root: &Self::Root) {
        if let Cmd::ImportProgress(done, expected) = msg {
            if let Some(busy) = &mut self.busy {
                busy.done = done;
                busy.total = expected;
            }
            return;
        }
        if let Cmd::WorkProgress(done, total) = msg {
            // The worker keeps counting under whatever bar is up; only the
            // numbers move, the bar never clears here.
            if let Some(busy) = &mut self.busy {
                busy.done = done;
                busy.total = Some(total);
            }
            return;
        }
        if let Cmd::Worked(result) = msg {
            self.busy = None;
            let done = match result {
                Ok(done) => done,
                Err(error) => {
                    sender.input(Msg::Toast(t(error).to_string()));
                    return;
                }
            };
            let WorkDone {
                label,
                frames_touched,
                scale,
                shift,
                frames,
            } = *done;
            let (fx, fy) = scale;
            let (dx, dy) = shift;
            let (change, _) = self.editor.edit(label, frames_touched, |d| {
                for (i, frame) in frames {
                    d.frames[i] = frame;
                }
                d.scale_overlays(fx, fy);
                for o in &mut d.overlays {
                    o.transform.x -= dx;
                    o.transform.y -= dy;
                }
            });
            scale_in_flight_canvas(&mut self.drag, &mut self.crop_rect, fx, fy, dx, dy);
            self.after_edit();
            self.toast_if_document_wide(&sender, &change, self.frame_count());
            return;
        }
        if let Cmd::Estimated(rev, result) = msg {
            if let Ok(bytes) = result {
                self.estimate = Some((rev, bytes));
            }
            if self.estimate_pending == Some(rev) {
                self.estimate_pending = None;
            }
            return;
        }
        // The probe answered; either go straight on or put the cost to the user.
        if let Cmd::Planned(path, plan) = msg {
            match *plan {
                Ok(plan) if plan.is_reduced() => {
                    self.busy = None;
                    confirm_oversize(root, &path, &plan, self.settings.max_import_bytes, &sender);
                }
                // The probe never checks the flag, so a click during the probe
                // still stands: this is the same flag the load was seeded with.
                Ok(plan) => {
                    let cancel = self
                        .busy
                        .as_ref()
                        .and_then(|busy| busy.cancel.clone())
                        .expect("import in flight");
                    load(path, Some(plan), self.import_options(), cancel, &sender);
                }
                Err(e) => {
                    self.busy = None;
                    sender.input(Msg::Toast(e));
                }
            }
            return;
        }
        match msg {
            Cmd::ImportProgress(..)
            | Cmd::WorkProgress(..)
            | Cmd::Worked(..)
            | Cmd::Planned(..)
            | Cmd::Estimated(..) => {
                unreachable!("handled above")
            }
            Cmd::Imported(outcome) => {
                // An import ends the bar it started; an export never owns the
                // bar, so its result leaves whatever is running alone.
                self.busy = None;
                let append = std::mem::take(&mut self.import_append);
                match *outcome {
                    ImportOutcome::Loaded(path, frames) if append => {
                        if !frames.is_empty() {
                            let frames = resize_frames_to(frames, self.editor.doc.size());
                            let touched = frames.len();
                            let (change, _) = self.editor.edit(n("Frames added"), touched, |d| {
                                d.frames.extend(frames);
                            });
                            self.after_edit();
                            sender.input(Msg::Toast(change.message()));
                        }
                        let _ = path;
                    }
                    ImportOutcome::Loaded(path, frames) => {
                        self.editor = Editor::new(Document::from_frames(frames));
                        self.path = Some(path);
                        self.playhead = 0;
                        self.selection.clear();
                        self.selected_overlay = None;
                        self.scope = ScopeChoice::ThisFrame;
                        self.after_edit();
                    }
                    ImportOutcome::Cancelled => {}
                    ImportOutcome::Failed(e) => sender.input(Msg::Toast(e)),
                }
            }
            Cmd::Exported(Ok((path, size))) => {
                let kb = size as f64 / 1024.0;
                sender.input(Msg::Toast(fill(
                    t("Exported to {path} · {size} KB"),
                    &[
                        ("path", &path.display().to_string()),
                        ("size", &format!("{kb:.0}")),
                    ],
                )));
            }
            Cmd::Exported(Err(e)) => sender.input(Msg::Toast(e)),
        }
    }

    fn update_with_view(
        &mut self,
        widgets: &mut Widgets,
        msg: Msg,
        sender: ComponentSender<Self>,
        root: &Self::Root,
    ) {
        // A toast for an edit offers Undo; a notice is feedback for
        // something with nothing to undo — a copy — and must not, or its
        // button would undo whatever edit happened to come before it.
        let toast = match &msg {
            Msg::Toast(text) => Some((text.clone(), true)),
            Msg::Notice(text) => Some((text.clone(), false)),
            _ => None,
        };
        self.update(msg, sender.clone(), root);
        self.schedule_estimate(&sender);
        if let Some((text, undoable)) = toast {
            let toast = adw::Toast::new(&text);
            if undoable {
                toast.set_button_label(Some(t("Undo")));
                let s = sender.clone();
                toast.connect_button_clicked(move |_| s.input(Msg::Undo));
            }
            widgets.toasts.add_toast(toast);
        }
        self.update_view(widgets, sender);
    }

    fn update_view(&self, widgets: &mut Widgets, sender: ComponentSender<Self>) {
        // Everything below writes widgets from the model, and a widget that is
        // written fires the handler that would send the value back. See
        // `connect_pair`.
        widgets.sync.set(true);
        let count = self.frame_count();
        widgets
            .stack
            .set_visible_child_name(if count == 0 { "empty" } else { "editor" });

        widgets.import_bar.set_visible(self.busy.is_some());
        widgets
            .import_cancel
            .set_visible(self.busy.as_ref().map(|busy| busy.kind) == Some(BusyKind::Import));
        if let Some(busy) = &self.busy {
            let (done, expected) = (busy.done, busy.total);
            match expected.filter(|e| *e > 0) {
                Some(total) => {
                    widgets
                        .import_bar
                        .set_fraction((done as f64 / total as f64).min(1.0));
                    // Each label marks its own literal: the extractor picks
                    // msgids out of `t(...)` calls, not out of the match this
                    // arms.
                    let label = match &busy.kind {
                        BusyKind::Import => t("Importing… {done} / {total} frames"),
                        // Translators: Progress while every frame is being resized; both values are frame counts.
                        BusyKind::Resize => t("Resizing… {done} / {total} frames"),
                        // Translators: Progress while selected frames are being zoomed; both values are frame counts.
                        BusyKind::Zoom => t("Zooming… {done} / {total} frames"),
                        // Translators: Progress while this frame's crop is being applied.
                        BusyKind::Shrink => t("Cropping this frame…"),
                        // Translators: Progress while every frame is being cropped; both values are frame counts.
                        BusyKind::Crop => t("Cropping… {done} / {total} frames"),
                    };
                    widgets.import_bar.set_text(Some(&fill(
                        label,
                        &[("done", &done.to_string()), ("total", &total.to_string())],
                    )));
                }
                // Only the import can lack a total: a container with no
                // duration has nothing to be a fraction of. The frame works
                // always know theirs.
                None => {
                    widgets.import_bar.pulse();
                    widgets.import_bar.set_text(Some(&fill(
                        t("Importing… {done} frames"),
                        &[("done", &done.to_string())],
                    )));
                }
            }
        }

        widgets.title.set_title(
            &self
                .path
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().to_string())
                // Translators: Window title when no file has been opened.
                .unwrap_or_else(|| t("Untitled").into()),
        );
        widgets.title.set_subtitle(&fill(
            t("{count} frames · {seconds} s"),
            &[
                ("count", &count.to_string()),
                (
                    "seconds",
                    &format!("{:.1}", self.editor.doc.duration_cs() as f32 / 100.0),
                ),
            ],
        ));

        // Tooltips are built from the keymap, never typed, so a rebind moves
        // every hint with it.
        let keys = self.keymap.borrow();
        // Undo and redo would reorder frames, which the busy guard drops —
        // grey the buttons instead of leaving clicks that do nothing.
        widgets
            .undo
            .set_sensitive(self.busy.is_none() && self.editor.can_undo());
        widgets
            .redo
            .set_sensitive(self.busy.is_none() && self.editor.can_redo());
        // Translators: {change} is the name of the edit, e.g. "Frames deleted".
        let named = |template: &'static str, label: Option<&str>| match label {
            Some(label) => fill(t(template), &[("change", &lookup(label))]),
            None => t(template
                .split_once(" {change}")
                .map_or(template, |(head, _)| head))
            .into(),
        };
        widgets.undo.set_tooltip_text(Some(&keys.tip(
            &named("Undo {change}", self.editor.undo_label()),
            Action::Undo,
        )));
        widgets.redo.set_tooltip_text(Some(&keys.tip(
            &named("Redo {change}", self.editor.redo_label()),
            Action::Redo,
        )));
        widgets
            .export
            .set_tooltip_text(Some(&keys.tip(t("Export GIF"), Action::Export)));
        widgets
            .play
            .set_tooltip_text(Some(&keys.tip(t("Play/pause"), Action::PlayPause)));
        for (tool, button) in &widgets.tool_buttons {
            button.set_tooltip_text(Some(&keys.tip(t(tool_label(*tool)), tool_action(*tool))));
        }
        {
            let shape = widgets.shape_tool.get();
            widgets
                .shape_button
                .set_tooltip_text(Some(&keys.tip(t(tool_label(shape)), tool_action(shape))));
        }
        widgets.crop_button.set_tooltip_text(Some(&keys.tip(
            t("Crop or zoom: drag a box on the canvas"),
            Action::ToolCrop,
        )));
        // The scope buttons are where multi-frame selection is discoverable at
        // all, so the hint lives on the tooltip rather than nowhere.
        widgets.scope_buttons[1].set_tooltip_text(Some(&format!(
            "{}\n{}",
            keys.tip(t("Every frame in the document"), Action::SelectAll),
            t("Shift+click the strip for a run of frames, Ctrl+click to add to it"),
        )));
        drop(keys);
        widgets
            .export
            .set_sensitive(count > 0 && self.busy.is_none());
        let idle = self.busy.is_none();
        for name in [
            "frame-delete",
            "frame-duplicate",
            "frame-reverse",
            "optimize-remove",
            "optimize-smart",
            "optimize-resize",
            "optimize-crop",
        ] {
            if let Some(action) = widgets
                .actions
                .lookup_action(name)
                .and_downcast::<gio::SimpleAction>()
            {
                action.set_enabled(idle && count > 0);
            }
        }
        if let Some(action) = widgets
            .actions
            .lookup_action("open")
            .and_downcast::<gio::SimpleAction>()
        {
            action.set_enabled(idle);
        }
        if let Some(action) = widgets
            .actions
            .lookup_action("export")
            .and_downcast::<gio::SimpleAction>()
        {
            action.set_enabled(idle && count > 0);
        }
        // The three buttons act on the drawn box; before one exists there is
        // nothing for them to do.
        let crop = self.crop_rect.filter(|(_, _, w, h)| *w >= 2.0 && *h >= 2.0);
        widgets.crop_apply.set_sensitive(idle && crop.is_some());
        widgets.zoom_apply.set_sensitive(idle && crop.is_some());
        widgets.shrink_apply.set_sensitive(idle && crop.is_some());
        if let Some(action) = widgets
            .actions
            .lookup_action("optimize-zoom-all")
            .and_downcast::<gio::SimpleAction>()
        {
            action.set_enabled(idle && crop.is_some());
        }
        let document_editing = !matches!(
            self.busy.as_ref().map(|busy| busy.kind),
            Some(BusyKind::Import)
        );
        widgets.properties.set_sensitive(document_editing);
        for name in [
            "insert-text",
            "insert-rect",
            "insert-ellipse",
            "insert-arrow",
        ] {
            if let Some(action) = widgets
                .actions
                .lookup_action(name)
                .and_downcast::<gio::SimpleAction>()
            {
                action.set_enabled(document_editing && count > 0);
            }
        }
        for (_, button) in &widgets.tool_buttons {
            button.set_sensitive(document_editing && count > 0);
        }
        widgets
            .shape_button
            .set_sensitive(document_editing && count > 0);
        widgets.play.set_icon_name(if self.playing {
            "media-playback-pause-symbolic"
        } else {
            "media-playback-start-symbolic"
        });
        widgets.time.set_label(&fill(
            t("{elapsed} / {total} · {fps} fps"),
            &[
                ("elapsed", &timecode(self.elapsed_cs())),
                ("total", &timecode(self.editor.doc.duration_cs())),
                ("fps", &fps_of(&self.editor.doc).to_string()),
            ],
        ));

        // Range only exists while there is a selection to bind it to.
        let picked = &self.selection;
        widgets.scope_buttons[2].set_visible(!picked.is_empty());
        if let (Some(first), Some(last)) = (picked.first(), picked.last()) {
            let contiguous = last - first + 1 == picked.len();
            widgets.scope_buttons[2].set_label(&if contiguous {
                fill(
                    // Translators: Scope button. Frame numbers are 1-based and inclusive.
                    t("Range {first}–{last}"),
                    &[
                        ("first", &(first + 1).to_string()),
                        ("last", &(last + 1).to_string()),
                    ],
                )
            } else {
                fill(
                    // Translators: Scope button when the picked frames do not form a run.
                    tn("{count} frame", "{count} frames", picked.len()),
                    &[("count", &picked.len().to_string())],
                )
            });
        }
        let active = match self.scope {
            ScopeChoice::ThisFrame => 0,
            ScopeChoice::AllFrames => 1,
            ScopeChoice::Range => 2,
        };
        for (i, button) in widgets.scope_buttons.iter().enumerate() {
            if button.is_active() != (i == active) {
                button.set_active(i == active);
            }
        }

        if let Some(texture) = self.composite_playhead() {
            widgets.canvas.set_paintable(Some(&texture));
        }
        let alpha = self.editor.doc.has_alpha();
        if alpha != widgets.canvas_frame.has_css_class("checkerboard") {
            if alpha {
                widgets.canvas_frame.add_css_class("checkerboard");
            } else {
                widgets.canvas_frame.remove_css_class("checkerboard");
            }
        }

        let keys = strip_keys(&self.editor.doc);
        let rebuilt = *self.strip_keys.borrow() != keys;
        if rebuilt {
            *self.strip_keys.borrow_mut() = keys;
            rebuild_strip(
                &widgets.strip,
                &self.editor.doc,
                &sender,
                &widgets.scope_mirror,
                &widgets.drop_dividers,
            );
        }
        let zoom = self.strip_zoom.get();
        // Whatever the frames' aspect made of the thumbnails is what a cell
        // measures out to, so the bands read their pitch off a real one.
        let thumb_w = self
            .editor
            .doc
            .frames
            .first()
            .map_or(THUMB_BOX, |frame| frame.thumb.width() as i32);
        self.strip_pitch.set(cell_pitch(thumb_w, zoom));
        if rebuilt || self.strip_zoom_shown.get() != zoom {
            self.strip_zoom_shown.set(zoom);
            for (cell, frame) in thumb_children(&widgets.strip).zip(&self.editor.doc.frames) {
                let Some(picture) = cell
                    .first_child()
                    .and_then(|overlay| overlay.first_child())
                    .and_downcast::<gtk::Picture>()
                else {
                    continue;
                };
                set_thumb_zoom(&picture, &frame.thumb, zoom);
            }
            widgets.bands.queue_draw();
        }
        let in_scope = self.scope_frames();
        *widgets.scope_mirror.borrow_mut() = in_scope.clone();
        for (i, thumb) in thumb_children(&widgets.strip).enumerate() {
            set_class(&thumb, "playhead", i == self.playhead);
            set_class(
                &thumb,
                "in-scope",
                in_scope.contains(&i) && i != self.playhead,
            );
            set_class(&thumb, "selected", self.selection.contains(&i));
        }

        let ranges: Vec<Range<usize>> = self
            .editor
            .doc
            .overlays
            .iter()
            .map(|o| o.range.clone())
            .collect();
        let rows = pack_rows(&ranges);
        *widgets.bands_model.borrow_mut() = self
            .editor
            .doc
            .overlays
            .iter()
            .zip(&rows)
            .map(|(o, row)| Band {
                id: o.id,
                name: o.name.clone(),
                range: o.range.clone(),
                selected: Some(o.id) == self.selected_overlay,
                row: *row,
            })
            .collect();
        let used_rows = rows.iter().copied().max().map_or(0, |r| r + 1);
        widgets
            .bands
            .set_content_height((used_rows as f64 * BAND_H) as i32);
        widgets
            .bands
            .set_content_width(strip_width(count, self.strip_pitch.get()));
        widgets.bands.queue_draw();

        // The overlay list costs the canvas whatever it takes, so it takes
        // nothing until there is an overlay, then grows a row at a time up to
        // `BANDS_COLLAPSED_ROWS` and scrolls behind an expander after that.
        let hidden = used_rows.saturating_sub(BANDS_COLLAPSED_ROWS);
        let shown = if self.bands_expanded {
            used_rows
        } else {
            used_rows.min(BANDS_COLLAPSED_ROWS)
        };
        widgets.bands_scroll.set_visible(used_rows > 0);
        widgets
            .bands_scroll
            .set_max_content_height((shown as f64 * BAND_H) as i32);
        widgets.bands_expander.set_visible(hidden > 0);
        widgets.bands_expander.set_label(&if self.bands_expanded {
            fill(
                t("▾ {count} overlay rows"),
                &[("count", &used_rows.to_string())],
            )
        } else {
            fill(
                // Translators: Expander under the filmstrip when overlay rows are collapsed.
                tn(
                    "▸ {count} more overlay row",
                    "▸ {count} more overlay rows",
                    hidden,
                ),
                &[("count", &hidden.to_string())],
            )
        });

        // Frame view: the delay of the frame(s) in scope, and a picker for
        // the overlays sitting on the frame on screen. `overlay_group` below
        // edits whichever row is picked. Once the scope names more than one
        // frame, delay still applies to every one of them (`SetScopeDelay`),
        // but the overlay picker and the overlay/text editors give way to a
        // summary: which overlay to show, or which overlay's properties to
        // edit, has no single answer across several frames at once.
        let multi = in_scope.len() > 1;
        widgets
            .frame_delay
            .set_sensitive(count > 0 && self.busy.is_none());
        if let Some(frame) = self.editor.doc.frames.get(self.playhead) {
            set_spin(&widgets.frame_delay, frame.delay_cs as f64);
        }
        widgets
            .frame_group
            .set_title(&frame_scope_summary(self.scope, &in_scope));
        widgets.delay_row.set_subtitle(&if multi {
            fill(
                // Translators: Shown under the delay field while multiple frames are selected; changing it applies to all of them at once.
                tn(
                    "Applies to {count} selected frame",
                    "Applies to {count} selected frames",
                    in_scope.len(),
                ),
                &[("count", &in_scope.len().to_string())],
            )
        } else {
            String::new()
        });
        // Topmost first, so the list reads the way the layers stack; see
        // `stacked_overlays`. `overlay_list_ids` holds that row order —
        // what the row-selected handler and each row's own buttons index by.
        let stacked = self.stacked_overlays();
        let ids: Vec<OverlayId> = stacked.iter().map(|(id, _)| *id).collect();
        if *widgets.overlay_list_ids.borrow() != ids {
            while let Some(child) = widgets.overlay_list.first_child() {
                widgets.overlay_list.remove(&child);
            }
            let bottom = ids.len().saturating_sub(1);
            for (row, (id, name)) in stacked.iter().enumerate() {
                widgets.overlay_list.append(&overlay_row(
                    *id,
                    name,
                    row > 0,
                    row < bottom,
                    &sender,
                ));
            }
            *widgets.overlay_list_ids.borrow_mut() = ids.clone();
            let (keep, cap) = layer_list_heights(&widgets.overlay_list);
            set_layer_list_heights(&widgets.overlay_list_scroll, keep, cap);
        }
        match self
            .selected_overlay
            .and_then(|sel| ids.iter().position(|id| *id == sel))
            .and_then(|i| widgets.overlay_list.row_at_index(i as i32))
        {
            Some(row) => {
                widgets.overlay_list.select_row(Some(&row));
                // The pick may have come from the canvas or a band in the
                // strip, naming a layer the list has scrolled past.
                show_row(&widgets.overlay_list_scroll, &widgets.overlay_list, &row);
            }
            None => widgets.overlay_list.unselect_all(),
        }
        widgets
            .overlay_list_group
            .set_visible(!multi && !self.crop_tool && !stacked.is_empty());
        widgets.text_group.set_visible(!multi);

        let (w, h) = self.editor.doc.size();
        // The overlay editor is part of the frame view: it shows only for an
        // overlay that is actually on the frame on screen. Selecting one on
        // another frame and navigating here leaves the plain frame view; going
        // back to its frame brings the editor back.
        let selected = self
            .editing_overlay()
            .and_then(|id| self.editor.doc.overlay(id));
        let kind = selected.map(|o| o.kind.clone());
        widgets.overlay_group.set_visible(kind.is_some() && !multi);
        let is_text = matches!(kind, Some(OverlayKind::Text(_)));
        let is_shape = matches!(kind, Some(OverlayKind::Shape(_)));
        for row in &widgets.text_rows {
            row.set_visible(is_text);
        }
        for row in &widgets.shape_rows {
            row.set_visible(is_shape);
        }
        widgets.text_row.set_visible(is_text);
        match &kind {
            Some(OverlayKind::Text(t)) => {
                if widgets.text_entry.text() != t.text {
                    widgets.text_entry.set_text(&t.text);
                }
                let want = pango::FontDescription::from_string(&t.font);
                if widgets.font_button.font_desc().as_ref() != Some(&want) {
                    widgets.font_button.set_font_desc(&want);
                }
                set_spin(&widgets.text_size, t.size_px as f64);
                set_color(&widgets.fill_button, t.color);
                widgets.fill_on.set_visible(false);
                set_color(
                    &widgets.outline_button,
                    t.outline.map_or([0, 0, 0, 255], |(c, _)| c),
                );
                set_spin(
                    &widgets.outline_width,
                    t.outline.map_or(0.0, |(_, w)| w as f64),
                );
                for (align, button) in &widgets.align_buttons {
                    button.set_active(*align == t.align);
                }
                widgets.antialias.set_active(t.antialias);
            }
            Some(OverlayKind::Shape(sh)) => {
                widgets.fill_on.set_visible(true);
                widgets.fill_on.set_active(sh.fill.is_some());
                set_color(&widgets.fill_button, sh.fill.unwrap_or([255, 60, 60, 255]));
                set_color(
                    &widgets.stroke_button,
                    sh.stroke.map_or([255, 60, 60, 255], |(c, _)| c),
                );
                set_spin(
                    &widgets.stroke_width,
                    sh.stroke.map_or(0.0, |(_, w)| w as f64),
                );
            }
            _ => {}
        }

        widgets.crop_button.set_active(self.crop_tool);
        widgets.crop_group.set_visible(self.crop_tool);
        widgets.crop_label.set_label(&match crop {
            Some((x, y, w, h)) => fill(
                t("{width} × {height} at {x}, {y}"),
                &[
                    ("width", &format!("{w:.0}")),
                    ("height", &format!("{h:.0}")),
                    ("x", &format!("{x:.0}")),
                    ("y", &format!("{y:.0}")),
                ],
            ),
            None => t("Drag a box on the canvas.").into(),
        });

        {
            let keys = self.keymap.borrow();
            let mut state = widgets.canvas_state.borrow_mut();
            state.image = (w as f32, h as f32);
            // Only box the selection when it actually paints on this frame;
            // stay selected for the sidebar otherwise. Matches `overlays_on`.
            state.selected = selected
                .filter(|o| !o.hidden && o.range.contains(&self.playhead))
                .map(|o| o.transform)
                .filter(|_| !self.crop_tool);
            state.movable = if self.crop_tool {
                Vec::new()
            } else {
                self.editor
                    .doc
                    .overlays_on(self.playhead)
                    .map(|o| o.transform)
                    .collect()
            };
            state.crop = self.crop_rect.filter(|_| self.crop_tool);
            state.rotate = keys.mods(Modal::Rotate);
            state.hint = grip_hint(&keys);
            state.move_hint = move_hint(&keys);
        }
        widgets.canvas_overlay.queue_draw();

        let estimate = match self.estimate {
            Some((rev, bytes)) if rev == self.rev => {
                format!(
                    "\n{}",
                    fill(t("≈ {size} as a GIF"), &[("size", &size(bytes))])
                )
            }
            Some((_, bytes)) => format!(
                "\n{}",
                fill(
                    t("≈ {size} as a GIF (updating…)"),
                    &[("size", &size(bytes))]
                )
            ),
            // Translators: Shown while the exported GIF size is still being measured.
            None if count > 0 => format!("\n{}", t("sizing…")),
            None => String::new(),
        };
        widgets.doc_info.set_label(&format!(
            "{}{estimate}",
            fill(
                t("{width} × {height} · {frames} frames · {overlays} overlays"),
                &[
                    ("width", &w.to_string()),
                    ("height", &h.to_string()),
                    ("frames", &count.to_string()),
                    ("overlays", &self.editor.doc.overlays.len().to_string()),
                ],
            )
        ));
        widgets.sync.set(false);
    }
}

impl App {
    /// Debounced so a run of keystrokes measures once, not once per character.
    fn schedule_estimate(&mut self, sender: &ComponentSender<Self>) {
        let current = self.rev;
        let measured = self.estimate.map(|(rev, _)| rev) == Some(current);
        if self.frame_count() == 0 || measured || self.estimate_pending == Some(current) {
            return;
        }
        self.estimate_pending = Some(current);
        let sender = sender.clone();
        glib::timeout_add_local_once(MEASURE_DEBOUNCE, move || {
            sender.input(Msg::Estimate(current));
        });
    }

    /// Move `picked` so the run lands at `to`, an index into the list with
    /// the picked frames already removed — what `move_target_for_set`
    /// produces from a divider gap. One history step; the selection follows
    /// the frames it named (they come out contiguous at their new home) and
    /// the playhead rides whatever frame it was on. `None` when nothing
    /// would move, so the caller can skip the toast.
    fn move_picked(&mut self, picked: &[usize], to: usize) -> Option<Change> {
        let mut picked: Vec<usize> = picked.to_vec();
        picked.sort_unstable();
        picked.dedup();
        if picked.is_empty() || picked.iter().enumerate().all(|(k, &p)| p == to + k) {
            return None;
        }
        let (change, _) = self.editor.edit(n("Frames moved"), picked.len(), |d| {
            d.move_frames_to(&picked, to)
        });
        if let Some(at) = picked.iter().position(|&p| p == self.playhead) {
            self.playhead = to + at;
        }
        if picked.len() > 1 {
            self.selection = (to..to + picked.len()).collect();
            self.anchor = Some(to);
            self.scope = ScopeChoice::Range;
        }
        self.after_edit();
        Some(change)
    }

    /// The frame operations, shared by the toolbar menu (which acts on the
    /// scope) and a frame's own context menu (which acts on that frame).
    fn run_frame_op(&mut self, op: FrameOp, frames: Vec<usize>, sender: &ComponentSender<Self>) {
        if self.frame_count() == 0 || frames.is_empty() {
            return;
        }
        let total = self.frame_count();
        let touched = frames.len();
        let (first, last) = (frames[0], frames[frames.len() - 1]);
        if op == FrameOp::Cut {
            self.copy_frames(&frames);
        }
        let (label, playhead) = match op {
            FrameOp::Delete => (n("Frames deleted"), first),
            FrameOp::Cut => (n("Frames cut"), first),
            FrameOp::Duplicate => (n("Frames duplicated"), self.playhead),
            FrameOp::Reverse => (n("Frames reversed"), self.playhead),
        };
        let (change, _) = self.editor.edit(label, touched, |d| match op {
            FrameOp::Delete | FrameOp::Cut => d.delete_frames_at(&frames),
            // back to front, so each insert leaves the rest of the
            // selection's indices alone
            FrameOp::Duplicate => {
                for i in frames.iter().rev() {
                    d.duplicate_frame(*i);
                }
            }
            // Reversing a set of frames that are not a run has no meaning
            // beyond reversing what they span.
            FrameOp::Reverse => d.reverse_frames(first..last + 1),
        });
        self.playhead = playhead;
        self.selection.clear();
        if self.scope == ScopeChoice::Range {
            self.scope = ScopeChoice::ThisFrame;
        }
        self.after_edit();
        self.toast_if_document_wide(sender, &change, total);
    }

    /// Take a copy of the named frames onto the clipboard, in strip order.
    /// `frames` must be sorted, as `scope_frames` returns it.
    fn copy_frames(&mut self, frames: &[usize]) {
        self.clipboard = frames
            .iter()
            .filter_map(|&i| self.editor.doc.frames.get(i).cloned())
            .collect();
    }

    /// Paste the clipboard in directly after the frame on screen, the way a
    /// duplicate lands beside its source, and leave the pasted run selected
    /// so a second edit acts on what just arrived. `None` when there is
    /// nothing on the clipboard, so the caller can skip the toast.
    fn paste_frames(&mut self) -> Option<Change> {
        if self.clipboard.is_empty() {
            return None;
        }
        let frames = self.clipboard.clone();
        let count = frames.len();
        let at = if self.frame_count() == 0 {
            0
        } else {
            self.playhead + 1
        };
        let (change, _) = self.editor.edit(n("Frames pasted"), count, |d| {
            d.insert_frames_at(at, frames)
        });
        // The playhead rides the *last* pasted frame, so pasting twice
        // stacks the runs rather than splitting the first one down the
        // middle at "after the frame on screen".
        self.playhead = at + count - 1;
        if count > 1 {
            self.selection = (at..at + count).collect();
            self.anchor = Some(at);
            self.scope = ScopeChoice::Range;
        } else {
            self.selection.clear();
        }
        self.after_edit();
        Some(change)
    }

    /// The overlay a z-order step lands next to: the one shown beside `id`
    /// in the layer list, which lists the overlays on the frame on screen.
    /// Not the neighbouring entry in `doc.overlays` — that one may sit on
    /// frames nowhere near here, and stepping past it would move nothing
    /// the user can see. `up` is toward the top of the list, which is the
    /// overlay painted last.
    fn restack_neighbour(&self, id: OverlayId, up: bool) -> Option<OverlayId> {
        let on_frame = self.overlays_on(self.playhead);
        let at = on_frame.iter().position(|o| *o == id)?;
        if up {
            on_frame.get(at + 1).copied()
        } else {
            on_frame.get(at.checked_sub(1)?).copied()
        }
    }

    /// Move one overlay a step through the z-order, past the layer shown
    /// next to it. Keeps it selected, so the sidebar stays on the layer that
    /// just moved rather than on whatever row now sits under the pointer.
    fn restack_overlay(&mut self, id: OverlayId, up: bool) {
        let Some(other) = self.restack_neighbour(id, up) else {
            return;
        };
        let touched = self.editor.doc.overlay(id).map_or(0, |o| o.range.len());
        self.editor.edit(n("Overlay reordered"), touched, |d| {
            d.restack_overlay(id, other, up)
        });
        self.selected_overlay = Some(id);
        self.after_edit();
    }

    /// Remove one overlay, whichever way it was named: the sidebar's trash
    /// button and the band menu act on the selection, the layer list's X on
    /// its own row.
    fn delete_overlay(&mut self, id: OverlayId, sender: &ComponentSender<Self>) {
        let Some(overlay) = self.editor.doc.overlay(id) else {
            return;
        };
        let touched = overlay.range.len();
        let (change, _) = self.editor.edit(n("Overlay deleted"), touched, |d| {
            d.remove_overlay(id);
        });
        self.after_edit();
        self.toast_if_document_wide(sender, &change, self.frame_count());
    }

    /// Everything a document change invalidates.
    fn after_edit(&mut self) {
        self.rev += 1;
        self.playhead = self.playhead.min(self.frame_count().saturating_sub(1));
        let count = self.frame_count();
        self.selection.retain(|i| *i < count);
        if self.selection.is_empty() && self.scope == ScopeChoice::Range {
            self.scope = ScopeChoice::ThisFrame;
        }
        self.anchor = self.anchor.filter(|i| *i < count);
        if self
            .selected_overlay
            .is_some_and(|id| self.editor.doc.overlay(id).is_none())
        {
            self.selected_overlay = None;
        }
    }

    /// The strip and the canvas already show what a scoped edit did; a toast
    /// earns its interruption only when the edit reached every frame the
    /// document had when it started. `total` is that frame count, taken
    /// before an edit that adds or removes frames changes it.
    fn toast_if_document_wide(
        &self,
        sender: &ComponentSender<Self>,
        change: &Change,
        total: usize,
    ) {
        if change.frames_touched >= total.max(1) {
            sender.input(Msg::Toast(change.message()));
        }
    }
    /// One crop, threaded like a resize or zoom: cropping copies the kept
    /// region rather than resampling it, but a few hundred full frames is
    /// still real work, and it must not freeze the window either. A
    /// full-canvas crop is ignored so the dialog's defaults do not create an
    /// empty undo step.
    fn start_crop(&mut self, rect: (u32, u32, u32, u32), sender: &ComponentSender<Self>) {
        if !crop_changes_canvas(self.editor.doc.size(), rect) {
            return;
        }
        self.crop_tool = false;
        let work = FrameWork::Crop { rect };
        self.busy = Some(Busy {
            kind: BusyKind::Crop,
            done: 0,
            total: Some(work.touched(&self.editor.doc)),
            cancel: None,
        });
        run_frame_work(&self.editor.doc, work, sender);
    }

    /// Decode a still image and fit it to the canvas, ready to splice in as one
    /// frame. `None` if the file will not decode (the caller toasts).
    fn decode_image_frame(&self, path: &Path, delay_cs: u16) -> Option<Frame> {
        let img = image::open(path).ok()?.to_rgba8();
        let size = self.editor.doc.size();
        let img = if size == (0, 0) || img.dimensions() == size {
            img
        } else {
            image::imageops::resize(&img, size.0, size.1, image::imageops::FilterType::Triangle)
        };
        Some(Frame::new(img, delay_cs))
    }

    /// Insert one frame at `at` as a single "Frame added" history step and move
    /// the playhead onto it.
    fn splice_frame(&mut self, at: usize, frame: Frame, sender: &ComponentSender<Self>) {
        let (change, _) = self
            .editor
            .edit(n("Frame added"), 1, |d| d.insert_frame_at(at, frame));
        self.playhead = at;
        self.after_edit();
        self.toast_if_document_wide(sender, &change, self.frame_count());
    }

    /// Fill the canvas from the drawn crop box on `frames`, threaded like a
    /// resize. No-op if no box is drawn, it degenerates, or `frames` is empty.
    fn start_zoom(&mut self, frames: Vec<usize>, sender: &ComponentSender<Self>) {
        let Some(rect) = self.crop_rect.take() else {
            return;
        };
        let Some(rect) = normalize_canvas_rect(self.editor.doc.size(), rect) else {
            return;
        };
        if frames.is_empty() {
            return;
        }
        self.crop_tool = false;
        let work = FrameWork::Zoom { frames, rect };
        self.busy = Some(Busy {
            kind: BusyKind::Zoom,
            done: 0,
            total: Some(work.touched(&self.editor.doc)),
            cancel: None,
        });
        run_frame_work(&self.editor.doc, work, sender);
    }
}

fn normalize_canvas_rect(
    (cw, ch): (u32, u32),
    (x, y, w, h): (f32, f32, f32, f32),
) -> Option<(u32, u32, u32, u32)> {
    let left = x.floor().clamp(0.0, cw as f32) as u32;
    let top = y.floor().clamp(0.0, ch as f32) as u32;
    let right = (x + w).ceil().clamp(0.0, cw as f32) as u32;
    let bottom = (y + h).ceil().clamp(0.0, ch as f32) as u32;
    (right > left && bottom > top).then_some((left, top, right - left, bottom - top))
}

fn resize_fits_budget(w: u32, h: u32, frames: usize, limit: usize) -> bool {
    (w as usize)
        .checked_mul(h as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .and_then(|bytes| bytes.checked_mul(frames))
        .is_some_and(|bytes| bytes <= limit)
}

fn crop_changes_canvas((cw, ch): (u32, u32), (x, y, w, h): (u32, u32, u32, u32)) -> bool {
    cw > 0 && ch > 0 && !(x == 0 && y == 0 && w >= cw && h >= ch)
}

/// Fit imported frames onto an existing canvas before mixing them in — a
/// second file rarely decodes at the same size, and appending frames the
/// compositor cannot assume are uniform would break every op that reads
/// `Document::size()` from the first frame. `(0, 0)` means the document was
/// empty, so whatever the new frames decoded at becomes the canvas.
fn resize_frames_to(frames: Vec<Frame>, (cw, ch): (u32, u32)) -> Vec<Frame> {
    if cw == 0 || ch == 0 {
        return frames;
    }
    frames
        .into_iter()
        .map(|frame| {
            if frame.pixels.dimensions() == (cw, ch) {
                return frame;
            }
            let pixels = image::imageops::resize(
                frame.pixels.as_ref(),
                cw,
                ch,
                image::imageops::FilterType::Triangle,
            );
            let mut resized = Frame::new(pixels, frame.delay_cs);
            resized.detached = frame.detached;
            resized
        })
        .collect()
}

/// Extensions "Add frames from file" splices in synchronously as one still
/// frame. Anything else — video, GIF, animated WebP — goes through the async
/// import pipeline so it gets probing, progress, cancel and the resize prompt.
fn is_still_image(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "png"
                | "jpg"
                | "jpeg"
                | "bmp"
                | "tif"
                | "tiff"
                | "tga"
                | "ico"
                | "qoi"
                | "ppm"
                | "pgm"
                | "pbm"
                | "pnm"
        )
    })
}

/// Probe before decoding. ffprobe is cheap but it is still a subprocess, so it
/// runs off the main thread like the decode does.
fn plan_import(
    path: PathBuf,
    options: ImportOptions,
    cancel: Arc<AtomicBool>,
    sender: &ComponentSender<App>,
) {
    // GIFs arrive frame-exact and small; there is nothing to warn about.
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("gif"))
    {
        load(path, None, options, cancel, sender);
        return;
    }
    sender.spawn_command(move |out| {
        let plan = video::plan(&path, &options).map_err(|e| e.to_string());
        out.emit(Cmd::Planned(path, Box::new(plan)));
    });
}

/// The heavy half of a resize or zoom: a snapshot of the document goes to a
/// worker thread, which produces the frames one at a time and reports
/// progress; the finished set comes back as one `Cmd::Worked` and is applied
/// as a single history step. `Document` clones as pointer copies, so the
/// snapshot costs nothing but a couple of `Vec` headers.
fn run_frame_work(doc: &Document, work: FrameWork, sender: &ComponentSender<App>) {
    let doc = doc.clone();
    sender.spawn_command(move |out| {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // Same throttle as the import decode: a fast job would otherwise
            // post a message per frame, and the main loop has to drain each one.
            let mut next = std::time::Instant::now();
            let mut progress = |done, total| {
                let now = std::time::Instant::now();
                if now < next {
                    return;
                }
                next = now + std::time::Duration::from_millis(80);
                let _ = out.send(Cmd::WorkProgress(done, total));
            };
            let frames = match &work {
                FrameWork::Resize(w, h) => doc.resized_frames(*w, *h, &mut progress),
                FrameWork::Zoom { frames, rect } => {
                    let (x, y, w, h) = *rect;
                    doc.zoomed_frames(frames, x, y, w, h, &mut progress)
                }
                FrameWork::Shrink { frames, rect } => {
                    let (x, y, w, h) = *rect;
                    doc.shrunk_frame_list(frames, x, y, w, h, &mut progress)
                }
                FrameWork::Crop { rect } => {
                    let (x, y, w, h) = *rect;
                    doc.cropped_frames(x, y, w, h, &mut progress)
                }
            };
            Box::new(WorkDone {
                label: work.label(),
                frames_touched: frames.len(),
                scale: work.scale(&doc),
                shift: work.shift(),
                frames,
            })
        }))
        .map_err(|_| n("The frame operation failed."));
        out.emit(Cmd::Worked(result));
    });
}
/// The file cannot come in whole, so let the user pick the size it comes in at
/// rather than only telling them what we decided. Memory is exact; the GIF
/// figure is a range, because the content decides that one.
fn confirm_oversize(
    root: &adw::ApplicationWindow,
    path: &Path,
    plan: &ImportPlan,
    max_bytes: usize,
    sender: &ComponentSender<App>,
) {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string());
    let dialog = adw::AlertDialog::new(
        Some(t("This video is large")),
        Some(&oversize_body(&name, plan)),
    );
    dialog.set_body_use_markup(true);

    let source = plan.source.clone();
    let recommended = (plan.width, plan.height);
    let mut sizes = vec![recommended];
    sizes.extend(
        video::size_presets(&source)
            .into_iter()
            .filter(|s| *s != recommended),
    );

    let mut labels: Vec<String> = sizes.iter().map(|(w, h)| format!("{w} × {h}")).collect();
    labels[0] = fill(t("{size} (recommended)"), &[("size", &labels[0])]);
    labels.push(t("Custom…").into());
    let custom = labels.len() - 1;
    let strings: Vec<&str> = labels.iter().map(String::as_str).collect();
    let presets = gtk::DropDown::from_strings(&strings);

    let width = size_spin(source.width, plan.width);
    let height = size_spin(source.height, plan.height);
    // Without a rate control, shrinking the frame just buys back frame rate and
    // the memory figure never moves, which makes the picker look broken.
    let rate = gtk::SpinButton::with_range(1.0, source.fps.clamp(1.0, 50.0), 1.0);
    rate.set_value(plan.fps.floor().max(1.0));
    rate.set_width_chars(3);
    rate.set_max_width_chars(3);
    let custom_row = adw::ActionRow::builder().title(t("Size")).build();
    let spins = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    spins.set_valign(gtk::Align::Center);
    spins.append(&width);
    spins.append(&gtk::Label::new(Some("×")));
    spins.append(&height);
    // Translators: Abbreviation for pixels, shown after a pair of size fields.
    spins.append(&gtk::Label::new(Some(t("px"))));
    custom_row.add_suffix(&spins);

    let rate_row = adw::ActionRow::builder().title(t("Frame rate")).build();
    rate.set_valign(gtk::Align::Center);
    let rate_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    rate_box.set_valign(gtk::Align::Center);
    rate_box.append(&rate);
    // Translators: Abbreviation for frames per second.
    rate_box.append(&gtk::Label::new(Some(t("fps"))));
    rate_row.add_suffix(&rate_box);

    let size_row = adw::ActionRow::builder().title(t("Import at")).build();
    presets.set_valign(gtk::Align::Center);
    size_row.add_suffix(&presets);

    let summary = gtk::Label::new(None);
    summary.add_css_class("dim-label");
    summary.set_xalign(0.0);
    summary.set_wrap(true);
    summary.set_margin_top(6);

    dialog.set_content_width(480);

    let group = adw::PreferencesGroup::new();
    group.add(&size_row);
    group.add(&custom_row);
    group.add(&rate_row);
    let form = gtk::Box::new(gtk::Orientation::Vertical, 0);
    form.append(&group);
    form.append(&summary);
    dialog.set_extra_child(Some(&form));

    // The plan the Import response will run, rebuilt on every change so the
    // preview and the decode can never disagree.
    let chosen = Rc::new(RefCell::new(plan.clone()));
    // A measurement belongs to the settings it was taken at. Changing anything
    // retires it and any measurement still in flight.
    let generation = Rc::new(std::cell::Cell::new(0u64));

    let measure: Rc<dyn Fn()> = Rc::new({
        let (chosen, summary, generation) = (chosen.clone(), summary.clone(), generation.clone());
        let path = path.to_path_buf();
        move || {
            let started_at = generation.get();
            let plan = chosen.borrow().clone();
            let (summary, generation) = (summary.clone(), generation.clone());
            let work = relm4::spawn_blocking({
                let path = path.clone();
                move || (crate::pipeline::estimate_gif_size(&path, &plan), plan)
            });
            glib::spawn_future_local(async move {
                let Ok((result, plan)) = work.await else {
                    return;
                };
                // The settings moved while we were measuring, so this answer is
                // about a file nobody asked for any more.
                if generation.get() != started_at {
                    return;
                }
                match result {
                    Ok(bytes) => summary.set_label(&plan_summary(&plan, Some(bytes))),
                    Err(e) => summary.set_label(&format!(
                        "{}\n{}",
                        plan_summary(&plan, None),
                        fill(
                            t("Could not measure the size: {error}"),
                            &[("error", &e.to_string())]
                        ),
                    )),
                }
            });
        }
    });

    let refresh = {
        let (source, chosen, summary) = (source.clone(), chosen.clone(), summary.clone());
        let dialog = dialog.clone();
        let (width, height, rate) = (width.clone(), height.clone(), rate.clone());
        let (generation, measure) = (generation.clone(), measure.clone());
        move || {
            generation.set(generation.get() + 1);
            let started_at = generation.get();

            let target = (width.value_as_int() as u32, height.value_as_int() as u32);
            let options = ImportOptions {
                target: Some(target),
                fps: Some(rate.value()),
                max_bytes,
                ..Default::default()
            };
            let plan = video::plan_for(source.clone(), &options);
            summary.set_label(&plan_summary(&plan, None));
            let fits = !plan.over_budget();
            *chosen.borrow_mut() = plan;
            dialog.set_response_enabled("import", fits);

            // No point spending ffmpeg on settings that cannot be imported.
            if fits {
                let (generation, measure) = (generation.clone(), measure.clone());
                glib::timeout_add_local_once(MEASURE_DEBOUNCE, move || {
                    if generation.get() == started_at {
                        measure();
                    }
                });
            }
        }
    };
    dialog.add_response("cancel", t("Cancel"));
    dialog.add_response("import", t("Import"));
    dialog.set_response_appearance("import", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    let refresh = Rc::new(refresh);
    refresh();

    {
        let (width, height, refresh) = (width.clone(), height.clone(), refresh.clone());
        let sizes = sizes.clone();
        presets.connect_selected_notify(move |drop| {
            let picked = drop.selected() as usize;
            // Custom starts from wherever the last preset left the spins.
            if let Some((w, h)) = sizes.get(picked) {
                width.set_value(*w as f64);
                height.set_value(*h as f64);
            }
            width.set_sensitive(picked == custom);
            height.set_sensitive(picked == custom);
            refresh();
        });
    }
    width.set_sensitive(false);
    height.set_sensitive(false);
    for spin in [&width, &height, &rate] {
        let refresh = refresh.clone();
        spin.connect_value_changed(move |_| refresh());
    }

    let sender = sender.clone();
    let path = path.to_path_buf();
    dialog.choose(Some(root), gio::Cancellable::NONE, move |response| {
        if response == "import" {
            sender.input(Msg::LoadConfirmed(path, Box::new(chosen.borrow().clone())));
        }
    });
}

/// The dialog's opening line: what the file is, and why it cannot come in as
/// it stands. The numbers for a given choice live in the live summary instead.
/// Pango markup, so the filename stands out from the numbers around it.
/// Everything interpolated is escaped; only the emphasis is ours.
fn oversize_body(name: &str, plan: &ImportPlan) -> String {
    let src = &plan.source;
    let length = match src.duration_s {
        Some(d) => fill(t("runs {duration}"), &[("duration", &clock(d))]),
        None => t("has no duration the container will admit to").into(),
    };
    // Translators: {name} arrives already marked up as bold; keep the tags.
    let opening = fill(
        // Translators: Pango markup: keep the <b> tags. {length} is "runs 1:30" or the no-duration phrase.
        t(
            "<b>{name}</b> {length} at {width}×{height}, {fps} fps — more than will fit in \
           memory at full size.",
        ),
        &[
            ("name", &glib::markup_escape_text(name)),
            ("length", &glib::markup_escape_text(&length)),
            ("width", &src.width.to_string()),
            ("height", &src.height.to_string()),
            ("fps", &format!("{:.0}", src.fps)),
        ],
    );
    format!(
        "{opening}\n\n{}",
        t(
            "Pick a smaller size below, or cancel and trim or crop the file first, which \
           gives a better GIF and a much faster import."
        )
    )
}

/// Even values only, which is what every scaler downstream wants.
fn size_spin(source: u32, value: u32) -> gtk::SpinButton {
    let spin = gtk::SpinButton::with_range(16.0, source.max(16) as f64, 2.0);
    spin.set_value(value as f64);
    spin.set_width_chars(4);
    spin.set_max_width_chars(4);
    spin
}

/// The live preview under the picker.
fn plan_summary(plan: &ImportPlan, measured: Option<usize>) -> String {
    let rate = if plan.fps < plan.source.fps - 0.01 {
        fill(
            t("{fps} fps, down from {source}"),
            &[
                ("fps", &format!("{:.0}", plan.fps.max(1.0))),
                ("source", &format!("{:.0}", plan.source.fps)),
            ],
        )
    } else {
        fill(t("{fps} fps"), &[("fps", &format!("{:.0}", plan.fps))])
    };
    let Some(frames) = plan.frames() else {
        return fill(
            // Translators: {rate} is a phrase such as "4 fps, down from 60"; it opens the sentence.
            t("{rate}. The container gives no duration, so there is no count to preview."),
            &[("rate", &rate)],
        );
    };

    let memory = plan.bytes().map_or(String::new(), |b| {
        format!(" · {}", fill(t("{size} in memory"), &[("size", &size(b))]))
    });
    let gif = match measured {
        Some(bytes) => format!(
            " · {}",
            fill(t("{size} as a GIF"), &[("size", &size(bytes))])
        ),
        None => format!(" · {}", t("measuring the GIF size…")),
    };
    let head = fill(
        t("{frames} frames at {rate}"),
        &[("frames", &frames.to_string()), ("rate", &rate)],
    );
    if plan.over_budget() {
        return format!(
            "{head}{memory}\n{}",
            t(
                "That is more memory than an import may use. Choose a smaller size or a \
               lower frame rate."
            )
        );
    }
    format!("{head}{memory}{gif}")
}

fn size(bytes: usize) -> String {
    match bytes {
        b if b >= 1 << 30 => fill(
            t("{n} GB"),
            &[("n", &format!("{:.1}", b as f64 / (1u64 << 30) as f64))],
        ),
        b if b >= 1 << 20 => fill(
            t("{n} MB"),
            &[("n", &format!("{:.0}", b as f64 / (1u64 << 20) as f64))],
        ),
        b => fill(t("{n} KB"), &[("n", &format!("{:.0}", b as f64 / 1024.0))]),
    }
}

fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

fn load(
    path: PathBuf,
    plan: Option<ImportPlan>,
    options: ImportOptions,
    cancel: Arc<AtomicBool>,
    sender: &ComponentSender<App>,
) {
    sender.spawn_command(move |out| {
        // Throttled: a 60 fps decode would otherwise post thousands of messages
        // the main loop has to drain before it can draw any of them.
        let mut next = std::time::Instant::now();
        let mut progress = |done, expected| {
            if cancel.load(Ordering::Relaxed) {
                return false;
            }
            let now = std::time::Instant::now();
            if now < next {
                return true;
            }
            next = now + std::time::Duration::from_millis(80);
            out.send(Cmd::ImportProgress(done, expected)).is_ok()
        };
        let decoded = match &plan {
            Some(plan) => video::import_planned(&path, plan, &mut progress),
            None => import_any(&path, &options, &mut progress),
        };
        // The pipelines come back with whatever decoded before the stop, so
        // the flag, not the result shape, is what says the user cancelled.
        let outcome = if cancel.load(Ordering::Relaxed) {
            ImportOutcome::Cancelled
        } else {
            match decoded {
                Ok(frames) => ImportOutcome::Loaded(path, frames),
                Err(e) => ImportOutcome::Failed(e.to_string()),
            }
        };
        out.emit(Cmd::Imported(Box::new(outcome)));
    });
}

fn timecode(cs: u32) -> String {
    format!(
        "{:02}:{:02}.{}",
        cs / 6000,
        (cs / 100) % 60,
        (cs % 100) / 10
    )
}

fn fps_of(doc: &Document) -> u32 {
    let total = doc.duration_cs();
    if total == 0 {
        0
    } else {
        (doc.frames.len() as f32 * 100.0 / total as f32).round() as u32
    }
}

/// Ctrl+click: add or remove one frame, wherever it sits. The result need not
/// be a run, which is the whole difference from Shift.
fn toggle_frame(selection: &mut Vec<usize>, frame: usize) {
    match selection.iter().position(|f| *f == frame) {
        Some(at) => {
            selection.remove(at);
        }
        None => selection.push(frame),
    }
    selection.sort_unstable();
}

/// Shift+click: the run between the anchor and here, in either direction.
fn run_between(anchor: usize, frame: usize) -> Vec<usize> {
    (anchor.min(frame)..=anchor.max(frame)).collect()
}

/// The frames an overlay edit lands on: where the scope reaches into the
/// overlay's own range. When the scope does not reach the frame on screen, the
/// frame on screen wins — an edit must land where the user is looking, never
/// somewhere they cannot see. An empty result is the caller's no-op.
fn overlay_edit_span(overlay: Range<usize>, scope: Range<usize>, playhead: usize) -> Range<usize> {
    let span = overlay.start.max(scope.start)..overlay.end.min(scope.end);
    if span.is_empty() {
        return 0..0;
    }
    if !span.contains(&playhead) && overlay.contains(&playhead) {
        return playhead..playhead + 1;
    }
    span
}

/// One thumbnail dimension at `zoom`, rounded to whole pixels the way the
/// widget will be sized, never off to nothing.
fn zoomed(px: u32, zoom: f64) -> i32 {
    (px as f64 * zoom).round().max(1.0) as i32
}

/// Distance from one strip cell to the next: a zoomed thumbnail plus the
/// strip `Box`'s spacing, which is a fixed number of pixels and *not* zoomed
/// — the reason this is not `(thumb_w + THUMB_SPACING) * zoom`. The bands
/// under the strip put their per-frame columns here, so a pitch that does
/// not match what the cells measure out to slides every band sideways by a
/// growing fraction of a frame.
fn cell_pitch(thumb_w: i32, zoom: f64) -> f64 {
    (zoomed(thumb_w.max(1) as u32, zoom) + THUMB_SPACING) as f64
}

fn strip_width(count: usize, pitch: f64) -> i32 {
    (count as f64 * pitch) as i32
}

/// Size one strip thumbnail for `zoom`. Zooming in is a size request over
/// the thumbnail's own texture, which a `GtkPicture` scales up to whatever
/// it is given. Zooming out has to shrink the texture: a picture never
/// measures smaller than its paintable, so below 1x a request alone left
/// every cell at its 1x width while the bands under them shrank away from
/// the frames they annotate. The scaled copies are cheap — a thumbnail is
/// `THUMB_BOX` pixels on its long side — and only built when the zoom moves.
fn set_thumb_zoom(picture: &gtk::Picture, thumb: &image::RgbaImage, zoom: f64) {
    let (w, h) = (zoomed(thumb.width(), zoom), zoomed(thumb.height(), zoom));
    let paintable_w = picture.paintable().map(|p| p.intrinsic_width());
    if zoom < 1.0 {
        if paintable_w != Some(w) {
            let small = image::imageops::resize(
                thumb,
                w as u32,
                h as u32,
                image::imageops::FilterType::Triangle,
            );
            picture.set_paintable(Some(&texture_from(&small)));
        }
    } else if paintable_w != Some(thumb.width() as i32) {
        picture.set_paintable(Some(&texture_from(thumb)));
    }
    picture.set_size_request(w, h);
}

/// New timeline-strip zoom after a `Msg::StripZoom(factor)`: `factor` multiplies
/// the current zoom and the result is clamped to the bounds; `0.0` is the reset
/// sentinel back to 1x.
fn next_strip_zoom(current: f64, factor: f64) -> f64 {
    if factor == 0.0 {
        1.0
    } else {
        (current * factor).clamp(STRIP_ZOOM_MIN, STRIP_ZOOM_MAX)
    }
}

fn texture_from(img: &image::RgbaImage) -> gdk::Texture {
    let (w, h) = img.dimensions();
    gdk::MemoryTexture::new(
        w as i32,
        h as i32,
        gdk::MemoryFormat::R8g8b8a8,
        &glib::Bytes::from_owned(img.as_raw().clone()),
        (w * 4) as usize,
    )
    .upcast()
}

fn set_class(widget: &gtk::Widget, class: &str, on: bool) {
    if on {
        widget.add_css_class(class);
    } else {
        widget.remove_css_class(class);
    }
}

/// The strip's thumbnail cells, in order — never the frame context menu's
/// popover, which is also parented to the strip (a `Popover` needs a parent
/// to show itself) and would otherwise turn up in `first_child()` right along
/// with the real cells, one position ahead of every thumbnail after it.
/// Regression: that shift left the playhead border one frame behind whatever
/// was actually clicked, while the canvas — which reads `self.playhead`
/// directly, not this walk — showed the right one.
fn thumb_children(strip: &gtk::Box) -> impl Iterator<Item = gtk::Widget> + use<> {
    let mut child = strip.first_child();
    std::iter::from_fn(move || {
        while let Some(widget) = child.take() {
            child = widget.next_sibling();
            if widget.has_css_class("thumb") {
                return Some(widget);
            }
        }
        None
    })
}

/// A move removes the picked frames, which shifts every later index down,
/// then inserts at `to` — so where the run actually lands depends on how
/// many picked frames sat before the gap. `gap` names a position between
/// frames as shown before the move (`count` is past the last); this is the
/// `to` that puts the run exactly there once `picked` is gone from the
/// list, matching what the divider promised. `picked` must be sorted.
fn move_target_for_set(picked: &[usize], gap: usize) -> usize {
    gap - picked.iter().filter(|&&p| p < gap).count()
}

/// The gap "nudge the whole selection one slot earlier/later" aims at: just
/// before the unselected frame preceding it, or just after the one
/// following it — swapping the block with that neighbour. `None` when the
/// selection already sits against that edge, or names every frame and
/// leaves nothing to swap with. `picked` must be sorted.
fn selection_nudge_gap(picked: &[usize], earlier: bool, frame_count: usize) -> Option<usize> {
    let first = *picked.first()?;
    let last = *picked.last()?;
    if earlier {
        (first > 0).then(|| first - 1)
    } else {
        (last + 1 < frame_count).then(|| last + 2)
    }
}

/// Which gap between thumbnails `x` (strip-local) is nearest: `0` is before
/// the first frame, `count` is after the last. Splits each cell at its own
/// midpoint, so the divider promises exactly where a drop will land.
fn gap_at(strip: &gtk::Box, x: f64) -> usize {
    let cells: Vec<gtk::Widget> = thumb_children(strip).collect();
    for (i, cell) in cells.iter().enumerate() {
        let Some(bounds) = cell.compute_bounds(strip) else {
            continue;
        };
        if x < bounds.x() as f64 + bounds.width() as f64 / 2.0 {
            return i;
        }
    }
    cells.len()
}

/// Shows the drag-drop divider at `gap`, the same accent blue as the
/// playhead border: on the left edge of the cell that gap sits before, or —
/// for the gap past the last frame, which has no cell after it — on the
/// right edge of the last cell instead. The dividers are Overlay children
/// of the thumbnails, so the line paints on top of the picture; they are
/// rebuilt with the strip (`rebuild_strip` fills the vec in strip order).
fn mark_drop_gap(dividers: &[gtk::Widget], gap: Option<usize>) {
    let count = dividers.len();
    for (i, divider) in dividers.iter().enumerate() {
        let (show, at_end) = match gap {
            Some(g) if g == i => (true, false),
            Some(g) if g == count && i + 1 == count => (true, true),
            _ => (false, false),
        };
        divider.set_visible(show);
        if show {
            divider.set_halign(if at_end {
                gtk::Align::End
            } else {
                gtk::Align::Start
            });
        }
    }
}

/// What the strip is showing. Only a change here is worth a rebuild: overlay
/// edits move the document revision too, and there are hundreds of thumbnails.
fn strip_keys(doc: &Document) -> Vec<(u64, bool)> {
    doc.frames.iter().map(|f| (f.key, f.detached)).collect()
}

/// ponytail: the strip rebuilds a widget per frame when the frame list changes.
/// The thumbnails themselves are already built (see `Frame::new`), so this is a
/// hitch rather than a freeze; swap in a virtualized list when someone imports
/// something long enough to notice.
fn rebuild_strip(
    strip: &gtk::Box,
    doc: &Document,
    sender: &ComponentSender<App>,
    scope_mirror: &Rc<RefCell<Vec<usize>>>,
    drop_dividers: &Rc<RefCell<Vec<gtk::Widget>>>,
) {
    while let Some(child) = strip.first_child() {
        strip.remove(&child);
    }
    drop_dividers.borrow_mut().clear();

    // Two faces of the one popover: the frame under the pointer when it was
    // opened either stands alone — the menu acts on that frame — or belongs
    // to the active selection, and then it acts on the whole scope. The
    // right-click handler picks which model to show; the actions are
    // scope-based either way, because a plain right-click outside the
    // selection seeks first, which resets the scope to that one frame.
    let edit_section = gio::Menu::new();
    edit_section.append(Some(t("Delete this frame")), Some("frame.delete"));
    edit_section.append(Some(t("Duplicate this frame")), Some("frame.duplicate"));
    edit_section.append(Some(t("Cut this frame")), Some("frame.cut"));
    edit_section.append(Some(t("Copy this frame")), Some("frame.copy"));
    edit_section.append(Some(t("Paste frames")), Some("frame.paste"));
    edit_section.append(Some(t("Set delay…")), Some("frame.delay"));
    let move_section = gio::Menu::new();
    move_section.append(Some(t("Move earlier")), Some("frame.move-earlier"));
    move_section.append(Some(t("Move later")), Some("frame.move-later"));
    move_section.append(Some(t("Move to position…")), Some("frame.move-to"));
    let import_section = gio::Menu::new();
    import_section.append(Some(t("Add frame from image…")), Some("frame.add-image"));
    let menu = gio::Menu::new();
    menu.append_section(None, &edit_section);
    menu.append_section(None, &move_section);
    menu.append_section(None, &import_section);
    let selection_edit_section = gio::Menu::new();
    selection_edit_section.append(Some(t("Delete selected frames")), Some("frame.delete"));
    selection_edit_section.append(
        Some(t("Duplicate selected frames")),
        Some("frame.duplicate"),
    );
    selection_edit_section.append(Some(t("Cut selected frames")), Some("frame.cut"));
    selection_edit_section.append(Some(t("Copy selected frames")), Some("frame.copy"));
    selection_edit_section.append(Some(t("Paste frames")), Some("frame.paste"));
    selection_edit_section.append(Some(t("Set delay…")), Some("frame.delay"));
    let selection_move_section = gio::Menu::new();
    selection_move_section.append(Some(t("Move earlier")), Some("frame.move-earlier"));
    selection_move_section.append(Some(t("Move later")), Some("frame.move-later"));
    let selection_menu = gio::Menu::new();
    selection_menu.append_section(None, &selection_edit_section);
    selection_menu.append_section(None, &selection_move_section);
    selection_menu.append_section(None, &import_section);
    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.set_parent(strip);
    popover.set_has_arrow(false);
    // A popover outlives its parent unless it is told not to, and this strip is
    // torn down on every frame-list change.
    {
        let popover = popover.clone();
        strip.connect_destroy(move |_| popover.unparent());
    }
    // Which frame the last right-click landed on.
    let target = Rc::new(std::cell::Cell::new(0usize));
    let group = gio::SimpleActionGroup::new();
    for (name, op) in [
        ("delete", FrameOp::Delete),
        ("duplicate", FrameOp::Duplicate),
        ("cut", FrameOp::Cut),
    ] {
        let action = gio::SimpleAction::new(name, None);
        let sender = sender.clone();
        action.connect_activate(move |_, _| sender.input(Msg::FrameOp(op)));
        group.add_action(&action);
    }
    // Function pointers rather than values: an action fires every time the
    // item is picked, and `Msg` is not `Clone`.
    for (name, msg) in [
        ("copy", (|| Msg::FrameCopy) as fn() -> Msg),
        ("paste", || Msg::FramePaste),
    ] {
        let action = gio::SimpleAction::new(name, None);
        let sender = sender.clone();
        action.connect_activate(move |_, _| sender.input(msg()));
        group.add_action(&action);
    }
    let delay_action = gio::SimpleAction::new("delay", None);
    {
        let (sender, target, strip, scope_mirror) = (
            sender.clone(),
            target.clone(),
            strip.clone(),
            scope_mirror.clone(),
        );
        let delays: Vec<u16> = doc.frames.iter().map(|f| f.delay_cs).collect();
        delay_action.connect_activate(move |_, _| {
            let i = target.get();
            let scope = scope_mirror.borrow().clone();
            delay_scope_dialog(
                &strip,
                &scope,
                delays.get(i).copied().unwrap_or(10),
                &sender,
            );
        });
    }
    group.add_action(&delay_action);
    let move_earlier = gio::SimpleAction::new("move-earlier", None);
    {
        let sender = sender.clone();
        move_earlier.connect_activate(move |_, _| {
            sender.input(Msg::MoveSelection { earlier: true });
        });
    }
    group.add_action(&move_earlier);
    let move_later = gio::SimpleAction::new("move-later", None);
    {
        let sender = sender.clone();
        move_later.connect_activate(move |_, _| {
            sender.input(Msg::MoveSelection { earlier: false });
        });
    }
    group.add_action(&move_later);
    let move_to = gio::SimpleAction::new("move-to", None);
    {
        let (sender, target) = (sender.clone(), target.clone());
        move_to.connect_activate(move |_, _| sender.input(Msg::MoveFrameDialog(target.get())));
    }
    group.add_action(&move_to);
    let add_image = gio::SimpleAction::new("add-image", None);
    {
        let (sender, target, strip) = (sender.clone(), target.clone(), strip.clone());
        add_image.connect_activate(move |_, _| {
            let dialog = gtk::FileDialog::builder()
                .title(t("Add frame from image"))
                .build();
            let filter = gtk::FileFilter::new();
            filter.set_name(Some(t("Images")));
            filter.add_mime_type("image/*");
            let filters = gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);
            dialog.set_filters(Some(&filters));
            let window = strip.root().and_downcast::<gtk::Window>();
            let (sender, index) = (sender.clone(), target.get());
            dialog.open(window.as_ref(), gio::Cancellable::NONE, move |res| {
                if let Some(path) = res.ok().and_then(|f| f.path()) {
                    sender.input(Msg::InsertImageFrame(index, path));
                }
            });
        });
    }
    group.add_action(&add_image);
    strip.insert_action_group("frame", Some(&group));

    for (i, frame) in doc.frames.iter().enumerate() {
        // Sized by `set_thumb_zoom`, which `update_view` runs over the whole
        // strip right after any rebuild.
        let picture = gtk::Picture::for_paintable(&texture_from(&frame.thumb));
        // The drag-reorder divider is an Overlay child rather than CSS on
        // the cell: an inset box-shadow painted below the child widgets and
        // never showed over the thumbnail. Overlays paint on top.
        let divider = gtk::Box::new(gtk::Orientation::Vertical, 0);
        divider.add_css_class("drop-divider");
        divider.set_width_request(3);
        divider.set_visible(false);
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&picture));
        overlay.add_overlay(&divider);
        drop_dividers.borrow_mut().push(divider.upcast());

        let cell = gtk::Box::new(gtk::Orientation::Vertical, 2);
        cell.add_css_class("thumb");
        cell.append(&overlay);
        let label = gtk::Label::new(Some(&(i + 1).to_string()));
        label.add_css_class("dim-label");
        label.add_css_class("caption");
        label.add_css_class("tnum");
        // Ellipsized so a four-digit frame number cannot widen the cell past
        // its thumbnail box and slide the bands out from under the strip.
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        cell.append(&label);
        if frame.detached {
            let badge = gtk::Image::from_icon_name("document-edit-symbolic");
            badge.set_tooltip_text(Some(t("Edited outside; overlays skip this frame")));
            cell.append(&badge);
        }
        cell.set_tooltip_text(Some(&fill(
            // Translators: "cs" is centiseconds, the unit GIF stores frame delays in.
            t("Frame {number} · {delay} cs"),
            &[
                ("number", &(i + 1).to_string()),
                ("delay", &frame.delay_cs.to_string()),
            ],
        )));

        // A press on a frame that is inside the active selection must not
        // collapse the selection on the spot: the click may be the start of
        // a drag that moves the whole selection, and the drop still needs
        // to find it. So the press only arms a pending seek; if a drag
        // begins it is dropped, and a plain release — no drag — collapses
        // the selection to that one frame, which is what a click means.
        let pending_seek = Rc::new(Cell::new(None::<usize>));
        let click = gtk::GestureClick::new();
        {
            let (sender, scope_mirror, pending_seek) =
                (sender.clone(), scope_mirror.clone(), pending_seek.clone());
            // Released-handler copies: the press closure moves its own in.
            let (released_sender, released_pending) = (sender.clone(), pending_seek.clone());
            click.connect_pressed(move |gesture, _, _, _| {
                let state = gesture.current_event_state();
                let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
                let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
                if shift {
                    sender.input(Msg::ExtendSelection(i));
                } else if ctrl {
                    sender.input(Msg::ToggleSelection(i));
                } else {
                    let on_selection = {
                        let scope = scope_mirror.borrow();
                        scope.len() > 1 && scope.contains(&i)
                    };
                    if on_selection {
                        pending_seek.set(Some(i));
                    } else {
                        sender.input(Msg::Seek(i));
                    }
                }
            });
            click.connect_released(move |_, _, _, _| {
                if let Some(i) = released_pending.take() {
                    released_sender.input(Msg::Seek(i));
                }
            });
        }
        cell.add_controller(click);

        // Right-click acts on the frame under the pointer, not on the scope,
        // which is the whole reason to have it as well as the ⋮ menu — but
        // only when that frame stands alone. Right-clicking a frame already
        // inside the active selection keeps the selection and switches the
        // popover to its scope-acting menu. The popover is shared: one per
        // frame is a widget tree per thumbnail.
        let secondary = gtk::GestureClick::new();
        secondary.set_button(gdk::BUTTON_SECONDARY);
        {
            let (menu, selection_menu) = (menu.clone(), selection_menu.clone());
            let (sender, target, popover, strip, scope_mirror) = (
                sender.clone(),
                target.clone(),
                popover.clone(),
                strip.clone(),
                scope_mirror.clone(),
            );
            let cell = cell.clone();
            secondary.connect_pressed(move |_, _, x, y| {
                target.set(i);
                let on_selection = {
                    let scope = scope_mirror.borrow();
                    scope.len() > 1 && scope.contains(&i)
                };
                if on_selection {
                    sender.input(Msg::SeekKeepSelection(i));
                    popover.set_menu_model(Some(&selection_menu));
                } else {
                    sender.input(Msg::Seek(i));
                    popover.set_menu_model(Some(&menu));
                }
                let point = cell
                    .compute_point(&strip, &gtk::graphene::Point::new(x as f32, y as f32))
                    .unwrap_or_else(|| gtk::graphene::Point::new(x as f32, y as f32));
                popover.set_pointing_to(Some(&gdk::Rectangle::new(
                    point.x() as i32,
                    point.y() as i32,
                    1,
                    1,
                )));
                popover.popup();
            });
        }
        cell.add_controller(secondary);

        // Drag one thumbnail to reorder it; the payload is just the source
        // index. Where it lands is decided by the strip's own drop target
        // (added once, in `build`), which tracks the gap nearest the pointer
        // across the whole strip rather than per cell.
        let drag_source = gtk::DragSource::new();
        drag_source.set_actions(gdk::DragAction::MOVE);
        {
            let pending_seek = pending_seek.clone();
            drag_source.connect_drag_begin(move |_, _| {
                // The press became a drag: the drop moves whatever the
                // selection then names, so it must still be whole here —
                // cancel the collapse-on-release instead.
                pending_seek.take();
            });
        }
        {
            let from = i as u32;
            drag_source.connect_prepare(move |_, _, _| {
                Some(gdk::ContentProvider::for_value(&from.to_value()))
            });
        }
        cell.add_controller(drag_source);

        strip.append(&cell);
    }
}

/// The message an action fires. Kept next to the keymap so a new action is one
/// arm here and one entry in `keymap::ACTIONS`, not a scattered edit.
fn message_for(action: Action) -> Msg {
    match action {
        Action::Open => Msg::Open,
        Action::Export => Msg::Export,
        Action::Undo => Msg::Undo,
        Action::Redo => Msg::Redo,
        Action::PlayPause => Msg::TogglePlay,
        Action::SelectAll => Msg::SelectAllFrames,
        Action::Delete => Msg::DeleteSelection,
        Action::ShowShortcuts => Msg::Toast(String::new()),
        Action::ToolText => Msg::AddOverlay(Tool::Text),
        Action::ToolRect => Msg::AddOverlay(Tool::Rect),
        Action::ToolEllipse => Msg::AddOverlay(Tool::Ellipse),
        Action::ToolArrow => Msg::AddOverlay(Tool::Arrow),
        Action::ToolCrop => Msg::ToggleCropTool,
        Action::FrameDelete => Msg::FrameOp(FrameOp::Delete),
        Action::FrameDuplicate => Msg::FrameOp(FrameOp::Duplicate),
        Action::FrameCut => Msg::FrameOp(FrameOp::Cut),
        Action::FrameCopy => Msg::FrameCopy,
        Action::FramePaste => Msg::FramePaste,
        Action::FrameReverse => Msg::FrameOp(FrameOp::Reverse),
        Action::ZoomToSelection => Msg::ApplyZoom,
        Action::StripZoomIn => Msg::StripZoom(STRIP_ZOOM_STEP),
        Action::StripZoomOut => Msg::StripZoom(1.0 / STRIP_ZOOM_STEP),
        Action::StripZoomReset => Msg::StripZoom(0.0),
    }
}

/// One controller reading the live keymap, rather than a match on hardcoded
/// keys. The keymap is shared rather than owned because the controller cannot
/// borrow the model.
fn install_shortcuts(
    root: &adw::ApplicationWindow,
    sender: &ComponentSender<App>,
    keymap: Rc<RefCell<Keymap>>,
) {
    let keys = shortcuts_controller();
    let esc_sender = sender.clone();
    let sender = sender.clone();
    let root_for_dialog = root.clone();
    // Where the tool cycle is up to, for the case where several tools share a
    // key (AGENTS.md: that is a cycle, not a duplicate).
    let cycle = std::cell::Cell::new(0usize);
    keys.connect_key_pressed(move |controller, key, _, state| {
        let focus = controller
            .widget()
            .and_then(|w| w.downcast::<gtk::Window>().ok())
            .and_then(|w| gtk::prelude::GtkWindowExt::focus(&w));
        if focus_owns_keys(focus.as_ref()) {
            return glib::Propagation::Proceed;
        }
        let Some(chord) = Chord::from_event(key, state) else {
            return glib::Propagation::Proceed;
        };
        let actions = keymap.borrow().actions_for(&chord);
        let Some(action) = (match actions.len() {
            0 => None,
            1 => Some(actions[0]),
            _ => {
                let next = cycle.get().wrapping_add(1) % actions.len();
                cycle.set(next);
                Some(actions[next])
            }
        }) else {
            return glib::Propagation::Proceed;
        };
        if action == Action::ShowShortcuts {
            shortcuts_dialog(&root_for_dialog, &keymap, &sender);
        } else {
            sender.input(message_for(action));
        }
        glib::Propagation::Stop
    });
    root.add_controller(keys);

    // Escape is not a bindable action: it just backs out of the crop tool or
    // the overlay selection. Bubble phase, so a focused entry clearing itself
    // wins first.
    let esc = gtk::EventControllerKey::new();
    {
        esc.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                esc_sender.input(Msg::Escape);
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
    }
    root.add_controller(esc);
}

/// The window's key controller, in the capture phase rather than the default
/// bubble one. A focused widget handles a key before a bubble-phase controller
/// on the window ever sees it, and GTK both activates a focused button on Space
/// and parks the initial focus on the first focusable widget it finds — so with
/// a document open, Space added an overlay instead of playing. Capture is the
/// only phase that gets there first; `focus_owns_keys` is what hands the
/// keystroke back to the widgets that genuinely own it.
fn shortcuts_controller() -> gtk::EventControllerKey {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys
}

/// Whether the focused widget keeps its own keystrokes rather than handing them
/// to the shortcuts. A text entry does, or typing "a" in a caption inserts an
/// arrow; so does anything inside a dialog, which has its own keys and its own
/// idea of Escape. A button does not: it would eat Space, which is play/pause.
fn focus_owns_keys(focus: Option<&gtk::Widget>) -> bool {
    focus.is_some_and(|f| {
        f.is::<gtk::Editable>()
            || f.is::<gtk::TextView>()
            || f.ancestor(adw::Dialog::static_type()).is_some()
    })
}

/// The keybinding editor, which is also the Ctrl+? shortcuts window: one list
/// that both shows what a key does and lets it be changed, rather than a
/// read-only window and a settings page saying the same things twice.
fn shortcuts_dialog(
    root: &adw::ApplicationWindow,
    keymap: &Rc<RefCell<Keymap>>,
    sender: &ComponentSender<App>,
) {
    let dialog = adw::PreferencesDialog::new();
    dialog.set_title(t("Keyboard shortcuts"));
    let page = adw::PreferencesPage::new();

    let working = Rc::new(RefCell::new(keymap.borrow().clone()));
    // Which row is waiting for a keypress, if any. Chords and canvas modifiers
    // capture the same way but read different halves of the event.
    let capturing: Rc<RefCell<Option<Action>>> = Rc::new(RefCell::new(None));
    let capturing_modal: Rc<RefCell<Option<Modal>>> = Rc::new(RefCell::new(None));
    let rows: Rc<RefCell<Vec<(Action, adw::ActionRow, gtk::Button)>>> =
        Rc::new(RefCell::new(Vec::new()));
    let modal_rows: Rc<RefCell<Vec<(Modal, gtk::Button)>>> = Rc::new(RefCell::new(Vec::new()));

    let refresh: Rc<dyn Fn()> = {
        let (working, rows, capturing) = (working.clone(), rows.clone(), capturing.clone());
        let (modal_rows, capturing_modal) = (modal_rows.clone(), capturing_modal.clone());
        Rc::new(move || {
            let map = working.borrow();
            let conflicts = map.conflicts();
            let waiting = *capturing.borrow();
            let waiting_modal = *capturing_modal.borrow();
            for (modal, button) in modal_rows.borrow().iter() {
                let mods = map.mods(*modal);
                button.set_label(&if waiting_modal == Some(*modal) {
                    // Translators: A canvas modifier row waiting for the user to hold a key.
                    t("Hold a modifier…").to_string()
                } else if mods.is_empty() {
                    t("Unassigned").to_string()
                } else {
                    mods.display()
                });
            }
            for (action, row, button) in rows.borrow().iter() {
                let chords: Vec<String> = map
                    .chords(*action)
                    .iter()
                    .map(crate::keymap::Chord::display)
                    .collect();
                button.set_label(&if waiting == Some(*action) {
                    t("Press a key…").to_string()
                } else if chords.is_empty() {
                    t("Unassigned").to_string()
                } else {
                    chords.join(", ")
                });
                let clash = conflicts.contains(action);
                set_class(button.upcast_ref(), "bind-conflict", clash);
                row.set_subtitle(if clash {
                    t("Already used by another action")
                } else {
                    ""
                });
            }
        })
    };

    let mut group: Option<adw::PreferencesGroup> = None;
    let mut last = "";
    for action in crate::keymap::ACTIONS {
        if action.group() != last {
            last = action.group();
            let g = adw::PreferencesGroup::builder().title(t(last)).build();
            page.add(&g);
            group = Some(g);
        }
        let row = adw::ActionRow::builder().title(t(action.label())).build();
        let button = gtk::Button::with_label("");
        button.set_valign(gtk::Align::Center);
        button.set_width_request(150);
        {
            let (capturing, capturing_modal, refresh) =
                (capturing.clone(), capturing_modal.clone(), refresh.clone());
            button.connect_clicked(move |_| {
                *capturing_modal.borrow_mut() = None;
                *capturing.borrow_mut() = Some(action);
                refresh();
            });
        }
        let clear = gtk::Button::from_icon_name("edit-clear-symbolic");
        clear.set_valign(gtk::Align::Center);
        clear.add_css_class("flat");
        clear.set_tooltip_text(Some(t("Unbind")));
        {
            let (working, refresh) = (working.clone(), refresh.clone());
            clear.connect_clicked(move |_| {
                working.borrow_mut().set(action, Vec::new());
                refresh();
            });
        }
        row.add_suffix(&button);
        row.add_suffix(&clear);
        if let Some(g) = &group {
            g.add(&row);
        }
        rows.borrow_mut().push((action, row, button));
    }

    // Canvas modifiers: held during a drag rather than pressed as a chord, so
    // they live in their own group and capture a bare modifier.
    let canvas_group = adw::PreferencesGroup::builder()
        .title(t("Canvas"))
        .description(t("Held while dragging an overlay's grip."))
        .build();
    for modal in MODALS {
        let row = adw::ActionRow::builder().title(t(modal.label())).build();
        let button = gtk::Button::with_label("");
        button.set_valign(gtk::Align::Center);
        button.set_width_request(150);
        {
            let (capturing, capturing_modal, refresh) =
                (capturing.clone(), capturing_modal.clone(), refresh.clone());
            button.connect_clicked(move |_| {
                *capturing.borrow_mut() = None;
                *capturing_modal.borrow_mut() = Some(modal);
                refresh();
            });
        }
        let clear = gtk::Button::from_icon_name("edit-clear-symbolic");
        clear.set_valign(gtk::Align::Center);
        clear.add_css_class("flat");
        clear.set_tooltip_text(Some(t("Unbind")));
        {
            let (working, refresh, sender) = (working.clone(), refresh.clone(), sender.clone());
            clear.connect_clicked(move |_| {
                working.borrow_mut().set_mods(modal, Mods::default());
                refresh();
                sender.input(Msg::SetKeymap(Box::new(working.borrow().clone())));
            });
        }
        row.add_suffix(&button);
        row.add_suffix(&clear);
        canvas_group.add(&row);
        modal_rows.borrow_mut().push((modal, button));
    }
    page.add(&canvas_group);

    // Capture runs in the capture phase, so the dialog's own widgets never see
    // the keystroke being bound.
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let (working, capturing, refresh) = (working.clone(), capturing.clone(), refresh.clone());
        let sender = sender.clone();
        let capturing_modal = capturing_modal.clone();
        keys.connect_key_pressed(move |_, key, _, state| {
            if let Some(modal) = *capturing_modal.borrow() {
                if key == gdk::Key::Escape {
                    *capturing_modal.borrow_mut() = None;
                    refresh();
                    return glib::Propagation::Stop;
                }
                // Anything that is not a modifier is not an answer to this
                // question, so it is swallowed rather than bound.
                let Some(mods) = Mods::from_event(key, state) else {
                    return glib::Propagation::Stop;
                };
                working.borrow_mut().set_mods(modal, mods);
                *capturing_modal.borrow_mut() = None;
                refresh();
                sender.input(Msg::SetKeymap(Box::new(working.borrow().clone())));
                return glib::Propagation::Stop;
            }
            let Some(action) = *capturing.borrow() else {
                return glib::Propagation::Proceed;
            };
            if key == gdk::Key::Escape {
                *capturing.borrow_mut() = None;
                refresh();
                return glib::Propagation::Stop;
            }
            let Some(chord) = Chord::from_event(key, state) else {
                return glib::Propagation::Stop;
            };
            working.borrow_mut().set(action, vec![chord]);
            *capturing.borrow_mut() = None;
            refresh();
            sender.input(Msg::SetKeymap(Box::new(working.borrow().clone())));
            glib::Propagation::Stop
        });
    }
    dialog.add_controller(keys);

    let file_group = adw::PreferencesGroup::builder()
        .title(t("Keybindings file"))
        .description(t("Bindings are saved as you change them."))
        .build();
    let file_row = adw::ActionRow::builder()
        .title(t("Import or export"))
        .subtitle(
            crate::keymap::path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| t("no config directory").into()),
        )
        .build();
    let import = gtk::Button::with_label(t("Import…"));
    let export = gtk::Button::with_label(t("Export…"));
    let reset = gtk::Button::with_label(t("Reset"));
    for b in [&import, &export, &reset] {
        b.set_valign(gtk::Align::Center);
        file_row.add_suffix(b);
    }
    {
        let (working, refresh, sender, parent) = (
            working.clone(),
            refresh.clone(),
            sender.clone(),
            root.clone(),
        );
        import.connect_clicked(move |_| {
            let parent = parent.clone();
            let (working, refresh, sender) = (working.clone(), refresh.clone(), sender.clone());
            gtk::FileDialog::builder()
                .title(t("Import keybindings"))
                .build()
                .open(Some(&parent), gio::Cancellable::NONE, move |res| {
                    let Some(path) = res.ok().and_then(|f| f.path()) else {
                        return;
                    };
                    match std::fs::read_to_string(&path) {
                        Ok(text) => {
                            working.borrow_mut().apply(&text);
                            refresh();
                            sender.input(Msg::SetKeymap(Box::new(working.borrow().clone())));
                        }
                        Err(e) => sender.input(Msg::Toast(fill(
                            t("Could not read {path}: {error}"),
                            &[
                                ("path", &path.display().to_string()),
                                ("error", &e.to_string()),
                            ],
                        ))),
                    }
                });
        });
    }
    {
        let (working, sender, parent) = (working.clone(), sender.clone(), root.clone());
        export.connect_clicked(move |_| {
            let parent = parent.clone();
            let (working, sender) = (working.clone(), sender.clone());
            let save = gtk::FileDialog::builder()
                .title(t("Export keybindings"))
                .build();
            save.set_initial_name(Some("keybindings.conf"));
            save.save(Some(&parent), gio::Cancellable::NONE, move |res| {
                let Some(path) = res.ok().and_then(|f| f.path()) else {
                    return;
                };
                let text = working.borrow().to_text();
                match std::fs::write(&path, text) {
                    Ok(()) => sender.input(Msg::Toast(fill(
                        t("Saved {path}"),
                        &[("path", &path.display().to_string())],
                    ))),
                    Err(e) => sender.input(Msg::Toast(fill(
                        t("Could not write: {error}"),
                        &[("error", &e.to_string())],
                    ))),
                }
            });
        });
    }
    {
        let (working, refresh, sender) = (working.clone(), refresh.clone(), sender.clone());
        reset.connect_clicked(move |_| {
            *working.borrow_mut() = Keymap::default();
            refresh();
            sender.input(Msg::SetKeymap(Box::new(working.borrow().clone())));
        });
    }
    file_group.add(&file_row);
    page.add(&file_group);

    dialog.add(&page);
    refresh();
    dialog.present(Some(root));
}

fn build(root: &adw::ApplicationWindow, model: &App, sender: &ComponentSender<App>) -> Widgets {
    let keymap = model.keymap.clone();
    let title = adw::WindowTitle::new("Untitled", "");
    let header = adw::HeaderBar::builder().title_widget(&title).build();

    let history_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    history_box.add_css_class("linked");
    let undo = icon_button("edit-undo-symbolic", "Undo (Ctrl+Z)");
    let redo = icon_button("edit-redo-symbolic", "Redo (Ctrl+Shift+Z)");
    connect(&undo, sender, || Msg::Undo);
    connect(&redo, sender, || Msg::Redo);
    history_box.append(&undo);
    history_box.append(&redo);
    header.pack_start(&history_box);

    let export = gtk::Button::with_label(t("Export"));
    export.add_css_class("suggested-action");
    export.set_tooltip_text(Some(&keymap.borrow().tip(t("Export GIF"), Action::Export)));
    connect(&export, sender, || Msg::Export);
    header.pack_end(&export);

    let menu = gio::Menu::new();
    let file = gio::Menu::new();
    file.append(Some(t("Open…")), Some("win.open"));
    file.append(Some(t("Export GIF…")), Some("win.export"));
    // Translators: Decodes another video or GIF and appends its frames, so two
    // clips can be mixed into one timeline.
    file.append(Some(t("Add frames from file…")), Some("win.import-more"));
    menu.append_section(None, &file);

    let insert = gio::Menu::new();
    for (tool, action) in [
        (Tool::Text, "win.insert-text"),
        (Tool::Rect, "win.insert-rect"),
        (Tool::Ellipse, "win.insert-ellipse"),
        (Tool::Arrow, "win.insert-arrow"),
    ] {
        insert.append(Some(t(tool_label(tool))), Some(action));
    }
    menu.append_submenu(Some(t("Insert")), &insert);

    let image = gio::Menu::new();
    image.append(Some(t("Resize frames…")), Some("win.optimize-resize"));
    // Translators: Like the Zoom and resize button, but ignoring the frame
    // scope — needs a box drawn with the crop tool first.
    image.append(
        Some(t("Zoom and resize all frames")),
        Some("win.optimize-zoom-all"),
    );
    menu.append_submenu(Some(t("Image")), &image);

    // Everything that makes the GIF smaller, in one place. "Halve frame rate"
    // used to sit in the frame menu pretending to be a rate control; it was
    // deleting every second frame, which is what these say they do.
    let optimize = gio::Menu::new();
    optimize.append(Some(t("Remove frames…")), Some("win.optimize-remove"));
    optimize.append(Some(t("Smart remove frames…")), Some("win.optimize-smart"));
    menu.append_submenu(Some(t("Optimize")), &optimize);

    let settings_menu = gio::Menu::new();
    settings_menu.append(Some(t("Keyboard shortcuts…")), Some("win.shortcuts"));
    settings_menu.append(Some(t("About")), Some("win.about"));
    menu.append_section(None, &settings_menu);
    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu)
        .build();
    no_focus_steal(&menu_button);
    header.pack_end(&menu_button);

    // Empty state doubles as the welcome screen.
    let status = adw::StatusPage::builder()
        .icon_name("image-x-generic-symbolic")
        .title(t("No document"))
        .description(t("Open a video or GIF to start editing"))
        .build();
    let welcome_buttons = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    welcome_buttons.set_halign(gtk::Align::Center);
    let open_button = gtk::Button::with_label(t("Open…"));
    open_button.add_css_class("pill");
    open_button.add_css_class("suggested-action");
    connect(&open_button, sender, || Msg::Open);
    // A missing piece disables the action with the reason attached to it,
    // rather than failing after the user has picked a file.
    if let Some(reason) = model.caps.import_blocker() {
        open_button.set_sensitive(false);
        open_button.set_tooltip_text(Some(t(reason)));
    }
    let record_button = gtk::Button::with_label(t("Record"));
    record_button.add_css_class("pill");
    record_button.set_sensitive(false);
    // Translators: Screen recording. Not implemented yet, so the button is disabled.
    record_button.set_tooltip_text(Some(t(model
        .caps
        .record_blocker()
        .unwrap_or(n("Screen recording is not wired up yet")))));
    welcome_buttons.append(&record_button);
    welcome_buttons.append(&open_button);

    // The decode and the frame work run off the main thread, so without a bar
    // here a long job is indistinguishable from a hang. It lives in the
    // toolbar so it is visible with a document open, not only on the welcome
    // page.
    let import_bar = gtk::ProgressBar::builder()
        .show_text(true)
        .visible(false)
        .build();
    import_bar.set_size_request(320, -1);
    let import_cancel = gtk::Button::from_icon_name("window-close-symbolic");
    import_cancel.add_css_class("circular");
    import_cancel.add_css_class("flat");
    import_cancel.set_valign(gtk::Align::Center);
    import_cancel.set_visible(false);
    // Translators: Tooltip on the X beside the import progress bar; it cancels the running decode.
    import_cancel.set_tooltip_text(Some(t("Cancel import")));
    connect(&import_cancel, sender, || Msg::CancelImport);
    // The X sits to the right of the bar, which carries the message; one row
    // so the toolbar takes or drops them together.
    let import_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    import_row.set_halign(gtk::Align::Center);
    import_row.append(&import_bar);
    import_row.append(&import_cancel);
    let welcome = gtk::Box::new(gtk::Orientation::Vertical, 18);
    welcome.append(&welcome_buttons);
    status.set_child(Some(&welcome));

    // Left rail. These place a default-sized overlay under the current scope;
    // the canvas handles do the rest.
    let rail = gtk::Box::new(gtk::Orientation::Vertical, 4);
    rail.set_margin_all(6);
    let mut tool_buttons = Vec::new();
    {
        let button = gtk::Button::from_icon_name(tool_icon_name(Tool::Text));
        button.add_css_class("flat");
        connect(&button, sender, || Msg::AddOverlay(Tool::Text));
        rail.append(&button);
        tool_buttons.push((Tool::Text, button));
    }
    // Rect, Ellipse and Arrow share one button: the icon and the click both
    // follow whichever of the three was used last (Impasto's `ToolBoxWidget`
    // groups related tools the same way), and the dropdown arrow opens a
    // flyout to pick a different one.
    let shape_tool = Rc::new(Cell::new(Tool::Rect));
    let shape_button = adw::SplitButton::builder()
        .icon_name(tool_icon_name(shape_tool.get()))
        .build();
    shape_button.add_css_class("flat");
    no_focus_steal(&shape_button);
    {
        let (sender, shape_tool) = (sender.clone(), shape_tool.clone());
        shape_button.connect_clicked(move |_| sender.input(Msg::AddOverlay(shape_tool.get())));
    }
    let shape_popover = gtk::Popover::new();
    let shape_list = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    for tool in [Tool::Rect, Tool::Ellipse, Tool::Arrow] {
        let item = gtk::Button::from_icon_name(tool_icon_name(tool));
        item.add_css_class("flat");
        item.set_tooltip_text(Some(t(tool_label(tool))));
        no_focus_steal(&item);
        let (sender, shape_tool, shape_button, shape_popover) = (
            sender.clone(),
            shape_tool.clone(),
            shape_button.clone(),
            shape_popover.clone(),
        );
        item.connect_clicked(move |_| {
            shape_tool.set(tool);
            shape_button.set_icon_name(tool_icon_name(tool));
            sender.input(Msg::AddOverlay(tool));
            shape_popover.popdown();
        });
        shape_list.append(&item);
    }
    shape_popover.set_child(Some(&shape_list));
    shape_button.set_popover(Some(&shape_popover));
    rail.append(&shape_button);
    let crop_button = gtk::ToggleButton::builder()
        .icon_name("tool-crop-symbolic")
        .build();
    crop_button.add_css_class("flat");
    no_focus_steal(&crop_button);
    {
        let sender = sender.clone();
        crop_button.connect_clicked(move |_| sender.input(Msg::ToggleCropTool));
    }
    rail.append(&crop_button);

    let canvas = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::ScaleDown)
        .can_shrink(true)
        .build();

    // Handles and the crop box live in a transparent layer over the picture,
    // which keeps the picture's texture path and gives the gestures one widget
    // whose size is the thing being mapped from.
    let canvas_state: Rc<RefCell<CanvasState>> = Rc::new(RefCell::new(CanvasState::default()));
    let canvas_overlay = gtk::DrawingArea::new();
    {
        let canvas_state = canvas_state.clone();
        canvas_overlay.set_draw_func(move |area, cr, w, h| {
            draw_canvas_overlay(area, cr, w as f64, h as f64, &canvas_state.borrow());
        });
    }
    install_canvas_gestures(&canvas_overlay, &canvas_state, sender);

    let stack_overlay = gtk::Overlay::new();
    stack_overlay.set_child(Some(&canvas));
    stack_overlay.add_overlay(&canvas_overlay);

    let canvas_frame = gtk::Frame::builder().child(&stack_overlay).build();
    canvas_frame.add_css_class("canvas-frame");
    canvas_frame.set_margin_all(12);
    canvas_frame.set_hexpand(true);
    canvas_frame.set_vexpand(true);

    let canvas_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    canvas_row.append(&rail);
    canvas_row.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    canvas_row.append(&canvas_frame);

    // Footer: transport, scope, bands, strip.
    let footer = gtk::Box::new(gtk::Orientation::Vertical, 6);
    footer.set_margin_all(8);

    let frame_menu = gio::Menu::new();
    frame_menu.append(Some(t("Delete")), Some("win.frame-delete"));
    frame_menu.append(Some(t("Duplicate")), Some("win.frame-duplicate"));
    frame_menu.append(Some(t("Cut")), Some("win.frame-cut"));
    frame_menu.append(Some(t("Copy")), Some("win.frame-copy"));
    frame_menu.append(Some(t("Paste")), Some("win.frame-paste"));
    frame_menu.append(Some(t("Reverse")), Some("win.frame-reverse"));
    frame_menu.append(
        Some(t("Set delay for all frames…")),
        Some("win.frame-delay-all"),
    );
    let frame_menu_button = gtk::MenuButton::builder()
        .icon_name("view-more-symbolic")
        .menu_model(&frame_menu)
        .tooltip_text(t("Frame operations"))
        .build();
    no_focus_steal(&frame_menu_button);

    let transport = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let play = icon_button("media-playback-start-symbolic", "Play/pause (Space)");
    connect(&play, sender, || Msg::TogglePlay);
    let time = gtk::Label::new(Some("00:00.0 / 00:00.0 · 0 fps"));
    time.add_css_class("tnum");
    time.add_css_class("dim-label");
    transport.append(&play);
    transport.append(&time);

    let scope_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    scope_box.add_css_class("linked");
    scope_box.set_halign(gtk::Align::End);
    scope_box.set_hexpand(true);
    let this_frame = gtk::ToggleButton::with_label(t("This frame"));
    let all_frames = gtk::ToggleButton::with_label(t("All frames"));
    let range = gtk::ToggleButton::with_label(t("Range"));
    all_frames.set_group(Some(&this_frame));
    range.set_group(Some(&this_frame));
    this_frame.set_active(true);
    range.set_visible(false);
    for (button, choice) in [
        (&this_frame, ScopeChoice::ThisFrame),
        (&all_frames, ScopeChoice::AllFrames),
        (&range, ScopeChoice::Range),
    ] {
        let sender = sender.clone();
        button.connect_toggled(move |b| {
            if b.is_active() {
                sender.input(Msg::SetScope(choice));
            }
        });
        no_focus_steal(button);
        scope_box.append(button);
    }
    transport.append(&scope_box);
    transport.append(&frame_menu_button);
    footer.append(&transport);

    let strip = gtk::Box::new(gtk::Orientation::Horizontal, THUMB_SPACING);
    let scope_mirror: Rc<RefCell<Vec<usize>>> = Rc::new(RefCell::new(Vec::new()));
    let drop_dividers: Rc<RefCell<Vec<gtk::Widget>>> = Rc::new(RefCell::new(Vec::new()));
    // One drop target for the whole strip, not one per cell: it tracks the
    // gap nearest the pointer as it moves and paints the divider there, so a
    // release lands exactly where the line promised rather than leaving the
    // player to guess between "before" and "after" the cell under it.
    let drop_target = gtk::DropTarget::new(u32::static_type(), gdk::DragAction::MOVE);
    {
        let (strip, drop_dividers) = (strip.clone(), drop_dividers.clone());
        drop_target.connect_motion(move |_, x, _y| {
            mark_drop_gap(&drop_dividers.borrow(), Some(gap_at(&strip, x)));
            gdk::DragAction::MOVE
        });
    }
    {
        let drop_dividers = drop_dividers.clone();
        drop_target.connect_leave(move |_| mark_drop_gap(&drop_dividers.borrow(), None));
    }
    {
        let (sender, strip, drop_dividers) = (sender.clone(), strip.clone(), drop_dividers.clone());
        drop_target.connect_drop(move |_, value, x, _y| {
            let Ok(from) = value.get::<u32>() else {
                return false;
            };
            let gap = gap_at(&strip, x);
            mark_drop_gap(&drop_dividers.borrow(), None);
            sender.input(Msg::MoveSelectionTo {
                from: from as usize,
                gap,
            });
            true
        });
    }
    strip.add_controller(drop_target);

    let bands_model: Rc<RefCell<Vec<Band>>> = Rc::new(std::cell::RefCell::new(Vec::new()));
    let bands = gtk::DrawingArea::new();
    bands.set_content_height(0);
    {
        let bands_model = bands_model.clone();
        let strip_pitch = model.strip_pitch.clone();
        bands.set_draw_func(move |area, cr, _, _| {
            draw_bands(area, cr, &bands_model.borrow(), strip_pitch.get());
        });
    }
    let band_click = gtk::GestureClick::new();
    band_click.set_button(gdk::BUTTON_PRIMARY);
    {
        let bands_model = bands_model.clone();
        let sender = sender.clone();
        let strip_pitch = model.strip_pitch.clone();
        band_click.connect_pressed(move |_, _, x, y| {
            let hit = band_at(&bands_model.borrow(), x, y, strip_pitch.get());
            sender.input(Msg::SelectOverlay(hit));
        });
    }
    bands.add_controller(band_click);

    // Right-click a band to act on that overlay: in the strip the band *is*
    // the overlay, so deleting it there beats a trip through the sidebar.
    // Picking it first means the menu always names what is under the pointer.
    let band_menu = gio::Menu::new();
    band_menu.append(Some(t("Delete overlay")), Some("overlay.delete"));
    let band_popover = gtk::PopoverMenu::from_model(Some(&band_menu));
    band_popover.set_parent(&bands);
    band_popover.set_has_arrow(false);
    {
        let band_popover = band_popover.clone();
        bands.connect_destroy(move |_| band_popover.unparent());
    }
    {
        let group = gio::SimpleActionGroup::new();
        let action = gio::SimpleAction::new("delete", None);
        let sender = sender.clone();
        action.connect_activate(move |_, _| sender.input(Msg::DeleteSelection));
        group.add_action(&action);
        bands.insert_action_group("overlay", Some(&group));
    }
    let band_secondary = gtk::GestureClick::new();
    band_secondary.set_button(gdk::BUTTON_SECONDARY);
    {
        let bands_model = bands_model.clone();
        let sender = sender.clone();
        let strip_pitch = model.strip_pitch.clone();
        let band_popover = band_popover.clone();
        band_secondary.connect_pressed(move |_, _, x, y| {
            // Empty space under the pointer: no menu at all, rather than one
            // that would delete whatever happened to be selected elsewhere.
            let Some(id) = band_at(&bands_model.borrow(), x, y, strip_pitch.get()) else {
                return;
            };
            sender.input(Msg::SelectOverlay(Some(id)));
            band_popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            band_popover.popup();
        });
    }
    bands.add_controller(band_secondary);

    // The band list can outgrow the strip, so it scrolls in its own right and
    // collapses to a few rows until asked to expand.
    let bands_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::External)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .propagate_natural_height(true)
        .child(&bands)
        .build();
    bands_scroll.set_max_content_height((BANDS_COLLAPSED_ROWS as f64 * BAND_H) as i32);
    bands_scroll.set_visible(false);
    let bands_expander = gtk::Button::builder().visible(false).build();
    bands_expander.add_css_class("flat");
    bands_expander.add_css_class("caption");
    bands_expander.set_halign(gtk::Align::Start);
    connect(&bands_expander, sender, || Msg::ToggleBandsExpanded);

    // Frames first, then the overlays that annotate them: the bands are a
    // legend under the strip, not a header over it.
    let strip_column = gtk::Box::new(gtk::Orientation::Vertical, 4);
    strip_column.append(&strip);
    strip_column.append(&bands_scroll);
    strip_column.append(&bands_expander);
    let strip_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::External)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .child(&strip_column)
        .build();
    // The footer asks for what the strip actually needs, so this has to carry
    // the strip's own height out rather than reporting a scrollable minimum.
    strip_scroll.set_propagate_natural_height(true);
    dont_chase_focus(&strip_scroll);
    // The horizontal scrollbar is a sibling rather than the scrolled window's
    // own. A non-overlay `Automatic` scrollbar takes its height out of the
    // viewport without adding it to what the scrolled window measures, so
    // wherever the footer sat at its minimum height — which is where a
    // `GtkPaned` end child lands — the strip lost exactly the row of frame
    // numbers under the thumbnails, with no scrollbar to reach it by. Out
    // here it is measured along with everything else, and it only takes
    // space while the strip really is wider than the window.
    let strip_bar = gtk::Scrollbar::new(
        gtk::Orientation::Horizontal,
        Some(&strip_scroll.hadjustment()),
    );
    strip_bar.set_visible(false);
    {
        let bar = strip_bar.clone();
        strip_scroll.hadjustment().connect_changed(move |adj| {
            bar.set_visible(adj.upper() > adj.page_size() + 1.0);
        });
    }

    // Ctrl+wheel over the strip zooms the thumbnails; a plain wheel still
    // scrolls it. Capture phase so it beats the scrolled window's own handler.
    let strip_wheel = gtk::EventControllerScroll::new(
        gtk::EventControllerScrollFlags::VERTICAL | gtk::EventControllerScrollFlags::DISCRETE,
    );
    strip_wheel.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let sender = sender.clone();
        strip_wheel.connect_scroll(move |controller, _, dy| {
            if !controller
                .current_event_state()
                .contains(gdk::ModifierType::CONTROL_MASK)
                || dy == 0.0
            {
                return glib::Propagation::Proceed;
            }
            let factor = if dy < 0.0 {
                STRIP_ZOOM_STEP
            } else {
                1.0 / STRIP_ZOOM_STEP
            };
            sender.input(Msg::StripZoom(factor));
            glib::Propagation::Stop
        });
    }
    strip_scroll.add_controller(strip_wheel);
    footer.append(&strip_scroll);
    footer.append(&strip_bar);

    // No fixed split: the footer asks for what the strip and its bands
    // actually need, and the canvas gets everything else. Adding an overlay
    // row costs the canvas one row, not a third of the window.
    let paned = gtk::Paned::builder()
        .orientation(gtk::Orientation::Vertical)
        .start_child(&canvas_row)
        .end_child(&footer)
        .resize_start_child(true)
        .resize_end_child(false)
        .shrink_end_child(false)
        .build();

    // Properties: one contextual page, an AdwPreferencesGroup so the rows come
    // out consistent for free.
    let properties = gtk::Box::new(gtk::Orientation::Vertical, 12);
    properties.set_margin_all(12);
    properties.set_size_request(280, -1);
    let text_group = adw::PreferencesGroup::builder()
        .title(t("Properties"))
        .build();
    // Every sidebar setter fires its own notify handler, so `update_view` holds
    // this up to say "this value came from the model, do not send it back".
    let sync = Rc::new(Cell::new(false));
    let text_entry = gtk::Entry::builder()
        .placeholder_text(t("Overlay text"))
        .build();
    {
        let (sender, sync) = (sender.clone(), sync.clone());
        text_entry.connect_changed(move |entry| {
            if sync.get() {
                return;
            }
            sender.input(Msg::EditText(entry.text().to_string()));
        });
    }
    let text_row = adw::ActionRow::builder().title(t("Text")).build();
    text_row.add_suffix(&text_entry);
    text_row.set_visible(false);
    text_group.add(&text_row);

    // Overlay styling. Font choice covers bold and italic, because a font
    // description already carries weight and style: two toggles that fight the
    // font dialog over the same field would be the bug, not the feature.
    let overlay_group = adw::PreferencesGroup::builder()
        .title(t("Overlay"))
        .visible(false)
        .build();
    // Deleting the overlay belongs with the controls that edit it — the same
    // one the band menu in the strip acts on. Destructive, so it takes the
    // accent-free red styling rather than sitting in the row list where a
    // stray click while changing a colour would land on it.
    let overlay_delete = gtk::Button::from_icon_name("user-trash-symbolic");
    overlay_delete.set_valign(gtk::Align::Center);
    overlay_delete.add_css_class("flat");
    overlay_delete.add_css_class("destructive-action");
    overlay_delete.set_tooltip_text(Some(t("Delete overlay")));
    connect(&overlay_delete, sender, || Msg::DeleteSelection);
    overlay_group.set_header_suffix(Some(&overlay_delete));
    let font_button = gtk::FontDialogButton::new(Some(gtk::FontDialog::new()));
    font_button.set_valign(gtk::Align::Center);
    {
        let (sender, sync) = (sender.clone(), sync.clone());
        font_button.connect_font_desc_notify(move |b| {
            if let Some(desc) = b.font_desc()
                && !sync.get()
            {
                sender.input(Msg::SetOverlayProp(OverlayProp::Font(
                    desc.to_str().to_string(),
                )));
            }
        });
    }
    let font_row = suffixed(t("Font"), &font_button);

    let text_size = gtk::SpinButton::with_range(4.0, 512.0, 1.0);
    text_size.set_valign(gtk::Align::Center);
    {
        let (sender, sync) = (sender.clone(), sync.clone());
        text_size.connect_value_changed(move |spin| {
            if sync.get() {
                return;
            }
            sender.input(Msg::SetOverlayProp(OverlayProp::TextSize(
                spin.value() as f32
            )));
        });
    }
    let size_row = suffixed(t("Size"), &text_size);

    // Four icons rather than a dropdown: alignment is the one text property
    // that reads faster as a picture than as a word.
    let align_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    align_box.set_valign(gtk::Align::Center);
    align_box.add_css_class("linked");
    let mut align_buttons: Vec<(TextAlign, gtk::ToggleButton)> = Vec::new();
    for (align, icon, tip) in [
        (TextAlign::Left, "format-justify-left-symbolic", t("Left")),
        (
            TextAlign::Center,
            "format-justify-center-symbolic",
            t("Centered"),
        ),
        (
            TextAlign::Right,
            "format-justify-right-symbolic",
            t("Right"),
        ),
        (
            TextAlign::Justify,
            "format-justify-fill-symbolic",
            // Translators: Text alignment that stretches every line but the last to both margins.
            t("Justified"),
        ),
    ] {
        let button = gtk::ToggleButton::new();
        button.set_icon_name(icon);
        button.set_tooltip_text(Some(tip));
        if let Some((_, first)) = align_buttons.first() {
            button.set_group(Some(first));
        }
        {
            let (sender, sync) = (sender.clone(), sync.clone());
            button.connect_toggled(move |b| {
                if sync.get() || !b.is_active() {
                    return;
                }
                sender.input(Msg::SetOverlayProp(OverlayProp::Align(align)));
            });
        }
        align_box.append(&button);
        align_buttons.push((align, button));
    }
    let align_row = suffixed(t("Align"), &align_box);

    let antialias = gtk::Switch::new();
    antialias.set_valign(gtk::Align::Center);
    {
        let (sender, sync) = (sender.clone(), sync.clone());
        antialias.connect_active_notify(move |sw| {
            if sync.get() {
                return;
            }
            sender.input(Msg::SetOverlayProp(OverlayProp::Antialias(sw.is_active())));
        });
    }
    // Translators: Smoothing of glyph edges. Off gives hard pixel edges, for pixel-art captures.
    let antialias_row = suffixed(t("Smooth edges"), &antialias);

    let fill_button = color_button();
    let fill_on = gtk::Switch::new();
    fill_on.set_valign(gtk::Align::Center);
    let fill_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    fill_box.set_valign(gtk::Align::Center);
    fill_box.append(&fill_on);
    fill_box.append(&fill_button);
    // Translators: The interior colour of a shape, or the colour of the glyphs for text.
    let fill_row = suffixed(t("Fill"), &fill_box);

    let outline_button = color_button();
    let outline_width = width_spin();
    let outline_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    outline_box.set_valign(gtk::Align::Center);
    outline_box.append(&outline_width);
    outline_box.append(&outline_button);
    // Translators: The contrasting edge drawn behind text so it stays readable over any image.
    let outline_row = suffixed(t("Outline"), &outline_box);

    let stroke_button = color_button();
    let stroke_width = width_spin();
    let stroke_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    stroke_box.set_valign(gtk::Align::Center);
    stroke_box.append(&stroke_width);
    stroke_box.append(&stroke_button);
    // Translators: The outline of a shape, as distinct from the outline behind text.
    let stroke_row = suffixed(t("Stroke"), &stroke_box);

    for row in [
        &font_row,
        &size_row,
        &align_row,
        &fill_row,
        &outline_row,
        &antialias_row,
        &stroke_row,
    ] {
        overlay_group.add(row);
    }
    // A width of zero is how the outline and the stroke are turned off; a
    // separate switch for each would be a second way to say the same thing.
    {
        let sender = sender.clone();
        connect_pair(&outline_button, &outline_width, &sync, move |v| {
            sender.input(Msg::SetOverlayProp(OverlayProp::Outline(v)));
        });
    }
    {
        let sender = sender.clone();
        connect_pair(&stroke_button, &stroke_width, &sync, move |v| {
            sender.input(Msg::SetOverlayProp(OverlayProp::Stroke(v)));
        });
    }
    {
        let (sender, switch, sync) = (sender.clone(), fill_on.clone(), sync.clone());
        fill_button.connect_rgba_notify(move |b| {
            if sync.get() {
                return;
            }
            let colour = rgba_bytes(b.rgba());
            // The switch is hidden for text, where a fill is not optional.
            let on = !switch.is_visible() || switch.is_active();
            sender.input(Msg::SetOverlayProp(OverlayProp::Fill(on.then_some(colour))));
        });
    }
    {
        let (sender, button, sync) = (sender.clone(), fill_button.clone(), sync.clone());
        fill_on.connect_active_notify(move |switch| {
            if sync.get() {
                return;
            }
            let colour = rgba_bytes(button.rgba());
            sender.input(Msg::SetOverlayProp(OverlayProp::Fill(
                switch.is_active().then_some(colour),
            )));
        });
    }

    let crop_group = adw::PreferencesGroup::builder()
        .title(t("Crop / zoom"))
        .visible(false)
        .build();
    let crop_label = gtk::Label::new(None);
    crop_label.add_css_class("dim-label");
    crop_label.set_wrap(true);
    crop_label.set_xalign(0.0);
    let crop_buttons = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let crop_apply = gtk::Button::with_label(t("Crop all frames"));
    crop_apply.set_tooltip_text(Some(t(
        "Crops every frame in the document to this box, shrinking the canvas.",
    )));
    // Translators: Zoom follows the frame scope — the frame on screen, or the
    // selected frames when there is a selection. "Zoom and resize all frames"
    // in the Optimize menu ignores the scope and takes the whole document.
    let zoom_apply = gtk::Button::with_label(t("Zoom and resize"));
    zoom_apply.set_tooltip_text(Some(t(
        "Fills the canvas from this box, on the frame on screen or the selected frames.",
    )));
    // Translators: Crops the frame(s) in scope in place, leaving the rest of
    // each one transparent instead of scaling the kept region back up.
    let shrink_apply = gtk::Button::with_label(t("Crop and keep size"));
    shrink_apply.set_tooltip_text(Some(t(
        "Crops this box on every frame in scope, in place, without scaling it \
         up, and blanks the rest of each frame to transparent.",
    )));
    connect(&crop_apply, sender, || Msg::ApplyCrop);
    connect(&zoom_apply, sender, || Msg::ApplyZoom);
    connect(&shrink_apply, sender, || Msg::ApplyShrink);
    crop_buttons.append(&crop_apply);
    crop_buttons.append(&zoom_apply);
    crop_buttons.append(&shrink_apply);
    let crop_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    crop_box.append(&crop_label);
    crop_box.append(&crop_buttons);
    crop_group.add(&crop_box);

    // Frame view: the delay of the frame(s) in scope, then a picker for the
    // overlays sitting on the frame on screen. `overlay_group` below edits
    // whichever overlay is picked. `update_view` swaps this group's title and
    // the delay row's subtitle to a "N frames selected" summary, and hides
    // the overlay picker and the overlay/text editors below, whenever the
    // scope names more than one frame: editing an overlay's own properties,
    // or picking which overlay sits on "the" frame, has no single answer
    // once more than one frame is in play.
    let frame_group = adw::PreferencesGroup::builder().title(t("Frame")).build();
    let frame_delay = gtk::SpinButton::with_range(1.0, u16::MAX as f64, 1.0);
    frame_delay.set_valign(gtk::Align::Center);
    {
        let (sender, sync) = (sender.clone(), sync.clone());
        frame_delay.connect_value_changed(move |spin| {
            if sync.get() {
                return;
            }
            sender.input(Msg::SetScopeDelay(spin.value() as u16));
        });
    }
    // Translators: Per-frame hold time, in centiseconds (1/100 s).
    let delay_row = suffixed(t("Delay (cs)"), &frame_delay);
    frame_group.add(&delay_row);

    let overlay_list_group = adw::PreferencesGroup::builder()
        .title(t("Overlays on this frame"))
        .visible(false)
        .build();
    let overlay_list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .build();
    overlay_list.add_css_class("boxed-list");
    let overlay_list_ids: Rc<RefCell<Vec<OverlayId>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let (sender, sync, ids) = (sender.clone(), sync.clone(), overlay_list_ids.clone());
        overlay_list.connect_row_selected(move |_, row| {
            if sync.get() {
                return;
            }
            let picked = row.and_then(|r| ids.borrow().get(r.index() as usize).copied());
            sender.input(Msg::SelectOverlay(picked));
        });
    }
    // Past a handful of layers the list scrolls rather than pushing the
    // overlay editor below it off the panel, the same bargain the strip's
    // band area makes. The cap is a row height measured off the real rows
    // (`update_view`), not a guess at what an ActionRow comes out as.
    let overlay_list_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .propagate_natural_height(true)
        .child(&overlay_list)
        .build();
    overlay_list_group.add(&overlay_list_scroll);

    let doc_info = gtk::Label::new(Some(""));
    doc_info.add_css_class("dim-label");
    doc_info.set_wrap(true);
    doc_info.set_xalign(0.0);
    properties.append(&frame_group);
    properties.append(&overlay_list_group);
    properties.append(&text_group);
    properties.append(&overlay_group);
    properties.append(&crop_group);
    properties.append(&doc_info);

    // The panel is taller than a short window whenever a text overlay's
    // editor is up, so it scrolls; without this the rows below the fold —
    // the colour pickers, the crop buttons, the document summary — were
    // simply unreachable.
    let properties_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .propagate_natural_width(true)
        .child(&properties)
        .build();
    let split = adw::OverlaySplitView::builder()
        .sidebar_position(gtk::PackType::End)
        .content(&paned)
        .sidebar(&properties_scroll)
        .build();

    let breakpoint = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        900.0,
        adw::LengthUnit::Px,
    ));
    breakpoint.add_setter(&split, "collapsed", Some(&true.to_value()));
    root.add_breakpoint(breakpoint);

    let stack = gtk::Stack::new();
    stack.add_named(&status, Some("empty"));
    stack.add_named(&split, Some("editor"));

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.add_top_bar(&import_row);
    toolbar.set_content(Some(&stack));

    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&toolbar));
    root.set_content(Some(&toasts));

    let open_action = gio::SimpleAction::new("open", None);
    {
        let sender = sender.clone();
        open_action.connect_activate(move |_, _| sender.input(Msg::Open));
    }
    let actions = gio::SimpleActionGroup::new();
    actions.add_action(&open_action);
    for (name, op) in [
        ("frame-delete", FrameOp::Delete),
        ("frame-duplicate", FrameOp::Duplicate),
        ("frame-reverse", FrameOp::Reverse),
        ("frame-cut", FrameOp::Cut),
    ] {
        let action = gio::SimpleAction::new(name, None);
        let sender = sender.clone();
        action.connect_activate(move |_, _| sender.input(Msg::FrameOp(op)));
        actions.add_action(&action);
    }
    for (name, make) in [
        ("export", (|| Msg::Export) as fn() -> Msg),
        ("frame-copy", || Msg::FrameCopy),
        ("frame-paste", || Msg::FramePaste),
        ("insert-text", || Msg::AddOverlay(Tool::Text)),
        ("insert-rect", || Msg::AddOverlay(Tool::Rect)),
        ("insert-ellipse", || Msg::AddOverlay(Tool::Ellipse)),
        ("insert-arrow", || Msg::AddOverlay(Tool::Arrow)),
        ("shortcuts", || Msg::Toast(String::new())),
    ] {
        let action = gio::SimpleAction::new(name, None);
        let sender = sender.clone();
        let root = root.clone();
        let keymap = keymap.clone();
        action.connect_activate(move |_, _| match name {
            "shortcuts" => shortcuts_dialog(&root, &keymap, &sender),
            _ => sender.input(make()),
        });
        actions.add_action(&action);
    }
    for (name, dialog) in [
        ("optimize-remove", OptimizeDialog::Remove),
        ("optimize-smart", OptimizeDialog::Smart),
        ("optimize-resize", OptimizeDialog::Resize),
    ] {
        let action = gio::SimpleAction::new(name, None);
        let (sender, root) = (sender.clone(), root.clone());
        action.connect_activate(move |_, _| optimize_dialog(&root, dialog, &sender));
        actions.add_action(&action);
    }
    // The crop-all dialog needs the live canvas size, so the model opens it.
    let crop_all = gio::SimpleAction::new("optimize-crop", None);
    {
        let sender = sender.clone();
        crop_all.connect_activate(move |_, _| sender.input(Msg::CropAllDialog));
    }
    actions.add_action(&crop_all);
    let zoom_all = gio::SimpleAction::new("optimize-zoom-all", None);
    {
        let sender = sender.clone();
        zoom_all.connect_activate(move |_, _| sender.input(Msg::ApplyZoomAll));
    }
    actions.add_action(&zoom_all);
    let delay_all = gio::SimpleAction::new("frame-delay-all", None);
    {
        let sender = sender.clone();
        delay_all.connect_activate(move |_, _| sender.input(Msg::DelayAllDialog));
    }
    actions.add_action(&delay_all);
    let import_more = gio::SimpleAction::new("import-more", None);
    {
        let sender = sender.clone();
        import_more.connect_activate(move |_, _| sender.input(Msg::ImportMore));
    }
    actions.add_action(&import_more);
    root.insert_action_group("win", Some(&actions));

    Widgets {
        title,
        stack,
        import_bar,
        import_cancel,
        toasts,
        canvas,
        canvas_frame,
        strip,
        bands,
        play,
        time,
        undo,
        redo,
        actions,
        export,
        scope_buttons: [this_frame, all_frames, range],
        properties,
        text_entry,
        text_row,
        text_group,
        frame_group,
        frame_delay,
        delay_row,
        overlay_list,
        overlay_list_scroll,
        overlay_list_group,
        overlay_list_ids,
        scope_mirror,
        drop_dividers,
        text_rows: vec![
            font_row.into(),
            size_row.into(),
            align_row.into(),
            outline_row.into(),
            antialias_row.into(),
        ],
        shape_rows: vec![stroke_row.into()],
        overlay_group,
        font_button,
        text_size,
        fill_button,
        fill_on,
        outline_button,
        outline_width,
        align_buttons,
        antialias,
        stroke_button,
        stroke_width,
        crop_group,
        crop_label,
        crop_button,
        crop_apply,
        zoom_apply,
        shrink_apply,
        tool_buttons,
        shape_button,
        shape_tool,
        bands_scroll,
        bands_expander,
        canvas_overlay,
        canvas_state,
        doc_info,
        bands_model,
        sync,
    }
}

/// A colour button and a width spin encode one `Option<(colour, width)>`
/// between them, so each handler has to read the other widget. `update_view`
/// sets the two one at a time, which means the first setter's handler reports
/// the *previous* overlay's half of the pair — a value the model disagrees
/// with, so it gets applied, which re-runs the sync, which sends the pair back
/// again. That is a loop, not a stray edit, so `sync` gates both handlers
/// rather than `OverlayProp::changes` catching it afterwards.
fn connect_pair(
    colour: &gtk::ColorDialogButton,
    width: &gtk::SpinButton,
    sync: &Rc<Cell<bool>>,
    emit: impl Fn(Option<(crate::core::model::Rgba8, f32)>) + Clone + 'static,
) {
    let value = |c: &gtk::ColorDialogButton, w: &gtk::SpinButton| {
        let w = w.value() as f32;
        (w > 0.0).then(|| (rgba_bytes(c.rgba()), w))
    };
    {
        let (w, sync, emit) = (width.clone(), sync.clone(), emit.clone());
        colour.connect_rgba_notify(move |c| {
            if !sync.get() {
                emit(value(c, &w));
            }
        });
    }
    let c = colour.clone();
    let sync = sync.clone();
    width.connect_value_changed(move |w| {
        if !sync.get() {
            emit(value(&c, w));
        }
    });
}

/// One row per overlay, each band spanning the frames its range covers. This is
/// the layer list; a second one in the sidebar would list every object twice.
/// The overlay whose packed row and frame span contain the click. `bands` is
/// not one entry per visual row — `pack_rows` puts non-overlapping overlays
/// side by side on the same row — so this searches rather than indexing.
fn band_at(bands: &[Band], x: f64, y: f64, pitch: f64) -> Option<OverlayId> {
    let row = (y / BAND_H) as usize;
    let frame = (x / pitch) as usize;
    bands
        .iter()
        .find(|band| band.row == row && band.range.contains(&frame))
        .map(|band| band.id)
}

fn draw_bands(area: &gtk::DrawingArea, cr: &cairo::Context, bands: &[Band], pitch: f64) {
    let color = area.color();
    for band in bands {
        let row = band.row;
        let x = band.range.start as f64 * pitch;
        let w = (band.range.len() as f64 * pitch - THUMB_SPACING as f64).max(4.0);
        let y = row as f64 * BAND_H + 2.0;
        let h = BAND_H - 4.0;
        cr.set_source_rgba(
            color.red() as f64,
            color.green() as f64,
            color.blue() as f64,
            if band.selected { 0.45 } else { 0.22 },
        );
        cr.rectangle(x, y, w, h);
        let _ = cr.fill();
        cr.set_source_rgba(
            color.red() as f64,
            color.green() as f64,
            color.blue() as f64,
            0.9,
        );
        cr.move_to(x + 6.0, y + h - 4.0);
        cr.set_font_size(h - 6.0);
        let _ = cr.show_text(&band.name);
    }
}

/// Where the picture actually sits inside its widget: `ContentFit::ScaleDown`
/// centres the image and never enlarges it, so the canvas gestures have to undo
/// exactly that to land on a pixel.
fn canvas_map(widget: (f64, f64), image: (f64, f64)) -> (f64, f64, f64) {
    if image.0 <= 0.0 || image.1 <= 0.0 {
        return (1.0, 0.0, 0.0);
    }
    let scale = (widget.0 / image.0)
        .min(widget.1 / image.1)
        .clamp(0.0001, 1.0);
    (
        (scale),
        (widget.0 - image.0 * scale) / 2.0,
        (widget.1 - image.1 * scale) / 2.0,
    )
}

/// Hit-testing reads the point in the box's own frame, so a rotated overlay is
/// grabbed where it looks, not where its un-rotated box used to be.
fn contains(t: Transform, x: f32, y: f32) -> bool {
    let (x, y) = t.to_local(x, y);
    let (x0, x1) = (t.x.min(t.x + t.w), t.x.max(t.x + t.w));
    let (y0, y1) = (t.y.min(t.y + t.h), t.y.max(t.y + t.h));
    (x0..=x1).contains(&x) && (y0..=y1).contains(&y)
}

/// Corners in TL, TR, BL, BR order — the same order `resize_corner` and the
/// handle drawing use.
fn corners(t: Transform) -> [(f32, f32); 4] {
    [
        (t.x, t.y),
        (t.x + t.w, t.y),
        (t.x, t.y + t.h),
        (t.x + t.w, t.y + t.h),
    ]
}

fn handle_at(t: Transform, x: f32, y: f32, grab: f32) -> Option<usize> {
    let (x, y) = t.to_local(x, y);
    corners(t)
        .iter()
        .position(|(hx, hy)| (hx - x).abs() <= grab && (hy - y).abs() <= grab)
}

/// Where the grips actually are on the image, rotation included.
fn oriented_corners(t: Transform) -> [(f32, f32); 4] {
    corners(t).map(|(x, y)| t.to_image(x, y))
}

/// Drag one corner, in the box's own frame. The opposite corner stays put
/// unless `from_center`, in which case the centre does. Width and height may
/// pass through zero and come out negative, which `contains` and the
/// rasterizers already tolerate, so there is no need to forbid it.
fn resize_corner(
    t: Transform,
    corner: usize,
    dx: f32,
    dy: f32,
    keep_aspect: bool,
    from_center: bool,
) -> Transform {
    let left = corner == 0 || corner == 2;
    let top = corner == 0 || corner == 1;
    // Resizing about the centre moves both edges, so the drag counts twice.
    let reach = if from_center { 2.0 } else { 1.0 };
    let mut w = t.w + dx * reach * if left { -1.0 } else { 1.0 };
    let mut h = t.h + dy * reach * if top { -1.0 } else { 1.0 };

    if keep_aspect && t.w.abs() > f32::EPSILON && t.h.abs() > f32::EPSILON {
        // Take the larger of the two scale factors and give both axes its
        // magnitude, keeping whichever signs the drag produced so a flip
        // through zero still flips.
        let (sx, sy) = (w / t.w, h / t.h);
        let factor = sx.abs().max(sy.abs());
        w = t.w * factor * sx.signum();
        h = t.h * factor * sy.signum();
    }

    let (cx, cy) = t.center();
    let (x, y) = if from_center {
        (cx - w / 2.0, cy - h / 2.0)
    } else {
        (
            if left { t.x + t.w - w } else { t.x },
            if top { t.y + t.h - h } else { t.y },
        )
    };
    Transform {
        x,
        y,
        w,
        h,
        angle: t.angle,
    }
}

/// `resize_corner` pins the anchor in the box's own frame, but the box rotates
/// about its centre and the centre just moved — so on the image the anchor
/// drifts. Slide the result back onto it.
fn pin_anchor(
    origin: Transform,
    mut resized: Transform,
    corner: usize,
    from_center: bool,
) -> Transform {
    if origin.angle == 0.0 {
        return resized;
    }
    let anchor = |t: Transform| {
        // Opposite corner, in the TL, TR, BL, BR order `corners` uses.
        if from_center {
            t.center()
        } else {
            corners(t)[3 - corner]
        }
    };
    let (was_x, was_y) = origin.to_image(anchor(origin).0, anchor(origin).1);
    let (now_x, now_y) = resized.to_image(anchor(resized).0, anchor(resized).1);
    resized.x += was_x - now_x;
    resized.y += was_y - now_y;
    resized
}

/// Impasto's rotate cursor, hotspot centred on the texture. Impasto centres it
/// the same way and for the same reason: a hotspot past the edge of the loaded
/// image trips a GDK assertion.
fn rotate_cursor() -> Option<gdk::Cursor> {
    let texture = gdk::Texture::from_bytes(&glib::Bytes::from_static(ROTATE_CURSOR)).ok()?;
    Some(gdk::Cursor::from_texture(
        &texture,
        texture.width() / 2,
        texture.height() / 2,
        None,
    ))
}

/// Cursor glyph for a corner of a box rotated by `angle`, ported from Impasto's
/// `ResizeCursors`: the glyph snaps within the corner's own diagonal family, so
/// the arrow keeps pointing along the rotated edge.
fn corner_cursor(corner: usize, angle: f32) -> &'static str {
    // Screen-space octants, [E, SE, S, SW, W, NW, N, NE].
    const CURSORS: [&str; 8] = [
        "e-resize",
        "se-resize",
        "s-resize",
        "sw-resize",
        "w-resize",
        "nw-resize",
        "n-resize",
        "ne-resize",
    ];
    // Axis-aligned octant of each corner, in TL, TR, BL, BR order.
    const BASE: [i32; 4] = [5, 7, 3, 1];
    let base = BASE[corner % 4];
    let parity = base & 1;
    let rotated = base as f32 * 45.0 + angle.to_degrees();
    let steps = ((rotated - parity as f32 * 45.0) / 90.0).round() as i32;
    CURSORS[(((parity + 2 * steps) % 8 + 8) % 8) as usize]
}

/// Impasto's grip hint, built from the keymap rather than typed, so a rebind
/// moves it (AGENTS.md, Keybindings).
fn grip_hint(keys: &Keymap) -> String {
    fill(
        // Translators: Canvas tooltip over an overlay's resize grip. Keep the line breaks.
        t(
            "Drag to resize\n{aspect}: keep the aspect ratio\n{center}+drag: resize from the \
           center\n{rotate}+drag: rotate",
        ),
        &[
            ("aspect", &keys.mods(Modal::KeepAspect).display()),
            ("center", &keys.mods(Modal::FromCenter).display()),
            ("rotate", &keys.mods(Modal::Rotate).display()),
        ],
    )
}

/// The body of an overlay, where a drag moves it and the corners are elsewhere.
fn move_hint(keys: &Keymap) -> String {
    fill(
        // Translators: Canvas tooltip over the body of an overlay. Keep the line break.
        t("Drag to move\n{rotate}+drag: rotate"),
        &[("rotate", &keys.mods(Modal::Rotate).display())],
    )
}

/// Pack overlay bands into rows: each one takes the lowest row where nothing
/// already sits over the frames it covers. Without this an overlay's row was
/// its index in the list, so the tenth overlay drew ten rows down even when the
/// nine below it were nowhere near its frames.
fn pack_rows(ranges: &[Range<usize>]) -> Vec<usize> {
    let mut occupied: Vec<Vec<Range<usize>>> = Vec::new();
    ranges
        .iter()
        .map(|range| {
            let row = occupied
                .iter()
                .position(|taken| {
                    taken
                        .iter()
                        .all(|t| t.end <= range.start || range.end <= t.start)
                })
                .unwrap_or_else(|| {
                    occupied.push(Vec::new());
                    occupied.len() - 1
                });
            occupied[row].push(range.clone());
            row
        })
        .collect()
}

/// Click to select, drag to move, corner to resize — and with the crop tool
/// armed, drag to define the box. All of it in image pixels, which is the only
/// coordinate system the model knows.
fn install_canvas_gestures(
    area: &gtk::DrawingArea,
    state: &Rc<RefCell<CanvasState>>,
    sender: &ComponentSender<App>,
) {
    let to_image = {
        let (state, area) = (state.clone(), area.clone());
        move |x: f64, y: f64| -> (f32, f32, f32) {
            let image = state.borrow().image;
            let (scale, ox, oy) = canvas_map(
                (area.width() as f64, area.height() as f64),
                (image.0 as f64, image.1 as f64),
            );
            (
                ((x - ox) / scale) as f32,
                ((y - oy) / scale) as f32,
                scale as f32,
            )
        }
    };

    let drag = gtk::GestureDrag::new();
    {
        let (to_image, sender) = (to_image.clone(), sender.clone());
        drag.connect_drag_begin(move |gesture, x, y| {
            let (ix, iy, scale) = to_image(x, y);
            let state = gesture.current_event_state();
            sender.input(Msg::CanvasPress {
                x: ix,
                y: iy,
                scale,
                state,
            });
        });
    }
    {
        let (to_image, sender) = (to_image.clone(), sender.clone());
        drag.connect_drag_update(move |gesture, dx, dy| {
            let Some((sx, sy)) = gesture.start_point() else {
                return;
            };
            let (ix, iy, _) = to_image(sx + dx, sy + dy);
            // Read live: the modifiers may have gone down after the press.
            let state = gesture.current_event_state();
            sender.input(Msg::CanvasDrag {
                x: ix,
                y: iy,
                state,
            });
        });
    }
    {
        let sender = sender.clone();
        drag.connect_drag_end(move |_, _, _| sender.input(Msg::CanvasRelease));
    }
    area.add_controller(drag);
    install_canvas_hover(area, state);
}

/// Hover feedback, Impasto's: the resize glyph for the grip under the pointer,
/// the rotate cursor while the rotate modifier is down, and the grip hint as
/// the canvas tooltip. It reads the shared canvas state directly, because a
/// cursor that waits for a message round trip lags the pointer.
fn install_canvas_hover(area: &gtk::DrawingArea, state: &Rc<RefCell<CanvasState>>) {
    let motion = gtk::EventControllerMotion::new();
    let (state, area_ref) = (state.clone(), area.clone());
    let rotate_cursor = rotate_cursor();
    // Motion fires per pixel, and both setters below allocate, so each one only
    // runs when its value actually changed.
    let shown: Rc<Cell<&'static str>> = Rc::new(Cell::new(""));
    motion.connect_motion(move |controller, x, y| {
        let state = state.borrow();
        let (scale, ox, oy) = canvas_map(
            (area_ref.width() as f64, area_ref.height() as f64),
            (state.image.0 as f64, state.image.1 as f64),
        );
        let (ix, iy) = (((x - ox) / scale) as f32, ((y - oy) / scale) as f32);
        let grab = (HANDLE_PX / scale.max(0.01)) as f32;
        let rotating = state.rotate.held(controller.current_event_state());

        let cursor = hover_cursor(&state, ix, iy, grab, rotating);
        if shown.replace(cursor) != cursor {
            match (cursor, &rotate_cursor) {
                ("rotate", Some(glyph)) => area_ref.set_cursor(Some(glyph)),
                // No CSS cursor name means rotation, so a build that could not
                // read the texture falls back to the nearest thing every theme
                // ships rather than to no feedback at all.
                ("rotate", None) => area_ref.set_cursor_from_name(Some("grab")),
                _ => area_ref.set_cursor_from_name(Some(cursor)),
            }
        }

        let want = match cursor {
            "default" => None,
            "move" => Some(state.move_hint.clone()),
            _ => Some(state.hint.clone()),
        };
        if area_ref.tooltip_text().map(|t| t.to_string()) != want {
            area_ref.set_tooltip_text(want.as_deref());
        }
    });
    area.add_controller(motion);
}

/// Which glyph the pointer gets, in the same precedence the press handler uses:
/// the rotate modifier outranks a grip, a grip outranks the body it belongs to,
/// and the body of *any* overlay promises a move, not just the selected one's.
fn hover_cursor(state: &CanvasState, x: f32, y: f32, grab: f32, rotating: bool) -> &'static str {
    let grip = state
        .selected
        .and_then(|t| handle_at(t, x, y, grab).map(|corner| (t, corner)));
    match (grip, rotating) {
        (_, true) if state.selected.is_some() => "rotate",
        (Some((t, corner)), _) => corner_cursor(corner, t.angle),
        _ if state.movable.iter().any(|t| contains(*t, x, y)) => "move",
        _ => "default",
    }
}

/// Selection handles and the crop box. Drawn over the picture rather than into
/// the composite, so nothing here can end up in the exported GIF.
fn draw_canvas_overlay(
    area: &gtk::DrawingArea,
    cr: &cairo::Context,
    width: f64,
    height: f64,
    state: &CanvasState,
) {
    let (scale, ox, oy) = canvas_map(
        (width, height),
        (state.image.0 as f64, state.image.1 as f64),
    );
    let to_widget = |x: f32, y: f32| (ox + x as f64 * scale, oy + y as f64 * scale);
    let accent = area.color();
    let (image_w, image_h) = (state.image.0 as f64 * scale, state.image.1 as f64 * scale);
    cr.set_line_width(1.0);

    // The widget is wider or taller than the picture whenever the aspect ratios
    // differ, and an overlay dragged into that margin is clipped out of every
    // frame. Say where the picture ends rather than letting it vanish.
    if image_w > 0.0 && image_h > 0.0 && state.crop.is_none() {
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.30);
        cr.set_fill_rule(cairo::FillRule::EvenOdd);
        cr.rectangle(0.0, 0.0, width, height);
        cr.rectangle(ox, oy, image_w, image_h);
        let _ = cr.fill();
        cr.set_fill_rule(cairo::FillRule::Winding);
        cr.set_source_rgba(
            accent.red() as f64,
            accent.green() as f64,
            accent.blue() as f64,
            0.55,
        );
        cr.rectangle(ox + 0.5, oy + 0.5, image_w - 1.0, image_h - 1.0);
        let _ = cr.stroke();
    }

    if let Some((x, y, w, h)) = state.crop {
        let (px, py) = to_widget(x, y);
        let (pw, ph) = (w as f64 * scale, h as f64 * scale);
        // Dim what the crop would throw away, so the box reads as a keep-area:
        // two rectangles, even-odd, so the inner one is a hole.
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.45);
        cr.set_fill_rule(cairo::FillRule::EvenOdd);
        cr.rectangle(0.0, 0.0, width, height);
        cr.rectangle(px, py, pw, ph);
        let _ = cr.fill();
        cr.set_fill_rule(cairo::FillRule::Winding);
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.rectangle(px, py, pw, ph);
        let _ = cr.stroke();
        return;
    }

    let Some(t) = state.selected else { return };
    let grips = oriented_corners(t).map(|(x, y)| to_widget(x, y));
    // TL, TR, BR, BL: the outline walks the box, `corners` lists it in reading
    // order, so the last two swap.
    let outline = [grips[0], grips[1], grips[3], grips[2]];

    cr.set_source_rgba(
        accent.red() as f64,
        accent.green() as f64,
        accent.blue() as f64,
        0.9,
    );
    cr.set_dash(&[4.0, 3.0], 0.0);
    cr.move_to(outline[0].0, outline[0].1);
    for (x, y) in &outline[1..] {
        cr.line_to(*x, *y);
    }
    cr.close_path();
    let _ = cr.stroke();
    cr.set_dash(&[], 0.0);

    // Impasto's grips: a blue dot with a translucent white ring, at a constant
    // widget size whatever the zoom.
    for (cx, cy) in grips {
        cr.arc(cx, cy, HANDLE_R, 0.0, std::f64::consts::TAU);
        cr.set_source_rgb(HANDLE_FILL.0, HANDLE_FILL.1, HANDLE_FILL.2);
        let _ = cr.fill_preserve();
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.7);
        let _ = cr.stroke();
    }
}

#[derive(Clone, Copy, Debug)]
enum OptimizeDialog {
    Remove,
    Smart,
    Resize,
}

/// The optimize menu's three dialogs. One function because they are the same
/// dialog with a different spin button in it.
fn optimize_dialog(
    root: &adw::ApplicationWindow,
    which: OptimizeDialog,
    sender: &ComponentSender<App>,
) {
    let (title, body) = match which {
        OptimizeDialog::Remove => (
            t("Remove frames"),
            t(
                "Delete one frame out of every N. Total duration is preserved, so the \
               result plays at the same speed with fewer frames in it.",
            ),
        ),
        OptimizeDialog::Smart => (
            t("Smart remove frames"),
            t(
                "Drop the frames that change the least, so a still section thins out \
               before a moving one does. Duration is preserved.",
            ),
        ),
        OptimizeDialog::Resize => (
            t("Resize"),
            t("Scale every frame, and the overlays with them."),
        ),
    };
    let dialog = adw::AlertDialog::new(Some(title), Some(body));
    dialog.set_content_width(440);

    let group = adw::PreferencesGroup::new();
    let spin = match which {
        OptimizeDialog::Remove => gtk::SpinButton::with_range(2.0, 60.0, 1.0),
        OptimizeDialog::Smart => gtk::SpinButton::with_range(5.0, 90.0, 5.0),
        OptimizeDialog::Resize => gtk::SpinButton::with_range(16.0, 8192.0, 2.0),
    };
    spin.set_valign(gtk::Align::Center);
    spin.set_value(match which {
        OptimizeDialog::Remove => 2.0,
        OptimizeDialog::Smart => 30.0,
        OptimizeDialog::Resize => 480.0,
    });
    let height = gtk::SpinButton::with_range(16.0, 8192.0, 2.0);
    height.set_valign(gtk::Align::Center);
    height.set_value(270.0);
    let row = adw::ActionRow::builder()
        .title(match which {
            OptimizeDialog::Remove => t("Remove 1 frame in every"),
            OptimizeDialog::Smart => t("Remove this share of frames"),
            OptimizeDialog::Resize => t("New size"),
        })
        .build();
    let suffix = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    suffix.set_valign(gtk::Align::Center);
    suffix.append(&spin);
    match which {
        // Translators: Unit after the "remove 1 frame in every N" field.
        OptimizeDialog::Remove => suffix.append(&gtk::Label::new(Some(t("frames")))),
        OptimizeDialog::Smart => suffix.append(&gtk::Label::new(Some("%"))),
        OptimizeDialog::Resize => {
            suffix.append(&gtk::Label::new(Some("×")));
            suffix.append(&height);
            suffix.append(&gtk::Label::new(Some(t("px"))));
        }
    }
    row.add_suffix(&suffix);
    group.add(&row);
    dialog.set_extra_child(Some(&group));

    dialog.add_response("cancel", t("Cancel"));
    dialog.add_response("apply", t("Apply"));
    dialog.set_response_appearance("apply", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("apply"));
    dialog.set_close_response("cancel");

    let sender = sender.clone();
    dialog.choose(Some(root), gio::Cancellable::NONE, move |response| {
        if response != "apply" {
            return;
        }
        let value = spin.value();
        sender.input(match which {
            OptimizeDialog::Remove => Msg::DropEveryNth(value as usize),
            OptimizeDialog::Smart => Msg::SmartDrop(value as usize),
            OptimizeDialog::Resize => Msg::Resize(value as u32, height.value() as u32),
        });
    });
}

/// Crop every frame, from the Optimize menu. Four fields, in image pixels;
/// `Document::crop` clamps whatever comes back. The canvas size is live when
/// the dialog opens, which is why the model opens it and the action does not.
fn crop_dialog(root: &adw::ApplicationWindow, cw: u32, ch: u32, sender: &ComponentSender<App>) {
    let dialog = adw::AlertDialog::new(
        Some(t("Crop all frames")),
        // Translators: Explains that this crop changes the shared GIF canvas and moves overlays with it.
        Some(t(
            "Keep this box in every frame. The canvas shrinks to fit it, and \
             overlays move with the crop.",
        )),
    );
    dialog.set_content_width(440);
    let x = gtk::SpinButton::with_range(0.0, (cw - 1) as f64, 1.0);
    let y = gtk::SpinButton::with_range(0.0, (ch - 1) as f64, 1.0);
    let width = gtk::SpinButton::with_range(1.0, cw as f64, 1.0);
    let height = gtk::SpinButton::with_range(1.0, ch as f64, 1.0);
    for (spin, value) in [
        (&x, 0.0),
        (&y, 0.0),
        (&width, cw as f64),
        (&height, ch as f64),
    ] {
        spin.set_value(value);
        spin.set_valign(gtk::Align::Center);
    }
    {
        let adjustment = width.adjustment();
        x.connect_value_changed(move |x| adjustment.set_upper((cw as f64 - x.value()).max(1.0)));
    }
    {
        let adjustment = height.adjustment();
        y.connect_value_changed(move |y| adjustment.set_upper((ch as f64 - y.value()).max(1.0)));
    }
    let group = adw::PreferencesGroup::new();
    for (title, spin) in [
        ("X", &x),
        ("Y", &y),
        // Translators: Horizontal size of the crop box, in image pixels.
        (t("Width"), &width),
        // Translators: Vertical size of the crop box, in image pixels.
        (t("Height"), &height),
    ] {
        let row = adw::ActionRow::builder().title(title).build();
        row.add_suffix(spin);
        group.add(&row);
    }
    dialog.set_extra_child(Some(&group));

    dialog.add_response("cancel", t("Cancel"));
    dialog.add_response("apply", t("Apply"));
    dialog.set_response_appearance("apply", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("apply"));
    dialog.set_close_response("cancel");

    let sender = sender.clone();
    dialog.choose(Some(root), gio::Cancellable::NONE, move |response| {
        if response == "apply" {
            sender.input(Msg::CropAll(
                x.value() as u32,
                y.value() as u32,
                width.value() as u32,
                height.value() as u32,
            ));
        }
    });
}

/// Per-frame delay, from the frame's own context menu. `scope` is what
/// "apply" lands on: the one right-clicked frame, or the whole active
/// selection when the menu was opened from inside one.
fn delay_scope_dialog(
    anchor: &impl IsA<gtk::Widget>,
    scope: &[usize],
    delay_cs: u16,
    sender: &ComponentSender<App>,
) {
    let dialog = adw::AlertDialog::new(
        Some(t("Frame delay")),
        // Translators: GIF stores frame delays in centiseconds.
        Some(t("How long this frame is held, in hundredths of a second.")),
    );
    let spin = gtk::SpinButton::with_range(1.0, 6000.0, 1.0);
    spin.set_value(delay_cs.max(1) as f64);
    spin.set_valign(gtk::Align::Center);
    let group = adw::PreferencesGroup::new();
    let row = if scope.len() == 1 {
        adw::ActionRow::builder()
            .title(fill(
                t("Frame {number}"),
                &[("number", &(scope[0] + 1).to_string())],
            ))
            .build()
    } else {
        adw::ActionRow::builder()
            .title(fill(
                tn(
                    "{count} selected frame",
                    "{count} selected frames",
                    scope.len(),
                ),
                &[("count", &scope.len().to_string())],
            ))
            .build()
    };
    row.add_suffix(&spin);
    group.add(&row);
    dialog.set_extra_child(Some(&group));
    dialog.add_response("cancel", t("Cancel"));
    dialog.add_response("apply", t("Apply"));
    dialog.set_response_appearance("apply", adw::ResponseAppearance::Suggested);
    dialog.set_close_response("cancel");

    let sender = sender.clone();
    dialog.choose(
        Some(anchor.as_ref()),
        gio::Cancellable::NONE,
        move |response| {
            if response == "apply" {
                sender.input(Msg::SetScopeDelay(spin.value() as u16));
            }
        },
    );
}

/// From the "Frame operations" menu: set every frame's delay at once, rather
/// than selecting all frames and finding there is no menu entry for it.
fn delay_all_dialog(
    root: &adw::ApplicationWindow,
    count: usize,
    delay_cs: u16,
    sender: &ComponentSender<App>,
) {
    let dialog = adw::AlertDialog::new(
        Some(t("Set delay for all frames")),
        // Translators: GIF stores frame delays in centiseconds.
        Some(t(
            "How long every frame is held, in hundredths of a second.",
        )),
    );
    let spin = gtk::SpinButton::with_range(1.0, 6000.0, 1.0);
    spin.set_value(delay_cs.max(1) as f64);
    spin.set_valign(gtk::Align::Center);
    let group = adw::PreferencesGroup::new();
    let row = adw::ActionRow::builder()
        .title(fill(
            t("All {count} frames"),
            &[("count", &count.to_string())],
        ))
        .build();
    row.add_suffix(&spin);
    group.add(&row);
    dialog.set_extra_child(Some(&group));
    dialog.add_response("cancel", t("Cancel"));
    dialog.add_response("apply", t("Apply"));
    dialog.set_response_appearance("apply", adw::ResponseAppearance::Suggested);
    dialog.set_close_response("cancel");

    let sender = sender.clone();
    dialog.choose(Some(root), gio::Cancellable::NONE, move |response| {
        if response == "apply" {
            sender.input(Msg::SetAllFramesDelay(spin.value() as u16));
        }
    });
}

/// A frame's own context menu: type an exact 1-based target position rather
/// than nudging one step at a time.
fn move_frame_dialog(
    anchor: &impl IsA<gtk::Widget>,
    from: usize,
    count: usize,
    sender: &ComponentSender<App>,
) {
    let dialog = adw::AlertDialog::new(
        Some(t("Move frame")),
        Some(t("Where should this frame go?")),
    );
    let spin = gtk::SpinButton::with_range(1.0, count as f64, 1.0);
    spin.set_value((from + 1) as f64);
    spin.set_valign(gtk::Align::Center);
    let group = adw::PreferencesGroup::new();
    let row = adw::ActionRow::builder()
        .title(fill(
            t("Frame {number}"),
            &[("number", &(from + 1).to_string())],
        ))
        .build();
    row.add_suffix(&spin);
    group.add(&row);
    dialog.set_extra_child(Some(&group));
    dialog.add_response("cancel", t("Cancel"));
    dialog.add_response("apply", t("Move"));
    dialog.set_response_appearance("apply", adw::ResponseAppearance::Suggested);
    dialog.set_close_response("cancel");

    let sender = sender.clone();
    dialog.choose(
        Some(anchor.as_ref()),
        gio::Cancellable::NONE,
        move |response| {
            if response == "apply" {
                let to = (spin.value() as usize).saturating_sub(1).min(count - 1);
                sender.input(Msg::MoveFrame(from, to));
            }
        },
    );
}

/// Icon name for the bundled left-rail glyph (`resources/README.md`).
fn tool_icon_name(tool: Tool) -> &'static str {
    match tool {
        Tool::Text => "tool-text-symbolic",
        Tool::Rect => "tool-rect-symbolic",
        Tool::Ellipse => "tool-ellipse-symbolic",
        Tool::Arrow => "tool-arrow-symbolic",
    }
}

/// Marked with `n` and translated where it is shown: the same words are also
/// the overlay names stored in the document, which stay English.
fn tool_label(tool: Tool) -> &'static str {
    match tool {
        Tool::Text => n("Text"),
        Tool::Rect => n("Rectangle"),
        Tool::Ellipse => n("Ellipse"),
        Tool::Arrow => n("Arrow"),
    }
}

fn tool_action(tool: Tool) -> Action {
    match tool {
        Tool::Text => Action::ToolText,
        Tool::Rect => Action::ToolRect,
        Tool::Ellipse => Action::ToolEllipse,
        Tool::Arrow => Action::ToolArrow,
    }
}

fn suffixed(title: &str, suffix: &impl IsA<gtk::Widget>) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(title).build();
    row.add_suffix(suffix);
    row
}

/// One row of the sidebar layer list: the overlay's name, then its two
/// z-order steps and a quick delete. Each button carries the overlay it
/// belongs to rather than acting on the selection, so no row can delete or
/// restack a different layer than the one it sits on. `can_raise` and
/// `can_lower` are false at the ends of the list, where the step has
/// nowhere to go.
fn overlay_row(
    id: OverlayId,
    name: &str,
    can_raise: bool,
    can_lower: bool,
    sender: &ComponentSender<App>,
) -> adw::ActionRow {
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    buttons.set_valign(gtk::Align::Center);
    for (icon, tip, up, enabled) in [
        ("go-up-symbolic", t("Move overlay up"), true, can_raise),
        ("go-down-symbolic", t("Move overlay down"), false, can_lower),
    ] {
        let step = gtk::Button::from_icon_name(icon);
        step.add_css_class("flat");
        step.set_tooltip_text(Some(tip));
        step.set_sensitive(enabled);
        connect(&step, sender, move || Msg::RestackOverlay { id, up });
        buttons.append(&step);
    }
    // The X is the quick one, beside the layer it removes; the red trash
    // button in the overlay editor below deletes the selected overlay.
    let delete = gtk::Button::from_icon_name("window-close-symbolic");
    delete.add_css_class("flat");
    delete.set_tooltip_text(Some(t("Delete overlay")));
    connect(&delete, sender, move || Msg::DeleteOverlay(id));
    buttons.append(&delete);
    suffixed(name, &buttons)
}

/// The heights to hold the layer list between: `LAYER_ROWS_KEPT` rows it
/// never shrinks below and `LAYER_ROWS_SHOWN` it never grows past, in units
/// of its own rows — measured rather than assumed, since a row's height is
/// whatever the theme's `AdwActionRow` plus the buttons in it comes out as.
/// The floor is what makes the panel around it overflow and scroll instead
/// of squeezing the list down to a sliver; the ceiling is what keeps the
/// editor for the layer that is picked from being pushed off the panel.
/// `(0, 0)` while the list is empty, which is also when the group is hidden.
fn layer_list_heights(list: &gtk::ListBox) -> (i32, i32) {
    let Some(row) = list.first_child() else {
        return (0, 0);
    };
    let (_, row_h, _, _) = row.measure(gtk::Orientation::Vertical, -1);
    let rows = list.observe_children().n_items() as usize;
    (
        row_h * rows.min(LAYER_ROWS_KEPT) as i32,
        row_h * LAYER_ROWS_SHOWN as i32,
    )
}

/// Stop a scroller's viewport from scrolling itself to wherever the keyboard
/// focus went. The strip's frame menu is a `Popover` parented to the strip, so
/// closing it — which is what duplicating, deleting or pasting a frame does —
/// moved the focus, and the viewport answered by scrolling the timeline off to
/// whatever it thought the focus was nearest, usually back to frame 1. Nothing
/// inside the strip takes the focus, so there is nothing there worth chasing.
fn dont_chase_focus(scroll: &gtk::ScrolledWindow) {
    if let Some(viewport) = scroll.child().and_downcast::<gtk::Viewport>() {
        viewport.set_scroll_to_focus(false);
    }
}

/// Apply `layer_list_heights` to the list's scroller in whichever order
/// leaves the pair consistent at every step. GTK checks each half against
/// the other as it is set and drops the write that would cross it, so
/// raising the floor past the standing ceiling has to raise the ceiling
/// first — which is exactly what a list refilling after it went empty does,
/// both bounds having been pinned to zero while it had no rows. The other
/// direction needs the floor lowered first, and `cap >= keep` holds by
/// construction (`LAYER_ROWS_KEPT <= LAYER_ROWS_SHOWN`), so one comparison
/// picks the safe order.
fn set_layer_list_heights(scroll: &gtk::ScrolledWindow, keep: i32, cap: i32) {
    if keep > scroll.max_content_height() {
        scroll.set_max_content_height(cap);
        scroll.set_min_content_height(keep);
    } else {
        scroll.set_min_content_height(keep);
        scroll.set_max_content_height(cap);
    }
}

/// Scroll `row` into `scroll`'s visible window, by the shortest move that
/// gets it there — nothing at all when it is already in view, so picking a
/// row does not jerk the list around. Silent before the first layout, when
/// the row has no bounds to read yet.
fn show_row(scroll: &gtk::ScrolledWindow, list: &gtk::ListBox, row: &gtk::ListBoxRow) {
    let Some(bounds) = row.compute_bounds(list) else {
        return;
    };
    let (top, bottom) = (bounds.y() as f64, (bounds.y() + bounds.height()) as f64);
    let adjustment = scroll.vadjustment();
    let (shown, page) = (adjustment.value(), adjustment.page_size());
    if top < shown {
        adjustment.set_value(top);
    } else if bottom > shown + page {
        adjustment.set_value(bottom - page);
    }
}

/// The sidebar heading for the frame view: "Frame" for a single frame in
/// scope (or none), or a summary of which frames are once the scope names
/// more than one — the panel this titles trades per-frame overlay editing
/// for a scoped delay edit (`SetScopeDelay`) once that happens. `in_scope`
/// is `scope_frames()`: sorted, deduplicated, already clamped to the
/// document.
fn frame_scope_summary(scope: ScopeChoice, in_scope: &[usize]) -> String {
    if in_scope.len() <= 1 {
        return t("Frame").into();
    }
    if scope == ScopeChoice::AllFrames {
        // Translators: Sidebar heading while every frame is in scope.
        return fill(
            t("All {count} frames selected"),
            &[("count", &in_scope.len().to_string())],
        );
    }
    let (first, last) = (in_scope[0], in_scope[in_scope.len() - 1]);
    if last - first + 1 == in_scope.len() {
        return fill(
            // Translators: Sidebar heading for a run of selected frames. Frame numbers are 1-based and inclusive.
            t("Frames {first}–{last} selected"),
            &[
                ("first", &(first + 1).to_string()),
                ("last", &(last + 1).to_string()),
            ],
        );
    }
    if in_scope.len() <= 8 {
        let numbers = in_scope
            .iter()
            .map(|i| (i + 1).to_string())
            .collect::<Vec<_>>()
            .join(", ");
        // Translators: Sidebar heading for a non-contiguous set of selected frames; {numbers} is a comma-separated, 1-based list.
        return fill(t("Frames {numbers} selected"), &[("numbers", &numbers)]);
    }
    // Translators: Sidebar heading when too many non-contiguous frames are selected to list by number.
    fill(
        tn(
            "{count} frame selected",
            "{count} frames selected",
            in_scope.len(),
        ),
        &[("count", &in_scope.len().to_string())],
    )
}

fn color_button() -> gtk::ColorDialogButton {
    let button = gtk::ColorDialogButton::new(Some(gtk::ColorDialog::new()));
    button.set_valign(gtk::Align::Center);
    button
}

/// Zero means off, which is why the range starts there.
fn width_spin() -> gtk::SpinButton {
    let spin = gtk::SpinButton::with_range(0.0, 40.0, 1.0);
    spin.set_valign(gtk::Align::Center);
    spin.set_width_chars(2);
    spin.set_max_width_chars(2);
    spin
}

fn rgba_bytes(c: gdk::RGBA) -> crate::core::model::Rgba8 {
    let to_u8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    [
        to_u8(c.red()),
        to_u8(c.green()),
        to_u8(c.blue()),
        to_u8(c.alpha()),
    ]
}

/// Setters that no-op when nothing changed: assigning fires the notify handler,
/// which would send the value straight back as an edit.
fn set_color(button: &gtk::ColorDialogButton, [r, g, b, a]: crate::core::model::Rgba8) {
    let want = gdk::RGBA::new(
        r as f32 / 255.0,
        g as f32 / 255.0,
        b as f32 / 255.0,
        a as f32 / 255.0,
    );
    if button.rgba() != want {
        button.set_rgba(&want);
    }
}

fn set_spin(spin: &gtk::SpinButton, value: f64) {
    if (spin.value() - value).abs() > 0.001 {
        spin.set_value(value);
    }
}

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::from_icon_name(icon);
    button.set_tooltip_text(Some(tooltip));
    button
}

fn connect(button: &gtk::Button, sender: &ComponentSender<App>, msg: impl Fn() -> Msg + 'static) {
    no_focus_steal(button);
    let sender = sender.clone();
    button.connect_clicked(move |_| sender.input(msg()));
}

/// Clicking a toolbar button must not leave it holding the keyboard focus, so
/// that Enter and the arrow keys keep meaning whatever the canvas means by
/// them. Tab still reaches the button. Space is handled by `install_shortcuts`
/// before any focused widget sees it, so this is not what makes play/pause
/// work.
fn no_focus_steal(widget: &impl IsA<gtk::Widget>) {
    widget.as_ref().set_focus_on_click(false);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::video::VideoInfo;

    fn doc(frames: usize) -> Document {
        Document::from_frames(
            (0..frames)
                .map(|i| {
                    let px = image::Rgba([i as u8, 0, 0, 255]);
                    Frame::new(image::RgbaImage::from_pixel(8, 8, px), 5)
                })
                .collect(),
        )
    }

    /// A minimal `App` for testing scope logic without a display: nothing
    /// on the model itself is GTK-owned, so this needs no window or sender.
    fn app_with(doc: Document, scope: ScopeChoice, selection: Vec<usize>, playhead: usize) -> App {
        App {
            editor: Editor::new(doc),
            caps: Caps::default(),
            settings: Settings::default(),
            path: None,
            playhead,
            playing: false,
            scope,
            selection,
            anchor: None,
            selected_overlay: None,
            busy: None,
            estimate: None,
            estimate_pending: None,
            keymap: Rc::new(RefCell::new(Keymap::default())),
            drag: None,
            crop_tool: false,
            crop_rect: None,
            bands_expanded: false,
            strip_keys: RefCell::new(Vec::new()),
            strip_zoom: Rc::new(Cell::new(1.0)),
            strip_pitch: Rc::new(Cell::new(cell_pitch(THUMB_BOX, 1.0))),
            strip_zoom_shown: Cell::new(1.0),
            rev: 0,
            import_append: false,
            clipboard: Vec::new(),
        }
    }

    /// Scope-driven tools - "Crop and keep size" and "Zoom and resize" on the
    /// canvas, and the frame-operations menu (delete/duplicate/reverse) - must
    /// land on exactly what the scope names: the frame on screen, a selection,
    /// or every frame. Regression: `Msg::ApplyShrink` and `Msg::ApplyZoom` both
    /// used to hardcode `vec![self.playhead]`, so selecting "All frames" and
    /// drawing a crop box still only touched the frame on screen.
    #[test]
    fn scope_driven_tools_touch_exactly_what_the_scope_names() {
        let cases: Vec<(App, Vec<usize>)> = vec![
            (
                app_with(doc(5), ScopeChoice::ThisFrame, Vec::new(), 3),
                vec![3],
            ),
            (
                app_with(doc(5), ScopeChoice::Range, vec![4, 1, 1], 0),
                vec![1, 4],
            ),
            (
                app_with(doc(5), ScopeChoice::AllFrames, Vec::new(), 0),
                (0..5).collect(),
            ),
        ];
        for (app, want) in &cases {
            assert_eq!(
                &app.scope_frames(),
                want,
                "scope_frames() must resolve to {want:?}"
            );

            // The exact path Msg::ApplyShrink takes, minus dispatching the
            // work to the worker thread.
            let work = app.shrink_work((1.0, 1.0, 4.0, 4.0)).unwrap();
            let FrameWork::Shrink { frames, .. } = work else {
                panic!("expected Shrink work");
            };
            assert_eq!(&frames, want, "shrink must touch exactly the scoped frames");

            // The panel's "Zoom and resize" (Msg::ApplyZoom) follows the same
            // scope; the Image menu's "all frames" variant never does.
            assert_eq!(
                &app.zoom_frames(false),
                want,
                "scoped zoom must touch exactly the scoped frames"
            );
            assert_eq!(
                app.zoom_frames(true),
                (0..app.frame_count()).collect::<Vec<_>>(),
                "'all frames' zoom must ignore the scope"
            );
        }
    }

    /// Regression: the strip was keyed on the document revision, which overlay
    /// edits bump too, so every keystroke in the text entry rebuilt a widget and
    /// a texture per frame.
    #[test]
    fn overlay_edits_leave_the_strip_alone() {
        let mut doc = doc(4);
        let before = strip_keys(&doc);

        let id = doc.add_overlay(
            "caption",
            OverlayKind::Text(TextOverlay {
                text: "hi".into(),
                ..Default::default()
            }),
            Transform::at(0.0, 0.0, 8.0, 8.0),
            0..4,
        );
        assert_eq!(
            strip_keys(&doc),
            before,
            "adding an overlay changes no frame"
        );

        if let Some(OverlayKind::Text(t)) = doc.overlay_mut(id).map(|o| &mut o.kind) {
            t.text = "hi there".into();
        }
        assert_eq!(strip_keys(&doc), before, "typing changes no frame");

        doc.remove_overlay(id);
        assert_eq!(strip_keys(&doc), before);
    }

    #[test]
    fn frame_edits_do_rebuild_the_strip() {
        let mut doc = doc(4);
        let before = strip_keys(&doc);

        doc.duplicate_frame(0);
        assert_ne!(strip_keys(&doc), before);

        let after_duplicate = strip_keys(&doc);
        doc.delete_frames(0..1);
        assert_ne!(strip_keys(&doc), after_duplicate);

        // A frame back from an external editor gets the pencil badge, so the
        // flag rides along with the key.
        let mut doc = doc.clone();
        let opaque = strip_keys(&doc);
        doc.replace_frame_pixels(0, image::RgbaImage::new(8, 8));
        assert_ne!(strip_keys(&doc), opaque);
        assert!(strip_keys(&doc)[0].1, "detached");
    }

    /// Regression: the sidebar overlay editor stayed up for an overlay on a
    /// different frame, so a frame with nothing on it still showed text fields.
    #[test]
    fn sidebar_edits_an_overlay_only_while_it_is_on_the_frame() {
        let mut app = app_with(doc(6), ScopeChoice::ThisFrame, Vec::new(), 0);
        let id = app.editor.doc.add_overlay(
            "cap",
            OverlayKind::Text(TextOverlay {
                text: "hi".into(),
                ..Default::default()
            }),
            Transform::at(0.0, 0.0, 8.0, 8.0),
            1..3,
        );
        app.selected_overlay = Some(id);

        app.playhead = 2;
        assert_eq!(app.editing_overlay(), Some(id), "on the overlay's frame");

        app.playhead = 4;
        assert_eq!(app.editing_overlay(), None, "off it: plain frame view");
        assert!(app.overlays_on(4).is_empty());

        app.playhead = 1;
        assert_eq!(app.editing_overlay(), Some(id), "memory: back on its frame");
    }

    /// The layer list reads top-down like the canvas stacks — the last
    /// overlay in `doc.overlays` paints on top, so it is the first row — and
    /// a step moves past the layer *shown* next to it, skipping overlays that
    /// are not on this frame and would make the button look dead.
    #[test]
    fn layer_list_is_topmost_first_and_steps_past_what_it_shows() {
        let mut app = app_with(doc(4), ScopeChoice::ThisFrame, Vec::new(), 0);
        let shape = || {
            OverlayKind::Shape(ShapeOverlay {
                shape: Shape::Rect,
                fill: Some([1, 2, 3, 255]),
                stroke: None,
            })
        };
        let box_at = Transform::at(0.0, 0.0, 8.0, 8.0);
        let bottom = app.editor.doc.add_overlay("bottom", shape(), box_at, 0..4);
        let elsewhere = app
            .editor
            .doc
            .add_overlay("elsewhere", shape(), box_at, 2..4);
        let top = app.editor.doc.add_overlay("top", shape(), box_at, 0..4);

        let rows = |app: &App| {
            app.stacked_overlays()
                .into_iter()
                .map(|(_, name)| name)
                .collect::<Vec<_>>()
        };
        assert_eq!(rows(&app), vec!["top", "bottom"], "topmost row first");

        assert_eq!(
            app.restack_neighbour(bottom, true),
            Some(top),
            "a step up goes past the row above, not past `elsewhere`"
        );
        assert_eq!(app.restack_neighbour(top, true), None, "already on top");
        assert_eq!(app.restack_neighbour(bottom, false), None, "already bottom");

        app.restack_overlay(bottom, true);
        assert_eq!(rows(&app), vec!["bottom", "top"]);
        assert_eq!(
            app.editor.doc.overlays.last().map(|o| o.id),
            Some(bottom),
            "the first row is the overlay painted last"
        );
        assert_eq!(
            app.editor.doc.overlay(elsewhere).map(|o| o.range.clone()),
            Some(2..4),
            "an overlay the step skipped is untouched"
        );
        assert_eq!(app.selected_overlay, Some(bottom), "stays on what moved");
    }

    /// Copy leaves the document alone and paste splices the clipboard in
    /// after the frame on screen, keeping the clipboard for the next paste.
    #[test]
    fn copied_frames_paste_in_after_the_frame_on_screen() {
        let mut app = app_with(doc(4), ScopeChoice::ThisFrame, Vec::new(), 0);
        let reds = |app: &App| {
            app.editor
                .doc
                .frames
                .iter()
                .map(|f| f.pixels.get_pixel(0, 0).0[0])
                .collect::<Vec<u8>>()
        };
        assert_eq!(app.paste_frames(), None, "nothing copied yet");

        app.copy_frames(&[0, 1]);
        assert_eq!(reds(&app), vec![0, 1, 2, 3], "copying edits nothing");

        app.playhead = 2;
        assert!(app.paste_frames().is_some());
        assert_eq!(reds(&app), vec![0, 1, 2, 0, 1, 3], "the run lands in order");
        assert_eq!(app.playhead, 4, "on the last pasted frame");
        assert_eq!(app.selection, vec![3, 4], "the pasted run stays picked");

        assert!(app.paste_frames().is_some(), "the clipboard is still there");
        assert_eq!(reds(&app), vec![0, 1, 2, 0, 1, 0, 1, 3]);
    }

    /// Regression: the band click read `bands_model[pixel_row]`, so with more
    /// than one overlay - two packed onto row 0 side by side, a third on row 1 -
    /// clicking the second or third band selected the wrong overlay or nothing.
    #[test]
    fn band_click_hits_the_band_under_the_point() {
        let pitch = cell_pitch(THUMB_BOX, 1.0);
        let mid = |frame: usize| (frame as f64 + 0.5) * pitch;
        let bands = vec![
            Band {
                id: OverlayId(1),
                name: "A".into(),
                range: 0..3,
                selected: false,
                row: 0,
            },
            Band {
                id: OverlayId(2),
                name: "B".into(),
                range: 5..8,
                selected: false,
                row: 0,
            },
            Band {
                id: OverlayId(3),
                name: "C".into(),
                range: 1..6,
                selected: false,
                row: 1,
            },
        ];
        let y0 = BAND_H * 0.5;
        let y1 = BAND_H * 1.5;
        assert_eq!(band_at(&bands, mid(1), y0, pitch), Some(OverlayId(1)));
        assert_eq!(
            band_at(&bands, mid(6), y0, pitch),
            Some(OverlayId(2)),
            "2nd band on row 0"
        );
        assert_eq!(band_at(&bands, mid(4), y0, pitch), None, "gap between them");
        assert_eq!(
            band_at(&bands, mid(3), y1, pitch),
            Some(OverlayId(3)),
            "band on row 1"
        );
        assert_eq!(
            band_at(&bands, mid(3), BAND_H * 2.5, pitch),
            None,
            "empty row"
        );
    }

    /// Strip zoom scales the band hit-test pitch: a point that lands on frame 6
    /// at 1x sits on frame 3 when the strip is drawn at half size.
    #[test]
    fn band_hit_test_follows_strip_zoom() {
        let pitch = cell_pitch(THUMB_BOX, 1.0);
        let bands = vec![
            Band {
                id: OverlayId(1),
                name: "A".into(),
                range: 0..4,
                selected: false,
                row: 0,
            },
            Band {
                id: OverlayId(2),
                name: "B".into(),
                range: 5..8,
                selected: false,
                row: 0,
            },
        ];
        let x = (6.0 + 0.5) * pitch;
        let y = BAND_H * 0.5;
        assert_eq!(band_at(&bands, x, y, pitch), Some(OverlayId(2)));
        // Half the zoom: the thumbnails halve but the gaps between them do
        // not, so the same x is over frame 12, past every band.
        assert_eq!(band_at(&bands, x, y, cell_pitch(THUMB_BOX, 0.5)), None);
        // Double: the same x is over frame 3, inside band A.
        assert_eq!(
            band_at(&bands, x, y, cell_pitch(THUMB_BOX, 2.0)),
            Some(OverlayId(1))
        );
    }

    /// The divider shown while dragging promises the frames will land right
    /// next to a specific neighbor; `move_target_for_set` is what turns that
    /// promise into the post-removal index `Document::move_frames_to` needs.
    #[test]
    fn move_target_for_set_lands_where_the_divider_promised() {
        // Dragging frame 0 to the gap right after itself is a no-op.
        assert_eq!(move_target_for_set(&[0], 1), 0);
        // Forward, past several frames still in the way after 0 moves out.
        assert_eq!(move_target_for_set(&[0], 3), 2);
        // Backward: nothing before the target gap moved, so it is the
        // final index outright.
        assert_eq!(move_target_for_set(&[3], 1), 1);
        // Past the last frame of 5.
        assert_eq!(move_target_for_set(&[0], 5), 4);
        // A gappy selection of three: the gap sits before all of them, so
        // the whole run slides down by three.
        assert_eq!(move_target_for_set(&[2, 5, 7], 1), 1);
        // …and after two of them: the run lands two earlier than the gap.
        assert_eq!(move_target_for_set(&[2, 5, 7], 6), 4);
    }

    /// End-to-end through the real `Document::move_frames_to`: whatever gap
    /// the divider is drawn at, the dragged frames end up adjacent to
    /// exactly the neighbor the divider promised, dragging either direction
    /// and as a multi-frame selection.
    #[test]
    fn move_target_for_set_round_trips_through_move_frames_to() {
        let ids_at = |d: &Document| -> Vec<u8> {
            d.frames
                .iter()
                .map(|f| f.pixels.get_pixel(0, 0)[0])
                .collect()
        };

        // [0,1,2,3,4], drag 0 to the gap before 3 (gap 3): lands just before it.
        let mut d = doc(5);
        d.move_frames_to(&[0], move_target_for_set(&[0], 3));
        assert_eq!(ids_at(&d), vec![1, 2, 0, 3, 4]);

        // Same document, drag 3 to the gap before 1 (gap 1): lands just before it.
        let mut d = doc(5);
        d.move_frames_to(&[3], move_target_for_set(&[3], 1));
        assert_eq!(ids_at(&d), vec![0, 3, 1, 2, 4]);

        // Drag 0 to the gap past the last frame: lands at the very end.
        let mut d = doc(5);
        d.move_frames_to(&[0], move_target_for_set(&[0], 5));
        assert_eq!(ids_at(&d), vec![1, 2, 3, 4, 0]);

        // A three-frame selection dropped between frames 1 and 2.
        let mut d = doc(8);
        d.move_frames_to(&[2, 5, 7], move_target_for_set(&[2, 5, 7], 2));
        assert_eq!(ids_at(&d), vec![0, 1, 2, 5, 7, 3, 4, 6]);
    }

    /// "Move earlier"/"Move later" swaps the selection with the one
    /// unselected frame beside it: the gap aims just before (or after) that
    /// neighbour, and refuses at either edge of the timeline.
    #[test]
    fn selection_nudge_gap_swaps_with_the_beside_neighbour() {
        // Contiguous run 2..5 of 8: earlier aims before frame 1.
        assert_eq!(selection_nudge_gap(&[2, 3, 4], true, 8), Some(1));
        // Later aims past frame 4, the neighbour after the run.
        assert_eq!(selection_nudge_gap(&[2, 3, 4], false, 8), Some(6));
        // A single frame nudges exactly like the old move-by-one.
        assert_eq!(selection_nudge_gap(&[3], true, 8), Some(2));
        assert_eq!(selection_nudge_gap(&[3], false, 8), Some(5));
        // Already at an edge: nothing to swap with.
        assert_eq!(selection_nudge_gap(&[0, 1], true, 8), None);
        assert_eq!(selection_nudge_gap(&[6, 7], false, 8), None);
        // Every frame selected: the nudge would be a no-op anyway.
        assert_eq!(
            selection_nudge_gap(&(0..8).collect::<Vec<_>>(), true, 8),
            None
        );
        assert_eq!(
            selection_nudge_gap(&(0..8).collect::<Vec<_>>(), false, 8),
            None
        );
    }

    /// The full drag path at model level: the selection moves as one run,
    /// follows itself to its new home, and a drag in place does nothing.
    #[test]
    fn move_picked_moves_the_whole_selection_and_follows_it() {
        let ids = |app: &App| -> Vec<u8> {
            app.editor
                .doc
                .frames
                .iter()
                .map(|f| f.pixels.get_pixel(0, 0)[0])
                .collect()
        };

        // In place means the picked frames already form the run `to` starts:
        // [2,3,4] dropped at gap 2 would land exactly where they are.
        let mut app = app_with(doc(8), ScopeChoice::Range, vec![2, 3, 4], 3);
        assert!(app.move_picked(&[2, 3, 4], 2).is_none(), "already in place");

        // Dropping a gappy selection [2,5,7] between 0 and 1.
        let mut app = app_with(doc(8), ScopeChoice::Range, vec![2, 5, 7], 5);
        assert!(app.move_picked(&[2, 5, 7], 1).is_some());
        assert_eq!(ids(&app), vec![0, 2, 5, 7, 1, 3, 4, 6]);
        assert_eq!(
            app.selection,
            vec![1, 2, 3],
            "the selection is now the contiguous run at its new home"
        );
        assert_eq!(
            app.playhead, 2,
            "the frame the playhead was on (5) sits at index 2"
        );
        assert_eq!(app.scope, ScopeChoice::Range);

        // A one-frame drag with nothing selected moves just that frame.
        let mut app = app_with(doc(8), ScopeChoice::ThisFrame, Vec::new(), 4);
        assert!(app.move_picked(&[4], 1).is_some());
        assert_eq!(ids(&app), vec![0, 4, 1, 2, 3, 5, 6, 7]);
        assert!(app.selection.is_empty(), "no selection to carry");
        assert_eq!(app.playhead, 1);
    }

    /// "Move earlier" end to end: the run swaps with the neighbour before
    /// it and the selection keeps covering the same frames.
    #[test]
    fn move_selection_earlier_swaps_the_block_with_its_neighbour() {
        let ids = |app: &App| -> Vec<u8> {
            app.editor
                .doc
                .frames
                .iter()
                .map(|f| f.pixels.get_pixel(0, 0)[0])
                .collect()
        };
        let mut app = app_with(doc(8), ScopeChoice::Range, vec![2, 3, 4], 3);
        let gap = selection_nudge_gap(&[2, 3, 4], true, 8).unwrap();
        let to = move_target_for_set(&[2, 3, 4], gap);
        assert!(app.move_picked(&[2, 3, 4], to).is_some());
        assert_eq!(ids(&app), vec![0, 2, 3, 4, 1, 5, 6, 7]);
        assert_eq!(app.selection, vec![1, 2, 3]);
    }

    /// Ctrl+wheel / Ctrl+Up-Down feed `Msg::StripZoom(factor)`: the factor
    /// multiplies, the result clamps to the bounds, and `0.0` resets to 1x.
    #[test]
    fn strip_zoom_multiplies_clamps_and_resets() {
        let step = STRIP_ZOOM_STEP;
        assert!(
            (next_strip_zoom(1.0, step) - step).abs() < 1e-9,
            "one step in"
        );
        assert!(next_strip_zoom(1.0, 1.0 / step) < 1.0, "one step out");

        let mut z = 1.0;
        for _ in 0..50 {
            z = next_strip_zoom(z, step);
        }
        assert_eq!(z, STRIP_ZOOM_MAX, "clamped to the max");
        for _ in 0..99 {
            z = next_strip_zoom(z, 1.0 / step);
        }
        assert_eq!(z, STRIP_ZOOM_MIN, "clamped to the min");

        assert_eq!(next_strip_zoom(z, 0.0), 1.0, "0.0 resets to 1x");
    }

    /// The bands `DrawingArea` is exactly as wide as the zoomed strip, so its
    /// per-frame columns stay lined up with the thumbnails above them. Only
    /// the thumbnails zoom: the gap between two cells is the strip `Box`'s
    /// spacing, a fixed number of pixels, so the width is not linear in the
    /// zoom the way it was assumed to be.
    #[test]
    fn strip_width_zooms_the_thumbnails_and_not_the_gaps() {
        let width = |zoom| strip_width(10, cell_pitch(THUMB_BOX, zoom));
        assert_eq!(width(1.0), 10 * (THUMB_BOX + THUMB_SPACING));
        assert_eq!(width(2.0), 10 * (2 * THUMB_BOX + THUMB_SPACING));
        assert_eq!(width(0.5), 10 * (THUMB_BOX / 2 + THUMB_SPACING));
    }

    /// Regression: the bands under the strip are drawn a `cell_pitch` per
    /// frame, so a cell has to really sit there at every zoom. A `GtkPicture`
    /// never measures smaller than its paintable — a size request alone left
    /// the cells at 1x below it — and a four-digit frame number could measure
    /// wider than the thumbnail over it. Either slid the whole band legend
    /// right of the frames it annotates, a little further with every column.
    fn strip_cells_sit_at_the_pitch_the_bands_are_drawn_at() {
        // A portrait frame's thumbnail: the narrow cell, and the one a
        // `THUMB_BOX`-wide pitch was wrong about even at 1x.
        let thumb = image::RgbaImage::new(41, THUMB_BOX as u32);
        let strip = gtk::Box::new(gtk::Orientation::Horizontal, THUMB_SPACING);
        for i in 0..6 {
            let picture = gtk::Picture::for_paintable(&texture_from(&thumb));
            let cell = gtk::Box::new(gtk::Orientation::Vertical, 2);
            cell.append(&picture);
            // Four digits: wider than the thumbnail once the strip zooms out.
            let label = gtk::Label::new(Some(&format!("{}", 1000 + i)));
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            cell.append(&label);
            strip.append(&cell);
        }
        let window = gtk::Window::new();
        window.set_child(Some(&strip));
        for zoom in [STRIP_ZOOM_MIN, 0.5, 1.0, 2.0, STRIP_ZOOM_MAX] {
            for cell in thumb_children(&strip) {
                let picture = cell.first_child().and_downcast::<gtk::Picture>().unwrap();
                set_thumb_zoom(&picture, &thumb, zoom);
            }
            let (_, width, _, _) = strip.measure(gtk::Orientation::Horizontal, -1);
            strip.allocate(width, 400, -1, None);
            let pitch = cell_pitch(thumb.width() as i32, zoom);
            for (i, cell) in thumb_children(&strip).enumerate() {
                let bounds = cell.compute_bounds(&strip).expect("laid out");
                assert_eq!(
                    (bounds.x() as f64, bounds.width() as f64),
                    (i as f64 * pitch, pitch - THUMB_SPACING as f64),
                    "cell {i} at {zoom}x"
                );
            }
        }
        window.set_child(None::<&gtk::Widget>);
    }

    /// The sidebar heading swaps "Frame" for a summary of which frames are
    /// in scope once more than one is — the panel this titles trades
    /// per-frame overlay editing for a scoped delay edit at that point, so
    /// the heading has to say which frames the edit will land on.
    #[test]
    fn frame_scope_summary_names_the_range_or_the_numbers() {
        assert_eq!(
            frame_scope_summary(ScopeChoice::ThisFrame, &[3]),
            "Frame",
            "a single frame in scope reads as the plain frame view"
        );
        assert_eq!(
            frame_scope_summary(ScopeChoice::Range, &[]),
            "Frame",
            "an empty scope falls back the same way"
        );
        assert_eq!(
            frame_scope_summary(ScopeChoice::AllFrames, &(0..12).collect::<Vec<_>>()),
            "All 12 frames selected"
        );
        assert_eq!(
            frame_scope_summary(ScopeChoice::Range, &[2, 3, 4, 5]),
            "Frames 3–6 selected",
            "a contiguous run reads as a range, 1-based and inclusive"
        );
        assert_eq!(
            frame_scope_summary(ScopeChoice::Range, &[1, 4, 7]),
            "Frames 2, 5, 8 selected",
            "a gappy pick small enough to read spells out the numbers"
        );
        assert_eq!(
            frame_scope_summary(ScopeChoice::Range, &(0..20).step_by(2).collect::<Vec<_>>()),
            "10 frames selected",
            "too many gappy picks to list falls back to a count"
        );
    }

    /// "Add frames from file" routes still images to the synchronous one-frame
    /// splice and everything with possible motion to the async import pipeline.
    #[test]
    fn still_images_bypass_the_import_pipeline() {
        for ext in ["png", "jpg", "jpeg", "PNG", "bmp", "tiff", "webp.png"] {
            assert!(
                is_still_image(Path::new(&format!("x.{ext}"))),
                "{ext} is a still image"
            );
        }
        for ext in ["gif", "mp4", "mov", "webm", "webp", "mkv", "apng", ""] {
            assert!(
                !is_still_image(Path::new(&format!("x.{ext}"))),
                "{ext} must take the async LoadAppend path"
            );
        }
        assert!(!is_still_image(Path::new("noextension")));
    }

    /// Regression: a band's row used to be its index in the overlay list, so
    /// the tenth overlay drew ten rows down even when the nine below it covered
    /// entirely different frames.
    #[test]
    fn bands_pack_side_by_side_when_their_frames_do_not_overlap() {
        assert_eq!(
            pack_rows(&[0..5, 5..10, 10..15]),
            vec![0, 0, 0],
            "a chain shares one row"
        );
        assert_eq!(
            pack_rows(&[0..10, 2..4, 3..8]),
            vec![0, 1, 2],
            "overlaps stack"
        );
        // Touching ranges are not overlapping ranges: 0..5 ends where 5..10 starts.
        assert_eq!(pack_rows(&[0..5, 3..9, 5..7, 9..12]), vec![0, 1, 0, 0]);
        assert!(pack_rows(&[]).is_empty());
    }

    /// An overlay edit lands where the scope reaches into the overlay's own
    /// range; when the scope does not reach the frame on screen, that frame
    /// wins. This is what makes a one-frame drag stay a one-frame drag.
    #[test]
    fn an_overlay_edit_lands_where_the_scope_reaches() {
        // All-frames scope over a full-range overlay: everything, in place.
        assert_eq!(overlay_edit_span(0..10, 0..10, 3), 0..10);
        // This-frame scope inside the overlay: the one frame.
        assert_eq!(overlay_edit_span(0..10, 4..5, 4), 4..5);
        // A range scope reaching partway: the overlap.
        assert_eq!(overlay_edit_span(0..10, 5..20, 6), 5..10);
        // Scope elsewhere, playhead still on the overlay: the frame on screen.
        assert_eq!(overlay_edit_span(0..10, 7..9, 2), 2..3);
        // Scope elsewhere and the overlay nowhere near: nothing to edit.
        assert_eq!(overlay_edit_span(7..9, 0..2, 5), 0..0);
    }

    /// The canvas maps widget pixels to image pixels; every gesture and every
    /// handle depends on it agreeing with `ContentFit::ScaleDown`.
    #[test]
    fn the_canvas_mapping_centres_and_never_enlarges() {
        // A picture smaller than its widget is centred at 1:1, not blown up.
        let (scale, ox, oy) = canvas_map((400.0, 300.0), (100.0, 100.0));
        assert_eq!(scale, 1.0);
        assert_eq!((ox, oy), (150.0, 100.0));

        // A larger one is scaled down to fit the tighter axis and letterboxed.
        let (scale, ox, oy) = canvas_map((400.0, 300.0), (800.0, 400.0));
        assert_eq!(scale, 0.5);
        assert_eq!((ox, oy), (0.0, 50.0));

        // Round trip: a click at the centre of the widget is the centre pixel.
        let point = |x: f64| (x - ox) / scale;
        assert_eq!(point(200.0), 400.0);

        // Degenerate sizes must not divide by zero or hand back a NaN.
        let (scale, ..) = canvas_map((400.0, 300.0), (0.0, 0.0));
        assert!(scale.is_finite() && scale > 0.0);
    }

    #[test]
    fn handles_sit_on_the_corners_and_resizing_pins_the_opposite_one() {
        let t = Transform::at(10.0, 20.0, 100.0, 50.0);
        assert!(contains(t, 10.0, 20.0) && contains(t, 110.0, 70.0));
        assert!(!contains(t, 9.0, 20.0));

        assert_eq!(handle_at(t, 10.0, 20.0, 4.0), Some(0), "top left");
        assert_eq!(handle_at(t, 110.0, 70.0, 4.0), Some(3), "bottom right");
        assert_eq!(
            handle_at(t, 60.0, 45.0, 4.0),
            None,
            "the middle is a move, not a resize"
        );

        // Dragging the top left by (5, 5) moves the origin and shrinks the box;
        // the bottom right corner does not budge.
        let dragged = resize_corner(t, 0, 5.0, 5.0, false, false);
        assert_eq!((dragged.x, dragged.y), (15.0, 25.0));
        assert_eq!(
            (dragged.x + dragged.w, dragged.y + dragged.h),
            (110.0, 70.0)
        );

        let dragged = resize_corner(t, 3, -20.0, -10.0, false, false);
        assert_eq!((dragged.x, dragged.y), (10.0, 20.0), "the origin is pinned");
        assert_eq!((dragged.w, dragged.h), (80.0, 40.0));
    }

    /// The two modifiers Impasto's grip hint promises. Shift takes the larger
    /// of the two scale factors; Ctrl pins the centre instead of a corner.
    #[test]
    fn shift_keeps_the_aspect_ratio_and_ctrl_resizes_about_the_center() {
        let t = Transform::at(0.0, 0.0, 100.0, 50.0);

        // The x drag doubles the width, the y drag would only grow the height
        // by a tenth, so the aspect clamp takes the doubling for both.
        let square = resize_corner(t, 3, 100.0, 5.0, true, false);
        assert_eq!((square.w, square.h), (200.0, 100.0));
        assert_eq!(
            (square.x, square.y),
            (0.0, 0.0),
            "the origin is still pinned"
        );

        // From the centre the drag counts on both sides, and the centre holds.
        let centred = resize_corner(t, 3, 25.0, 10.0, false, true);
        assert_eq!((centred.w, centred.h), (150.0, 70.0));
        assert_eq!(centred.center(), t.center());

        // Dragging a corner past its opposite flips rather than clamping.
        let flipped = resize_corner(t, 3, -150.0, 0.0, false, false);
        assert!(flipped.w < 0.0, "{flipped:?}");
    }

    /// A rotated overlay has to be grabbed where it looks, and a grip drag has
    /// to leave the opposite corner on the pixel it was already on.
    #[test]
    fn a_rotated_overlay_is_hit_and_resized_in_its_own_frame() {
        let mut t = Transform::at(10.0, 45.0, 80.0, 10.0);
        t.angle = std::f32::consts::FRAC_PI_2; // a quarter turn: wide becomes tall

        // The un-rotated box covers (10, 45); after the turn that point is
        // outside it and the box reaches up and down through the centre.
        let (cx, cy) = t.center();
        assert!(contains(t, cx, cy - 30.0), "tall after the turn");
        assert!(!contains(t, cx - 30.0, cy), "no longer wide");

        // The bottom-right grip is where the drawing puts it.
        let grip = oriented_corners(t)[3];
        assert_eq!(handle_at(t, grip.0, grip.1, 2.0), Some(3));

        // Drag it and the top-left grip stays on its pixel.
        let anchor_before = oriented_corners(t)[0];
        let resized = pin_anchor(t, resize_corner(t, 3, 20.0, 6.0, false, false), 3, false);
        let anchor_after = oriented_corners(resized)[0];
        assert!(
            (anchor_before.0 - anchor_after.0).abs() < 0.01
                && (anchor_before.1 - anchor_after.1).abs() < 0.01,
            "{anchor_before:?} vs {anchor_after:?}"
        );
        assert_eq!(resized.angle, t.angle, "a resize is not a rotation");
    }

    /// Regression: the shortcuts controller ran in the bubble phase, so the
    /// widget holding the focus handled the key first. GTK activates a focused
    /// button on Space and puts the initial focus on the first one it finds, so
    /// Space straight after opening a document added an overlay instead of
    /// playing. Only the capture phase gets there first.
    fn the_shortcuts_controller_runs_before_the_focused_widget() {
        assert_eq!(
            shortcuts_controller().propagation_phase(),
            gtk::PropagationPhase::Capture,
            "a bubble-phase controller loses Space to whatever holds the focus"
        );
        assert_ne!(
            gtk::EventControllerKey::new().propagation_phase(),
            gtk::PropagationPhase::Capture,
            "the default is what bit us, so the setter is doing real work"
        );
    }

    /// The capture phase means every keystroke passes here first, so the widgets
    /// that genuinely own their keys have to be handed them back — otherwise
    /// typing "a" in a caption inserts an arrow.
    fn a_text_entry_keeps_its_own_keystrokes_but_a_button_does_not() {
        assert!(focus_owns_keys(Some(gtk::Entry::new().upcast_ref())));
        assert!(focus_owns_keys(Some(gtk::TextView::new().upcast_ref())));
        assert!(focus_owns_keys(Some(
            gtk::SpinButton::with_range(0.0, 1.0, 1.0).upcast_ref()
        )));
        assert!(!focus_owns_keys(Some(
            gtk::Button::with_label("T").upcast_ref()
        )));
        assert!(
            !focus_owns_keys(None),
            "nothing focused is the shortcuts' case"
        );
    }

    /// Clicking a toolbar button should not park the focus on it either, so the
    /// keys the canvas cares about keep working after a tool is picked.
    fn a_clicked_toolbar_button_does_not_keep_the_keyboard_focus() {
        let button = gtk::Button::with_label("T");
        assert!(button.gets_focus_on_click(), "GTK's default is what bit us");
        no_focus_steal(&button);
        assert!(!button.gets_focus_on_click());
        assert!(button.is_focusable(), "Tab still reaches it");
    }

    /// The move cursor is the only hint that an overlay is draggable at all, so
    /// it has to win over "default" everywhere the press handler would start a
    /// drag — and lose to the grip and rotate glyphs, which it sits under.
    #[test]
    fn the_body_of_any_overlay_offers_the_move_cursor() {
        let selected = Transform::at(10.0, 10.0, 40.0, 20.0);
        let other = Transform::at(100.0, 60.0, 30.0, 30.0);
        let state = CanvasState {
            image: (200.0, 120.0),
            selected: Some(selected),
            movable: vec![selected, other],
            ..Default::default()
        };
        let at = |x, y| hover_cursor(&state, x, y, 4.0, false);

        assert_eq!(at(30.0, 20.0), "move", "inside the selected overlay");
        assert_eq!(at(115.0, 75.0), "move", "inside an unselected one too");
        assert_eq!(at(80.0, 40.0), "default", "bare canvas between them");

        // A corner of the selection outranks its own body.
        assert_eq!(at(10.0, 10.0), "nw-resize");
        // And the rotate modifier outranks both.
        assert_eq!(hover_cursor(&state, 30.0, 20.0, 4.0, true), "rotate");

        // An unselected overlay has no grips, so its corner is still a move,
        // and rotate needs a selection to act on.
        assert_eq!(at(100.0, 60.0), "move");
        let nothing_selected = CanvasState {
            selected: None,
            movable: vec![selected, other],
            ..Default::default()
        };
        assert_eq!(
            hover_cursor(&nothing_selected, 115.0, 75.0, 4.0, true),
            "move",
            "the rotate modifier over nothing selected still just moves"
        );
    }

    /// The rotate cursor is a texture, not a CSS name, so nothing catches a
    /// missing or unreadable PNG until the pointer is over a grip and the glyph
    /// silently falls back to "grab".
    fn the_rotate_cursor_texture_decodes() {
        let texture = gdk::Texture::from_bytes(&glib::Bytes::from_static(ROTATE_CURSOR))
            .expect("resources/rotate-handle.png decodes");
        assert_eq!(
            (texture.width(), texture.height()),
            (32, 32),
            "re-render it at 32x32; see resources/README.md"
        );
        assert!(rotate_cursor().is_some());
    }

    /// The gresource is embedded and registered by hand (no build.rs step),
    /// so a stale or malformed `icons.gresource` would otherwise only show up
    /// as tool buttons silently falling back to GTK's "missing image" glyph.
    fn the_tool_icons_resolve() {
        register_tool_icons();
        let theme = gtk::IconTheme::for_display(&gdk::Display::default().expect("display"));
        for icon in [
            "tool-text-symbolic",
            "tool-rect-symbolic",
            "tool-ellipse-symbolic",
            "tool-arrow-symbolic",
            "tool-crop-symbolic",
        ] {
            assert!(theme.has_icon(icon), "missing bundled icon: {icon}");
        }
    }

    /// The resize glyph follows the content round, so a corner of a rotated box
    /// still points along its own edge (Impasto's `ResizeCursors`).
    #[test]
    fn the_resize_cursor_turns_with_the_box() {
        assert_eq!(corner_cursor(0, 0.0), "nw-resize", "top left, axis aligned");
        assert_eq!(corner_cursor(3, 0.0), "se-resize", "bottom right");
        // A quarter turn clockwise carries the top-left corner round to the top right.
        assert_eq!(corner_cursor(0, std::f32::consts::FRAC_PI_2), "ne-resize");
        // Half a turn puts it opposite itself, and the glyph is symmetric.
        assert_eq!(corner_cursor(0, std::f32::consts::PI), "se-resize");
        // Corners never borrow an edge glyph, whatever the angle.
        for corner in 0..4 {
            for step in 0..24 {
                let cursor = corner_cursor(corner, step as f32 * 0.3);
                assert!(cursor.len() > "n-resize".len(), "{cursor} is an edge glyph");
            }
        }
    }

    /// The busy gates are the one thing standing between a running worker and
    /// a corrupted frame-index landing, so the classification is spelled out
    /// rather than trusted to the matches! arms staying in sync by eye.
    #[test]
    fn busy_gate_classification_covers_every_variant() {
        let press = || Msg::CanvasPress {
            x: 0.0,
            y: 0.0,
            scale: 1.0,
            state: gdk::ModifierType::empty(),
        };

        // Frame-moving edits: blocked in every kind of work.
        for msg in [
            Msg::Load(PathBuf::new()),
            Msg::LoadAppend(PathBuf::new()),
            Msg::Undo,
            Msg::Redo,
            Msg::DeleteSelection,
            Msg::FrameOp(FrameOp::Delete),
            Msg::MoveSelection { earlier: true },
            Msg::MoveSelectionTo { from: 0, gap: 2 },
            Msg::SetScopeDelay(5),
            Msg::SetAllFramesDelay(5),
            Msg::ApplyCrop,
            Msg::CropAll(0, 0, 1, 1),
            Msg::ApplyZoom,
            Msg::ApplyZoomAll,
            Msg::ApplyShrink,
            Msg::DropEveryNth(2),
            Msg::SmartDrop(30),
            Msg::Resize(10, 10),
            Msg::InsertImageFrame(0, PathBuf::new()),
            Msg::MoveFrame(0, 1),
        ] {
            assert!(msg.changes_frames(), "{msg:?} must wait");
            assert!(msg.requires_idle(), "{msg:?} must wait");
        }

        // Snapshot/dialog openers: blocked too, or the follow-up edit lands
        // against a stale canvas.
        for msg in [
            Msg::Open,
            Msg::Export,
            Msg::CropAllDialog,
            Msg::DelayAllDialog,
            Msg::ImportMore,
        ] {
            assert!(!msg.changes_frames(), "{msg:?} does not move frames itself");
            assert!(msg.requires_idle(), "{msg:?} must wait");
        }

        // Overlay-only edits: live during resize/zoom (the Worked handler
        // scales them in), blocked only while an import would discard them.
        for msg in [
            Msg::AddOverlay(Tool::Text),
            Msg::EditText("hi".into()),
            Msg::SetOverlayProp(OverlayProp::TextSize(12.0)),
            Msg::ToggleCropTool,
            press(),
            Msg::CanvasDrag {
                x: 0.0,
                y: 0.0,
                state: gdk::ModifierType::empty(),
            },
            Msg::CanvasRelease,
        ] {
            assert!(
                !msg.changes_frames(),
                "{msg:?} stays live during frame work"
            );
            assert!(msg.edits_document(), "{msg:?} must wait during import");
        }

        // Pure view/transport messages: never blocked. `MoveFrameDialog` only
        // reads the frame count, which frame work never changes, so it needs
        // no more guarding than opening any other frame's context menu does.
        for msg in [
            Msg::Tick,
            Msg::Seek(0),
            Msg::TogglePlay,
            Msg::Toast(String::new()),
            Msg::SetScope(ScopeChoice::AllFrames),
            Msg::SelectOverlay(None),
            Msg::MoveFrameDialog(0),
        ] {
            assert!(
                !msg.changes_frames() && !msg.requires_idle() && !msg.edits_document(),
                "{msg:?} passes through"
            );
        }
    }

    /// Ctrl picks frames one at a time and Shift picks the run between the
    /// anchor and here, in either direction. Both were previously a single
    /// growing range, so Ctrl could not skip a frame and Shift could not shrink.
    #[test]
    fn ctrl_toggles_single_frames_and_shift_takes_a_run_either_way() {
        let mut picked = Vec::new();
        toggle_frame(&mut picked, 2);
        toggle_frame(&mut picked, 7);
        toggle_frame(&mut picked, 4);
        assert_eq!(picked, vec![2, 4, 7], "a set, sorted, with the gaps kept");
        toggle_frame(&mut picked, 4);
        assert_eq!(
            picked,
            vec![2, 7],
            "clicking a picked frame takes it back out"
        );

        // Shift measures from the anchor, so clicking before it is a run too.
        assert_eq!(run_between(5, 8), vec![5, 6, 7, 8]);
        assert_eq!(
            run_between(5, 2),
            vec![2, 3, 4, 5],
            "backwards is the same run"
        );
        assert_eq!(run_between(3, 3), vec![3]);

        // The scope keeps the gaps, and only widens to span them for an
        // overlay, which has to be contiguous.
        let scope = Scope::Frames(vec![2, 7]);
        assert_eq!(scope.resolve(0, 10), vec![2, 7]);
        assert_eq!(scope.span(0, 10), 2..8);
    }

    #[test]
    fn resize_completion_keeps_active_canvas_work_in_image_coordinates() {
        let mut drag = Some(Drag {
            mode: DragMode::Move,
            from: (10.0, 20.0),
            origin: Transform::at(5.0, 10.0, 20.0, 30.0),
            current: Transform::at(7.0, 12.0, 20.0, 30.0),
            moved: true,
        });
        let mut crop = Some((10.0, 20.0, 30.0, 40.0));

        scale_in_flight_canvas(&mut drag, &mut crop, 0.5, 2.0, 0.0, 0.0);

        let drag = drag.unwrap();
        assert_eq!(drag.from, (5.0, 40.0));
        assert_eq!(drag.origin, Transform::at(2.5, 20.0, 10.0, 60.0));
        assert_eq!(drag.current, Transform::at(3.5, 24.0, 10.0, 60.0));
        assert_eq!(crop, Some((5.0, 40.0, 15.0, 80.0)));
    }

    /// A crop landing mid-drag moves the origin instead of scaling it: the
    /// canvas keeps its resolution, but pixel (0, 0) is now somewhere else.
    #[test]
    fn crop_completion_shifts_active_canvas_work_by_the_kept_origin() {
        let mut drag = Some(Drag {
            mode: DragMode::Move,
            from: (10.0, 20.0),
            origin: Transform::at(5.0, 10.0, 20.0, 30.0),
            current: Transform::at(7.0, 12.0, 20.0, 30.0),
            moved: true,
        });
        let mut crop = Some((10.0, 20.0, 30.0, 40.0));

        scale_in_flight_canvas(&mut drag, &mut crop, 1.0, 1.0, 4.0, 6.0);

        let drag = drag.unwrap();
        assert_eq!(drag.from, (6.0, 14.0));
        assert_eq!(drag.origin, Transform::at(1.0, 4.0, 20.0, 30.0));
        assert_eq!(drag.current, Transform::at(3.0, 6.0, 20.0, 30.0));
        assert_eq!(crop, Some((6.0, 14.0, 30.0, 40.0)));
    }

    #[test]
    fn full_canvas_crop_is_not_an_edit() {
        assert!(!crop_changes_canvas((100, 80), (0, 0, 100, 80)));
        assert!(!crop_changes_canvas((100, 80), (0, 0, 500, 500)));
        assert!(crop_changes_canvas((100, 80), (1, 0, 99, 80)));
        assert!(crop_changes_canvas((100, 80), (0, 0, 99, 80)));
        assert!(!crop_changes_canvas((0, 0), (0, 0, 1, 1)));
    }

    #[test]
    fn canvas_rects_intersect_the_image_before_rounding() {
        assert_eq!(
            normalize_canvas_rect((100, 80), (-20.0, -10.0, 50.0, 30.0)),
            Some((0, 0, 30, 20))
        );
        assert_eq!(
            normalize_canvas_rect((100, 80), (10.2, 20.2, 5.2, 4.2)),
            Some((10, 20, 6, 5))
        );
        assert_eq!(
            normalize_canvas_rect((100, 80), (-20.0, 0.0, 10.0, 10.0)),
            None
        );
    }

    #[test]
    fn resize_budget_uses_checked_rgba_size() {
        assert!(resize_fits_budget(100, 100, 10, 400_000));
        assert!(!resize_fits_budget(100, 100, 11, 400_000));
        assert!(!resize_fits_budget(
            u32::MAX,
            u32::MAX,
            usize::MAX,
            usize::MAX
        ));
    }

    /// Regression: the sidebar sets its widgets from the model, and each setter
    /// fires its own notify handler. Without this check, selecting an overlay
    /// pushed an undo step for every property it has.
    #[test]
    fn echoing_a_property_back_unchanged_is_not_an_edit() {
        let text = OverlayKind::Text(TextOverlay {
            text: "hi".into(),
            font: "Sans Bold".into(),
            size_px: 32.0,
            color: [255, 255, 255, 255],
            outline: Some(([0, 0, 0, 255], 2.0)),
            align: TextAlign::Center,
            antialias: true,
        });
        assert!(!OverlayProp::Font("Sans Bold".into()).changes(&text));
        assert!(OverlayProp::Font("Serif".into()).changes(&text));
        assert!(!OverlayProp::TextSize(32.0).changes(&text));
        assert!(OverlayProp::TextSize(33.0).changes(&text));
        assert!(!OverlayProp::Outline(Some(([0, 0, 0, 255], 2.0))).changes(&text));
        assert!(
            OverlayProp::Outline(None).changes(&text),
            "turning it off is a change"
        );
        assert!(!OverlayProp::Align(TextAlign::Center).changes(&text));
        assert!(OverlayProp::Align(TextAlign::Justify).changes(&text));
        assert!(!OverlayProp::Antialias(true).changes(&text));
        assert!(OverlayProp::Antialias(false).changes(&text));

        let shape = OverlayKind::Shape(ShapeOverlay {
            shape: Shape::Rect,
            fill: None,
            stroke: Some(([255, 60, 60, 255], 3.0)),
        });
        assert!(!OverlayProp::Fill(None).changes(&shape));
        assert!(OverlayProp::Fill(Some([1, 2, 3, 255])).changes(&shape));
        // A text property offered to a shape is not a change to anything.
        assert!(!OverlayProp::Font("Serif".into()).changes(&shape));
    }

    /// Regression: adding an overlay hung the app. `update_view` sets the
    /// outline colour before the outline width, so the colour button's handler
    /// read a width of zero and sent `Outline(None)` — which the model *did*
    /// disagree with, so it was applied, which re-ran the sync, which sent the
    /// pair back the other way, for ever. A sync must emit nothing at all.
    /// GTK is single-threaded and `cargo test` is not, so every check that needs
    /// a widget hangs off this one entry point. Split them back into separate
    /// `#[test]`s and the suite segfaults on the second `gtk::init`.
    #[test]
    fn gtk_widget_regressions() {
        if gtk::init().is_err() {
            eprintln!("skipped: no display");
            return;
        }
        syncing_a_colour_and_width_pair_sends_nothing_back();
        the_rotate_cursor_texture_decodes();
        the_tool_icons_resolve();
        the_shortcuts_controller_runs_before_the_focused_widget();
        a_text_entry_keeps_its_own_keystrokes_but_a_button_does_not();
        a_clicked_toolbar_button_does_not_keep_the_keyboard_focus();
        thumb_children_skips_the_frames_popover();
        the_layer_list_is_bounded_by_its_own_row_height();
        the_layer_list_bounds_survive_the_list_emptying();
        strip_cells_sit_at_the_pitch_the_bands_are_drawn_at();
        the_strip_viewport_does_not_chase_the_focus();
    }

    /// Regression: the frame menu is a popover parented to the strip, and
    /// closing it moves the keyboard focus. A viewport that follows the focus
    /// answered every duplicate, delete and paste by scrolling the timeline
    /// somewhere else — reproducibly back to the first frame, with the strip
    /// scrolled a couple of thousand pixels along.
    fn the_strip_viewport_does_not_chase_the_focus() {
        let scroll = gtk::ScrolledWindow::builder()
            .child(&gtk::Box::new(gtk::Orientation::Horizontal, 0))
            .build();
        let viewport = scroll
            .child()
            .and_downcast::<gtk::Viewport>()
            .expect("the scroller wraps its child in a viewport");
        assert!(viewport.is_scroll_to_focus(), "GTK's default");
        dont_chase_focus(&scroll);
        assert!(!viewport.is_scroll_to_focus());
    }

    /// A document with dozens of overlays must scroll the layer list rather
    /// than push the editor for the layer that is picked off the panel, and
    /// a panel with no room left must scroll itself rather than squeeze the
    /// list down to a sliver. Both bounds are the list's own row height,
    /// measured, because a row is as tall as the theme makes it.
    fn the_layer_list_is_bounded_by_its_own_row_height() {
        let list = gtk::ListBox::new();
        assert_eq!(layer_list_heights(&list), (0, 0), "no rows, no size");
        let row_h = 40;
        let add = |count| {
            for _ in 0..count {
                let row = gtk::ListBoxRow::new();
                row.set_height_request(row_h);
                list.append(&row);
            }
        };
        let cap = row_h * LAYER_ROWS_SHOWN as i32;
        add(2);
        assert_eq!(
            layer_list_heights(&list),
            (2 * row_h, cap),
            "a list shorter than the floor asks for exactly its rows"
        );
        add(10);
        assert_eq!(
            layer_list_heights(&list),
            (row_h * LAYER_ROWS_KEPT as i32, cap),
            "twelve layers scroll between the floor and the ceiling"
        );
    }

    /// Regression: deleting the last overlay pins both bounds to zero, and
    /// the next overlay added then asks for a floor above that ceiling.
    /// Setting the floor first made GTK refuse it (`min_content_height`
    /// asserts against the standing `max_content_height`), leaving the list
    /// with a zero floor — it collapsed instead of holding its rows open.
    fn the_layer_list_bounds_survive_the_list_emptying() {
        let scroll = gtk::ScrolledWindow::new();
        let (row_h, cap) = (40, 40 * LAYER_ROWS_SHOWN as i32);
        set_layer_list_heights(&scroll, 2 * row_h, cap);
        assert_eq!(
            (scroll.min_content_height(), scroll.max_content_height()),
            (2 * row_h, cap)
        );
        set_layer_list_heights(&scroll, 0, 0);
        assert_eq!(
            (scroll.min_content_height(), scroll.max_content_height()),
            (0, 0),
            "an empty list asks for no height at all"
        );
        set_layer_list_heights(&scroll, row_h, cap);
        assert_eq!(
            (scroll.min_content_height(), scroll.max_content_height()),
            (row_h, cap),
            "a list refilling past a zeroed ceiling raises the ceiling first"
        );
    }

    /// Regression: a `Popover` parented to the strip (`rebuild_strip`'s
    /// per-frame context menu) turns up in `first_child()`/`next_sibling()`
    /// right along with the real thumbnails, one position ahead of them all.
    /// Counting it shifted every playhead/scope/selected border one frame
    /// behind whatever was actually clicked.
    fn thumb_children_skips_the_frames_popover() {
        let strip = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let popover = gtk::Popover::new();
        popover.set_parent(&strip);
        let cells: Vec<gtk::Box> = (0..3)
            .map(|_| {
                let cell = gtk::Box::new(gtk::Orientation::Vertical, 0);
                cell.add_css_class("thumb");
                strip.append(&cell);
                cell
            })
            .collect();
        let seen: Vec<gtk::Widget> = thumb_children(&strip).collect();
        assert_eq!(seen.len(), 3, "the popover must not count as a thumbnail");
        for (want, got) in cells.iter().zip(&seen) {
            assert_eq!(want.upcast_ref::<gtk::Widget>(), got, "order preserved");
        }
        popover.unparent();
    }

    fn syncing_a_colour_and_width_pair_sends_nothing_back() {
        let sent = Rc::new(RefCell::new(Vec::new()));
        let sync = Rc::new(Cell::new(false));
        let (colour, width) = (color_button(), width_spin());
        {
            let sent = sent.clone();
            connect_pair(&colour, &width, &sync, move |v| sent.borrow_mut().push(v));
        }

        // What update_view does for a text overlay with the default outline.
        sync.set(true);
        set_color(&colour, [0, 0, 0, 255]);
        set_spin(&width, 2.0);
        sync.set(false);
        assert!(
            sent.borrow().is_empty(),
            "the sync echoed: {:?}",
            sent.borrow()
        );

        // The user's own edits still get through.
        width.set_value(5.0);
        assert_eq!(*sent.borrow(), vec![Some(([0, 0, 0, 255], 5.0))]);
    }

    fn plan_1080p60() -> ImportPlan {
        ImportPlan {
            source: VideoInfo {
                width: 1920,
                height: 1080,
                fps: 60.0,
                duration_s: Some(90.0),
            },
            width: 1280,
            height: 720,
            fps: 3.79,
            cap: 341,
        }
    }

    /// The dialog earns its interruption by naming the file, not by being vague.
    #[test]
    fn the_warning_says_what_the_file_is() {
        let body = oversize_body("lecture.mp4", &plan_1080p60());
        for want in [
            "lecture.mp4",
            "runs 1:30",
            "1920×1080",
            "60 fps",
            "trim or crop",
        ] {
            assert!(body.contains(want), "missing {want:?} in:\n{body}");
        }
    }

    #[test]
    fn a_file_with_no_duration_still_reads_as_a_sentence() {
        let plan = ImportPlan {
            source: VideoInfo {
                width: 800,
                height: 600,
                fps: 30.0,
                duration_s: None,
            },
            width: 800,
            height: 600,
            fps: 30.0,
            cap: 2000,
        };
        assert!(oversize_body("stream.mkv", &plan).contains("no duration"));
        let summary = plan_summary(&plan, None);
        assert!(summary.contains("no duration"), "{summary}");
        assert!(!summary.contains("memory"), "no invented size: {summary}");
    }

    /// The preview is the whole point of the picker: it has to move with the
    /// resolution, and the memory figure has to be the real one.
    #[test]
    fn the_preview_tracks_the_chosen_resolution() {
        let source = VideoInfo {
            width: 1920,
            height: 1080,
            fps: 60.0,
            duration_s: Some(90.0),
        };
        let at = |w: u32, h: u32| {
            let options = ImportOptions {
                target: Some((w, h)),
                ..Default::default()
            };
            video::plan_for(source.clone(), &options)
        };

        let big = at(1280, 720);
        let small = at(640, 360);
        let budget = ImportOptions::default().max_bytes;
        assert!(big.bytes().unwrap() <= budget && small.bytes().unwrap() <= budget);
        // The budget is a ceiling, not a target: a smaller frame spends the
        // same memory on more frames rather than banking the difference.
        assert!(small.fps > big.fps, "{} vs {}", small.fps, big.fps);
        assert!(small.frames() > big.frames());

        let summary = plan_summary(&big, None);
        assert!(summary.contains("frames at"), "{summary}");
        assert!(summary.contains("in memory"), "{summary}");
        assert!(summary.contains("measuring the GIF size"), "{summary}");
        assert!(
            summary.contains("down from 60"),
            "says what it gave up: {summary}"
        );
    }

    /// With the rate pinned, a smaller frame is the saving the picker promises.
    #[test]
    fn a_chosen_rate_makes_resolution_the_lever() {
        let source = VideoInfo {
            width: 1920,
            height: 1080,
            fps: 60.0,
            duration_s: Some(90.0),
        };
        let at = |w: u32, h: u32| {
            let options = ImportOptions {
                target: Some((w, h)),
                fps: Some(4.0),
                ..Default::default()
            };
            video::plan_for(source.clone(), &options)
        };

        let big = at(1280, 720);
        let small = at(480, 270);
        assert_eq!(
            small.fps, big.fps,
            "the rate is the user's, not the budget's"
        );
        assert!(
            small.bytes().unwrap() * 4 < big.bytes().unwrap(),
            "{} vs {}",
            small.bytes().unwrap(),
            big.bytes().unwrap()
        );
    }

    /// A rate the budget cannot cover is refused, not trimmed. The clip is the
    /// user's; the settings are the thing that has to give.
    #[test]
    fn an_over_budget_rate_is_refused_rather_than_trimmed() {
        let source = VideoInfo {
            width: 1920,
            height: 1080,
            fps: 60.0,
            duration_s: Some(90.0),
        };
        let options = ImportOptions {
            target: Some((1280, 720)),
            fps: Some(30.0),
            ..Default::default()
        };
        let plan = video::plan_for(source, &options);

        assert_eq!(plan.fps, 30.0, "the rate the user asked for");
        assert!(plan.over_budget());
        assert_eq!(
            plan.frames(),
            Some(2700),
            "the whole clip, not a capped count"
        );

        let summary = plan_summary(&plan, None);
        assert!(
            summary.contains("more memory than an import may use"),
            "{summary}"
        );
        assert!(
            summary.contains("smaller size or a lower frame rate"),
            "{summary}"
        );
        assert!(
            !summary.contains("dropped"),
            "nothing is dropped: {summary}"
        );
        assert!(
            !summary.contains("measuring"),
            "not measured either: {summary}"
        );
    }

    /// The budget comes from the user's settings file, so the same clip can be
    /// fine on one machine and refused on another.
    #[test]
    fn the_budget_comes_from_settings() {
        let source = VideoInfo {
            width: 1920,
            height: 1080,
            fps: 30.0,
            duration_s: Some(20.0),
        };
        let at = |mb: usize| {
            let options = ImportOptions {
                target: Some((1280, 720)),
                fps: Some(30.0),
                max_bytes: mb << 20,
                ..Default::default()
            };
            video::plan_for(source.clone(), &options)
        };
        assert!(at(256).over_budget(), "a tight budget refuses it");
        assert!(!at(4096).over_budget(), "a generous one takes it");
    }

    /// Only a real encode ever puts a size on screen. Until one lands the
    /// summary says so, rather than showing a number nobody measured.
    #[test]
    fn a_size_is_shown_only_once_it_has_been_measured() {
        let plan = plan_1080p60();
        let pending = plan_summary(&plan, None);
        assert!(pending.contains("measuring"), "{pending}");
        assert!(!pending.contains("roughly"), "no guess: {pending}");
        assert!(!pending.contains('–'), "no range: {pending}");

        let measured = plan_summary(&plan, Some(7_340_032));
        assert!(measured.contains("7 MB as a GIF"), "{measured}");
        assert!(!measured.contains("measuring"), "{measured}");
    }

    /// The picker is allowed to squash: an explicit target overrides the aspect.
    #[test]
    fn a_custom_size_may_change_the_aspect_ratio() {
        let source = VideoInfo {
            width: 1920,
            height: 1080,
            fps: 30.0,
            duration_s: Some(10.0),
        };
        let options = ImportOptions {
            target: Some((600, 600)),
            ..Default::default()
        };
        let plan = video::plan_for(source, &options);
        assert_eq!((plan.width, plan.height), (600, 600));
    }

    /// Odd numbers reach ffmpeg's scaler as even ones or not at all.
    #[test]
    fn odd_targets_are_rounded_to_even() {
        let source = VideoInfo {
            width: 1920,
            height: 1080,
            fps: 30.0,
            duration_s: Some(10.0),
        };
        let options = ImportOptions {
            target: Some((641, 361)),
            ..Default::default()
        };
        let plan = video::plan_for(source, &options);
        assert_eq!((plan.width % 2, plan.height % 2), (0, 0), "{plan:?}");
    }

    #[test]
    fn presets_only_offer_sizes_that_fit_inside_the_source() {
        let source = VideoInfo {
            width: 1920,
            height: 1080,
            fps: 30.0,
            duration_s: Some(10.0),
        };
        let presets = video::size_presets(&source);
        assert_eq!(presets[0], (1280, 720));
        assert!(presets.iter().all(|(w, h)| *w <= 1920 && *h < 1080));
        assert!(presets.iter().all(|(w, h)| w % 2 == 0 && h % 2 == 0));

        let already_small = VideoInfo {
            width: 320,
            height: 240,
            fps: 30.0,
            duration_s: None,
        };
        assert!(
            video::size_presets(&already_small).is_empty(),
            "nothing to step down to"
        );
    }
}
