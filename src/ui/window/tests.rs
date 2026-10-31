use super::view::{fps_of, timecode};
use super::*;
use crate::pipeline::video::VideoInfo;

fn doc(frames: usize) -> Document {
    Document::from_frames(
        (0..frames)
            .map(|i| {
                let px = image::Rgba([i as u8, 0, 0, 255]);
                Frame::new(image::RgbaImage::from_pixel(8, 8, px), 5)
            })
            .collect(),
    )
}

/// A minimal `App` for testing scope logic without a display: nothing
/// on the model itself is GTK-owned, so this needs no window or sender.
fn app_with(doc: Document, scope: ScopeChoice, selection: Vec<usize>, playhead: usize) -> App {
    App {
        playhead,
        scope,
        selection,
        ..App::new(doc, Caps::default(), Settings::default(), Keymap::default())
    }
}

/// Scope-driven tools - "Crop and keep size" and "Zoom and resize" on the
/// canvas, and the frame-operations menu (delete/duplicate/reverse) - must
/// land on exactly what the scope names: the frame on screen, a selection,
/// or every frame. Regression: `Msg::ApplyShrink` and `Msg::ApplyZoom` both
/// used to hardcode `vec![self.playhead]`, so selecting "All frames" and
/// drawing a crop box still only touched the frame on screen.
#[test]
fn scope_driven_tools_touch_exactly_what_the_scope_names() {
    let cases: Vec<(App, Vec<usize>)> = vec![
        (
            app_with(doc(5), ScopeChoice::ThisFrame, Vec::new(), 3),
            vec![3],
        ),
        (
            app_with(doc(5), ScopeChoice::Range, vec![4, 1, 1], 0),
            vec![1, 4],
        ),
        (
            app_with(doc(5), ScopeChoice::AllFrames, Vec::new(), 0),
            (0..5).collect(),
        ),
    ];
    for (app, want) in &cases {
        assert_eq!(
            &app.scope_frames(),
            want,
            "scope_frames() must resolve to {want:?}"
        );

        // The exact path Msg::ApplyShrink takes, minus dispatching the
        // work to the worker thread.
        let work = app.shrink_work((1.0, 1.0, 4.0, 4.0)).unwrap();
        let FrameWork::Shrink { frames, .. } = work else {
            panic!("expected Shrink work");
        };
        assert_eq!(&frames, want, "shrink must touch exactly the scoped frames");

        // The panel's "Zoom and resize" (Msg::ApplyZoom) and the band menu's
        // "Copy overlay to selected frames" follow the same scope; their
        // "all frames" variants never do.
        assert_eq!(
            &app.frames_or_scope(false),
            want,
            "scoped zoom and overlay copy must touch exactly the scoped frames"
        );
        assert_eq!(
            app.frames_or_scope(true),
            (0..app.frame_count()).collect::<Vec<_>>(),
            "'all frames' zoom and copy must ignore the scope"
        );
    }
}

/// Regression: an overlay spanning many frames could only be picked from
/// the canvas, whose edits land on the frame on screen and split it off.
/// Picking its band reaches the whole range.
#[test]
fn a_band_pick_edits_the_overlay_across_its_whole_range() {
    let mut app = app_with(doc(8), ScopeChoice::ThisFrame, Vec::new(), 0);
    let id = app.editor.doc.add_overlay(
        "box",
        OverlayKind::Shape(ShapeOverlay {
            shape: Shape::Rect,
            fill: Some([1, 2, 3, 255]),
            stroke: None,
        }),
        Transform::at(0.0, 0.0, 8.0, 8.0),
        2..7,
    );
    app.select_band(id);
    assert!((2..7).contains(&app.playhead), "the overlay is on screen");
    assert_eq!(app.editing_overlay(), Some(id));
    assert_eq!(app.edit_span(id), 2..7);

    let moved = Transform::at(5.0, 5.0, 8.0, 8.0);
    let span = app.edit_span(id);
    app.editor.edit("Overlay moved", span.len(), |d| {
        d.edit_on_frames(id, span, |o| o.transform = moved)
    });
    assert_eq!(app.editor.doc.overlays.len(), 1, "nothing was split off");
    assert_eq!(app.editor.doc.overlay(id).map(|o| o.transform), Some(moved));
}

/// Regression: the strip was keyed on the document revision, which overlay
/// edits bump too, so every keystroke in the text entry rebuilt a widget and
/// a texture per frame.
#[test]
fn overlay_edits_leave_the_strip_alone() {
    let mut doc = doc(4);
    let before = strip_keys(&doc);

    let id = doc.add_overlay(
        "caption",
        OverlayKind::Text(TextOverlay {
            text: "hi".into(),
            ..Default::default()
        }),
        Transform::at(0.0, 0.0, 8.0, 8.0),
        0..4,
    );
    assert_eq!(
        strip_keys(&doc),
        before,
        "adding an overlay changes no frame"
    );

    if let Some(OverlayKind::Text(t)) = doc.overlay_mut(id).map(|o| &mut o.kind) {
        t.text = "hi there".into();
    }
    assert_eq!(strip_keys(&doc), before, "typing changes no frame");

    doc.remove_overlay(id);
    assert_eq!(strip_keys(&doc), before);
}

#[test]
fn frame_edits_do_rebuild_the_strip() {
    let mut doc = doc(4);
    let before = strip_keys(&doc);

    doc.duplicate_frames_at(&[0]);
    assert_ne!(strip_keys(&doc), before);

    let after_duplicate = strip_keys(&doc);
    doc.delete_frames(0..1);
    assert_ne!(strip_keys(&doc), after_duplicate);

    // A frame back from an external editor gets the pencil badge, so the
    // flag rides along with the key.
    let mut doc = doc.clone();
    let opaque = strip_keys(&doc);
    let mut edited = Frame::new(image::RgbaImage::new(8, 8), 10);
    edited.detached = true;
    doc.frames[0] = edited;
    assert_ne!(strip_keys(&doc), opaque);
    assert!(strip_keys(&doc)[0].1, "detached");
}

/// Regression: the sidebar overlay editor stayed up for an overlay on a
/// different frame, so a frame with nothing on it still showed text fields.
#[test]
fn sidebar_edits_an_overlay_only_while_it_is_on_the_frame() {
    let mut app = app_with(doc(6), ScopeChoice::ThisFrame, Vec::new(), 0);
    let id = app.editor.doc.add_overlay(
        "cap",
        OverlayKind::Text(TextOverlay {
            text: "hi".into(),
            ..Default::default()
        }),
        Transform::at(0.0, 0.0, 8.0, 8.0),
        1..3,
    );
    app.selected_overlay = Some(id);

    app.playhead = 2;
    assert_eq!(app.editing_overlay(), Some(id), "on the overlay's frame");

    app.playhead = 4;
    assert_eq!(app.editing_overlay(), None, "off it: plain frame view");
    assert!(app.overlays_on(4).is_empty());

    app.playhead = 1;
    assert_eq!(app.editing_overlay(), Some(id), "memory: back on its frame");
}

/// Regression: an armed crop tool hid the selected overlay's outline, so
/// picking text looked like nothing happened. Picking from the timeline
/// or the layer list leaves the tool, box or not. On the canvas, a press on text is a pick
/// only while no box is drawn; with a box up it redraws the box.
#[test]
fn picking_an_overlay_leaves_crop() {
    let mut app = app_with(doc(4), ScopeChoice::ThisFrame, Vec::new(), 0);
    let id = app.editor.doc.add_overlay(
        "cap",
        OverlayKind::Text(TextOverlay::default()),
        Transform::at(10.0, 10.0, 20.0, 20.0),
        0..4,
    );
    app.crop_tool = true;

    app.crop_rect = Some((15.0, 15.0, 0.0, 0.0)); // a bare click, no drag
    assert!(app.crop_yields_to(15.0, 15.0), "on text, no box: a pick");
    assert!(!app.crop_yields_to(1.0, 1.0), "off text: a new box");

    app.crop_rect = Some((0.0, 0.0, 40.0, 40.0));
    assert!(!app.crop_yields_to(15.0, 15.0), "box up: redraw it");

    app.pick_overlay(Some(id));
    assert!(!app.crop_tool);
    assert_eq!(app.crop_rect, None);
    assert_eq!(app.editing_overlay(), Some(id));

    // A timeline band click, which also picks the band's frames.
    app.crop_tool = true;
    app.crop_rect = Some((0.0, 0.0, 40.0, 40.0));
    app.select_band(id);
    assert!(!app.crop_tool);
    assert_eq!(app.crop_rect, None);
    assert_eq!(app.editing_overlay(), Some(id));
}

/// A 100×100 single-frame canvas holding one 20×20 box at (10, 10).
fn canvas_with_box() -> (App, OverlayId) {
    let doc = Document::from_frames(vec![Frame::new(image::RgbaImage::new(100, 100), 10)]);
    let mut app = app_with(doc, ScopeChoice::ThisFrame, Vec::new(), 0);
    let id = app.editor.doc.add_overlay(
        "box",
        OverlayKind::Shape(ShapeOverlay {
            shape: Shape::Rect,
            fill: Some([1, 2, 3, 255]),
            stroke: None,
        }),
        Transform::at(10.0, 10.0, 20.0, 20.0),
        0..1,
    );
    (app, id)
}

fn transform_of(app: &App, id: OverlayId) -> Transform {
    app.editor.doc.overlay(id).unwrap().transform
}

const NO_MODS: gdk::ModifierType = gdk::ModifierType::empty();

/// A dragged box pulls onto the nearest canvas guide, shows it while it
/// holds, and commits where it was pulled as one undo step. With
/// snapping off it lands exactly where it was dragged.
#[test]
fn a_dragged_overlay_snaps_onto_the_canvas_centre_line() {
    let (mut app, id) = canvas_with_box();
    // The box's centre goes 20 → 52: 2 px off the vertical centre line,
    // both edges 8+ px off every guide, the y axis nowhere near one.
    app.canvas_press(20.0, 20.0, 1.0, NO_MODS);
    app.canvas_drag(35.0, 20.0, NO_MODS);
    app.canvas_drag(52.0, 20.0, NO_MODS);
    let centred = Transform::at(40.0, 10.0, 20.0, 20.0);
    assert_eq!(transform_of(&app, id), centred, "live on the canvas");
    let on_centre = Guides {
        x: Some(snap::Guide::Center),
        y: None,
    };
    assert_eq!(app.guides, on_centre);

    assert!(app.canvas_release().is_some());
    assert_eq!(
        app.guides,
        Guides::default(),
        "the guide goes with the drag"
    );
    assert_eq!(transform_of(&app, id), centred);
    assert!(app.editor.undo());
    assert_eq!(
        transform_of(&app, id),
        Transform::at(10.0, 10.0, 20.0, 20.0),
        "one undo returns to where the drag started"
    );
    assert!(!app.editor.can_undo(), "the drag was a single step");

    app.snap = false;
    app.canvas_press(20.0, 20.0, 1.0, NO_MODS);
    app.canvas_drag(52.0, 20.0, NO_MODS);
    assert_eq!(app.guides, Guides::default());
    app.canvas_release();
    assert_eq!(
        transform_of(&app, id),
        Transform::at(42.0, 10.0, 20.0, 20.0)
    );
}

/// The snap reach is a screen distance: zoomed in 4×, 2 image px off a
/// guide is 8 screen px and out of reach, 1 image px is in.
#[test]
fn snap_reach_stays_constant_on_screen_at_any_zoom() {
    let (mut app, id) = canvas_with_box();
    app.canvas_press(20.0, 20.0, 4.0, NO_MODS);
    app.canvas_drag(52.0, 20.0, NO_MODS);
    assert_eq!(app.guides, Guides::default());
    assert_eq!(transform_of(&app, id).x, 42.0);
    app.canvas_drag(51.0, 20.0, NO_MODS);
    assert_eq!(app.guides.x, Some(snap::Guide::Center));
    assert_eq!(transform_of(&app, id).x, 40.0);
}

/// `update_view` recomposites the canvas only when `canvas_key` moves, so
/// everything that changes the picture on screen must move it: a recorded
/// edit, each live drag step, and a release that puts the box back without
/// recording anything (the scope here misses the overlay, so the drag has
/// nowhere to land).
#[test]
fn the_canvas_key_moves_whenever_the_picture_would() {
    let mut app = app_with(doc(3), ScopeChoice::Range, vec![2], 0);
    let id = app.editor.doc.add_overlay(
        "box",
        OverlayKind::Shape(ShapeOverlay {
            shape: Shape::Rect,
            fill: Some([1, 2, 3, 255]),
            stroke: None,
        }),
        Transform::at(1.0, 1.0, 4.0, 4.0),
        0..1,
    );
    let mut shown = vec![app.canvas_key()];
    let mut moved = |app: &App, why: &str| {
        let key = app.canvas_key();
        assert!(!shown.contains(&key), "{why}: {key:?} already shown");
        shown.push(key);
    };

    app.canvas_press(2.0, 2.0, 1.0, NO_MODS);
    app.canvas_drag(4.0, 2.0, NO_MODS);
    moved(&app, "live drag");
    assert!(app.canvas_release().is_none(), "nothing in scope to commit");
    assert_eq!(transform_of(&app, id), Transform::at(1.0, 1.0, 4.0, 4.0));
    moved(&app, "box put back");

    app.editor
        .edit("Overlay deleted", 1, |d| d.remove_overlay(id));
    app.after_edit();
    moved(&app, "edit on the frame on screen");
}

/// Regression: a release whose scope misses the dragged overlay puts it back
/// without recording an edit, and used to leave `rev` where the last drag
/// step had it — so a size measured mid-drag, of the moved box, still
/// passed for the restored document's and was never measured again.
#[test]
fn a_release_that_puts_the_overlay_back_retires_the_estimate() {
    let mut app = app_with(doc(3), ScopeChoice::Range, vec![2], 0);
    let id = app.editor.doc.add_overlay(
        "box",
        OverlayKind::Shape(ShapeOverlay {
            shape: Shape::Rect,
            fill: Some([1, 2, 3, 255]),
            stroke: None,
        }),
        Transform::at(1.0, 1.0, 4.0, 4.0),
        0..1,
    );
    app.canvas_press(2.0, 2.0, 1.0, NO_MODS);
    app.canvas_drag(4.0, 2.0, NO_MODS);
    // The debounce fired while the button was held: this measures the box
    // where the drag had it.
    app.estimate = Some((app.rev, 1234));

    assert!(app.canvas_release().is_none(), "nothing in scope to commit");
    assert_eq!(transform_of(&app, id), Transform::at(1.0, 1.0, 4.0, 4.0));
    assert_ne!(
        app.estimate.map(|(rev, _)| rev),
        Some(app.rev),
        "the mid-drag measurement must not pass for the restored document"
    );
}

/// A dragged corner of an upright box pulls onto a guide. Locked to its
/// aspect, or rotated, it follows the pointer: the guide would have to be
/// solved back through the resize.
#[test]
fn a_resized_corner_snaps_only_on_an_upright_unlocked_box() {
    let (mut app, id) = canvas_with_box();
    app.selected_overlay = Some(id);
    app.canvas_press(30.0, 30.0, 1.0, NO_MODS); // the bottom-right grip
    app.canvas_drag(97.0, 60.0, NO_MODS);
    let on_right_edge = Guides {
        x: Some(snap::Guide::End),
        y: None,
    };
    assert_eq!(app.guides, on_right_edge);
    assert_eq!(
        transform_of(&app, id),
        Transform::at(10.0, 10.0, 90.0, 50.0)
    );
    app.canvas_release();
    app.editor.undo();

    let shift = gdk::ModifierType::SHIFT_MASK;
    app.canvas_press(30.0, 30.0, 1.0, shift);
    app.canvas_drag(97.0, 60.0, shift);
    assert_eq!(app.guides, Guides::default(), "aspect locked");
    app.canvas_release();
    app.editor.undo();

    let tilted = Transform {
        angle: 0.3,
        ..transform_of(&app, id)
    };
    app.editor.doc.overlay_mut(id).unwrap().transform = tilted;
    let (gx, gy) = tilted.to_image(30.0, 30.0);
    app.canvas_press(gx, gy, 1.0, NO_MODS);
    app.canvas_drag(97.0, 60.0, NO_MODS);
    assert!(
        matches!(
            app.drag,
            Some(Drag {
                mode: DragMode::Resize(_),
                ..
            })
        ),
        "the rotated grip was caught"
    );
    assert_eq!(app.guides, Guides::default(), "rotated");
}

/// Alt turns a press on a box into a rotation about its centre; Shift
/// steps it to 1/32 turns. Size and position stay put.
#[test]
fn an_alt_drag_rotates_and_shift_steps_the_angle() {
    let (mut app, id) = canvas_with_box(); // centred on (20, 20)
    let alt = gdk::ModifierType::ALT_MASK;
    let turned = |a: f32| (20.0 + 8.0 * a.cos(), 20.0 + 8.0 * a.sin());
    app.canvas_press(28.0, 20.0, 1.0, alt);
    let (x, y) = turned(0.3);
    app.canvas_drag(x, y, alt);
    assert!((transform_of(&app, id).angle - 0.3).abs() < 1e-4);
    app.canvas_drag(x, y, alt | gdk::ModifierType::SHIFT_MASK);
    let step = std::f32::consts::TAU / ROTATE_STEPS;
    assert!((transform_of(&app, id).angle - 2.0 * step).abs() < 1e-4);
    app.canvas_release();
    assert_eq!(
        transform_of(&app, id),
        Transform {
            angle: 2.0 * step,
            ..Transform::at(10.0, 10.0, 20.0, 20.0)
        }
    );
}

/// A click that does not move records nothing, and neither does a crop
/// box, which is drawn in any direction as a positive rect.
#[test]
fn clicks_and_crop_boxes_leave_no_undo_step() {
    let (mut app, _) = canvas_with_box();
    app.canvas_press(20.0, 20.0, 1.0, NO_MODS);
    app.canvas_drag(20.3, 19.8, NO_MODS); // hand tremor
    assert!(app.canvas_release().is_none());

    app.crop_tool = true;
    app.canvas_press(60.0, 60.0, 1.0, NO_MODS);
    app.canvas_drag(40.0, 80.0, NO_MODS);
    assert_eq!(app.crop_rect, Some((40.0, 60.0, 20.0, 20.0)));
    assert!(app.canvas_release().is_none());
    assert_eq!(
        app.crop_rect,
        Some((40.0, 60.0, 20.0, 20.0)),
        "the box stays"
    );
    assert!(!app.editor.can_undo());
}

/// The clock under the canvas reads when the frame on screen starts, as
/// mm:ss.d, and the frame rate is frames per second of total duration.
#[test]
fn the_clock_reads_the_start_of_the_frame_on_screen() {
    let doc = doc_with_delays(&[4000, 1999, 7]);
    let app = app_with(doc, ScopeChoice::ThisFrame, Vec::new(), 2);
    assert_eq!(app.elapsed_cs(), 5999);
    assert_eq!(timecode(app.elapsed_cs()), "00:59.9");
    assert_eq!(timecode(6000), "01:00.0", "the minute rolls over");
    assert_eq!(timecode(6059), "01:00.5");

    assert_eq!(fps_of(&doc_with_delays(&[10, 10, 10, 10])), 10);
    assert_eq!(fps_of(&doc_with_delays(&[3, 3, 4])), 30, "rounded");
    assert_eq!(fps_of(&doc_with_delays(&[])), 0, "no duration, no rate");
}

fn doc_with_delays(delays: &[u16]) -> Document {
    let mut d = doc(delays.len());
    for (frame, cs) in d.frames.iter_mut().zip(delays) {
        frame.delay_cs = *cs;
    }
    d
}

/// The layer list reads top-down like the canvas stacks — the last
/// overlay in `doc.overlays` paints on top, so it is the first row — and
/// a step moves past the layer *shown* next to it, skipping overlays that
/// are not on this frame and would make the button look dead.
#[test]
fn layer_list_is_topmost_first_and_steps_past_what_it_shows() {
    let mut app = app_with(doc(4), ScopeChoice::ThisFrame, Vec::new(), 0);
    let shape = || {
        OverlayKind::Shape(ShapeOverlay {
            shape: Shape::Rect,
            fill: Some([1, 2, 3, 255]),
            stroke: None,
        })
    };
    let box_at = Transform::at(0.0, 0.0, 8.0, 8.0);
    let bottom = app.editor.doc.add_overlay("bottom", shape(), box_at, 0..4);
    let elsewhere = app
        .editor
        .doc
        .add_overlay("elsewhere", shape(), box_at, 2..4);
    let top = app.editor.doc.add_overlay("top", shape(), box_at, 0..4);

    let rows = |app: &App| {
        app.stacked_overlays()
            .into_iter()
            .map(|(_, name)| name)
            .collect::<Vec<_>>()
    };
    assert_eq!(rows(&app), vec!["top", "bottom"], "topmost row first");

    assert_eq!(
        app.restack_neighbour(bottom, true),
        Some(top),
        "a step up goes past the row above, not past `elsewhere`"
    );
    assert_eq!(app.restack_neighbour(top, true), None, "already on top");
    assert_eq!(app.restack_neighbour(bottom, false), None, "already bottom");

    app.restack_overlay(bottom, true);
    assert_eq!(rows(&app), vec!["bottom", "top"]);
    assert_eq!(
        app.editor.doc.overlays.last().map(|o| o.id),
        Some(bottom),
        "the first row is the overlay painted last"
    );
    assert_eq!(
        app.editor.doc.overlay(elsewhere).map(|o| o.range.clone()),
        Some(2..4),
        "an overlay the step skipped is untouched"
    );
    assert_eq!(app.selected_overlay, Some(bottom), "stays on what moved");
}

/// Copy leaves the document alone and paste splices the clipboard in
/// after the frame on screen, keeping the clipboard for the next paste.
#[test]
fn copied_frames_paste_in_after_the_frame_on_screen() {
    let mut app = app_with(doc(4), ScopeChoice::ThisFrame, Vec::new(), 0);
    let reds = |app: &App| {
        app.editor
            .doc
            .frames
            .iter()
            .map(|f| f.pixels.get_pixel(0, 0).0[0])
            .collect::<Vec<u8>>()
    };
    assert_eq!(app.paste_frames(), None, "nothing copied yet");

    app.copy_frames(&[0, 1]);
    assert_eq!(reds(&app), vec![0, 1, 2, 3], "copying edits nothing");

    app.playhead = 2;
    assert!(app.paste_frames().is_some());
    assert_eq!(reds(&app), vec![0, 1, 2, 0, 1, 3], "the run lands in order");
    assert_eq!(app.playhead, 4, "on the last pasted frame");
    assert_eq!(app.selection, vec![3, 4], "the pasted run stays picked");

    assert!(app.paste_frames().is_some(), "the clipboard is still there");
    assert_eq!(reds(&app), vec![0, 1, 2, 0, 1, 0, 1, 3]);
}

/// Regression: the band click read `bands_model[pixel_row]`, so with more
/// than one overlay - two packed onto row 0 side by side, a third on row 1 -
/// clicking the second or third band selected the wrong overlay or nothing.
#[test]
fn band_click_hits_the_band_under_the_point() {
    let pitch = cell_pitch(THUMB_BOX, 1.0);
    let mid = |frame: usize| (frame as f64 + 0.5) * pitch;
    let bands = vec![
        Band {
            id: OverlayId(1),
            name: "A".into(),
            range: 0..3,
            selected: false,
            row: 0,
        },
        Band {
            id: OverlayId(2),
            name: "B".into(),
            range: 5..8,
            selected: false,
            row: 0,
        },
        Band {
            id: OverlayId(3),
            name: "C".into(),
            range: 1..6,
            selected: false,
            row: 1,
        },
    ];
    let y0 = BAND_H * 0.5;
    let y1 = BAND_H * 1.5;
    assert_eq!(band_at(&bands, mid(1), y0, pitch), Some(OverlayId(1)));
    assert_eq!(
        band_at(&bands, mid(6), y0, pitch),
        Some(OverlayId(2)),
        "2nd band on row 0"
    );
    assert_eq!(band_at(&bands, mid(4), y0, pitch), None, "gap between them");
    assert_eq!(
        band_at(&bands, mid(3), y1, pitch),
        Some(OverlayId(3)),
        "band on row 1"
    );
    assert_eq!(
        band_at(&bands, mid(3), BAND_H * 2.5, pitch),
        None,
        "empty row"
    );
}

/// Strip zoom scales the band hit-test pitch: a point that lands on frame 6
/// at 1x sits on frame 3 when the strip is drawn at half size.
#[test]
fn band_hit_test_follows_strip_zoom() {
    let pitch = cell_pitch(THUMB_BOX, 1.0);
    let bands = vec![
        Band {
            id: OverlayId(1),
            name: "A".into(),
            range: 0..4,
            selected: false,
            row: 0,
        },
        Band {
            id: OverlayId(2),
            name: "B".into(),
            range: 5..8,
            selected: false,
            row: 0,
        },
    ];
    let x = (6.0 + 0.5) * pitch;
    let y = BAND_H * 0.5;
    assert_eq!(band_at(&bands, x, y, pitch), Some(OverlayId(2)));
    // Half the zoom: the thumbnails halve but the gaps between them do
    // not, so the same x is over frame 12, past every band.
    assert_eq!(band_at(&bands, x, y, cell_pitch(THUMB_BOX, 0.5)), None);
    // Double: the same x is over frame 3, inside band A.
    assert_eq!(
        band_at(&bands, x, y, cell_pitch(THUMB_BOX, 2.0)),
        Some(OverlayId(1))
    );
}

/// The divider shown while dragging promises the frames will land right
/// next to a specific neighbor; `move_target_for_set` is what turns that
/// promise into the post-removal index `Document::move_frames_to` needs.
#[test]
fn move_target_for_set_lands_where_the_divider_promised() {
    // Dragging frame 0 to the gap right after itself is a no-op.
    assert_eq!(move_target_for_set(&[0], 1), 0);
    // Forward, past several frames still in the way after 0 moves out.
    assert_eq!(move_target_for_set(&[0], 3), 2);
    // Backward: nothing before the target gap moved, so it is the
    // final index outright.
    assert_eq!(move_target_for_set(&[3], 1), 1);
    // Past the last frame of 5.
    assert_eq!(move_target_for_set(&[0], 5), 4);
    // A gappy selection of three: the gap sits before all of them, so
    // the whole run slides down by three.
    assert_eq!(move_target_for_set(&[2, 5, 7], 1), 1);
    // …and after two of them: the run lands two earlier than the gap.
    assert_eq!(move_target_for_set(&[2, 5, 7], 6), 4);
}

/// End-to-end through the real `Document::move_frames_to`: whatever gap
/// the divider is drawn at, the dragged frames end up adjacent to
/// exactly the neighbor the divider promised, dragging either direction
/// and as a multi-frame selection.
#[test]
fn move_target_for_set_round_trips_through_move_frames_to() {
    let ids_at = |d: &Document| -> Vec<u8> {
        d.frames
            .iter()
            .map(|f| f.pixels.get_pixel(0, 0)[0])
            .collect()
    };

    // [0,1,2,3,4], drag 0 to the gap before 3 (gap 3): lands just before it.
    let mut d = doc(5);
    d.move_frames_to(&[0], move_target_for_set(&[0], 3));
    assert_eq!(ids_at(&d), vec![1, 2, 0, 3, 4]);

    // Same document, drag 3 to the gap before 1 (gap 1): lands just before it.
    let mut d = doc(5);
    d.move_frames_to(&[3], move_target_for_set(&[3], 1));
    assert_eq!(ids_at(&d), vec![0, 3, 1, 2, 4]);

    // Drag 0 to the gap past the last frame: lands at the very end.
    let mut d = doc(5);
    d.move_frames_to(&[0], move_target_for_set(&[0], 5));
    assert_eq!(ids_at(&d), vec![1, 2, 3, 4, 0]);

    // A three-frame selection dropped between frames 1 and 2.
    let mut d = doc(8);
    d.move_frames_to(&[2, 5, 7], move_target_for_set(&[2, 5, 7], 2));
    assert_eq!(ids_at(&d), vec![0, 1, 2, 5, 7, 3, 4, 6]);
}

/// "Move earlier"/"Move later" swaps the selection with the one
/// unselected frame beside it: the gap aims just before (or after) that
/// neighbour, and refuses at either edge of the timeline.
#[test]
fn selection_nudge_gap_swaps_with_the_beside_neighbour() {
    // Contiguous run 2..5 of 8: earlier aims before frame 1.
    assert_eq!(selection_nudge_gap(&[2, 3, 4], true, 8), Some(1));
    // Later aims past frame 4, the neighbour after the run.
    assert_eq!(selection_nudge_gap(&[2, 3, 4], false, 8), Some(6));
    // A single frame nudges exactly like the old move-by-one.
    assert_eq!(selection_nudge_gap(&[3], true, 8), Some(2));
    assert_eq!(selection_nudge_gap(&[3], false, 8), Some(5));
    // Already at an edge: nothing to swap with.
    assert_eq!(selection_nudge_gap(&[0, 1], true, 8), None);
    assert_eq!(selection_nudge_gap(&[6, 7], false, 8), None);
    // Every frame selected: the nudge would be a no-op anyway.
    assert_eq!(
        selection_nudge_gap(&(0..8).collect::<Vec<_>>(), true, 8),
        None
    );
    assert_eq!(
        selection_nudge_gap(&(0..8).collect::<Vec<_>>(), false, 8),
        None
    );
}

/// The full drag path at model level: the selection moves as one run,
/// follows itself to its new home, and a drag in place does nothing.
#[test]
fn move_picked_moves_the_whole_selection_and_follows_it() {
    let ids = |app: &App| -> Vec<u8> {
        app.editor
            .doc
            .frames
            .iter()
            .map(|f| f.pixels.get_pixel(0, 0)[0])
            .collect()
    };

    // In place means the picked frames already form the run `to` starts:
    // [2,3,4] dropped at gap 2 would land exactly where they are.
    let mut app = app_with(doc(8), ScopeChoice::Range, vec![2, 3, 4], 3);
    assert!(app.move_picked(&[2, 3, 4], 2).is_none(), "already in place");

    // Dropping a gappy selection [2,5,7] between 0 and 1.
    let mut app = app_with(doc(8), ScopeChoice::Range, vec![2, 5, 7], 5);
    assert!(app.move_picked(&[2, 5, 7], 1).is_some());
    assert_eq!(ids(&app), vec![0, 2, 5, 7, 1, 3, 4, 6]);
    assert_eq!(
        app.selection,
        vec![1, 2, 3],
        "the selection is now the contiguous run at its new home"
    );
    assert_eq!(
        app.playhead, 2,
        "the frame the playhead was on (5) sits at index 2"
    );
    assert_eq!(app.scope, ScopeChoice::Range);

    // A one-frame drag with nothing selected moves just that frame.
    let mut app = app_with(doc(8), ScopeChoice::ThisFrame, Vec::new(), 4);
    assert!(app.move_picked(&[4], 1).is_some());
    assert_eq!(ids(&app), vec![0, 4, 1, 2, 3, 5, 6, 7]);
    assert!(app.selection.is_empty(), "no selection to carry");
    assert_eq!(app.playhead, 1);
}

/// "Move earlier" end to end: the run swaps with the neighbour before
/// it and the selection keeps covering the same frames.
#[test]
fn move_selection_earlier_swaps_the_block_with_its_neighbour() {
    let ids = |app: &App| -> Vec<u8> {
        app.editor
            .doc
            .frames
            .iter()
            .map(|f| f.pixels.get_pixel(0, 0)[0])
            .collect()
    };
    let mut app = app_with(doc(8), ScopeChoice::Range, vec![2, 3, 4], 3);
    let gap = selection_nudge_gap(&[2, 3, 4], true, 8).unwrap();
    let to = move_target_for_set(&[2, 3, 4], gap);
    assert!(app.move_picked(&[2, 3, 4], to).is_some());
    assert_eq!(ids(&app), vec![0, 2, 3, 4, 1, 5, 6, 7]);
    assert_eq!(app.selection, vec![1, 2, 3]);
}

/// Ctrl+wheel / Ctrl+Up-Down feed `Msg::StripZoom(factor)`: the factor
/// multiplies, the result clamps to the bounds, and `0.0` resets to 1x.
#[test]
fn strip_zoom_multiplies_clamps_and_resets() {
    let step = STRIP_ZOOM_STEP;
    assert!(
        (next_strip_zoom(1.0, step) - step).abs() < 1e-9,
        "one step in"
    );
    assert!(next_strip_zoom(1.0, 1.0 / step) < 1.0, "one step out");

    let mut z = 1.0;
    for _ in 0..50 {
        z = next_strip_zoom(z, step);
    }
    assert_eq!(z, STRIP_ZOOM_MAX, "clamped to the max");
    for _ in 0..99 {
        z = next_strip_zoom(z, 1.0 / step);
    }
    assert_eq!(z, STRIP_ZOOM_MIN, "clamped to the min");

    assert_eq!(next_strip_zoom(z, 0.0), 1.0, "0.0 resets to 1x");
}

/// The bands `DrawingArea` is exactly as wide as the zoomed strip, so its
/// per-frame columns stay lined up with the thumbnails above them. Only
/// the thumbnails zoom: the gap between two cells is the strip `Box`'s
/// spacing, a fixed number of pixels, so the width is not linear in the
/// zoom the way it was assumed to be.
#[test]
fn strip_width_zooms_the_thumbnails_and_not_the_gaps() {
    let width = |zoom| strip_width(10, cell_pitch(THUMB_BOX, zoom));
    assert_eq!(width(1.0), 10 * (THUMB_BOX + THUMB_SPACING));
    assert_eq!(width(2.0), 10 * (2 * THUMB_BOX + THUMB_SPACING));
    assert_eq!(width(0.5), 10 * (THUMB_BOX / 2 + THUMB_SPACING));
}

/// Regression: the bands under the strip are drawn a `cell_pitch` per
/// frame, so a cell has to really sit there at every zoom. A `GtkPicture`
/// never measures smaller than its paintable — a size request alone left
/// the cells at 1x below it — and a four-digit frame number could measure
/// wider than the thumbnail over it. Either slid the whole band legend
/// right of the frames it annotates, a little further with every column.
fn strip_cells_sit_at_the_pitch_the_bands_are_drawn_at() {
    // A portrait frame's thumbnail: the narrow cell, and the one a
    // `THUMB_BOX`-wide pitch was wrong about even at 1x.
    let thumb = image::RgbaImage::new(41, THUMB_BOX as u32);
    let strip = gtk::Box::new(gtk::Orientation::Horizontal, THUMB_SPACING);
    for i in 0..6 {
        let picture = gtk::Picture::new();
        let cell = gtk::Box::new(gtk::Orientation::Vertical, 2);
        cell.append(&picture);
        // Four digits: wider than the thumbnail once the strip zooms out.
        let label = gtk::Label::new(Some(&format!("{}", 1000 + i)));
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        cell.append(&label);
        strip.append(&cell);
    }
    let window = gtk::Window::new();
    window.set_child(Some(&strip));
    let mut textures = ThumbTextures::new();
    for zoom in [STRIP_ZOOM_MIN, 0.5, 1.0, 2.0, STRIP_ZOOM_MAX] {
        for cell in thumb_children(&strip) {
            let picture = cell.first_child().and_downcast::<gtk::Picture>().unwrap();
            let texture = thumb_texture(&mut textures, 0, &thumb, zoom);
            set_thumb_zoom(&picture, &texture, thumb.dimensions(), zoom);
        }
        let (_, width, _, _) = strip.measure(gtk::Orientation::Horizontal, -1);
        strip.allocate(width, 400, -1, None);
        let pitch = cell_pitch(thumb.width() as i32, zoom);
        for (i, cell) in thumb_children(&strip).enumerate() {
            let bounds = cell.compute_bounds(&strip).expect("laid out");
            assert_eq!(
                (bounds.x() as f64, bounds.width() as f64),
                (i as f64 * pitch, pitch - THUMB_SPACING as f64),
                "cell {i} at {zoom}x"
            );
        }
    }
    window.set_child(None::<&gtk::Widget>);
}

/// The sidebar heading swaps "Frame" for a summary of which frames are
/// in scope once more than one is — the panel this titles trades
/// per-frame overlay editing for a scoped delay edit at that point, so
/// the heading has to say which frames the edit will land on.
#[test]
fn frame_scope_summary_names_the_range_or_the_numbers() {
    assert_eq!(
        frame_scope_summary(ScopeChoice::ThisFrame, &[3]),
        "Frame",
        "a single frame in scope reads as the plain frame view"
    );
    assert_eq!(
        frame_scope_summary(ScopeChoice::Range, &[]),
        "Frame",
        "an empty scope falls back the same way"
    );
    assert_eq!(
        frame_scope_summary(ScopeChoice::AllFrames, &(0..12).collect::<Vec<_>>()),
        "All 12 frames selected"
    );
    assert_eq!(
        frame_scope_summary(ScopeChoice::Range, &[2, 3, 4, 5]),
        "Frames 3–6 selected",
        "a contiguous run reads as a range, 1-based and inclusive"
    );
    assert_eq!(
        frame_scope_summary(ScopeChoice::Range, &[1, 4, 7]),
        "Frames 2, 5, 8 selected",
        "a gappy pick small enough to read spells out the numbers"
    );
    assert_eq!(
        frame_scope_summary(ScopeChoice::Range, &(0..20).step_by(2).collect::<Vec<_>>()),
        "10 frames selected",
        "too many gappy picks to list falls back to a count"
    );
}

/// A still image decodes through the `image` crate as one frame; anything
/// that can animate has to reach a decoder that sees every frame.
#[test]
fn still_images_are_the_ones_with_no_motion_to_decode() {
    for ext in ["png", "jpg", "jpeg", "PNG", "bmp", "tiff", "webp.png"] {
        assert!(
            pipeline::is_still_image(Path::new(&format!("x.{ext}"))),
            "{ext} is a still image"
        );
    }
    for ext in ["gif", "mp4", "mov", "webm", "webp", "mkv", "apng", ""] {
        assert!(
            !pipeline::is_still_image(Path::new(&format!("x.{ext}"))),
            "{ext} must reach a multi-frame decoder"
        );
    }
    assert!(!pipeline::is_still_image(Path::new("noextension")));
}

/// Regression: a band's row used to be its index in the overlay list, so
/// the tenth overlay drew ten rows down even when the nine below it covered
/// entirely different frames.
#[test]
fn bands_pack_side_by_side_when_their_frames_do_not_overlap() {
    assert_eq!(
        pack_rows(&[0..5, 5..10, 10..15]),
        vec![0, 0, 0],
        "a chain shares one row"
    );
    assert_eq!(
        pack_rows(&[0..10, 2..4, 3..8]),
        vec![0, 1, 2],
        "overlaps stack"
    );
    // Touching ranges are not overlapping ranges: 0..5 ends where 5..10 starts.
    assert_eq!(pack_rows(&[0..5, 3..9, 5..7, 9..12]), vec![0, 1, 0, 0]);
    assert!(pack_rows(&[]).is_empty());
}

/// An overlay edit lands where the scope reaches into the overlay's own
/// range; when the scope does not reach the frame on screen, that frame
/// wins. This is what makes a one-frame drag stay a one-frame drag.
#[test]
fn an_overlay_edit_lands_where_the_scope_reaches() {
    // All-frames scope over a full-range overlay: everything, in place.
    assert_eq!(overlay_edit_span(0..10, 0..10, 3), 0..10);
    // This-frame scope inside the overlay: the one frame.
    assert_eq!(overlay_edit_span(0..10, 4..5, 4), 4..5);
    // A range scope reaching partway: the overlap.
    assert_eq!(overlay_edit_span(0..10, 5..20, 6), 5..10);
    // Scope elsewhere, playhead still on the overlay: the frame on screen.
    assert_eq!(overlay_edit_span(0..10, 7..9, 2), 2..3);
    // Scope elsewhere and the overlay nowhere near: nothing to edit.
    assert_eq!(overlay_edit_span(7..9, 0..2, 5), 0..0);
}

/// The canvas maps widget pixels to image pixels; every gesture and every
/// handle depends on it agreeing with `ContentFit::ScaleDown`.
#[test]
fn the_canvas_mapping_centres_and_never_enlarges() {
    // A picture smaller than its widget is centred at 1:1, not blown up.
    let (scale, ox, oy) = canvas_map((400.0, 300.0), (100.0, 100.0));
    assert_eq!(scale, 1.0);
    assert_eq!((ox, oy), (150.0, 100.0));

    // A larger one is scaled down to fit the tighter axis and letterboxed.
    let (scale, ox, oy) = canvas_map((400.0, 300.0), (800.0, 400.0));
    assert_eq!(scale, 0.5);
    assert_eq!((ox, oy), (0.0, 50.0));

    // Round trip: a click at the centre of the widget is the centre pixel.
    let point = |x: f64| (x - ox) / scale;
    assert_eq!(point(200.0), 400.0);

    // Degenerate sizes must not divide by zero or hand back a NaN.
    let (scale, ..) = canvas_map((400.0, 300.0), (0.0, 0.0));
    assert!(scale.is_finite() && scale > 0.0);
}

#[test]
fn handles_sit_on_the_corners_and_resizing_pins_the_opposite_one() {
    let t = Transform::at(10.0, 20.0, 100.0, 50.0);
    assert!(contains(t, 10.0, 20.0) && contains(t, 110.0, 70.0));
    assert!(!contains(t, 9.0, 20.0));

    assert_eq!(handle_at(t, 10.0, 20.0, 4.0), Some(0), "top left");
    assert_eq!(handle_at(t, 110.0, 70.0, 4.0), Some(3), "bottom right");
    assert_eq!(
        handle_at(t, 60.0, 45.0, 4.0),
        None,
        "the middle is a move, not a resize"
    );

    // Dragging the top left by (5, 5) moves the origin and shrinks the box;
    // the bottom right corner does not budge.
    let dragged = resize_corner(t, 0, 5.0, 5.0, false, false);
    assert_eq!((dragged.x, dragged.y), (15.0, 25.0));
    assert_eq!(
        (dragged.x + dragged.w, dragged.y + dragged.h),
        (110.0, 70.0)
    );

    let dragged = resize_corner(t, 3, -20.0, -10.0, false, false);
    assert_eq!((dragged.x, dragged.y), (10.0, 20.0), "the origin is pinned");
    assert_eq!((dragged.w, dragged.h), (80.0, 40.0));
}

/// The two modifiers Impasto's grip hint promises. Shift takes the larger
/// of the two scale factors; Ctrl pins the centre instead of a corner.
#[test]
fn shift_keeps_the_aspect_ratio_and_ctrl_resizes_about_the_center() {
    let t = Transform::at(0.0, 0.0, 100.0, 50.0);

    // The x drag doubles the width, the y drag would only grow the height
    // by a tenth, so the aspect clamp takes the doubling for both.
    let square = resize_corner(t, 3, 100.0, 5.0, true, false);
    assert_eq!((square.w, square.h), (200.0, 100.0));
    assert_eq!(
        (square.x, square.y),
        (0.0, 0.0),
        "the origin is still pinned"
    );

    // From the centre the drag counts on both sides, and the centre holds.
    let centred = resize_corner(t, 3, 25.0, 10.0, false, true);
    assert_eq!((centred.w, centred.h), (150.0, 70.0));
    assert_eq!(centred.center(), t.center());

    // Dragging a corner past its opposite flips rather than clamping.
    let flipped = resize_corner(t, 3, -150.0, 0.0, false, false);
    assert!(flipped.w < 0.0, "{flipped:?}");
}

/// A rotated overlay has to be grabbed where it looks, and a grip drag has
/// to leave the opposite corner on the pixel it was already on.
#[test]
fn a_rotated_overlay_is_hit_and_resized_in_its_own_frame() {
    let mut t = Transform::at(10.0, 45.0, 80.0, 10.0);
    t.angle = std::f32::consts::FRAC_PI_2; // a quarter turn: wide becomes tall

    // The un-rotated box covers (10, 45); after the turn that point is
    // outside it and the box reaches up and down through the centre.
    let (cx, cy) = t.center();
    assert!(contains(t, cx, cy - 30.0), "tall after the turn");
    assert!(!contains(t, cx - 30.0, cy), "no longer wide");

    // The bottom-right grip is where the drawing puts it.
    let grip = oriented_corners(t)[3];
    assert_eq!(handle_at(t, grip.0, grip.1, 2.0), Some(3));

    // Drag it and the top-left grip stays on its pixel.
    let anchor_before = oriented_corners(t)[0];
    let resized = pin_anchor(t, resize_corner(t, 3, 20.0, 6.0, false, false), 3, false);
    let anchor_after = oriented_corners(resized)[0];
    assert!(
        (anchor_before.0 - anchor_after.0).abs() < 0.01
            && (anchor_before.1 - anchor_after.1).abs() < 0.01,
        "{anchor_before:?} vs {anchor_after:?}"
    );
    assert_eq!(resized.angle, t.angle, "a resize is not a rotation");
}

/// Regression: the shortcuts controller ran in the bubble phase, so the
/// widget holding the focus handled the key first. GTK activates a focused
/// button on Space and puts the initial focus on the first one it finds, so
/// Space straight after opening a document added an overlay instead of
/// playing. Only the capture phase gets there first.
fn the_shortcuts_controller_runs_before_the_focused_widget() {
    assert_eq!(
        shortcuts_controller().propagation_phase(),
        gtk::PropagationPhase::Capture,
        "a bubble-phase controller loses Space to whatever holds the focus"
    );
    assert_ne!(
        gtk::EventControllerKey::new().propagation_phase(),
        gtk::PropagationPhase::Capture,
        "the default is what bit us, so the setter is doing real work"
    );
}

/// The capture phase means every keystroke passes here first, so the widgets
/// that genuinely own their keys have to be handed them back — otherwise
/// typing "a" in a caption inserts an arrow.
fn a_text_entry_keeps_its_own_keystrokes_but_a_button_does_not() {
    assert!(focus_owns_keys(Some(gtk::Entry::new().upcast_ref())));
    assert!(focus_owns_keys(Some(gtk::TextView::new().upcast_ref())));
    assert!(focus_owns_keys(Some(
        gtk::SpinButton::with_range(0.0, 1.0, 1.0).upcast_ref()
    )));
    assert!(!focus_owns_keys(Some(
        gtk::Button::with_label("T").upcast_ref()
    )));
    assert!(
        !focus_owns_keys(None),
        "nothing focused is the shortcuts' case"
    );
}

/// Clicking a toolbar button should not park the focus on it either, so the
/// keys the canvas cares about keep working after a tool is picked.
fn a_clicked_toolbar_button_does_not_keep_the_keyboard_focus() {
    let button = gtk::Button::with_label("T");
    assert!(button.gets_focus_on_click(), "GTK's default is what bit us");
    no_focus_steal(&button);
    assert!(!button.gets_focus_on_click());
    assert!(button.is_focusable(), "Tab still reaches it");
}

/// The move cursor is the only hint that an overlay is draggable at all, so
/// it has to win over "default" everywhere the press handler would start a
/// drag — and lose to the grip and rotate glyphs, which it sits under.
#[test]
fn the_body_of_any_overlay_offers_the_move_cursor() {
    let selected = Transform::at(10.0, 10.0, 40.0, 20.0);
    let other = Transform::at(100.0, 60.0, 30.0, 30.0);
    let state = CanvasState {
        image: (200.0, 120.0),
        selected: Some(selected),
        movable: vec![selected, other],
        ..Default::default()
    };
    let at = |x, y| hover_cursor(&state, x, y, 4.0, false);

    assert_eq!(at(30.0, 20.0), "move", "inside the selected overlay");
    assert_eq!(at(115.0, 75.0), "move", "inside an unselected one too");
    assert_eq!(at(80.0, 40.0), "default", "bare canvas between them");

    // A corner of the selection outranks its own body.
    assert_eq!(at(10.0, 10.0), "nw-resize");
    // And the rotate modifier outranks both.
    assert_eq!(hover_cursor(&state, 30.0, 20.0, 4.0, true), "rotate");

    // An unselected overlay has no grips, so its corner is still a move,
    // and rotate needs a selection to act on.
    assert_eq!(at(100.0, 60.0), "move");
    let nothing_selected = CanvasState {
        selected: None,
        movable: vec![selected, other],
        ..Default::default()
    };
    assert_eq!(
        hover_cursor(&nothing_selected, 115.0, 75.0, 4.0, true),
        "move",
        "the rotate modifier over nothing selected still just moves"
    );
}

/// The rotate cursor is a texture, not a CSS name, so nothing catches a
/// missing or unreadable PNG until the pointer is over a grip and the glyph
/// silently falls back to "grab".
fn the_rotate_cursor_texture_decodes() {
    let texture = gdk::Texture::from_bytes(&glib::Bytes::from_static(ROTATE_CURSOR))
        .expect("resources/rotate-handle.png decodes");
    assert_eq!(
        (texture.width(), texture.height()),
        (32, 32),
        "re-render it at 32x32; see resources/README.md"
    );
    assert!(rotate_cursor().is_some());
}

/// The gresource is embedded and registered by hand (no build.rs step),
/// so a stale or malformed `icons.gresource` would otherwise only show up
/// as tool buttons silently falling back to GTK's "missing image" glyph.
fn the_tool_icons_resolve() {
    register_tool_icons();
    let theme = gtk::IconTheme::for_display(&gdk::Display::default().expect("display"));
    for icon in [
        "tool-text-symbolic",
        "tool-rect-symbolic",
        "tool-ellipse-symbolic",
        "tool-arrow-symbolic",
        "tool-crop-symbolic",
    ] {
        assert!(theme.has_icon(icon), "missing bundled icon: {icon}");
    }
}

/// The resize glyph follows the content round, so a corner of a rotated box
/// still points along its own edge (Impasto's `ResizeCursors`).
#[test]
fn the_resize_cursor_turns_with_the_box() {
    assert_eq!(corner_cursor(0, 0.0), "nw-resize", "top left, axis aligned");
    assert_eq!(corner_cursor(3, 0.0), "se-resize", "bottom right");
    // A quarter turn clockwise carries the top-left corner round to the top right.
    assert_eq!(corner_cursor(0, std::f32::consts::FRAC_PI_2), "ne-resize");
    // Half a turn puts it opposite itself, and the glyph is symmetric.
    assert_eq!(corner_cursor(0, std::f32::consts::PI), "se-resize");
    // Corners never borrow an edge glyph, whatever the angle.
    for corner in 0..4 {
        for step in 0..24 {
            let cursor = corner_cursor(corner, step as f32 * 0.3);
            assert!(cursor.len() > "n-resize".len(), "{cursor} is an edge glyph");
        }
    }
}

/// The busy gates are the one thing standing between a running worker and
/// a corrupted frame-index landing, so the classification is spelled out
/// rather than trusted to the matches! arms staying in sync by eye.
#[test]
fn busy_gate_classification_covers_every_variant() {
    let press = || Msg::CanvasPress {
        x: 0.0,
        y: 0.0,
        scale: 1.0,
        state: gdk::ModifierType::empty(),
    };

    // Frame-moving edits: blocked in every kind of work.
    for msg in [
        Msg::Load(PathBuf::new()),
        Msg::LoadInto(PathBuf::new(), 0),
        Msg::Undo,
        Msg::Redo,
        Msg::DeleteSelection,
        Msg::FrameOp(FrameOp::Delete),
        Msg::MoveSelection { earlier: true },
        Msg::MoveSelectionTo { from: 0, gap: 2 },
        Msg::SetScopeDelay(5),
        Msg::SetAllFramesDelay(5),
        Msg::ApplyCrop,
        Msg::CropAll(0, 0, 1, 1),
        Msg::ApplyZoom,
        Msg::ApplyZoomAll,
        Msg::ApplyShrink,
        Msg::DropEveryNth(2),
        Msg::SmartDrop(30),
        Msg::Resize(10, 10),
        Msg::SpliceFrames {
            at: 0,
            frames: Vec::new(),
            fit: FitMode::Stretch,
        },
        Msg::MoveFrame(0, 1),
    ] {
        assert!(msg.changes_frames(), "{msg:?} must wait");
        assert!(msg.requires_idle(), "{msg:?} must wait");
    }

    // Snapshot/dialog openers: blocked too, or the follow-up edit lands
    // against a stale canvas.
    for msg in [
        Msg::Open,
        Msg::Export,
        Msg::CropAllDialog,
        Msg::ResizeDialog,
        Msg::DelayAllDialog,
        Msg::ImportInto(AT_END),
    ] {
        assert!(!msg.changes_frames(), "{msg:?} does not move frames itself");
        assert!(msg.requires_idle(), "{msg:?} must wait");
    }

    // Overlay-only edits: live during resize/zoom (the Worked handler
    // scales them in), blocked only while an import would discard them.
    for msg in [
        Msg::AddOverlay(Tool::Text),
        Msg::EditText("hi".into()),
        Msg::SetOverlayProp(OverlayProp::TextSize(12.0)),
        Msg::ToggleCropTool,
        press(),
        Msg::CanvasDrag {
            x: 0.0,
            y: 0.0,
            state: gdk::ModifierType::empty(),
        },
        Msg::CanvasRelease,
    ] {
        assert!(
            !msg.changes_frames(),
            "{msg:?} stays live during frame work"
        );
        assert!(msg.edits_document(), "{msg:?} must wait during import");
    }

    // Pure view/transport messages: never blocked. `MoveFrameDialog` only
    // reads the frame count, which frame work never changes, so it needs
    // no more guarding than opening any other frame's context menu does;
    // `FrameDelayDialog` only opens a dialog whose answer is gated itself.
    for msg in [
        Msg::Tick,
        Msg::Seek(0),
        Msg::TogglePlay,
        Msg::Toast(String::new()),
        Msg::SetScope(ScopeChoice::AllFrames),
        Msg::SelectOverlay(None),
        Msg::MoveFrameDialog(0),
        Msg::FrameDelayDialog(0),
    ] {
        assert!(
            !msg.changes_frames() && !msg.requires_idle() && !msg.edits_document(),
            "{msg:?} passes through"
        );
    }
}

/// Ctrl picks frames one at a time and Shift picks the run between the
/// anchor and here, in either direction. Both were previously a single
/// growing range, so Ctrl could not skip a frame and Shift could not shrink.
#[test]
fn ctrl_toggles_single_frames_and_shift_takes_a_run_either_way() {
    let mut picked = Vec::new();
    toggle_frame(&mut picked, 2);
    toggle_frame(&mut picked, 7);
    toggle_frame(&mut picked, 4);
    assert_eq!(picked, vec![2, 4, 7], "a set, sorted, with the gaps kept");
    toggle_frame(&mut picked, 4);
    assert_eq!(
        picked,
        vec![2, 7],
        "clicking a picked frame takes it back out"
    );

    // Shift measures from the anchor, so clicking before it is a run too.
    assert_eq!(run_between(5, 8), vec![5, 6, 7, 8]);
    assert_eq!(
        run_between(5, 2),
        vec![2, 3, 4, 5],
        "backwards is the same run"
    );
    assert_eq!(run_between(3, 3), vec![3]);

    // The scope keeps the gaps, and only widens to span them for an
    // overlay, which has to be contiguous.
    let scope = Scope::Frames(vec![2, 7]);
    assert_eq!(scope.resolve(0, 10), vec![2, 7]);
    assert_eq!(scope.span(0, 10), 2..8);
}

#[test]
fn resize_completion_keeps_active_canvas_work_in_image_coordinates() {
    let mut drag = Some(Drag {
        mode: DragMode::Move,
        from: (10.0, 20.0),
        origin: Transform::at(5.0, 10.0, 20.0, 30.0),
        current: Transform::at(7.0, 12.0, 20.0, 30.0),
        snap_reach: 8.0,
        moved: true,
    });
    let mut crop = Some((10.0, 20.0, 30.0, 40.0));

    scale_in_flight_canvas(&mut drag, &mut crop, 0.5, 2.0, 0.0, 0.0);

    let drag = drag.unwrap();
    assert_eq!(drag.from, (5.0, 40.0));
    assert_eq!(drag.origin, Transform::at(2.5, 20.0, 10.0, 60.0));
    assert_eq!(drag.current, Transform::at(3.5, 24.0, 10.0, 60.0));
    assert_eq!(crop, Some((5.0, 40.0, 15.0, 80.0)));
}

/// A crop landing mid-drag moves the origin instead of scaling it: the
/// canvas keeps its resolution, but pixel (0, 0) is now somewhere else.
#[test]
fn crop_completion_shifts_active_canvas_work_by_the_kept_origin() {
    let mut drag = Some(Drag {
        mode: DragMode::Move,
        from: (10.0, 20.0),
        origin: Transform::at(5.0, 10.0, 20.0, 30.0),
        current: Transform::at(7.0, 12.0, 20.0, 30.0),
        snap_reach: 8.0,
        moved: true,
    });
    let mut crop = Some((10.0, 20.0, 30.0, 40.0));

    scale_in_flight_canvas(&mut drag, &mut crop, 1.0, 1.0, -4.0, -6.0);

    let drag = drag.unwrap();
    assert_eq!(drag.from, (6.0, 14.0));
    assert_eq!(drag.origin, Transform::at(1.0, 4.0, 20.0, 30.0));
    assert_eq!(drag.current, Transform::at(3.0, 6.0, 20.0, 30.0));
    assert_eq!(crop, Some((6.0, 14.0, 30.0, 40.0)));
}

#[test]
fn full_canvas_crop_is_not_an_edit() {
    assert!(!crop_changes_canvas((100, 80), (0, 0, 100, 80)));
    assert!(!crop_changes_canvas((100, 80), (0, 0, 500, 500)));
    assert!(crop_changes_canvas((100, 80), (1, 0, 99, 80)));
    assert!(crop_changes_canvas((100, 80), (0, 0, 99, 80)));
    assert!(!crop_changes_canvas((0, 0), (0, 0, 1, 1)));
}

#[test]
fn canvas_rects_intersect_the_image_before_rounding() {
    assert_eq!(
        normalize_canvas_rect((100, 80), (-20.0, -10.0, 50.0, 30.0)),
        Some((0, 0, 30, 20))
    );
    assert_eq!(
        normalize_canvas_rect((100, 80), (10.2, 20.2, 5.2, 4.2)),
        Some((10, 20, 6, 5))
    );
    assert_eq!(
        normalize_canvas_rect((100, 80), (-20.0, 0.0, 10.0, 10.0)),
        None
    );
}
#[test]
fn resize_budget_uses_checked_rgba_size() {
    assert!(resize_fits_budget(100, 100, 10, 400_000));
    assert!(!resize_fits_budget(100, 100, 11, 400_000));
    assert!(!resize_fits_budget(
        u32::MAX,
        u32::MAX,
        usize::MAX,
        usize::MAX
    ));
}

/// The aspect lock links the two resize fields: editing one width of a
/// 16:9 canvas recomputes the other, and extremes clamp to the range.
#[test]
fn paired_dimension_keeps_the_canvas_ratio() {
    assert_eq!(paired_dimension(960, 480, 270), 540);
    assert_eq!(paired_dimension(540, 270, 480), 960);
    assert_eq!(paired_dimension(1, 480, 270), 16);
    assert_eq!(paired_dimension(u32::MAX, 480, 270), 8192);
}

/// The dialog's memory estimate counts checked RGBA bytes, so an absurd
/// canvas reports unrepresentable instead of wrapping around to tiny.
#[test]
fn resize_bytes_counts_checked_rgba() {
    assert_eq!(resize_bytes(480, 270, 300), Some(155_520_000));
    assert_eq!(resize_bytes(u32::MAX, u32::MAX, usize::MAX), None);
}

/// A resize must fit the per-operation budget with its new frames and the
/// total with old plus new, since undo keeps the old ones while the
/// worker holds the new. 480×270×3 is 1,555,200 bytes a side.
#[test]
fn resize_budget_checks_output_and_total() {
    assert!(resize_fits_budgets(
        480,
        270,
        480,
        270,
        3,
        usize::MAX,
        usize::MAX
    ));
    assert!(
        !resize_fits_budgets(480, 270, 8192, 8192, 300, 1, usize::MAX),
        "an output over the operation budget refuses"
    );
    assert!(
        !resize_fits_budgets(480, 270, 480, 270, 3, usize::MAX, 2_000_000),
        "old plus new over the total refuses"
    );
    assert!(
        !resize_fits_budgets(
            u32::MAX,
            u32::MAX,
            u32::MAX,
            u32::MAX,
            usize::MAX,
            usize::MAX,
            usize::MAX
        ),
        "overflow refuses"
    );
}

/// Regression: the sidebar sets its widgets from the model, and each setter
/// fires its own notify handler. Without this check, selecting an overlay
/// pushed an undo step for every property it has.
#[test]
fn echoing_a_property_back_unchanged_is_not_an_edit() {
    let text = OverlayKind::Text(TextOverlay {
        text: "hi".into(),
        font: "Sans Bold".into(),
        size_px: 32.0,
        color: [255, 255, 255, 255],
        outline: Some(([0, 0, 0, 255], 2.0)),
        align: TextAlign::Center,
        antialias: true,
    });
    assert!(!OverlayProp::Font("Sans Bold".into()).changes(&text));
    assert!(OverlayProp::Font("Serif".into()).changes(&text));
    assert!(!OverlayProp::TextSize(32.0).changes(&text));
    assert!(OverlayProp::TextSize(33.0).changes(&text));
    assert!(!OverlayProp::Outline(Some(([0, 0, 0, 255], 2.0))).changes(&text));
    assert!(
        OverlayProp::Outline(None).changes(&text),
        "turning it off is a change"
    );
    assert!(!OverlayProp::Align(TextAlign::Center).changes(&text));
    assert!(OverlayProp::Align(TextAlign::Justify).changes(&text));
    assert!(!OverlayProp::Antialias(true).changes(&text));
    assert!(OverlayProp::Antialias(false).changes(&text));

    let shape = OverlayKind::Shape(ShapeOverlay {
        shape: Shape::Rect,
        fill: None,
        stroke: Some(([255, 60, 60, 255], 3.0)),
    });
    assert!(!OverlayProp::Fill(None).changes(&shape));
    assert!(OverlayProp::Fill(Some([1, 2, 3, 255])).changes(&shape));
    // A text property offered to a shape is not a change to anything.
    assert!(!OverlayProp::Font("Serif".into()).changes(&shape));
}

/// The context menu's shortcuts are read off the keymap, so a rebind moves
/// them; GTK takes them from the item's "accel" attribute, which is the
/// only channel it has for a shortcut owned by our own key controller.
#[test]
fn the_frame_menu_shows_what_the_keymap_binds() {
    let accel_of = |item: &gio::MenuItem| {
        item.attribute_value("accel", None)
            .and_then(|value| value.str().map(str::to_string))
    };
    let mut keys = Keymap::default();
    let cut = frame_item("Cut this frame", "win.frame-cut", &keys, Action::FrameCut);
    assert_eq!(accel_of(&cut).as_deref(), Some("<Control>x"));

    keys.set(
        Action::FrameCut,
        vec![Chord::parse("Ctrl+Shift+X").unwrap()],
    );
    let cut = frame_item("Cut this frame", "win.frame-cut", &keys, Action::FrameCut);
    assert_eq!(
        accel_of(&cut).as_deref(),
        Some("<Control><Shift>x"),
        "a rebind moves the menu's hint with it"
    );

    // Deleting frames ships unbound: no chord means no attribute, rather
    // than an empty one GTK would try to parse.
    let delete = frame_item(
        "Delete this frame",
        "win.frame-delete",
        &keys,
        Action::FrameDelete,
    );
    assert_eq!(accel_of(&delete), None);
}

/// Regression: adding an overlay hung the app. `update_view` sets the
/// outline colour before the outline width, so the colour button's handler
/// read a width of zero and sent `Outline(None)` — which the model *did*
/// disagree with, so it was applied, which re-ran the sync, which sent the
/// pair back the other way, for ever. A sync must emit nothing at all.
/// GTK is single-threaded and `cargo test` is not, so every check that needs
/// a widget hangs off this one entry point. Split them back into separate
/// `#[test]`s and the suite segfaults on the second `gtk::init`.
#[test]
fn gtk_widget_regressions() {
    if gtk::init().is_err() {
        eprintln!("skipped: no display");
        return;
    }
    syncing_a_colour_and_width_pair_sends_nothing_back();
    a_typed_delay_commits_once_and_not_onto_the_next_frame();
    the_rotate_cursor_texture_decodes();
    the_tool_icons_resolve();
    the_shortcuts_controller_runs_before_the_focused_widget();
    a_text_entry_keeps_its_own_keystrokes_but_a_button_does_not();
    a_clicked_toolbar_button_does_not_keep_the_keyboard_focus();
    thumb_children_skips_the_frames_popover();
    the_layer_list_is_bounded_by_its_own_row_height();
    the_layer_list_bounds_survive_the_list_emptying();
    the_strip_viewport_does_not_chase_the_focus();
    strip_cells_sit_at_the_pitch_the_bands_are_drawn_at();
    the_fit_chooser_maps_one_radio_to_each_mode();
    the_resize_dialog_offers_cancel_and_apply();
    linked_resize_fields_track_each_other_live();
    linked_import_fields_stay_even_and_in_range();
    the_advanced_import_rows_explain_themselves_without_crowding();
    the_window_refuses_to_shrink_under_its_own_chrome();
}

/// Every (label, action) item in `model`, sections flattened, in order.
fn menu_items(model: &gio::MenuModel) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for i in 0..model.n_items() {
        if let Some(section) = model.item_link(i, "section") {
            out.extend(menu_items(&section));
            continue;
        }
        let attr = |name: &str| {
            model
                .item_attribute_value(i, name, None)
                .and_then(|v| v.str().map(str::to_string))
                .unwrap_or_default()
        };
        out.push((attr("label"), attr("action")));
    }
    out
}

/// The band context menu's two faces: each names one copy, aimed at the
/// selection or at the whole document, and the same delete under it.
/// Every item has to point at an action the bands really add — a name
/// nothing answers is an entry that greys out or does nothing, which the
/// code that wrote it cannot show. Needs no display: a menu model is not
/// a widget.
#[test]
fn the_band_menu_only_names_actions_the_bands_carry() {
    let (all_menu, selection_menu) = band_menus();
    assert_eq!(
        menu_items(all_menu.upcast_ref()),
        vec![
            (
                t("Copy overlay to all frames").to_string(),
                "overlay.copy-all".to_string()
            ),
            (
                t("Delete overlay").to_string(),
                "overlay.delete".to_string()
            ),
        ]
    );
    assert_eq!(
        menu_items(selection_menu.upcast_ref()),
        vec![
            (
                t("Copy overlay to selected frames").to_string(),
                "overlay.copy-selected".to_string()
            ),
            (
                t("Delete overlay").to_string(),
                "overlay.delete".to_string()
            ),
        ],
        "the selection face differs from the other only in the copy"
    );

    let carried: Vec<&str> = band_actions().iter().map(|(name, _)| *name).collect();
    for (label, action) in menu_items(all_menu.upcast_ref())
        .into_iter()
        .chain(menu_items(selection_menu.upcast_ref()))
    {
        let name = action.strip_prefix("overlay.").unwrap_or(&action);
        assert!(
            carried.contains(&name),
            "{label:?} names overlay.{name}, which band_actions() never adds"
        );
    }
}

/// The header menus and the strip's frame menu share the `win` actions, so
/// a renamed or dropped one would grey out a menu entry nothing else
/// notices. Every `win.*` item has to name an action `install_win_actions`
/// registers: one from `win_actions()`, or one of the dialogs it opens
/// straight from the window.
#[test]
fn the_menus_only_name_win_actions_the_window_carries() {
    let dialogs = ["shortcuts", "about", "optimize-remove", "optimize-smart"];
    let carried: Vec<&str> = win_actions()
        .iter()
        .map(|(name, _)| *name)
        .chain(dialogs)
        .collect();
    let (frame_menu, selection_menu) = (gio::Menu::new(), gio::Menu::new());
    fill_frame_menus(&frame_menu, &selection_menu, &Keymap::default());
    let menus: Vec<gio::MenuModel> = edit_menus()
        .into_iter()
        .map(|(_, menu)| menu.upcast())
        .chain([frame_menu.upcast(), selection_menu.upcast()])
        .collect();
    for (label, action) in menus.iter().flat_map(menu_items) {
        if let Some(name) = action.strip_prefix("win.") {
            assert!(
                carried.contains(&name),
                "{label:?} names win.{name}, which the window never adds"
            );
        }
    }
}

/// The frame menus are built once with the window and refilled on a rebind,
/// so the refill has to replace what they held — not stack a second copy
/// under it — and show the new chord on both faces.
#[test]
fn a_rebind_refills_the_frame_menus_in_place() {
    let delete_accel = |menu: &gio::Menu| {
        menu.item_link(0, "section")
            .and_then(|edits| edits.item_attribute_value(0, "accel", None))
            .and_then(|v| v.str().map(str::to_string))
    };
    let (frame_menu, selection_menu) = (gio::Menu::new(), gio::Menu::new());
    fill_frame_menus(&frame_menu, &selection_menu, &Keymap::default());
    let items = menu_items(frame_menu.upcast_ref());

    let mut keys = Keymap::default();
    keys.set(Action::FrameDelete, vec![Chord::parse("ctrl+k").unwrap()]);
    fill_frame_menus(&frame_menu, &selection_menu, &keys);
    assert_eq!(
        menu_items(frame_menu.upcast_ref()),
        items,
        "the same items, once"
    );
    assert_eq!(delete_accel(&frame_menu).as_deref(), Some("<Control>k"));
    assert_eq!(delete_accel(&selection_menu).as_deref(), Some("<Control>k"));
}

/// Regression: the window had no minimum, so a drag could take it under
/// the width its own chrome needs — 465 px for the header and the import
/// row — and libadwaita answered with a warning per frame
/// ("AdwToastOverlay exceeds AdwApplicationWindow width: requested 465
/// px, 360 px available") while the toolbar painted clipped. The floor is
/// what the compositor is told, so the drag stops there instead.
fn the_window_refuses_to_shrink_under_its_own_chrome() {
    let window = App::init_root();
    assert_eq!(
        (window.width_request(), window.height_request()),
        (MIN_WINDOW_W, MIN_WINDOW_H),
        "the floor is a size request, which is what reaches the compositor"
    );
    // The measured minimum of the assembled editor is 465x210, and of the
    // window with the import dialog over it 458x104; the floor clears
    // both. It is stated in `MIN_WINDOW_W`/`MIN_WINDOW_H` rather than
    // asserted here, because a comparison between two constants is
    // checked by reading them, not by running them.
}

/// Regression: the resize rewrite built the fields but never added the
/// responses, so the dialog showed with nothing to click. The fields come
/// seeded from the canvas with the lock on, and a document the budget
/// refuses disables Apply with the reason shown instead of a toast after.
fn the_resize_dialog_offers_cancel_and_apply() {
    let (dialog, width, height, lock, summary) =
        build_resize_dialog(480, 270, 3, usize::MAX, usize::MAX);
    assert!(dialog.has_response("cancel"), "dismisses without resizing");
    assert!(dialog.has_response("apply"), "sends Msg::Resize");
    assert_eq!(dialog.default_response().as_deref(), Some("apply"));
    assert_eq!(dialog.close_response(), "cancel");
    assert_eq!(width.value() as u32, 480, "seeded from the live canvas");
    assert_eq!(height.value() as u32, 270, "seeded from the live canvas");
    assert!(lock.is_active(), "locked until the user says otherwise");
    assert!(
        dialog.is_response_enabled("apply"),
        "a small document may resize"
    );
    assert!(
        summary.label().contains("in memory"),
        "the cost shows before the worker starts: {}",
        summary.label()
    );

    let (too_big, _, _, _, too_big_summary) = build_resize_dialog(8192, 8192, 300, 1, 1);
    assert!(
        !too_big.is_response_enabled("apply"),
        "an over-budget resize cannot be started"
    );
    assert!(
        too_big_summary.label().contains("configured limit"),
        "the reason shows in the dialog: {}",
        too_big_summary.label()
    );
}

/// Regression: with the lock on, editing one resize field left the other
/// alone until the lock was toggled — only committed values were wired,
/// not keystrokes. Both paths now funnel through one link, so the peer
/// tracks typing, arrows, unlock isolation and re-locking.
fn linked_resize_fields_track_each_other_live() {
    let width = gtk::SpinButton::with_range(16.0, 8192.0, 1.0);
    let height = gtk::SpinButton::with_range(16.0, 8192.0, 1.0);
    let lock = adw::SwitchRow::builder().build();
    lock.set_active(true);
    width.set_value(480.0);
    height.set_value(270.0);
    link_aspect_fields(&width, &height, &lock, 480, 270);
    width.set_value(960.0);
    assert_eq!(
        height.value() as u32,
        540,
        "committed width recomputes height"
    );
    height.set_value(135.0);
    assert_eq!(
        width.value() as u32,
        240,
        "committed height recomputes width"
    );
    width.set_text("320");
    assert_eq!(
        height.value() as u32,
        180,
        "keystrokes recompute before the spin commits them"
    );
    width.set_text("");
    assert_eq!(
        height.value() as u32,
        180,
        "a cleared field mid-edit links nothing"
    );
    lock.set_active(false);
    width.set_value(100.0);
    assert_eq!(
        height.value() as u32,
        180,
        "unlocked: the peer is untouched"
    );
    lock.set_active(true);
    assert_eq!(
        height.value() as u32,
        56,
        "re-locking snaps the height back onto the canvas ratio"
    );
}

/// The import picker's size fields step by two and stop at the source, so
/// the shared link has to fit the peer's own adjustment: an odd ratio
/// lands on an even number rather than one the decode would round away, a
/// preset pair is written whole instead of re-derived off its own
/// rounding, and a typed absurdity is read back clamped.
fn linked_import_fields_stay_even_and_in_range() {
    let width = size_spin(1920, 1920);
    let height = size_spin(1080, 1080);
    let lock = adw::SwitchRow::builder().build();
    lock.set_active(true);
    let set_pair = link_aspect_fields(&width, &height, &lock, 1920, 1080);
    height.set_value(480.0);
    assert_eq!(
        width.value() as u32,
        854,
        "16:9 at 480 high is 853.3 wide, and the field only takes even numbers"
    );
    set_pair(852, 480);
    assert_eq!(
        (width.value() as u32, height.value() as u32),
        (852, 480),
        "a preset pair is written as it stands"
    );
    width.set_text("4000");
    assert_eq!(
        height.value() as u32,
        1080,
        "the peer stops at the source height"
    );
    assert_eq!(
        requested_dimension(&width),
        1920,
        "a typed absurdity reads back as what the source can give"
    );
}

/// Regression: the advanced options first shipped as action rows with the
/// control hung off the end and the explanation as a subtitle, which left
/// the titles squeezed to an ellipsis at any sane dialog width. Each row
/// now names itself in a label with its question mark immediately after
/// the words — a tooltip for a mouse, a popover for a finger — and the
/// control alone on the right, where the resolution picker keeps its own.
fn the_advanced_import_rows_explain_themselves_without_crowding() {
    let source = VideoInfo {
        width: 640,
        height: 480,
        fps: 30.0,
        duration_s: Some(90.0),
    };
    let (advanced, units, start, end, drop_nth, ignore_limit) = advanced_import_rows(&source);
    assert!(!advanced.is_expanded(), "advanced stays out of the way");

    assert_eq!(
        units.model().map(|m| m.n_items()),
        Some(2),
        "seconds, frames"
    );
    assert_eq!(
        units.selected(),
        0,
        "seconds first: a clip is scrubbed in time"
    );
    assert_eq!(drop_nth.value(), 1.0, "no frames dropped until asked");
    assert!(
        !ignore_limit.is_active(),
        "the configured memory limit stands until it is switched off"
    );

    // The format lives in the placeholder alone — a web form's habit, and
    // it disappears as soon as the field has something in it. The example
    // is the clip's own start and end, in the units the fields take, so it
    // doubles as the range they will be checked against.
    let placeholder = |entry: &gtk::Entry| entry.placeholder_text().map(|s| s.to_string());
    assert_eq!(placeholder(&start).as_deref(), Some("00:00:00.000"));
    assert_eq!(
        placeholder(&end).as_deref(),
        Some("00:01:30.000"),
        "the end of this 90 second clip, not an invented example"
    );
    set_trim_examples(&start, &end, true, &source);
    assert_eq!(placeholder(&start).as_deref(), Some("0"));
    assert_eq!(
        placeholder(&end).as_deref(),
        Some("2699"),
        "2700 frames, so the last index is 2699"
    );
    start.set_text("0:02");
    assert_eq!(
        placeholder(&start).as_deref(),
        Some("0"),
        "a placeholder is not the value: typing hides it, nothing rewrites it"
    );

    // Every explained row carries its question mark directly after the
    // label, not out beyond the control and not in front of the text.
    let mut labelled = Vec::new();
    let mut next = advanced.upcast_ref::<gtk::Widget>().first_child();
    let mut stack = Vec::new();
    while let Some(child) = next {
        if let Some(button) = child.downcast_ref::<gtk::MenuButton>() {
            let label = button
                .prev_sibling()
                .and_downcast::<gtk::Label>()
                .map(|l| l.label().to_string());
            assert!(
                button.next_sibling().is_none(),
                "the question mark ends its box; a control after it would \
                 put the help between the label and the field"
            );
            assert!(
                button.tooltip_text().is_none(),
                "one way in: the popover explains it, a hover tooltip would \
                 say the same thing over the row it is explaining"
            );
            let explanation = button
                .popover()
                .and_then(|popover| popover.child())
                .and_downcast::<gtk::Label>()
                .map(|label| label.label().to_string())
                .unwrap_or_default();
            assert!(
                explanation.len() > 40,
                "a click has to explain it: {explanation:?}"
            );
            labelled.push(label.expect("a question mark follows its own label"));
        }
        if let Some(first) = child.first_child() {
            stack.push(first);
        }
        next = child.next_sibling().or_else(|| stack.pop());
    }
    // Order of discovery follows the walk, not the rows; what matters is
    // that each explained row has exactly one, behind its own label.
    labelled.sort();
    assert_eq!(
        labelled,
        vec![
            "Drop one frame in every".to_string(),
            "Ignore the memory limit".to_string(),
            "Trim by".to_string(),
        ],
        "one help button per explained row, each behind its own label"
    );
}

/// The splice reads the picked mode off the button index, so the rows have
/// to be one radio group in `FitMode::ALL` order with exactly one active:
/// two groups would let two modes be picked and the first one would win.
fn the_fit_chooser_maps_one_radio_to_each_mode() {
    let (_group, buttons) = fit_options((480, 320), (800, 600));
    assert_eq!(buttons.len(), FitMode::ALL.len());
    let active = |buttons: &[gtk::CheckButton]| -> Vec<usize> {
        buttons
            .iter()
            .enumerate()
            .filter(|(_, radio)| radio.is_active())
            .map(|(i, _)| i)
            .collect()
    };
    assert_eq!(active(&buttons), vec![0], "the least destructive fit");
    buttons[2].set_active(true);
    assert_eq!(
        active(&buttons),
        vec![2],
        "picking one clears the rest, so they share a group"
    );
    let (title, subtitle) = fit_labels(FitMode::GrowPad, (480, 320), (800, 600));
    assert!(!title.is_empty());
    assert!(
        subtitle.contains("800 × 600"),
        "a growing fit names the size the canvas becomes: {subtitle}"
    );
}

/// Regression: the frame menu is a popover parented to the strip, and
/// closing it moves the keyboard focus. A viewport that follows the focus
/// answered every duplicate, delete and paste by scrolling the timeline
/// somewhere else — reproducibly back to the first frame, with the strip
/// scrolled a couple of thousand pixels along.
fn the_strip_viewport_does_not_chase_the_focus() {
    let scroll = gtk::ScrolledWindow::builder()
        .child(&gtk::Box::new(gtk::Orientation::Horizontal, 0))
        .build();
    let viewport = scroll
        .child()
        .and_downcast::<gtk::Viewport>()
        .expect("the scroller wraps its child in a viewport");
    assert!(viewport.is_scroll_to_focus(), "GTK's default");
    dont_chase_focus(&scroll);
    assert!(!viewport.is_scroll_to_focus());
}

/// A document with dozens of overlays must scroll the layer list rather
/// than push the editor for the layer that is picked off the panel, and
/// a panel with no room left must scroll itself rather than squeeze the
/// list down to a sliver. Both bounds are the list's own row height,
/// measured, because a row is as tall as the theme makes it.
fn the_layer_list_is_bounded_by_its_own_row_height() {
    let list = gtk::ListBox::new();
    assert_eq!(layer_list_heights(&list), (0, 0), "no rows, no size");
    let row_h = 40;
    let add = |count| {
        for _ in 0..count {
            let row = gtk::ListBoxRow::new();
            row.set_height_request(row_h);
            list.append(&row);
        }
    };
    let cap = row_h * LAYER_ROWS_SHOWN as i32;
    add(2);
    assert_eq!(
        layer_list_heights(&list),
        (2 * row_h, cap),
        "a list shorter than the floor asks for exactly its rows"
    );
    add(10);
    assert_eq!(
        layer_list_heights(&list),
        (row_h * LAYER_ROWS_KEPT as i32, cap),
        "twelve layers scroll between the floor and the ceiling"
    );
}

/// Regression: deleting the last overlay pins both bounds to zero, and
/// the next overlay added then asks for a floor above that ceiling.
/// Setting the floor first made GTK refuse it (`min_content_height`
/// asserts against the standing `max_content_height`), leaving the list
/// with a zero floor — it collapsed instead of holding its rows open.
fn the_layer_list_bounds_survive_the_list_emptying() {
    let scroll = gtk::ScrolledWindow::new();
    let (row_h, cap) = (40, 40 * LAYER_ROWS_SHOWN as i32);
    set_layer_list_heights(&scroll, 2 * row_h, cap);
    assert_eq!(
        (scroll.min_content_height(), scroll.max_content_height()),
        (2 * row_h, cap)
    );
    set_layer_list_heights(&scroll, 0, 0);
    assert_eq!(
        (scroll.min_content_height(), scroll.max_content_height()),
        (0, 0),
        "an empty list asks for no height at all"
    );
    set_layer_list_heights(&scroll, row_h, cap);
    assert_eq!(
        (scroll.min_content_height(), scroll.max_content_height()),
        (row_h, cap),
        "a list refilling past a zeroed ceiling raises the ceiling first"
    );
}

/// Regression: a `Popover` parented to the strip (`rebuild_strip`'s
/// per-frame context menu) turns up in `first_child()`/`next_sibling()`
/// right along with the real thumbnails, one position ahead of them all.
/// Counting it shifted every playhead/scope/selected border one frame
/// behind whatever was actually clicked.
fn thumb_children_skips_the_frames_popover() {
    let strip = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let popover = gtk::Popover::new();
    popover.set_parent(&strip);
    let cells: Vec<gtk::Box> = (0..3)
        .map(|_| {
            let cell = gtk::Box::new(gtk::Orientation::Vertical, 0);
            cell.add_css_class("thumb");
            strip.append(&cell);
            cell
        })
        .collect();
    let seen: Vec<gtk::Widget> = thumb_children(&strip).collect();
    assert_eq!(seen.len(), 3, "the popover must not count as a thumbnail");
    for (want, got) in cells.iter().zip(&seen) {
        assert_eq!(want.upcast_ref::<gtk::Widget>(), got, "order preserved");
    }
    popover.unparent();
}

/// The delay field after Smart remove: type a delay for one frame, click
/// another thumbnail (which takes no focus, so nothing commits), and the
/// typed value used to land on the second frame at the next focus-out.
fn a_typed_delay_commits_once_and_not_onto_the_next_frame() {
    let sync = Rc::new(Cell::new(false));
    let spin = gtk::SpinButton::with_range(1.0, u16::MAX as f64, 1.0);
    spin.set_value(28.0);
    // What the real handler would send: only changes seen unguarded.
    let sent = Rc::new(RefCell::new(Vec::new()));
    {
        let (sent, sync) = (sent.clone(), sync.clone());
        spin.connect_value_changed(move |s| {
            if !sync.get() {
                sent.borrow_mut().push(s.value());
            }
        });
    }

    assert_eq!(commit_typed(&spin, &sync), None, "nothing typed");
    spin.set_text("7");
    assert_eq!(commit_typed(&spin, &sync), Some(7.0));
    assert!(
        sent.borrow().is_empty(),
        "the caller applies it, not the handler"
    );
    assert!(!sync.get(), "the guard is released again");

    // Next frame holds 7 too: set_spin skips the write, and a later
    // focus-out must find nothing left to commit onto it.
    set_spin(&spin, 7.0);
    spin.update();
    assert!(
        sent.borrow().is_empty(),
        "the typed value landed on the next frame"
    );
}

fn syncing_a_colour_and_width_pair_sends_nothing_back() {
    let sent = Rc::new(RefCell::new(Vec::new()));
    let sync = Rc::new(Cell::new(false));
    let (colour, width) = (ColorPick::new(), width_spin());
    {
        let sent = sent.clone();
        connect_pair(&colour, &width, &sync, move |v| sent.borrow_mut().push(v));
    }

    // What update_view does for a text overlay with the default outline.
    sync.set(true);
    colour.set_rgba([0, 0, 0, 255]);
    set_spin(&width, 2.0);
    sync.set(false);
    assert!(
        sent.borrow().is_empty(),
        "the sync echoed: {:?}",
        sent.borrow()
    );

    // The user's own edits still get through.
    width.set_value(5.0);
    assert_eq!(*sent.borrow(), vec![Some(([0, 0, 0, 255], 5.0))]);

    // Regression: at width 0 the pair used to collapse to `None`, which
    // threw the colour away, so the next nonzero width painted the
    // default instead of the colour picked. It stays `Some`.
    width.set_value(0.0);
    width.set_value(3.0);
    assert_eq!(
        sent.borrow()[1..],
        [Some(([0, 0, 0, 255], 0.0)), Some(([0, 0, 0, 255], 3.0))]
    );
}

fn plan_1080p60() -> ImportPlan {
    ImportPlan {
        source: VideoInfo {
            width: 1920,
            height: 1080,
            fps: 60.0,
            duration_s: Some(90.0),
        },
        width: 1280,
        height: 720,
        fps: 3.79,
        start_s: 0.0,
        span_s: None,
        drop_nth: None,
        cap: 341,
    }
}

/// The dialog earns its interruption by naming the file. It says nothing
/// else when the file fits: the fields are in front of the user and the
/// preview under them prices every choice, so a sentence pointing at the
/// Advanced row was one more line to read past every import.
#[test]
fn the_warning_says_what_the_file_is() {
    let body = import_body("lecture.mp4", &plan_1080p60());
    for want in ["lecture.mp4", "runs 1:30", "1920×1080", "60 fps"] {
        assert!(body.contains(want), "missing {want:?} in:\n{body}");
    }
    assert!(
        !body.contains("Advanced"),
        "a file that fits gets the facts and nothing else:\n{body}"
    );

    // Over budget, the body has to say so: this is the sentence that
    // explains the greyed-out Import button.
    let mut refused = plan_1080p60();
    refused.fps = 60.0;
    assert!(
        refused.over_budget(),
        "5400 frames of 1280x720 over a 341 cap"
    );
    let body = import_body("lecture.mp4", &refused);
    assert!(body.contains("will fit in memory"), "{body}");
}

#[test]
fn a_file_with_no_duration_still_reads_as_a_sentence() {
    let plan = ImportPlan {
        source: VideoInfo {
            width: 800,
            height: 600,
            fps: 30.0,
            duration_s: None,
        },
        width: 800,
        height: 600,
        fps: 30.0,
        start_s: 0.0,
        span_s: None,
        drop_nth: None,
        cap: 2000,
    };
    assert!(import_body("stream.mkv", &plan).contains("no duration"));
    let summary = plan_summary(&plan, None);
    assert!(summary.contains("no duration"), "{summary}");
    assert!(!summary.contains("memory"), "no invented size: {summary}");
}

/// The preview is the whole point of the picker: it has to move with the
/// resolution, and the memory figure has to be the real one.
#[test]
fn the_preview_tracks_the_chosen_resolution() {
    let source = VideoInfo {
        width: 1920,
        height: 1080,
        fps: 60.0,
        duration_s: Some(90.0),
    };
    let at = |w: u32, h: u32| {
        let options = ImportOptions {
            target: Some((w, h)),
            ..Default::default()
        };
        video::plan_for(source.clone(), &options)
    };

    let big = at(1280, 720);
    let small = at(640, 360);
    let budget = ImportOptions::default().max_bytes;
    assert!(big.bytes().unwrap() <= budget && small.bytes().unwrap() <= budget);
    // The budget is a ceiling, not a target: a smaller frame spends the
    // same memory on more frames rather than banking the difference.
    assert!(small.fps > big.fps, "{} vs {}", small.fps, big.fps);
    assert!(small.frames() > big.frames());

    let summary = plan_summary(&big, None);
    assert!(summary.contains("frames at"), "{summary}");
    assert!(summary.contains("in memory"), "{summary}");
    assert!(summary.contains("measuring the GIF size"), "{summary}");
    assert!(
        summary.contains("down from 60"),
        "says what it gave up: {summary}"
    );
}

/// A typed time is held to what a GIF can hold: hundredths of a second. A
/// typo is `None` rather than a guess, so the trim it feeds turns `Invalid`.
#[test]
fn typed_seconds_read_to_the_hundredth() {
    assert_eq!(parse_seconds("12.5"), Some(12.5));
    assert_eq!(
        parse_seconds("0:01.123"),
        Some(1.12),
        "a GIF holds hundredths, so the millisecond is rounded away here \
         rather than pretended at through the import"
    );
    assert_eq!(
        parse_seconds("0:01.126"),
        Some(1.13),
        "rounded, not floored"
    );
    assert_eq!(parse_seconds(" 1:02 "), Some(62.0));
    assert_eq!(parse_seconds("1:02:03"), Some(3723.0));
    for junk in ["", "abc", "1:2:3:4", "-5", "1:", "0:0x"] {
        assert_eq!(parse_seconds(junk), None, "on {junk:?}");
    }
}

/// 20 seconds at 25 fps: 500 frames, indices 0 to 499.
fn twenty_second_clip() -> VideoInfo {
    VideoInfo {
        width: 640,
        height: 480,
        fps: 25.0,
        duration_s: Some(20.0),
    }
}

/// The trim fields take what a person types at a video: bare seconds, a
/// timecode, or a frame number. A typo is `Invalid` rather than a silent
/// whole-clip import, because that is the mistake that costs minutes and
/// gigabytes.
#[test]
fn the_trim_fields_read_timecodes_and_frames() {
    let source = twenty_second_clip();
    let read = |in_frames, from: &str, to: &str| read_trim(in_frames, from, to, &source);

    assert_eq!(read(false, "", ""), TrimInput::Whole);
    assert_eq!(
        read(false, "0:01.123", "0:2.001"),
        TrimInput::Set(Trim::Seconds {
            start: 1.12,
            end: Some(2.0)
        }),
        "the example from the request, at the precision a GIF can hold"
    );
    assert_eq!(
        read(false, "10", ""),
        TrimInput::Set(Trim::Seconds {
            start: 10.0,
            end: None
        }),
        "an open end runs to the end of the clip"
    );
    assert_eq!(
        read(false, "", "5"),
        TrimInput::Set(Trim::Seconds {
            start: 0.0,
            end: Some(5.0)
        }),
        "an open start begins at the beginning"
    );
    assert_eq!(
        read(true, "234", "499"),
        TrimInput::Set(Trim::Frames {
            first: 234,
            last: Some(499)
        }),
        "the last frame the clip has is still inside it"
    );
    assert_eq!(read(true, "10", "1.5"), TrimInput::Invalid);
    assert_eq!(read(false, "10", "nonsense"), TrimInput::Invalid);
    assert_eq!(
        read(false, "10", "10"),
        TrimInput::Invalid,
        "an end at its start is an empty import, not a whole one"
    );
    assert_eq!(read(false, "10", "5"), TrimInput::Invalid);
    assert_eq!(
        TrimInput::Invalid.trim(),
        None,
        "an invalid pair never reaches a plan"
    );
}

/// A time the file does not reach is `Invalid` too: clamped, it would be an
/// import nobody asked for.
#[test]
fn a_trim_past_the_end_of_the_file_is_refused() {
    let source = twenty_second_clip();
    let read = |in_frames, from: &str, to: &str| read_trim(in_frames, from, to, &source);
    assert_eq!(
        read(false, "", "20.01"),
        TrimInput::Invalid,
        "a hundredth past a 20 second clip is not in the file"
    );
    assert_eq!(read(false, "25", ""), TrimInput::Invalid);
    assert_eq!(
        read(true, "", "500"),
        TrimInput::Invalid,
        "frame 500 of 500"
    );
    assert_eq!(read(true, "500", ""), TrimInput::Invalid);
    assert_eq!(
        read(false, "", "20"),
        TrimInput::Set(Trim::Seconds {
            start: 0.0,
            end: Some(20.0)
        }),
        "the end of the clip itself is a legitimate end"
    );

    // A container with no duration has nothing to check against, so the
    // shape of the input is all that can be judged.
    let endless = VideoInfo {
        duration_s: None,
        ..source.clone()
    };
    assert_eq!(
        read_trim(false, "", "9999", &endless),
        TrimInput::Set(Trim::Seconds {
            start: 0.0,
            end: Some(9999.0)
        })
    );
}

/// The trim fields are aimed at a length, so the section states it: the
/// clip's own duration to the millisecond and the frames it holds.
#[test]
fn the_source_length_reads_to_the_millisecond() {
    assert_eq!(timestamp(0.0), "00:00:00.000");
    assert_eq!(timestamp(1.123), "00:00:01.123");
    assert_eq!(timestamp(90.0), "00:01:30.000");
    assert_eq!(timestamp(3723.456), "01:02:03.456");
    let source = VideoInfo {
        width: 640,
        height: 480,
        fps: 30.0,
        duration_s: Some(90.0),
    };
    assert_eq!(source_frames(&source), Some(2700));
    assert_eq!(
        source_frames(&VideoInfo {
            duration_s: None,
            ..source
        }),
        None,
        "no duration, no count to state"
    );
}

/// The steppers exist so a trim can be nudged rather than typed, so they
/// step in the unit the field is being read in, start from the end they
/// stand for when the field is empty, and can never step outside the file
/// — a button that produces a value the field then refuses is worse than
/// no button.
#[test]
fn the_trim_steppers_move_by_a_second_or_ten_frames() {
    // 20 seconds at 25 fps: 500 frames, indices 0 to 499.
    let source = VideoInfo {
        width: 640,
        height: 480,
        fps: 25.0,
        duration_s: Some(20.0),
    };
    let step = |text: &str, forward: bool, in_frames: bool, is_end: bool| {
        stepped_trim(text, forward, in_frames, is_end, &source)
    };

    assert_eq!(step("00:00:05.000", true, false, false), "00:00:06.000");
    assert_eq!(step("00:00:05.000", false, false, false), "00:00:04.000");
    assert_eq!(
        step("0:05.50", true, false, false),
        "00:00:06.500",
        "a second on from whatever the field holds, in the format it shows"
    );
    assert_eq!(
        step("", true, false, false),
        "00:00:01.000",
        "an empty From steps from the start of the clip"
    );
    assert_eq!(
        step("", false, false, true),
        "00:00:19.000",
        "an empty To steps back from the end of the clip"
    );
    assert_eq!(
        step("nonsense", true, false, false),
        "00:00:01.000",
        "a field being typed into is stepped from its own end, not ignored"
    );
    assert_eq!(
        step("00:00:00.000", false, false, false),
        "00:00:00.000",
        "the start of the clip is as far back as it goes"
    );
    assert_eq!(
        step("00:00:20.000", true, false, true),
        "00:00:20.000",
        "and its end is as far on: stepping never leaves the file"
    );

    assert_eq!(step("100", true, true, false), "110");
    assert_eq!(step("100", false, true, false), "90");
    assert_eq!(step("5", false, true, false), "0", "clamped, not wrapped");
    assert_eq!(
        step("495", true, true, true),
        "499",
        "the last frame the clip has"
    );
    assert_eq!(
        step("", false, true, true),
        "489",
        "an empty To in frames steps back from the last index"
    );

    // No duration to clamp against: the steps still work, and only the
    // floor at zero applies.
    let endless = VideoInfo {
        duration_s: None,
        ..source
    };
    assert_eq!(
        stepped_trim("00:00:03.000", true, false, false, &endless),
        "00:00:04.000"
    );
    assert_eq!(stepped_trim("3", false, true, false, &endless), "0");
}

/// The summary is what the advanced options are steered by, so it has to
/// name the span the trim leaves and the rate the drop leaves behind.
#[test]
fn the_preview_reports_the_trim_and_the_drop() {
    let source = VideoInfo {
        width: 640,
        height: 360,
        fps: 30.0,
        duration_s: Some(600.0),
    };
    let plan = video::plan_for(
        source,
        &ImportOptions {
            fps: Some(30.0),
            trim: Some(Trim::Seconds {
                start: 65.0,
                end: Some(75.0),
            }),
            drop_nth: Some(3),
            ..Default::default()
        },
    );
    let summary = plan_summary(&plan, None);
    assert!(summary.contains("1:05 to 1:15"), "the span: {summary}");
    assert!(summary.contains("200 frames"), "the kept count: {summary}");
    assert!(
        summary.contains("20 fps, down from 30"),
        "the rate they play at, not the decode's: {summary}"
    );
}

/// With the rate pinned, a smaller frame is the saving the picker promises.
#[test]
fn a_chosen_rate_makes_resolution_the_lever() {
    let source = VideoInfo {
        width: 1920,
        height: 1080,
        fps: 60.0,
        duration_s: Some(90.0),
    };
    let at = |w: u32, h: u32| {
        let options = ImportOptions {
            target: Some((w, h)),
            fps: Some(4.0),
            ..Default::default()
        };
        video::plan_for(source.clone(), &options)
    };

    let big = at(1280, 720);
    let small = at(480, 270);
    assert_eq!(
        small.fps, big.fps,
        "the rate is the user's, not the budget's"
    );
    assert!(
        small.bytes().unwrap() * 4 < big.bytes().unwrap(),
        "{} vs {}",
        small.bytes().unwrap(),
        big.bytes().unwrap()
    );
}

/// A rate the budget cannot cover is refused, not trimmed. The clip is the
/// user's; the settings are the thing that has to give.
#[test]
fn an_over_budget_rate_is_refused_rather_than_trimmed() {
    let source = VideoInfo {
        width: 1920,
        height: 1080,
        fps: 60.0,
        duration_s: Some(90.0),
    };
    let options = ImportOptions {
        target: Some((1280, 720)),
        fps: Some(30.0),
        ..Default::default()
    };
    let plan = video::plan_for(source, &options);

    assert_eq!(plan.fps, 30.0, "the rate the user asked for");
    assert!(plan.over_budget());
    assert_eq!(
        plan.frames(),
        Some(2700),
        "the whole clip, not a capped count"
    );

    let summary = plan_summary(&plan, None);
    assert!(
        summary.contains("more memory than an import may use"),
        "{summary}"
    );
    assert!(
        summary.contains("smaller size or a lower frame rate"),
        "{summary}"
    );
    assert!(
        !summary.contains("dropped"),
        "nothing is dropped: {summary}"
    );
    assert!(
        !summary.contains("measuring"),
        "not measured either: {summary}"
    );
}

/// The budget comes from the user's settings file, so the same clip can be
/// fine on one machine and refused on another.
#[test]
fn the_budget_comes_from_settings() {
    let source = VideoInfo {
        width: 1920,
        height: 1080,
        fps: 30.0,
        duration_s: Some(20.0),
    };
    let at = |mb: usize| {
        let options = ImportOptions {
            target: Some((1280, 720)),
            fps: Some(30.0),
            max_bytes: mb << 20,
            ..Default::default()
        };
        video::plan_for(source.clone(), &options)
    };
    assert!(at(256).over_budget(), "a tight budget refuses it");
    assert!(!at(4096).over_budget(), "a generous one takes it");
}

/// Only a real encode ever puts a size on screen. Until one lands the
/// summary says so, rather than showing a number nobody measured.
#[test]
fn a_size_is_shown_only_once_it_has_been_measured() {
    let plan = plan_1080p60();
    let pending = plan_summary(&plan, None);
    assert!(pending.contains("measuring"), "{pending}");
    assert!(!pending.contains("roughly"), "no guess: {pending}");
    assert!(!pending.contains('–'), "no range: {pending}");

    let measured = plan_summary(&plan, Some(7_340_032));
    assert!(measured.contains("7 MB as a GIF"), "{measured}");
    assert!(!measured.contains("measuring"), "{measured}");
}

/// The picker is allowed to squash: an explicit target overrides the aspect.
#[test]
fn a_custom_size_may_change_the_aspect_ratio() {
    let source = VideoInfo {
        width: 1920,
        height: 1080,
        fps: 30.0,
        duration_s: Some(10.0),
    };
    let options = ImportOptions {
        target: Some((600, 600)),
        ..Default::default()
    };
    let plan = video::plan_for(source, &options);
    assert_eq!((plan.width, plan.height), (600, 600));
}

/// Odd numbers reach ffmpeg's scaler as even ones or not at all.
#[test]
fn odd_targets_are_rounded_to_even() {
    let source = VideoInfo {
        width: 1920,
        height: 1080,
        fps: 30.0,
        duration_s: Some(10.0),
    };
    let options = ImportOptions {
        target: Some((641, 361)),
        ..Default::default()
    };
    let plan = video::plan_for(source, &options);
    assert_eq!((plan.width % 2, plan.height % 2), (0, 0), "{plan:?}");
}

#[test]
fn presets_only_offer_sizes_that_fit_inside_the_source() {
    let source = VideoInfo {
        width: 1920,
        height: 1080,
        fps: 30.0,
        duration_s: Some(10.0),
    };
    let presets = video::size_presets(&source);
    assert_eq!(presets[0], (1280, 720));
    assert!(presets.iter().all(|(w, h)| *w <= 1920 && *h < 1080));
    assert!(presets.iter().all(|(w, h)| w % 2 == 0 && h % 2 == 0));

    let already_small = VideoInfo {
        width: 320,
        height: 240,
        fps: 30.0,
        duration_s: None,
    };
    assert!(
        video::size_presets(&already_small).is_empty(),
        "nothing to step down to"
    );
}
