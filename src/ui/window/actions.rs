//! App methods behind the messages, plus the frame-work runners they spawn.

use super::*;

impl App {
    /// Debounced so a run of keystrokes measures once, not once per character.
    pub(super) fn schedule_estimate(&mut self, sender: &ComponentSender<Self>) {
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
    pub(super) fn move_picked(&mut self, picked: &[usize], to: usize) -> Option<Change> {
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
    pub(super) fn run_frame_op(
        &mut self,
        op: FrameOp,
        frames: Vec<usize>,
        sender: &ComponentSender<Self>,
    ) {
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
            FrameOp::Duplicate => d.duplicate_frames_at(&frames),
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
    pub(super) fn copy_frames(&mut self, frames: &[usize]) {
        self.clipboard = frames
            .iter()
            .filter_map(|&i| self.editor.doc.frames.get(i).cloned())
            .collect();
    }

    /// Paste the clipboard in directly after the frame on screen, the way a
    /// duplicate lands beside its source, and leave the pasted run selected
    /// so a second edit acts on what just arrived. `None` when there is
    /// nothing on the clipboard, so the caller can skip the toast.
    pub(super) fn paste_frames(&mut self) -> Option<Change> {
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
    pub(super) fn restack_neighbour(&self, id: OverlayId, up: bool) -> Option<OverlayId> {
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
    pub(super) fn restack_overlay(&mut self, id: OverlayId, up: bool) {
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
    pub(super) fn delete_overlay(&mut self, id: OverlayId, sender: &ComponentSender<Self>) {
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

    /// Put one overlay on frames it does not cover yet: the band menu's copy,
    /// to every frame (`all`) or to the scope, so a Ctrl+clicked handful gets
    /// a piece each and nothing between them. A copy that has nowhere to land
    /// says so rather than spending an undo step on a document it did not
    /// change.
    pub(super) fn copy_overlay_to_frames(
        &mut self,
        id: OverlayId,
        all: bool,
        sender: &ComponentSender<Self>,
    ) {
        let frames = self.frames_or_scope(all);
        let Some(overlay) = self.editor.doc.overlay(id) else {
            return;
        };
        let covered = overlay.range.clone();
        let landing = frames.iter().filter(|f| !covered.contains(f)).count();
        if landing == 0 {
            sender.input(Msg::Notice(
                t("The overlay is on those frames already").into(),
            ));
            return;
        }
        let (change, _) = self.editor.edit(n("Overlay copied"), landing, |d| {
            d.copy_overlay_to(id, &frames);
        });
        // The source keeps the lower id through any coalescing, so the
        // sidebar stays on the layer that was copied.
        self.selected_overlay = Some(id);
        self.after_edit();
        self.toast_if_document_wide(sender, &change, self.frame_count());
    }

    /// The crop box, once it is big enough to act on. A bare click leaves a
    /// zero-size rect behind, which is not a box.
    pub(super) fn usable_crop(&self) -> Option<(f32, f32, f32, f32)> {
        self.crop_rect.filter(|(_, _, w, h)| *w >= 2.0 && *h >= 2.0)
    }

    /// Whether a canvas press at `(x, y)` is a pick rather than a crop box: it
    /// lands on an overlay before any box is drawn. With a box up, the press
    /// redraws the box instead, so a box can start on top of text.
    pub(super) fn crop_yields_to(&self, x: f32, y: f32) -> bool {
        self.crop_tool
            && self.usable_crop().is_none()
            && self
                .editor
                .doc
                .overlays_on(self.playhead)
                .any(|o| contains(o.transform, x, y))
    }

    /// Select an overlay from the timeline or layer list, or clear the pick.
    pub(super) fn pick_overlay(&mut self, id: Option<OverlayId>) {
        self.selected_overlay = id;
        if let Some(id) = id {
            self.leave_crop();
            self.seek_to_overlay(id);
        }
    }

    /// Turn the crop tool off and forget its box. Picking an overlay does
    /// this too: an armed tool hides the selection's outline.
    pub(super) fn leave_crop(&mut self) {
        self.crop_tool = false;
        self.crop_rect = None;
    }

    /// Button down on the canvas: start a crop box, a handle drag on the
    /// selection, or a move of the topmost overlay under the pointer.
    /// `scale` is widget pixels per image pixel.
    pub(super) fn canvas_press(&mut self, x: f32, y: f32, scale: f32, state: gdk::ModifierType) {
        if self.crop_yields_to(x, y) {
            self.leave_crop();
        }
        if self.crop_tool {
            self.crop_rect = Some((x, y, 0.0, 0.0));
            let origin = Transform::at(x, y, 0.0, 0.0);
            self.drag = Some(Drag::start(DragMode::CropRect, (x, y), origin, 0.0));
            return;
        }
        // A handle of the current selection wins over what is under the
        // pointer, or a small overlay could never be resized. The
        // rotate modifier outranks both, as it does in Impasto.
        let grab = (HANDLE_PX / scale.max(0.01) as f64) as f32;
        let snap_reach = (SNAP_PX / scale.max(0.01) as f64) as f32;
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
                self.drag = Some(Drag::start(DragMode::Rotate, (x, y), transform, snap_reach));
                return;
            }
            if let Some(corner) = handle_at(transform, x, y, grab) {
                let mode = DragMode::Resize(corner);
                self.drag = Some(Drag::start(mode, (x, y), transform, snap_reach));
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
                let mode = if rotating {
                    DragMode::Rotate
                } else {
                    DragMode::Move
                };
                self.drag = Some(Drag::start(mode, (x, y), transform, snap_reach));
            }
            None => self.selected_overlay = None,
        }
    }

    /// Pointer motion with the button down: the drag in flight follows it,
    /// live and outside the history until `canvas_release`.
    pub(super) fn canvas_drag(&mut self, x: f32, y: f32, state: gdk::ModifierType) {
        let keys = self.keymap.borrow();
        let (keep_aspect, from_center) = (
            keys.mods(Modal::KeepAspect).held(state),
            keys.mods(Modal::FromCenter).held(state),
        );
        drop(keys);
        let (image_w, image_h) = self.editor.doc.size();
        let image = (image_w as f32, image_h as f32);
        let snap_on = self.snap;
        let Some(drag) = &mut self.drag else { return };
        let (dx, dy) = (x - drag.from.0, y - drag.from.1);
        if dx.abs() < 0.5 && dy.abs() < 0.5 && !drag.moved {
            return;
        }
        drag.moved = true;
        let mut guides = Guides::default();
        match drag.mode {
            DragMode::CropRect => {
                let (x0, y0) = drag.from;
                self.crop_rect = Some((x0.min(x), y0.min(y), (x - x0).abs(), (y - y0).abs()));
                return;
            }
            DragMode::Move => {
                let moved = Transform {
                    x: drag.origin.x + dx,
                    y: drag.origin.y + dy,
                    ..drag.origin
                };
                (drag.current, guides) = if snap_on {
                    snap::snap_box(moved, image, drag.snap_reach)
                } else {
                    (moved, guides)
                };
            }
            DragMode::Resize(corner) => {
                // Corner drags are simple only in the box's own frame,
                // so both ends of the drag go through it first.
                let (fx, fy) = drag.origin.to_local(drag.from.0, drag.from.1);
                let (tx, ty) = drag.origin.to_local(x, y);
                let (mut dx, mut dy) = (tx - fx, ty - fy);
                // The dragged corner lands wherever the pointer takes
                // it, from the centre or not, so that is the point to
                // pull. ponytail: only an upright box without Shift
                // snaps; a rotated corner or an aspect-locked one
                // would need the guide solved back through the resize.
                if snap_on && drag.origin.angle == 0.0 && !keep_aspect {
                    let (cx, cy) = corners(drag.origin)[corner];
                    let ((sx, sy), pulled) =
                        snap::snap_point((cx + dx, cy + dy), image, drag.snap_reach);
                    (dx, dy, guides) = (sx - cx, sy - cy, pulled);
                }
                let resized = resize_corner(drag.origin, corner, dx, dy, keep_aspect, from_center);
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
        self.guides = guides;
        self.rev += 1;
    }

    /// Button up: commit the drag in flight as one undo step on the frames
    /// in scope. Returns the change, for the caller's toast.
    pub(super) fn canvas_release(&mut self) -> Option<Change> {
        self.guides = Guides::default();
        let drag = self.drag.take()?;
        if !drag.moved || drag.mode == DragMode::CropRect {
            return None;
        }
        let id = self.selected_overlay?;
        let (origin, final_t) = (drag.origin, drag.current);
        // Put the transform back before recording, so undo returns to
        // where the drag started rather than to one frame before it.
        if let Some(o) = self.editor.doc.overlay_mut(id) {
            o.transform = origin;
        }
        // The scope decides which frames the drag commits to; a scope
        // narrower than the overlay's range splits it, so the rest of
        // the frames keep where it was.
        let span = self.edit_span(id);
        if span.is_empty() {
            return None;
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
        Some(change)
    }

    /// Everything a document change invalidates.
    pub(super) fn after_edit(&mut self) {
        self.rev += 1;
        self.playhead = self.clamp_frame(self.playhead);
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
    pub(super) fn toast_if_document_wide(
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
    pub(super) fn start_crop(
        &mut self,
        rect: (u32, u32, u32, u32),
        sender: &ComponentSender<Self>,
    ) {
        if !crop_changes_canvas(self.editor.doc.size(), rect) {
            return;
        }
        self.crop_tool = false;
        self.start_frame_work(FrameWork::Crop { rect }, sender);
    }

    /// Probe and decode off the main thread. `import_at` has already been set
    /// to say where the frames land.
    pub(super) fn start_import(&mut self, path: PathBuf, sender: &ComponentSender<Self>) {
        let cancel = Arc::new(AtomicBool::new(false));
        self.busy = Some(Busy::new(BusyKind::Import, None, Some(cancel.clone())));
        // A new import starts from the configured budget. The raise the last
        // dialog asked for belonged to the file it was asked about, and it has
        // to outlive the decode itself — the fit chooser's own landing check
        // runs after it — so this is where it expires rather than on the way
        // out of `Cmd::Imported`.
        self.import_budget = None;
        plan_import(path, self.import_options(), cancel, sender);
    }

    /// Frames decoded and headed for the timeline at `at`. A file that
    /// already matches the canvas needs no decision, and neither does the
    /// first file into an empty document; anything else is the user's call.
    pub(super) fn ask_fit(
        &self,
        root: &adw::ApplicationWindow,
        path: &Path,
        at: usize,
        mut frames: Vec<Frame>,
        sender: &ComponentSender<Self>,
    ) {
        let Some(arriving) = frames.first().map(|f| f.pixels.dimensions()) else {
            return;
        };
        // A still image carries no timing of its own, and 10cs next to frames
        // that hold for a second reads as a dropout rather than a frame.
        if let [only] = frames.as_mut_slice() {
            let neighbour = at
                .checked_sub(1)
                .and_then(|i| self.editor.doc.frames.get(i))
                .or_else(|| self.editor.doc.frames.first());
            if let Some(neighbour) = neighbour {
                only.delay_cs = neighbour.delay_cs;
            }
        }
        let canvas = self.editor.doc.size();
        if canvas == (0, 0) || canvas == arriving {
            sender.input(Msg::SpliceFrames {
                at,
                frames,
                fit: FitMode::Stretch,
            });
            return;
        }
        fit_dialog(root, path, canvas, arriving, at, frames, sender);
    }

    /// The splice's resampling, threaded like a resize: the growing modes
    /// redraw every frame the document already had, which at a few hundred
    /// frames is seconds of work the window must stay alive through.
    pub(super) fn start_splice(
        &mut self,
        at: usize,
        frames: Vec<Frame>,
        fit: FitMode,
        sender: &ComponentSender<Self>,
    ) {
        let Some(arriving) = frames.first().map(|f| f.pixels.dimensions()) else {
            return;
        };
        let (w, h) = fit.canvas(self.editor.doc.size(), arriving);
        let landing = self.frame_count() + frames.len();
        if !resize_fits_budget(w, h, landing, self.import_budget()) {
            toast_over_memory_limit(sender);
            return;
        }
        self.busy = Some(Busy::new(BusyKind::Fit, None, None));
        run_splice(&self.editor.doc, at, frames, fit, sender);
    }

    /// Fill the canvas from the drawn crop box on `frames`, threaded like a
    /// resize. No-op if no box is drawn, it degenerates, or `frames` is empty.
    pub(super) fn start_zoom(&mut self, frames: Vec<usize>, sender: &ComponentSender<Self>) {
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
        self.start_frame_work(FrameWork::Zoom { frames, rect }, sender);
    }

    /// Claim the progress bar for `work` and hand it to the worker.
    pub(super) fn start_frame_work(&mut self, work: FrameWork, sender: &ComponentSender<Self>) {
        let total = work.touched(&self.editor.doc);
        self.busy = Some(Busy::new(work.busy_kind(), Some(total), None));
        run_frame_work(&self.editor.doc, work, sender);
    }
}

pub(super) fn normalize_canvas_rect(
    (cw, ch): (u32, u32),
    (x, y, w, h): (f32, f32, f32, f32),
) -> Option<(u32, u32, u32, u32)> {
    let left = x.floor().clamp(0.0, cw as f32) as u32;
    let top = y.floor().clamp(0.0, ch as f32) as u32;
    let right = (x + w).ceil().clamp(0.0, cw as f32) as u32;
    let bottom = (y + h).ceil().clamp(0.0, ch as f32) as u32;
    (right > left && bottom > top).then_some((left, top, right - left, bottom - top))
}

pub(super) fn resize_fits_budget(w: u32, h: u32, frames: usize, limit: usize) -> bool {
    resize_bytes(w, h, frames).is_some_and(|bytes| bytes <= limit)
}

/// Whether a resize fits both memory budgets: the new frames within the
/// per-operation budget, and old frames plus new within the total — undo
/// keeps the old ones while the worker holds the new, so the transient peak
/// is the sum. Overflow anywhere refuses, like `resize_fits_budget`.
pub(super) fn resize_fits_budgets(
    cw: u32,
    ch: u32,
    w: u32,
    h: u32,
    frames: usize,
    oper_max: usize,
    total_max: usize,
) -> bool {
    let (Some(old), Some(new)) = (resize_bytes(cw, ch, frames), resize_bytes(w, h, frames)) else {
        return false;
    };
    new <= oper_max && old.checked_add(new).is_some_and(|sum| sum <= total_max)
}

/// Refuse a resize the memory budgets cannot hold.
pub(super) fn toast_over_memory_limit(sender: &ComponentSender<App>) {
    // Translators: Shown when resized RGBA frames would exceed the configured memory limit.
    let text = t("That resize would use more memory than the configured limit.");
    sender.input(Msg::Toast(text.into()));
}

pub(super) fn crop_changes_canvas(
    (cw, ch): (u32, u32),
    (x, y, w, h): (u32, u32, u32, u32),
) -> bool {
    cw > 0 && ch > 0 && !(x == 0 && y == 0 && w >= cw && h >= ch)
}

/// Probe before decoding. ffprobe is cheap but it is still a subprocess, so it
/// runs off the main thread like the decode does. Only ffmpeg's inputs get
/// probed: a GIF arrives frame-exact and small, and a still image is one
/// frame, so neither has a cost to warn about.
pub(super) fn plan_import(
    path: PathBuf,
    options: ImportOptions,
    cancel: Arc<AtomicBool>,
    sender: &ComponentSender<App>,
) {
    if !pipeline::needs_ffmpeg(&path) {
        load(path, None, options, cancel, sender);
        return;
    }
    sender.spawn_command(move |out| {
        let plan = video::plan(&path, &options).map_err(|e| e.to_string());
        out.emit(Cmd::Planned(path, Box::new(plan)));
    });
}

/// How often a worker may post progress. A fast job would otherwise post a
/// message per frame, and the main loop has to drain each one before it can
/// draw any of them.
const PROGRESS_INTERVAL: std::time::Duration = std::time::Duration::from_millis(80);

/// Lets a worker's progress through at most once per `PROGRESS_INTERVAL`.
pub(super) struct Throttle {
    next: std::time::Instant,
}

impl Throttle {
    pub(super) fn new() -> Self {
        Self {
            next: std::time::Instant::now(),
        }
    }

    /// Whether a message may go now; if so, the next one waits an interval.
    pub(super) fn ready(&mut self) -> bool {
        let now = std::time::Instant::now();
        if now < self.next {
            return false;
        }
        self.next = now + PROGRESS_INTERVAL;
        true
    }
}

/// A frame job's progress callback: throttled `Cmd::WorkProgress` posts.
pub(super) fn work_progress(out: &relm4::Sender<Cmd>) -> impl FnMut(usize, usize) + '_ {
    let mut throttle = Throttle::new();
    move |done, total| {
        if throttle.ready() {
            let _ = out.send(Cmd::WorkProgress(done, total));
        }
    }
}

/// The heavy half of a splice, threaded exactly like `run_frame_work`: the
/// resampling happens on a worker against a snapshot of the document, and the
/// finished `Splice` comes back as one `Cmd::Spliced` to land as one history
/// step.
pub(super) fn run_splice(
    doc: &Document,
    at: usize,
    frames: Vec<Frame>,
    fit: FitMode,
    sender: &ComponentSender<App>,
) {
    let doc = doc.clone();
    sender.spawn_command(move |out| {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Box::new(fit::plan_splice(&doc, at, frames, fit, work_progress(&out)))
        }))
        .map_err(|_| n("The frame operation failed."));
        out.emit(Cmd::Spliced(result));
    });
}

/// The heavy half of a resize or zoom: a snapshot of the document goes to a
/// worker thread, which produces the frames one at a time and reports
/// progress; the finished set comes back as one `Cmd::Worked` and is applied
/// as a single history step. `Document` clones as pointer copies, so the
/// snapshot costs nothing but a couple of `Vec` headers.
pub(super) fn run_frame_work(doc: &Document, work: FrameWork, sender: &ComponentSender<App>) {
    let doc = doc.clone();
    sender.spawn_command(move |out| {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut progress = work_progress(&out);
            let frames = match &work {
                FrameWork::Resize(w, h) => doc.resized_frames(*w, *h, &mut progress),
                FrameWork::Zoom { frames, rect } => {
                    let (x, y, w, h) = *rect;
                    doc.zoomed_frames(frames, x, y, w, h, &mut progress)
                }
                FrameWork::Shrink { frames, rect } => {
                    let (x, y, w, h) = *rect;
                    doc.shrunk_frames(frames, x, y, w, h, &mut progress)
                }
                FrameWork::Crop { rect } => {
                    let (x, y, w, h) = *rect;
                    doc.cropped_frames(x, y, w, h, &mut progress)
                }
                FrameWork::Mirror { frames, axis } => {
                    doc.mirrored_frames(frames, *axis, &mut progress)
                }
                FrameWork::Fade {
                    frames,
                    fade,
                    colour,
                } => doc.faded_frames(frames, *fade, *colour, &mut progress),
                FrameWork::LoopFade { count } => doc.loop_faded_frames(*count, &mut progress),
            };
            Box::new(WorkDone {
                label: work.label(),
                frames_touched: frames.len(),
                scale: work.scale(&doc),
                offset: work.offset(),
                frames,
                land: work.landing(),
            })
        }))
        .map_err(|_| n("The frame operation failed."));
        out.emit(Cmd::Worked(result));
    });
}
