//! Keyboard shortcuts: action dispatch, install, and the shortcuts/keybinding window.

use super::*;

/// The message an action fires. Kept next to the keymap so a new action is one
/// arm here and one entry in `keymap::ACTIONS`, not a scattered edit.
pub(super) fn message_for(action: Action) -> Msg {
    match action {
        Action::Open => Msg::Open,
        Action::Export => Msg::Export,
        Action::Undo => Msg::Undo,
        Action::Redo => Msg::Redo,
        Action::PlayPause => Msg::TogglePlay,
        Action::SelectAll => Msg::SelectAllFrames,
        Action::Delete => Msg::DeleteSelection,
        Action::ShowShortcuts => Msg::Toast(String::new()),
        Action::ToolText => Msg::AddOverlay(Tool::Text),
        Action::ToolRect => Msg::AddOverlay(Tool::Rect),
        Action::ToolEllipse => Msg::AddOverlay(Tool::Ellipse),
        Action::ToolArrow => Msg::AddOverlay(Tool::Arrow),
        Action::ToolCrop => Msg::ToggleCropTool,
        Action::ToggleSnap => Msg::ToggleSnap,
        Action::FrameDelete => Msg::FrameOp(FrameOp::Delete),
        Action::FrameDuplicate => Msg::FrameOp(FrameOp::Duplicate),
        Action::FrameCut => Msg::FrameOp(FrameOp::Cut),
        Action::FrameCopy => Msg::FrameCopy,
        Action::FramePaste => Msg::FramePaste,
        Action::FrameReverse => Msg::FrameOp(FrameOp::Reverse),
        Action::ZoomToSelection => Msg::ApplyZoom,
        Action::StripZoomIn => Msg::StripZoom(STRIP_ZOOM_STEP),
        Action::StripZoomOut => Msg::StripZoom(1.0 / STRIP_ZOOM_STEP),
        Action::StripZoomReset => Msg::StripZoom(0.0),
        Action::MirrorHorizontal => Msg::Mirror(Axis::Horizontal),
        Action::MirrorVertical => Msg::Mirror(Axis::Vertical),
        Action::FadeIn => Msg::FadeDialog(Fade::In),
        Action::FadeOut => Msg::FadeDialog(Fade::Out),
        Action::LoopFade => Msg::LoopFadeDialog,
    }
}

/// One controller reading the live keymap, rather than a match on hardcoded
/// keys. The keymap is shared rather than owned because the controller cannot
/// borrow the model.
pub(super) fn install_shortcuts(
    root: &adw::ApplicationWindow,
    sender: &ComponentSender<App>,
    keymap: Rc<RefCell<Keymap>>,
) {
    let keys = shortcuts_controller();
    let esc_sender = sender.clone();
    let sender = sender.clone();
    let root_for_dialog = root.clone();
    // Where the tool cycle is up to, for the case where several tools share a
    // key (AGENTS.md: that is a cycle, not a duplicate).
    let cycle = std::cell::Cell::new(0usize);
    keys.connect_key_pressed(move |controller, key, _, state| {
        let focus = controller
            .widget()
            .and_then(|w| w.downcast::<gtk::Window>().ok())
            .and_then(|w| gtk::prelude::GtkWindowExt::focus(&w));
        if focus_owns_keys(focus.as_ref()) {
            return glib::Propagation::Proceed;
        }
        let Some(chord) = Chord::from_event(key, state) else {
            return glib::Propagation::Proceed;
        };
        let actions = keymap.borrow().actions_for(&chord);
        let Some(action) = (match actions.len() {
            0 => None,
            1 => Some(actions[0]),
            _ => {
                let next = cycle.get().wrapping_add(1) % actions.len();
                cycle.set(next);
                Some(actions[next])
            }
        }) else {
            return glib::Propagation::Proceed;
        };
        if action == Action::ShowShortcuts {
            shortcuts_dialog(&root_for_dialog, &keymap, &sender);
        } else {
            sender.input(message_for(action));
        }
        glib::Propagation::Stop
    });
    root.add_controller(keys);

    // Escape is not a bindable action: it just backs out of the crop tool or
    // the overlay selection. Bubble phase, so a focused entry clearing itself
    // wins first.
    let esc = gtk::EventControllerKey::new();
    {
        esc.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                esc_sender.input(Msg::Escape);
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
    }
    root.add_controller(esc);
}

/// The window's key controller, in the capture phase rather than the default
/// bubble one. A focused widget handles a key before a bubble-phase controller
/// on the window ever sees it, and GTK both activates a focused button on Space
/// and parks the initial focus on the first focusable widget it finds — so with
/// a document open, Space added an overlay instead of playing. Capture is the
/// only phase that gets there first; `focus_owns_keys` is what hands the
/// keystroke back to the widgets that genuinely own it.
pub(super) fn shortcuts_controller() -> gtk::EventControllerKey {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys
}

/// Whether the focused widget keeps its own keystrokes rather than handing them
/// to the shortcuts. A text entry does, or typing "a" in a caption inserts an
/// arrow; so does anything inside a dialog, which has its own keys and its own
/// idea of Escape. A button does not: it would eat Space, which is play/pause.
pub(super) fn focus_owns_keys(focus: Option<&gtk::Widget>) -> bool {
    focus.is_some_and(|f| {
        f.is::<gtk::Editable>()
            || f.is::<gtk::TextView>()
            || f.ancestor(adw::Dialog::static_type()).is_some()
    })
}

/// The keybinding editor, which is also the Ctrl+? shortcuts window: one list
/// that both shows what a key does and lets it be changed, rather than a
/// read-only window and a settings page saying the same things twice.
pub(super) fn shortcuts_dialog(
    root: &adw::ApplicationWindow,
    keymap: &Rc<RefCell<Keymap>>,
    sender: &ComponentSender<App>,
) {
    let dialog = adw::PreferencesDialog::new();
    dialog.set_title(t("Keyboard shortcuts"));
    let page = adw::PreferencesPage::new();
    let editor = BindingEditor {
        working: Rc::new(RefCell::new(keymap.borrow().clone())),
        capturing: Rc::default(),
        capturing_modal: Rc::default(),
        rows: Rc::default(),
        modal_rows: Rc::default(),
        sender: sender.clone(),
    };
    add_action_groups(&page, &editor);
    page.add(&canvas_group(&editor));

    // Capture runs in the capture phase, so the dialog's own widgets never see
    // the keystroke being bound.
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let editor = editor.clone();
        keys.connect_key_pressed(move |_, key, _, state| editor.capture(key, state));
    }
    dialog.add_controller(keys);

    page.add(&file_group(root, &editor));
    dialog.add(&page);
    editor.refresh();
    dialog.present(Some(root));
}

/// What every row and handler of the keybinding editor shares.
#[derive(Clone)]
struct BindingEditor {
    /// The keymap being edited, handed to the app whole on each change.
    working: Rc<RefCell<Keymap>>,
    /// Which row is waiting for a keypress, if any. Chords and canvas
    /// modifiers capture the same way but read different halves of the event.
    capturing: Rc<Cell<Option<Action>>>,
    capturing_modal: Rc<Cell<Option<Modal>>>,
    rows: Rc<RefCell<Vec<(Action, adw::ActionRow, gtk::Button)>>>,
    modal_rows: Rc<RefCell<Vec<(Modal, gtk::Button)>>>,
    sender: ComponentSender<App>,
}

impl BindingEditor {
    /// Writes every row's binding, the capture prompt, and the conflict marks.
    fn refresh(&self) {
        let map = self.working.borrow();
        let conflicts = map.conflicts();
        let waiting = self.capturing.get();
        let waiting_modal = self.capturing_modal.get();
        for (modal, button) in self.modal_rows.borrow().iter() {
            let mods = map.mods(*modal);
            button.set_label(&if waiting_modal == Some(*modal) {
                // Translators: A canvas modifier row waiting for the user to hold a key.
                t("Hold a modifier…").to_string()
            } else if mods.is_empty() {
                t("Unassigned").to_string()
            } else {
                mods.display()
            });
        }
        for (action, row, button) in self.rows.borrow().iter() {
            let chords: Vec<String> = map
                .chords(*action)
                .iter()
                .map(crate::keymap::Chord::display)
                .collect();
            button.set_label(&if waiting == Some(*action) {
                t("Press a key…").to_string()
            } else if chords.is_empty() {
                t("Unassigned").to_string()
            } else {
                chords.join(", ")
            });
            let clash = conflicts.contains(action);
            set_class(button.upcast_ref(), "bind-conflict", clash);
            row.set_subtitle(if clash {
                t("Already used by another action")
            } else {
                ""
            });
        }
    }

    /// Every edit to the bindings: apply `edit`, redraw the rows, and hand the
    /// keymap to the app, which applies and saves it. One path, so no button
    /// can change the list without saving it.
    fn change(&self, edit: impl FnOnce(&mut Keymap)) {
        edit(&mut self.working.borrow_mut());
        self.refresh();
        self.sender
            .input(Msg::SetKeymap(Box::new(self.working.borrow().clone())));
    }

    /// A keypress while the dialog is up: the answer to whichever row is
    /// waiting for one, or Escape to stop waiting.
    fn capture(&self, key: gdk::Key, state: gdk::ModifierType) -> glib::Propagation {
        if let Some(modal) = self.capturing_modal.get() {
            if key == gdk::Key::Escape {
                self.capturing_modal.set(None);
                self.refresh();
                return glib::Propagation::Stop;
            }
            // Anything that is not a modifier is not an answer to this
            // question, so it is swallowed rather than bound.
            let Some(mods) = Mods::from_event(key, state) else {
                return glib::Propagation::Stop;
            };
            self.capturing_modal.set(None);
            self.change(|k| k.set_mods(modal, mods));
            return glib::Propagation::Stop;
        }
        let Some(action) = self.capturing.get() else {
            return glib::Propagation::Proceed;
        };
        if key == gdk::Key::Escape {
            self.capturing.set(None);
            self.refresh();
            return glib::Propagation::Stop;
        }
        let Some(chord) = Chord::from_event(key, state) else {
            return glib::Propagation::Stop;
        };
        self.capturing.set(None);
        self.change(|k| k.set(action, vec![chord]));
        glib::Propagation::Stop
    }
}

/// A binding row titled `title`: the button that shows the binding and
/// starts a capture, then the one that clears it.
fn binding_row(title: &str) -> (adw::ActionRow, gtk::Button, gtk::Button) {
    let row = adw::ActionRow::builder().title(title).build();
    let button = gtk::Button::with_label("");
    button.set_valign(gtk::Align::Center);
    button.set_width_request(150);
    let clear = gtk::Button::from_icon_name("edit-clear-symbolic");
    clear.set_valign(gtk::Align::Center);
    clear.add_css_class("flat");
    clear.set_tooltip_text(Some(t("Unbind")));
    row.add_suffix(&button);
    row.add_suffix(&clear);
    (row, button, clear)
}

/// One group per `Action::group`, in `keymap::ACTIONS` order, with a chord
/// row per action.
fn add_action_groups(page: &adw::PreferencesPage, editor: &BindingEditor) {
    let mut group: Option<adw::PreferencesGroup> = None;
    let mut last = "";
    for action in crate::keymap::ACTIONS {
        if action.group() != last {
            last = action.group();
            let g = adw::PreferencesGroup::builder().title(t(last)).build();
            page.add(&g);
            group = Some(g);
        }
        let (row, button, clear) = binding_row(t(action.label()));
        {
            let editor = editor.clone();
            button.connect_clicked(move |_| {
                editor.capturing_modal.set(None);
                editor.capturing.set(Some(action));
                editor.refresh();
            });
        }
        {
            let editor = editor.clone();
            clear.connect_clicked(move |_| {
                editor.change(|k| k.set(action, Vec::new()));
            });
        }
        if let Some(g) = &group {
            g.add(&row);
        }
        editor.rows.borrow_mut().push((action, row, button));
    }
}

/// Canvas modifiers: held during a drag rather than pressed as a chord, so
/// they live in their own group and capture a bare modifier.
fn canvas_group(editor: &BindingEditor) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title(t("Canvas"))
        .description(t("Held while dragging an overlay's grip."))
        .build();
    for modal in MODALS {
        let (row, button, clear) = binding_row(t(modal.label()));
        {
            let editor = editor.clone();
            button.connect_clicked(move |_| {
                editor.capturing.set(None);
                editor.capturing_modal.set(Some(modal));
                editor.refresh();
            });
        }
        {
            let editor = editor.clone();
            clear.connect_clicked(move |_| {
                editor.change(|k| k.set_mods(modal, Mods::default()));
            });
        }
        group.add(&row);
        editor.modal_rows.borrow_mut().push((modal, button));
    }
    group
}

/// Import, export and reset for the keybindings file.
fn file_group(root: &adw::ApplicationWindow, editor: &BindingEditor) -> adw::PreferencesGroup {
    let file_group = adw::PreferencesGroup::builder()
        .title(t("Keybindings file"))
        .description(t("Bindings are saved as you change them."))
        .build();
    let file_row = adw::ActionRow::builder()
        .title(t("Import or export"))
        .subtitle(
            crate::keymap::path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| t("no config directory").into()),
        )
        .build();
    let import = gtk::Button::with_label(t("Import…"));
    let export = gtk::Button::with_label(t("Export…"));
    let reset = gtk::Button::with_label(t("Reset"));
    for b in [&import, &export, &reset] {
        b.set_valign(gtk::Align::Center);
        file_row.add_suffix(b);
    }
    {
        let (editor, parent) = (editor.clone(), root.clone());
        import.connect_clicked(move |_| {
            let editor = editor.clone();
            gtk::FileDialog::builder()
                .title(t("Import keybindings"))
                .build()
                .open(Some(&parent), gio::Cancellable::NONE, move |res| {
                    let Some(path) = res.ok().and_then(|f| f.path()) else {
                        return;
                    };
                    match std::fs::read_to_string(&path) {
                        Ok(text) => {
                            editor.change(|k| k.apply(&text));
                        }
                        Err(e) => editor.sender.input(Msg::Toast(fill(
                            t("Could not read {path}: {error}"),
                            &[
                                ("path", &path.display().to_string()),
                                ("error", &e.to_string()),
                            ],
                        ))),
                    }
                });
        });
    }
    {
        let (editor, parent) = (editor.clone(), root.clone());
        export.connect_clicked(move |_| {
            let editor = editor.clone();
            let save = gtk::FileDialog::builder()
                .title(t("Export keybindings"))
                .build();
            save.set_initial_name(Some("keybindings.conf"));
            save.save(Some(&parent), gio::Cancellable::NONE, move |res| {
                let Some(path) = res.ok().and_then(|f| f.path()) else {
                    return;
                };
                let text = editor.working.borrow().to_text();
                match std::fs::write(&path, text) {
                    Ok(()) => editor.sender.input(Msg::Toast(fill(
                        t("Saved {path}"),
                        &[("path", &path.display().to_string())],
                    ))),
                    Err(e) => editor.sender.input(Msg::Toast(fill(
                        t("Could not write: {error}"),
                        &[("error", &e.to_string())],
                    ))),
                }
            });
        });
    }
    {
        let editor = editor.clone();
        reset.connect_clicked(move |_| {
            editor.change(|k| *k = Keymap::default());
        });
    }
    file_group.add(&file_row);
    file_group
}
