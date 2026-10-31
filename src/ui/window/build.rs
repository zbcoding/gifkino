//! Builds the window's widget tree and wires its signals.

use super::*;

pub(super) fn build(
    root: &adw::ApplicationWindow,
    model: &App,
    sender: &ComponentSender<App>,
) -> Widgets {
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

    let file = gio::Menu::new();
    file.append(Some(t("Open…")), Some("win.open"));
    file.append(Some(t("Export GIF…")), Some("win.export"));
    // Translators: Decodes another video or GIF and appends its frames, so two
    // clips can be mixed into one timeline.
    file.append(Some(t("Add frames from file…")), Some("win.import-more"));
    let settings_menu = gio::Menu::new();
    settings_menu.append(Some(t("Keyboard shortcuts…")), Some("win.shortcuts"));
    settings_menu.append(Some(t("About")), Some("win.about"));

    // Every editing command sits in one row of menus in the header. A narrow
    // window has no room for that row, so the breakpoint below hides it and
    // the ☰ menu carries the same menus as submenus instead.
    let menubar_model = gio::Menu::new();
    let menu = gio::Menu::new();
    let narrow_menu = gio::Menu::new();
    menu.append_section(None, &file);
    narrow_menu.append_section(None, &file);
    let narrow_edit = gio::Menu::new();
    for (label, submenu) in edit_menus() {
        menubar_model.append_submenu(Some(label), &submenu);
        narrow_edit.append_submenu(Some(label), &submenu);
    }
    narrow_menu.append_section(None, &narrow_edit);
    menu.append_section(None, &settings_menu);
    narrow_menu.append_section(None, &settings_menu);
    let menubar = gtk::PopoverMenuBar::from_model(Some(&menubar_model));
    header.pack_start(&menubar);
    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu)
        .build();
    no_focus_steal(&menu_button);
    header.pack_end(&menu_button);

    // Empty state doubles as the welcome screen.
    let status = adw::StatusPage::builder()
        .icon_name("image-x-generic-symbolic")
        .title(t("Open Image"))
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
    let import_cancel = gtk::Button::from_icon_name("window-close-symbolic");
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
    {
        let button = gtk::Button::from_icon_name(tool_icon_name(Tool::Text));
        button.add_css_class("flat");
        connect(&button, sender, || Msg::AddOverlay(Tool::Text));
        rail.append(&button);
        tool_buttons.push((Tool::Text, button));
    }
    // Rect, Ellipse and Arrow share one button: the icon and the click both
    // follow whichever of the three was used last (Impasto's `ToolBoxWidget`
    // groups related tools the same way), and the dropdown arrow opens a
    // flyout to pick a different one.
    let shape_tool = Rc::new(Cell::new(Tool::Rect));
    let shape_button = adw::SplitButton::builder()
        .icon_name(tool_icon_name(shape_tool.get()))
        .build();
    shape_button.add_css_class("flat");
    no_focus_steal(&shape_button);
    {
        let (sender, shape_tool) = (sender.clone(), shape_tool.clone());
        shape_button.connect_clicked(move |_| sender.input(Msg::AddOverlay(shape_tool.get())));
    }
    let shape_popover = gtk::Popover::new();
    let shape_list = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    for tool in [Tool::Rect, Tool::Ellipse, Tool::Arrow] {
        let item = gtk::Button::from_icon_name(tool_icon_name(tool));
        item.add_css_class("flat");
        item.set_tooltip_text(Some(t(tool_label(tool))));
        no_focus_steal(&item);
        let (sender, shape_tool, shape_button, shape_popover) = (
            sender.clone(),
            shape_tool.clone(),
            shape_button.clone(),
            shape_popover.clone(),
        );
        item.connect_clicked(move |_| {
            shape_tool.set(tool);
            shape_button.set_icon_name(tool_icon_name(tool));
            sender.input(Msg::AddOverlay(tool));
            shape_popover.popdown();
        });
        shape_list.append(&item);
    }
    shape_popover.set_child(Some(&shape_list));
    shape_button.set_popover(Some(&shape_popover));
    rail.append(&shape_button);
    let crop_button = gtk::ToggleButton::builder()
        .icon_name("tool-crop-symbolic")
        .build();
    crop_button.add_css_class("flat");
    no_focus_steal(&crop_button);
    {
        let sender = sender.clone();
        crop_button.connect_clicked(move |_| sender.input(Msg::ToggleCropTool));
    }
    rail.append(&crop_button);
    // A setting rather than a tool, so it sits apart from the tools above it.
    rail.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    let snap_button = gtk::ToggleButton::builder()
        .icon_name("view-grid-symbolic")
        .active(model.snap)
        .build();
    snap_button.add_css_class("flat");
    no_focus_steal(&snap_button);
    {
        let sender = sender.clone();
        snap_button.connect_clicked(move |_| sender.input(Msg::ToggleSnap));
    }
    rail.append(&snap_button);

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
    footer.append(&transport);

    let strip = gtk::Box::new(gtk::Orientation::Horizontal, THUMB_SPACING);
    let scope_mirror: Rc<RefCell<Vec<usize>>> = Rc::new(RefCell::new(Vec::new()));
    let drop_dividers: Rc<RefCell<Vec<gtk::Widget>>> = Rc::new(RefCell::new(Vec::new()));
    // One drop target for the whole strip, not one per cell: it tracks the
    // gap nearest the pointer as it moves and paints the divider there, so a
    // release lands exactly where the line promised rather than leaving the
    // player to guess between "before" and "after" the cell under it.
    let drop_target = gtk::DropTarget::new(u32::static_type(), gdk::DragAction::MOVE);
    {
        let (strip, drop_dividers) = (strip.clone(), drop_dividers.clone());
        drop_target.connect_motion(move |_, x, _y| {
            mark_drop_gap(&drop_dividers.borrow(), Some(gap_at(&strip, x)));
            gdk::DragAction::MOVE
        });
    }
    {
        let drop_dividers = drop_dividers.clone();
        drop_target.connect_leave(move |_| mark_drop_gap(&drop_dividers.borrow(), None));
    }
    {
        let (sender, strip, drop_dividers) = (sender.clone(), strip.clone(), drop_dividers.clone());
        drop_target.connect_drop(move |_, value, x, _y| {
            let Ok(from) = value.get::<u32>() else {
                return false;
            };
            let gap = gap_at(&strip, x);
            mark_drop_gap(&drop_dividers.borrow(), None);
            sender.input(Msg::MoveSelectionTo {
                from: from as usize,
                gap,
            });
            true
        });
    }
    strip.add_controller(drop_target);

    let bands_model: Rc<RefCell<Vec<Band>>> = Rc::new(std::cell::RefCell::new(Vec::new()));
    let bands = gtk::DrawingArea::new();
    bands.set_content_height(0);
    {
        let bands_model = bands_model.clone();
        let strip_pitch = model.strip_pitch.clone();
        bands.set_draw_func(move |area, cr, _, _| {
            draw_bands(area, cr, &bands_model.borrow(), strip_pitch.get());
        });
    }
    let band_click = gtk::GestureClick::new();
    band_click.set_button(gdk::BUTTON_PRIMARY);
    {
        let bands_model = bands_model.clone();
        let sender = sender.clone();
        let strip_pitch = model.strip_pitch.clone();
        band_click.connect_pressed(move |_, _, x, y| {
            let hit = band_at(&bands_model.borrow(), x, y, strip_pitch.get());
            sender.input(Msg::SelectBand(hit));
        });
    }
    bands.add_controller(band_click);

    // Right-click a band to act on that overlay: in the strip the band *is*
    // the overlay, so deleting it there beats a trip through the sidebar.
    // Picking it first means the menu always names what is under the pointer.
    let (band_menu, band_selection_menu) = band_menus();
    let band_popover = gtk::PopoverMenu::from_model(Some(&band_menu));
    band_popover.set_parent(&bands);
    band_popover.set_has_arrow(false);
    {
        let band_popover = band_popover.clone();
        bands.connect_destroy(move |_| band_popover.unparent());
    }
    {
        let group = gio::SimpleActionGroup::new();
        for (name, msg) in band_actions() {
            let action = gio::SimpleAction::new(name, None);
            let sender = sender.clone();
            action.connect_activate(move |_, _| sender.input(msg()));
            group.add_action(&action);
        }
        bands.insert_action_group("overlay", Some(&group));
    }
    let band_secondary = gtk::GestureClick::new();
    band_secondary.set_button(gdk::BUTTON_SECONDARY);
    {
        let bands_model = bands_model.clone();
        let sender = sender.clone();
        let strip_pitch = model.strip_pitch.clone();
        let band_popover = band_popover.clone();
        let (band_menu, band_selection_menu) = (band_menu.clone(), band_selection_menu.clone());
        let scope_mirror = scope_mirror.clone();
        band_secondary.connect_pressed(move |_, _, x, y| {
            // Empty space under the pointer: no menu at all, rather than one
            // that would delete whatever happened to be selected elsewhere.
            let Some(id) = band_at(&bands_model.borrow(), x, y, strip_pitch.get()) else {
                return;
            };
            sender.input(Msg::SelectOverlay(Some(id)));
            // More than one frame in scope means the copy has a selection to
            // aim at; a lone frame leaves the whole document as the only
            // useful target. Same rule the strip's frame menu switches on.
            let selection_up = scope_mirror.borrow().len() > 1;
            band_popover.set_menu_model(Some(if selection_up {
                &band_selection_menu
            } else {
                &band_menu
            }));
            band_popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            band_popover.popup();
        });
    }
    bands.add_controller(band_secondary);

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
        .hscrollbar_policy(gtk::PolicyType::External)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .child(&strip_column)
        .build();
    // The footer asks for what the strip actually needs, so this has to carry
    // the strip's own height out rather than reporting a scrollable minimum.
    strip_scroll.set_propagate_natural_height(true);
    dont_chase_focus(&strip_scroll);
    // The horizontal scrollbar is a sibling rather than the scrolled window's
    // own. A non-overlay `Automatic` scrollbar takes its height out of the
    // viewport without adding it to what the scrolled window measures, so
    // wherever the footer sat at its minimum height — which is where a
    // `GtkPaned` end child lands — the strip lost exactly the row of frame
    // numbers under the thumbnails, with no scrollbar to reach it by. Out
    // here it is measured along with everything else, and it only takes
    // space while the strip really is wider than the window.
    let strip_bar = gtk::Scrollbar::new(
        gtk::Orientation::Horizontal,
        Some(&strip_scroll.hadjustment()),
    );
    strip_bar.set_visible(false);
    {
        let bar = strip_bar.clone();
        strip_scroll.hadjustment().connect_changed(move |adj| {
            bar.set_visible(adj.upper() > adj.page_size() + 1.0);
        });
    }

    // Ctrl+wheel over the strip zooms the thumbnails; a plain wheel still
    // scrolls it. Capture phase so it beats the scrolled window's own handler.
    let strip_wheel = gtk::EventControllerScroll::new(
        gtk::EventControllerScrollFlags::VERTICAL | gtk::EventControllerScrollFlags::DISCRETE,
    );
    strip_wheel.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let sender = sender.clone();
        strip_wheel.connect_scroll(move |controller, _, dy| {
            if !controller
                .current_event_state()
                .contains(gdk::ModifierType::CONTROL_MASK)
                || dy == 0.0
            {
                return glib::Propagation::Proceed;
            }
            let factor = if dy < 0.0 {
                STRIP_ZOOM_STEP
            } else {
                1.0 / STRIP_ZOOM_STEP
            };
            sender.input(Msg::StripZoom(factor));
            glib::Propagation::Stop
        });
    }
    strip_scroll.add_controller(strip_wheel);
    footer.append(&strip_scroll);
    footer.append(&strip_bar);

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
    let text_group = adw::PreferencesGroup::builder()
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
    text_group.add(&text_row);

    // Overlay styling. Font choice covers bold and italic, because a font
    // description already carries weight and style: two toggles that fight the
    // font dialog over the same field would be the bug, not the feature.
    let overlay_group = adw::PreferencesGroup::builder()
        .title(t("Overlay"))
        .visible(false)
        .build();
    // Deleting the overlay belongs with the controls that edit it — the same
    // one the band menu in the strip acts on. Destructive, so it takes the
    // accent-free red styling rather than sitting in the row list where a
    // stray click while changing a colour would land on it.
    let overlay_delete = gtk::Button::from_icon_name("user-trash-symbolic");
    overlay_delete.set_valign(gtk::Align::Center);
    overlay_delete.add_css_class("flat");
    overlay_delete.add_css_class("destructive-action");
    overlay_delete.set_tooltip_text(Some(t("Delete overlay")));
    connect(&overlay_delete, sender, || Msg::DeleteSelection);
    overlay_group.set_header_suffix(Some(&overlay_delete));
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

    let fill_button = ColorPick::new();
    let fill_on = gtk::Switch::new();
    fill_on.set_valign(gtk::Align::Center);
    let fill_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    fill_box.set_valign(gtk::Align::Center);
    fill_box.append(&fill_on);
    fill_box.append(fill_button.widget());
    // Translators: The interior colour of a shape, or the colour of the glyphs for text.
    let fill_row = suffixed(t("Fill"), &fill_box);

    let outline_button = ColorPick::new();
    let outline_width = width_spin();
    let outline_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    outline_box.set_valign(gtk::Align::Center);
    outline_box.append(&outline_width);
    outline_box.append(outline_button.widget());
    // Translators: The contrasting edge drawn behind text so it stays readable over any image.
    let outline_row = suffixed(t("Outline"), &outline_box);

    let stroke_button = ColorPick::new();
    let stroke_width = width_spin();
    let stroke_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    stroke_box.set_valign(gtk::Align::Center);
    stroke_box.append(&stroke_width);
    stroke_box.append(stroke_button.widget());
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
        // No `sync` guard: unlike a GTK colour button, the picker reports the
        // user's choices and stays quiet when the model writes to it.
        let (sender, switch) = (sender.clone(), fill_on.clone());
        fill_button.connect_changed(move |colour| {
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
            let colour = button.rgba();
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
    let crop_buttons = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let crop_apply = gtk::Button::with_label(t("Crop all frames"));
    crop_apply.set_tooltip_text(Some(t(
        "Crops every frame in the document to this box, shrinking the canvas.",
    )));
    // Translators: Zoom follows the frame scope — the frame on screen, or the
    // selected frames when there is a selection. "Zoom and resize all frames"
    // in the Optimize menu ignores the scope and takes the whole document.
    let zoom_apply = gtk::Button::with_label(t("Zoom and resize"));
    zoom_apply.set_tooltip_text(Some(t(
        "Fills the canvas from this box, on the frame on screen or the selected frames.",
    )));
    // Translators: Crops the frame(s) in scope in place, leaving the rest of
    // each one transparent instead of scaling the kept region back up.
    let shrink_apply = gtk::Button::with_label(t("Crop and keep size"));
    shrink_apply.set_tooltip_text(Some(t(
        "Crops this box on every frame in scope, in place, without scaling it \
         up, and blanks the rest of each frame to transparent.",
    )));
    connect(&crop_apply, sender, || Msg::ApplyCrop);
    connect(&zoom_apply, sender, || Msg::ApplyZoom);
    connect(&shrink_apply, sender, || Msg::ApplyShrink);
    crop_buttons.append(&crop_apply);
    crop_buttons.append(&zoom_apply);
    crop_buttons.append(&shrink_apply);
    let crop_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    crop_box.append(&crop_label);
    crop_box.append(&crop_buttons);
    crop_group.add(&crop_box);

    // Frame view: the delay of the frame(s) in scope, then a picker for the
    // overlays sitting on the frame on screen. `overlay_group` below edits
    // whichever overlay is picked. `update_view` swaps this group's title and
    // the delay row's subtitle to a "N frames selected" summary, and hides
    // the overlay picker below whenever the scope names more than one frame:
    // which overlays sit on "the" frame has no single answer then. A picked
    // overlay keeps its editor, which edits it on every frame in scope.
    let frame_group = adw::PreferencesGroup::builder().title(t("Frame")).build();
    let frame_delay = gtk::SpinButton::with_range(1.0, u16::MAX as f64, 1.0);
    frame_delay.set_valign(gtk::Align::Center);
    {
        let (sender, sync) = (sender.clone(), sync.clone());
        frame_delay.connect_value_changed(move |spin| {
            if sync.get() {
                return;
            }
            sender.input(Msg::SetScopeDelay(spin.value() as u16));
        });
    }
    // Translators: Per-frame hold time, in centiseconds (1/100 s).
    let delay_row = suffixed(t("Delay (cs)"), &frame_delay);
    frame_group.add(&delay_row);

    let overlay_list_group = adw::PreferencesGroup::builder()
        .title(t("Overlays on this frame"))
        .visible(false)
        .build();
    let overlay_list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .build();
    overlay_list.add_css_class("boxed-list");
    let overlay_list_ids: Rc<RefCell<Vec<OverlayId>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let (sender, sync, ids) = (sender.clone(), sync.clone(), overlay_list_ids.clone());
        overlay_list.connect_row_selected(move |_, row| {
            if sync.get() {
                return;
            }
            let picked = row.and_then(|r| ids.borrow().get(r.index() as usize).copied());
            sender.input(Msg::SelectOverlay(picked));
        });
    }
    // Past a handful of layers the list scrolls rather than pushing the
    // overlay editor below it off the panel, the same bargain the strip's
    // band area makes. The cap is a row height measured off the real rows
    // (`update_view`), not a guess at what an ActionRow comes out as.
    let overlay_list_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .propagate_natural_height(true)
        .child(&overlay_list)
        .build();
    overlay_list_group.add(&overlay_list_scroll);

    let doc_info = gtk::Label::new(Some(""));
    doc_info.add_css_class("dim-label");
    doc_info.set_wrap(true);
    doc_info.set_xalign(0.0);
    properties.append(&frame_group);
    properties.append(&overlay_list_group);
    properties.append(&text_group);
    properties.append(&overlay_group);
    properties.append(&crop_group);
    properties.append(&doc_info);

    // The panel is taller than a short window whenever a text overlay's
    // editor is up, so it scrolls; without this the rows below the fold —
    // the colour pickers, the crop buttons, the document summary — were
    // simply unreachable.
    let properties_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .propagate_natural_width(true)
        .child(&properties)
        .build();
    let split = adw::OverlaySplitView::builder()
        .sidebar_position(gtk::PackType::End)
        .content(&paned)
        .sidebar(&properties_scroll)
        .build();

    let breakpoint = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        900.0,
        adw::LengthUnit::Px,
    ));
    breakpoint.add_setter(&split, "collapsed", Some(&true.to_value()));
    root.add_breakpoint(breakpoint);
    // Only one breakpoint applies at a time — the last that matches — so the
    // narrow one repeats the sidebar collapse.
    let narrow = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        MENUBAR_MIN_WINDOW_W,
        adw::LengthUnit::Px,
    ));
    narrow.add_setter(&split, "collapsed", Some(&true.to_value()));
    narrow.add_setter(&menubar, "visible", Some(&false.to_value()));
    narrow.add_setter(&menu_button, "menu-model", Some(&narrow_menu.to_value()));
    root.add_breakpoint(narrow);

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
        ("frame-cut", FrameOp::Cut),
    ] {
        let action = gio::SimpleAction::new(name, None);
        let sender = sender.clone();
        action.connect_activate(move |_, _| sender.input(Msg::FrameOp(op)));
        actions.add_action(&action);
    }
    for (name, make) in [
        ("export", (|| Msg::Export) as fn() -> Msg),
        ("frame-copy", || Msg::FrameCopy),
        ("frame-paste", || Msg::FramePaste),
        ("insert-text", || Msg::AddOverlay(Tool::Text)),
        ("insert-rect", || Msg::AddOverlay(Tool::Rect)),
        ("insert-ellipse", || Msg::AddOverlay(Tool::Ellipse)),
        ("insert-arrow", || Msg::AddOverlay(Tool::Arrow)),
        ("mirror-horizontal", || Msg::Mirror(Axis::Horizontal)),
        ("mirror-vertical", || Msg::Mirror(Axis::Vertical)),
        ("fade-in", || Msg::FadeDialog(Fade::In)),
        ("fade-out", || Msg::FadeDialog(Fade::Out)),
        ("loop-fade", || Msg::LoopFadeDialog),
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
    // The menu item shipped before the dialog did; without this action GTK
    // greys it out.
    let about = gio::SimpleAction::new("about", None);
    {
        let root = root.clone();
        about.connect_activate(move |_, _| about_dialog().present(Some(&root)));
    }
    actions.add_action(&about);
    for (name, dialog) in [
        ("optimize-remove", OptimizeDialog::Remove),
        ("optimize-smart", OptimizeDialog::Smart),
    ] {
        let action = gio::SimpleAction::new(name, None);
        let (sender, root) = (sender.clone(), root.clone());
        action.connect_activate(move |_, _| optimize_dialog(&root, dialog, &sender));
        actions.add_action(&action);
    }
    // The crop-all and resize dialogs need the live canvas size, so the model
    // opens them.
    let crop_all = gio::SimpleAction::new("optimize-crop", None);
    {
        let sender = sender.clone();
        crop_all.connect_activate(move |_, _| sender.input(Msg::CropAllDialog));
    }
    actions.add_action(&crop_all);
    let resize = gio::SimpleAction::new("optimize-resize", None);
    {
        let sender = sender.clone();
        resize.connect_activate(move |_, _| sender.input(Msg::ResizeDialog));
    }
    actions.add_action(&resize);
    let zoom_all = gio::SimpleAction::new("optimize-zoom-all", None);
    {
        let sender = sender.clone();
        zoom_all.connect_activate(move |_, _| sender.input(Msg::ApplyZoomAll));
    }
    actions.add_action(&zoom_all);
    let delay_all = gio::SimpleAction::new("frame-delay-all", None);
    {
        let sender = sender.clone();
        delay_all.connect_activate(move |_, _| sender.input(Msg::DelayAllDialog));
    }
    actions.add_action(&delay_all);
    let import_more = gio::SimpleAction::new("import-more", None);
    {
        let sender = sender.clone();
        import_more.connect_activate(move |_, _| sender.input(Msg::ImportInto(AT_END)));
    }
    actions.add_action(&import_more);
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
        text_group,
        frame_group,
        frame_delay,
        delay_row,
        overlay_list,
        overlay_list_scroll,
        overlay_list_group,
        overlay_list_ids,
        scope_mirror,
        drop_dividers,
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
        shrink_apply,
        snap_button,
        tool_buttons,
        shape_button,
        shape_tool,
        bands_scroll,
        bands_expander,
        canvas_overlay,
        canvas_state,
        doc_info,
        bands_model,
        thumb_textures: ThumbTextures::new(),
        sync,
    }
}

/// The header's editing menus, in the order they appear, each with the
/// `win.*` actions `build` registers. Built once and shared by the menu bar
/// and by the ☰ menu's narrow-window fallback.
pub(super) fn edit_menus() -> [(&'static str, gio::Menu); 5] {
    let frames = gio::Menu::new();
    let clipboard = gio::Menu::new();
    clipboard.append(Some(t("Cut")), Some("win.frame-cut"));
    clipboard.append(Some(t("Copy")), Some("win.frame-copy"));
    clipboard.append(Some(t("Paste")), Some("win.frame-paste"));
    let list = gio::Menu::new();
    list.append(Some(t("Delete")), Some("win.frame-delete"));
    list.append(Some(t("Duplicate")), Some("win.frame-duplicate"));
    list.append(Some(t("Reverse")), Some("win.frame-reverse"));
    let timing = gio::Menu::new();
    timing.append(
        Some(t("Set delay for all frames…")),
        Some("win.frame-delay-all"),
    );
    frames.append_section(None, &clipboard);
    frames.append_section(None, &list);
    frames.append_section(None, &timing);

    let insert = gio::Menu::new();
    for (tool, action) in [
        (Tool::Text, "win.insert-text"),
        (Tool::Rect, "win.insert-rect"),
        (Tool::Ellipse, "win.insert-ellipse"),
        (Tool::Arrow, "win.insert-arrow"),
    ] {
        insert.append(Some(t(tool_label(tool))), Some(action));
    }

    let effects = gio::Menu::new();
    let mirror = gio::Menu::new();
    for (action, name) in [
        (Action::MirrorHorizontal, "win.mirror-horizontal"),
        (Action::MirrorVertical, "win.mirror-vertical"),
    ] {
        mirror.append(Some(t(action.label())), Some(name));
    }
    let fades = gio::Menu::new();
    for (action, name) in [
        (Action::FadeIn, "win.fade-in"),
        (Action::FadeOut, "win.fade-out"),
        (Action::LoopFade, "win.loop-fade"),
    ] {
        fades.append(Some(t(action.label())), Some(name));
    }
    effects.append_section(None, &mirror);
    effects.append_section(None, &fades);

    let image = gio::Menu::new();
    image.append(Some(t("Resize frames…")), Some("win.optimize-resize"));
    // Translators: Crops every frame to a box typed in pixels, shrinking the canvas.
    image.append(Some(t("Crop frames…")), Some("win.optimize-crop"));
    // Translators: Like the Zoom and resize button, but ignoring the frame
    // scope — needs a box drawn with the crop tool first.
    image.append(
        Some(t("Zoom and resize all frames")),
        Some("win.optimize-zoom-all"),
    );

    // Everything that makes the GIF smaller, in one place.
    let optimize = gio::Menu::new();
    optimize.append(Some(t("Remove frames…")), Some("win.optimize-remove"));
    optimize.append(Some(t("Smart remove frames…")), Some("win.optimize-smart"));

    [
        (t("Frames"), frames),
        (t("Insert"), insert),
        // Translators: Header menu with mirror, fade and loop cross-fade.
        (t("Effects"), effects),
        (t("Image"), image),
        (t("Optimize"), optimize),
    ]
}

/// A colour picker and a width spin encode one `Option<(colour, width)>`
/// between them, so each handler has to read the other widget. Width 0 is
/// still `Some`: the pair paints nothing, but the colour picked while it is 0
/// stays on the overlay instead of snapping back to a default, and is the
/// colour the first nonzero width paints in. `update_view`
/// sets the two one at a time, which means the spin's handler reports the
/// *previous* overlay's colour — a value the model disagrees with, so it gets
/// applied, which re-runs the sync, which sends the pair back again. That is a
/// loop, not a stray edit, so `sync` gates the spin rather than
/// `OverlayProp::changes` catching it afterwards. The picker needs no gate: it
/// reports the user's choices and nothing else.
pub(super) fn connect_pair(
    colour: &ColorPick,
    width: &gtk::SpinButton,
    sync: &Rc<Cell<bool>>,
    emit: impl Fn(Option<(crate::core::model::Rgba8, f32)>) + Clone + 'static,
) {
    let pair =
        |colour: crate::core::model::Rgba8, w: &gtk::SpinButton| Some((colour, w.value() as f32));
    {
        let (w, emit) = (width.clone(), emit.clone());
        colour.connect_changed(move |colour| emit(pair(colour, &w)));
    }
    let picker = colour.clone();
    let sync = sync.clone();
    width.connect_value_changed(move |w| {
        if !sync.get() {
            emit(pair(picker.rgba(), w));
        }
    });
}
