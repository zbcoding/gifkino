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
        let blocked = self.busy.as_ref().is_some_and(|busy| match busy.kind {
            BusyKind::Import => msg.requires_idle() || msg.edits_document(),
            BusyKind::Resize
            | BusyKind::Zoom
            | BusyKind::Shrink
            | BusyKind::Crop
            | BusyKind::Fit
            | BusyKind::Effect => {
                msg.requires_idle()
                    && !(matches!(msg, Msg::DeleteSelection) && self.selected_overlay.is_some())
            }
            // An export encodes a snapshot, so edits stay safe while it runs;
            // only a second job fighting over the one bar waits.
            BusyKind::Export => msg.requires_idle() && !msg.edits_document(),
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
                self.import_at = None;
                self.start_import(path, &sender);
            }
            Msg::LoadInto(path, at) => {
                if self.frame_count() == 0 {
                    // Nothing to splice into: behave like a normal open.
                    sender.input(Msg::Load(path));
                    return;
                }
                self.import_at = Some(at.min(self.frame_count()));
                self.start_import(path, &sender);
            }
            Msg::ImportInto(at) => {
                pick_import_file(root, at, &sender);
            }
            Msg::SpliceFrames { at, frames, fit } => {
                self.start_splice(at, frames, fit, &sender);
            }
            Msg::LoadConfirmed(path, plan, budget) => {
                let cancel = Arc::new(AtomicBool::new(false));
                let total = plan.frames();
                self.busy = Some(Busy::new(BusyKind::Import, total, Some(cancel.clone())));
                // Only for this import, and only upwards from the configured
                // budget: the plan was measured against it, and the frames it
                // lands still have to pass the splice's own check.
                self.import_budget = (budget > self.settings.max_import_bytes).then_some(budget);
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
                    sender.input(Msg::BeginExport {
                        path,
                        frames,
                        settings,
                    });
                });
            }
            Msg::BeginExport {
                path,
                frames,
                settings,
            } => {
                let total = frames.len();
                self.busy = Some(Busy::new(BusyKind::Export, Some(total), None));
                let enc = Encodable { frames };
                sender.spawn_command(move |out| {
                    let mut progress = work_progress(&out);
                    // bind so the closure captures the PathBuf, not `*path`
                    let path = path;
                    let result = gif_pipeline::export_path(&path, &enc, &settings, &mut progress)
                        .map(|size| (path, size))
                        .map_err(|e| e.to_string());
                    out.emit(Cmd::Exported(result));
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
                self.playhead = self.clamp_frame(i);
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
                let i = self.clamp_frame(i);
                let anchor = self.clamp_frame(self.anchor.unwrap_or(self.playhead));
                self.selection = run_between(anchor, i);
                self.anchor = Some(anchor);
                self.playhead = i;
                self.scope = ScopeChoice::Range;
            }
            Msg::ToggleSelection(i) => {
                // Ctrl adds or removes one frame anywhere, which is the whole
                // difference from Shift: the result need not be a run.
                let i = self.clamp_frame(i);
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
                let (kind, transform) = tool.new_overlay(self.editor.doc.size());
                let touched = frames.len();
                let (change, ids) = self.editor.edit(tool.added_label(), touched, |d| {
                    d.add_overlay_over(tool_label(tool), kind, transform, &frames)
                });
                self.leave_crop();
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
                self.pick_overlay(id);
            }
            Msg::SelectBand(Some(id)) => self.select_band(id),
            Msg::SelectBand(None) => self.selected_overlay = None,
            Msg::EditText(text) => {
                let Some(id) = self.selected_overlay else {
                    return;
                };
                // The scope decides which frames the text change lands on; a
                // narrow scope splits the overlay so the rest keeps its text.
                let span = self.edit_span(id);
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
                    self.scope = ScopeChoice::ThisFrame;
                    self.after_edit();
                    self.toast_if_document_wide(&sender, &change, total);
                }
            }
            Msg::DeleteOverlay(id) => {
                self.delete_overlay(id, &sender);
            }
            Msg::CopyOverlayToFrames { all } => {
                if let Some(id) = self.selected_overlay {
                    self.copy_overlay_to_frames(id, all, &sender);
                }
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
                self.playhead = self.clamp_frame(i);
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
            Msg::SetScopeDelay(cs) => self.set_scope_delay(cs, &sender),
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
                let span = self.edit_span(id);
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
                    self.leave_crop();
                } else {
                    self.selected_overlay = None;
                }
            }
            Msg::CanvasPress { x, y, scale, state } => self.canvas_press(x, y, scale, state),
            Msg::CanvasDrag { x, y, state } => self.canvas_drag(x, y, state),
            Msg::ToggleSnap => self.snap = !self.snap,
            Msg::CanvasRelease => {
                if let Some(change) = self.canvas_release() {
                    self.toast_if_document_wide(&sender, &change, self.frame_count());
                }
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
            Msg::ApplyZoom => self.start_zoom(self.frames_or_scope(false), &sender),
            Msg::ApplyZoomAll => self.start_zoom(self.frames_or_scope(true), &sender),
            Msg::ApplyShrink => {
                let Some(rect) = self.crop_rect.take() else {
                    return;
                };
                let Some(work) = self.shrink_work(rect) else {
                    return;
                };
                self.crop_tool = false;
                self.start_frame_work(work, &sender);
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
            Msg::ResizeDialog => {
                let (cw, ch) = self.editor.doc.size();
                if cw == 0 || ch == 0 {
                    return;
                }
                let (oper_max, total_max) =
                    (self.settings.max_oper_bytes, self.settings.max_total_bytes);
                resize_dialog(
                    root,
                    cw,
                    ch,
                    self.frame_count(),
                    oper_max,
                    total_max,
                    &sender,
                );
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
                // The operation budget caps the new frames and the total caps
                // old plus new: the old frames stay resident in undo history
                // while the worker holds the new ones.
                let (cw, ch) = self.editor.doc.size();
                let fits = resize_fits_budgets(
                    cw,
                    ch,
                    w,
                    h,
                    self.frame_count(),
                    self.settings.max_oper_bytes,
                    self.settings.max_total_bytes,
                );
                if !fits {
                    toast_over_memory_limit(&sender);
                    return;
                }
                self.start_frame_work(FrameWork::Resize(w, h), &sender);
            }
            Msg::Mirror(axis) => {
                let frames = self.scope_frames();
                if !frames.is_empty() {
                    self.start_frame_work(FrameWork::Mirror { frames, axis }, &sender);
                }
            }
            Msg::FadeDialog(fade) => {
                fade_dialog(root, fade, self.scope_frames().len(), &sender);
            }
            Msg::ApplyFade { fade, colour } => {
                let frames = self.scope_frames();
                if frames.len() >= 2 {
                    let work = FrameWork::Fade {
                        frames,
                        fade,
                        colour,
                    };
                    self.start_frame_work(work, &sender);
                }
            }
            Msg::LoopFadeDialog => {
                loop_fade_dialog(root, self.editor.doc.loop_fade_limit(), &sender);
            }
            Msg::ApplyLoopFade(count) => {
                if count.min(self.editor.doc.loop_fade_limit()) > 0 {
                    self.start_frame_work(FrameWork::LoopFade { count }, &sender);
                }
            }
            Msg::SetKeymap(map) => {
                map.save();
                *self.keymap.borrow_mut() = *map;
                // The frame context menu shows its shortcuts, and the popover
                // carrying it is built with the strip — so drop the cached
                // frame list to make `update_view` rebuild both. A rebind is
                // rare enough that the thumbnails' hitch does not matter.
                self.strip_keys.borrow_mut().clear();
            }
            Msg::Toast(_) | Msg::Notice(_) => {}
            Msg::StripZoom(factor) => {
                self.strip_zoom
                    .set(next_strip_zoom(self.strip_zoom.get(), factor));
            }
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

    fn update_view(&self, widgets: &mut Widgets, sender: ComponentSender<Self>) {
        self.sync_view(widgets, &sender);
    }
}
