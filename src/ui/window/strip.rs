//! The frame strip and overlay bands under it.

use super::*;

/// Ctrl+click: add or remove one frame, wherever it sits. The result need not
/// be a run, which is the whole difference from Shift.
pub(super) fn toggle_frame(selection: &mut Vec<usize>, frame: usize) {
    match selection.iter().position(|f| *f == frame) {
        Some(at) => {
            selection.remove(at);
        }
        None => selection.push(frame),
    }
    selection.sort_unstable();
}

/// Shift+click: the run between the anchor and here, in either direction.
pub(super) fn run_between(anchor: usize, frame: usize) -> Vec<usize> {
    (anchor.min(frame)..=anchor.max(frame)).collect()
}

/// The frames an overlay edit lands on: where the scope reaches into the
/// overlay's own range. When the scope does not reach the frame on screen, the
/// frame on screen wins — an edit must land where the user is looking, never
/// somewhere they cannot see. An empty result is the caller's no-op.
pub(super) fn overlay_edit_span(
    overlay: Range<usize>,
    scope: Range<usize>,
    playhead: usize,
) -> Range<usize> {
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
pub(super) fn zoomed(px: u32, zoom: f64) -> i32 {
    (px as f64 * zoom).round().max(1.0) as i32
}

/// Distance from one strip cell to the next: a zoomed thumbnail plus the
/// strip `Box`'s spacing, which is a fixed number of pixels and *not* zoomed
/// — the reason this is not `(thumb_w + THUMB_SPACING) * zoom`. The bands
/// under the strip put their per-frame columns here, so a pitch that does
/// not match what the cells measure out to slides every band sideways by a
/// growing fraction of a frame.
pub(super) fn cell_pitch(thumb_w: i32, zoom: f64) -> f64 {
    (zoomed(thumb_w.max(1) as u32, zoom) + THUMB_SPACING) as f64
}

pub(super) fn strip_width(count: usize, pitch: f64) -> i32 {
    (count as f64 * pitch) as i32
}

/// Strip thumbnail textures by frame key and drawn width. A rebuild makes
/// fresh cells for every frame, but most of the frames are the ones it
/// showed before; their textures — and below 1x their rescaled copies — are
/// reused instead of rebuilt for the whole document on every edit.
pub(super) type ThumbTextures = std::collections::HashMap<(u64, i32), gdk::Texture>;

/// The texture a strip cell shows for the frame keyed `key` at `zoom`.
/// Zooming in is a size request over the thumbnail's own texture, which a
/// `GtkPicture` scales up to whatever it is given. Zooming out has to shrink
/// the texture: a picture never measures smaller than its paintable, so
/// below 1x a request alone left every cell at its 1x width while the bands
/// under them shrank away from the frames they annotate.
pub(super) fn thumb_texture(
    cache: &mut ThumbTextures,
    key: u64,
    thumb: &image::RgbaImage,
    zoom: f64,
) -> gdk::Texture {
    let full_w = thumb.width() as i32;
    let w = if zoom < 1.0 {
        zoomed(thumb.width(), zoom)
    } else {
        full_w
    };
    cache
        .entry((key, w))
        .or_insert_with(|| {
            if w == full_w {
                return texture_from(thumb);
            }
            let h = zoomed(thumb.height(), zoom) as u32;
            let small =
                image::imageops::resize(thumb, w as u32, h, image::imageops::FilterType::Triangle);
            texture_from(&small)
        })
        .clone()
}

/// Show `texture` in a strip thumbnail sized for `zoom`; `size` is the 1x
/// thumbnail's.
pub(super) fn set_thumb_zoom(
    picture: &gtk::Picture,
    texture: &gdk::Texture,
    (w, h): (u32, u32),
    zoom: f64,
) {
    if picture.paintable().as_ref() != Some(texture.upcast_ref::<gdk::Paintable>()) {
        picture.set_paintable(Some(texture));
    }
    picture.set_size_request(zoomed(w, zoom), zoomed(h, zoom));
}

/// Membership of each frame index below `count` in `frames`, so a per-cell
/// pass over the strip asks in constant time instead of scanning a list
/// that can be every frame long.
pub(super) fn frame_mask(frames: &[usize], count: usize) -> Vec<bool> {
    let mut mask = vec![false; count];
    for &i in frames.iter().filter(|i| **i < count) {
        mask[i] = true;
    }
    mask
}

/// New timeline-strip zoom after a `Msg::StripZoom(factor)`: `factor` multiplies
/// the current zoom and the result is clamped to the bounds; `0.0` is the reset
/// sentinel back to 1x.
pub(super) fn next_strip_zoom(current: f64, factor: f64) -> f64 {
    if factor == 0.0 {
        1.0
    } else {
        (current * factor).clamp(STRIP_ZOOM_MIN, STRIP_ZOOM_MAX)
    }
}

pub(super) fn texture_from(img: &image::RgbaImage) -> gdk::Texture {
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

pub(super) fn set_class(widget: &gtk::Widget, class: &str, on: bool) {
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
pub(super) fn thumb_children(strip: &gtk::Box) -> impl Iterator<Item = gtk::Widget> + use<> {
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
pub(super) fn move_target_for_set(picked: &[usize], gap: usize) -> usize {
    gap - picked.iter().filter(|&&p| p < gap).count()
}

/// The gap "nudge the whole selection one slot earlier/later" aims at: just
/// before the unselected frame preceding it, or just after the one
/// following it — swapping the block with that neighbour. `None` when the
/// selection already sits against that edge, or names every frame and
/// leaves nothing to swap with. `picked` must be sorted.
pub(super) fn selection_nudge_gap(
    picked: &[usize],
    earlier: bool,
    frame_count: usize,
) -> Option<usize> {
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
pub(super) fn gap_at(strip: &gtk::Box, x: f64) -> usize {
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
pub(super) fn mark_drop_gap(dividers: &[gtk::Widget], gap: Option<usize>) {
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
pub(super) fn strip_keys(doc: &Document) -> Vec<(u64, bool)> {
    doc.frames.iter().map(|f| (f.key, f.detached)).collect()
}

/// A frame-menu item that shows whatever the keymap has bound to it. GTK finds
/// a menu item's accelerator through a `GtkApplication` accel table or a
/// shortcut controller in the item's own widget tree, and this app's shortcuts
/// live in neither: `install_shortcuts` reads the keymap in one capture-phase
/// key controller. So the chord is handed over as the item's "accel" attribute,
/// which GTK prefers over its own lookup, and the frame menu is refilled on
/// `Msg::SetKeymap` so a rebind moves what the menu shows.
pub(super) fn frame_item(label: &str, action: &str, keys: &Keymap, bound: Action) -> gio::MenuItem {
    let item = gio::MenuItem::new(Some(label), Some(action));
    if let Some(accel) = keys.chords(bound).first().and_then(Chord::accel) {
        item.set_attribute_value("accel", Some(&accel.to_variant()));
    }
    item
}

/// What a band context menu item can do: the action's own name under the
/// `overlay` group, and the message activating it sends. `band_menus` points
/// its items at these names, so an item cannot name an action the bands never
/// added — a menu entry that greys out or does nothing, either of which is
/// invisible from the code that wrote it.
pub(super) fn band_actions() -> [MsgAction; 3] {
    [
        ("copy-all", || Msg::CopyOverlayToFrames { all: true }),
        ("copy-selected", || Msg::CopyOverlayToFrames { all: false }),
        ("delete", || Msg::DeleteSelection),
    ]
}

/// The two faces of the band context menu, in the order the right-click
/// handler chooses between them: the copy aims at the whole document, or at
/// the frames in scope when a multi-frame selection is up. Only the copy's
/// wording and action differ; the delete is the same either way.
pub(super) fn band_menus() -> (gio::Menu, gio::Menu) {
    let faced = |label: &str, action: &str| {
        let copy = gio::Menu::new();
        copy.append(Some(label), Some(action));
        let delete = gio::Menu::new();
        delete.append(Some(t("Delete overlay")), Some("overlay.delete"));
        let menu = gio::Menu::new();
        menu.append_section(None, &copy);
        menu.append_section(None, &delete);
        menu
    };
    (
        faced(t("Copy overlay to all frames"), "overlay.copy-all"),
        faced(
            t("Copy overlay to selected frames"),
            "overlay.copy-selected",
        ),
    )
}

/// The frame menu's clipboard and list items: the header's own `win.frame-*`
/// actions, which the strip's popover reaches through its parent chain up to
/// the window, each with the keymap action whose chord it shows.
const FRAME_EDITS: [(&str, Action); 5] = [
    ("win.frame-delete", Action::FrameDelete),
    ("win.frame-duplicate", Action::FrameDuplicate),
    ("win.frame-cut", Action::FrameCut),
    ("win.frame-copy", Action::FrameCopy),
    ("win.frame-paste", Action::FramePaste),
];

/// Two faces of the one frame popover: the frame under the pointer when it
/// was opened either stands alone — the menu acts on that frame — or belongs
/// to the active selection, and then it acts on the whole scope. The
/// right-click handler picks which model to show; the actions are
/// scope-based either way, because a plain right-click outside the selection
/// seeks first, which resets the scope to that one frame. Only the wording
/// differs, plus "Move to position…", which names one frame. Both menus are
/// refilled in place, so whatever already points at them shows the new
/// chords without being handed a new model.
pub(super) fn fill_frame_menus(frame_menu: &gio::Menu, selection_menu: &gio::Menu, keys: &Keymap) {
    let fill = |menu: &gio::Menu, edits: [&str; 4], move_to: bool| {
        let edit_section = gio::Menu::new();
        for (label, (action, bound)) in edits
            .into_iter()
            .chain([t("Paste frames")])
            .zip(FRAME_EDITS)
        {
            edit_section.append_item(&frame_item(label, action, keys, bound));
        }
        edit_section.append(Some(t("Set delay…")), Some("frame.delay"));
        let move_section = gio::Menu::new();
        move_section.append(Some(t("Move earlier")), Some("frame.move-earlier"));
        move_section.append(Some(t("Move later")), Some("frame.move-later"));
        if move_to {
            move_section.append(Some(t("Move to position…")), Some("frame.move-to"));
        }
        let import_section = gio::Menu::new();
        import_section.append(Some(t("Insert frames from file…")), Some("frame.add-image"));
        menu.remove_all();
        menu.append_section(None, &edit_section);
        menu.append_section(None, &move_section);
        menu.append_section(None, &import_section);
    };
    fill(
        frame_menu,
        [
            t("Delete this frame"),
            t("Duplicate this frame"),
            t("Cut this frame"),
            t("Copy this frame"),
        ],
        true,
    );
    fill(
        selection_menu,
        [
            t("Delete selected frames"),
            t("Duplicate selected frames"),
            t("Cut selected frames"),
            t("Copy selected frames"),
        ],
        false,
    );
}

/// The strip's right-click popover, shared by every thumbnail: one per frame
/// is a widget tree per thumbnail. Built once with the window, not with the
/// strip — its `frame.*` actions read the right-clicked frame from `target`
/// and ask the model for the rest when they fire, so a frame-list change
/// leaves nothing here stale. Only a rebind does, and `show_keys` answers it.
#[derive(Clone)]
pub(super) struct FrameMenu {
    popover: gtk::PopoverMenu,
    /// The face for a frame that stands alone.
    frame_menu: gio::Menu,
    /// The face for a frame inside the active selection.
    selection_menu: gio::Menu,
    /// Which frame the last right-click landed on.
    target: Rc<Cell<usize>>,
}

impl FrameMenu {
    /// The popover, parented to `strip` alongside the `frame` actions its
    /// items name. Both faces are empty until the first `show_keys`.
    pub(super) fn new(strip: &gtk::Box, sender: &ComponentSender<App>) -> Self {
        let (frame_menu, selection_menu) = (gio::Menu::new(), gio::Menu::new());
        let popover = gtk::PopoverMenu::from_model(Some(&frame_menu));
        popover.set_parent(strip);
        popover.set_has_arrow(false);
        // A popover outlives its parent unless it is told not to.
        {
            let popover = popover.clone();
            strip.connect_destroy(move |_| popover.unparent());
        }
        let target = Rc::new(Cell::new(0usize));
        strip.insert_action_group("frame", Some(&frame_actions(strip, &target, sender)));
        FrameMenu {
            popover,
            frame_menu,
            selection_menu,
            target,
        }
    }

    /// Refill both faces with the chords `keys` binds.
    pub(super) fn show_keys(&self, keys: &Keymap) {
        fill_frame_menus(&self.frame_menu, &self.selection_menu, keys);
    }

    /// Open on `frame` at `(x, y)` in `cell`'s coordinates, with the face
    /// for a frame inside the selection or one standing alone.
    fn popup(&self, frame: usize, on_selection: bool, cell: &gtk::Widget, (x, y): (f64, f64)) {
        self.target.set(frame);
        self.popover.set_menu_model(Some(if on_selection {
            &self.selection_menu
        } else {
            &self.frame_menu
        }));
        let at = gtk::graphene::Point::new(x as f32, y as f32);
        let point = self
            .popover
            .parent()
            .and_then(|strip| cell.compute_point(&strip, &at))
            .unwrap_or(at);
        self.popover.set_pointing_to(Some(&gdk::Rectangle::new(
            point.x() as i32,
            point.y() as i32,
            1,
            1,
        )));
        self.popover.popup();
    }
}

/// The `frame` action group the popover's own items name, each acting on
/// `target` as it stands when the item is picked.
fn frame_actions(
    strip: &gtk::Box,
    target: &Rc<Cell<usize>>,
    sender: &ComponentSender<App>,
) -> gio::SimpleActionGroup {
    let group = gio::SimpleActionGroup::new();
    {
        let (sender, target) = (sender.clone(), target.clone());
        add_action(&group, "delay", move || {
            sender.input(Msg::FrameDelayDialog(target.get()));
        });
    }
    add_msg_action(&group, "move-earlier", sender, || Msg::MoveSelection {
        earlier: true,
    });
    add_msg_action(&group, "move-later", sender, || Msg::MoveSelection {
        earlier: false,
    });
    {
        let (sender, target) = (sender.clone(), target.clone());
        add_action(&group, "move-to", move || {
            sender.input(Msg::MoveFrameDialog(target.get()));
        });
    }
    {
        let (sender, target, strip) = (sender.clone(), target.clone(), strip.clone());
        add_action(&group, "add-image", move || {
            let Some(window) = strip.root().and_downcast::<gtk::Window>() else {
                return;
            };
            // Directly after the frame that was right-clicked.
            pick_import_file(&window, target.get() + 1, &sender);
        });
    }
    group
}

/// Whether frame `i` is one of several frames in `scope`: a press there is
/// aimed at the selection, not at `i` alone.
fn inside_selection(scope: &[usize], i: usize) -> bool {
    scope.len() > 1 && scope.contains(&i)
}

/// ponytail: the strip rebuilds a widget per frame when the frame list changes.
/// The thumbnails themselves are already built (see `Frame::new`), so this is a
/// hitch rather than a freeze; swap in a virtualized list when someone imports
/// something long enough to notice.
pub(super) fn rebuild_strip(
    strip: &gtk::Box,
    doc: &Document,
    sender: &ComponentSender<App>,
    scope_mirror: &Rc<RefCell<Vec<usize>>>,
    drop_dividers: &Rc<RefCell<Vec<gtk::Widget>>>,
    frame_menu: &FrameMenu,
) {
    // `thumb_children` steps past each cell before handing it out, so
    // removing it cannot cut the walk short — and the popover stays.
    for cell in thumb_children(strip) {
        strip.remove(&cell);
    }
    let mut dividers = Vec::with_capacity(doc.frames.len());
    for (i, frame) in doc.frames.iter().enumerate() {
        let (cell, divider) = thumb_cell(i, frame);
        dividers.push(divider);
        // Armed by a press inside the selection, cancelled by a drag.
        let pending_seek = Rc::new(Cell::new(None::<usize>));
        connect_thumb_click(&cell, i, sender, scope_mirror, &pending_seek);
        connect_thumb_menu(&cell, i, sender, scope_mirror, frame_menu);
        connect_thumb_drag(&cell, i, pending_seek);
        strip.append(&cell);
    }
    *drop_dividers.borrow_mut() = dividers;
}

/// One strip cell for frame `i`: its thumbnail, number and — for a frame
/// edited outside — the pencil badge, plus the drag-reorder divider laid
/// over the thumbnail, returned for `drop_dividers`. The tooltip comes from
/// `set_thumb_tooltip`, since a delay changes without a rebuild.
fn thumb_cell(i: usize, frame: &Frame) -> (gtk::Box, gtk::Widget) {
    // Empty until `update_view`, which runs `set_thumb_zoom` over the
    // whole strip right after any rebuild.
    let picture = gtk::Picture::new();
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
    (cell, divider.upcast())
}

/// The hover text for frame `i`'s cell: its number and delay.
pub(super) fn set_thumb_tooltip(cell: &gtk::Widget, i: usize, delay_cs: u16) {
    cell.set_tooltip_text(Some(&fill(
        // Translators: "cs" is centiseconds, the unit GIF stores frame delays in.
        t("Frame {number} · {delay} cs"),
        &[
            ("number", &(i + 1).to_string()),
            ("delay", &delay_cs.to_string()),
        ],
    )));
}

/// A press on a frame that is inside the active selection must not
/// collapse the selection on the spot: the click may be the start of
/// a drag that moves the whole selection, and the drop still needs
/// to find it. So the press only arms `pending_seek`; if a drag
/// begins it is dropped, and a plain release — no drag — collapses
/// the selection to that one frame, which is what a click means.
fn connect_thumb_click(
    cell: &gtk::Box,
    i: usize,
    sender: &ComponentSender<App>,
    scope_mirror: &Rc<RefCell<Vec<usize>>>,
    pending_seek: &Rc<Cell<Option<usize>>>,
) {
    let click = gtk::GestureClick::new();
    {
        let (sender, scope_mirror, pending_seek) =
            (sender.clone(), scope_mirror.clone(), pending_seek.clone());
        click.connect_pressed(move |gesture, _, _, _| {
            let state = gesture.current_event_state();
            if state.contains(gdk::ModifierType::SHIFT_MASK) {
                sender.input(Msg::ExtendSelection(i));
            } else if state.contains(gdk::ModifierType::CONTROL_MASK) {
                sender.input(Msg::ToggleSelection(i));
            } else if inside_selection(&scope_mirror.borrow(), i) {
                pending_seek.set(Some(i));
            } else {
                sender.input(Msg::Seek(i));
            }
        });
    }
    {
        let (sender, pending_seek) = (sender.clone(), pending_seek.clone());
        click.connect_released(move |_, _, _, _| {
            if let Some(i) = pending_seek.take() {
                sender.input(Msg::Seek(i));
            }
        });
    }
    cell.add_controller(click);
}

/// Right-click acts on the frame under the pointer, not on the scope,
/// which is the whole reason to have it as well as the ⋮ menu — but
/// only when that frame stands alone. Right-clicking a frame already
/// inside the active selection keeps the selection and switches the
/// popover to its scope-acting menu.
fn connect_thumb_menu(
    cell: &gtk::Box,
    i: usize,
    sender: &ComponentSender<App>,
    scope_mirror: &Rc<RefCell<Vec<usize>>>,
    frame_menu: &FrameMenu,
) {
    let secondary = gtk::GestureClick::new();
    secondary.set_button(gdk::BUTTON_SECONDARY);
    let (sender, scope_mirror, frame_menu) =
        (sender.clone(), scope_mirror.clone(), frame_menu.clone());
    let cell_widget = cell.clone().upcast::<gtk::Widget>();
    secondary.connect_pressed(move |_, _, x, y| {
        let on_selection = inside_selection(&scope_mirror.borrow(), i);
        sender.input(if on_selection {
            Msg::SeekKeepSelection(i)
        } else {
            Msg::Seek(i)
        });
        frame_menu.popup(i, on_selection, &cell_widget, (x, y));
    });
    cell.add_controller(secondary);
}

/// Drag one thumbnail to reorder it; the payload is just the source
/// index. Where it lands is decided by the strip's own drop target
/// (added once, in `build`), which tracks the gap nearest the pointer
/// across the whole strip rather than per cell.
fn connect_thumb_drag(cell: &gtk::Box, i: usize, pending_seek: Rc<Cell<Option<usize>>>) {
    let drag_source = gtk::DragSource::new();
    drag_source.set_actions(gdk::DragAction::MOVE);
    drag_source.connect_drag_begin(move |_, _| {
        // The press became a drag: the drop moves whatever the
        // selection then names, so it must still be whole here —
        // cancel the collapse-on-release instead.
        pending_seek.take();
    });
    let from = i as u32;
    drag_source
        .connect_prepare(move |_, _, _| Some(gdk::ContentProvider::for_value(&from.to_value())));
    cell.add_controller(drag_source);
}

/// One row per overlay, each band spanning the frames its range covers. This is
/// the layer list; a second one in the sidebar would list every object twice.
/// The overlay whose packed row and frame span contain the click. `bands` is
/// not one entry per visual row — `pack_rows` puts non-overlapping overlays
/// side by side on the same row — so this searches rather than indexing.
pub(super) fn band_at(bands: &[Band], x: f64, y: f64, pitch: f64) -> Option<OverlayId> {
    let row = (y / BAND_H) as usize;
    let frame = (x / pitch) as usize;
    bands
        .iter()
        .find(|band| band.row == row && band.range.contains(&frame))
        .map(|band| band.id)
}

pub(super) fn draw_bands(area: &gtk::DrawingArea, cr: &cairo::Context, bands: &[Band], pitch: f64) {
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

/// Pack overlay bands into rows: each one takes the lowest row where nothing
/// already sits over the frames it covers. Without this an overlay's row was
/// its index in the list, so the tenth overlay drew ten rows down even when the
/// nine below it were nowhere near its frames.
pub(super) fn pack_rows(ranges: &[Range<usize>]) -> Vec<usize> {
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
