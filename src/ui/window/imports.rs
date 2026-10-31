//! Import planning: the oversize confirmation, trim fields, fit dialog and the loader.

use super::*;

/// The file cannot come in whole, so let the user pick the size it comes in at
/// rather than only telling them what we decided. Memory is exact; the GIF
/// figure is a range, because the content decides that one.
pub(super) fn confirm_oversize(
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
        Some(if plan.over_budget() {
            t("This video is large")
        } else {
            t("Import video")
        }),
        Some(&import_body(&name, plan)),
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

    let lock = adw::SwitchRow::builder()
        .title(t("Keep aspect ratio"))
        .build();
    lock.set_active(true);

    // ---- Advanced: what part of the file comes in, how much of it is kept,
    // and how much memory this one import may spend.
    let (advanced, trim_units, start, end, drop_nth, ignore_limit) = advanced_import_rows(&source);

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

    // Above the form, not under it: with Advanced open the form is taller
    // than the dialog and scrolls, and the preview is the number every one of
    // those fields is being steered by. Centred, because it reads as the last
    // line of the body it sits under.
    let summary = gtk::Label::new(None);
    summary.add_css_class("dim-label");
    summary.set_justify(gtk::Justification::Center);
    summary.set_wrap(true);
    summary.set_margin_bottom(12);

    // Wide enough for a preference row's title and its control to share a
    // line: the advanced rows say what they do in words, not abbreviations.
    dialog.set_content_width(560);

    let group = adw::PreferencesGroup::new();
    group.add(&size_row);
    group.add(&custom_row);
    group.add(&lock);
    group.add(&rate_row);
    group.add(&advanced);
    let form = gtk::Box::new(gtk::Orientation::Vertical, 0);
    form.append(&summary);
    form.append(&group);
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

    // The budget the plan is measured against and the import will run under:
    // whatever the memory field says, in bytes.
    let chosen_budget = Rc::new(Cell::new(max_bytes));

    let refresh = {
        let (source, chosen, summary) = (source.clone(), chosen.clone(), summary.clone());
        let dialog = dialog.clone();
        let (width, height, rate) = (width.clone(), height.clone(), rate.clone());
        let (trim_units, start, end) = (trim_units.clone(), start.clone(), end.clone());
        let (drop_nth, ignore_limit) = (drop_nth.clone(), ignore_limit.clone());
        let (generation, measure) = (generation.clone(), measure.clone());
        let chosen_budget = chosen_budget.clone();
        move || {
            generation.set(generation.get() + 1);
            let started_at = generation.get();

            let in_frames = trim_units.selected() == 1;
            let trim = read_trim(in_frames, &start.text(), &end.text(), &source);
            // A half-typed field is not a reason to preview something nobody
            // asked for, so the field says so and Import waits for it.
            for entry in [&start, &end] {
                entry.remove_css_class("error");
            }
            if let TrimInput::Invalid = trim {
                for entry in [&start, &end] {
                    if !entry.text().trim().is_empty() {
                        entry.add_css_class("error");
                    }
                }
            }

            // Switched off, the configured budget stands. Switched on, the
            // ceiling is set past anything a decode could reach rather than
            // removed: `cap` is still what stops a stream with no duration,
            // and `usize::MAX` would overflow the arithmetic that derives it.
            let bytes = if ignore_limit.is_active() {
                NO_IMPORT_LIMIT
            } else {
                max_bytes
            };
            chosen_budget.set(bytes);
            let target = (requested_dimension(&width), requested_dimension(&height));
            let options = ImportOptions {
                target: Some(target),
                fps: Some(rate.value()),
                trim: trim.trim(),
                drop_nth: Some(drop_nth.value() as usize),
                max_bytes: bytes,
                ..Default::default()
            };
            let plan = video::plan_for(source.clone(), &options);
            summary.set_label(&plan_summary(&plan, None));
            // A file whose container names no duration has no count to
            // extrapolate a GIF size from, and the preview says so instead —
            // so the ffmpeg spawns behind the measurement would be thrown away.
            let fits = !plan.over_budget() && !matches!(trim, TrimInput::Invalid);
            let worth_measuring = fits && plan.frames().is_some();
            *chosen.borrow_mut() = plan;
            dialog.set_response_enabled("import", fits);

            // No point spending ffmpeg on settings that cannot be imported.
            if worth_measuring {
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

    let set_pair = link_aspect_fields(&width, &height, &lock, source.width, source.height);
    {
        let (width, height, lock) = (width.clone(), height.clone(), lock.clone());
        let (refresh, set_pair) = (refresh.clone(), set_pair.clone());
        let sizes = sizes.clone();
        presets.connect_selected_notify(move |drop| {
            let picked = drop.selected() as usize;
            // Custom starts from wherever the last preset left the spins. A
            // preset carries its own pair, so both fields are written at once
            // rather than letting the aspect lock re-derive one of them off
            // the other's rounding.
            if let Some((w, h)) = sizes.get(picked) {
                set_pair(*w, *h);
            }
            width.set_sensitive(picked == custom);
            height.set_sensitive(picked == custom);
            lock.set_sensitive(picked == custom);
            refresh();
        });
    }
    width.set_sensitive(false);
    height.set_sensitive(false);
    lock.set_sensitive(false);
    for spin in [&width, &height] {
        // Both commit paths, the way the resize dialog links its fields: the
        // entry's `changed` for keystrokes the spin has not committed yet, and
        // `value-changed` for arrows, Enter and the aspect lock's own writes.
        let refresh = refresh.clone();
        let commit = refresh.clone();
        spin.connect_changed(move |_| refresh());
        spin.connect_value_changed(move |_| commit());
    }
    {
        let refresh = refresh.clone();
        rate.connect_value_changed(move |_| refresh());
    }
    {
        // Both commit paths, like the size fields: a typed drop count moves
        // the preview as it is typed, not only when the field commits.
        let refresh = refresh.clone();
        let commit = refresh.clone();
        drop_nth.connect_changed(move |_| refresh());
        drop_nth.connect_value_changed(move |_| commit());
    }
    {
        let refresh = refresh.clone();
        ignore_limit.connect_active_notify(move |_| refresh());
    }
    for entry in [&start, &end] {
        let refresh = refresh.clone();
        entry.connect_changed(move |_| refresh());
    }
    {
        let refresh = refresh.clone();
        let (start, end) = (start.clone(), end.clone());
        trim_units.connect_selected_notify(move |units| {
            // The example follows the unit: a timecode reads as nonsense in a
            // field that wants a frame number.
            set_trim_examples(&start, &end, units.selected() == 1, &source);
            refresh();
        });
    }

    let sender = sender.clone();
    let path = path.to_path_buf();
    dialog.choose(Some(root), gio::Cancellable::NONE, move |response| {
        if response == "import" {
            sender.input(Msg::LoadConfirmed(
                path,
                Box::new(chosen.borrow().clone()),
                chosen_budget.get(),
            ));
        }
    });
}

/// The import dialog's advanced section: which part of the file comes in, how
/// many of its frames are kept, and how much memory this one import may
/// spend. Full-width preference rows rather than an action row with the
/// control hung off the end — a title long enough to say what a field does
/// and a field wide enough to type into do not fit on one line together, and
/// cramming them squeezes the title to an ellipsis. Built apart from the
/// dialog so the widget test can hold the rows without showing them.
pub(super) fn advanced_import_rows(
    source: &VideoInfo,
) -> (
    adw::ExpanderRow,
    gtk::DropDown,
    gtk::Entry,
    gtk::Entry,
    gtk::SpinButton,
    gtk::Switch,
) {
    // What the trim fields are aimed at, in the units they are typed in:
    // without it the only way to find the end of the clip is to overshoot and
    // have the field turn red.
    let extent = match (source.duration_s, source_frames(source)) {
        (Some(seconds), Some(frames)) => fill(
            // Translators: The length of the file being imported. {time} is HH:MM:SS.mmm.
            t("{time} · {frames} frames"),
            &[
                ("time", &timestamp(seconds)),
                ("frames", &frames.to_string()),
            ],
        ),
        _ => t("no duration the container will admit to").into(),
    };
    let extent_label = gtk::Label::new(Some(&extent));
    extent_label.add_css_class("dim-label");
    extent_label.set_valign(gtk::Align::Center);
    extent_label.set_selectable(true);
    let extent_row = adw::ActionRow::builder().title(t("This video")).build();
    extent_row.add_suffix(&extent_label);

    // The same control the resolution picker uses, in the same kind of row:
    // one dropdown style across the dialog rather than two that differ only
    // in which widget happened to be reached for.
    let trim_units = gtk::DropDown::from_strings(&[t("Seconds"), t("Frames")]);
    trim_units.set_valign(gtk::Align::Center);
    // Translators: Help for the trim fields. A GIF's delays are whole centiseconds, so hundredths are the finest a cut can be.
    let trim_help = t(
        "Leave both blank for the whole clip. Seconds read 0:01.12 — a GIF holds \
        hundredths, so that is as fine as a cut gets. Frames counts frame numbers, both ends included.",
    );
    let trim_row = helped_row(t("Trim by"), trim_help);
    trim_row.add_suffix(&trim_units);

    // A label on the left and an input on the right, the way a web form does
    // it: the example lives in the placeholder alone, where it disappears the
    // moment the field has something in it. `AdwEntryRow` would put its title
    // over that placeholder and show the format twice.
    let (start, end) = (trim_entry(), trim_entry());
    set_trim_examples(&start, &end, false, source);
    let start_row = trim_field_row(t("From"), &start, false, &trim_units, source);
    let end_row = trim_field_row(t("To"), &end, true, &trim_units, source);

    // The same optimization the Optimize menu offers, done on the way in:
    // the frames dropped here never cost the memory the budget is counting.
    let drop_nth = gtk::SpinButton::with_range(1.0, 60.0, 1.0);
    drop_nth.set_valign(gtk::Align::Center);
    // Two digits is the whole range it offers, so it is sized for two.
    drop_nth.set_width_chars(2);
    drop_nth.set_max_width_chars(2);
    let drop_help = t(
        "1 keeps every frame. More drops one frame in every N on the way in, so \
        those frames never cost memory and the ones kept still cover the same stretch of time.",
    );
    let drop_row = helped_row(t("Drop one frame in every"), drop_help);
    // The unit belongs after the number, the way the size and rate rows put
    // px and fps after theirs: "Drop one frame in every 3" says nothing about
    // what the 3 counts.
    let drop_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    drop_box.set_valign(gtk::Align::Center);
    drop_box.append(&drop_nth);
    // Translators: The unit after the frame-drop field: "Drop one frame in every 3 frames".
    drop_box.append(&gtk::Label::new(Some(t("frames"))));
    drop_row.add_suffix(&drop_box);

    // Not a figure in megabytes: the limit is there to stop an accidental
    // 30 GB import, and the answer to "I meant it" is one switch. Nobody can
    // estimate the number that would have been typed here anyway.
    let ignore_limit = gtk::Switch::new();
    ignore_limit.set_valign(gtk::Align::Center);
    let limit_help = t(
        "Frames are held uncompressed in memory while you edit, which is what the \
        limit counts. This lets one import past it; the configured limit is left alone.",
    );
    let limit_row = helped_row(t("Ignore the memory limit"), limit_help);
    limit_row.add_suffix(&ignore_limit);

    let advanced = adw::ExpanderRow::builder().title(t("Advanced")).build();
    for row in [
        extent_row.upcast_ref::<gtk::Widget>(),
        trim_row.upcast_ref(),
        start_row.upcast_ref(),
        end_row.upcast_ref(),
        drop_row.upcast_ref(),
        limit_row.upcast_ref(),
    ] {
        advanced.add_row(row);
    }
    (advanced, trim_units, start, end, drop_nth, ignore_limit)
}

/// A preference row whose label carries its own question mark, right after
/// the words it explains rather than out at the far edge where it reads as
/// one more control. `AdwActionRow` has no slot inside its title, so the
/// label and the button share the row's prefix and the title stays empty.
pub(super) fn helped_row(title: &str, explanation: &str) -> adw::ActionRow {
    let label = gtk::Label::new(Some(title));
    label.set_xalign(0.0);
    let heading = gtk::Box::new(gtk::Orientation::Horizontal, 3);
    heading.set_valign(gtk::Align::Center);
    heading.append(&label);
    heading.append(&help_button(explanation));

    let row = adw::ActionRow::new();
    row.add_prefix(&heading);
    row
}

/// A question mark beside a field, explaining it when clicked or tapped. One
/// way in, not two: a tooltip that says the same thing as the popover fires
/// on the way to the control and covers the row it is explaining.
pub(super) fn help_button(explanation: &str) -> gtk::MenuButton {
    let label = gtk::Label::builder()
        .label(explanation)
        .wrap(true)
        .max_width_chars(38)
        .xalign(0.0)
        .build();
    let popover = gtk::Popover::builder().child(&label).build();
    let button = gtk::MenuButton::builder()
        .icon_name("help-about-symbolic")
        .popover(&popover)
        .valign(gtk::Align::Center)
        .build();
    button.add_css_class("flat");
    button.add_css_class("circular");
    // Nothing on hover, but a screen reader still has to be able to say what
    // the button is for.
    button.update_property(&[gtk::accessible::Property::Label(t("Help"))]);
    button
}

/// One end of the trim, sized for the longest thing it can hold:
/// `00:00:00.000` is twelve characters, and a field that ellipsises its own
/// placeholder is not showing the format at all.
pub(super) fn trim_entry() -> gtk::Entry {
    gtk::Entry::builder()
        .width_chars(12)
        .max_width_chars(12)
        .valign(gtk::Align::Center)
        .build()
}

/// One trim end as a row: the field, and the pair of steppers beside it that
/// nudge it by the unit it is being read in. A trim is adjusted far more
/// often than it is typed from scratch, and stepping can only ever land
/// inside the file, so it is also the way to find the ends without being told
/// off by a red field.
pub(super) fn trim_field_row(
    title: &str,
    field: &gtk::Entry,
    is_end: bool,
    units: &gtk::DropDown,
    source: &VideoInfo,
) -> adw::ActionRow {
    let steppers = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    steppers.set_valign(gtk::Align::Center);
    steppers.add_css_class("linked");
    for (icon, forward, tip) in [
        // The icons a `GtkSpinButton` uses for its own steppers, so these read
        // as the same control as the size and rate fields rather than as a
        // remove and an add.
        // Translators: Tooltip on the trim field's minus button. The step is a second, or ten frames.
        ("value-decrease-symbolic", false, t("One step earlier")),
        // Translators: Tooltip on the trim field's plus button.
        ("value-increase-symbolic", true, t("One step later")),
    ] {
        let button = gtk::Button::from_icon_name(icon);
        button.set_tooltip_text(Some(tip));
        let (field, units, source) = (field.clone(), units.clone(), source.clone());
        button.connect_clicked(move |_| {
            let stepped = stepped_trim(
                &field.text(),
                forward,
                units.selected() == 1,
                is_end,
                &source,
            );
            // Writing the text is what refreshes the preview: the entry's own
            // `changed` is already wired to it.
            field.set_text(&stepped);
        });
        steppers.append(&button);
    }

    let field_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    field_box.set_valign(gtk::Align::Center);
    field_box.append(field);
    field_box.append(&steppers);

    let row = adw::ActionRow::builder().title(title).build();
    row.add_suffix(&field_box);
    row
}

/// A second, in the seconds field.
pub(super) const TRIM_STEP_S: f64 = 1.0;
/// Ten frames, in the frames field: one frame at a time is the wrong grain
/// for a button, and ten is about a third of a second of most footage.
pub(super) const TRIM_STEP_FRAMES: usize = 10;

/// What a trim field should read after one step. An empty or unparseable
/// field steps from the end it stands for — the start of the clip, or its
/// end — so the first click lands somewhere useful instead of nowhere, and
/// the result is clamped inside the file, so stepping can never produce a
/// value the field would then refuse. Pure, so the arithmetic is tested
/// without a display.
pub(super) fn stepped_trim(
    text: &str,
    forward: bool,
    in_frames: bool,
    is_end: bool,
    source: &VideoInfo,
) -> String {
    if in_frames {
        let last = source_frames(source).map(|n| n.saturating_sub(1));
        let current = parse_index(text).unwrap_or(if is_end { last.unwrap_or(0) } else { 0 });
        let stepped = if forward {
            current.saturating_add(TRIM_STEP_FRAMES)
        } else {
            current.saturating_sub(TRIM_STEP_FRAMES)
        };
        return last.map_or(stepped, |max| stepped.min(max)).to_string();
    }
    let end = source.duration_s.unwrap_or(f64::MAX);
    let current = parse_seconds(text).unwrap_or(if is_end { end } else { 0.0 });
    let stepped = if forward {
        current + TRIM_STEP_S
    } else {
        current - TRIM_STEP_S
    };
    timestamp(stepped.clamp(0.0, end))
}

/// Show the format the trim fields take, in the file's own numbers: the start
/// of the clip and its end, as `HH:MM:SS.mmm`, or the first and last frame
/// index when the trim is counted in frames. Placeholder only — the row's
/// label says which end it is, and it also says what the field will do if
/// left alone, since a blank end is the end of the clip. A container with no
/// duration has no numbers of its own, so it gets the bare format.
pub(super) fn set_trim_examples(
    start: &gtk::Entry,
    end: &gtk::Entry,
    in_frames: bool,
    source: &VideoInfo,
) {
    // The examples are not translated: a timecode and a frame index read the
    // same everywhere, and a translator has nothing to decide about them.
    let (from, to) = match (in_frames, source.duration_s, source_frames(source)) {
        (true, _, Some(frames)) => ("0".to_string(), (frames.saturating_sub(1)).to_string()),
        (true, ..) => ("0".to_string(), "1234".to_string()),
        (false, Some(seconds), _) => (timestamp(0.0), timestamp(seconds)),
        (false, ..) => (timestamp(0.0), "00:01:23.450".to_string()),
    };
    start.set_placeholder_text(Some(&from));
    end.set_placeholder_text(Some(&to));
}

/// The dialog's opening line: what the file is, and — when it will not fit —
/// why. The numbers for a given choice live in the live summary instead.
/// Pango markup, so the filename stands out from the numbers around it.
/// Everything interpolated is escaped; only the emphasis is ours.
pub(super) fn import_body(name: &str, plan: &ImportPlan) -> String {
    let src = &plan.source;
    let length = match src.duration_s {
        Some(d) => fill(t("runs {duration}"), &[("duration", &clock(d))]),
        None => t("has no duration the container will admit to").into(),
    };
    let opening = fill(
        // Translators: Pango markup: keep the <b> tags. {length} is "runs 1:30" or the no-duration phrase.
        t("<b>{name}</b> {length} at {width}×{height}, {fps} fps."),
        &[
            ("name", &glib::markup_escape_text(name)),
            ("length", &glib::markup_escape_text(&length)),
            ("width", &src.width.to_string()),
            ("height", &src.height.to_string()),
            ("fps", &format!("{:.0}", src.fps)),
        ],
    );
    // A file that fits needs no advice: the fields are in front of the user
    // and the preview under them says what they cost. Only a refusal has
    // something to explain — the greyed-out Import button.
    let Some(advice) = plan.over_budget().then(|| {
        t(
            "That is more than will fit in memory at full size. Pick a smaller size or rate, \
          or open Advanced to trim it, drop frames, or ignore the limit for this import.",
        )
    }) else {
        return opening;
    };
    format!("{opening}\n\n{advice}")
}

/// What the two trim fields currently say. `Invalid` is its own answer rather
/// than a silent fall back to the whole clip: a typo that imports 30 minutes
/// instead of 3 seconds is the expensive mistake here.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum TrimInput {
    Whole,
    Set(Trim),
    Invalid,
}

impl TrimInput {
    pub(super) fn trim(self) -> Option<Trim> {
        match self {
            TrimInput::Set(trim) => Some(trim),
            TrimInput::Whole | TrimInput::Invalid => None,
        }
    }
}

/// Read the trim fields against the file they describe: blank is the whole
/// clip, one end alone is open at the other, and `Invalid` covers anything
/// unparseable, an end at or before its start, and either end past what the
/// source actually holds. A container that admits no duration has nothing to
/// check against, so only the shape of the input is judged there. Pure, so
/// the parsing has unit tests without a display.
pub(super) fn read_trim(in_frames: bool, start: &str, end: &str, source: &VideoInfo) -> TrimInput {
    let (start, end) = (start.trim(), end.trim());
    if start.is_empty() && end.is_empty() {
        return TrimInput::Whole;
    }
    if in_frames {
        // Inclusive indices, so the last frame a 300-frame clip has is 299.
        let last_index = source_frames(source).map(|n| n.saturating_sub(1));
        let within = |n: usize| last_index.is_none_or(|max| n <= max);
        let first = match parse_index(start) {
            Some(n) if within(n) => n,
            None if start.is_empty() => 0,
            _ => return TrimInput::Invalid,
        };
        let last = match parse_index(end) {
            Some(n) if n > first && within(n) => Some(n),
            None if end.is_empty() => None,
            _ => return TrimInput::Invalid,
        };
        return TrimInput::Set(Trim::Frames { first, last });
    }
    // The last moment worth starting at: a start on the final frame's own
    // timestamp still has that frame to give, one past the end has nothing.
    let within = |s: f64| source.duration_s.is_none_or(|d| s <= d);
    let from = match parse_seconds(start) {
        Some(s) if within(s) => s,
        None if start.is_empty() => 0.0,
        _ => return TrimInput::Invalid,
    };
    let to = match parse_seconds(end) {
        Some(s) if s > from && within(s) => Some(s),
        None if end.is_empty() => None,
        _ => return TrimInput::Invalid,
    };
    TrimInput::Set(Trim::Seconds {
        start: from,
        end: to,
    })
}

/// Frames the source holds, at its own rate. `None` when the container names
/// no duration and there is nothing to count.
pub(super) fn source_frames(source: &VideoInfo) -> Option<usize> {
    source.estimated_frames(source.fps)
}

/// A length as `HH:MM:SS.mmm`, which is what the trim fields are measured
/// against and so what the source has to be stated in — `clock`'s `m:ss` is
/// too coarse to compare a typed hundredth to.
pub(super) fn timestamp(seconds: f64) -> String {
    let ms = (seconds.max(0.0) * 1000.0).round() as u64;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        ms / 3_600_000,
        (ms / 60_000) % 60,
        (ms / 1000) % 60,
        ms % 1000
    )
}

/// A frame number, as typed. Nothing clever: a frame index is an integer.
pub(super) fn parse_index(text: &str) -> Option<usize> {
    text.trim().parse().ok()
}

/// A time as typed into a trim field: `12.5`, `0:01.12`, `1:02:03`. Colons
/// climb the units, so the same parser reads seconds, mm:ss and h:mm:ss, and
/// anything else is rejected rather than guessed at.
///
/// Rounded to a whole centisecond, because that is the finest a GIF can hold:
/// frame delays are stored in hundredths, so a millisecond typed here would be
/// a promise the format cannot keep.
pub(super) fn parse_seconds(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let parts: Vec<&str> = text.split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    let mut total = 0.0f64;
    for part in parts {
        let value: f64 = part.trim().parse().ok()?;
        if !value.is_finite() || value < 0.0 {
            return None;
        }
        total = total * 60.0 + value;
    }
    Some((total * 100.0).round() / 100.0)
}

/// One file chooser for every splice, whether it was asked for from the file
/// menu or from a frame's own context menu — the only difference is `at`.
pub(super) fn pick_import_file(
    root: &impl IsA<gtk::Window>,
    at: usize,
    sender: &ComponentSender<App>,
) {
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
            sender.input(Msg::LoadInto(path, at));
        }
    });
}

/// A GIF has one canvas, so frames arriving at another size cannot all be
/// kept as they are. Which side gives way is a composition decision, not
/// something to guess at: the four answers are laid out with what each one
/// costs, and the frames wait in the closure until one is picked.
pub(super) fn fit_dialog(
    root: &adw::ApplicationWindow,
    path: &Path,
    canvas: (u32, u32),
    arriving: (u32, u32),
    at: usize,
    frames: Vec<Frame>,
    sender: &ComponentSender<App>,
) {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string());
    let dialog = adw::AlertDialog::new(
        Some(t("Sizes do not match")),
        Some(&fill(
            // Translators: {file} is a filename; {arriving} and {canvas} are sizes like "800 × 600".
            t("{file} is {arriving}, and this animation is {canvas}. Pick how they fit together."),
            &[
                ("file", &name),
                ("arriving", &size_pair(arriving)),
                ("canvas", &size_pair(canvas)),
            ],
        )),
    );
    dialog.set_content_width(520);

    let (group, buttons) = fit_options(canvas, arriving);
    dialog.set_extra_child(Some(&group));

    dialog.add_response("cancel", t("Cancel"));
    // Translators: Confirms the fit chooser and splices the frames in.
    dialog.add_response("add", t("Add frames"));
    dialog.set_response_appearance("add", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("add"));
    dialog.set_close_response("cancel");

    // The decoded frames are megabytes and the response fires once, so they
    // wait in a cell rather than being cloned into the closure.
    let waiting = RefCell::new(Some(frames));
    let sender = sender.clone();
    dialog.choose(Some(root), gio::Cancellable::NONE, move |response| {
        let Some(frames) = waiting.borrow_mut().take() else {
            return;
        };
        if response != "add" {
            return;
        }
        let picked = buttons
            .iter()
            .position(|radio| radio.is_active())
            .unwrap_or(0);
        sender.input(Msg::SpliceFrames {
            at,
            frames,
            fit: FitMode::ALL[picked],
        });
    });
}

/// One row per fit, radio-grouped, in `FitMode::ALL` order — so the index of
/// the active button is the mode that was picked. Built apart from the dialog
/// because that is the half worth testing.
pub(super) fn fit_options(
    canvas: (u32, u32),
    arriving: (u32, u32),
) -> (adw::PreferencesGroup, Vec<gtk::CheckButton>) {
    let group = adw::PreferencesGroup::new();
    let mut buttons: Vec<gtk::CheckButton> = Vec::new();
    for mode in FitMode::ALL {
        let (title, subtitle) = fit_labels(mode, canvas, arriving);
        let row = adw::ActionRow::builder()
            .title(&title)
            .subtitle(&subtitle)
            .build();
        let radio = gtk::CheckButton::new();
        radio.set_valign(gtk::Align::Center);
        match buttons.first() {
            Some(first) => radio.set_group(Some(first)),
            // The first row is the default: it changes the least — only what
            // is coming in, and only its aspect ratio.
            None => radio.set_active(true),
        }
        row.add_prefix(&radio);
        row.set_activatable_widget(Some(&radio));
        group.add(&row);
        buttons.push(radio);
    }
    (group, buttons)
}

pub(super) fn size_pair((w, h): (u32, u32)) -> String {
    format!("{w} × {h}")
}

/// What each fit does, said in terms of the two sizes in front of the user.
pub(super) fn fit_labels(
    mode: FitMode,
    canvas: (u32, u32),
    arriving: (u32, u32),
) -> (String, String) {
    let (canvas, arriving) = (size_pair(canvas), size_pair(arriving));
    match mode {
        FitMode::Stretch => (
            t("Stretch what is coming in").into(),
            fill(
                // Translators: {canvas} is a size like "480 × 320".
                t("Fills {canvas}, changing its aspect ratio."),
                &[("canvas", &canvas)],
            ),
        ),
        FitMode::Pad => (
            t("Scale what is coming in to fit").into(),
            fill(
                t("Keeps its aspect ratio inside {canvas}; transparency fills the rest."),
                &[("canvas", &canvas)],
            ),
        ),
        FitMode::GrowPad => (
            t("Resize this animation to fit").into(),
            fill(
                // Translators: {arriving} is the size of the file being added.
                t(
                    "The canvas becomes {arriving}; every existing frame keeps its aspect \
                   ratio, with transparency around it.",
                ),
                &[("arriving", &arriving)],
            ),
        ),
        FitMode::GrowStretch => (
            t("Resize this animation and stretch it").into(),
            fill(
                t("The canvas becomes {arriving}; every existing frame is stretched to it."),
                &[("arriving", &arriving)],
            ),
        ),
    }
}

/// Even values only, which is what every scaler downstream wants.
pub(super) fn size_spin(source: u32, value: u32) -> gtk::SpinButton {
    let spin = gtk::SpinButton::with_range(16.0, source.max(16) as f64, 2.0);
    spin.set_value(value as f64);
    spin.set_width_chars(4);
    spin.set_max_width_chars(4);
    spin
}

/// The live preview under the picker. Speaks in what the import lands: the
/// rate the frames will play at (a drop takes some out), the span the trim
/// leaves, the memory the frames hold, and the measured GIF size.
pub(super) fn plan_summary(plan: &ImportPlan, measured: Option<usize>) -> String {
    let played = plan.playback_fps();
    let rate = if played < plan.source.fps - 0.01 {
        fill(
            t("{fps} fps, down from {source}"),
            &[
                ("fps", &format!("{:.0}", played.max(1.0))),
                ("source", &format!("{:.0}", plan.source.fps)),
            ],
        )
    } else {
        fill(t("{fps} fps"), &[("fps", &format!("{:.0}", played))])
    };
    let Some(frames) = plan.frames() else {
        return fill(
            // Translators: {rate} is a phrase such as "4 fps, down from 60"; it opens the sentence.
            t("{rate}. The container gives no duration, so there is no count to preview."),
            &[("rate", &rate)],
        );
    };

    // Only when something was trimmed: the whole clip needs no announcing.
    let span = match (plan.start_s, plan.span_s) {
        (start, Some(span)) => format!(
            " · {}",
            fill(
                // Translators: The part of the file being imported, as two timecodes.
                t("{from} to {to}"),
                &[("from", &clock_cs(start)), ("to", &clock_cs(start + span))],
            )
        ),
        (start, None) if start > 0.0 => format!(
            " · {}",
            fill(t("from {from} on"), &[("from", &clock_cs(start))])
        ),
        _ => String::new(),
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
            "{head}{span}{memory}\n{}",
            t(
                "That is more memory than an import may use. Choose a smaller size or a \
               lower frame rate."
            )
        );
    }
    format!("{head}{span}{memory}{gif}")
}

pub(super) fn size(bytes: usize) -> String {
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

pub(super) fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// The same clock, to the centisecond, for a trim the user typed to that
/// precision — and back to whole seconds when there is no fraction to show,
/// so an untrimmed-looking span does not grow a `.00`.
pub(super) fn clock_cs(seconds: f64) -> String {
    let cs = (seconds.max(0.0) * 100.0).round() as u64;
    if cs.is_multiple_of(100) {
        return clock(cs as f64 / 100.0);
    }
    format!("{}:{:02}.{:02}", cs / 6000, (cs / 100) % 60, cs % 100)
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

pub(super) fn load(
    path: PathBuf,
    plan: Option<ImportPlan>,
    options: ImportOptions,
    cancel: Arc<AtomicBool>,
    sender: &ComponentSender<App>,
) {
    sender.spawn_command(move |out| {
        // Throttled like frame work, and checking the stop flag on every
        // frame rather than only on the ones that get reported.
        let mut throttle = Throttle::new();
        let mut progress = |done, expected| {
            if cancel.load(Ordering::Relaxed) {
                return false;
            }
            !throttle.ready() || out.send(Cmd::ImportProgress(done, expected)).is_ok()
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
