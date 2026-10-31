//! Modal dialogs: about, optimize, resize, crop, delay and move-frame.

use super::*;

/// Where the About dialog points. Filing an issue is the route that gets
/// tracked and answered; the form is for someone who has a minute's worth of
/// feedback and no GitHub account, so it sits under the issue link rather than
/// competing with it.
pub(super) const REPOSITORY_URL: &str = "https://github.com/zbcoding/gifkino";
pub(super) const ISSUES_URL: &str = "https://github.com/zbcoding/gifkino/issues";
pub(super) const FEEDBACK_FORM_URL: &str = "https://tally.so/r/A7kKVz";

/// A row that opens a URL in the browser. The subtitle carries what the link
/// costs the user — an account, a minute - so the choice is made before the
/// browser opens, not after.
pub(super) fn link_row(title: &str, subtitle: &str, url: &'static str) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(title)
        .subtitle(subtitle)
        .activatable(true)
        .build();
    let arrow = gtk::Image::from_icon_name("adw-external-link-symbolic");
    arrow.add_css_class("dim-label");
    row.add_suffix(&arrow);
    row.connect_activated(move |row| {
        let parent = row.root().and_downcast::<gtk::Window>();
        gtk::UriLauncher::new(url).launch(
            parent.as_ref(),
            gio::Cancellable::NONE,
            |_res: Result<(), glib::Error>| {},
        );
    });
    row
}

/// A row that only states a fact: the version, or what a bundled program is
/// licensed under.
pub(super) fn fact_row(title: &str, value: &str) -> adw::ActionRow {
    adw::ActionRow::builder()
        .title(title)
        .subtitle(value)
        .css_classes(["property"])
        .build()
}

/// The About dialog: Details, Feedback, Licenses, in that order, because that
/// is the order someone reads them in — what this is, how to say something
/// about it, what it is made of.
///
/// `AdwAboutDialog` is not used: its pages are fixed (Details, Report an
/// Issue, Legal), it buries extra links one level down under Details, and it
/// cannot put the issue tracker and the form together under one heading.
///
/// The version is the crate's, so a release bumps `Cargo.toml` and nothing
/// here. The licences name what a download carries besides Gifkino's own MIT
/// code: the GTK stack it links against and the two programs it drives as
/// subprocesses, which are the copyleft ones. Statically linked crates are
/// not listed — a licence name in a dialog is not attribution, and their
/// texts belong in the bundled third-party notice file.
pub(super) fn about_dialog() -> adw::PreferencesDialog {
    let dialog = adw::PreferencesDialog::builder()
        .title(t("About"))
        .content_width(420)
        .build();
    let page = adw::PreferencesPage::new();

    let heading = gtk::Box::new(gtk::Orientation::Vertical, 6);
    heading.set_margin_bottom(6);
    let icon = gtk::Image::from_icon_name(crate::desktop::APP_ID);
    icon.set_pixel_size(96);
    icon.set_accessible_role(gtk::AccessibleRole::Presentation);
    let name = gtk::Label::new(Some("Gifkino"));
    name.add_css_class("title-1");
    let by = gtk::Label::new(Some("zbcoding"));
    by.add_css_class("dim-label");
    heading.append(&icon);
    heading.append(&name);
    heading.append(&by);
    let banner = adw::PreferencesGroup::new();
    banner.add(&heading);
    page.add(&banner);

    let details = adw::PreferencesGroup::builder().title(t("Details")).build();
    details.add(&fact_row(t("Version"), env!("CARGO_PKG_VERSION")));
    details.add(&link_row(
        t("Repository"),
        "github.com/zbcoding/gifkino",
        REPOSITORY_URL,
    ));
    page.add(&details);

    let feedback = adw::PreferencesGroup::builder()
        .title(t("Feedback"))
        .build();
    feedback.add(&link_row(
        t("Report an issue on GitHub"),
        // Translators: Why this is the first of the two feedback rows.
        t("Tracked and answered. Needs a GitHub account."),
        ISSUES_URL,
    ));
    feedback.add(&link_row(
        t("Quick feedback form"),
        // Translators: The second feedback row, for a user with no account.
        t("About a minute, no account needed."),
        FEEDBACK_FORM_URL,
    ));
    page.add(&feedback);

    let licenses = adw::PreferencesGroup::builder()
        .title(t("Licenses"))
        .description("© 2026 zbcoding")
        .build();
    licenses.add(&fact_row("Gifkino", "MIT"));
    licenses.add(&fact_row("GTK · libadwaita", "LGPL-2.1-or-later"));
    // Ubuntu builds ffmpeg with --enable-gpl and the flatpak does not; either
    // way it is a separate program, not linked in.
    licenses.add(&fact_row(
        "FFmpeg",
        "LGPL-2.1-or-later (GPL-2.0-or-later in GPL-enabled builds)",
    ));
    licenses.add(&fact_row("Gifsicle", "GPL-2.0-or-later"));
    page.add(&licenses);

    dialog.add(&page);
    dialog
}

#[derive(Clone, Copy, Debug)]
pub(super) enum OptimizeDialog {
    Remove,
    Smart,
}

/// The optimize menu's two frame-drop dialogs. One function because they are
/// the same dialog with a different spin button in it.
pub(super) fn optimize_dialog(
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
    };
    let dialog = adw::AlertDialog::new(Some(title), Some(body));
    dialog.set_content_width(440);

    let group = adw::PreferencesGroup::new();
    let spin = match which {
        OptimizeDialog::Remove => gtk::SpinButton::with_range(2.0, 60.0, 1.0),
        OptimizeDialog::Smart => gtk::SpinButton::with_range(5.0, 90.0, 5.0),
    };
    spin.set_valign(gtk::Align::Center);
    spin.set_value(match which {
        OptimizeDialog::Remove => 2.0,
        OptimizeDialog::Smart => 30.0,
    });
    let row = adw::ActionRow::builder()
        .title(match which {
            OptimizeDialog::Remove => t("Remove 1 frame in every"),
            OptimizeDialog::Smart => t("Remove this share of frames"),
        })
        .build();
    let suffix = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    suffix.set_valign(gtk::Align::Center);
    suffix.append(&spin);
    match which {
        // Translators: Unit after the "remove 1 frame in every N" field.
        OptimizeDialog::Remove => suffix.append(&gtk::Label::new(Some(t("frames")))),
        OptimizeDialog::Smart => suffix.append(&gtk::Label::new(Some("%"))),
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
        });
    });
}

/// The paired dimension that keeps the `edited_other`:`fixed_other` ratio:
/// `edited * fixed_other / edited_other`, rounded to a pixel and clamped to
/// the range a size field accepts. Pure so the linking has a unit test
/// without a display.
pub(super) fn paired_dimension(edited: u32, edited_other: u32, fixed_other: u32) -> u32 {
    if edited_other == 0 {
        return edited.clamp(16, 8192);
    }
    ((edited as f64 * fixed_other as f64 / edited_other as f64).round() as u32).clamp(16, 8192)
}

/// Writes the paired dimension into `peer`, fitted to what `peer` itself will
/// take: inside its own range, and on its own step — the import fields step
/// by two, so the number shown is the even one the decode will use.
pub(super) fn set_paired_dimension(
    peer: &gtk::SpinButton,
    edited: u32,
    edited_other: u32,
    fixed_other: u32,
) {
    let paired = paired_dimension(edited, edited_other, fixed_other) as f64;
    let adjustment = peer.adjustment();
    let (lower, upper) = (adjustment.lower(), adjustment.upper());
    let step = adjustment.step_increment().max(1.0);
    let snapped = lower + ((paired - lower) / step).round() * step;
    peer.set_value(snapped.clamp(lower, upper));
}

/// What a size field is asking for right now: its text while a keystroke is
/// still uncommitted, its committed value otherwise, clamped to the range the
/// field offers so a typed absurdity never reaches a plan.
pub(super) fn requested_dimension(spin: &gtk::SpinButton) -> u32 {
    let adjustment = spin.adjustment();
    match spin.text().parse::<f64>() {
        Ok(value) if value.is_finite() && value >= 0.0 => {
            value.clamp(adjustment.lower(), adjustment.upper()) as u32
        }
        _ => spin.value_as_int().max(0) as u32,
    }
}

/// Resize every frame, from the Optimize menu. Seeded from the live canvas so
/// opening it on an untouched document offers a no-op, a "Keep aspect ratio"
/// switch links the two fields live, and a memory estimate warns before a
/// huge canvas on a long document becomes a stuck worker. `Document::resize`
/// clamps whatever comes back. Built by `build_resize_dialog` so the widget
/// test can hold it without showing it.
/// Checked RGBA bytes a resize to `w × h` would need for `frames` frames, or
/// `None` when the multiplication overflows — the same arithmetic
/// `resize_fits_budget` gates Apply on.
pub(super) fn resize_bytes(w: u32, h: u32, frames: usize) -> Option<usize> {
    (w as usize)
        .checked_mul(h as usize)?
        .checked_mul(4)?
        .checked_mul(frames)
}

/// Refreshes the resize dialog's memory summary and whether Apply may run. A
/// big canvas on a long document dwarfs the GIF itself, so the cost shows
/// before the worker starts rather than as a toast after it refuses.
pub(super) fn refresh_resize_summary(
    dialog: &adw::AlertDialog,
    summary: &gtk::Label,
    width: &gtk::SpinButton,
    height: &gtk::SpinButton,
    cw: u32,
    ch: u32,
    frames: usize,
    oper_max: usize,
    total_max: usize,
) {
    let (w, h) = (width.value() as u32, height.value() as u32);
    let fits = resize_fits_budgets(cw, ch, w, h, frames, oper_max, total_max);
    let mut text = match resize_bytes(w, h, frames) {
        Some(bytes) => fill(t("{size} in memory"), &[("size", &size(bytes))]),
        None => String::new(),
    };
    if !fits {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(t(
            "That resize would use more memory than the configured limit.",
        ));
    }
    summary.set_label(&text);
    dialog.set_response_enabled("apply", fits);
}

/// Reads the entry text so keystrokes recompute the peer before the spin
/// commits them. Reports whether the text was a number; a cleared field
/// mid-edit links nothing.
pub(super) fn relink_from_text(
    edited: &gtk::SpinButton,
    peer: &gtk::SpinButton,
    edited_other: u32,
    fixed_other: u32,
) -> bool {
    let Ok(value) = edited.text().parse::<f64>() else {
        return false;
    };
    if !value.is_finite() || value < 0.0 {
        return false;
    }
    set_paired_dimension(peer, value as u32, edited_other, fixed_other);
    true
}

/// Links a dialog's width and height fields to its aspect lock: editing one
/// recomputes the other from the `cw`:`ch` ratio the dialog opened at. Both
/// commit paths funnel through one guard — the entry's `changed` for
/// keystrokes the spin has not committed yet, `value-changed` for arrows,
/// Enter and focus-out — so the peer tracks live typing instead of only lock
/// toggles, and a programmatic set never echoes back. The returned setter
/// writes both fields at once, for a caller that already holds a matched pair:
/// the import picker's resolution presets, whose own rounding the link would
/// otherwise undo.
pub(super) fn link_aspect_fields(
    width: &gtk::SpinButton,
    height: &gtk::SpinButton,
    lock: &adw::SwitchRow,
    cw: u32,
    ch: u32,
) -> Rc<dyn Fn(u32, u32)> {
    // A programmatic set fires its own handler, so guard the echo.
    let syncing = Rc::new(Cell::new(false));
    for (edited, peer, edited_other, fixed_other) in
        [(width, height, cw, ch), (height, width, ch, cw)]
    {
        let (edited, peer, syncing) = (edited.clone(), peer.clone(), syncing.clone());
        let lock = lock.clone();
        let (e, p) = (edited.clone(), peer.clone());
        let relink = Rc::new(move || {
            if !lock.is_active() || syncing.get() {
                return;
            }
            syncing.set(true);
            relink_from_text(&e, &p, edited_other, fixed_other);
            syncing.set(false);
        });
        let commit = relink.clone();
        edited.connect_changed(move |_| relink());
        edited.connect_value_changed(move |_| commit());
    }
    {
        let (width, height, syncing) = (width.clone(), height.clone(), syncing.clone());
        lock.connect_active_notify(move |lock| {
            if !lock.is_active() {
                return;
            }
            // Re-locking re-anchors the ratio at the field last touched: the
            // height follows the width back onto the ratio.
            syncing.set(true);
            set_paired_dimension(&height, width.value() as u32, cw, ch);
            syncing.set(false);
        });
    }

    let (width, height) = (width.clone(), height.clone());
    Rc::new(move |w, h| {
        syncing.set(true);
        width.set_value(w as f64);
        height.set_value(h as f64);
        syncing.set(false);
    })
}

/// Builds the resize dialog and its live widgets: the size fields seeded from
/// the canvas, the aspect lock, the memory summary, and the Cancel/Apply
/// responses. Split out so the widget test can hold the dialog without
/// showing it.
pub(super) fn build_resize_dialog(
    cw: u32,
    ch: u32,
    frames: usize,
    oper_max: usize,
    total_max: usize,
) -> (
    adw::AlertDialog,
    gtk::SpinButton,
    gtk::SpinButton,
    adw::SwitchRow,
    gtk::Label,
) {
    let dialog = adw::AlertDialog::new(
        Some(t("Resize")),
        Some(t("Scale every frame, and the overlays with them.")),
    );
    dialog.set_content_width(440);

    let width = gtk::SpinButton::with_range(16.0, 8192.0, 1.0);
    width.set_valign(gtk::Align::Center);
    width.set_value(cw as f64);
    let height = gtk::SpinButton::with_range(16.0, 8192.0, 1.0);
    height.set_valign(gtk::Align::Center);
    height.set_value(ch as f64);
    let row = adw::ActionRow::builder().title(t("New size")).build();
    let spins = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    spins.set_valign(gtk::Align::Center);
    spins.append(&width);
    spins.append(&gtk::Label::new(Some("×")));
    spins.append(&height);
    // Translators: Abbreviation for pixels, shown after a pair of size fields.
    spins.append(&gtk::Label::new(Some(t("px"))));
    row.add_suffix(&spins);

    let lock = adw::SwitchRow::builder()
        .title(t("Keep aspect ratio"))
        .build();
    lock.set_active(true);

    let group = adw::PreferencesGroup::new();
    group.add(&row);
    group.add(&lock);
    let summary = gtk::Label::new(None);
    summary.add_css_class("dim-label");
    summary.set_xalign(0.0);
    summary.set_wrap(true);
    summary.set_margin_top(6);
    let form = gtk::Box::new(gtk::Orientation::Vertical, 0);
    form.append(&group);
    form.append(&summary);
    dialog.set_extra_child(Some(&form));

    dialog.add_response("cancel", t("Cancel"));
    dialog.add_response("apply", t("Apply"));
    dialog.set_response_appearance("apply", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("apply"));
    dialog.set_close_response("cancel");
    refresh_resize_summary(
        &dialog, &summary, &width, &height, cw, ch, frames, oper_max, total_max,
    );
    (dialog, width, height, lock, summary)
}

/// Shows the resize dialog: links the fields, keeps the memory summary live,
/// and sends `Msg::Resize` for the size Apply leaves behind.
pub(super) fn resize_dialog(
    root: &adw::ApplicationWindow,
    cw: u32,
    ch: u32,
    frames: usize,
    oper_max: usize,
    total_max: usize,
    sender: &ComponentSender<App>,
) {
    let (dialog, width, height, lock, summary) =
        build_resize_dialog(cw, ch, frames, oper_max, total_max);
    link_aspect_fields(&width, &height, &lock, cw, ch);
    for spin in [&width, &height] {
        let (dialog, summary, width, height) = (
            dialog.clone(),
            summary.clone(),
            width.clone(),
            height.clone(),
        );
        spin.connect_value_changed(move |_| {
            refresh_resize_summary(
                &dialog, &summary, &width, &height, cw, ch, frames, oper_max, total_max,
            );
        });
    }

    let sender = sender.clone();
    dialog.choose(Some(root), gio::Cancellable::NONE, move |response| {
        if response != "apply" {
            return;
        }
        sender.input(Msg::Resize(width.value() as u32, height.value() as u32));
    });
}

/// Crop every frame, from the Optimize menu. Four fields, in image pixels;
/// `Document::crop` clamps whatever comes back. The canvas size is live when
/// the dialog opens, which is why the model opens it and the action does not.
pub(super) fn crop_dialog(
    root: &adw::ApplicationWindow,
    cw: u32,
    ch: u32,
    sender: &ComponentSender<App>,
) {
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
pub(super) fn delay_scope_dialog(
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
pub(super) fn delay_all_dialog(
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
pub(super) fn move_frame_dialog(
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
