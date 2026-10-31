//! Canvas hit testing, gestures, cursors and overlay handle drawing.

use super::*;

/// Where the picture actually sits inside its widget: `ContentFit::ScaleDown`
/// centres the image and never enlarges it, so the canvas gestures have to undo
/// exactly that to land on a pixel.
pub(super) fn canvas_map(widget: (f64, f64), image: (f64, f64)) -> (f64, f64, f64) {
    if image.0 <= 0.0 || image.1 <= 0.0 {
        return (1.0, 0.0, 0.0);
    }
    let scale = (widget.0 / image.0)
        .min(widget.1 / image.1)
        .clamp(0.0001, 1.0);
    (
        (scale),
        (widget.0 - image.0 * scale) / 2.0,
        (widget.1 - image.1 * scale) / 2.0,
    )
}

/// Hit-testing reads the point in the box's own frame, so a rotated overlay is
/// grabbed where it looks, not where its un-rotated box used to be.
pub(super) fn contains(t: Transform, x: f32, y: f32) -> bool {
    let (x, y) = t.to_local(x, y);
    let (x0, x1) = (t.x.min(t.x + t.w), t.x.max(t.x + t.w));
    let (y0, y1) = (t.y.min(t.y + t.h), t.y.max(t.y + t.h));
    (x0..=x1).contains(&x) && (y0..=y1).contains(&y)
}

/// Corners in TL, TR, BL, BR order — the same order `resize_corner` and the
/// handle drawing use.
pub(super) fn corners(t: Transform) -> [(f32, f32); 4] {
    [
        (t.x, t.y),
        (t.x + t.w, t.y),
        (t.x, t.y + t.h),
        (t.x + t.w, t.y + t.h),
    ]
}

pub(super) fn handle_at(t: Transform, x: f32, y: f32, grab: f32) -> Option<usize> {
    let (x, y) = t.to_local(x, y);
    corners(t)
        .iter()
        .position(|(hx, hy)| (hx - x).abs() <= grab && (hy - y).abs() <= grab)
}

/// Where the grips actually are on the image, rotation included.
pub(super) fn oriented_corners(t: Transform) -> [(f32, f32); 4] {
    corners(t).map(|(x, y)| t.to_image(x, y))
}

/// Drag one corner, in the box's own frame. The opposite corner stays put
/// unless `from_center`, in which case the centre does. Width and height may
/// pass through zero and come out negative, which `contains` and the
/// rasterizers already tolerate, so there is no need to forbid it.
pub(super) fn resize_corner(
    t: Transform,
    corner: usize,
    dx: f32,
    dy: f32,
    keep_aspect: bool,
    from_center: bool,
) -> Transform {
    let left = corner == 0 || corner == 2;
    let top = corner == 0 || corner == 1;
    // Resizing about the centre moves both edges, so the drag counts twice.
    let reach = if from_center { 2.0 } else { 1.0 };
    let mut w = t.w + dx * reach * if left { -1.0 } else { 1.0 };
    let mut h = t.h + dy * reach * if top { -1.0 } else { 1.0 };

    if keep_aspect && t.w.abs() > f32::EPSILON && t.h.abs() > f32::EPSILON {
        // Take the larger of the two scale factors and give both axes its
        // magnitude, keeping whichever signs the drag produced so a flip
        // through zero still flips.
        let (sx, sy) = (w / t.w, h / t.h);
        let factor = sx.abs().max(sy.abs());
        w = t.w * factor * sx.signum();
        h = t.h * factor * sy.signum();
    }

    let (cx, cy) = t.center();
    let (x, y) = if from_center {
        (cx - w / 2.0, cy - h / 2.0)
    } else {
        (
            if left { t.x + t.w - w } else { t.x },
            if top { t.y + t.h - h } else { t.y },
        )
    };
    Transform {
        x,
        y,
        w,
        h,
        angle: t.angle,
    }
}

/// `resize_corner` pins the anchor in the box's own frame, but the box rotates
/// about its centre and the centre just moved — so on the image the anchor
/// drifts. Slide the result back onto it.
pub(super) fn pin_anchor(
    origin: Transform,
    mut resized: Transform,
    corner: usize,
    from_center: bool,
) -> Transform {
    if origin.angle == 0.0 {
        return resized;
    }
    let anchor = |t: Transform| {
        // Opposite corner, in the TL, TR, BL, BR order `corners` uses.
        if from_center {
            t.center()
        } else {
            corners(t)[3 - corner]
        }
    };
    let (was_x, was_y) = origin.to_image(anchor(origin).0, anchor(origin).1);
    let (now_x, now_y) = resized.to_image(anchor(resized).0, anchor(resized).1);
    resized.x += was_x - now_x;
    resized.y += was_y - now_y;
    resized
}

/// Impasto's rotate cursor, hotspot centred on the texture. Impasto centres it
/// the same way and for the same reason: a hotspot past the edge of the loaded
/// image trips a GDK assertion.
pub(super) fn rotate_cursor() -> Option<gdk::Cursor> {
    let texture = gdk::Texture::from_bytes(&glib::Bytes::from_static(ROTATE_CURSOR)).ok()?;
    Some(gdk::Cursor::from_texture(
        &texture,
        texture.width() / 2,
        texture.height() / 2,
        None,
    ))
}

/// Cursor glyph for a corner of a box rotated by `angle`, ported from Impasto's
/// `ResizeCursors`: the glyph snaps within the corner's own diagonal family, so
/// the arrow keeps pointing along the rotated edge.
pub(super) fn corner_cursor(corner: usize, angle: f32) -> &'static str {
    // Screen-space octants, [E, SE, S, SW, W, NW, N, NE].
    const CURSORS: [&str; 8] = [
        "e-resize",
        "se-resize",
        "s-resize",
        "sw-resize",
        "w-resize",
        "nw-resize",
        "n-resize",
        "ne-resize",
    ];
    // Axis-aligned octant of each corner, in TL, TR, BL, BR order.
    const BASE: [i32; 4] = [5, 7, 3, 1];
    let base = BASE[corner % 4];
    let parity = base & 1;
    let rotated = base as f32 * 45.0 + angle.to_degrees();
    let steps = ((rotated - parity as f32 * 45.0) / 90.0).round() as i32;
    CURSORS[(((parity + 2 * steps) % 8 + 8) % 8) as usize]
}

/// Impasto's grip hint, built from the keymap rather than typed, so a rebind
/// moves it (AGENTS.md, Keybindings).
pub(super) fn grip_hint(keys: &Keymap) -> String {
    fill(
        // Translators: Canvas tooltip over an overlay's resize grip. Keep the line breaks.
        t(
            "Drag to resize\n{aspect}: keep the aspect ratio\n{center}+drag: resize from the \
           center\n{rotate}+drag: rotate",
        ),
        &[
            ("aspect", &keys.mods(Modal::KeepAspect).display()),
            ("center", &keys.mods(Modal::FromCenter).display()),
            ("rotate", &keys.mods(Modal::Rotate).display()),
        ],
    )
}

/// The body of an overlay, where a drag moves it and the corners are elsewhere.
pub(super) fn move_hint(keys: &Keymap) -> String {
    fill(
        // Translators: Canvas tooltip over the body of an overlay. Keep the line break.
        t("Drag to move\n{rotate}+drag: rotate"),
        &[("rotate", &keys.mods(Modal::Rotate).display())],
    )
}

/// Click to select, drag to move, corner to resize — and with the crop tool
/// armed, drag to define the box. All of it in image pixels, which is the only
/// coordinate system the model knows.
pub(super) fn install_canvas_gestures(
    area: &gtk::DrawingArea,
    state: &Rc<RefCell<CanvasState>>,
    sender: &ComponentSender<App>,
) {
    let to_image = {
        let (state, area) = (state.clone(), area.clone());
        move |x: f64, y: f64| -> (f32, f32, f32) {
            let image = state.borrow().image;
            let (scale, ox, oy) = canvas_map(
                (area.width() as f64, area.height() as f64),
                (image.0 as f64, image.1 as f64),
            );
            (
                ((x - ox) / scale) as f32,
                ((y - oy) / scale) as f32,
                scale as f32,
            )
        }
    };

    let drag = gtk::GestureDrag::new();
    {
        let (to_image, sender) = (to_image.clone(), sender.clone());
        drag.connect_drag_begin(move |gesture, x, y| {
            let (ix, iy, scale) = to_image(x, y);
            let state = gesture.current_event_state();
            sender.input(Msg::CanvasPress {
                x: ix,
                y: iy,
                scale,
                state,
            });
        });
    }
    {
        let (to_image, sender) = (to_image.clone(), sender.clone());
        drag.connect_drag_update(move |gesture, dx, dy| {
            let Some((sx, sy)) = gesture.start_point() else {
                return;
            };
            let (ix, iy, _) = to_image(sx + dx, sy + dy);
            // Read live: the modifiers may have gone down after the press.
            let state = gesture.current_event_state();
            sender.input(Msg::CanvasDrag {
                x: ix,
                y: iy,
                state,
            });
        });
    }
    {
        let sender = sender.clone();
        drag.connect_drag_end(move |_, _, _| sender.input(Msg::CanvasRelease));
    }
    area.add_controller(drag);
    install_canvas_hover(area, state);
}

/// Hover feedback, Impasto's: the resize glyph for the grip under the pointer,
/// the rotate cursor while the rotate modifier is down, and the grip hint as
/// the canvas tooltip. It reads the shared canvas state directly, because a
/// cursor that waits for a message round trip lags the pointer.
pub(super) fn install_canvas_hover(area: &gtk::DrawingArea, state: &Rc<RefCell<CanvasState>>) {
    let motion = gtk::EventControllerMotion::new();
    let (state, area_ref) = (state.clone(), area.clone());
    let rotate_cursor = rotate_cursor();
    // Motion fires per pixel, and both setters below allocate, so each one only
    // runs when its value actually changed.
    let shown: Rc<Cell<&'static str>> = Rc::new(Cell::new(""));
    motion.connect_motion(move |controller, x, y| {
        let state = state.borrow();
        let (scale, ox, oy) = canvas_map(
            (area_ref.width() as f64, area_ref.height() as f64),
            (state.image.0 as f64, state.image.1 as f64),
        );
        let (ix, iy) = (((x - ox) / scale) as f32, ((y - oy) / scale) as f32);
        let grab = (HANDLE_PX / scale.max(0.01)) as f32;
        let rotating = state.rotate.held(controller.current_event_state());

        let cursor = hover_cursor(&state, ix, iy, grab, rotating);
        if shown.replace(cursor) != cursor {
            match (cursor, &rotate_cursor) {
                ("rotate", Some(glyph)) => area_ref.set_cursor(Some(glyph)),
                // No CSS cursor name means rotation, so a build that could not
                // read the texture falls back to the nearest thing every theme
                // ships rather than to no feedback at all.
                ("rotate", None) => area_ref.set_cursor_from_name(Some("grab")),
                _ => area_ref.set_cursor_from_name(Some(cursor)),
            }
        }

        let want = match cursor {
            "default" => None,
            "move" => Some(state.move_hint.clone()),
            _ => Some(state.hint.clone()),
        };
        if area_ref.tooltip_text().map(|t| t.to_string()) != want {
            area_ref.set_tooltip_text(want.as_deref());
        }
    });
    area.add_controller(motion);
}

/// Which glyph the pointer gets, in the same precedence the press handler uses:
/// the rotate modifier outranks a grip, a grip outranks the body it belongs to,
/// and the body of *any* overlay promises a move, not just the selected one's.
pub(super) fn hover_cursor(
    state: &CanvasState,
    x: f32,
    y: f32,
    grab: f32,
    rotating: bool,
) -> &'static str {
    let grip = state
        .selected
        .and_then(|t| handle_at(t, x, y, grab).map(|corner| (t, corner)));
    match (grip, rotating) {
        (_, true) if state.selected.is_some() => "rotate",
        (Some((t, corner)), _) => corner_cursor(corner, t.angle),
        _ if state.movable.iter().any(|t| contains(*t, x, y)) => "move",
        _ => "default",
    }
}

/// Selection handles and the crop box. Drawn over the picture rather than into
/// the composite, so nothing here can end up in the exported GIF.
pub(super) fn draw_canvas_overlay(
    area: &gtk::DrawingArea,
    cr: &cairo::Context,
    width: f64,
    height: f64,
    state: &CanvasState,
) {
    let (scale, ox, oy) = canvas_map(
        (width, height),
        (state.image.0 as f64, state.image.1 as f64),
    );
    let to_widget = |x: f32, y: f32| (ox + x as f64 * scale, oy + y as f64 * scale);
    let accent = area.color();
    let (image_w, image_h) = (state.image.0 as f64 * scale, state.image.1 as f64 * scale);
    cr.set_line_width(1.0);

    // The widget is wider or taller than the picture whenever the aspect ratios
    // differ, and an overlay dragged into that margin is clipped out of every
    // frame. Say where the picture ends rather than letting it vanish.
    if image_w > 0.0 && image_h > 0.0 && state.crop.is_none() {
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.30);
        cr.set_fill_rule(cairo::FillRule::EvenOdd);
        cr.rectangle(0.0, 0.0, width, height);
        cr.rectangle(ox, oy, image_w, image_h);
        let _ = cr.fill();
        cr.set_fill_rule(cairo::FillRule::Winding);
        cr.set_source_rgba(
            accent.red() as f64,
            accent.green() as f64,
            accent.blue() as f64,
            0.55,
        );
        cr.rectangle(ox + 0.5, oy + 0.5, image_w - 1.0, image_h - 1.0);
        let _ = cr.stroke();
    }

    if let Some((x, y, w, h)) = state.crop {
        let (px, py) = to_widget(x, y);
        let (pw, ph) = (w as f64 * scale, h as f64 * scale);
        // Dim what the crop would throw away, so the box reads as a keep-area:
        // two rectangles, even-odd, so the inner one is a hole.
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.45);
        cr.set_fill_rule(cairo::FillRule::EvenOdd);
        cr.rectangle(0.0, 0.0, width, height);
        cr.rectangle(px, py, pw, ph);
        let _ = cr.fill();
        cr.set_fill_rule(cairo::FillRule::Winding);
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.rectangle(px, py, pw, ph);
        let _ = cr.stroke();
        return;
    }

    draw_guides(cr, state, scale, (ox, oy));
    let Some(t) = state.selected else { return };
    let grips = oriented_corners(t).map(|(x, y)| to_widget(x, y));
    // TL, TR, BR, BL: the outline walks the box, `corners` lists it in reading
    // order, so the last two swap.
    let outline = [grips[0], grips[1], grips[3], grips[2]];

    cr.set_source_rgba(
        accent.red() as f64,
        accent.green() as f64,
        accent.blue() as f64,
        0.9,
    );
    cr.set_dash(&[4.0, 3.0], 0.0);
    cr.move_to(outline[0].0, outline[0].1);
    for (x, y) in &outline[1..] {
        cr.line_to(*x, *y);
    }
    cr.close_path();
    let _ = cr.stroke();
    cr.set_dash(&[], 0.0);

    // Impasto's grips: a blue dot with a translucent white ring, at a constant
    // widget size whatever the zoom.
    for (cx, cy) in grips {
        cr.arc(cx, cy, HANDLE_R, 0.0, std::f64::consts::TAU);
        cr.set_source_rgb(HANDLE_FILL.0, HANDLE_FILL.1, HANDLE_FILL.2);
        let _ = cr.fill_preserve();
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.7);
        let _ = cr.stroke();
    }
}

/// The canvas edges and centre lines a drag is snapped to, dashed across the
/// picture for as long as it holds there — Impasto's `DrawSnapGuides`.
pub(super) fn draw_guides(
    cr: &cairo::Context,
    state: &CanvasState,
    scale: f64,
    (ox, oy): (f64, f64),
) {
    let (w, h) = (state.image.0 as f64, state.image.1 as f64);
    if let Some(guide) = state.guides.x {
        let x = (ox + guide.at(state.image.0) as f64 * scale).round() + 0.5;
        cr.move_to(x, oy);
        cr.line_to(x, oy + h * scale);
    }
    if let Some(guide) = state.guides.y {
        let y = (oy + guide.at(state.image.1) as f64 * scale).round() + 0.5;
        cr.move_to(ox, y);
        cr.line_to(ox + w * scale, y);
    }
    cr.set_source_rgba(0.2, 0.5, 1.0, 0.9);
    cr.set_line_width(1.0);
    cr.set_dash(&[4.0, 4.0], 0.0);
    let _ = cr.stroke();
    cr.set_dash(&[], 0.0);
}
