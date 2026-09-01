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
    Document, Editor, Frame, OverlayId, OverlayKind, Scope, Shape, ShapeOverlay, TextAlign,
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

const THUMB_W: i32 = crate::core::model::THUMB_W as i32;
const THUMB_SPACING: i32 = 4;
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
.tnum { font-feature-settings: 'tnum'; }
.bind-conflict { color: @error_color; }
";

/// How many overlay rows the band area shows before it starts scrolling. Past
/// this the list is taller than the thumbnails it annotates.
const BANDS_COLLAPSED_ROWS: usize = 5;
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
    ExtendSelection(usize),
    SetScope(ScopeChoice),
    AddOverlay(Tool),
    SelectOverlay(Option<OverlayId>),
    FrameOp(FrameOp),
    /// A frame's own context menu acts on that frame, not on the scope.
    FrameOpAt(usize, FrameOp),
    SetFrameDelay(usize, u16),
    EditText(String),
    /// Every overlay property the sidebar can change, in one message: they all
    /// do the same thing to history and the view.
    SetOverlayProp(OverlayProp),
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
    ApplyCrop,
    ApplyZoom,
    /// Open the crop-all dialog. The dialog needs the live canvas size, which
    /// only the model knows — action closures do not.
    CropAllDialog,
    /// Crop every frame to this box, the four dialog fields in pixels.
    CropAll(u32, u32, u32, u32),
    DropEveryNth(usize),
    SmartDrop(usize),
    Resize(u32, u32),
    SetKeymap(Box<Keymap>),
    Toast(String),
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
                | Msg::Undo
                | Msg::Redo
                | Msg::DeleteSelection
                | Msg::FrameOp(_)
                | Msg::FrameOpAt(_, _)
                | Msg::SetFrameDelay(_, _)
                | Msg::ApplyCrop
                | Msg::CropAll(_, _, _, _)
                | Msg::ApplyZoom
                | Msg::DropEveryNth(_)
                | Msg::SmartDrop(_)
                | Msg::Resize(_, _)
        )
    }

    /// Operations whose result would be stale or whose follow-up message would
    /// be discarded while an import, resize, or zoom owns the document.
    fn requires_idle(&self) -> bool {
        self.changes_frames() || matches!(self, Msg::Open | Msg::Export | Msg::CropAllDialog)
    }

    fn edits_document(&self) -> bool {
        matches!(
            self,
            Msg::Undo
                | Msg::Redo
                | Msg::AddOverlay(_)
                | Msg::FrameOp(_)
                | Msg::FrameOpAt(_, _)
                | Msg::SetFrameDelay(_, _)
                | Msg::EditText(_)
                | Msg::SetOverlayProp(_)
                | Msg::DeleteSelection
                | Msg::CanvasPress { .. }
                | Msg::CanvasDrag { .. }
                | Msg::CanvasRelease
                | Msg::ToggleCropTool
                | Msg::ApplyCrop
                | Msg::ApplyZoom
                | Msg::CropAll(_, _, _, _)
                | Msg::DropEveryNth(_)
                | Msg::SmartDrop(_)
                | Msg::Resize(_, _)
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
) {
    if (fx, fy) == (1.0, 1.0) {
        return;
    }
    if let Some((x, y, w, h)) = crop_rect {
        (*x, *y, *w, *h) = (*x * fx, *y * fy, *w * fx, *h * fy);
    }
    if let Some(drag) = drag {
        drag.from = (drag.from.0 * fx, drag.from.1 * fy);
        for transform in [&mut drag.origin, &mut drag.current] {
            transform.x *= fx;
            transform.y *= fy;
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
}

impl FrameWork {
    /// The history label, marked: the toast translates it when it is shown.
    fn label(&self) -> &'static str {
        match self {
            FrameWork::Resize(..) => n("Resized"),
            FrameWork::Zoom { .. } => n("Zoomed"),
        }
    }

    /// How many frames the edit will claim.
    fn touched(&self, doc: &Document) -> usize {
        match self {
            FrameWork::Resize(..) => doc.frames.len(),
            FrameWork::Zoom { frames, .. } => frames.len(),
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
            FrameWork::Zoom { .. } => (1.0, 1.0),
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
    rev: u64,
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
    tool_buttons: Vec<(Tool, gtk::Button)>,
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
            rev: 0,
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
            BusyKind::Resize | BusyKind::Zoom => {
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
                let cancel = Arc::new(AtomicBool::new(false));
                self.busy = Some(Busy {
                    kind: BusyKind::Import,
                    done: 0,
                    total: None,
                    cancel: Some(cancel.clone()),
                });
                plan_import(path, self.import_options(), cancel, &sender);
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
                let range = self.scope_span();
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
                let touched = range.len();
                let added = match tool {
                    Tool::Text => n("Text added"),
                    Tool::Rect => n("Rectangle added"),
                    Tool::Ellipse => n("Ellipse added"),
                    Tool::Arrow => n("Arrow added"),
                };
                let (change, id) = self.editor.edit(added, touched, |d| {
                    d.add_overlay(name, kind, transform, range)
                });
                self.selected_overlay = Some(id);
                self.after_edit();
                sender.input(Msg::Toast(change.message()));
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
                    let touched = self.editor.doc.overlay(id).map_or(0, |o| o.range.len());
                    let (change, _) = self.editor.edit(n("Overlay deleted"), touched, |d| {
                        d.remove_overlay(id);
                    });
                    self.after_edit();
                    sender.input(Msg::Toast(change.message()));
                } else if !self.selection.is_empty() {
                    let frames = std::mem::take(&mut self.selection);
                    let touched = frames.len();
                    let (change, _) = self.editor.edit(n("Frames deleted"), touched, |d| {
                        d.delete_frames_at(&frames)
                    });
                    self.playhead = self.playhead.min(self.frame_count().saturating_sub(1));
                    self.scope = ScopeChoice::ThisFrame;
                    self.after_edit();
                    sender.input(Msg::Toast(change.message()));
                }
            }
            Msg::FrameOp(op) => {
                let frames = self.scope_frames();
                self.run_frame_op(op, frames, &sender);
            }
            Msg::FrameOpAt(i, op) => self.run_frame_op(op, vec![i], &sender),
            Msg::SetFrameDelay(i, cs) => {
                if i >= self.frame_count() {
                    return;
                }
                let (change, _) =
                    // Translators: Past-tense edit name, used inside "{change} on {count} frames".
                    self.editor.edit(n("Delay set"), 1, |d| d.set_delay(i..i + 1, cs.max(1)));
                self.after_edit();
                sender.input(Msg::Toast(change.message()));
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
                sender.input(Msg::Toast(change.message()));
            }
            Msg::ApplyCrop => {
                let Some((x, y, w, h)) = self.crop_rect.take() else {
                    return;
                };
                let Some(rect) = normalize_canvas_rect(self.editor.doc.size(), (x, y, w, h)) else {
                    return;
                };
                self.crop_tool = false;
                if let Some(message) = self.apply_crop_rect(rect) {
                    sender.input(Msg::Toast(message));
                }
            }
            Msg::ApplyZoom => {
                let Some(rect) = self.crop_rect.take() else {
                    return;
                };
                let Some(rect) = normalize_canvas_rect(self.editor.doc.size(), rect) else {
                    return;
                };
                let work = FrameWork::Zoom {
                    frames: self.scope_frames(),
                    rect,
                };
                self.crop_tool = false;
                self.busy = Some(Busy {
                    kind: BusyKind::Zoom,
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
                if let Some(message) = self.apply_crop(x as f32, y as f32, w as f32, h as f32) {
                    sender.input(Msg::Toast(message));
                }
            }
            Msg::DropEveryNth(every) => {
                if self.frame_count() == 0 || every < 2 {
                    return;
                }
                let touched = self.frame_count() / every;
                let (change, _) = self
                    .editor
                    .edit(n("Frames removed"), touched, |d| d.drop_every_nth(every));
                self.playhead = 0;
                self.after_edit();
                sender.input(Msg::Toast(change.message()));
            }
            Msg::SmartDrop(percent) => {
                let count = self.frame_count() * percent.min(95) / 100;
                if count == 0 {
                    return;
                }
                let (change, _) = self
                    .editor
                    .edit(n("Frames removed"), count, |d| d.drop_low_motion(count));
                self.playhead = 0;
                self.after_edit();
                sender.input(Msg::Toast(change.message()));
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
            Msg::Toast(_) => {}
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
                frames,
            } = *done;
            let (fx, fy) = scale;
            let (change, _) = self.editor.edit(label, frames_touched, |d| {
                for (i, frame) in frames {
                    d.frames[i] = frame;
                }
                d.scale_overlays(fx, fy);
            });
            scale_in_flight_canvas(&mut self.drag, &mut self.crop_rect, fx, fy);
            self.after_edit();
            sender.input(Msg::Toast(change.message()));
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
                match *outcome {
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
        let toast = matches!(&msg, Msg::Toast(_)).then(|| match &msg {
            Msg::Toast(t) => t.clone(),
            _ => unreachable!(),
        });
        self.update(msg, sender.clone(), root);
        self.schedule_estimate(&sender);
        if let Some(text) = toast {
            let toast = adw::Toast::new(&text);
            toast.set_button_label(Some(t("Undo")));
            let s = sender.clone();
            toast.connect_button_clicked(move |_| s.input(Msg::Undo));
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
        widgets.crop_apply.set_sensitive(idle);
        widgets.zoom_apply.set_sensitive(idle);
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
        if *self.strip_keys.borrow() != keys {
            *self.strip_keys.borrow_mut() = keys;
            rebuild_strip(&widgets.strip, &self.editor.doc, &sender);
        }
        let in_scope = self.scope_frames();
        let mut child = widgets.strip.first_child();
        let mut i = 0;
        while let Some(thumb) = child {
            set_class(&thumb, "playhead", i == self.playhead);
            set_class(
                &thumb,
                "in-scope",
                in_scope.contains(&i) && i != self.playhead,
            );
            set_class(&thumb, "selected", self.selection.contains(&i));
            child = thumb.next_sibling();
            i += 1;
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
        widgets.bands.set_content_width(strip_width(count));
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

        let (w, h) = self.editor.doc.size();
        let selected = self
            .selected_overlay
            .and_then(|id| self.editor.doc.overlay(id));
        let kind = selected.map(|o| o.kind.clone());
        widgets.overlay_group.set_visible(kind.is_some());
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
        let crop = self.crop_rect.filter(|(_, _, w, h)| *w >= 2.0 && *h >= 2.0);
        widgets.crop_group.set_visible(self.crop_tool);
        widgets.crop_label.set_label(&match crop {
            Some((x, y, w, h)) => format!(
                "{}\n{}",
                fill(
                    t("{width} × {height} at {x}, {y}"),
                    &[
                        ("width", &format!("{w:.0}")),
                        ("height", &format!("{h:.0}")),
                        ("x", &format!("{x:.0}")),
                        ("y", &format!("{y:.0}")),
                    ],
                ),
                t(
                    "Crop resizes every frame; zoom fills the canvas from this box on the \
                   frames in scope."
                ),
            ),
            None => t("Drag a box on the canvas.").into(),
        });

        {
            let keys = self.keymap.borrow();
            let mut state = widgets.canvas_state.borrow_mut();
            state.image = (w as f32, h as f32);
            state.selected = selected.map(|o| o.transform).filter(|_| !self.crop_tool);
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

    /// The frame operations, shared by the toolbar menu (which acts on the
    /// scope) and a frame's own context menu (which acts on that frame).
    fn run_frame_op(&mut self, op: FrameOp, frames: Vec<usize>, sender: &ComponentSender<Self>) {
        if self.frame_count() == 0 || frames.is_empty() {
            return;
        }
        let touched = frames.len();
        let (first, last) = (frames[0], frames[frames.len() - 1]);
        let (label, playhead) = match op {
            FrameOp::Delete => (n("Frames deleted"), first),
            FrameOp::Duplicate => (n("Frames duplicated"), self.playhead),
            FrameOp::Reverse => (n("Frames reversed"), self.playhead),
        };
        let (change, _) = self.editor.edit(label, touched, |d| match op {
            FrameOp::Delete => d.delete_frames_at(&frames),
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
        sender.input(Msg::Toast(change.message()));
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
    /// One crop, applied to every frame. Synchronous on purpose: cropping
    /// copies the kept region rather than resampling it. A full-canvas crop is
    /// ignored so the dialog's defaults do not create an empty undo step.
    fn apply_crop(&mut self, x: f32, y: f32, w: f32, h: f32) -> Option<String> {
        let rect = normalize_canvas_rect(self.editor.doc.size(), (x, y, w, h))?;
        self.apply_crop_rect(rect)
    }

    /// A crop whose rectangle is already normalized to the canvas.
    fn apply_crop_rect(&mut self, rect: (u32, u32, u32, u32)) -> Option<String> {
        if !crop_changes_canvas(self.editor.doc.size(), rect) {
            return None;
        }
        let touched = self.frame_count();
        let (change, _) = self.editor.edit(n("Cropped"), touched, |d| {
            d.crop(rect.0, rect.1, rect.2, rect.3)
        });
        self.after_edit();
        Some(change.message())
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
            };
            Box::new(WorkDone {
                label: work.label(),
                frames_touched: frames.len(),
                scale: work.scale(&doc),
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

fn strip_width(count: usize) -> i32 {
    count as i32 * (THUMB_W + THUMB_SPACING)
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

/// What the strip is showing. Only a change here is worth a rebuild: overlay
/// edits move the document revision too, and there are hundreds of thumbnails.
fn strip_keys(doc: &Document) -> Vec<(u64, bool)> {
    doc.frames.iter().map(|f| (f.key, f.detached)).collect()
}

/// ponytail: the strip rebuilds a widget per frame when the frame list changes.
/// The thumbnails themselves are already built (see `Frame::new`), so this is a
/// hitch rather than a freeze; swap in a virtualized list when someone imports
/// something long enough to notice.
fn rebuild_strip(strip: &gtk::Box, doc: &Document, sender: &ComponentSender<App>) {
    while let Some(child) = strip.first_child() {
        strip.remove(&child);
    }
    let menu = gio::Menu::new();
    menu.append(Some(t("Delete this frame")), Some("frame.delete"));
    menu.append(Some(t("Duplicate this frame")), Some("frame.duplicate"));
    menu.append(Some(t("Set delay…")), Some("frame.delay"));
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
    ] {
        let action = gio::SimpleAction::new(name, None);
        let (sender, target) = (sender.clone(), target.clone());
        action.connect_activate(move |_, _| sender.input(Msg::FrameOpAt(target.get(), op)));
        group.add_action(&action);
    }
    let delay_action = gio::SimpleAction::new("delay", None);
    {
        let (sender, target, strip) = (sender.clone(), target.clone(), strip.clone());
        let delays: Vec<u16> = doc.frames.iter().map(|f| f.delay_cs).collect();
        delay_action.connect_activate(move |_, _| {
            let i = target.get();
            delay_dialog(&strip, i, delays.get(i).copied().unwrap_or(10), &sender);
        });
    }
    group.add_action(&delay_action);
    strip.insert_action_group("frame", Some(&group));

    for (i, frame) in doc.frames.iter().enumerate() {
        let thumb_h = frame.thumb.height() as i32;
        let picture = gtk::Picture::for_paintable(&texture_from(&frame.thumb));
        picture.set_size_request(THUMB_W, thumb_h.max(1));

        let cell = gtk::Box::new(gtk::Orientation::Vertical, 2);
        cell.add_css_class("thumb");
        cell.append(&picture);
        let label = gtk::Label::new(Some(&(i + 1).to_string()));
        label.add_css_class("dim-label");
        label.add_css_class("caption");
        label.add_css_class("tnum");
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

        let click = gtk::GestureClick::new();
        {
            let sender = sender.clone();
            click.connect_pressed(move |gesture, _, _, _| {
                let state = gesture.current_event_state();
                let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
                let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
                sender.input(match (shift, ctrl) {
                    (true, _) => Msg::ExtendSelection(i),
                    (_, true) => Msg::ToggleSelection(i),
                    _ => Msg::Seek(i),
                });
            });
        }
        cell.add_controller(click);

        // Right-click acts on the frame under the pointer, not on the scope,
        // which is the whole reason to have it as well as the ⋮ menu. The
        // popover is shared: one per frame is a widget tree per thumbnail.
        let secondary = gtk::GestureClick::new();
        secondary.set_button(gdk::BUTTON_SECONDARY);
        {
            let (sender, target, popover, strip) = (
                sender.clone(),
                target.clone(),
                popover.clone(),
                strip.clone(),
            );
            let cell = cell.clone();
            secondary.connect_pressed(move |_, _, x, y| {
                target.set(i);
                sender.input(Msg::Seek(i));
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
        Action::FrameReverse => Msg::FrameOp(FrameOp::Reverse),
        Action::ZoomToSelection => Msg::ApplyZoom,
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

    // Everything that makes the GIF smaller, in one place. "Halve frame rate"
    // used to sit in the frame menu pretending to be a rate control; it was
    // deleting every second frame, which is what these say they do.
    let optimize = gio::Menu::new();
    optimize.append(Some(t("Remove frames…")), Some("win.optimize-remove"));
    optimize.append(Some(t("Smart remove frames…")), Some("win.optimize-smart"));
    optimize.append(Some(t("Resize…")), Some("win.optimize-resize"));
    // Translators: The same crop the canvas tool applies, offered
    // document-wide from the Optimize menu.
    optimize.append(Some(t("Crop all frames…")), Some("win.optimize-crop"));
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
    let import_cancel = gtk::Button::from_icon_name("process-stop-symbolic");
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
    for tool in [Tool::Text, Tool::Rect, Tool::Ellipse, Tool::Arrow] {
        let button = gtk::Button::with_label(tool_letter(tool));
        button.add_css_class("flat");
        connect(&button, sender, move || Msg::AddOverlay(tool));
        rail.append(&button);
        tool_buttons.push((tool, button));
    }
    let crop_button = gtk::ToggleButton::with_label("C");
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
    frame_menu.append(Some(t("Reverse")), Some("win.frame-reverse"));
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
    let bands_model: Rc<RefCell<Vec<Band>>> = Rc::new(std::cell::RefCell::new(Vec::new()));
    let bands = gtk::DrawingArea::new();
    bands.set_content_height(0);
    {
        let bands_model = bands_model.clone();
        bands.set_draw_func(move |area, cr, _, _| {
            draw_bands(area, cr, &bands_model.borrow());
        });
    }
    let band_click = gtk::GestureClick::new();
    {
        let bands_model = bands_model.clone();
        let sender = sender.clone();
        band_click.connect_pressed(move |_, _, x, y| {
            let row = (y / BAND_H) as usize;
            let frame = (x / (THUMB_W + THUMB_SPACING) as f64) as usize;
            let hit = bands_model
                .borrow()
                .get(row)
                .filter(|band| band.range.contains(&frame))
                .map(|band| band.id);
            sender.input(Msg::SelectOverlay(hit));
        });
    }
    bands.add_controller(band_click);

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
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .child(&strip_column)
        .build();
    // The footer asks for what the strip actually needs, so this has to carry
    // the strip's own height out rather than reporting a scrollable minimum.
    strip_scroll.set_propagate_natural_height(true);
    footer.append(&strip_scroll);

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
    let group = adw::PreferencesGroup::builder()
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
    group.add(&text_row);

    // Overlay styling. Font choice covers bold and italic, because a font
    // description already carries weight and style: two toggles that fight the
    // font dialog over the same field would be the bug, not the feature.
    let overlay_group = adw::PreferencesGroup::builder()
        .title(t("Overlay"))
        .visible(false)
        .build();
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
    let crop_buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let crop_apply = gtk::Button::with_label(t("Crop all frames"));
    // Translators: Applies the zoom to the frames the scope control names, not the whole document.
    let zoom_apply = gtk::Button::with_label(t("Zoom scope"));
    connect(&crop_apply, sender, || Msg::ApplyCrop);
    connect(&zoom_apply, sender, || Msg::ApplyZoom);
    crop_buttons.append(&crop_apply);
    crop_buttons.append(&zoom_apply);
    let crop_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    crop_box.append(&crop_label);
    crop_box.append(&crop_buttons);
    crop_group.add(&crop_box);

    let doc_info = gtk::Label::new(Some(""));
    doc_info.add_css_class("dim-label");
    doc_info.set_wrap(true);
    doc_info.set_xalign(0.0);
    properties.append(&group);
    properties.append(&overlay_group);
    properties.append(&crop_group);
    properties.append(&doc_info);

    let split = adw::OverlaySplitView::builder()
        .sidebar_position(gtk::PackType::End)
        .content(&paned)
        .sidebar(&properties)
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
    ] {
        let action = gio::SimpleAction::new(name, None);
        let sender = sender.clone();
        action.connect_activate(move |_, _| sender.input(Msg::FrameOp(op)));
        actions.add_action(&action);
    }
    for (name, make) in [
        ("export", (|| Msg::Export) as fn() -> Msg),
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
        tool_buttons,
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
fn draw_bands(area: &gtk::DrawingArea, cr: &cairo::Context, bands: &[Band]) {
    let color = area.color();
    let pitch = (THUMB_W + THUMB_SPACING) as f64;
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

/// Per-frame delay, from the frame's own context menu.
fn delay_dialog(
    anchor: &impl IsA<gtk::Widget>,
    index: usize,
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
    let row = adw::ActionRow::builder()
        .title(fill(
            t("Frame {number}"),
            &[("number", &(index + 1).to_string())],
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
    dialog.choose(
        Some(anchor.as_ref()),
        gio::Cancellable::NONE,
        move |response| {
            if response == "apply" {
                sender.input(Msg::SetFrameDelay(index, spin.value() as u16));
            }
        },
    );
}

fn tool_letter(tool: Tool) -> &'static str {
    match tool {
        Tool::Text => "T",
        Tool::Rect => "R",
        Tool::Ellipse => "O",
        Tool::Arrow => "A",
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
            Msg::Undo,
            Msg::Redo,
            Msg::DeleteSelection,
            Msg::FrameOp(FrameOp::Delete),
            Msg::FrameOpAt(0, FrameOp::Delete),
            Msg::SetFrameDelay(0, 5),
            Msg::ApplyCrop,
            Msg::CropAll(0, 0, 1, 1),
            Msg::ApplyZoom,
            Msg::DropEveryNth(2),
            Msg::SmartDrop(30),
            Msg::Resize(10, 10),
        ] {
            assert!(msg.changes_frames(), "{msg:?} must wait");
            assert!(msg.requires_idle(), "{msg:?} must wait");
        }

        // Snapshot/dialog openers: blocked too, or the follow-up edit lands
        // against a stale canvas.
        for msg in [Msg::Open, Msg::Export, Msg::CropAllDialog] {
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

        // Pure view/transport messages: never blocked.
        for msg in [
            Msg::Tick,
            Msg::Seek(0),
            Msg::TogglePlay,
            Msg::Toast(String::new()),
            Msg::SetScope(ScopeChoice::AllFrames),
            Msg::SelectOverlay(None),
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

        scale_in_flight_canvas(&mut drag, &mut crop, 0.5, 2.0);

        let drag = drag.unwrap();
        assert_eq!(drag.from, (5.0, 40.0));
        assert_eq!(drag.origin, Transform::at(2.5, 20.0, 10.0, 60.0));
        assert_eq!(drag.current, Transform::at(3.5, 24.0, 10.0, 60.0));
        assert_eq!(crop, Some((5.0, 40.0, 15.0, 80.0)));
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
        the_shortcuts_controller_runs_before_the_focused_widget();
        a_text_entry_keeps_its_own_keystrokes_but_a_button_does_not();
        a_clicked_toolbar_button_does_not_keep_the_keyboard_focus();
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
