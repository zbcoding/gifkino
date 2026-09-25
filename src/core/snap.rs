//! Canvas guides: the image's edges and centre lines, which an overlay drag
//! pulls onto when it comes within a tolerance. Ported from Impasto's
//! `CanvasGridManager.SnapExtentToGuides`, the fallback it snaps to when no
//! grid is shown — three lines per axis that pull rather than quantize.

use super::model::Transform;

/// One of the three guides along an axis: the leading edge, the centre line,
/// or the trailing edge of the image.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Guide {
    Start,
    Center,
    End,
}

impl Guide {
    const ALL: [Guide; 3] = [Guide::Start, Guide::Center, Guide::End];

    /// Where this guide sits on an axis `extent` long.
    pub fn at(self, extent: f32) -> f32 {
        match self {
            Guide::Start => 0.0,
            Guide::Center => extent / 2.0,
            Guide::End => extent,
        }
    }
}

/// The guide a drag is resting on along each axis, if any: `x` names a
/// vertical line, `y` a horizontal one.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Guides {
    pub x: Option<Guide>,
    pub y: Option<Guide>,
}

/// Along one axis, tries the span's leading edge, centre and trailing edge
/// against each guide and returns the origin that lands the closest pair
/// together, with the guide it landed on. Nothing within `tolerance` leaves
/// the origin where it was. A point is a span of size 0.
pub fn snap_extent(origin: f32, size: f32, extent: f32, tolerance: f32) -> (f32, Option<Guide>) {
    let mut best = (origin, None);
    let mut best_distance = tolerance;
    for guide in Guide::ALL {
        let line = guide.at(extent);
        for offset in [0.0, size / 2.0, size] {
            let distance = (origin + offset - line).abs();
            if distance >= best_distance {
                continue;
            }
            best_distance = distance;
            best = (line - offset, Some(guide));
        }
    }
    best
}

/// Moves `t` so the box it paints — rotation included — lands on the nearest
/// guides of an `image`-sized canvas. The angle and size are untouched.
pub fn snap_box(t: Transform, image: (f32, f32), tolerance: f32) -> (Transform, Guides) {
    let (x0, y0, x1, y1) = painted_bounds(t);
    let (x, gx) = snap_extent(x0, x1 - x0, image.0, tolerance);
    let (y, gy) = snap_extent(y0, y1 - y0, image.1, tolerance);
    let snapped = Transform {
        x: t.x + x - x0,
        y: t.y + y - y0,
        ..t
    };
    (snapped, Guides { x: gx, y: gy })
}

/// Pulls one point, such as a dragged corner, onto the nearest guides.
pub fn snap_point((x, y): (f32, f32), image: (f32, f32), tolerance: f32) -> ((f32, f32), Guides) {
    let (x, gx) = snap_extent(x, 0.0, image.0, tolerance);
    let (y, gy) = snap_extent(y, 0.0, image.1, tolerance);
    ((x, y), Guides { x: gx, y: gy })
}

/// The axis-aligned box a rotated transform paints into, as
/// (left, top, right, bottom). Tolerates the negative width or height a
/// resize through zero leaves behind.
fn painted_bounds(t: Transform) -> (f32, f32, f32, f32) {
    let corners = [
        (t.x, t.y),
        (t.x + t.w, t.y),
        (t.x, t.y + t.h),
        (t.x + t.w, t.y + t.h),
    ]
    .map(|(x, y)| t.to_image(x, y));
    corners.iter().fold(
        (f32::MAX, f32::MAX, f32::MIN, f32::MIN),
        |(l, top, r, b), &(x, y)| (l.min(x), top.min(y), r.max(x), b.max(y)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // Offering the whole span to the guides is the point: a drag that is
    // merely near centred lands exactly centred, which a corner-only anchor
    // cannot do.
    #[test]
    fn a_span_near_the_centre_line_lands_centred() {
        assert_eq!(
            snap_extent(96.0, 100.0, 300.0, 8.0),
            (100.0, Some(Guide::Center))
        );
    }

    #[test]
    fn a_trailing_edge_near_the_far_side_lands_flush() {
        assert_eq!(
            snap_extent(196.0, 100.0, 300.0, 8.0),
            (200.0, Some(Guide::End))
        );
    }

    // Two candidates in reach: the closer one wins, so a box never jumps past
    // the line the user is actually aiming for.
    #[test]
    fn the_nearest_of_two_guides_in_reach_wins() {
        // Leading edge 5 from the left edge, centre (48) 2 from the centre line.
        assert_eq!(
            snap_extent(5.0, 86.0, 100.0, 8.0),
            (7.0, Some(Guide::Center))
        );
    }

    #[test]
    fn a_rotated_box_snaps_by_what_it_paints_not_its_unrotated_rect() {
        // A 20x100 box stood on its side paints 100 wide, 20 tall, about the
        // same centre. Its unrotated left edge is 4 from the image's left
        // edge, but what it paints hangs 36 past it.
        let t = Transform {
            angle: std::f32::consts::FRAC_PI_2,
            ..Transform::at(4.0, 40.0, 20.0, 100.0)
        };
        let (snapped, guides) = snap_box(t, (200.0, 300.0), 8.0);
        assert_eq!(guides.x, None, "painted left edge is -36, out of reach");
        // Painted 80..100 tall: out of reach of 0, 150 and 300.
        assert_eq!(guides.y, None);
        assert_eq!(snapped, t);

        // Centre 103 paints 53..153 across: 3 off the centre line.
        let t = Transform { x: 93.0, ..t };
        let (snapped, guides) = snap_box(t, (200.0, 300.0), 8.0);
        assert_eq!(guides.x, Some(Guide::Center));
        assert_eq!((snapped.w, snapped.h, snapped.angle), (t.w, t.h, t.angle));
        assert!((snapped.center().0 - 100.0).abs() < 1e-3);
    }

    #[test]
    fn a_point_snaps_each_axis_on_its_own() {
        let (p, guides) = snap_point((197.0, 60.0), (200.0, 100.0), 8.0);
        assert_eq!(p, (200.0, 60.0));
        assert_eq!(
            guides,
            Guides {
                x: Some(Guide::End),
                y: None
            }
        );
    }
}
