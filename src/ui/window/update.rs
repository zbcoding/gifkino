//! The relm4 component: init, message dispatch and command results.

use super::*;

impl Component for App {
    type Init = Option<PathBuf>;
    type Input = Msg;
    type Output = ();
    type CommandOutput = Cmd;
    type Root = adw::ApplicationWindow;
    type Widgets = Widgets;

    fn init_root() -> Self::Root {
        let window = adw::ApplicationWindow::builder()
            // A proper noun, not a phrase: not marked for translation, so no
            // locale can rename the application.
            .title("Gifkino")
            .default_width(1100)
            .default_height(760)
            .build();
        // A floor, so a drag stops at the width the editor needs instead of
        // squeezing past it: below this the toolbar and the import row are
        // wider than the window that holds them, which libadwaita reports as
        // a warning per frame and paints as clipped chrome. The dialogs are
        // narrower than this, so they fit inside whatever it allows.
        window.set_size_request(MIN_WINDOW_W, MIN_WINDOW_H);
        window
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        relm4::set_global_css(CSS);
        register_tool_icons();

        let model = App::new(
            Document::default(),
            Caps::probe(),
            Settings::load(),
            Keymap::load(),
        );

        let widgets = build(&root, &model, &sender);
        install_shortcuts(&root, &sender, model.keymap.clone());

        if let Some(path) = init {
            sender.input(Msg::Load(path));
        }

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Msg, sender: ComponentSender<Self>, root: &Self::Root) {
        if self.busy_blocks(&msg) {
            return;
        }
        match msg {
            Msg::Open => open_file_dialog(root, &sender),
            Msg::Load(path) => self.load_path(path, &sender),
            Msg::LoadInto(path, at) => self.load_into(path, at, &sender),
            Msg::ImportInto(at) => pick_import_file(root, at, &sender),
            Msg::SpliceFrames { at, frames, fit } => self.start_splice(at, frames, fit, &sender),
            Msg::LoadConfirmed(path, plan, budget) => {
                self.load_confirmed(path, *plan, budget, &sender)
            }
            Msg::CancelImport => self.cancel_import(),
            Msg::Estimate(rev) => self.measure_estimate(rev, &sender),
            Msg::Export => self.export_dialog(root, &sender),
            Msg::BeginExport {
                path,
                frames,
                settings,
            } => self.begin_export(path, frames, settings, &sender),
            Msg::Undo => self.undo(),
            Msg::Redo => self.redo(),
            Msg::TogglePlay => self.toggle_play(&sender),
            Msg::Tick => self.tick(&sender),
            Msg::Seek(i) => self.seek(i),
            Msg::ExtendSelection(i) => self.extend_selection(i),
            Msg::ToggleSelection(i) => self.toggle_selection(i),
            Msg::SetScope(choice) => self.set_scope(choice),
            Msg::AddOverlay(tool) => self.add_overlay(tool, &sender),
            Msg::SelectOverlay(id) => self.pick_overlay(id),
            Msg::SelectBand(Some(id)) => self.select_band(id),
            Msg::SelectBand(None) => self.selected_overlay = None,
            Msg::EditText(text) => self.edit_text(text),
            Msg::DeleteSelection => self.delete_selection(&sender),
            Msg::DeleteOverlay(id) => self.delete_overlay(id, &sender),
            Msg::CopyOverlayToFrames { all } => self.copy_selected_overlay(all, &sender),
            Msg::RestackOverlay { id, up } => self.restack_overlay(id, up),
            Msg::FrameCopy => self.copy_scope_frames(&sender),
            Msg::FramePaste => self.paste_clipboard(&sender),
            Msg::FrameOp(op) => self.run_frame_op(op, self.scope_frames(), &sender),
            Msg::SeekKeepSelection(i) => self.playhead = self.clamp_frame(i),
            Msg::MoveSelection { earlier } => self.nudge_selection(earlier, &sender),
            Msg::MoveSelectionTo { from, gap } => self.move_selection_to(from, gap, &sender),
            Msg::MoveFrame(from, to) => self.move_frame(from, to, &sender),
            Msg::MoveFrameDialog(i) => self.open_move_frame_dialog(root, i, &sender),
            Msg::FrameDelayDialog(i) => self.open_frame_delay_dialog(root, i, &sender),
            Msg::SetScopeDelay(cs) => self.set_scope_delay(cs, &sender),
            Msg::DelayAllDialog => self.open_delay_all_dialog(root, &sender),
            Msg::SetAllFramesDelay(cs) => self.set_all_frames_delay(cs, &sender),
            Msg::SelectAllFrames => self.select_all_frames(),
            Msg::ToggleBandsExpanded => self.bands_expanded = !self.bands_expanded,
            Msg::SetOverlayProp(prop) => self.restyle_overlay(prop),
            Msg::ToggleCropTool => self.toggle_crop_tool(),
            Msg::Escape => self.escape(),
            Msg::CanvasPress { x, y, scale, state } => self.canvas_press(x, y, scale, state),
            Msg::CanvasDrag { x, y, state } => self.canvas_drag(x, y, state),
            Msg::ToggleSnap => self.snap = !self.snap,
            Msg::CanvasRelease => self.finish_canvas_drag(&sender),
            Msg::ApplyCrop => self.apply_crop(&sender),
            Msg::ApplyZoom => self.start_zoom(self.frames_or_scope(false), &sender),
            Msg::ApplyZoomAll => self.start_zoom(self.frames_or_scope(true), &sender),
            Msg::ApplyShrink => self.apply_shrink(&sender),
            Msg::CropAllDialog => self.open_crop_all_dialog(root, &sender),
            Msg::CropAll(x, y, w, h) => self.start_crop((x, y, w, h), &sender),
            Msg::ResizeDialog => self.open_resize_dialog(root, &sender),
            Msg::DropEveryNth(every) => self.drop_every_nth(every, &sender),
            Msg::SmartDrop(percent) => self.smart_drop(percent, &sender),
            Msg::Resize(w, h) => self.start_resize(w, h, &sender),
            Msg::Mirror(axis) => self.mirror(axis, &sender),
            Msg::FadeDialog(fade) => fade_dialog(root, fade, self.scope_frames().len(), &sender),
            Msg::ApplyFade { fade, colour } => self.apply_fade(fade, colour, &sender),
            Msg::LoopFadeDialog => {
                loop_fade_dialog(root, self.editor.doc.loop_fade_limit(), &sender)
            }
            Msg::ApplyLoopFade(count) => self.apply_loop_fade(count, &sender),
            Msg::SetKeymap(map) => self.set_keymap(*map),
            Msg::Toast(_) | Msg::Notice(_) => {}
            Msg::StripZoom(factor) => self
                .strip_zoom
                .set(next_strip_zoom(self.strip_zoom.get(), factor)),
        }
    }

    fn update_cmd(&mut self, msg: Cmd, sender: ComponentSender<Self>, root: &Self::Root) {
        match msg {
            Cmd::ImportProgress(done, expected) => self.set_progress(done, expected),
            // The worker keeps counting under whatever bar is up; only the
            // numbers move, the bar never clears here.
            Cmd::WorkProgress(done, total) => self.set_progress(done, Some(total)),
            Cmd::Worked(result) => {
                if let Some(done) = self.finish_work(result, &sender) {
                    self.land_work(*done, &sender);
                }
            }
            Cmd::Spliced(result) => {
                if let Some(splice) = self.finish_work(result, &sender) {
                    self.land_splice(*splice, &sender);
                }
            }
            Cmd::Estimated(rev, result) => {
                if let Ok(bytes) = result {
                    self.estimate = Some((rev, bytes));
                }
                if self.estimate_pending == Some(rev) {
                    self.estimate_pending = None;
                }
            }
            // The probe answered. Every video stops here: the dialog is where
            // the size, the rate, the trim, the frame drop and this import's
            // memory budget are chosen, and a clip that already fits has the
            // same questions to answer as one that does not.
            Cmd::Planned(path, plan) => {
                self.busy = None;
                match *plan {
                    Ok(plan) => confirm_oversize(
                        root,
                        &path,
                        &plan,
                        self.settings.max_import_bytes,
                        &sender,
                    ),
                    Err(e) => sender.input(Msg::Toast(e)),
                }
            }
            Cmd::Imported(outcome) => {
                // An import ends the bar it started.
                self.busy = None;
                self.land_import(*outcome, root, &sender);
            }
            Cmd::Exported(Ok((path, size))) => {
                self.busy = None;
                let kb = size as f64 / 1024.0;
                sender.input(Msg::Toast(fill(
                    t("Exported to {path} · {size} KB"),
                    &[
                        ("path", &path.display().to_string()),
                        ("size", &format!("{kb:.0}")),
                    ],
                )));
            }
            Cmd::Exported(Err(e)) => {
                self.busy = None;
                sender.input(Msg::Toast(e))
            }
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
        // A delay typed into the spin stays uncommitted text until Enter or
        // focus-out, and a thumbnail click takes no focus. Left alone, a
        // seek either threw the typed value away or — when the next frame's
        // delay matched the spin's stale value, so `set_spin` skipped the
        // write — kept showing it, then committed it onto the new frame at
        // the next focus-out. Commit it here, onto the frames it was typed
        // for, before this message can move the playhead or change the scope.
        if let Some(cs) = commit_typed(&widgets.frame_delay, &widgets.sync) {
            self.set_scope_delay(cs as u16, &sender);
        }
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

    /// A job landing — an opened file, a resize, an effect — changes the
    /// document from here rather than from `update`, and needs measuring as
    /// much as an edit does; without this the readout said "sizing…" until
    /// the next click.
    fn update_cmd_with_view(
        &mut self,
        widgets: &mut Widgets,
        msg: Cmd,
        sender: ComponentSender<Self>,
        root: &Self::Root,
    ) {
        self.update_cmd(msg, sender.clone(), root);
        self.schedule_estimate(&sender);
        self.update_view(widgets, sender);
    }

    fn update_view(&self, widgets: &mut Widgets, sender: ComponentSender<Self>) {
        self.sync_view(widgets, &sender);
    }
}

/// The arms of `update` that take more than a call: each one message's worth
/// of model change, in the order the messages are declared.
impl App {
    /// Open `path` as the whole document.
    fn load_path(&mut self, path: PathBuf, sender: &ComponentSender<Self>) {
        self.import_at = None;
        self.start_import(path, sender);
    }

    /// Import `path` into the open document before frame `at`.
    fn load_into(&mut self, path: PathBuf, at: usize, sender: &ComponentSender<Self>) {
        if self.frame_count() == 0 {
            // Nothing to splice into: behave like a normal open.
            sender.input(Msg::Load(path));
            return;
        }
        self.import_at = Some(at.min(self.frame_count()));
        self.start_import(path, sender);
    }

    /// The oversize dialog said import: load `plan` under `budget`.
    fn load_confirmed(
        &mut self,
        path: PathBuf,
        plan: ImportPlan,
        budget: usize,
        sender: &ComponentSender<Self>,
    ) {
        let cancel = Arc::new(AtomicBool::new(false));
        let total = plan.frames();
        self.busy = Some(Busy::new(BusyKind::Import, total, Some(cancel.clone())));
        // Only for this import, and only upwards from the configured
        // budget: the plan was measured against it, and the frames it
        // lands still have to pass the splice's own check.
        self.import_budget = (budget > self.settings.max_import_bytes).then_some(budget);
        load(path, Some(plan), self.import_options(), cancel, sender);
    }

    fn cancel_import(&self) {
        // The flag is all the UI does; the worker reports back through
        // `Cmd::Imported` and is the only thing that clears the bar.
        if let Some(cancel) = self.busy.as_ref().and_then(|busy| busy.cancel.as_ref()) {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    fn undo(&mut self) {
        if self.editor.undo() {
            self.after_edit();
        }
    }

    fn redo(&mut self) {
        if self.editor.redo() {
            self.after_edit();
        }
    }

    /// Start or stop playback; a single frame has nothing to play.
    fn toggle_play(&mut self, sender: &ComponentSender<Self>) {
        self.playing = !self.playing && self.frame_count() > 1;
        if self.playing {
            self.schedule_tick(sender);
        }
    }

    /// Advance the playhead one frame, wrapping, and wait for the next.
    fn tick(&mut self, sender: &ComponentSender<Self>) {
        if self.playing && self.frame_count() > 0 {
            self.playhead = (self.playhead + 1) % self.frame_count();
            self.schedule_tick(sender);
        }
    }

    /// Jump to frame `i` and make it the one frame being worked on.
    fn seek(&mut self, i: usize) {
        self.playhead = self.clamp_frame(i);
        self.selection.clear();
        self.anchor = Some(self.playhead);
        // A seek is navigation to one frame, which is also choosing
        // one frame to work on: the scope follows it whatever it was.
        // Sticky All frames past a click is how a one-frame drag
        // became a hundred-frame edit.
        self.scope = ScopeChoice::ThisFrame;
    }

    /// Range needs a selection to be the range of.
    fn set_scope(&mut self, choice: ScopeChoice) {
        if choice != ScopeChoice::Range || !self.selection.is_empty() {
            self.scope = choice;
        }
    }

    fn copy_selected_overlay(&mut self, all: bool, sender: &ComponentSender<Self>) {
        if let Some(id) = self.selected_overlay {
            self.copy_overlay_to_frames(id, all, sender);
        }
    }

    /// Put the frames in scope on the clipboard and say how many.
    fn copy_scope_frames(&mut self, sender: &ComponentSender<Self>) {
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

    fn paste_clipboard(&mut self, sender: &ComponentSender<Self>) {
        let total = self.frame_count();
        if let Some(change) = self.paste_frames() {
            self.toast_if_document_wide(sender, &change, total);
        }
    }

    /// Move one frame from `from` to `to`, and the playhead with it.
    fn move_frame(&mut self, from: usize, to: usize, sender: &ComponentSender<Self>) {
        let total = self.frame_count();
        if from >= total || to >= total || from == to {
            return;
        }
        let (change, _) = self
            .editor
            .edit(n("Frame moved"), 1, |d| d.move_frame(from, to));
        self.playhead = to;
        self.after_edit();
        self.toast_if_document_wide(sender, &change, total);
    }

    fn open_move_frame_dialog(
        &self,
        root: &adw::ApplicationWindow,
        i: usize,
        sender: &ComponentSender<Self>,
    ) {
        if i >= self.frame_count() {
            return;
        }
        move_frame_dialog(root, i, self.frame_count(), sender);
    }

    /// The delay dialog for the scope, filled in with frame `i`'s delay as
    /// it stands now — not as it stood when the strip was last built.
    fn open_frame_delay_dialog(
        &self,
        root: &adw::ApplicationWindow,
        i: usize,
        sender: &ComponentSender<Self>,
    ) {
        let Some(frame) = self.editor.doc.frames.get(i) else {
            return;
        };
        delay_scope_dialog(root, &self.scope_frames(), frame.delay_cs, sender);
    }

    /// The delay dialog for every frame, filled in with the first one's.
    fn open_delay_all_dialog(&self, root: &adw::ApplicationWindow, sender: &ComponentSender<Self>) {
        if self.frame_count() == 0 {
            return;
        }
        let default = self.editor.doc.frames[0].delay_cs;
        delay_all_dialog(root, self.frame_count(), default, sender);
    }

    fn set_all_frames_delay(&mut self, cs: u16, sender: &ComponentSender<Self>) {
        let total = self.frame_count();
        if total == 0 {
            return;
        }
        let (change, _) = self
            .editor
            .edit(n("Delay set"), total, |d| d.set_delay(0..total, cs.max(1)));
        self.after_edit();
        self.toast_if_document_wide(sender, &change, total);
    }

    fn select_all_frames(&mut self) {
        if self.frame_count() > 0 {
            self.selection.clear();
            self.scope = ScopeChoice::AllFrames;
        }
    }

    /// Arm or disarm the crop tool; an armed one drops the overlay pick.
    fn toggle_crop_tool(&mut self) {
        self.crop_tool = !self.crop_tool;
        self.crop_rect = None;
        if self.crop_tool {
            self.selected_overlay = None;
        }
    }

    /// Back out of the crop tool if it is armed, else drop the overlay pick.
    fn escape(&mut self) {
        if self.crop_tool {
            self.leave_crop();
        } else {
            self.selected_overlay = None;
        }
    }

    fn finish_canvas_drag(&mut self, sender: &ComponentSender<Self>) {
        if let Some(change) = self.canvas_release() {
            self.toast_if_document_wide(sender, &change, self.frame_count());
        }
    }

    /// Crop the canvas to the rectangle drawn with the crop tool.
    fn apply_crop(&mut self, sender: &ComponentSender<Self>) {
        let Some((x, y, w, h)) = self.crop_rect.take() else {
            return;
        };
        let Some(rect) = normalize_canvas_rect(self.editor.doc.size(), (x, y, w, h)) else {
            return;
        };
        self.start_crop(rect, sender);
    }

    /// Shrink the frames in scope into the rectangle drawn with the crop tool.
    fn apply_shrink(&mut self, sender: &ComponentSender<Self>) {
        let Some(rect) = self.crop_rect.take() else {
            return;
        };
        let Some(work) = self.shrink_work(rect) else {
            return;
        };
        self.crop_tool = false;
        self.start_frame_work(work, sender);
    }

    fn open_crop_all_dialog(&self, root: &adw::ApplicationWindow, sender: &ComponentSender<Self>) {
        let (cw, ch) = self.editor.doc.size();
        if cw == 0 || ch == 0 {
            return;
        }
        crop_dialog(root, cw, ch, sender);
    }

    fn open_resize_dialog(&self, root: &adw::ApplicationWindow, sender: &ComponentSender<Self>) {
        let (cw, ch) = self.editor.doc.size();
        if cw == 0 || ch == 0 {
            return;
        }
        let (oper_max, total_max) = (self.settings.max_oper_bytes, self.settings.max_total_bytes);
        resize_dialog(
            root,
            cw,
            ch,
            self.frame_count(),
            oper_max,
            total_max,
            sender,
        );
    }

    fn drop_every_nth(&mut self, every: usize, sender: &ComponentSender<Self>) {
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
        self.toast_if_document_wide(sender, &change, total);
    }

    /// Drop the `percent` of frames that move least, never more than 95%.
    fn smart_drop(&mut self, percent: usize, sender: &ComponentSender<Self>) {
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
        self.toast_if_document_wide(sender, &change, total);
    }

    fn mirror(&mut self, axis: Axis, sender: &ComponentSender<Self>) {
        let frames = self.scope_frames();
        if !frames.is_empty() {
            self.start_frame_work(FrameWork::Mirror { frames, axis }, sender);
        }
    }

    /// A fade needs two frames to ramp across.
    fn apply_fade(&mut self, fade: Fade, colour: Rgba8, sender: &ComponentSender<Self>) {
        let frames = self.scope_frames();
        if frames.len() >= 2 {
            let work = FrameWork::Fade {
                frames,
                fade,
                colour,
            };
            self.start_frame_work(work, sender);
        }
    }

    fn apply_loop_fade(&mut self, count: usize, sender: &ComponentSender<Self>) {
        if count.min(self.editor.doc.loop_fade_limit()) > 0 {
            self.start_frame_work(FrameWork::LoopFade { count }, sender);
        }
    }

    /// Save and adopt the keymap the shortcuts editor sent.
    fn set_keymap(&mut self, map: Keymap) {
        map.save();
        *self.keymap.borrow_mut() = map;
        // The frame context menu shows its shortcuts.
        self.frame_menu_stale.set(true);
    }
}
