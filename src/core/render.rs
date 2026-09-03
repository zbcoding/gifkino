//! Compositing. The invariant: a frame's output is a pure function of its
//! pixels plus the overlays whose range contains it. Nothing is baked in as a
//! side effect.

use image::{Rgba, RgbaImage};

use super::model::{
    Document, Overlay, OverlayKind, Rgba8, Shape, ShapeOverlay, TextOverlay, Transform,
};

/// Text layout needs Pango, which lives in the UI layer. Core takes it as a
/// parameter rather than owning a font stack.
pub type TextRasterizer<'a> = &'a dyn Fn(&TextOverlay, u32, u32) -> RgbaImage;

/// A stub rasterizer for headless tests and for frames rendered before the UI
/// hands one over.
pub fn no_text(_: &TextOverlay, w: u32, h: u32) -> RgbaImage {
    RgbaImage::new(w.max(1), h.max(1))
}

pub fn composite(doc: &Document, index: usize, text: TextRasterizer<'_>) -> Option<RgbaImage> {
    let frame = doc.frames.get(index)?;
    let mut out = (*frame.pixels).clone();
    if frame.detached {
        // Its pixels are now just pixels; overlays covering it skip it.
        return Some(out);
    }
    for overlay in doc.overlays_on(index) {
        stamp(&mut out, overlay, text);
    }
    Some(out)
}

fn stamp(dst: &mut RgbaImage, overlay: &Overlay, text: TextRasterizer<'_>) {
    let t = overlay.transform;
    let (w, h) = (t.w.abs().round() as u32, t.h.abs().round() as u32);
    if w == 0 || h == 0 || overlay.opacity <= 0.0 {
        return;
    }
    let src = match &overlay.kind {
        OverlayKind::Text(o) => text(o, w, h),
        OverlayKind::Shape(o) => rasterize_shape(o, w, h),
        OverlayKind::Image(o) => image::imageops::resize(
            o.pixels.as_ref(),
            w,
            h,
            image::imageops::FilterType::Triangle,
        ),
    };
    draw_transformed(dst, &src, t, overlay.opacity);
}

/// Place `src` under the overlay's oriented box. Rotation samples nearest
/// neighbour; GIF output is 256 colors and the input is mostly flat UI color.
fn draw_transformed(dst: &mut RgbaImage, src: &RgbaImage, t: Transform, opacity: f32) {
    let (dw, dh) = dst.dimensions();
    let (sw, sh) = src.dimensions();
    let alpha = opacity.clamp(0.0, 1.0);

    if t.angle == 0.0 {
        // `t.w`/`t.h` go negative when a corner drag crosses the opposite
        // edge (`resize_corner` flips the box instead of clamping it), and
        // `t.x`/`t.y` stay pinned to the un-dragged corner, which then reads
        // as the box's right/bottom edge rather than its left/top. Walking
        // forward from `t.x` regardless of sign drew every flipped overlay a
        // full box-width off from where its own selection handles sat.
        // Instead walk the destination forward from the box's actual
        // top-left and read the source backward on a flipped axis, matching
        // what the rotated path below derives from `(px - t.x) / t.w`.
        let flip_x = t.w < 0.0;
        let flip_y = t.h < 0.0;
        let ox = t.x.min(t.x + t.w).round() as i64;
        let oy = t.y.min(t.y + t.h).round() as i64;
        for y in 0..sh {
            for x in 0..sw {
                let (dx, dy) = (ox + x as i64, oy + y as i64);
                if dx < 0 || dy < 0 || dx >= dw as i64 || dy >= dh as i64 {
                    continue;
                }
                let sx = if flip_x { sw - 1 - x } else { x };
                let sy = if flip_y { sh - 1 - y } else { y };
                blend(dst, dx as u32, dy as u32, src.get_pixel(sx, sy).0, alpha);
            }
        }
        return;
    }

    let (cx, cy) = t.center();
    let (sin, cos) = t.angle.sin_cos();
    // Bounding box of the rotated rect, clipped to the canvas.
    let corners = [
        (t.x, t.y),
        (t.x + t.w, t.y),
        (t.x, t.y + t.h),
        (t.x + t.w, t.y + t.h),
    ];
    let rot = |(px, py): (f32, f32)| {
        let (ux, uy) = (px - cx, py - cy);
        (cx + ux * cos - uy * sin, cy + ux * sin + uy * cos)
    };
    let pts: Vec<(f32, f32)> = corners.into_iter().map(rot).collect();
    let min_x = pts
        .iter()
        .map(|p| p.0)
        .fold(f32::MAX, f32::min)
        .floor()
        .max(0.0) as u32;
    let max_x = (pts.iter().map(|p| p.0).fold(f32::MIN, f32::max).ceil()).min(dw as f32) as u32;
    let min_y = pts
        .iter()
        .map(|p| p.1)
        .fold(f32::MAX, f32::min)
        .floor()
        .max(0.0) as u32;
    let max_y = (pts.iter().map(|p| p.1).fold(f32::MIN, f32::max).ceil()).min(dh as f32) as u32;

    for dy in min_y..max_y {
        for dx in min_x..max_x {
            // inverse-rotate the destination pixel into un-rotated box space
            let (ux, uy) = (dx as f32 + 0.5 - cx, dy as f32 + 0.5 - cy);
            let px = cx + ux * cos + uy * sin;
            let py = cy - ux * sin + uy * cos;
            let sx = ((px - t.x) / t.w * sw as f32).floor();
            let sy = ((py - t.y) / t.h * sh as f32).floor();
            if sx < 0.0 || sy < 0.0 || sx >= sw as f32 || sy >= sh as f32 {
                continue;
            }
            blend(dst, dx, dy, src.get_pixel(sx as u32, sy as u32).0, alpha);
        }
    }
}

fn blend(dst: &mut RgbaImage, x: u32, y: u32, src: Rgba8, opacity: f32) {
    let a = src[3] as f32 / 255.0 * opacity;
    if a <= 0.0 {
        return;
    }
    let d = dst.get_pixel(x, y).0;
    let da = d[3] as f32 / 255.0;
    let out_a = a + da * (1.0 - a);
    let mix = |s: u8, d: u8| {
        if out_a <= 0.0 {
            0
        } else {
            ((s as f32 * a + d as f32 * da * (1.0 - a)) / out_a).round() as u8
        }
    };
    dst.put_pixel(
        x,
        y,
        Rgba([
            mix(src[0], d[0]),
            mix(src[1], d[1]),
            mix(src[2], d[2]),
            (out_a * 255.0).round() as u8,
        ]),
    );
}

fn rasterize_shape(o: &ShapeOverlay, w: u32, h: u32) -> RgbaImage {
    let mut img = RgbaImage::new(w, h);
    let stroke_w = o.stroke.map_or(0.0, |(_, sw)| sw.max(0.0));
    match o.shape {
        Shape::Rect => {
            for y in 0..h {
                for x in 0..w {
                    let edge = (x as f32) < stroke_w
                        || (y as f32) < stroke_w
                        || ((w - 1 - x) as f32) < stroke_w
                        || ((h - 1 - y) as f32) < stroke_w;
                    put(&mut img, x, y, pick(o, edge));
                }
            }
        }
        Shape::Ellipse => {
            let (rx, ry) = (w as f32 / 2.0, h as f32 / 2.0);
            for y in 0..h {
                for x in 0..w {
                    let nx = (x as f32 + 0.5 - rx) / rx;
                    let ny = (y as f32 + 0.5 - ry) / ry;
                    let d = nx * nx + ny * ny;
                    if d > 1.0 {
                        continue;
                    }
                    let inner_x = (rx - stroke_w).max(0.001);
                    let inner_y = (ry - stroke_w).max(0.001);
                    let ix = (x as f32 + 0.5 - rx) / inner_x;
                    let iy = (y as f32 + 0.5 - ry) / inner_y;
                    put(&mut img, x, y, pick(o, ix * ix + iy * iy > 1.0));
                }
            }
        }
        Shape::Arrow => rasterize_arrow(&mut img, o, w, h),
    }
    img
}

/// Left-to-right within the box; direction comes from the transform's angle.
/// Same edge rule as `rasterize_shape`'s Rect and Ellipse arms: a band
/// `stroke_w` in from the outer silhouette picks the stroke color (falling
/// back to fill so a strokeless arrow stays solid, matching the old
/// behaviour), and the interior picks fill — nothing, if fill is unset,
/// leaving it hollow. Previously the whole arrow painted one flat color
/// (fill if set, else stroke, else white) and `stroke`'s width did nothing.
fn rasterize_arrow(img: &mut RgbaImage, o: &ShapeOverlay, w: u32, h: u32) {
    let stroke_w = o.stroke.map_or(0.0, |(_, sw)| sw.max(0.0));
    let head_len = (w as f32 * 0.35).min(h as f32 * 1.5);
    let shaft_h = (h as f32 * 0.3).max(1.0);
    let mid = h as f32 / 2.0;
    let inner_head_len = (head_len - stroke_w).max(0.0);
    let inner_shaft_h = (shaft_h - stroke_w * 2.0).max(0.0);
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let in_shaft = fx < w as f32 - head_len && (fy - mid).abs() <= shaft_h / 2.0;
            let head_t = ((w as f32 - fx) / head_len).clamp(0.0, 1.0);
            let in_head = fx >= w as f32 - head_len && (fy - mid).abs() <= head_t * mid;
            if !in_shaft && !in_head {
                continue;
            }
            let in_inner_shaft = fx >= stroke_w
                && fx < w as f32 - inner_head_len
                && (fy - mid).abs() <= inner_shaft_h / 2.0;
            let inner_head_t =
                ((w as f32 - fx - stroke_w) / inner_head_len.max(0.001)).clamp(0.0, 1.0);
            let in_inner_head = fx >= w as f32 - inner_head_len
                && fx <= w as f32 - stroke_w
                && (fy - mid).abs() <= inner_head_t * (mid - stroke_w).max(0.0);
            put(img, x, y, pick(o, !(in_inner_shaft || in_inner_head)));
        }
    }
}

fn pick(o: &ShapeOverlay, on_edge: bool) -> Option<Rgba8> {
    if on_edge {
        o.stroke.map(|(c, _)| c).or(o.fill)
    } else {
        o.fill
    }
}

fn put(img: &mut RgbaImage, x: u32, y: u32, color: Option<Rgba8>) {
    if let Some(c) = color {
        img.put_pixel(x, y, Rgba(c));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::Frame;

    fn red_rect() -> OverlayKind {
        OverlayKind::Shape(ShapeOverlay {
            shape: Shape::Rect,
            fill: Some([255, 0, 0, 255]),
            stroke: None,
        })
    }

    fn doc() -> Document {
        Document::from_frames(
            (0..4)
                .map(|_| Frame::new(RgbaImage::new(10, 10), 10))
                .collect(),
        )
    }

    #[test]
    fn one_overlay_paints_only_its_range() {
        let mut d = doc();
        d.add_overlay("r", red_rect(), Transform::at(2.0, 2.0, 4.0, 4.0), 1..3);
        let painted = |i| composite(&d, i, &no_text).unwrap().get_pixel(3, 3).0;
        assert_eq!(painted(0), [0, 0, 0, 0]);
        assert_eq!(painted(1), [255, 0, 0, 255]);
        assert_eq!(painted(2), [255, 0, 0, 255]);
        assert_eq!(painted(3), [0, 0, 0, 0]);
        // and only inside the box
        assert_eq!(
            composite(&d, 1, &no_text).unwrap().get_pixel(8, 8).0,
            [0, 0, 0, 0]
        );
    }

    #[test]
    fn detached_frames_skip_overlays() {
        let mut d = doc();
        d.add_overlay("r", red_rect(), Transform::at(0.0, 0.0, 10.0, 10.0), 0..4);
        d.frames[2].detached = true;
        assert_eq!(
            composite(&d, 1, &no_text).unwrap().get_pixel(5, 5).0,
            [255, 0, 0, 255]
        );
        assert_eq!(
            composite(&d, 2, &no_text).unwrap().get_pixel(5, 5).0,
            [0, 0, 0, 0]
        );
    }

    #[test]
    fn hidden_and_transparent_overlays_do_nothing() {
        let mut d = doc();
        let id = d.add_overlay("r", red_rect(), Transform::at(0.0, 0.0, 10.0, 10.0), 0..4);
        d.overlay_mut(id).unwrap().hidden = true;
        assert_eq!(
            composite(&d, 0, &no_text).unwrap().get_pixel(5, 5).0,
            [0, 0, 0, 0]
        );
        d.overlay_mut(id).unwrap().hidden = false;
        d.overlay_mut(id).unwrap().opacity = 0.5;
        assert_eq!(
            composite(&d, 0, &no_text).unwrap().get_pixel(5, 5).0[3],
            128
        );
    }

    #[test]
    fn quarter_turn_lands_where_it_should() {
        let mut d = Document::from_frames(vec![Frame::new(RgbaImage::new(20, 20), 10)]);
        // a wide bar across the middle, rotated a quarter turn, becomes a tall one
        let id = d.add_overlay("r", red_rect(), Transform::at(2.0, 8.0, 16.0, 4.0), 0..1);
        d.overlay_mut(id).unwrap().transform.angle = std::f32::consts::FRAC_PI_2;
        let out = composite(&d, 0, &no_text).unwrap();
        assert_eq!(
            out.get_pixel(10, 3).0,
            [255, 0, 0, 255],
            "tall after rotation"
        );
        assert_eq!(out.get_pixel(3, 10).0, [0, 0, 0, 0], "no longer wide");
    }

    #[test]
    fn z_order_is_bottom_to_top() {
        let mut d = doc();
        d.add_overlay(
            "under",
            red_rect(),
            Transform::at(0.0, 0.0, 10.0, 10.0),
            0..4,
        );
        d.add_overlay(
            "over",
            OverlayKind::Shape(ShapeOverlay {
                shape: Shape::Rect,
                fill: Some([0, 0, 255, 255]),
                stroke: None,
            }),
            Transform::at(0.0, 0.0, 10.0, 10.0),
            0..4,
        );
        assert_eq!(
            composite(&d, 0, &no_text).unwrap().get_pixel(5, 5).0,
            [0, 0, 255, 255]
        );
    }

    #[test]
    fn a_flipped_box_paints_within_its_own_bounding_box_not_beyond_it() {
        // `resize_corner` flips `w`/`h` negative rather than clamping when a
        // corner drag crosses the opposite edge, and `contains`/the selection
        // handles already treat that box as spanning `x+w..x`. The old fast
        // path here ignored the sign and always walked forward from `x`,
        // painting the overlay a full box-width off from its own handles.
        let mut d = Document::from_frames(vec![Frame::new(RgbaImage::new(10, 10), 10)]);
        d.add_overlay("r", red_rect(), Transform::at(8.0, 2.0, -6.0, 6.0), 0..1);
        let out = composite(&d, 0, &no_text).unwrap();
        assert_eq!(
            out.get_pixel(3, 5).0,
            [255, 0, 0, 255],
            "inside the flipped box, which spans x 2..8"
        );
        assert_eq!(
            out.get_pixel(9, 5).0,
            [0, 0, 0, 0],
            "outside it — the old code painted here regardless of the negative width"
        );
    }

    #[test]
    fn arrow_stroke_draws_a_distinct_band_from_fill() {
        let mut d = Document::from_frames(vec![Frame::new(RgbaImage::new(40, 40), 10)]);
        let arrow = OverlayKind::Shape(ShapeOverlay {
            shape: Shape::Arrow,
            fill: Some([0, 255, 0, 255]),
            stroke: Some(([255, 0, 0, 255], 2.0)),
        });
        d.add_overlay("a", arrow, Transform::at(0.0, 0.0, 40.0, 40.0), 0..1);
        let out = composite(&d, 0, &no_text).unwrap();
        assert_eq!(
            out.get_pixel(3, 20).0,
            [0, 255, 0, 255],
            "shaft centre is fill"
        );
        assert_eq!(
            out.get_pixel(3, 14).0,
            [255, 0, 0, 255],
            "near the shaft's outer edge is stroke, not flattened to one solid colour"
        );
    }

    #[test]
    fn arrow_with_only_a_stroke_is_hollow() {
        let mut d = Document::from_frames(vec![Frame::new(RgbaImage::new(40, 40), 10)]);
        let arrow = OverlayKind::Shape(ShapeOverlay {
            shape: Shape::Arrow,
            fill: None,
            stroke: Some(([255, 0, 0, 255], 2.0)),
        });
        d.add_overlay("a", arrow, Transform::at(0.0, 0.0, 40.0, 40.0), 0..1);
        let out = composite(&d, 0, &no_text).unwrap();
        assert_eq!(
            out.get_pixel(3, 20).0,
            [0, 0, 0, 0],
            "no fill leaves the interior see-through, like Rect and Ellipse"
        );
        assert_eq!(
            out.get_pixel(3, 14).0,
            [255, 0, 0, 255],
            "the stroke band still draws"
        );
    }
}
