//! Frame-list math. Pure list logic, which is why it is tested here and not
//! through the UI.

use std::ops::Range;

use image::RgbaImage;

use super::model::{Document, Frame};

impl Document {
    /// Delete the frames `keep` returns false for, moving each dropped frame's
    /// delay onto the previous surviving frame so total duration is preserved,
    /// and remapping overlay ranges onto the new indices.
    fn retain_frames(&mut self, keep: impl Fn(usize) -> bool) {
        let n = self.frames.len();
        let keep: Vec<bool> = (0..n).map(&keep).collect();
        if keep.iter().all(|k| *k) {
            return;
        }

        let mut frames = Vec::with_capacity(keep.iter().filter(|k| **k).count());
        let mut carry = 0u32;
        for (i, frame) in std::mem::take(&mut self.frames).into_iter().enumerate() {
            if keep[i] {
                let mut frame = frame;
                frame.delay_cs = (frame.delay_cs as u32 + carry).min(u16::MAX as u32) as u16;
                carry = 0;
                frames.push(frame);
            } else if let Some(prev) = frames.last_mut() {
                // Predecessor absorbs the delay; without this the result plays fast.
                prev.delay_cs =
                    (prev.delay_cs as u32 + frame.delay_cs as u32).min(u16::MAX as u32) as u16;
            } else {
                // Nothing kept yet, so the delay rolls forward instead.
                carry += frame.delay_cs as u32;
            }
        }
        self.frames = frames;

        // new index of a frame = how many kept frames precede it
        let mut kept_before = Vec::with_capacity(n + 1);
        let mut count = 0;
        for k in &keep {
            kept_before.push(count);
            count += *k as usize;
        }
        kept_before.push(count);

        let at = |i: usize| kept_before[i.min(n)];
        self.overlays.retain_mut(|o| {
            o.range = at(o.range.start)..at(o.range.end);
            !o.range.is_empty()
        });
    }

    pub fn delete_frames(&mut self, range: Range<usize>) {
        self.retain_frames(|i| !range.contains(&i));
    }

    /// Delete an arbitrary set of frames. Ctrl+click in the strip picks frames
    /// one at a time, so what comes back need not be a run.
    pub fn delete_frames_at(&mut self, frames: &[usize]) {
        let doomed: std::collections::HashSet<usize> = frames.iter().copied().collect();
        self.retain_frames(|i| !doomed.contains(&i));
    }

    /// Freeze-frame: the copy lands directly after the source and inherits the
    /// overlays that cover it.
    pub fn duplicate_frame(&mut self, index: usize) {
        let Some(frame) = self.frames.get(index).cloned() else {
            return;
        };
        self.frames.insert(index + 1, frame);
        let pos = index + 1;
        for o in &mut self.overlays {
            let shift = |i: usize| if i >= pos { i + 1 } else { i };
            o.range = shift(o.range.start)..shift(o.range.end);
        }
    }

    pub fn reverse_frames(&mut self, range: Range<usize>) {
        if range.end <= self.frames.len() {
            self.frames[range].reverse();
        }
    }

    pub fn set_delay(&mut self, range: Range<usize>, delay_cs: u16) {
        for f in self.frames.get_mut(range).into_iter().flatten() {
            f.delay_cs = delay_cs;
        }
    }

    /// Delete every Nth frame, preserving total duration. Frame 0 always stays.
    pub fn drop_every_nth(&mut self, n: usize) {
        if n < 2 {
            return;
        }
        self.retain_frames(|i| (i + 1) % n != 0);
    }

    /// Replace one frame's pixels, as the external editor handoff does. The
    /// frame is detached from then on: overlays covering it skip it.
    pub fn replace_frame_pixels(&mut self, index: usize, pixels: RgbaImage) {
        if let Some(f) = self.frames.get_mut(index) {
            // Rebuild rather than assign: the thumbnail and the opacity flag
            // are cached off these pixels.
            *f = Frame::new(pixels, f.delay_cs);
            f.detached = true;
        }
    }

    /// Motion per frame: how much it differs from the one before it, measured
    /// on the cached thumbnails rather than the full frames. A 72px-wide
    /// thumbnail is enough to tell a static section from a moving one, and it
    /// is three orders of magnitude less work than the real pixels.
    pub fn motion_scores(&self) -> Vec<u32> {
        let mut scores = Vec::with_capacity(self.frames.len());
        for (i, frame) in self.frames.iter().enumerate() {
            let Some(prev) = i.checked_sub(1).and_then(|j| self.frames.get(j)) else {
                // Nothing to compare the first frame against, and it is never
                // a candidate for removal anyway.
                scores.push(u32::MAX);
                continue;
            };
            if prev.thumb.dimensions() != frame.thumb.dimensions() {
                scores.push(u32::MAX);
                continue;
            }
            let diff: u64 = prev
                .thumb
                .as_raw()
                .chunks_exact(4)
                .zip(frame.thumb.as_raw().chunks_exact(4))
                .map(|(a, b)| (0..3).map(|c| a[c].abs_diff(b[c]) as u64).sum::<u64>())
                .sum();
            let pixels = (frame.thumb.width() * frame.thumb.height()).max(1) as u64;
            scores.push((diff * 100 / (pixels * 3)).min(u32::MAX as u64) as u32);
        }
        scores
    }

    /// Drop the `count` frames that move the least, which is what "smart" means
    /// here: a pause loses frames before a pan does. Duration is preserved, so
    /// the result plays at the same speed with fewer frames in it.
    pub fn drop_low_motion(&mut self, count: usize) {
        if count == 0 || self.frames.len() <= 1 {
            return;
        }
        let scores = self.motion_scores();
        let mut order: Vec<usize> = (1..self.frames.len()).collect();
        // Ties break towards the later frame so a long static run thins from
        // the end rather than leaving a gap at its start.
        order.sort_by_key(|i| (scores[*i], std::cmp::Reverse(*i)));
        let doomed: std::collections::HashSet<usize> = order
            .into_iter()
            .take(count.min(self.frames.len() - 1))
            .collect();
        self.retain_frames(|i| !doomed.contains(&i));
    }

    /// Crop every frame to `rect`, clamped to the canvas. Document-wide by
    /// design: a GIF has one canvas, so frames of different sizes are not a
    /// thing this model can hold. Overlays keep their place on the image by
    /// moving with the origin.
    pub fn crop(&mut self, x: u32, y: u32, w: u32, h: u32) {
        let (cw, ch) = self.size();
        let x = x.min(cw.saturating_sub(1));
        let y = y.min(ch.saturating_sub(1));
        let w = w.min(cw - x).max(1);
        let h = h.min(ch - y).max(1);
        if (x, y, w, h) == (0, 0, cw, ch) {
            return;
        }
        for frame in &mut self.frames {
            let pixels = crop_with_padding(frame.pixels.as_ref(), x, y, w, h);
            let detached = frame.detached;
            *frame = Frame::new(pixels, frame.delay_cs);
            frame.detached = detached;
        }
        for o in &mut self.overlays {
            o.transform.x -= x as f32;
            o.transform.y -= y as f32;
        }
    }

    /// Scale the whole document. Overlays scale with it, so a caption keeps its
    /// proportions rather than sliding off a smaller canvas.
    pub fn resize(&mut self, w: u32, h: u32) {
        let (cw, ch) = self.size();
        let (w, h) = (w.max(1), h.max(1));
        if cw == 0 || ch == 0 || (w, h) == (cw, ch) {
            return;
        }
        let (fx, fy) = (w as f32 / cw as f32, h as f32 / ch as f32);
        for (i, frame) in self.resized_frames(w, h, |_, _| {}) {
            self.frames[i] = frame;
        }
        self.scale_overlays(fx, fy);
    }

    /// The frames a whole-document resize would produce, as `(index, frame)`
    /// pairs so a caller can apply them by position. This is the slow half of
    /// `resize`, the half a background worker runs; the mutator stays on the
    /// main thread because swapping indexed frames is cheap. `progress`
    /// reports `(done, total)` once per finished frame.
    pub fn resized_frames(
        &self,
        w: u32,
        h: u32,
        mut progress: impl FnMut(usize, usize),
    ) -> Vec<(usize, Frame)> {
        let (w, h) = (w.max(1), h.max(1));
        let total = self.frames.len();
        self.frames
            .iter()
            .enumerate()
            .map(|(i, frame)| {
                let pixels = image::imageops::resize(
                    frame.pixels.as_ref(),
                    w,
                    h,
                    image::imageops::FilterType::Triangle,
                );
                let mut produced = Frame::new(pixels, frame.delay_cs);
                produced.detached = frame.detached;
                progress(i + 1, total);
                (i, produced)
            })
            .collect()
    }

    /// Scale every overlay's box by `(fx, fy)`, a text overlay's point size
    /// following the height. `resize` passes the real factors; a zoom keeps
    /// the canvas, so it passes `(1.0, 1.0)` and nothing moves.
    pub fn scale_overlays(&mut self, fx: f32, fy: f32) {
        for o in &mut self.overlays {
            o.transform.x *= fx;
            o.transform.y *= fy;
            o.transform.w *= fx;
            o.transform.h *= fy;
            if let super::model::OverlayKind::Text(t) = &mut o.kind {
                t.size_px *= fy;
            }
        }
    }

    /// Blow `rect` up to fill the canvas, on the frames in `range` only. The
    /// canvas size never changes, which is what makes this safe per frame in a
    /// way `crop` is not.
    pub fn zoom_frames(&mut self, frames: &[usize], x: u32, y: u32, w: u32, h: u32) {
        for (i, frame) in self.zoomed_frames(frames, x, y, w, h, |_, _| {}) {
            self.frames[i] = frame;
        }
    }

    /// The frames a zoom would produce, as `(index, frame)` pairs, the way
    /// `resized_frames` does for a resize. Indices past the end of the frame
    /// list are skipped, and an empty document makes an empty answer — the
    /// canvas size must not reach the `cw - 1` below. `progress` reports
    /// `(done, total)` over the frames that will actually be produced.
    pub fn zoomed_frames(
        &self,
        indices: &[usize],
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        mut progress: impl FnMut(usize, usize),
    ) -> Vec<(usize, Frame)> {
        let (cw, ch) = self.size();
        if cw == 0 || ch == 0 {
            return Vec::new();
        }
        let (x, y) = (x.min(cw - 1), y.min(ch - 1));
        let (w, h) = (w.min(cw - x).max(1), h.min(ch - y).max(1));
        let wanted: Vec<usize> = indices
            .iter()
            .copied()
            .filter(|i| *i < self.frames.len())
            .collect();
        let total = wanted.len();
        wanted
            .into_iter()
            .enumerate()
            .map(|(done, i)| {
                let frame = &self.frames[i];
                let cropped = crop_with_padding(frame.pixels.as_ref(), x, y, w, h);
                let pixels = image::imageops::resize(
                    &cropped,
                    cw,
                    ch,
                    image::imageops::FilterType::Triangle,
                );
                let mut produced = Frame::new(pixels, frame.delay_cs);
                produced.detached = frame.detached;
                progress(done + 1, total);
                (i, produced)
            })
            .collect()
    }

    /// Rescale every delay, the export dialog's speed setting.
    pub fn scale_delays(&mut self, factor: f32) {
        for f in &mut self.frames {
            f.delay_cs =
                ((f.delay_cs as f32 / factor).round() as u32).clamp(1, u16::MAX as u32) as u16;
        }
    }
}

/// Crop against the document canvas even if a frame imported from an external
/// editor is smaller. Pixels outside that frame are transparent; every result
/// still has the document crop's dimensions.
fn crop_with_padding(image: &RgbaImage, x: u32, y: u32, w: u32, h: u32) -> RgbaImage {
    let mut output = RgbaImage::new(w, h);
    let copy_w = w.min(image.width().saturating_sub(x));
    let copy_h = h.min(image.height().saturating_sub(y));
    if copy_w == 0 || copy_h == 0 {
        return output;
    }
    let cropped = image::imageops::crop_imm(image, x, y, copy_w, copy_h).to_image();
    image::imageops::replace(&mut output, &cropped, 0, 0);
    output
}

/// Per-frame delays for `count` frames at `fps`, distributing the centisecond
/// remainder so the total matches the source instead of drifting.
///
/// 30fps is 3.33cs, which GIF cannot store; rounding every frame to 3cs gives a
/// 33.3fps animation that runs ahead of the recording.
pub fn delays_for_fps(fps: f64, count: usize) -> Vec<u16> {
    let mut delays = Vec::with_capacity(count);
    let mut elapsed = 0.0f64;
    let mut written = 0u32;
    for i in 0..count {
        elapsed = (i + 1) as f64 * 100.0 / fps;
        let want = elapsed.round() as u32;
        delays.push((want.saturating_sub(written)).max(1) as u16);
        written = written.max(want);
    }
    let _ = elapsed;
    delays
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{Frame, OverlayKind, Shape, ShapeOverlay, Transform};

    fn doc(n: usize, delay: u16) -> Document {
        Document::from_frames(
            (0..n)
                .map(|_| Frame::new(RgbaImage::new(2, 2), delay))
                .collect(),
        )
    }

    fn shape() -> OverlayKind {
        OverlayKind::Shape(ShapeOverlay {
            shape: Shape::Rect,
            fill: Some([1, 2, 3, 255]),
            stroke: None,
        })
    }

    #[test]
    fn drop_every_nth_preserves_duration() {
        let mut d = doc(10, 5);
        let before = d.duration_cs();
        d.drop_every_nth(2);
        assert_eq!(d.frames.len(), 5);
        assert_eq!(
            d.duration_cs(),
            before,
            "dropping frames must not speed it up"
        );
        assert!(d.frames.iter().all(|f| f.delay_cs == 10));
    }

    #[test]
    fn drop_every_nth_keeps_first_frame() {
        let mut d = doc(9, 4);
        d.drop_every_nth(3);
        assert_eq!(d.frames.len(), 6);
        assert_eq!(d.duration_cs(), 36);
    }

    #[test]
    fn delete_remaps_overlay_ranges() {
        let mut d = doc(10, 10);
        let spanning = d.add_overlay("a", shape(), Transform::at(0., 0., 1., 1.), 2..8);
        let doomed = d.add_overlay("b", shape(), Transform::at(0., 0., 1., 1.), 3..5);
        let later = d.add_overlay("c", shape(), Transform::at(0., 0., 1., 1.), 8..10);

        d.delete_frames(3..5);

        assert_eq!(d.frames.len(), 8);
        assert_eq!(d.overlay(spanning).unwrap().range, 2..6);
        assert!(
            d.overlay(doomed).is_none(),
            "an overlay with no frames left is gone"
        );
        assert_eq!(d.overlay(later).unwrap().range, 6..8);
    }

    /// Regression: `replace_frame_pixels` used to assign `pixels` in place,
    /// leaving the cached thumbnail and opacity flag describing the old image.
    #[test]
    fn replacing_pixels_refreshes_the_cached_thumbnail() {
        let mut doc = Document::from_frames(vec![Frame::new(
            RgbaImage::from_pixel(80, 80, image::Rgba([255, 0, 0, 255])),
            9,
        )]);
        let before = doc.frames[0].clone();

        doc.replace_frame_pixels(
            0,
            RgbaImage::from_pixel(80, 20, image::Rgba([0, 0, 255, 128])),
        );
        let after = &doc.frames[0];

        assert_eq!(after.delay_cs, 9, "the delay survives the swap");
        assert!(after.detached);
        assert!(!after.opaque, "the new pixels have alpha");
        assert_ne!(
            after.thumb.height(),
            before.thumb.height(),
            "thumbnail redrawn"
        );
        assert_ne!(
            after.key, before.key,
            "a different image is a different frame"
        );
    }

    /// Regression: the filmstrip only rebuilds when these keys change, so an
    /// operation that leaves a frame alone has to leave its key alone too.
    #[test]
    fn frame_keys_track_identity_not_position() {
        let mut doc = Document::from_frames(
            (0..3)
                .map(|i| Frame::new(RgbaImage::from_pixel(4, 4, image::Rgba([i, 0, 0, 255])), 5))
                .collect(),
        );
        let keys: Vec<u64> = doc.frames.iter().map(|f| f.key).collect();
        assert_eq!(keys.len(), 3);
        assert!(
            keys[0] != keys[1] && keys[1] != keys[2],
            "distinct frames, distinct keys"
        );

        doc.duplicate_frame(1);
        assert_eq!(
            doc.frames[1].key, doc.frames[2].key,
            "a copy is the same image"
        );

        doc.reverse_frames(0..4);
        let mut reversed: Vec<u64> = doc.frames.iter().map(|f| f.key).collect();
        reversed.sort_unstable();
        let mut expected = vec![keys[0], keys[1], keys[1], keys[2]];
        expected.sort_unstable();
        assert_eq!(
            reversed, expected,
            "reversing moves frames, it does not remake them"
        );
    }

    /// Ctrl+click selects frames that need not touch, so deletion has to take
    /// a set: doing it as a run would take the untouched frames between them.
    #[test]
    fn deleting_a_gappy_selection_leaves_the_frames_between_alone() {
        let mut d = Document::from_frames(
            (0..5)
                .map(|i| Frame::new(RgbaImage::from_pixel(2, 2, image::Rgba([i, 0, 0, 255])), 10))
                .collect(),
        );
        let before = d.duration_cs();

        d.delete_frames_at(&[1, 3]);

        let reds: Vec<u8> = d
            .frames
            .iter()
            .map(|f| f.pixels.get_pixel(0, 0).0[0])
            .collect();
        assert_eq!(reds, vec![0, 2, 4]);
        assert_eq!(d.duration_cs(), before, "removal must not speed the gif up");
    }

    #[test]
    fn delete_preserves_duration() {
        let mut d = doc(4, 10);
        d.delete_frames(1..3);
        assert_eq!(d.duration_cs(), 40);
        assert_eq!(d.frames[0].delay_cs, 30);
    }

    #[test]
    fn deleting_the_first_frame_rolls_delay_forward() {
        let mut d = doc(3, 10);
        d.delete_frames(0..1);
        assert_eq!(d.duration_cs(), 30);
        assert_eq!(d.frames[0].delay_cs, 20);
    }

    #[test]
    fn duplicate_extends_covering_overlays() {
        let mut d = doc(4, 10);
        let covering = d.add_overlay("a", shape(), Transform::at(0., 0., 1., 1.), 1..3);
        let after = d.add_overlay("b", shape(), Transform::at(0., 0., 1., 1.), 3..4);
        d.duplicate_frame(1);
        assert_eq!(d.frames.len(), 5);
        assert_eq!(d.overlay(covering).unwrap().range, 1..4);
        assert_eq!(d.overlay(after).unwrap().range, 4..5);
    }

    fn moving_doc() -> Document {
        // frames 0..3 identical, then a change, then identical again
        let still = || RgbaImage::from_pixel(40, 40, image::Rgba([10, 10, 10, 255]));
        let moved = || RgbaImage::from_pixel(40, 40, image::Rgba([240, 10, 10, 255]));
        Document::from_frames(vec![
            Frame::new(still(), 5),
            Frame::new(still(), 5),
            Frame::new(still(), 5),
            Frame::new(moved(), 5),
            Frame::new(moved(), 5),
        ])
    }

    /// The point of "smart" removal: the frames that repeat go first, the ones
    /// carrying the change stay.
    #[test]
    fn smart_removal_drops_the_still_frames_and_keeps_the_moving_one() {
        let mut d = moving_doc();
        let before = d.duration_cs();
        let scores = d.motion_scores();
        assert!(
            scores[3] > scores[2],
            "the change is the busiest frame: {scores:?}"
        );
        assert_eq!(scores[1], 0, "a repeat has no motion");

        d.drop_low_motion(2);
        assert_eq!(d.frames.len(), 3);
        assert_eq!(d.duration_cs(), before, "removal must not speed the gif up");
        // One of each still run survives, and so does the frame that changed.
        let reds: Vec<u8> = d
            .frames
            .iter()
            .map(|f| f.pixels.get_pixel(0, 0).0[0])
            .collect();
        assert!(reds.contains(&10) && reds.contains(&240), "{reds:?}");
    }

    #[test]
    fn smart_removal_never_takes_the_first_frame_or_more_than_it_has() {
        let mut d = moving_doc();
        d.drop_low_motion(99);
        assert_eq!(d.frames.len(), 1, "something always survives");

        let mut single = doc(1, 5);
        single.drop_low_motion(1);
        assert_eq!(single.frames.len(), 1);
    }

    /// Crop is document-wide and takes the overlays with it, so an annotation
    /// stays on the pixel it was pointing at.
    #[test]
    fn crop_moves_every_frame_and_the_overlays_with_it() {
        let mut d = Document::from_frames(
            (0..3)
                .map(|_| Frame::new(RgbaImage::new(100, 80), 5))
                .collect(),
        );
        let id = d.add_overlay("a", shape(), Transform::at(30.0, 25.0, 10.0, 10.0), 0..3);

        d.crop(20, 15, 50, 40);

        assert_eq!(d.size(), (50, 40));
        assert!(d.frames.iter().all(|f| f.pixels.dimensions() == (50, 40)));
        let t = d.overlay(id).unwrap().transform;
        assert_eq!((t.x, t.y), (10.0, 10.0), "same pixel, new origin");
        assert_eq!(
            d.frames[0].thumb.width(),
            crate::core::model::THUMB_W,
            "thumbnail redrawn"
        );
    }

    #[test]
    fn crop_clamps_to_the_canvas_rather_than_panicking() {
        let mut d = Document::from_frames(vec![Frame::new(RgbaImage::new(20, 20), 5)]);
        d.crop(15, 15, 400, 400);
        assert_eq!(d.size(), (5, 5));

        let mut d = Document::from_frames(vec![Frame::new(RgbaImage::new(20, 20), 5)]);
        d.crop(0, 0, 20, 20);
        assert_eq!(d.size(), (20, 20), "a no-op crop leaves the frames alone");
    }

    #[test]
    fn crop_and_zoom_pad_frames_smaller_than_the_document_canvas() {
        let mut cropped = Document::from_frames(vec![
            Frame::new(
                RgbaImage::from_pixel(20, 20, image::Rgba([1, 2, 3, 255])),
                5,
            ),
            Frame::new(RgbaImage::from_pixel(8, 8, image::Rgba([4, 5, 6, 255])), 5),
        ]);
        let mut zoomed = cropped.clone();

        cropped.crop(5, 5, 10, 10);
        assert!(
            cropped
                .frames
                .iter()
                .all(|frame| frame.pixels.dimensions() == (10, 10))
        );
        assert_eq!(cropped.frames[1].pixels.get_pixel(9, 9).0[3], 0);

        zoomed.zoom_frames(&[1], 5, 5, 10, 10);
        assert_eq!(zoomed.frames[1].pixels.dimensions(), (20, 20));
        assert_eq!(zoomed.frames[1].pixels.get_pixel(19, 19).0[3], 0);
    }

    #[test]
    fn resize_scales_the_overlays_too() {
        let mut d = Document::from_frames(vec![Frame::new(RgbaImage::new(100, 100), 5)]);
        let id = d.add_overlay("a", shape(), Transform::at(10.0, 20.0, 40.0, 40.0), 0..1);
        d.resize(50, 50);
        assert_eq!(d.size(), (50, 50));
        let t = d.overlay(id).unwrap().transform;
        assert_eq!((t.x, t.y, t.w, t.h), (5.0, 10.0, 20.0, 20.0));
    }

    /// Zoom is the per-frame answer to crop: the canvas keeps its size, so
    /// frames outside the range still line up with the ones inside it.
    #[test]
    fn zoom_keeps_the_canvas_size_and_only_touches_its_range() {
        let mut d = Document::from_frames(
            (0..4)
                .map(|i| {
                    let mut img = RgbaImage::new(40, 40);
                    img.put_pixel(4, 4, image::Rgba([i as u8 * 60, 0, 0, 255]));
                    Frame::new(img, 5)
                })
                .collect(),
        );
        let untouched = d.frames[3].key;

        d.zoom_frames(&[1, 2], 0, 0, 20, 20);

        assert!(
            d.frames.iter().all(|f| f.pixels.dimensions() == (40, 40)),
            "canvas is uniform"
        );
        assert_eq!(
            d.frames[3].key, untouched,
            "outside the range nothing is rebuilt"
        );
        assert_ne!(d.frames[1].key, untouched);
    }

    #[test]
    fn fps_delays_do_not_drift() {
        let d = delays_for_fps(30.0, 30);
        let total: u32 = d.iter().map(|x| *x as u32).sum();
        assert_eq!(total, 100, "one second of 30fps must stay one second");
        assert!(d.iter().all(|x| (3..=4).contains(x)));

        assert!(delays_for_fps(25.0, 25).iter().all(|x| *x == 4));
        assert!(
            delays_for_fps(200.0, 10).iter().all(|x| *x == 1),
            "delays never hit zero"
        );
    }

    /// The producer the async resize runs must agree with the mutator the sync
    /// path uses, down to what a wholesale frame swap would otherwise lose:
    /// the delay and the detached flag ride along.
    #[test]
    fn resized_frames_match_resize_and_carry_delay_and_detached() {
        let mut d = Document::from_frames(
            (0..3)
                .map(|i| {
                    let mut img = RgbaImage::new(40, 40);
                    img.put_pixel(4, 4, image::Rgba([i as u8 * 60, 0, 0, 255]));
                    let mut frame = Frame::new(img, 3 + i as u16);
                    frame.detached = i == 1;
                    frame
                })
                .collect(),
        );
        let id = d.add_overlay("a", shape(), Transform::at(10.0, 20.0, 40.0, 40.0), 0..3);
        let mut other = d.clone();

        d.resize(20, 20);

        let mut seen = Vec::new();
        let produced = other.resized_frames(20, 20, |done, total| seen.push((done, total)));
        for (i, frame) in produced {
            other.frames[i] = frame;
        }
        other.scale_overlays(0.5, 0.5);

        assert_eq!(other, d, "the producer must match what resize() does");
        assert_eq!(
            d.frames.iter().map(|f| f.delay_cs).collect::<Vec<_>>(),
            vec![3, 4, 5],
            "delays survive the resample"
        );
        assert!(d.frames[1].detached, "detached survives the resample");
        let t = d.overlay(id).unwrap().transform;
        assert_eq!((t.x, t.y, t.w, t.h), (5.0, 10.0, 20.0, 20.0));
        assert_eq!(
            seen,
            vec![(1, 3), (2, 3), (3, 3)],
            "progress is reported once per frame"
        );
    }

    #[test]
    fn resized_frames_clamp_zero_dimensions_like_resize() {
        let mut resized = doc(1, 5);
        let mut produced = resized.clone();

        resized.resize(0, 0);
        for (i, frame) in produced.resized_frames(0, 0, |_, _| {}) {
            produced.frames[i] = frame;
        }

        assert_eq!(produced, resized);
        assert_eq!(produced.size(), (1, 1));
    }

    /// The producer the async zoom runs must agree with the mutator: delay and
    /// detached ride along, progress counts only the frames it will produce,
    /// and indices past the end are skipped rather than misaligning the pairs.
    #[test]
    fn zoomed_frames_match_zoom_frames_and_skip_out_of_range() {
        let mut d = Document::from_frames(
            (0..4)
                .map(|i| {
                    let mut img = RgbaImage::new(40, 40);
                    img.put_pixel(4, 4, image::Rgba([i as u8 * 60, 0, 0, 255]));
                    let mut frame = Frame::new(img, 3 + i as u16);
                    frame.detached = i == 2;
                    frame
                })
                .collect(),
        );
        let mut other = d.clone();

        d.zoom_frames(&[1, 2], 0, 0, 20, 20);

        let mut seen = Vec::new();
        // Index 9 is past the end: it must not appear as a pair.
        let produced = other.zoomed_frames(&[1, 9, 2], 0, 0, 20, 20, |done, total| {
            seen.push((done, total))
        });
        for (i, frame) in produced {
            other.frames[i] = frame;
        }

        assert_eq!(other, d, "the producer must match what zoom_frames() does");
        assert_eq!(
            seen,
            vec![(1, 2), (2, 2)],
            "progress counts only the frames actually produced"
        );
        assert_eq!(
            d.frames.iter().map(|f| f.delay_cs).collect::<Vec<_>>(),
            vec![3, 4, 5, 6],
            "delays survive the refill"
        );
        assert!(d.frames[2].detached, "detached survives the refill");
        // Keys are per-build identity, so independently rebuilt frames never
        // share one — but frames outside the zoom must keep theirs.
        assert_eq!(d.frames[0].key, other.frames[0].key);
        assert_eq!(d.frames[3].key, other.frames[3].key);

        // An empty document makes an empty answer, not a panic.
        let empty = Document::default();
        let produced = empty.zoomed_frames(&[0], 0, 0, 10, 10, |_, _| {});
        assert!(produced.is_empty());
    }
}
