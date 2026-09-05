//! Fitting frames that arrive from a file onto the canvas the document
//! already has. A GIF has one canvas, so anything spliced in — a PNG, another
//! animation's frames, a video's — has to end up the same size as the frames
//! around it, and there is no single right way to get there. The user picks
//! one of four, and every importer routes through here rather than deciding
//! for itself.

use image::RgbaImage;

use super::model::{Document, Frame};

/// What gives way when incoming frames are not the canvas size. Two
/// questions, four answers: whose size wins — the open document's canvas or
/// the incoming file's — and whether the side that loses is stretched to the
/// winner's aspect ratio or scaled inside it with transparency around it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FitMode {
    /// Incoming frames stretched onto the canvas.
    Stretch,
    /// Incoming frames scaled to fit inside the canvas, transparency around.
    Pad,
    /// The canvas becomes the incoming size; the frames already in the
    /// document are scaled to fit inside it, transparency around them.
    GrowPad,
    /// The canvas becomes the incoming size; the frames already in the
    /// document are stretched onto it.
    GrowStretch,
}

impl FitMode {
    /// Menu order, so the chooser and any test walk the same list.
    pub const ALL: [FitMode; 4] = [
        FitMode::Stretch,
        FitMode::Pad,
        FitMode::GrowPad,
        FitMode::GrowStretch,
    ];

    /// Whether the canvas takes the incoming size, which is what decides
    /// which side of the splice has to be redrawn.
    pub fn grows_canvas(self) -> bool {
        matches!(self, FitMode::GrowPad | FitMode::GrowStretch)
    }

    /// Whether the side being redrawn keeps its proportions and gains
    /// transparent margins, rather than being stretched to fill.
    pub fn keeps_aspect(self) -> bool {
        matches!(self, FitMode::Pad | FitMode::GrowPad)
    }

    /// The canvas the document ends up with. An empty side has no size to
    /// impose, so the other one wins whatever the mode says.
    pub fn canvas(self, doc: (u32, u32), incoming: (u32, u32)) -> (u32, u32) {
        if is_empty(doc) {
            return incoming;
        }
        if is_empty(incoming) || !self.grows_canvas() {
            return doc;
        }
        incoming
    }

    /// Where a `src`-sized image lands on a `dst`-sized canvas under this mode.
    pub fn placement(self, src: (u32, u32), dst: (u32, u32)) -> Placement {
        if is_empty(src) || is_empty(dst) || src == dst {
            return Placement::identity(dst);
        }
        if !self.keeps_aspect() {
            return Placement {
                scale: (dst.0 as f32 / src.0 as f32, dst.1 as f32 / src.1 as f32),
                offset: (0, 0),
                size: dst,
            };
        }
        let factor = (dst.0 as f32 / src.0 as f32).min(dst.1 as f32 / src.1 as f32);
        let size = (
            ((src.0 as f32 * factor).round() as u32).clamp(1, dst.0),
            ((src.1 as f32 * factor).round() as u32).clamp(1, dst.1),
        );
        Placement {
            scale: (factor, factor),
            offset: ((dst.0 - size.0) / 2, (dst.1 - size.1) / 2),
            size,
        }
    }
}

/// Where a scaled image sits on the canvas: the factor it was scaled by on
/// each axis, the top-left corner it was drawn at, and the size it came out.
/// Pixels and overlay boxes both follow this one answer, which is what keeps a
/// caption over the thing it was written on when the canvas changes under it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Placement {
    pub scale: (f32, f32),
    pub offset: (u32, u32),
    pub size: (u32, u32),
}

impl Placement {
    fn identity(size: (u32, u32)) -> Self {
        Placement {
            scale: (1.0, 1.0),
            offset: (0, 0),
            size,
        }
    }
}

fn is_empty((w, h): (u32, u32)) -> bool {
    w == 0 || h == 0
}

/// Redraw `img` at `dst`. Stretching resamples straight onto the canvas;
/// keeping the aspect ratio resamples by the smaller factor and centers the
/// result on a transparent canvas, so nothing is distorted and nothing is cut.
pub fn refit(img: &RgbaImage, dst: (u32, u32), mode: FitMode) -> RgbaImage {
    let placed = mode.placement(img.dimensions(), dst);
    let scaled = image::imageops::resize(
        img,
        placed.size.0,
        placed.size.1,
        image::imageops::FilterType::Triangle,
    );
    if placed.size == dst {
        return scaled;
    }
    let mut canvas = RgbaImage::new(dst.0, dst.1);
    image::imageops::replace(
        &mut canvas,
        &scaled,
        placed.offset.0 as i64,
        placed.offset.1 as i64,
    );
    canvas
}

/// A splice worked out but not yet landed: the slow resampling is done, the
/// document has not been touched. `apply` is the cheap half, so the worker
/// thread can produce this and the main thread can land it as one history
/// step.
#[derive(Debug)]
pub struct Splice {
    /// Where the incoming run goes in the timeline.
    pub at: usize,
    /// Frames already in the document that the new canvas forced a redraw of,
    /// by index. Empty when the canvas did not change.
    pub existing: Vec<(usize, Frame)>,
    /// The incoming frames, fitted, in order.
    pub incoming: Vec<Frame>,
    /// How the frames already in the document moved, and so how their
    /// overlays have to move to stay on them.
    pub overlays: Placement,
}

/// Work out how `incoming` and the document's own frames both end up on one
/// canvas under `mode`. Every resample happens here, so this is the half that
/// belongs on a worker thread; `progress` reports `(done, total)` over the
/// frames that actually need redrawing.
pub fn plan_splice(
    doc: &Document,
    at: usize,
    incoming: Vec<Frame>,
    mode: FitMode,
    mut progress: impl FnMut(usize, usize),
) -> Splice {
    let canvas_before = doc.size();
    let arriving = incoming.first().map_or((0, 0), |f| f.pixels.dimensions());
    let canvas = mode.canvas(canvas_before, arriving);

    let needs_work = |frame: &Frame| frame.pixels.dimensions() != canvas;
    let total = doc.frames.iter().filter(|f| needs_work(f)).count()
        + incoming.iter().filter(|f| needs_work(f)).count();
    let mut done = 0;
    let mut redraw = |frame: &Frame| {
        let mut produced = Frame::new(refit(frame.pixels.as_ref(), canvas, mode), frame.delay_cs);
        produced.detached = frame.detached;
        done += 1;
        progress(done, total);
        produced
    };

    let existing = doc
        .frames
        .iter()
        .enumerate()
        .filter(|(_, frame)| needs_work(frame))
        .map(|(i, frame)| (i, redraw(frame)))
        .collect();
    let incoming = incoming
        .into_iter()
        .map(|frame| {
            if needs_work(&frame) {
                redraw(&frame)
            } else {
                frame
            }
        })
        .collect();

    Splice {
        at,
        existing,
        incoming,
        overlays: mode.placement(canvas_before, canvas),
    }
}

impl Splice {
    /// Land the splice: redrawn frames replace theirs, the incoming run goes
    /// in at `at`, and every overlay follows the canvas it was drawn on. The
    /// answer is how many frames the document gained, for the history step.
    pub fn apply(self, doc: &mut Document) -> usize {
        for (i, frame) in self.existing {
            if let Some(slot) = doc.frames.get_mut(i) {
                *slot = frame;
            }
        }
        let (fx, fy) = self.overlays.scale;
        doc.scale_overlays(fx, fy);
        let (dx, dy) = self.overlays.offset;
        for overlay in &mut doc.overlays {
            overlay.transform.x += dx as f32;
            overlay.transform.y += dy as f32;
        }
        let added = self.incoming.len();
        doc.insert_foreign_frames_at(self.at, self.incoming);
        added
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{OverlayKind, Shape, ShapeOverlay, Transform};

    fn frame(w: u32, h: u32) -> Frame {
        Frame::new(RgbaImage::from_pixel(w, h, image::Rgba([9, 9, 9, 255])), 10)
    }

    fn doc(size: (u32, u32), count: usize) -> Document {
        Document::from_frames((0..count).map(|_| frame(size.0, size.1)).collect())
    }

    #[test]
    fn only_the_growing_modes_move_the_canvas() {
        let canvas = (100, 50);
        let arriving = (40, 40);
        assert_eq!(FitMode::Stretch.canvas(canvas, arriving), canvas);
        assert_eq!(FitMode::Pad.canvas(canvas, arriving), canvas);
        assert_eq!(FitMode::GrowPad.canvas(canvas, arriving), arriving);
        assert_eq!(FitMode::GrowStretch.canvas(canvas, arriving), arriving);
    }

    /// An empty document has no canvas to impose, so the first file in decides
    /// it whichever mode the chooser was left on.
    #[test]
    fn an_empty_document_takes_the_incoming_size() {
        for mode in FitMode::ALL {
            assert_eq!(mode.canvas((0, 0), (32, 24)), (32, 24), "{mode:?}");
        }
    }

    #[test]
    fn keeping_the_aspect_ratio_centers_and_scales_by_the_smaller_factor() {
        let placed = FitMode::Pad.placement((40, 40), (100, 50));
        assert_eq!(placed.size, (50, 50), "height is the binding axis");
        assert_eq!(placed.offset, (25, 0), "centered on the wide axis");
        assert_eq!(placed.scale, (1.25, 1.25), "one factor, so nothing skews");
    }

    #[test]
    fn stretching_fills_the_canvas_with_two_factors() {
        let placed = FitMode::Stretch.placement((40, 40), (100, 50));
        assert_eq!(placed.size, (100, 50));
        assert_eq!(placed.offset, (0, 0));
        assert_eq!(placed.scale, (2.5, 1.25));
    }

    /// The padding has to be transparent, not black: the frame is composited
    /// over whatever the GIF's background is, and a black bar is not a margin.
    #[test]
    fn padding_is_transparent() {
        let solid = RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]));
        let padded = refit(&solid, (12, 4), FitMode::Pad);
        assert_eq!(padded.dimensions(), (12, 4));
        assert_eq!(padded.get_pixel(0, 0)[3], 0, "left margin is transparent");
        assert_eq!(padded.get_pixel(11, 3)[3], 0, "right margin is transparent");
        assert_eq!(padded.get_pixel(6, 2), &image::Rgba([1, 2, 3, 255]));
    }

    fn add_caption(document: &mut Document, box_: Transform, range: std::ops::Range<usize>) {
        document.add_overlay(
            "caption",
            OverlayKind::Shape(ShapeOverlay {
                shape: Shape::Rect,
                fill: Some([255, 0, 0, 255]),
                stroke: None,
            }),
            box_,
            range,
        );
    }

    #[test]
    fn a_matching_file_is_spliced_without_redrawing_anything() {
        let document = doc((32, 24), 3);
        let mut ticks = 0;
        let splice = plan_splice(&document, 3, vec![frame(32, 24)], FitMode::Pad, |_, _| {
            ticks += 1
        });
        assert!(splice.existing.is_empty(), "the canvas did not change");
        assert_eq!(ticks, 0, "nothing was resampled, so nothing was reported");
    }

    #[test]
    fn a_non_growing_mode_only_redraws_what_is_arriving() {
        let document = doc((32, 24), 3);
        let splice = plan_splice(
            &document,
            3,
            vec![frame(64, 64)],
            FitMode::Stretch,
            |_, _| {},
        );
        assert!(
            splice.existing.is_empty(),
            "the document's frames are untouched"
        );
        assert_eq!(splice.incoming[0].pixels.dimensions(), (32, 24));
        assert_eq!(splice.overlays.scale, (1.0, 1.0), "the canvas did not move");
    }

    /// The growing modes are the reason this is threaded: every frame the
    /// document already had is resampled, not just the one arriving.
    #[test]
    fn a_growing_mode_redraws_the_whole_document_onto_the_new_canvas() {
        let document = doc((32, 24), 3);
        let mut seen = Vec::new();
        let splice = plan_splice(
            &document,
            3,
            vec![frame(64, 64)],
            FitMode::GrowPad,
            |done, total| seen.push((done, total)),
        );
        assert_eq!(splice.existing.len(), 3);
        for (_, frame) in &splice.existing {
            assert_eq!(frame.pixels.dimensions(), (64, 64));
        }
        assert_eq!(splice.incoming[0].pixels.dimensions(), (64, 64));
        assert_eq!(
            seen,
            vec![(1, 3), (2, 3), (3, 3)],
            "progress counts only the frames that needed redrawing"
        );
    }

    /// A caption written on the old canvas has to end up over the same part of
    /// the picture after the picture is scaled and centered under it.
    #[test]
    fn overlays_follow_the_frames_onto_the_grown_canvas() {
        let mut document = doc((40, 40), 2);
        add_caption(&mut document, Transform::at(0.0, 0.0, 20.0, 20.0), 0..2);
        let splice = plan_splice(
            &document,
            2,
            vec![frame(100, 50)],
            FitMode::GrowPad,
            |_, _| {},
        );
        let placed = splice.overlays;
        splice.apply(&mut document);
        assert_eq!(document.size(), (100, 50), "the canvas grew to the image");
        assert_eq!(placed.scale, (1.25, 1.25));
        let box_ = document.overlays[0].transform;
        assert_eq!((box_.x, box_.y), (25.0, 0.0), "shifted by the margin");
        assert_eq!((box_.w, box_.h), (25.0, 25.0), "scaled with the frames");
    }

    /// The frames coming in are somebody else's footage; an overlay that
    /// happened to end where they land must not grow over them.
    #[test]
    fn an_appended_run_does_not_inherit_the_overlay_it_lands_after() {
        let mut document = doc((32, 24), 2);
        add_caption(&mut document, Transform::at(0.0, 0.0, 8.0, 8.0), 0..2);
        let splice = plan_splice(
            &document,
            2,
            vec![frame(32, 24), frame(32, 24)],
            FitMode::Stretch,
            |_, _| {},
        );
        assert_eq!(splice.apply(&mut document), 2, "two frames were added");
        assert_eq!(document.frames.len(), 4);
        assert_eq!(document.overlays[0].range, 0..2, "the caption stayed put");
    }
}
