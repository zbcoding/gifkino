//! Frame-list math. Pure list logic, which is why it is tested here and not
//! through the UI.

use std::ops::Range;

use image::RgbaImage;

use super::model::{Document, Frame, Overlay, OverlayId, Rgba8, runs, sorted_existing};

/// Where a removed frame's delay goes. A frame dropped from between two
/// survivors leaves the one before it on screen in its place, so that one
/// absorbs its time and the running time holds. A run trimmed off either end
/// has no survivor showing in its place — moving its time onto one would
/// turn trimming a GIF's opening second into a one-second freeze — so a
/// delete lets it go. Decimation keeps every dropped frame's time.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DroppedDelay {
    Keep,
    TrimEnds,
}

/// A mirror's axis: `Horizontal` swaps left and right, `Vertical` top and
/// bottom.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub enum Axis {
    Horizontal,
    Vertical,
}

/// Which end of a run of frames is the solid colour: a fade in starts on it,
/// a fade out ends on it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub enum Fade {
    In,
    Out,
}

/// How much two frames differ, measured on the cached thumbnails rather than
/// the full frames: a 72px-wide thumbnail is enough to tell a still section
/// from a moving one, at three orders of magnitude less work. Mean channel
/// difference in hundredths of a level; thumbnails that do not line up count
/// as maximally different.
fn thumb_diff(a: &Frame, b: &Frame) -> u64 {
    if a.thumb.dimensions() != b.thumb.dimensions() {
        return u32::MAX as u64;
    }
    let diff: u64 = a
        .thumb
        .as_raw()
        .chunks_exact(4)
        .zip(b.thumb.as_raw().chunks_exact(4))
        .map(|(a, b)| (0..3).map(|c| a[c].abs_diff(b[c]) as u64).sum::<u64>())
        .sum();
    let pixels = (a.thumb.width() * a.thumb.height()).max(1) as u64;
    diff * 100 / (pixels * 3)
}

impl Document {
    /// Delete the frames `keep` returns false for, remapping overlay ranges
    /// onto the new indices. `delay` says what happens to their time; under
    /// `Keep` a run dropped off the start rolls forward onto the first
    /// survivor, the only one there is to take it.
    fn retain_frames(&mut self, keep: impl Fn(usize) -> bool, delay: DroppedDelay) {
        let n = self.frames.len();
        let keep: Vec<bool> = (0..n).map(&keep).collect();
        if keep.iter().all(|k| *k) {
            return;
        }

        let mut frames = Vec::with_capacity(keep.iter().filter(|k| **k).count());
        let mut carry = 0u32;
        let last_kept = keep.iter().rposition(|k| *k);
        for (i, frame) in std::mem::take(&mut self.frames).into_iter().enumerate() {
            if keep[i] {
                let mut frame = frame;
                frame.delay_cs = add_delay(frame.delay_cs, carry);
                carry = 0;
                frames.push(frame);
            } else if delay == DroppedDelay::TrimEnds
                && (frames.is_empty() || last_kept.is_none_or(|last| i > last))
            {
                continue;
            } else if let Some(prev) = frames.last_mut() {
                // Predecessor absorbs the delay; without this the result plays fast.
                prev.delay_cs = add_delay(prev.delay_cs, frame.delay_cs as u32);
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
        self.retain_frames(|i| !range.contains(&i), DroppedDelay::TrimEnds);
    }

    /// Delete an arbitrary set of frames. Ctrl+click in the strip picks frames
    /// one at a time, so what comes back need not be a run.
    pub fn delete_frames_at(&mut self, frames: &[usize]) {
        let doomed = self.mask(frames);
        self.retain_frames(|i| !doomed[i], DroppedDelay::TrimEnds);
    }

    /// Freeze-frame every frame in `picked` at once: each copy lands directly
    /// after its source and inherits the overlays that cover it. One pass over
    /// the frames and one over the overlays, where duplicating them one at a
    /// time shifts the whole list once per copy. Stale indices are skipped.
    pub fn duplicate_frames_at(&mut self, picked: &[usize]) {
        let n = self.frames.len();
        let twinned = self.mask(picked);
        // `copies_before[b]`: how many copies land ahead of boundary `b`. A
        // copy of frame `j` sits at `j + 1`, so it pushes every boundary past
        // `j` along — an overlay ending just after `j` grows over the copy.
        let mut copies_before = Vec::with_capacity(n + 1);
        let mut frames = Vec::with_capacity(n + picked.len());
        for (i, frame) in std::mem::take(&mut self.frames).into_iter().enumerate() {
            copies_before.push(frames.len() - i);
            if twinned[i] {
                frames.push(frame.clone());
            }
            frames.push(frame);
        }
        copies_before.push(frames.len() - n);
        self.frames = frames;
        let at = |b: usize| b + copies_before[b.min(n)];
        for o in &mut self.overlays {
            o.range = at(o.range.start)..at(o.range.end);
        }
    }

    /// Splice a run of frames in at `at`, keeping their order. Paste: the
    /// clipboard's frames land together, and — like a duplicate — they
    /// inherit the overlays that cover the slot they go into.
    pub fn insert_frames_at(&mut self, at: usize, frames: Vec<Frame>) {
        let at = at.min(self.frames.len());
        let added = frames.len();
        self.frames.splice(at..at, frames);
        self.shift_overlay_ranges(at, added, true);
    }

    /// Splice frames from another file in at `at` without letting the
    /// overlays around the seam grow over them: an imported clip arrives with
    /// no captions on it, and inheriting the one that happened to end where it
    /// landed would paint somebody else's text on it. `insert_frames_at` is
    /// the other rule, for a duplicate or a paste of the document's own
    /// frames. An overlay the run lands *inside* still spans it, the same way
    /// `move_frame` leaves a frame dropped into the middle of a band covered.
    pub fn insert_foreign_frames_at(&mut self, at: usize, frames: Vec<Frame>) {
        let at = at.min(self.frames.len());
        let added = frames.len();
        if added == 0 {
            return;
        }
        self.frames.splice(at..at, frames);
        self.shift_overlay_ranges(at, added, false);
    }

    /// Make room in every overlay range for `by` frames arriving at `at`. A
    /// range starting at or after `at` moves along with the frames after it;
    /// one that merely ends at `at` grows over the arrivals only when
    /// `grow_touching`, which is the whole difference between a paste and an
    /// import or a moved frame.
    fn shift_overlay_ranges(&mut self, at: usize, by: usize, grow_touching: bool) {
        for o in &mut self.overlays {
            if o.range.start >= at {
                o.range.start += by;
            }
            if o.range.end > at || (grow_touching && o.range.end == at) {
                o.range.end += by;
            }
        }
    }

    /// One flag per frame, set for each index in `indices`. Indices past the
    /// end are skipped, so a stale selection never indexes out of bounds.
    fn mask(&self, indices: &[usize]) -> Vec<bool> {
        let mut mask = vec![false; self.frames.len()];
        for &i in indices {
            if let Some(flag) = mask.get_mut(i) {
                *flag = true;
            }
        }
        mask
    }

    /// Move `id` directly above or below `other` in the z-order, leaving
    /// every other overlay's relative order alone. Restacking is a step next
    /// to a *named* neighbour rather than a swap with the adjacent list
    /// entry, because the layer list shows only the overlays on the frame on
    /// screen: the entry beside `id` in `self.overlays` may not be one of
    /// them, and swapping with it would move nothing the user can see.
    pub fn restack_overlay(&mut self, id: OverlayId, other: OverlayId, above: bool) {
        if id == other {
            return;
        }
        let Some(from) = self.overlays.iter().position(|o| o.id == id) else {
            return;
        };
        let overlay = self.overlays.remove(from);
        let Some(target) = self.overlays.iter().position(|o| o.id == other) else {
            self.overlays.insert(from, overlay);
            return;
        };
        let at = if above { target + 1 } else { target };
        self.overlays.insert(at, overlay);
    }

    /// Reorder: pull the frame at `from` out and reinsert it at `to`. Whatever
    /// overlay(s) covered that frame come along with it: if the whole overlay
    /// moved (nothing was left behind), it just relocates; if only part of a
    /// wider overlay covered `from`, that part splits off into a fresh
    /// overlay parked at the new slot while the rest stays put and closes
    /// the gap, the same way `delete_frames_at` already shifts survivors.
    /// `coalesce_overlays` below re-merges adjacent identical pieces
    /// afterwards, so a whole selection sharing one overlay comes back out
    /// as the one overlay it started as once a multi-frame drag finishes.
    pub fn move_frame(&mut self, from: usize, to: usize) {
        let n = self.frames.len();
        if from >= n || to >= n || from == to {
            return;
        }
        let frame = self.frames.remove(from);

        // Remember which overlays covered the frame being pulled out, before
        // their ranges move under the shift below.
        let carried: Vec<usize> = self
            .overlays
            .iter()
            .enumerate()
            .filter(|(_, o)| o.range.contains(&from))
            .map(|(i, _)| i)
            .collect();

        let shift_out = |i: usize| if i > from { i - 1 } else { i };
        for o in &mut self.overlays {
            o.range = shift_out(o.range.start)..shift_out(o.range.end);
        }

        self.frames.insert(to, frame);

        // Reinsertion here moves an *existing* frame back in, not a fresh
        // duplicate, so — unlike a paste — a range that merely touches `to`
        // must not grow to swallow it; only the carried overlays below get
        // to cover the reinserted frame's new slot.
        self.shift_overlay_ranges(to, 1, false);

        for idx in carried {
            if self.overlays[idx].range.is_empty() {
                // Nothing of it was left behind: the whole overlay relocates.
                self.overlays[idx].range = to..to + 1;
            } else {
                let piece = self.piece_of(idx, to..to + 1);
                self.overlays.push(piece);
            }
        }

        self.coalesce_overlays();
    }

    /// Two overlays paint identically and so are safe to fold into one: same
    /// content, same box, same everything but the id and where they sit.
    /// Field by field rather than through a clone, since a multi-frame drag
    /// asks this for every overlay pair on every hop; the destructure keeps
    /// a field added to `Overlay` from slipping past it.
    fn overlays_mergeable(a: &Overlay, b: &Overlay) -> bool {
        let Overlay {
            id: _,
            range: _,
            name,
            kind,
            transform,
            opacity,
            hidden,
        } = a;
        *name == b.name
            && *kind == b.kind
            && *transform == b.transform
            && *opacity == b.opacity
            && *hidden == b.hidden
    }

    /// Fold adjacent overlays back into one where a move split them and they
    /// landed touching and otherwise unchanged. Runs after every
    /// `move_frame`, so a multi-frame drag's per-hop splits in
    /// `move_frames_to` reunite as the frames come back together. Keeps
    /// whichever id is lower — the original overlay, never one of the
    /// clones a split spawns — so a selection open on it survives the drag.
    fn coalesce_overlays(&mut self) {
        let mut i = 0;
        'outer: while i < self.overlays.len() {
            for j in 0..self.overlays.len() {
                if i == j {
                    continue;
                }
                let (a, b) = (&self.overlays[i], &self.overlays[j]);
                let touching = a.range.end == b.range.start || b.range.end == a.range.start;
                if touching && Self::overlays_mergeable(a, b) {
                    let start = a.range.start.min(b.range.start);
                    let end = a.range.end.max(b.range.end);
                    let (keep, drop) = if a.id <= b.id { (i, j) } else { (j, i) };
                    self.overlays[keep].range = start..end;
                    self.overlays.remove(drop);
                    continue 'outer;
                }
            }
            i += 1;
        }
    }

    /// Put one overlay on `frames` as well as the ones it already covers,
    /// splitting into a piece per contiguous run the way `add_overlay_over`
    /// does — an overlay carries one contiguous range, so a gappy selection
    /// cannot be one overlay. Frames the overlay already covers are left
    /// alone rather than stacked with a second copy of itself.
    ///
    /// The pieces go in directly above the source instead of on top of the
    /// list, so each one stacks against the overlays on its frames exactly as
    /// the source does on its own. A piece landing against the source's range
    /// folds back into it, and `coalesce_overlays` keeps the lower id — the
    /// source — so a selection open on it survives the copy.
    ///
    /// Returns the frames the copy landed on.
    pub fn copy_overlay_to(&mut self, id: OverlayId, frames: &[usize]) -> usize {
        let Some(source) = self.overlays.iter().position(|o| o.id == id) else {
            return 0;
        };
        let covered = self.overlays[source].range.clone();
        let mut targets = sorted_existing(frames, self.frames.len());
        targets.retain(|f| !covered.contains(f));
        if targets.is_empty() {
            return 0;
        }
        let pieces: Vec<Overlay> = runs(&targets)
            .map(|run| self.piece_of(source, run))
            .collect();
        self.overlays.splice(source + 1..source + 1, pieces);
        self.coalesce_overlays();
        targets.len()
    }

    /// Reorder a whole set of frames at once: pull every index in `picked`
    /// out (their order among themselves is kept) and reinsert them as one
    /// contiguous run at `to`, an index into the list with `picked` already
    /// removed. Dragging a multi-frame selection needs this rather than a
    /// loop of `move_frame` calls with hand-shifted indices. Each hop is a
    /// plain `move_frame`, so overlays carry along and re-coalesce exactly
    /// the way a single-frame move already handles them.
    pub fn move_frames_to(&mut self, picked: &[usize], to: usize) {
        let picked = sorted_existing(picked, self.frames.len());
        if picked.is_empty() || to >= self.frames.len() {
            return;
        }
        // Find each picked frame again by where it started, not by key: a
        // duplicate or a pasted copy shares its source's key, and a key
        // lookup would grab whichever of the two sits first. `slots[i]` is
        // the original index of the frame now at `i`, moved hop for hop.
        let mut slots: Vec<usize> = (0..self.frames.len()).collect();
        for (cursor, &original) in (to..).zip(&picked) {
            let Some(at) = slots.iter().position(|&s| s == original) else {
                continue;
            };
            if cursor < slots.len() {
                let slot = slots.remove(at);
                slots.insert(cursor, slot);
            }
            self.move_frame(at, cursor);
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

    /// Set delay on an arbitrary set of frames. Ctrl+click in the strip picks
    /// frames one at a time, so a scoped edit need not be a run — same shape
    /// as `delete_frames_at`.
    pub fn set_delay_at(&mut self, frames: &[usize], delay_cs: u16) {
        for &i in frames {
            if let Some(f) = self.frames.get_mut(i) {
                f.delay_cs = delay_cs;
            }
        }
    }

    /// Delete every Nth frame, preserving total duration. Frame 0 always stays.
    pub fn drop_every_nth(&mut self, n: usize) {
        if n < 2 {
            return;
        }
        self.retain_frames(|i| (i + 1) % n != 0, DroppedDelay::Keep);
    }

    /// Drop the `count` frames whose loss shows the least, which is what
    /// "smart" means here: a pause loses frames before a pan does. Duration is
    /// preserved, so the result plays at the same speed with fewer frames.
    ///
    /// Removing a frame leaves the kept frame before it on screen in its
    /// place for as long as it was held — its own delay plus whatever it had
    /// already absorbed — so that is its cost: how far it differs from the
    /// kept predecessor, times that hold. The removal is greedy and both
    /// factors are updated after every drop. Scoring once against the
    /// original neighbours let a slow pan, where each step is small, lose a
    /// whole run in a row, piling the run's delay onto one frame: a long hold
    /// and then a jump. Identical frames still cost nothing, so a true pause
    /// still collapses into one frame without changing what plays.
    pub fn drop_low_motion(&mut self, count: usize) {
        let n = self.frames.len();
        let count = count.min(n.saturating_sub(1));
        if count == 0 {
            return;
        }
        // Doubly linked over the surviving frames; `n` marks the end.
        let mut prev: Vec<usize> = (0..n).map(|i| i.wrapping_sub(1)).collect();
        let mut next: Vec<usize> = (1..=n).collect();
        // `step[i]`: difference from the kept predecessor. `held[i]`: how long
        // frame i is on screen, in centiseconds, counting absorbed delays. A
        // zero delay still plays, so it counts as one.
        let mut step: Vec<u64> = (0..n)
            .map(|i| match i {
                0 => 0,
                _ => thumb_diff(&self.frames[i - 1], &self.frames[i]),
            })
            .collect();
        let mut held: Vec<u64> = self
            .frames
            .iter()
            .map(|f| f.delay_cs.max(1) as u64)
            .collect();
        let cost = |step: &[u64], held: &[u64], i: usize| step[i].saturating_mul(held[i]);
        let mut costs: Vec<u64> = (0..n).map(|i| cost(&step, &held, i)).collect();
        // Min-heap on cost; ties break towards the later frame so a still run
        // thins from its end. Entries go stale when a neighbour drops and are
        // skipped on the way out. Frame 0 is never a candidate.
        let mut heap: std::collections::BinaryHeap<_> = (1..n)
            .map(|i| std::cmp::Reverse((costs[i], std::cmp::Reverse(i))))
            .collect();
        let mut doomed = vec![false; n];
        let mut dropped = 0;
        while dropped < count {
            let Some(std::cmp::Reverse((c, std::cmp::Reverse(i)))) = heap.pop() else {
                break;
            };
            if doomed[i] || c != costs[i] {
                continue;
            }
            doomed[i] = true;
            dropped += 1;
            let (p, nx) = (prev[i], next[i]);
            // p now covers i's hold, the same way `retain_frames` merges the delay.
            next[p] = nx;
            held[p] += held[i];
            if nx < n {
                prev[nx] = p;
                step[nx] = thumb_diff(&self.frames[p], &self.frames[nx]);
            }
            for k in [p, nx] {
                if k != 0 && k < n {
                    costs[k] = cost(&step, &held, k);
                    heap.push(std::cmp::Reverse((costs[k], std::cmp::Reverse(k))));
                }
            }
        }
        self.retain_frames(|i| !doomed[i], DroppedDelay::Keep);
    }

    /// The frames a crop to `rect` would produce, as `(index, frame)` pairs,
    /// `rect` clamped to the canvas — the slow half a background worker
    /// runs, since every frame changes together and a few hundred of them is
    /// not instant. Document-wide by design: a GIF has one canvas, so frames
    /// of different sizes are not a thing this model can hold. Overlays keep
    /// their place on the image by moving with the origin, which `land` does
    /// once for the whole list when given minus the kept corner as `offset`.
    pub fn cropped_frames(
        &self,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        progress: impl FnMut(usize, usize),
    ) -> Vec<(usize, Frame)> {
        let Some((x, y, w, h)) = clamp_rect(self.size(), x, y, w, h) else {
            return Vec::new();
        };
        self.produce(0..self.frames.len(), progress, |_, frame| {
            crop_with_padding(frame.pixels.as_ref(), x, y, w, h)
        })
    }

    /// The frames a whole-document resize would produce, as `(index, frame)`
    /// pairs so a caller can apply them by position. This is the slow half a
    /// background worker runs; `land` swaps them in on the main thread and
    /// scales the overlays with them, so a caption keeps its proportions
    /// rather than sliding off a smaller canvas. `progress` reports `(done,
    /// total)` once per finished frame.
    pub fn resized_frames(
        &self,
        w: u32,
        h: u32,
        progress: impl FnMut(usize, usize),
    ) -> Vec<(usize, Frame)> {
        let (w, h) = (w.max(1), h.max(1));
        self.produce(0..self.frames.len(), progress, |_, frame| {
            image::imageops::resize(
                frame.pixels.as_ref(),
                w,
                h,
                image::imageops::FilterType::Triangle,
            )
        })
    }

    /// Land a frame job: each produced frame replaces the one at its index,
    /// then every overlay is scaled by `scale` and moved by `offset` so it
    /// stays over the pixels it was drawn on. A resize passes its factors, a
    /// crop minus the corner it kept, a splice onto a grown canvas both; work
    /// that keeps the canvas passes `(1.0, 1.0)` and `(0.0, 0.0)`.
    pub fn land(
        &mut self,
        frames: Vec<(usize, Frame)>,
        (fx, fy): (f32, f32),
        (dx, dy): (f32, f32),
    ) {
        self.swap_in(frames);
        self.scale_overlays(fx, fy);
        self.translate_overlays(dx, dy);
    }

    /// Put each produced frame in at its index. A stale index is skipped.
    fn swap_in(&mut self, frames: Vec<(usize, Frame)>) {
        for (i, frame) in frames {
            if let Some(slot) = self.frames.get_mut(i) {
                *slot = frame;
            }
        }
    }

    /// Scale every overlay's box by `(fx, fy)`, a text overlay's point size
    /// following the height.
    fn scale_overlays(&mut self, fx: f32, fy: f32) {
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

    /// Move every overlay's box by `(dx, dy)`.
    fn translate_overlays(&mut self, dx: f32, dy: f32) {
        for o in &mut self.overlays {
            o.transform.x += dx;
            o.transform.y += dy;
        }
    }

    /// The frames a zoom would produce: `rect` blown up to fill the canvas,
    /// as `(index, frame)` pairs the way `resized_frames` does for a resize.
    /// The canvas size never changes, which is what makes this safe per frame
    /// in a way a crop is not. Indices past the end of the frame list are
    /// skipped, and an empty document makes an empty answer. `progress`
    /// reports `(done, total)` over the frames that will actually be
    /// produced.
    pub fn zoomed_frames(
        &self,
        indices: &[usize],
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        progress: impl FnMut(usize, usize),
    ) -> Vec<(usize, Frame)> {
        let (cw, ch) = self.size();
        let Some((x, y, w, h)) = clamp_rect((cw, ch), x, y, w, h) else {
            return Vec::new();
        };
        self.produce(self.existing(indices).into_iter(), progress, |_, frame| {
            let cropped = crop_with_padding(frame.pixels.as_ref(), x, y, w, h);
            image::imageops::resize(&cropped, cw, ch, image::imageops::FilterType::Triangle)
        })
    }

    /// The frames a shrink would produce, as `(index, frame)` pairs the way
    /// `zoomed_frames` does for a zoom: `rect` is kept and everything outside
    /// it made transparent. Unlike a zoom the kept region is not scaled back
    /// up to fill the canvas — it stays at its own size and sits where it was
    /// cropped from — so a frame can *look* smaller than the others without
    /// the model needing a per-frame canvas size.
    pub fn shrunk_frames(
        &self,
        indices: &[usize],
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        progress: impl FnMut(usize, usize),
    ) -> Vec<(usize, Frame)> {
        let (cw, ch) = self.size();
        let Some((x, y, w, h)) = clamp_rect((cw, ch), x, y, w, h) else {
            return Vec::new();
        };
        self.produce(self.existing(indices).into_iter(), progress, |_, frame| {
            let kept = image::imageops::crop_imm(frame.pixels.as_ref(), x, y, w, h).to_image();
            let mut canvas = RgbaImage::new(cw, ch);
            image::imageops::replace(&mut canvas, &kept, x as i64, y as i64);
            canvas
        })
    }

    /// The frames a mirror would produce, as `(index, frame)` pairs the way
    /// `zoomed_frames` does. Overlays are live and stay where they are; only
    /// the frame pixels flip.
    pub fn mirrored_frames(
        &self,
        indices: &[usize],
        axis: Axis,
        progress: impl FnMut(usize, usize),
    ) -> Vec<(usize, Frame)> {
        self.produce(
            self.existing(indices).into_iter(),
            progress,
            |_, frame| match axis {
                Axis::Horizontal => image::imageops::flip_horizontal(frame.pixels.as_ref()),
                Axis::Vertical => image::imageops::flip_vertical(frame.pixels.as_ref()),
            },
        )
    }

    /// The frames a fade over `indices` would produce. The picked frames are
    /// the ramp, in timeline order whether or not they touch: a fade in
    /// starts on solid `colour` and climbs towards the frame after the run, a
    /// fade out falls from the frame before the run to solid `colour` on its
    /// last frame. Each step mixes `1/len` more or less of the colour in.
    pub fn faded_frames(
        &self,
        indices: &[usize],
        fade: Fade,
        colour: Rgba8,
        progress: impl FnMut(usize, usize),
    ) -> Vec<(usize, Frame)> {
        let wanted = sorted_existing(indices, self.frames.len());
        let total = wanted.len();
        self.produce(wanted.into_iter(), progress, |step, frame| {
            let amount = match fade {
                Fade::In => 1.0 - step as f32 / total as f32,
                Fade::Out => (step + 1) as f32 / total as f32,
            };
            let mut pixels = (*frame.pixels).clone();
            for p in pixels.pixels_mut() {
                p.0 = mix(p.0, colour, amount);
            }
            pixels
        })
    }

    /// How many frames a loop cross-fade can blend: the head it fades into
    /// and the tail it fades over must not overlap.
    pub fn loop_fade_limit(&self) -> usize {
        self.frames.len() / 2
    }

    /// The tail frames a loop cross-fade of `count` frames produces. The
    /// last `count` frames dissolve into the first `count`, so the end plays
    /// into the start without a cut; landing them is half the edit, and
    /// deleting the first `count` frames — now played by the dissolve — is
    /// the other half, `apply_loop_fade`'s second step.
    pub fn loop_faded_frames(
        &self,
        count: usize,
        progress: impl FnMut(usize, usize),
    ) -> Vec<(usize, Frame)> {
        let count = count.min(self.loop_fade_limit());
        let tail_start = self.frames.len() - count;
        self.produce(tail_start..self.frames.len(), progress, |step, tail| {
            let head = &self.frames[step];
            let into_head = (step + 1) as f32 / (count + 1) as f32;
            let (w, h) = tail.pixels.dimensions();
            let head_pixels = crop_with_padding(head.pixels.as_ref(), 0, 0, w, h);
            let mut pixels = (*tail.pixels).clone();
            for (p, q) in pixels.pixels_mut().zip(head_pixels.pixels()) {
                p.0 = mix(p.0, q.0, into_head);
            }
            pixels
        })
    }

    /// Land a loop cross-fade: swap in the dissolved tail, then drop the head
    /// it now plays. The head's time goes with it — the dissolve already
    /// spends the tail's.
    pub fn apply_loop_fade(&mut self, tail: Vec<(usize, Frame)>) {
        let count = tail.len();
        self.swap_in(tail);
        self.delete_frames(0..count);
    }

    /// The frame-job driver: `redraw` each frame in `indices` — given its
    /// step through them, for a ramp — as a `rebuilt` frame paired with its
    /// index, reporting `(done, total)` after each one.
    fn produce(
        &self,
        indices: impl ExactSizeIterator<Item = usize>,
        mut progress: impl FnMut(usize, usize),
        mut redraw: impl FnMut(usize, &Frame) -> RgbaImage,
    ) -> Vec<(usize, Frame)> {
        let total = indices.len();
        indices
            .enumerate()
            .map(|(step, i)| {
                let frame = &self.frames[i];
                let produced = rebuilt(frame, redraw(step, frame));
                progress(step + 1, total);
                (i, produced)
            })
            .collect()
    }

    /// `indices` with anything past the end dropped, so a stale scope never
    /// indexes out of bounds.
    fn existing(&self, indices: &[usize]) -> Vec<usize> {
        indices
            .iter()
            .copied()
            .filter(|i| *i < self.frames.len())
            .collect()
    }
}

/// `(x, y, w, h)` clamped onto a `cw`×`ch` canvas: the corner inside it, the
/// size at least a pixel and no further than the far edge. `None` for an
/// empty canvas, which has nowhere to put the corner.
fn clamp_rect(
    (cw, ch): (u32, u32),
    x: u32,
    y: u32,
    w: u32,
    h: u32,
) -> Option<(u32, u32, u32, u32)> {
    if cw == 0 || ch == 0 {
        return None;
    }
    let (x, y) = (x.min(cw - 1), y.min(ch - 1));
    Some((x, y, w.min(cw - x).max(1), h.min(ch - y).max(1)))
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

/// A frame with new pixels and everything else carried over from `source`.
pub(super) fn rebuilt(source: &Frame, pixels: RgbaImage) -> Frame {
    let mut produced = Frame::new(pixels, source.delay_cs);
    produced.detached = source.detached;
    produced
}

/// `delay + extra`, held at the longest delay a GIF can store.
fn add_delay(delay: u16, extra: u32) -> u16 {
    (delay as u32 + extra).min(u16::MAX as u32) as u16
}

/// `a` with `amount` (0–1) of `b` mixed in, weighting colour by alpha so a
/// transparent pixel contributes no colour and fading towards transparent
/// thins the alpha instead of darkening the image.
fn mix(a: Rgba8, b: Rgba8, amount: f32) -> Rgba8 {
    let wa = a[3] as f32 * (1.0 - amount);
    let wb = b[3] as f32 * amount;
    let alpha = wa + wb;
    if alpha <= 0.0 {
        return [0, 0, 0, 0];
    }
    let channel = |c: usize| ((a[c] as f32 * wa + b[c] as f32 * wb) / alpha).round() as u8;
    [channel(0), channel(1), channel(2), alpha.round() as u8]
}

/// Per-frame delays for `count` frames at `fps`, distributing the centisecond
/// remainder so the total matches the source instead of drifting.
///
/// 30fps is 3.33cs, which GIF cannot store; rounding every frame to 3cs gives a
/// 33.3fps animation that runs ahead of the recording.
pub fn delays_for_fps(fps: f64, count: usize) -> Vec<u16> {
    let mut delays = Vec::with_capacity(count);
    let mut written = 0u32;
    for i in 0..count {
        let elapsed = (i + 1) as f64 * 100.0 / fps;
        let want = elapsed.round() as u32;
        delays.push((want.saturating_sub(written)).max(1) as u16);
        written = written.max(want);
    }
    delays
}

/// A move removes the picked frames, which shifts every later index down,
/// then inserts at `to` — so where the run actually lands depends on how
/// many picked frames sat before the gap. `gap` names a position between
/// frames as shown before the move (`count` is past the last); this is the
/// `to` that puts the run exactly there once `picked` is gone from the
/// list, matching what the divider promised. `picked` must be sorted.
pub fn move_target_for_set(picked: &[usize], gap: usize) -> usize {
    gap - picked.iter().filter(|&&p| p < gap).count()
}

/// The gap "nudge the whole selection one slot earlier/later" aims at: just
/// before the unselected frame preceding it, or just after the one
/// following it — swapping the block with that neighbour. `None` when the
/// selection already sits against that edge, or names every frame and
/// leaves nothing to swap with. `picked` must be sorted.
pub fn selection_nudge_gap(picked: &[usize], earlier: bool, frame_count: usize) -> Option<usize> {
    let first = *picked.first()?;
    let last = *picked.last()?;
    if earlier {
        (first > 0).then(|| first - 1)
    } else {
        (last + 1 < frame_count).then(|| last + 2)
    }
}

/// A box drawn on the canvas in image pixels, as whole pixels on a `cw`×`ch`
/// canvas: intersected with the image first, then widened outward to whole
/// pixels. `None` when nothing of it is on the image.
pub fn normalize_canvas_rect(
    (cw, ch): (u32, u32),
    (x, y, w, h): (f32, f32, f32, f32),
) -> Option<(u32, u32, u32, u32)> {
    let left = x.floor().clamp(0.0, cw as f32) as u32;
    let top = y.floor().clamp(0.0, ch as f32) as u32;
    let right = (x + w).ceil().clamp(0.0, cw as f32) as u32;
    let bottom = (y + h).ceil().clamp(0.0, ch as f32) as u32;
    (right > left && bottom > top).then_some((left, top, right - left, bottom - top))
}

/// Whether cropping a `cw`×`ch` canvas to `(x, y, w, h)` would change it. A
/// full-canvas crop is not an edit, so it must not cost an undo step.
pub fn crop_changes_canvas((cw, ch): (u32, u32), (x, y, w, h): (u32, u32, u32, u32)) -> bool {
    cw > 0 && ch > 0 && !(x == 0 && y == 0 && w >= cw && h >= ch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Frame, OverlayKind, Shape, ShapeOverlay, Transform};

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

    /// "Every 1st frame" would be every frame and "every 0th" a division by
    /// zero; neither is a thinning, so both leave the document alone.
    #[test]
    fn drop_every_nth_below_two_keeps_every_frame() {
        for n in [0, 1] {
            let mut d = doc(4, 5);
            d.drop_every_nth(n);
            assert_eq!(d.frames.len(), 4, "n = {n}");
        }
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

    /// Regression: replacing a frame's pixels used to assign them in place,
    /// leaving the cached thumbnail and opacity flag describing the old
    /// image. Every producer rebuilds through `rebuilt` instead.
    #[test]
    fn rebuilding_a_frame_refreshes_the_cached_thumbnail() {
        let mut before = Frame::new(
            RgbaImage::from_pixel(80, 80, image::Rgba([255, 0, 0, 255])),
            9,
        );
        before.detached = true;

        let after = rebuilt(
            &before,
            RgbaImage::from_pixel(80, 20, image::Rgba([0, 0, 255, 128])),
        );

        assert_eq!(after.delay_cs, 9, "the delay survives the swap");
        assert!(after.detached, "so does the detached flag");
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

        doc.duplicate_frames_at(&[1]);
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

    /// The reported bug: trimming a GIF's opening frames left the first
    /// survivor holding their whole running time, a multi-second freeze. A
    /// run deleted off either end takes its time with it; a frame deleted
    /// from between two survivors still hands its time to the one before.
    #[test]
    fn deleting_off_either_end_drops_the_time_but_between_survivors_keeps_it() {
        let delays = |d: &Document| d.frames.iter().map(|f| f.delay_cs).collect::<Vec<_>>();
        let numbered = || {
            Document::from_frames(
                (1..=6u16)
                    .map(|cs| Frame::new(RgbaImage::new(2, 2), cs * 10))
                    .collect(),
            )
        };

        let mut head = numbered();
        head.delete_frames(0..3);
        assert_eq!(delays(&head), vec![40, 50, 60], "trimmed start");

        let mut tail = numbered();
        tail.delete_frames_at(&[4, 5]);
        assert_eq!(delays(&tail), vec![10, 20, 30, 40], "trimmed end");

        let mut both = numbered();
        both.delete_frames_at(&[0, 2, 5]);
        assert_eq!(
            delays(&both),
            vec![20 + 30, 40, 50],
            "ends go, the middle merges"
        );
    }

    fn px(d: &Document, frame: usize, x: u32, y: u32) -> [u8; 4] {
        d.frames[frame].pixels.get_pixel(x, y).0
    }

    /// A 2×2 frame with one red corner at the top left, so each mirror axis
    /// moves it somewhere different.
    fn corner_doc(n: usize) -> Document {
        let mut img = RgbaImage::from_pixel(2, 2, image::Rgba([0, 0, 0, 255]));
        img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        Document::from_frames((0..n).map(|_| Frame::new(img.clone(), 7)).collect())
    }

    #[test]
    fn mirroring_flips_only_the_picked_frames_along_its_axis() {
        let mut d = corner_doc(3);
        d.frames[2].detached = true;
        let red = [255, 0, 0, 255];

        let horizontal = d.mirrored_frames(&[0, 2, 9], Axis::Horizontal, |_, _| {});
        assert_eq!(
            horizontal.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
            vec![0, 2],
            "the stale index is skipped"
        );
        let vertical = d.mirrored_frames(&[1], Axis::Vertical, |_, _| {});
        for (i, frame) in horizontal.into_iter().chain(vertical) {
            d.frames[i] = frame;
        }
        assert_eq!(px(&d, 0, 1, 0), red, "horizontal: top left → top right");
        assert_eq!(px(&d, 1, 0, 1), red, "vertical: top left → bottom left");
        assert_eq!(px(&d, 2, 1, 0), red);
        assert!(d.frames[2].detached, "detached rides along");
        assert!(
            d.frames.iter().all(|f| f.delay_cs == 7),
            "delays ride along"
        );
    }

    fn grey_doc(n: usize) -> Document {
        Document::from_frames(
            (0..n)
                .map(|_| {
                    Frame::new(
                        RgbaImage::from_pixel(2, 2, image::Rgba([200, 200, 200, 255])),
                        5,
                    )
                })
                .collect(),
        )
    }

    #[test]
    fn a_fade_ramps_from_and_to_the_colour_across_the_picked_run() {
        let black = [0, 0, 0, 255];
        let d = grey_doc(6);
        let red = |frames: Vec<(usize, Frame)>| -> Vec<(usize, u8)> {
            frames
                .into_iter()
                .map(|(i, f)| (i, f.pixels.get_pixel(0, 0)[0]))
                .collect()
        };

        // Picked out of order: the ramp still follows the timeline.
        let fade_in = red(d.faded_frames(&[3, 1, 2, 4], Fade::In, black, |_, _| {}));
        assert_eq!(fade_in, vec![(1, 0), (2, 50), (3, 100), (4, 150)]);

        let fade_out = red(d.faded_frames(&[2, 3, 4, 5], Fade::Out, black, |_, _| {}));
        assert_eq!(fade_out, vec![(2, 150), (3, 100), (4, 50), (5, 0)]);
    }

    #[test]
    fn fading_to_transparent_thins_alpha_instead_of_darkening() {
        let d = grey_doc(2);
        let faded = d.faded_frames(&[0, 1], Fade::Out, [0, 0, 0, 0], |_, _| {});
        assert_eq!(faded[0].1.pixels.get_pixel(0, 0).0, [200, 200, 200, 128]);
        assert_eq!(faded[1].1.pixels.get_pixel(0, 0).0, [0, 0, 0, 0]);
    }

    /// Frame `i` is solid grey level `i * 10`, so a blend reads back as the
    /// mix of the two levels it came from.
    fn level_doc(n: usize) -> Document {
        Document::from_frames(
            (0..n)
                .map(|i| {
                    let v = (i * 10) as u8;
                    Frame::new(RgbaImage::from_pixel(2, 2, image::Rgba([v, v, v, 255])), 4)
                })
                .collect(),
        )
    }

    /// The loop has to run tail → head without a cut: the last frame is
    /// mostly the head's last blended frame, and what plays next is the
    /// first frame the dissolve did not already show.
    #[test]
    fn a_loop_fade_dissolves_the_tail_into_the_head_and_drops_the_head() {
        let mut d = level_doc(10);
        d.add_overlay("head", shape(), Transform::at(0., 0., 1., 1.), 0..2);
        let tail = d.loop_faded_frames(2, |_, _| {});
        assert_eq!(tail.iter().map(|(i, _)| *i).collect::<Vec<_>>(), vec![8, 9]);
        d.apply_loop_fade(tail);

        let levels: Vec<u8> = (0..d.frames.len()).map(|i| px(&d, i, 0, 0)[0]).collect();
        // tail 80 → head 0 at a third: 53; tail 90 → head 10 at two thirds: 37
        assert_eq!(levels, vec![20, 30, 40, 50, 60, 70, 53, 37]);
        assert_eq!(d.duration_cs(), 8 * 4, "the head's time goes with it");
        assert!(
            d.overlays.is_empty(),
            "an overlay only on the head goes too"
        );
    }

    #[test]
    fn a_loop_fade_never_blends_a_frame_into_itself() {
        let d = level_doc(5);
        assert_eq!(d.loop_fade_limit(), 2);
        let tail = d.loop_faded_frames(99, |_, _| {});
        assert_eq!(tail.iter().map(|(i, _)| *i).collect::<Vec<_>>(), vec![3, 4]);
        assert!(level_doc(1).loop_faded_frames(3, |_, _| {}).is_empty());
    }

    #[test]
    fn set_delay_at_touches_only_the_named_frames_and_ignores_stale_indices() {
        let mut d = doc(5, 10);
        d.set_delay_at(&[3, 1, 1, 99], 40);
        assert_eq!(
            d.frames.iter().map(|f| f.delay_cs).collect::<Vec<_>>(),
            vec![10, 40, 10, 40, 10],
            "only frames 1 and 3 change; the duplicate and the out-of-range index are no-ops"
        );
    }

    /// Frames the test can tell apart: frame `i` paints its own index.
    fn doc_distinct(n: usize) -> Document {
        Document::from_frames(
            (0..n)
                .map(|i| {
                    Frame::new(
                        image::RgbaImage::from_pixel(2, 2, image::Rgba([i as u8, 0, 0, 255])),
                        10,
                    )
                })
                .collect(),
        )
    }

    fn order(d: &Document) -> Vec<u8> {
        d.frames
            .iter()
            .map(|f| f.pixels.get_pixel(0, 0)[0])
            .collect()
    }

    #[test]
    fn move_frames_to_of_one_frame_matches_move_frame() {
        let mut a = doc_distinct(5);
        a.move_frames_to(&[0], 3);
        let mut b = doc_distinct(5);
        b.move_frame(0, 3);
        assert_eq!(order(&a), vec![1, 2, 3, 0, 4]);
        assert_eq!(order(&a), order(&b), "a one-frame set is a plain move");
    }

    #[test]
    fn move_frames_to_extracts_a_gappy_selection_as_one_run() {
        let mut d = doc_distinct(8);
        d.move_frames_to(&[2, 5, 7], 1);
        assert_eq!(
            order(&d),
            vec![0, 2, 5, 7, 1, 3, 4, 6],
            "the picked frames keep their order and land together; the rest close up"
        );
    }

    #[test]
    fn move_frames_to_ignores_duplicates_and_stale_indices() {
        let mut a = doc_distinct(8);
        a.move_frames_to(&[7, 2, 5, 2], 1);
        let mut b = doc_distinct(8);
        b.move_frames_to(&[2, 5, 7], 1);
        assert_eq!(order(&a), order(&b));
    }

    #[test]
    fn move_frames_to_already_in_place_is_a_no_op() {
        let mut d = doc_distinct(5);
        d.move_frames_to(&[2, 3, 4], 2);
        assert_eq!(order(&d), vec![0, 1, 2, 3, 4]);
    }

    /// Keys are shared by a frame and its duplicate or pasted copy, so the
    /// frames a drag picked cannot be found again by key: the second hop
    /// would grab the copy the first hop just moved and leave the picked one
    /// behind.
    #[test]
    fn move_frames_to_moves_a_frame_and_its_duplicate_together() {
        let mut d = doc_distinct(3);
        d.duplicate_frames_at(&[2]);
        d.frames[3].delay_cs = 40; // tell the copy from its source
        d.move_frames_to(&[2, 3], 0);
        assert_eq!(order(&d), vec![2, 2, 0, 1], "both picked frames lead");
        assert_eq!(
            d.frames.iter().map(|f| f.delay_cs).collect::<Vec<_>>(),
            vec![10, 40, 10, 10],
            "in the order they were picked"
        );
    }

    #[test]
    fn move_frames_to_shifts_overlays_like_a_plain_move() {
        let mut a = doc_distinct(5);
        a.add_overlay("o", shape(), Transform::at(0., 0., 1., 1.), 2..5);
        a.move_frames_to(&[0], 4);
        let mut b = doc_distinct(5);
        b.add_overlay("o", shape(), Transform::at(0., 0., 1., 1.), 2..5);
        b.move_frame(0, 4);
        let ranges = |d: &Document| {
            d.overlays
                .iter()
                .map(|o| o.range.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(ranges(&a), ranges(&b), "each hop is a plain move_frame");
    }

    #[test]
    fn duplicate_extends_covering_overlays() {
        let mut d = doc(4, 10);
        let covering = d.add_overlay("a", shape(), Transform::at(0., 0., 1., 1.), 1..3);
        let after = d.add_overlay("b", shape(), Transform::at(0., 0., 1., 1.), 3..4);
        d.duplicate_frames_at(&[1]);
        assert_eq!(d.frames.len(), 5);
        assert_eq!(d.overlay(covering).unwrap().range, 1..4);
        assert_eq!(d.overlay(after).unwrap().range, 4..5);
    }

    /// Duplicating a selection in one pass has to land exactly where copying
    /// the frames one at a time, back to front, did — frames, copies and every
    /// overlay boundary — including at a band's edges and the last frame.
    #[test]
    fn duplicating_a_selection_at_once_matches_one_frame_at_a_time() {
        let t = Transform::at(0., 0., 1., 1.);
        let mut one_pass = doc_distinct(7);
        one_pass.add_overlay("starts-on-pick", shape(), t, 1..3);
        one_pass.add_overlay("ends-after-pick", shape(), t, 3..5);
        one_pass.add_overlay("untouched", shape(), t, 5..6);
        one_pass.add_overlay("to-the-end", shape(), t, 4..7);
        let mut one_by_one = one_pass.clone();

        let picked = [1, 4, 6, 99];
        one_pass.duplicate_frames_at(&picked);
        // The one-at-a-time definition: a copy spliced in right after it.
        for &i in picked.iter().rev().filter(|i| **i < 7) {
            let copy = one_by_one.frames[i].clone();
            one_by_one.insert_frames_at(i + 1, vec![copy]);
        }
        assert_eq!(order(&one_pass), vec![0, 1, 1, 2, 3, 4, 4, 5, 6, 6]);
        assert_eq!(one_pass, one_by_one);
    }

    #[test]
    fn inserting_one_frame_shifts_overlays_like_duplicate_does() {
        let mut d = doc(4, 10);
        let covering = d.add_overlay("a", shape(), Transform::at(0., 0., 1., 1.), 1..3);
        let after = d.add_overlay("b", shape(), Transform::at(0., 0., 1., 1.), 3..4);
        d.insert_frames_at(1, vec![Frame::new(RgbaImage::new(2, 2), 10)]);
        assert_eq!(d.frames.len(), 5);
        assert_eq!(d.overlay(covering).unwrap().range, 2..4);
        assert_eq!(d.overlay(after).unwrap().range, 4..5);
    }

    /// Paste: the run keeps its order and lands as one block, and — like a
    /// duplicate — the overlays covering the slot grow over it.
    #[test]
    fn insert_frames_at_keeps_the_run_in_order() {
        let mut d = doc_distinct(4);
        let covering = d.add_overlay("a", shape(), Transform::at(0., 0., 1., 1.), 0..2);
        let pasted = vec![d.frames[3].clone(), d.frames[2].clone()];
        d.insert_frames_at(1, pasted);
        let reds: Vec<u8> = d
            .frames
            .iter()
            .map(|f| f.pixels.get_pixel(0, 0).0[0])
            .collect();
        assert_eq!(reds, vec![0, 3, 2, 1, 2, 3], "clipboard order preserved");
        assert_eq!(d.overlay(covering).unwrap().range, 0..4);
    }

    /// An imported clip spliced mid-timeline: overlays past the seam slide
    /// right with their frames, the ones merely touching the seam stay off
    /// the new run, and one the run lands inside still spans it.
    #[test]
    fn foreign_frames_spliced_mid_timeline_shift_later_overlays_but_not_onto_the_run() {
        let mut d = doc_distinct(6);
        let t = Transform::at(0., 0., 1., 1.);
        let ends_at_seam = d.add_overlay("a", shape(), t, 0..2);
        let starts_at_seam = d.add_overlay("b", shape(), t, 2..4);
        let straddles = d.add_overlay("c", shape(), t, 1..3);
        let later = d.add_overlay("d", shape(), t, 4..6);
        let clip = Frame::new(
            RgbaImage::from_pixel(2, 2, image::Rgba([200, 0, 0, 255])),
            10,
        );

        d.insert_foreign_frames_at(2, vec![clip; 3]);

        assert_eq!(order(&d), vec![0, 1, 200, 200, 200, 2, 3, 4, 5]);
        let range = |id| d.overlay(id).unwrap().range.clone();
        assert_eq!(range(ends_at_seam), 0..2, "stops where the clip starts");
        assert_eq!(range(starts_at_seam), 5..7, "moves with its frames");
        assert_eq!(range(straddles), 1..6, "the clip landed inside it");
        assert_eq!(range(later), 7..9);
    }

    /// A frame index or range from before the document shrank is stale;
    /// acting on it must change nothing rather than panic or hit the wrong
    /// frames.
    #[test]
    fn a_stale_frame_index_changes_nothing() {
        let mut d = doc_distinct(3);
        let before = d.clone();
        d.duplicate_frames_at(&[3]);
        d.reverse_frames(1..4);
        assert_eq!(d, before);
    }

    /// A step in the layer list moves the overlay next to the neighbour it
    /// was shown beside; overlays it skipped over keep their own order.
    #[test]
    fn restack_moves_one_overlay_next_to_a_named_neighbour() {
        let mut d = doc(4, 10);
        let bottom = d.add_overlay("bottom", shape(), Transform::at(0., 0., 1., 1.), 0..4);
        let elsewhere = d.add_overlay("elsewhere", shape(), Transform::at(0., 0., 1., 1.), 3..4);
        let top = d.add_overlay("top", shape(), Transform::at(0., 0., 1., 1.), 0..4);
        let order = |d: &Document| d.overlays.iter().map(|o| o.id).collect::<Vec<_>>();

        d.restack_overlay(bottom, top, true);
        assert_eq!(
            order(&d),
            vec![elsewhere, top, bottom],
            "bottom lands directly above top, elsewhere stays put"
        );

        d.restack_overlay(bottom, top, false);
        assert_eq!(order(&d), vec![elsewhere, bottom, top], "and back down");

        d.restack_overlay(bottom, bottom, true);
        assert_eq!(order(&d), vec![elsewhere, bottom, top], "self is a no-op");
    }

    /// The layer list can still hold an id an undo just removed. Restacking
    /// next to a missing neighbour must put the overlay back in its own
    /// slot, not drop it or move it to the top; a missing overlay moves or
    /// copies nothing.
    #[test]
    fn restacking_or_copying_with_a_stale_id_leaves_the_stack_alone() {
        let mut d = doc(4, 10);
        let t = Transform::at(0., 0., 1., 1.);
        let a = d.add_overlay("a", shape(), t, 0..4);
        let b = d.add_overlay("b", shape(), t, 0..4);
        d.add_overlay("c", shape(), t, 0..4);
        let gone = d.add_overlay("gone", shape(), t, 0..4);
        d.remove_overlay(gone);
        let before = d.clone();

        d.restack_overlay(b, gone, true);
        assert_eq!(d, before, "b stays between a and c");
        d.restack_overlay(gone, a, true);
        assert_eq!(d, before);
        assert_eq!(d.copy_overlay_to(gone, &[0, 1]), 0);
        assert_eq!(d, before);
    }

    #[test]
    fn move_frame_reorders_the_list_and_leaves_out_of_range_calls_a_no_op() {
        let mut d = Document::from_frames(
            (0..4)
                .map(|i| Frame::new(RgbaImage::from_pixel(2, 2, image::Rgba([i, 0, 0, 255])), 5))
                .collect(),
        );
        d.move_frame(3, 1);
        let reds: Vec<u8> = d
            .frames
            .iter()
            .map(|f| f.pixels.get_pixel(0, 0).0[0])
            .collect();
        assert_eq!(reds, vec![0, 3, 1, 2], "frame 3 now sits at position 1");

        let before = d.clone();
        d.move_frame(9, 0);
        d.move_frame(0, 0);
        assert_eq!(d, before, "an out-of-range or no-op move changes nothing");
    }

    /// A single-frame overlay is wholly the moved frame's: it relocates with
    /// it rather than staying pinned to the timeline slot it left behind.
    #[test]
    fn move_frame_carries_a_single_frame_overlay_with_it() {
        let mut d = doc(6, 10);
        let id = d.add_overlay("a", shape(), Transform::at(0., 0., 1., 1.), 2..3);
        d.move_frame(2, 5);
        assert_eq!(
            d.overlay(id).unwrap().range,
            5..6,
            "followed frame 2 to its new slot"
        );
    }

    /// Moving one frame out of a wider overlay splits it: the untouched
    /// frames stay covered (now contiguous again), and the moved frame keeps
    /// its own copy at the new slot.
    #[test]
    fn move_frame_splits_an_overlay_when_only_part_of_it_moves() {
        let mut d = doc(6, 10);
        let id = d.add_overlay("a", shape(), Transform::at(0., 0., 1., 1.), 1..4);
        d.move_frame(2, 5);
        assert_eq!(
            d.overlay(id).unwrap().range,
            1..3,
            "frames 1 and 3 stayed behind and closed the gap"
        );
        assert_eq!(
            d.overlays.len(),
            2,
            "the moved frame split off its own copy"
        );
        let moved = d.overlays.iter().find(|o| o.id != id).unwrap();
        assert_eq!(
            moved.range,
            5..6,
            "the split copy sits on frame 2's new slot"
        );
    }

    /// Dragging a whole selection that shares one overlay should come back
    /// out as the one overlay it started as, not three per-hop fragments —
    /// each `move_frame` inside `move_frames_to` briefly splits it and the
    /// coalescing pass reunites it once the group lands together.
    #[test]
    fn move_frames_to_reunites_a_dragged_groups_overlay() {
        let mut d = doc_distinct(8);
        let id = d.add_overlay("a", shape(), Transform::at(0., 0., 1., 1.), 2..5);
        d.move_frames_to(&[2, 3, 4], 0);
        assert_eq!(order(&d), vec![2, 3, 4, 0, 1, 5, 6, 7]);
        assert_eq!(
            d.overlays.len(),
            1,
            "the split-then-merged pieces folded back into one"
        );
        assert_eq!(
            d.overlay(id).unwrap().range,
            0..3,
            "moved with the group, still the same id"
        );
    }

    /// The band menu's "Copy overlay to selected frames": a gappy selection
    /// gets a piece per run, frames already carrying the overlay are left
    /// alone, and a piece that lands against the source folds into it rather
    /// than sitting beside it as a second band of the same thing.
    #[test]
    fn copying_an_overlay_lands_a_piece_on_every_picked_run() {
        let mut d = doc(10, 10);
        let id = d.add_overlay("a", shape(), Transform::at(1., 2., 3., 4.), 2..4);
        d.overlay_mut(id).unwrap().opacity = 0.5;

        // 2 and 3 are already covered; 4 touches the range, 6..8 and 9 do not.
        let landed = d.copy_overlay_to(id, &[2, 3, 4, 6, 7, 9]);
        assert_eq!(landed, 4, "the two covered frames are not copied onto");
        let mut ranges: Vec<Range<usize>> = d.overlays.iter().map(|o| o.range.clone()).collect();
        ranges.sort_by_key(|r| r.start);
        assert_eq!(
            ranges,
            vec![2..5, 6..8, 9..10],
            "frame 4 folded into the source, the rest are one piece per run"
        );
        assert_eq!(
            d.overlay(id).unwrap().range,
            2..5,
            "the source keeps its id, so a selection on it survives"
        );
        for o in &d.overlays {
            assert_eq!(o.transform, Transform::at(1., 2., 3., 4.));
            assert_eq!(o.opacity, 0.5, "a copy paints like what it copied");
            assert_eq!(o.name, "a");
        }
    }

    /// A copy has to stack where the source stacks: the pieces go in beside
    /// it, not on top of a list whose order *is* the z-order — otherwise
    /// copying the bottom overlay would land it over everything on its new
    /// frames.
    #[test]
    fn a_copied_overlay_keeps_the_source_z_position() {
        let mut d = doc(6, 10);
        let under = d.add_overlay("under", shape(), Transform::at(0., 0., 1., 1.), 0..1);
        let over = d.add_overlay("over", shape(), Transform::at(9., 9., 1., 1.), 0..6);
        d.copy_overlay_to(under, &[3, 4]);
        let order: Vec<&str> = d.overlays.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(
            order,
            vec!["under", "under", "over"],
            "the copy sits directly above its source, still under {over:?}"
        );
    }

    /// Nothing to copy onto changes nothing: the UI turns this into a notice
    /// instead of an undo step, and the model must not invent a piece.
    #[test]
    fn copying_an_overlay_onto_frames_it_covers_changes_nothing() {
        let mut d = doc(4, 10);
        let id = d.add_overlay("a", shape(), Transform::at(0., 0., 1., 1.), 0..4);
        let before = d.clone();
        assert_eq!(d.copy_overlay_to(id, &[0, 1, 2, 3]), 0);
        assert_eq!(d.copy_overlay_to(id, &[7, 9]), 0, "past the last frame");
        assert_eq!(d, before);
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

    /// The reported bug: a slow pan, every step equally small. Scored once
    /// against the original neighbours, all steps tied and the removal took
    /// one contiguous run off the end, piling its delay onto a single frame
    /// near the end. Scoring against what stays on screen spreads it out.
    #[test]
    fn smart_removal_thins_a_slow_pan_evenly_instead_of_piling_up_one_hold() {
        let mut d = Document::from_frames(
            (0..10u8)
                .map(|i| {
                    Frame::new(
                        RgbaImage::from_pixel(40, 40, image::Rgba([i * 20, 10, 10, 255])),
                        5,
                    )
                })
                .collect(),
        );
        d.drop_low_motion(5);
        assert_eq!(
            d.frames.iter().map(|f| f.delay_cs).collect::<Vec<_>>(),
            vec![10; 5],
            "every other frame goes, each survivor holds two slots"
        );
        assert_eq!(
            d.frames
                .iter()
                .map(|f| f.pixels.get_pixel(0, 0).0[0])
                .collect::<Vec<_>>(),
            vec![0, 40, 80, 120, 160]
        );
    }

    /// A frame saved back from an external editor can come back another size,
    /// and then its thumbnail does not line up with its neighbour's. That is
    /// a change on screen, not a still: the truly identical frame goes first.
    #[test]
    fn smart_removal_counts_a_resized_frame_as_motion() {
        let px = image::Rgba([10, 10, 10, 255]);
        let mut d = Document::from_frames(vec![
            Frame::new(RgbaImage::from_pixel(40, 40, px), 5),
            Frame::new(RgbaImage::from_pixel(40, 40, px), 5),
            Frame::new(RgbaImage::from_pixel(40, 20, px), 5),
        ]);
        d.drop_low_motion(1);
        assert_eq!(
            d.frames
                .iter()
                .map(|f| f.pixels.dimensions())
                .collect::<Vec<_>>(),
            vec![(40, 40), (40, 20)]
        );
    }

    /// Land frames from a job that keeps the canvas: nothing to scale or move.
    fn land_in_place(d: &mut Document, frames: Vec<(usize, Frame)>) {
        d.land(frames, (1.0, 1.0), (0.0, 0.0));
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

        let cropped = d.cropped_frames(20, 15, 50, 40, |_, _| {});
        d.land(cropped, (1.0, 1.0), (-20.0, -15.0));

        assert_eq!(d.size(), (50, 40));
        assert!(d.frames.iter().all(|f| f.pixels.dimensions() == (50, 40)));
        let t = d.overlay(id).unwrap().transform;
        assert_eq!((t.x, t.y), (10.0, 10.0), "same pixel, new origin");
        assert_eq!(
            d.frames[0].thumb.width(),
            crate::model::THUMB_BOX,
            "thumbnail redrawn"
        );
    }

    #[test]
    fn crop_clamps_to_the_canvas_rather_than_panicking() {
        let mut d = Document::from_frames(vec![Frame::new(RgbaImage::new(20, 20), 5)]);
        let cropped = d.cropped_frames(15, 15, 400, 400, |_, _| {});
        land_in_place(&mut d, cropped);
        assert_eq!(d.size(), (5, 5));

        let empty = Document::default();
        assert!(empty.cropped_frames(0, 0, 10, 10, |_, _| {}).is_empty());
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

        let frames = cropped.cropped_frames(5, 5, 10, 10, |_, _| {});
        cropped.land(frames, (1.0, 1.0), (-5.0, -5.0));
        assert!(
            cropped
                .frames
                .iter()
                .all(|frame| frame.pixels.dimensions() == (10, 10))
        );
        assert_eq!(cropped.frames[1].pixels.get_pixel(9, 9).0[3], 0);

        let frames = zoomed.zoomed_frames(&[1], 5, 5, 10, 10, |_, _| {});
        land_in_place(&mut zoomed, frames);
        assert_eq!(zoomed.frames[1].pixels.dimensions(), (20, 20));
        assert_eq!(zoomed.frames[1].pixels.get_pixel(19, 19).0[3], 0);
    }

    /// What a wholesale frame swap would otherwise lose rides along with a
    /// crop: the delay and the detached flag. Progress counts every frame.
    #[test]
    fn cropped_frames_carry_delay_and_detached() {
        let d = Document::from_frames(
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

        let mut seen = Vec::new();
        let produced = d.cropped_frames(5, 5, 20, 20, |done, total| seen.push((done, total)));

        assert_eq!(
            produced.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert_eq!(
            produced.iter().map(|(_, f)| f.delay_cs).collect::<Vec<_>>(),
            vec![3, 4, 5],
            "delays survive the crop"
        );
        assert!(produced[1].1.detached, "detached survives the crop");
        assert_eq!(
            seen,
            vec![(1, 3), (2, 3), (3, 3)],
            "progress is reported once per frame"
        );
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

        let zoomed = d.zoomed_frames(&[1, 2], 0, 0, 20, 20, |_, _| {});
        land_in_place(&mut d, zoomed);

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

    /// A resize lands its frames and scales the overlays with them, down to
    /// what a wholesale frame swap would otherwise lose: the delay and the
    /// detached flag ride along.
    #[test]
    fn a_resize_lands_its_frames_and_scales_the_overlays() {
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
        let caption = d.add_overlay(
            "t",
            OverlayKind::Text(crate::TextOverlay::default()),
            Transform::at(0.0, 0.0, 8.0, 8.0),
            0..3,
        );

        let mut seen = Vec::new();
        let produced = d.resized_frames(20, 10, |done, total| seen.push((done, total)));
        d.land(produced, (0.5, 0.25), (0.0, 0.0));

        assert_eq!(d.size(), (20, 10));
        assert_eq!(
            d.frames.iter().map(|f| f.delay_cs).collect::<Vec<_>>(),
            vec![3, 4, 5],
            "delays survive the resample"
        );
        assert!(d.frames[1].detached, "detached survives the resample");
        let t = d.overlay(id).unwrap().transform;
        assert_eq!((t.x, t.y, t.w, t.h), (5.0, 5.0, 20.0, 10.0));
        let Some(OverlayKind::Text(text)) = d.overlay(caption).map(|o| &o.kind) else {
            panic!("the caption is text");
        };
        assert_eq!(text.size_px, 8.0, "a point size follows the height");
        assert_eq!(
            seen,
            vec![(1, 3), (2, 3), (3, 3)],
            "progress is reported once per frame"
        );
    }

    #[test]
    fn resized_frames_clamp_zero_dimensions() {
        let mut d = doc(1, 5);
        let produced = d.resized_frames(0, 0, |_, _| {});
        land_in_place(&mut d, produced);
        assert_eq!(d.size(), (1, 1));
    }

    /// The zoom producer carries delay and detached along, counts progress
    /// only over the frames it will produce, and skips indices past the end
    /// rather than misaligning the pairs.
    #[test]
    fn zoomed_frames_carry_delay_and_detached_and_skip_out_of_range() {
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

        let zoomed = d.zoomed_frames(&[1, 2], 0, 0, 20, 20, |_, _| {});
        land_in_place(&mut d, zoomed);

        let mut seen = Vec::new();
        // Index 9 is past the end: it must not appear as a pair.
        let produced = other.zoomed_frames(&[1, 9, 2], 0, 0, 20, 20, |done, total| {
            seen.push((done, total))
        });
        assert_eq!(
            produced.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
            vec![1, 2]
        );
        land_in_place(&mut other, produced);

        assert_eq!(other, d, "a stale index changes nothing");
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

    /// Unlike zoom, the kept region is not scaled back up: the frame stays
    /// canvas-sized (the model has no per-frame size) but everything outside
    /// the rect goes transparent, so the visible content looks smaller.
    #[test]
    fn shrink_keeps_canvas_size_and_blanks_outside_the_rect() {
        let mut d = Document::from_frames(
            (0..3)
                .map(|_| {
                    Frame::new(
                        RgbaImage::from_pixel(20, 20, image::Rgba([9, 9, 9, 255])),
                        5,
                    )
                })
                .collect(),
        );
        let untouched = d.frames[2].key;

        let shrunk = d.shrunk_frames(&[0, 1], 4, 4, 8, 8, |_, _| {});
        land_in_place(&mut d, shrunk);

        assert!(
            d.frames.iter().all(|f| f.pixels.dimensions() == (20, 20)),
            "canvas is uniform"
        );
        assert_eq!(d.frames[0].pixels.get_pixel(4, 4).0, [9, 9, 9, 255]);
        assert_eq!(
            d.frames[0].pixels.get_pixel(0, 0).0,
            [0, 0, 0, 0],
            "outside the kept rect is transparent"
        );
        assert_eq!(
            d.frames[2].key, untouched,
            "outside the scope nothing rebuilds"
        );
    }

    #[test]
    fn shrunk_frames_carry_detached_and_skip_out_of_range() {
        let mut d = Document::from_frames(
            (0..3)
                .map(|i| {
                    let mut frame = Frame::new(
                        RgbaImage::from_pixel(20, 20, image::Rgba([i, i, i, 255])),
                        5,
                    );
                    frame.detached = i == 1;
                    frame
                })
                .collect(),
        );
        let mut other = d.clone();

        let shrunk = d.shrunk_frames(&[0, 1], 2, 2, 10, 10, |_, _| {});
        land_in_place(&mut d, shrunk);

        let mut seen = Vec::new();
        let produced = other.shrunk_frames(&[0, 9, 1], 2, 2, 10, 10, |done, total| {
            seen.push((done, total))
        });
        land_in_place(&mut other, produced);

        assert_eq!(other, d, "a stale index changes nothing");
        assert_eq!(seen, vec![(1, 2), (2, 2)]);
        assert!(d.frames[1].detached, "detached survives the shrink");

        let empty = Document::default();
        assert!(
            empty
                .shrunk_frames(&[0], 0, 0, 10, 10, |_, _| {})
                .is_empty()
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

    /// End to end through the real `Document::move_frames_to`: whatever gap
    /// the divider is drawn at, the dragged frames end up adjacent to
    /// exactly the neighbor the divider promised, dragging either direction
    /// and as a multi-frame selection.
    #[test]
    fn move_target_for_set_round_trips_through_move_frames_to() {
        let numbered = |n: usize| {
            Document::from_frames(
                (0..n)
                    .map(|i| {
                        let px = image::Rgba([i as u8, 0, 0, 255]);
                        Frame::new(RgbaImage::from_pixel(8, 8, px), 5)
                    })
                    .collect(),
            )
        };
        let ids_at = |d: &Document| -> Vec<u8> {
            d.frames
                .iter()
                .map(|f| f.pixels.get_pixel(0, 0)[0])
                .collect()
        };

        // [0,1,2,3,4], drag 0 to the gap before 3 (gap 3): lands just before it.
        let mut d = numbered(5);
        d.move_frames_to(&[0], move_target_for_set(&[0], 3));
        assert_eq!(ids_at(&d), vec![1, 2, 0, 3, 4]);

        // Same document, drag 3 to the gap before 1 (gap 1): lands just before it.
        let mut d = numbered(5);
        d.move_frames_to(&[3], move_target_for_set(&[3], 1));
        assert_eq!(ids_at(&d), vec![0, 3, 1, 2, 4]);

        // Drag 0 to the gap past the last frame: lands at the very end.
        let mut d = numbered(5);
        d.move_frames_to(&[0], move_target_for_set(&[0], 5));
        assert_eq!(ids_at(&d), vec![1, 2, 3, 4, 0]);

        // A three-frame selection dropped between frames 1 and 2.
        let mut d = numbered(8);
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
}
