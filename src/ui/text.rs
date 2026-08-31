//! Pango text rasterization, the one piece of overlay rendering core cannot do
//! headlessly. Outline-then-fill, so a caption stays readable over any capture.

use cairo::{Context, Format, ImageSurface};
use gtk4::pango;
use image::{Rgba, RgbaImage};

use crate::core::{TextAlign, TextOverlay};

pub fn rasterize(overlay: &TextOverlay, w: u32, h: u32) -> RgbaImage {
    let (w, h) = (w.max(1), h.max(1));
    render(overlay, w, h).unwrap_or_else(|_| RgbaImage::new(w, h))
}

fn render(overlay: &TextOverlay, w: u32, h: u32) -> Result<RgbaImage, cairo::Error> {
    let surface = ImageSurface::create(Format::ARgb32, w as i32, h as i32)?;
    let cr = Context::new(&surface)?;

    // Set before anything is painted: it governs the glyph path fill and the
    // outline stroke alike, since the text goes down as a path, not as glyphs.
    cr.set_antialias(if overlay.antialias {
        cairo::Antialias::Best
    } else {
        cairo::Antialias::None
    });

    let layout = pangocairo::functions::create_layout(&cr);
    let mut font = pango::FontDescription::from_string(&overlay.font);
    font.set_absolute_size(overlay.size_px as f64 * pango::SCALE as f64);
    layout.set_font_description(Some(&font));
    layout.set_text(&overlay.text);
    layout.set_width(w as i32 * pango::SCALE);
    let (alignment, justify) = match overlay.align {
        TextAlign::Left => (pango::Alignment::Left, false),
        TextAlign::Center => (pango::Alignment::Center, false),
        TextAlign::Right => (pango::Alignment::Right, false),
        // Pango justifies by stretching to the alignment's edge, so the flag
        // needs a left alignment under it to mean what "justified" usually does.
        TextAlign::Justify => (pango::Alignment::Left, true),
    };
    layout.set_alignment(alignment);
    layout.set_justify(justify);
    layout.set_wrap(pango::WrapMode::WordChar);

    // vertically centered in the overlay's box
    let (_, text_h) = layout.pixel_size();
    cr.move_to(0.0, ((h as i32 - text_h) as f64 / 2.0).max(0.0));
    pangocairo::functions::layout_path(&cr, &layout);

    if let Some((color, width)) = overlay.outline {
        set_color(&cr, color);
        cr.set_line_width(width as f64 * 2.0);
        cr.set_line_join(cairo::LineJoin::Round);
        cr.stroke_preserve()?;
    }
    set_color(&cr, overlay.color);
    cr.fill()?;

    drop(cr);
    to_rgba(surface)
}

fn set_color(cr: &Context, [r, g, b, a]: [u8; 4]) {
    cr.set_source_rgba(
        r as f64 / 255.0,
        g as f64 / 255.0,
        b as f64 / 255.0,
        a as f64 / 255.0,
    );
}

/// Cairo's ARgb32 is premultiplied BGRA on little-endian machines.
fn to_rgba(surface: ImageSurface) -> Result<RgbaImage, cairo::Error> {
    let (w, h, stride) = (
        surface.width() as u32,
        surface.height() as u32,
        surface.stride() as usize,
    );
    let data = surface
        .take_data()
        .map_err(|_| cairo::Error::SurfaceFinished)?;
    let mut out = RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let i = y as usize * stride + x as usize * 4;
            let (b, g, r, a) = (data[i], data[i + 1], data[i + 2], data[i + 3]);
            let un = |c: u8| {
                if a == 0 {
                    0
                } else {
                    ((c as u32 * 255) / a as u32).min(255) as u8
                }
            };
            out.put_pixel(x, y, Rgba([un(r), un(g), un(b), a]));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::render::composite;
    use crate::core::{Document, Frame, OverlayKind, Transform};
    use image::{Rgba, RgbaImage};

    #[test]
    fn text_rasterizes_opaque_pixels_with_an_outline() {
        let overlay = TextOverlay {
            text: "Hello".into(),
            size_px: 40.0,
            ..Default::default()
        };
        let img = rasterize(&overlay, 200, 60);
        assert_eq!(img.dimensions(), (200, 60));

        let opaque = img.pixels().filter(|p| p.0[3] > 200).count();
        assert!(opaque > 100, "expected glyphs, got {opaque} opaque pixels");
        // white fill and a black outline both survive the premultiply round-trip
        assert!(img.pixels().any(|p| p.0[3] > 200 && p.0[0] > 240));
        assert!(img.pixels().any(|p| p.0[3] > 200 && p.0[0] < 20));
        // and the box is not filled edge to edge
        assert!(img.pixels().filter(|p| p.0[3] == 0).count() > 1000);
    }

    /// Alignment moves the glyphs within the box; justification is a flag under
    /// a left alignment, so it has to differ from plain left on wrapped text.
    #[test]
    fn alignment_places_the_glyphs_across_the_box() {
        let ink = |align| {
            let overlay = TextOverlay {
                text: "hi".into(),
                size_px: 30.0,
                align,
                ..Default::default()
            };
            let img = rasterize(&overlay, 300, 50);
            let xs: Vec<u32> = img
                .enumerate_pixels()
                .filter(|(_, _, p)| p.0[3] > 200)
                .map(|(x, _, _)| x)
                .collect();
            (*xs.iter().min().unwrap(), *xs.iter().max().unwrap())
        };
        let (left_min, left_max) = ink(TextAlign::Left);
        let (mid_min, mid_max) = ink(TextAlign::Center);
        let (right_min, right_max) = ink(TextAlign::Right);
        assert!(left_min < mid_min && mid_min < right_min, "left to right");
        assert!(left_max < mid_max && mid_max < right_max);
        assert!(right_max > 280, "right aligned text reaches the far edge");

        // Wrapped text, where justification has something to stretch: every
        // line but the last is pushed out to the right margin, so the ink
        // reaches further right than ragged left ever does.
        let rightmost = |align| {
            let overlay = TextOverlay {
                text: "the quick brown fox jumps over it".into(),
                size_px: 14.0,
                align,
                ..Default::default()
            };
            rasterize(&overlay, 120, 100)
                .enumerate_pixels()
                .filter(|(_, _, p)| p.0[3] > 200)
                .map(|(x, _, _)| x)
                .max()
                .unwrap()
        };
        assert!(
            rightmost(TextAlign::Justify) > rightmost(TextAlign::Left),
            "justified reaches the margin, ragged left stops short of it"
        );
    }

    /// Off means hard edges: with no antialiasing every glyph pixel is either
    /// fully opaque or fully clear, so the partial coverage disappears.
    #[test]
    fn antialiasing_is_the_difference_between_soft_and_hard_edges() {
        let partial = |antialias| {
            let overlay = TextOverlay {
                text: "Sg".into(),
                size_px: 48.0,
                antialias,
                ..Default::default()
            };
            rasterize(&overlay, 120, 60)
                .pixels()
                .filter(|p| (1..255).contains(&p.0[3]))
                .count()
        };
        assert_eq!(partial(false), 0, "no antialiasing, no partial coverage");
        assert!(partial(true) > 20, "the default softens the edges");
        assert!(TextOverlay::default().antialias, "on by default");
    }

    /// The whole premise, through the real font stack: one overlay, typed once,
    /// paints the frames its range covers and no others.
    #[test]
    fn a_caption_written_once_paints_its_whole_range() {
        let mut doc = Document::from_frames(
            (0..4)
                .map(|_| Frame::new(RgbaImage::from_pixel(240, 100, Rgba([20, 40, 90, 255])), 25))
                .collect(),
        );
        doc.add_overlay(
            "caption",
            OverlayKind::Text(TextOverlay {
                text: "one edit".into(),
                size_px: 24.0,
                ..Default::default()
            }),
            Transform::at(0.0, 30.0, 240.0, 40.0),
            1..4,
        );
        let glyph_pixels = |i| {
            composite(&doc, i, &rasterize)
                .unwrap()
                .pixels()
                .filter(|p| p.0[0] > 240 && p.0[1] > 240)
                .count()
        };
        assert_eq!(glyph_pixels(0), 0, "outside the range, nothing is painted");
        assert!(glyph_pixels(1) > 50);
        assert!(glyph_pixels(3) > 50);
    }
}
