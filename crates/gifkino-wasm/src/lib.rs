//! The browser editor's handle on `gifkino-core`. `Editor` owns one document
//! and its history and performs every edit the desktop window performs, with
//! the same guards, history labels and frame counts; what the desktop keeps
//! as window state — playhead, selection, scope — the caller passes in.
//! `API.md` beside this crate is the contract for the JavaScript side.

mod frames;

use std::cell::RefCell;

use gifkino_core::fit::{self, FitMode};
use gifkino_core::gif::{self, ESTIMATE_SAMPLES, Encodable, ExportSettings};
use gifkino_core::handles::{self, DragKeys, Grip};
use gifkino_core::model::{Overlay, fresh_overlay, overlay_edit_span};
use gifkino_core::ops::{
    crop_changes_canvas, delays_for_fps, move_target_for_set, normalize_canvas_rect,
    selection_nudge_gap,
};
use gifkino_core::render::{self, TextRasterizer, no_text};
use gifkino_core::snap::Guides;
use gifkino_core::{
    Axis, Change, Document, Fade, Frame, OverlayId, OverlayKind, Scope, Shape, TextOverlay,
    Transform,
};
use image::RgbaImage;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

pub use frames::{Frames, Image};

#[wasm_bindgen(typescript_custom_section)]
const TYPES: &'static str = r#"
export type Rgba8 = [number, number, number, number];
export interface Transform { x: number; y: number; w: number; h: number; angle: number }
export type TextAlign = "left" | "center" | "right" | "justify";
export interface TextOverlay {
  text: string; font: string; sizePx: number; color: Rgba8;
  outline: [Rgba8, number] | null; align: TextAlign; antialias: boolean;
}
export type Shape = "rect" | "ellipse" | "arrow";
export interface ShapeOverlay { shape: Shape; fill: Rgba8 | null; stroke: [Rgba8, number] | null }
export type OverlayKind = ({ type: "text" } & TextOverlay) | ({ type: "shape" } & ShapeOverlay);
export interface Overlay {
  id: number; name: string; kind: OverlayKind; range: { start: number; end: number };
  transform: Transform; opacity: number; hidden: boolean;
}
export interface FrameInfo { delayCs: number; detached: boolean; opaque: boolean; key: number }
export interface Change { label: string; framesTouched: number }
export interface OverlayEdit { change: Change; overlay: number }
export interface OverlaysAdded { change: Change; overlays: number[]; overlay: number }
export interface FramesMoved { change: Change; at: number }
export type FitMode = "stretch" | "pad" | "growPad" | "growStretch";
export type Tool = "text" | "rect" | "ellipse" | "arrow";
export type Axis = "horizontal" | "vertical";
export type Fade = "in" | "out";
export type Grip = "move" | "rotate" | { resize: 0 | 1 | 2 | 3 };
export type Guide = "start" | "center" | "end";
export interface Guides { x: Guide | null; y: Guide | null }
export interface Dragged { transform: Transform; guides: Guides }
export interface ExportSettings {
  width?: number | null; speed?: number; colors?: number; dither?: boolean;
  loops?: number | null; lossy?: number;
}
export type TextRasterizer =
  (text: TextOverlay, width: number, height: number) => Uint8ClampedArray | Uint8Array;
"#;

// History labels: the same English msgids the desktop marks with `n(...)`,
// so a browser translation can key off the desktop's catalogs.
// ponytail: the strings are copied rather than shared; moving them into core
// would need the desktop's string extractor to scan core too.
const FRAMES_DELETED: &str = "Frames deleted";
const FRAMES_CUT: &str = "Frames cut";
const FRAMES_DUPLICATED: &str = "Frames duplicated";
const FRAMES_REVERSED: &str = "Frames reversed";
const FRAMES_PASTED: &str = "Frames pasted";
const FRAMES_MOVED: &str = "Frames moved";
const FRAME_MOVED: &str = "Frame moved";
const FRAME_ADDED: &str = "Frame added";
const FRAMES_ADDED: &str = "Frames added";
const FRAMES_REMOVED: &str = "Frames removed";
const DELAY_SET: &str = "Delay set";
const RESIZED: &str = "Resized";
const ZOOMED: &str = "Zoomed";
const CROPPED: &str = "Cropped";
const FRAMES_MIRRORED: &str = "Frames mirrored";
const FADED_IN: &str = "Faded in";
const FADED_OUT: &str = "Faded out";
const LOOP_CROSS_FADED: &str = "Loop cross-faded";
const OVERLAY_DELETED: &str = "Overlay deleted";
const OVERLAY_REORDERED: &str = "Overlay reordered";
const OVERLAY_COPIED: &str = "Overlay copied";
const OVERLAY_RESTYLED: &str = "Overlay restyled";
const OVERLAY_MOVED: &str = "Overlay moved";
const OVERLAY_ROTATED: &str = "Overlay rotated";
const OVERLAY_RESIZED: &str = "Overlay resized";
const TEXT_EDITED: &str = "Text edited";

/// The overlay kinds the toolbar adds.
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Tool {
    Text,
    Rect,
    Ellipse,
    Arrow,
}

impl Tool {
    fn shape(self) -> Option<Shape> {
        match self {
            Tool::Text => None,
            Tool::Rect => Some(Shape::Rect),
            Tool::Ellipse => Some(Shape::Ellipse),
            Tool::Arrow => Some(Shape::Arrow),
        }
    }

    /// The name a new overlay of this kind gets.
    fn name(self) -> &'static str {
        match self {
            Tool::Text => "Text",
            Tool::Rect => "Rectangle",
            Tool::Ellipse => "Ellipse",
            Tool::Arrow => "Arrow",
        }
    }

    fn added_label(self) -> &'static str {
        match self {
            Tool::Text => "Text added",
            Tool::Rect => "Rectangle added",
            Tool::Ellipse => "Ellipse added",
            Tool::Arrow => "Arrow added",
        }
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
enum GripArg {
    Move,
    Rotate,
    Resize(usize),
}

impl From<GripArg> for Grip {
    fn from(grip: GripArg) -> Self {
        match grip {
            GripArg::Move => Grip::Move,
            GripArg::Rotate => Grip::Rotate,
            GripArg::Resize(corner) => Grip::Resize(corner.min(3)),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FrameInfo {
    delay_cs: u16,
    detached: bool,
    opaque: bool,
    key: u64,
}

#[derive(Serialize)]
struct OverlayEdit {
    change: Change,
    overlay: OverlayId,
}

#[derive(Serialize)]
struct OverlaysAdded {
    change: Change,
    overlays: Vec<OverlayId>,
    overlay: OverlayId,
}

#[derive(Serialize)]
struct FramesMoved {
    change: Change,
    at: usize,
}

#[derive(Serialize)]
struct Dragged {
    transform: Transform,
    guides: Guides,
}

/// A thrown JavaScript `Error` carrying `message`.
pub(crate) fn error(message: impl AsRef<str>) -> JsError {
    JsError::new(message.as_ref())
}

/// `value` as a plain JavaScript value: structs as objects, `None` as `null`.
fn to_js<T: Serialize + ?Sized>(value: &T) -> Result<JsValue, JsError> {
    let serializer = serde_wasm_bindgen::Serializer::new().serialize_missing_as_null(true);
    value
        .serialize(&serializer)
        .map_err(|e| error(e.to_string()))
}

fn to_js_or_undefined<T: Serialize>(value: Option<T>) -> Result<JsValue, JsError> {
    value.map_or(Ok(JsValue::UNDEFINED), |v| to_js(&v))
}

fn from_js<T: DeserializeOwned>(value: JsValue) -> Result<T, JsError> {
    serde_wasm_bindgen::from_value(value).map_err(|e| error(e.to_string()))
}

fn indices(frames: &[u32]) -> Vec<usize> {
    frames.iter().map(|&i| i as usize).collect()
}

fn js_message(thrown: JsValue) -> String {
    thrown
        .dyn_ref::<js_sys::Error>()
        .map(|e| String::from(e.message()))
        .or_else(|| thrown.as_string())
        .unwrap_or_else(|| "the text rasterizer threw".into())
}

/// One call to the page's text rasterizer.
fn rasterize_text(
    callback: &js_sys::Function,
    text: &TextOverlay,
    w: u32,
    h: u32,
) -> Result<RgbaImage, String> {
    let overlay = to_js(text).map_err(|_| "could not pass the text overlay".to_string())?;
    let returned = callback
        .call3(&JsValue::NULL, &overlay, &w.into(), &h.into())
        .map_err(js_message)?;
    let bytes = js_sys::Uint8Array::new(&returned).to_vec();
    let len = bytes.len();
    RgbaImage::from_raw(w, h, bytes)
        .ok_or_else(|| format!("the text rasterizer returned {len} bytes for {w}x{h}"))
}

/// A document, its undo history and the frame clipboard.
#[wasm_bindgen]
pub struct Editor {
    history: gifkino_core::Editor,
    clipboard: Vec<Frame>,
    text: Option<js_sys::Function>,
    revision: u32,
}

impl Default for Editor {
    fn default() -> Self {
        Editor {
            history: gifkino_core::Editor::new(Document::default()),
            clipboard: Vec::new(),
            text: None,
            revision: 0,
        }
    }
}

/// Internals: the document, scope resolution and the history step.
impl Editor {
    fn doc(&self) -> &Document {
        &self.history.doc
    }

    fn frame_count(&self) -> usize {
        self.doc().frames.len()
    }

    /// `frames` as the desktop resolves a frame selection: sorted,
    /// deduplicated, stale indices dropped.
    fn scope(&self, frames: &[u32]) -> Vec<usize> {
        Scope::Frames(indices(frames)).resolve(0, self.frame_count())
    }

    /// The frames an edit to overlay `id` lands on, as the desktop's
    /// `edit_span`: where the scope reaches into the overlay's range, or the
    /// frame on screen when the scope does not reach it.
    fn edit_span(&self, id: OverlayId, frames: &[u32], playhead: usize) -> std::ops::Range<usize> {
        let range = self.doc().overlay(id).map_or(0..0, |o| o.range.clone());
        let scope = Scope::Frames(indices(frames)).span(playhead, self.frame_count());
        overlay_edit_span(range, scope, playhead)
    }

    /// Run `f` as one undo step.
    fn record<T>(
        &mut self,
        label: &str,
        frames_touched: usize,
        f: impl FnOnce(&mut Document) -> T,
    ) -> (Change, T) {
        self.revision = self.revision.wrapping_add(1);
        self.history.edit(label, frames_touched, f)
    }

    /// Land produced frames as one step, overlays following the canvas.
    fn land(
        &mut self,
        label: &str,
        frames: Vec<(usize, Frame)>,
        scale: (f32, f32),
        offset: (f32, f32),
    ) -> Option<Change> {
        let touched = frames.len();
        let (change, _) = self.record(label, touched, |d| d.land(frames, scale, offset));
        Some(change)
    }

    /// `f` handed the page's text rasterizer, or a blank one when none is
    /// set. A rasterizer that throws or returns the wrong size draws nothing
    /// for that overlay and fails the whole call afterwards.
    fn with_text<T>(&self, f: impl FnOnce(TextRasterizer<'_>) -> T) -> Result<T, JsError> {
        let Some(callback) = &self.text else {
            return Ok(f(&no_text));
        };
        let failure: RefCell<Option<String>> = RefCell::new(None);
        let rasterize = |text: &TextOverlay, w: u32, h: u32| {
            rasterize_text(callback, text, w, h).unwrap_or_else(|message| {
                failure.borrow_mut().get_or_insert(message);
                RgbaImage::new(w.max(1), h.max(1))
            })
        };
        let out = f(&rasterize);
        match failure.into_inner() {
            Some(message) => Err(error(message)),
            None => Ok(out),
        }
    }

    fn overlay_edit(
        &mut self,
        id: OverlayId,
        span: std::ops::Range<usize>,
        label: &str,
        edit: impl FnOnce(&mut Overlay),
    ) -> Result<JsValue, JsError> {
        if span.is_empty() {
            return Ok(JsValue::UNDEFINED);
        }
        let (change, (overlay, _)) =
            self.record(label, span.len(), |d| d.edit_on_frames(id, span, edit));
        to_js(&OverlayEdit { change, overlay })
    }

    fn frames_moved(&mut self, picked: Vec<usize>, to: usize) -> Result<JsValue, JsError> {
        let in_place = picked.iter().enumerate().all(|(k, &p)| p == to + k);
        if picked.is_empty() || in_place {
            return Ok(JsValue::UNDEFINED);
        }
        let (change, _) = self.record(FRAMES_MOVED, picked.len(), |d| {
            d.move_frames_to(&picked, to)
        });
        to_js(&FramesMoved { change, at: to })
    }
}

#[wasm_bindgen]
impl Editor {
    /// An empty document.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Editor {
        Editor::default()
    }

    /// The function that draws a text overlay; `null` draws no text.
    #[wasm_bindgen(js_name = setTextRasterizer)]
    pub fn set_text_rasterizer(
        &mut self,
        #[wasm_bindgen(unchecked_param_type = "TextRasterizer | null")] rasterizer: Option<
            js_sys::Function,
        >,
    ) {
        self.text = rasterizer;
        self.revision = self.revision.wrapping_add(1);
    }

    // --- Document -----------------------------------------------------------

    /// Replace the document with `frames`, forgetting the undo history. The
    /// frame clipboard survives, as it does on the desktop.
    pub fn open(&mut self, frames: &Frames) {
        self.history = gifkino_core::Editor::new(Document::from_frames(frames.frames.clone()));
        self.revision = self.revision.wrapping_add(1);
    }

    /// Bumped by every change to what `composite` would draw, undo and redo
    /// included: a cache key.
    #[wasm_bindgen(getter)]
    pub fn revision(&self) -> u32 {
        self.revision
    }

    #[wasm_bindgen(getter)]
    pub fn width(&self) -> u32 {
        self.doc().size().0
    }

    #[wasm_bindgen(getter)]
    pub fn height(&self) -> u32 {
        self.doc().size().1
    }

    #[wasm_bindgen(getter, js_name = frameCount)]
    pub fn frame_count_js(&self) -> usize {
        self.frame_count()
    }

    #[wasm_bindgen(getter, js_name = durationCs)]
    pub fn duration_cs(&self) -> u32 {
        self.doc().duration_cs()
    }

    #[wasm_bindgen(getter, js_name = hasAlpha)]
    pub fn has_alpha(&self) -> bool {
        self.doc().has_alpha()
    }

    #[wasm_bindgen(unchecked_return_type = "FrameInfo[]")]
    pub fn frames(&self) -> Result<JsValue, JsError> {
        let frames: Vec<FrameInfo> = self
            .doc()
            .frames
            .iter()
            .map(|f| FrameInfo {
                delay_cs: f.delay_cs,
                detached: f.detached,
                opaque: f.opaque,
                key: f.key,
            })
            .collect();
        to_js(&frames)
    }

    /// Every overlay, bottom to top: list order is z-order.
    #[wasm_bindgen(unchecked_return_type = "Overlay[]")]
    pub fn overlays(&self) -> Result<JsValue, JsError> {
        to_js(&self.doc().overlays)
    }

    #[wasm_bindgen(unchecked_return_type = "Overlay | undefined")]
    pub fn overlay(&self, id: u32) -> Result<JsValue, JsError> {
        to_js_or_undefined(self.doc().overlay(OverlayId(id)))
    }

    /// Frame `index` with its overlays drawn on it.
    pub fn composite(&self, index: usize) -> Result<Option<Image>, JsError> {
        let doc = self.doc();
        let image = self.with_text(|text| render::composite(doc, index, text))?;
        Ok(image.map(Image::from))
    }

    /// Frame `index`'s filmstrip thumbnail, fitted to a 72px box, without
    /// overlays.
    pub fn thumbnail(&self, index: usize) -> Option<Image> {
        let frame = self.doc().frames.get(index)?;
        Some(Image::from((*frame.thumb).clone()))
    }

    // --- History ------------------------------------------------------------

    #[wasm_bindgen(getter, js_name = canUndo)]
    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    #[wasm_bindgen(getter, js_name = canRedo)]
    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    #[wasm_bindgen(getter, js_name = undoLabel)]
    pub fn undo_label(&self) -> Option<String> {
        self.history.undo_label().map(String::from)
    }

    #[wasm_bindgen(getter, js_name = redoLabel)]
    pub fn redo_label(&self) -> Option<String> {
        self.history.redo_label().map(String::from)
    }

    pub fn undo(&mut self) -> bool {
        let undone = self.history.undo();
        if undone {
            self.revision = self.revision.wrapping_add(1);
        }
        undone
    }

    pub fn redo(&mut self) -> bool {
        let redone = self.history.redo();
        if redone {
            self.revision = self.revision.wrapping_add(1);
        }
        redone
    }

    // --- Import -------------------------------------------------------------

    /// Whether splicing `frames` in needs the fit question asked: the
    /// document has a canvas and the incoming frames are another size.
    #[wasm_bindgen(js_name = needsFit)]
    pub fn needs_fit(&self, frames: &Frames) -> bool {
        let canvas = self.doc().size();
        let arriving = frames.frames.first().map(|f| f.pixels.dimensions());
        arriving.is_some_and(|arriving| canvas != (0, 0) && canvas != arriving)
    }

    /// Splice `frames` in at `at` (clamped to the frame count), fitted onto
    /// one canvas by `fit`. A single incoming frame takes the delay of the
    /// frame before `at`, or the first frame's: a still carries no timing.
    #[wasm_bindgen(unchecked_return_type = "Change | undefined")]
    pub fn splice(
        &mut self,
        frames: &Frames,
        at: usize,
        #[wasm_bindgen(unchecked_param_type = "FitMode")] fit: JsValue,
    ) -> Result<JsValue, JsError> {
        let mode: FitMode = from_js(fit)?;
        let mut incoming = frames.frames.clone();
        if incoming.is_empty() {
            return Ok(JsValue::UNDEFINED);
        }
        let at = at.min(self.frame_count());
        if let [only] = incoming.as_mut_slice() {
            let doc = self.doc();
            let neighbour = at
                .checked_sub(1)
                .and_then(|i| doc.frames.get(i))
                .or_else(|| doc.frames.first());
            if let Some(neighbour) = neighbour {
                only.delay_cs = neighbour.delay_cs;
            }
        }
        let splice = fit::plan_splice(self.doc(), at, incoming, mode, |_, _| {});
        let label = if splice.incoming.len() == 1 {
            FRAME_ADDED
        } else {
            FRAMES_ADDED
        };
        let added = splice.incoming.len();
        let (change, _) = self.record(label, added, |d| splice.apply(d));
        to_js(&change)
    }

    // --- Frames -------------------------------------------------------------

    #[wasm_bindgen(js_name = deleteFrames, unchecked_return_type = "Change | undefined")]
    pub fn delete_frames(&mut self, frames: &[u32]) -> Result<JsValue, JsError> {
        let picked = self.scope(frames);
        if picked.is_empty() {
            return Ok(JsValue::UNDEFINED);
        }
        let (change, _) = self.record(FRAMES_DELETED, picked.len(), |d| {
            d.delete_frames_at(&picked)
        });
        to_js(&change)
    }

    /// Copy `frames` onto the clipboard, then delete them.
    #[wasm_bindgen(js_name = cutFrames, unchecked_return_type = "Change | undefined")]
    pub fn cut_frames(&mut self, frames: &[u32]) -> Result<JsValue, JsError> {
        let picked = self.scope(frames);
        if picked.is_empty() {
            return Ok(JsValue::UNDEFINED);
        }
        self.copy_frames(frames);
        let (change, _) = self.record(FRAMES_CUT, picked.len(), |d| d.delete_frames_at(&picked));
        to_js(&change)
    }

    /// Put `frames` on the clipboard in strip order; how many went. Not an
    /// edit, so no undo step.
    #[wasm_bindgen(js_name = copyFrames)]
    pub fn copy_frames(&mut self, frames: &[u32]) -> usize {
        let picked = self.scope(frames);
        if picked.is_empty() {
            return 0;
        }
        let doc = &self.history.doc;
        self.clipboard = picked.iter().map(|&i| doc.frames[i].clone()).collect();
        self.clipboard.len()
    }

    #[wasm_bindgen(getter, js_name = clipboardLength)]
    pub fn clipboard_length(&self) -> usize {
        self.clipboard.len()
    }

    /// Paste the clipboard in at `at`; the desktop pastes after the frame on
    /// screen, `playhead + 1`. The pasted run inherits the overlays covering
    /// the slot it lands in.
    #[wasm_bindgen(js_name = pasteFrames, unchecked_return_type = "Change | undefined")]
    pub fn paste_frames(&mut self, at: usize) -> Result<JsValue, JsError> {
        if self.clipboard.is_empty() {
            return Ok(JsValue::UNDEFINED);
        }
        let frames = self.clipboard.clone();
        let at = at.min(self.frame_count());
        let (change, _) = self.record(FRAMES_PASTED, frames.len(), |d| {
            d.insert_frames_at(at, frames)
        });
        to_js(&change)
    }

    /// Each frame's copy lands directly after it.
    #[wasm_bindgen(js_name = duplicateFrames, unchecked_return_type = "Change | undefined")]
    pub fn duplicate_frames(&mut self, frames: &[u32]) -> Result<JsValue, JsError> {
        let picked = self.scope(frames);
        if picked.is_empty() {
            return Ok(JsValue::UNDEFINED);
        }
        let (change, _) = self.record(FRAMES_DUPLICATED, picked.len(), |d| {
            d.duplicate_frames_at(&picked)
        });
        to_js(&change)
    }

    /// Reverse the run from the first to the last of `frames`.
    #[wasm_bindgen(js_name = reverseFrames, unchecked_return_type = "Change | undefined")]
    pub fn reverse_frames(&mut self, frames: &[u32]) -> Result<JsValue, JsError> {
        let picked = self.scope(frames);
        let (Some(&first), Some(&last)) = (picked.first(), picked.last()) else {
            return Ok(JsValue::UNDEFINED);
        };
        let (change, _) = self.record(FRAMES_REVERSED, picked.len(), |d| {
            d.reverse_frames(first..last + 1)
        });
        to_js(&change)
    }

    /// Move one frame from `from` to `to`; overlays covering it come along.
    #[wasm_bindgen(js_name = moveFrame, unchecked_return_type = "Change | undefined")]
    pub fn move_frame(&mut self, from: usize, to: usize) -> Result<JsValue, JsError> {
        let total = self.frame_count();
        if from >= total || to >= total || from == to {
            return Ok(JsValue::UNDEFINED);
        }
        let (change, _) = self.record(FRAME_MOVED, 1, |d| d.move_frame(from, to));
        to_js(&change)
    }

    /// Move `frames` as one run to the divider `gap` (0 is before the first
    /// frame, `frameCount` after the last). `at` is where the run starts now.
    #[wasm_bindgen(js_name = moveFrames, unchecked_return_type = "FramesMoved | undefined")]
    pub fn move_frames(&mut self, frames: &[u32], gap: usize) -> Result<JsValue, JsError> {
        let picked = self.scope(frames);
        let to = move_target_for_set(&picked, gap.min(self.frame_count()));
        self.frames_moved(picked, to)
    }

    /// Swap `frames` as a block with the unselected frame before (`earlier`)
    /// or after them.
    #[wasm_bindgen(js_name = nudgeFrames, unchecked_return_type = "FramesMoved | undefined")]
    pub fn nudge_frames(&mut self, frames: &[u32], earlier: bool) -> Result<JsValue, JsError> {
        let picked = self.scope(frames);
        let Some(gap) = selection_nudge_gap(&picked, earlier, self.frame_count()) else {
            return Ok(JsValue::UNDEFINED);
        };
        let to = move_target_for_set(&picked, gap);
        self.frames_moved(picked, to)
    }

    /// Set each of `frames` to `delayCs` centiseconds, at least 1.
    #[wasm_bindgen(js_name = setDelay, unchecked_return_type = "Change | undefined")]
    pub fn set_delay(&mut self, frames: &[u32], delay_cs: u16) -> Result<JsValue, JsError> {
        let picked = self.scope(frames);
        if picked.is_empty() {
            return Ok(JsValue::UNDEFINED);
        }
        let (change, _) = self.record(DELAY_SET, picked.len(), |d| {
            d.set_delay_at(&picked, delay_cs.max(1))
        });
        to_js(&change)
    }

    /// Delete every `n`th frame (n ≥ 2), keeping the running time.
    #[wasm_bindgen(js_name = dropEveryNth, unchecked_return_type = "Change | undefined")]
    pub fn drop_every_nth(&mut self, n: usize) -> Result<JsValue, JsError> {
        let total = self.frame_count();
        if total == 0 || n < 2 {
            return Ok(JsValue::UNDEFINED);
        }
        let (change, _) = self.record(FRAMES_REMOVED, total / n, |d| d.drop_every_nth(n));
        to_js(&change)
    }

    /// Drop the `percent` of frames (at most 95) whose loss shows least,
    /// keeping the running time.
    #[wasm_bindgen(js_name = smartDrop, unchecked_return_type = "Change | undefined")]
    pub fn smart_drop(&mut self, percent: usize) -> Result<JsValue, JsError> {
        let count = self.frame_count() * percent.min(95) / 100;
        if count == 0 {
            return Ok(JsValue::UNDEFINED);
        }
        let (change, _) = self.record(FRAMES_REMOVED, count, |d| d.drop_low_motion(count));
        to_js(&change)
    }

    // --- Effects and canvas -------------------------------------------------

    #[wasm_bindgen(unchecked_return_type = "Change | undefined")]
    pub fn mirror(
        &mut self,
        frames: &[u32],
        #[wasm_bindgen(unchecked_param_type = "Axis")] axis: JsValue,
    ) -> Result<JsValue, JsError> {
        let axis: Axis = from_js(axis)?;
        let picked = self.scope(frames);
        if picked.is_empty() {
            return Ok(JsValue::UNDEFINED);
        }
        let produced = self.doc().mirrored_frames(&picked, axis, |_, _| {});
        to_js_or_undefined(self.land(FRAMES_MIRRORED, produced, (1.0, 1.0), (0.0, 0.0)))
    }

    /// Fade `frames` (at least two) in from or out to `colour`.
    #[wasm_bindgen(unchecked_return_type = "Change | undefined")]
    pub fn fade(
        &mut self,
        frames: &[u32],
        #[wasm_bindgen(unchecked_param_type = "Fade")] fade: JsValue,
        #[wasm_bindgen(unchecked_param_type = "Rgba8")] colour: JsValue,
    ) -> Result<JsValue, JsError> {
        let fade: Fade = from_js(fade)?;
        let colour: [u8; 4] = from_js(colour)?;
        let picked = self.scope(frames);
        if picked.len() < 2 {
            return Ok(JsValue::UNDEFINED);
        }
        let produced = self.doc().faded_frames(&picked, fade, colour, |_, _| {});
        let label = match fade {
            Fade::In => FADED_IN,
            Fade::Out => FADED_OUT,
        };
        to_js_or_undefined(self.land(label, produced, (1.0, 1.0), (0.0, 0.0)))
    }

    /// The most frames `loopFade` can blend: half the document.
    #[wasm_bindgen(getter, js_name = loopFadeLimit)]
    pub fn loop_fade_limit(&self) -> usize {
        self.doc().loop_fade_limit()
    }

    /// Dissolve the last `count` frames into the first `count`, then drop
    /// those first frames, so the end plays into the start without a cut.
    #[wasm_bindgen(js_name = loopFade, unchecked_return_type = "Change | undefined")]
    pub fn loop_fade(&mut self, count: usize) -> Result<JsValue, JsError> {
        if count.min(self.doc().loop_fade_limit()) == 0 {
            return Ok(JsValue::UNDEFINED);
        }
        let tail = self.doc().loop_faded_frames(count, |_, _| {});
        let (change, _) = self.record(LOOP_CROSS_FADED, tail.len(), |d| d.apply_loop_fade(tail));
        to_js(&change)
    }

    /// Crop every frame to the box `(x, y, w, h)` in image pixels, widened to
    /// whole pixels and clipped to the canvas. Overlays keep their place on
    /// the picture. A box covering the whole canvas is no edit.
    #[wasm_bindgen(unchecked_return_type = "Change | undefined")]
    pub fn crop(&mut self, x: f32, y: f32, w: f32, h: f32) -> Result<JsValue, JsError> {
        let size = self.doc().size();
        let Some(rect) = normalize_canvas_rect(size, (x, y, w, h)) else {
            return Ok(JsValue::UNDEFINED);
        };
        if !crop_changes_canvas(size, rect) {
            return Ok(JsValue::UNDEFINED);
        }
        let (x, y, w, h) = rect;
        let produced = self.doc().cropped_frames(x, y, w, h, |_, _| {});
        let offset = (-(x as f32), -(y as f32));
        to_js_or_undefined(self.land(CROPPED, produced, (1.0, 1.0), offset))
    }

    /// Keep the box on `frames` and make the rest of each transparent, the
    /// canvas size unchanged.
    #[wasm_bindgen(unchecked_return_type = "Change | undefined")]
    pub fn shrink(
        &mut self,
        frames: &[u32],
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    ) -> Result<JsValue, JsError> {
        let picked = self.scope(frames);
        let Some((x, y, w, h)) = normalize_canvas_rect(self.doc().size(), (x, y, w, h)) else {
            return Ok(JsValue::UNDEFINED);
        };
        if picked.is_empty() {
            return Ok(JsValue::UNDEFINED);
        }
        let produced = self.doc().shrunk_frames(&picked, x, y, w, h, |_, _| {});
        to_js_or_undefined(self.land(CROPPED, produced, (1.0, 1.0), (0.0, 0.0)))
    }

    /// Blow the box up to fill the canvas on `frames`.
    #[wasm_bindgen(unchecked_return_type = "Change | undefined")]
    pub fn zoom(
        &mut self,
        frames: &[u32],
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    ) -> Result<JsValue, JsError> {
        let picked = self.scope(frames);
        let Some((x, y, w, h)) = normalize_canvas_rect(self.doc().size(), (x, y, w, h)) else {
            return Ok(JsValue::UNDEFINED);
        };
        if picked.is_empty() {
            return Ok(JsValue::UNDEFINED);
        }
        let produced = self.doc().zoomed_frames(&picked, x, y, w, h, |_, _| {});
        to_js_or_undefined(self.land(ZOOMED, produced, (1.0, 1.0), (0.0, 0.0)))
    }

    /// Resample every frame to `width × height`; overlays scale with them.
    #[wasm_bindgen(unchecked_return_type = "Change | undefined")]
    pub fn resize(&mut self, width: u32, height: u32) -> Result<JsValue, JsError> {
        let (w, h) = (width.max(1), height.max(1));
        let (cw, ch) = self.doc().size();
        if self.frame_count() == 0 || (w, h) == (cw, ch) {
            return Ok(JsValue::UNDEFINED);
        }
        let produced = self.doc().resized_frames(w, h, |_, _| {});
        let scale = (w as f32 / cw.max(1) as f32, h as f32 / ch.max(1) as f32);
        to_js_or_undefined(self.land(RESIZED, produced, scale, (0.0, 0.0)))
    }

    // --- Overlays -----------------------------------------------------------

    /// Add a fresh overlay of `tool`'s kind over `frames`, one overlay per
    /// contiguous run. `overlay` is the piece on `playhead`, else the first:
    /// the one to select.
    #[wasm_bindgen(js_name = addOverlay, unchecked_return_type = "OverlaysAdded | undefined")]
    pub fn add_overlay(
        &mut self,
        #[wasm_bindgen(unchecked_param_type = "Tool")] tool: JsValue,
        frames: &[u32],
        playhead: usize,
    ) -> Result<JsValue, JsError> {
        let tool: Tool = from_js(tool)?;
        let picked = self.scope(frames);
        if picked.is_empty() {
            return Ok(JsValue::UNDEFINED);
        }
        let (kind, transform) = fresh_overlay(tool.shape(), self.doc().size());
        let (change, overlays) = self.record(tool.added_label(), picked.len(), |d| {
            d.add_overlay_over(tool.name(), kind, transform, &picked)
        });
        let doc = self.doc();
        let on_playhead = overlays
            .iter()
            .copied()
            .find(|&id| doc.overlay(id).is_some_and(|o| o.range.contains(&playhead)));
        let overlay = on_playhead.unwrap_or(overlays[0]);
        to_js(&OverlaysAdded {
            change,
            overlays,
            overlay,
        })
    }

    #[wasm_bindgen(js_name = deleteOverlay, unchecked_return_type = "Change | undefined")]
    pub fn delete_overlay(&mut self, id: u32) -> Result<JsValue, JsError> {
        let id = OverlayId(id);
        let Some(overlay) = self.doc().overlay(id) else {
            return Ok(JsValue::UNDEFINED);
        };
        let touched = overlay.range.len();
        let (change, _) = self.record(OVERLAY_DELETED, touched, |d| d.remove_overlay(id));
        to_js(&change)
    }

    /// Move overlay `id` one step up (towards the top, painted last) or down
    /// the layer list of frame `playhead`, past the overlay shown next to it
    /// there.
    #[wasm_bindgen(js_name = restackOverlay, unchecked_return_type = "Change | undefined")]
    pub fn restack_overlay(
        &mut self,
        id: u32,
        up: bool,
        playhead: usize,
    ) -> Result<JsValue, JsError> {
        let id = OverlayId(id);
        let on_frame: Vec<OverlayId> = self
            .doc()
            .overlays
            .iter()
            .filter(|o| o.range.contains(&playhead))
            .map(|o| o.id)
            .collect();
        let Some(at) = on_frame.iter().position(|o| *o == id) else {
            return Ok(JsValue::UNDEFINED);
        };
        let neighbour = if up {
            on_frame.get(at + 1)
        } else {
            at.checked_sub(1).and_then(|below| on_frame.get(below))
        };
        let Some(&other) = neighbour else {
            return Ok(JsValue::UNDEFINED);
        };
        let touched = self.doc().overlay(id).map_or(0, |o| o.range.len());
        let (change, _) = self.record(OVERLAY_REORDERED, touched, |d| {
            d.restack_overlay(id, other, up)
        });
        to_js(&change)
    }

    /// Retype text overlay `id` on the frames where `frames` reaches into it
    /// (the frame on screen if it does not), splitting it if that is not all
    /// of it. `overlay` is the piece that now carries the edit.
    #[wasm_bindgen(js_name = editText, unchecked_return_type = "OverlayEdit | undefined")]
    pub fn edit_text(
        &mut self,
        id: u32,
        text: String,
        frames: &[u32],
        playhead: usize,
    ) -> Result<JsValue, JsError> {
        let id = OverlayId(id);
        let span = self.edit_span(id, frames, playhead);
        self.overlay_edit(id, span, TEXT_EDITED, |o| {
            if let OverlayKind::Text(t) = &mut o.kind {
                t.text = text;
            }
        })
    }

    /// Restyle overlay `id` to `kind`, scoped like `editText`. A `kind` equal
    /// to the overlay's, or of the other type, changes nothing. A `playhead`
    /// off the overlay counts as its first frame, where the desktop seeks.
    #[wasm_bindgen(js_name = restyleOverlay, unchecked_return_type = "OverlayEdit | undefined")]
    pub fn restyle_overlay(
        &mut self,
        id: u32,
        #[wasm_bindgen(unchecked_param_type = "OverlayKind")] kind: JsValue,
        frames: &[u32],
        playhead: usize,
    ) -> Result<JsValue, JsError> {
        let id = OverlayId(id);
        let kind: OverlayKind = from_js(kind)?;
        let Some(overlay) = self.doc().overlay(id) else {
            return Ok(JsValue::UNDEFINED);
        };
        let same_type = std::mem::discriminant(&overlay.kind) == std::mem::discriminant(&kind);
        if !same_type || overlay.kind == kind {
            return Ok(JsValue::UNDEFINED);
        }
        let playhead = if overlay.range.contains(&playhead) {
            playhead
        } else {
            overlay
                .range
                .start
                .min(self.frame_count().saturating_sub(1))
        };
        let span = self.edit_span(id, frames, playhead);
        self.overlay_edit(id, span, OVERLAY_RESTYLED, |o| o.kind = kind)
    }

    /// Put overlay `id` on those of `frames` it does not cover yet, one piece
    /// per run.
    #[wasm_bindgen(js_name = copyOverlayTo, unchecked_return_type = "Change | undefined")]
    pub fn copy_overlay_to(&mut self, id: u32, frames: &[u32]) -> Result<JsValue, JsError> {
        let id = OverlayId(id);
        let picked = self.scope(frames);
        let Some(overlay) = self.doc().overlay(id) else {
            return Ok(JsValue::UNDEFINED);
        };
        let landing = picked.iter().filter(|f| !overlay.range.contains(f)).count();
        if landing == 0 {
            return Ok(JsValue::UNDEFINED);
        }
        let (change, _) = self.record(OVERLAY_COPIED, landing, |d| d.copy_overlay_to(id, &picked));
        to_js(&change)
    }

    /// The topmost visible overlay on frame `frame` under image point
    /// `(x, y)`.
    #[wasm_bindgen(js_name = overlayAt)]
    pub fn overlay_at(&self, frame: usize, x: f32, y: f32) -> Option<u32> {
        self.doc()
            .overlays_on(frame)
            .filter(|o| handles::contains(o.transform, x, y))
            .last()
            .map(|o| o.id.0)
    }

    /// How many frames a scoped edit of overlay `id` (`editText`,
    /// `restyleOverlay`, `commitOverlayDrag`) would land on: the desktop's
    /// "Applies to {count} selected frames" under the overlay editor.
    #[wasm_bindgen(js_name = overlayEditCount)]
    pub fn overlay_edit_count(&self, id: u32, frames: &[u32], playhead: usize) -> usize {
        self.edit_span(OverlayId(id), frames, playhead).len()
    }

    /// Where a canvas drag has taken `origin`: see `handles::drag`. `from`
    /// and `to` are the press and the pointer in image pixels; `snapReach`
    /// is the snap distance in image pixels, `null` with snapping off.
    #[wasm_bindgen(js_name = dragOverlay, unchecked_return_type = "Dragged")]
    #[allow(clippy::too_many_arguments)]
    pub fn drag_overlay(
        &self,
        #[wasm_bindgen(unchecked_param_type = "Grip")] grip: JsValue,
        #[wasm_bindgen(unchecked_param_type = "Transform")] origin: JsValue,
        from_x: f32,
        from_y: f32,
        to_x: f32,
        to_y: f32,
        snap_reach: Option<f32>,
        keep_aspect: bool,
        from_center: bool,
    ) -> Result<JsValue, JsError> {
        let grip: GripArg = from_js(grip)?;
        let origin: Transform = from_js(origin)?;
        let (w, h) = self.doc().size();
        let keys = DragKeys {
            keep_aspect,
            from_center,
        };
        let (transform, guides) = handles::drag(
            grip.into(),
            origin,
            (from_x, from_y),
            (to_x, to_y),
            (w as f32, h as f32),
            snap_reach,
            keys,
        );
        to_js(&Dragged { transform, guides })
    }

    /// Show overlay `id` at `transform` while a drag is in flight, outside
    /// the history: `commitOverlayDrag` records the drag as one step.
    #[wasm_bindgen(js_name = setOverlayTransformLive)]
    pub fn set_overlay_transform_live(
        &mut self,
        id: u32,
        #[wasm_bindgen(unchecked_param_type = "Transform")] transform: JsValue,
    ) -> Result<(), JsError> {
        let transform: Transform = from_js(transform)?;
        if let Some(o) = self.history.doc.overlay_mut(OverlayId(id)) {
            o.transform = transform;
            self.revision = self.revision.wrapping_add(1);
        }
        Ok(())
    }

    /// Button up: put overlay `id` back at `origin`, where the drag started,
    /// and record its move to `transform` as one undo step, scoped like
    /// `editText`. `grip` picks the label.
    #[wasm_bindgen(js_name = commitOverlayDrag, unchecked_return_type = "OverlayEdit | undefined")]
    pub fn commit_overlay_drag(
        &mut self,
        id: u32,
        #[wasm_bindgen(unchecked_param_type = "Grip")] grip: JsValue,
        #[wasm_bindgen(unchecked_param_type = "Transform")] origin: JsValue,
        #[wasm_bindgen(unchecked_param_type = "Transform")] transform: JsValue,
        frames: &[u32],
        playhead: usize,
    ) -> Result<JsValue, JsError> {
        let grip: GripArg = from_js(grip)?;
        let origin: Transform = from_js(origin)?;
        let transform: Transform = from_js(transform)?;
        let id = OverlayId(id);
        let Some(o) = self.history.doc.overlay_mut(id) else {
            return Ok(JsValue::UNDEFINED);
        };
        // Back to where the drag started before recording, so undo returns
        // there rather than to one motion event before the end.
        o.transform = origin;
        self.revision = self.revision.wrapping_add(1);
        let label = match grip {
            GripArg::Move => OVERLAY_MOVED,
            GripArg::Rotate => OVERLAY_ROTATED,
            GripArg::Resize(_) => OVERLAY_RESIZED,
        };
        let span = self.edit_span(id, frames, playhead);
        self.overlay_edit(id, span, label, |o| o.transform = transform)
    }

    // --- Export -------------------------------------------------------------

    /// The document as a GIF file. `settings` defaults to the desktop's
    /// export (full size, source speed, 256 colours, no dither, loop
    /// forever); `lossy` is ignored, there being no gifsicle here.
    /// `progress(done, total)` runs after each written frame.
    #[wasm_bindgen(js_name = exportGif)]
    pub fn export_gif(
        &self,
        #[wasm_bindgen(unchecked_param_type = "ExportSettings | null")] settings: JsValue,
        #[wasm_bindgen(unchecked_param_type = "((done: number, total: number) => void) | null")]
        progress: Option<js_sys::Function>,
    ) -> Result<Vec<u8>, JsError> {
        let settings: ExportSettings = if settings.is_null() || settings.is_undefined() {
            ExportSettings::default()
        } else {
            from_js(settings)?
        };
        let doc = self.doc();
        let enc = self.with_text(|text| Encodable::from_document(doc, text, &settings))?;
        let mut report = |done: usize, total: usize| {
            if let Some(callback) = &progress {
                let _ = callback.call2(&JsValue::NULL, &done.into(), &total.into());
            }
        };
        let mut bytes = Vec::new();
        gif::encode(&mut bytes, &enc, &settings, &mut report)
            .map_err(|e| error(format!("{e:#}")))?;
        Ok(bytes)
    }

    /// The export's size in bytes, measured by encoding a spread of frames
    /// with the default settings and extrapolating — the desktop's readout.
    #[wasm_bindgen(js_name = estimateSize)]
    pub fn estimate_size(&self) -> Result<usize, JsError> {
        let settings = ExportSettings::default();
        let doc = self.doc();
        let sample = self
            .with_text(|text| Encodable::sample_document(doc, text, &settings, ESTIMATE_SAMPLES))?;
        gif::estimate_size(&sample, self.frame_count(), &settings)
            .map_err(|e| error(format!("{e:#}")))
    }
}

/// The corner grip of `transform` within `grab` image pixels of `(x, y)`:
/// 0 TL, 1 TR, 2 BL, 3 BR.
#[wasm_bindgen(js_name = handleAt)]
pub fn handle_at(
    #[wasm_bindgen(unchecked_param_type = "Transform")] transform: JsValue,
    x: f32,
    y: f32,
    grab: f32,
) -> Result<Option<usize>, JsError> {
    let transform: Transform = from_js(transform)?;
    Ok(handles::handle_at(transform, x, y, grab))
}

/// Whether `(x, y)` lies inside `transform`'s box, rotation honoured.
#[wasm_bindgen]
pub fn contains(
    #[wasm_bindgen(unchecked_param_type = "Transform")] transform: JsValue,
    x: f32,
    y: f32,
) -> Result<bool, JsError> {
    let transform: Transform = from_js(transform)?;
    Ok(handles::contains(transform, x, y))
}

/// Where `transform`'s four grips sit on the image, rotation included: TL,
/// TR, BL, BR.
#[wasm_bindgen(js_name = orientedCorners, unchecked_return_type = "[number, number][]")]
pub fn oriented_corners(
    #[wasm_bindgen(unchecked_param_type = "Transform")] transform: JsValue,
) -> Result<JsValue, JsError> {
    let transform: Transform = from_js(transform)?;
    to_js(&handles::oriented_corners(transform))
}

/// The canvas a splice under `fit` ends up with, for the fit question.
#[wasm_bindgen(js_name = fitCanvas, unchecked_return_type = "[number, number]")]
pub fn fit_canvas(
    #[wasm_bindgen(unchecked_param_type = "FitMode")] fit: JsValue,
    canvas_width: u32,
    canvas_height: u32,
    incoming_width: u32,
    incoming_height: u32,
) -> Result<JsValue, JsError> {
    let mode: FitMode = from_js(fit)?;
    to_js(&mode.canvas(
        (canvas_width, canvas_height),
        (incoming_width, incoming_height),
    ))
}

/// Per-frame delays for `count` frames at `fps`, spreading the centisecond
/// remainder so the total keeps time: what a decoded video's frames get.
#[wasm_bindgen(js_name = delaysForFps)]
pub fn delays_for_fps_js(fps: f64, count: usize) -> Vec<u16> {
    delays_for_fps(fps, count)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The overlay editor's "Applies to {count} selected frames" has to name
    /// the frames the edit really lands on: a scope reaching past the overlay
    /// is clipped to it, one that touches the overlay but not the frame on
    /// screen falls back to that frame alone, and one missing it is nothing.
    #[test]
    fn edit_count_is_where_the_scope_meets_the_overlay() {
        let mut doc = Document::default();
        doc.frames = (0..6)
            .map(|_| Frame::new(RgbaImage::new(4, 4), 10))
            .collect();
        let (kind, transform) = fresh_overlay(Some(Shape::Rect), (4, 4));
        let id = doc.add_overlay_over("Rectangle", kind, transform, &[1, 2, 3])[0];
        let editor = Editor {
            history: gifkino_core::Editor::new(doc),
            ..Editor::default()
        };
        assert_eq!(editor.overlay_edit_count(id.0, &[0, 1, 2, 3, 4, 5], 2), 3);
        assert_eq!(editor.overlay_edit_count(id.0, &[2], 2), 1);
        assert_eq!(editor.overlay_edit_count(id.0, &[1, 2], 3), 1, "the frame on screen");
        assert_eq!(editor.overlay_edit_count(id.0, &[5], 2), 0, "nowhere to land");
    }
}
