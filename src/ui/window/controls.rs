//! Small widget helpers shared by the sidebar, layer list and dialogs.

use super::*;

/// Icon name for the bundled left-rail glyph (`resources/README.md`).
pub(super) fn tool_icon_name(tool: Tool) -> &'static str {
    match tool {
        Tool::Text => "tool-text-symbolic",
        Tool::Rect => "tool-rect-symbolic",
        Tool::Ellipse => "tool-ellipse-symbolic",
        Tool::Arrow => "tool-arrow-symbolic",
    }
}

/// Marked with `n` and translated where it is shown: the same words are also
/// the overlay names stored in the document, which stay English.
pub(super) fn tool_label(tool: Tool) -> &'static str {
    match tool {
        Tool::Text => n("Text"),
        Tool::Rect => n("Rectangle"),
        Tool::Ellipse => n("Ellipse"),
        Tool::Arrow => n("Arrow"),
    }
}

pub(super) fn tool_action(tool: Tool) -> Action {
    match tool {
        Tool::Text => Action::ToolText,
        Tool::Rect => Action::ToolRect,
        Tool::Ellipse => Action::ToolEllipse,
        Tool::Arrow => Action::ToolArrow,
    }
}

pub(super) fn suffixed(title: &str, suffix: &impl IsA<gtk::Widget>) -> adw::ActionRow {
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
pub(super) fn overlay_row(
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
pub(super) fn layer_list_heights(list: &gtk::ListBox) -> (i32, i32) {
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
pub(super) fn dont_chase_focus(scroll: &gtk::ScrolledWindow) {
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
pub(super) fn set_layer_list_heights(scroll: &gtk::ScrolledWindow, keep: i32, cap: i32) {
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
pub(super) fn show_row(scroll: &gtk::ScrolledWindow, list: &gtk::ListBox, row: &gtk::ListBoxRow) {
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
pub(super) fn frame_scope_summary(scope: ScopeChoice, in_scope: &[usize]) -> String {
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

/// Zero means off, which is why the range starts there.
pub(super) fn width_spin() -> gtk::SpinButton {
    let spin = gtk::SpinButton::with_range(0.0, 40.0, 1.0);
    spin.set_valign(gtk::Align::Center);
    spin.set_width_chars(2);
    spin.set_max_width_chars(2);
    spin
}

/// The spin setter no-ops when nothing changed: assigning fires the value
/// handler, which would send the value straight back as an edit.
pub(super) fn set_spin(spin: &gtk::SpinButton, value: f64) {
    if (spin.value() - value).abs() > 0.001 {
        spin.set_value(value);
    }
}

pub(super) fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::from_icon_name(icon);
    button.set_tooltip_text(Some(tooltip));
    button
}

pub(super) fn connect(
    button: &gtk::Button,
    sender: &ComponentSender<App>,
    msg: impl Fn() -> Msg + 'static,
) {
    no_focus_steal(button);
    let sender = sender.clone();
    button.connect_clicked(move |_| sender.input(msg()));
}

/// Clicking a toolbar button must not leave it holding the keyboard focus, so
/// that Enter and the arrow keys keep meaning whatever the canvas means by
/// them. Tab still reaches the button. Space is handled by `install_shortcuts`
/// before any focused widget sees it, so this is not what makes play/pause
/// work.
pub(super) fn no_focus_steal(widget: &impl IsA<gtk::Widget>) {
    widget.as_ref().set_focus_on_click(false);
}

/// Commit whatever the user has typed into `spin` without it reaching the
/// spin's own handler, returning the new value if the typing changed it. A
/// spin holds typed text uncommitted until Enter or focus-out; the caller
/// applies the value itself, to the target the text was typed for, before
/// the model moves on and the handler would apply it somewhere else.
pub(super) fn commit_typed(spin: &gtk::SpinButton, sync: &Cell<bool>) -> Option<f64> {
    let shown = spin.value();
    sync.set(true);
    spin.update();
    sync.set(false);
    ((spin.value() - shown).abs() > 0.001).then(|| spin.value())
}

/// `set_sensitive`, but a widget being greyed out while it holds the keyboard
/// focus hands the focus back to the window first. GTK leaves the focus on an
/// insensitive widget without a focus-out, so a text field keeps its cursor
/// blinking and warns "GtkText - did not receive a focus-out event".
pub(super) fn set_sensitive_releasing_focus(widget: &impl IsA<gtk::Widget>, sensitive: bool) {
    let widget = widget.as_ref();
    if !sensitive
        && widget.state_flags().contains(gtk::StateFlags::FOCUS_WITHIN)
        && let Some(root) = widget.root()
    {
        root.set_focus(None::<&gtk::Widget>);
    }
    widget.set_sensitive(sensitive);
}
