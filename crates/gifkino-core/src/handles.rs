//! An overlay's box under the pointer: hit testing, the corner grips, and
//! what a move, resize or rotate drag does to the box. Everything is in image
//! pixels; turning widget pixels into image pixels is the UI's job.

use super::model::Transform;
use super::snap::{self, Guides};

/// What a drag on an overlay is doing. Corners are TL, TR, BL, BR.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Grip {
    Move,
    Resize(usize),
    Rotate,
}

/// The modifiers held during a resize or rotate drag. `keep_aspect` also
/// constrains a rotation to `ROTATE_STEPS`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct DragKeys {
    pub keep_aspect: bool,
    pub from_center: bool,
}

/// Rotation constrains to this many steps of a full turn, as Impasto's
/// transform tool does.
pub const ROTATE_STEPS: f32 = 32.0;

/// Hit-testing reads the point in the box's own frame, so a rotated overlay is
/// grabbed where it looks, not where its un-rotated box used to be.
pub fn contains(t: Transform, x: f32, y: f32) -> bool {
    let (x, y) = t.to_local(x, y);
    let (x0, x1) = (t.x.min(t.x + t.w), t.x.max(t.x + t.w));
    let (y0, y1) = (t.y.min(t.y + t.h), t.y.max(t.y + t.h));
    (x0..=x1).contains(&x) && (y0..=y1).contains(&y)
}

/// Corners in TL, TR, BL, BR order — the same order `resize_corner` and the
/// handle drawing use.
pub fn corners(t: Transform) -> [(f32, f32); 4] {
    [
        (t.x, t.y),
        (t.x + t.w, t.y),
        (t.x, t.y + t.h),
        (t.x + t.w, t.y + t.h),
    ]
}

/// The corner grip within `grab` of `(x, y)`, if any.
pub fn handle_at(t: Transform, x: f32, y: f32, grab: f32) -> Option<usize> {
    let (x, y) = t.to_local(x, y);
    corners(t)
        .iter()
        .position(|(hx, hy)| (hx - x).abs() <= grab && (hy - y).abs() <= grab)
}

/// Where the grips actually are on the image, rotation included.
pub fn oriented_corners(t: Transform) -> [(f32, f32); 4] {
    corners(t).map(|(x, y)| t.to_image(x, y))
}

/// Drag one corner, in the box's own frame. The opposite corner stays put
/// unless `from_center`, in which case the centre does. Width and height may
/// pass through zero and come out negative, which `contains` and the
/// rasterizers already tolerate, so there is no need to forbid it.
pub fn resize_corner(
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
pub fn pin_anchor(
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

/// The box a `grip` drag that pressed at `from` on `origin` has made by the
/// time the pointer is at `to`, and the guides it snapped onto. Always
/// computed against the drag's start rather than accumulated, so rounding
/// never builds up. `snap_reach` is how near, in image pixels, an edge must
/// come to a guide of the `image`-sized canvas to snap; `None` turns snapping
/// off.
pub fn drag(
    grip: Grip,
    origin: Transform,
    from: (f32, f32),
    to: (f32, f32),
    image: (f32, f32),
    snap_reach: Option<f32>,
    keys: DragKeys,
) -> (Transform, Guides) {
    match grip {
        Grip::Move => {
            let moved = Transform {
                x: origin.x + to.0 - from.0,
                y: origin.y + to.1 - from.1,
                ..origin
            };
            match snap_reach {
                Some(reach) => snap::snap_box(moved, image, reach),
                None => (moved, Guides::default()),
            }
        }
        Grip::Resize(corner) => {
            // Corner drags are simple only in the box's own frame, so both
            // ends of the drag go through it first.
            let (fx, fy) = origin.to_local(from.0, from.1);
            let (tx, ty) = origin.to_local(to.0, to.1);
            let (mut dx, mut dy) = (tx - fx, ty - fy);
            let mut guides = Guides::default();
            // The dragged corner lands wherever the pointer takes it, from
            // the centre or not, so that is the point to pull. ponytail: only
            // an upright box without keep-aspect snaps; a rotated corner or an
            // aspect-locked one would need the guide solved back through the
            // resize.
            if let Some(reach) = snap_reach
                && origin.angle == 0.0
                && !keys.keep_aspect
            {
                let (cx, cy) = corners(origin)[corner];
                let ((sx, sy), pulled) = snap::snap_point((cx + dx, cy + dy), image, reach);
                (dx, dy, guides) = (sx - cx, sy - cy, pulled);
            }
            let resized = resize_corner(origin, corner, dx, dy, keys.keep_aspect, keys.from_center);
            let pinned = pin_anchor(origin, resized, corner, keys.from_center);
            (pinned, guides)
        }
        Grip::Rotate => {
            let (cx, cy) = origin.center();
            let start = (from.1 - cy).atan2(from.0 - cx);
            let now = (to.1 - cy).atan2(to.0 - cx);
            let mut angle = origin.angle + (now - start);
            if keys.keep_aspect {
                let step = std::f32::consts::TAU / ROTATE_STEPS;
                angle = (angle / step).round() * step;
            }
            (Transform { angle, ..origin }, Guides::default())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snap::Guide;

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

    /// A move pulls the box onto a guide only while snapping is on; with it
    /// off the box goes exactly where the pointer took it.
    #[test]
    fn a_move_snaps_only_when_asked_to() {
        let t = Transform::at(10.0, 10.0, 20.0, 20.0);
        let image = (100.0, 100.0);
        let drag_to_near_the_left_edge = |reach| {
            drag(
                Grip::Move,
                t,
                (15.0, 15.0),
                (8.0, 15.0),
                image,
                reach,
                DragKeys::default(),
            )
        };
        let (snapped, guides) = drag_to_near_the_left_edge(Some(4.0));
        assert_eq!(snapped.x, 0.0, "pulled onto the left edge");
        assert_eq!(guides.x, Some(Guide::Start));
        let (free, guides) = drag_to_near_the_left_edge(None);
        assert_eq!(free.x, 3.0);
        assert_eq!(guides, Guides::default());
    }

    /// Keep-aspect on a rotate drag lands on one of `ROTATE_STEPS` angles.
    #[test]
    fn a_constrained_rotation_lands_on_a_step() {
        let t = Transform::at(0.0, 0.0, 20.0, 20.0);
        let keys = DragKeys {
            keep_aspect: true,
            from_center: false,
        };
        let (turned, _) = drag(
            Grip::Rotate,
            t,
            (20.0, 10.0),
            (19.0, 13.0),
            (100.0, 100.0),
            None,
            keys,
        );
        let step = std::f32::consts::TAU / ROTATE_STEPS;
        let steps = turned.angle / step;
        assert!((steps - steps.round()).abs() < 1e-4, "{}", turned.angle);
        assert!(turned.angle > 0.0, "the drag turned it clockwise");
    }
}
