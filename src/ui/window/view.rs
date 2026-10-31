//! Writes widgets from the model: `Component::update_view`'s body.

use super::*;

impl App {
    pub(super) fn sync_view(&self, widgets: &mut Widgets, sender: ComponentSender<Self>) {
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
                        // Translators: Progress while frames being spliced in are resized to
                        // one canvas; both values are frame counts.
                        BusyKind::Fit => t("Fitting… {done} / {total} frames"),
                        // Translators: Progress while the document is being written out as a GIF; both values are frame counts.
                        BusyKind::Export => t("Exporting… {done} / {total} frames"),
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
        widgets.snap_button.set_tooltip_text(Some(
            &keys.tip(t("Snap to edges and center"), Action::ToggleSnap),
        ));
        if widgets.snap_button.is_active() != self.snap {
            widgets.snap_button.set_active(self.snap);
        }
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
        let crop = self.usable_crop();
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

        let frame_keys = strip_keys(&self.editor.doc);
        let rebuilt = *self.strip_keys.borrow() != frame_keys;
        if rebuilt {
            *self.strip_keys.borrow_mut() = frame_keys;
            rebuild_strip(
                &widgets.strip,
                &self.editor.doc,
                &sender,
                &widgets.scope_mirror,
                &widgets.drop_dividers,
                &self.keymap.borrow(),
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
        // but the overlay picker gives way to a summary: which overlays sit
        // on "the" frame has no single answer across several. An overlay
        // already picked keeps its editor, reaching every frame in scope.
        let multi = in_scope.len() > 1;
        // Playback rewrites this every tick; typing into it would land on
        // whichever frame the next tick shows.
        set_sensitive_releasing_focus(
            &widgets.frame_delay,
            count > 0 && self.busy.is_none() && !self.playing,
        );
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

        let (w, h) = self.editor.doc.size();
        // The overlay editor is part of the frame view: it shows only for an
        // overlay that is actually on the frame on screen. Selecting one on
        // another frame and navigating here leaves the plain frame view; going
        // back to its frame brings the editor back. A multi-frame scope keeps
        // it: the picked overlay is the answer to "which one", and the edit
        // lands on every frame of it the scope covers — a band click picks
        // the overlay's whole range for exactly that.
        let selected = self
            .editing_overlay()
            .and_then(|id| self.editor.doc.overlay(id));
        let kind = selected.map(|o| o.kind.clone());
        widgets.overlay_group.set_visible(kind.is_some());
        let is_text = matches!(kind, Some(OverlayKind::Text(_)));
        let is_shape = matches!(kind, Some(OverlayKind::Shape(_)));
        widgets.text_group.set_visible(!multi || is_text);
        let spread = selected.map_or(0, |o| self.edit_span(o.id).len());
        widgets.overlay_group.set_description(
            (spread > 1)
                .then(|| {
                    fill(
                        tn(
                            "Applies to {count} selected frame",
                            "Applies to {count} selected frames",
                            spread,
                        ),
                        &[("count", &spread.to_string())],
                    )
                })
                .as_deref(),
        );
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
                widgets.fill_button.set_rgba(t.color);
                widgets.fill_on.set_visible(false);
                widgets
                    .outline_button
                    .set_rgba(t.outline.map_or([0, 0, 0, 255], |(c, _)| c));
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
                widgets
                    .fill_button
                    .set_rgba(sh.fill.unwrap_or([255, 60, 60, 255]));
                widgets
                    .stroke_button
                    .set_rgba(sh.stroke.map_or([255, 60, 60, 255], |(c, _)| c));
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
            state.guides = self.guides;
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

pub(super) fn timecode(cs: u32) -> String {
    format!(
        "{:02}:{:02}.{}",
        cs / 6000,
        (cs / 100) % 60,
        (cs % 100) / 10
    )
}

pub(super) fn fps_of(doc: &Document) -> u32 {
    let total = doc.duration_cs();
    if total == 0 {
        0
    } else {
        (doc.frames.len() as f32 * 100.0 / total as f32).round() as u32
    }
}
