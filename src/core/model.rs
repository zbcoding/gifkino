//! Document model. The whole point is `Overlay::range`: an edit knows which
//! frames it applies to.

use std::fmt;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use image::RgbaImage;

pub type Rgba8 = [u8; 4];

/// Width of the filmstrip thumbnail. Lives here because the thumbnail is built
/// where the frame is, off the main thread.
pub const THUMB_W: u32 = 72;

static NEXT_KEY: AtomicU64 = AtomicU64::new(0);

/// Frame pixels are shared, not copied: history snapshots the whole document
/// (see `history::Editor`) and frame-list ops must stay cheap at 300 frames.
#[derive(Clone)]
pub struct Frame {
    pub pixels: Arc<RgbaImage>,
    pub delay_cs: u16,
    /// Came back from an external editor; overlays skip it. See design.md.
    pub detached: bool,
    /// Filmstrip thumbnail, and whether every pixel is fully opaque. Both are
    /// settled at construction, on whatever thread decoded the frame: computing
    /// either during a view update means doing it for the whole document, and a
    /// few hundred 1280x720 frames is seconds of frozen UI.
    pub thumb: Arc<RgbaImage>,
    pub opaque: bool,
    /// Identity for view caches. Two frames with the same key have the same
    /// pixels, because the only way to get one is to clone the other.
    pub key: u64,
}

impl Frame {
    pub fn new(pixels: RgbaImage, delay_cs: u16) -> Self {
        let (w, h) = pixels.dimensions();
        let thumb_h = (THUMB_W as f32 * h as f32 / w.max(1) as f32)
            .round()
            .max(1.0) as u32;
        let thumb = image::imageops::resize(
            &pixels,
            THUMB_W,
            thumb_h,
            image::imageops::FilterType::Triangle,
        );
        let opaque = pixels.as_raw().chunks_exact(4).all(|p| p[3] == 255);
        Frame {
            pixels: Arc::new(pixels),
            delay_cs,
            detached: false,
            thumb: Arc::new(thumb),
            opaque,
            key: NEXT_KEY.fetch_add(1, Ordering::Relaxed),
        }
    }
}

/// `thumb` and `opaque` are functions of the pixels and `key` is bookkeeping,
/// so equality is the pixels, the delay and the detached flag.
impl PartialEq for Frame {
    fn eq(&self, other: &Self) -> bool {
        self.delay_cs == other.delay_cs
            && self.detached == other.detached
            && self.pixels == other.pixels
    }
}

impl fmt::Debug for Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (w, h) = self.pixels.dimensions();
        write!(
            f,
            "Frame({w}x{h}, {}cs, detached={})",
            self.delay_cs, self.detached
        )
    }
}

/// Oriented box: un-rotated rect plus an angle, never a baked matrix.
/// Impasto's handles snap back to an axis-aligned box after a rotation and its
/// own notes call that a rewrite rather than a polish item — cheap here, so
/// carry the angle from the first commit.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Transform {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Radians, clockwise, about the rect's center.
    pub angle: f32,
}

impl Transform {
    pub fn at(x: f32, y: f32, w: f32, h: f32) -> Self {
        Transform {
            x,
            y,
            w,
            h,
            angle: 0.0,
        }
    }
    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    /// A point of the un-rotated box, placed on the image.
    pub fn to_image(self, x: f32, y: f32) -> (f32, f32) {
        spin(x, y, self.center(), self.angle)
    }

    /// The inverse: an image point read in the un-rotated box's own frame,
    /// which is the only frame hit tests and corner drags are simple in.
    pub fn to_local(self, x: f32, y: f32) -> (f32, f32) {
        spin(x, y, self.center(), -self.angle)
    }
}

fn spin(x: f32, y: f32, (cx, cy): (f32, f32), angle: f32) -> (f32, f32) {
    if angle == 0.0 {
        return (x, y);
    }
    let (sin, cos) = angle.sin_cos();
    let (ux, uy) = (x - cx, y - cy);
    (cx + ux * cos - uy * sin, cy + ux * sin + uy * cos)
}

/// Pango carries justification as a flag beside the alignment, but for a
/// caption box the four are one choice, so they are one enum here.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum TextAlign {
    Left,
    #[default]
    Center,
    Right,
    Justify,
}

#[derive(Clone, PartialEq, Debug)]
pub struct TextOverlay {
    pub text: String,
    pub font: String,
    pub size_px: f32,
    pub color: Rgba8,
    /// Outline color and width, the readable-over-anything default.
    pub outline: Option<(Rgba8, f32)>,
    pub align: TextAlign,
    /// Smooth glyph edges. On, because a caption is read, not counted; off is
    /// for pixel-art captures where a soft edge is the wrong look and costs
    /// palette entries besides.
    pub antialias: bool,
}

impl Default for TextOverlay {
    fn default() -> Self {
        TextOverlay {
            text: String::new(),
            font: "Sans Bold".into(),
            size_px: 32.0,
            color: [255, 255, 255, 255],
            outline: Some(([0, 0, 0, 255], 2.0)),
            align: TextAlign::default(),
            antialias: true,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    Rect,
    Ellipse,
    Arrow,
}

#[derive(Clone, PartialEq, Debug)]
pub struct ShapeOverlay {
    pub shape: Shape,
    pub fill: Option<Rgba8>,
    pub stroke: Option<(Rgba8, f32)>,
}

#[derive(Clone, PartialEq)]
pub struct ImageOverlay {
    pub pixels: Arc<RgbaImage>,
}

impl fmt::Debug for ImageOverlay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (w, h) = self.pixels.dimensions();
        write!(f, "ImageOverlay({w}x{h})")
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum OverlayKind {
    Text(TextOverlay),
    Shape(ShapeOverlay),
    Image(ImageOverlay),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct OverlayId(pub u32);

#[derive(Clone, PartialEq, Debug)]
pub struct Overlay {
    pub id: OverlayId,
    pub name: String,
    pub kind: OverlayKind,
    /// The frames this overlay appears on.
    pub range: Range<usize>,
    pub transform: Transform,
    pub opacity: f32,
    pub hidden: bool,
}

/// The toolbar control that gates every operation.
///
/// `Frames` is a set rather than a range because Ctrl+click in the strip picks
/// frames one at a time and they need not touch.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Scope {
    ThisFrame,
    AllFrames,
    Frames(Vec<usize>),
}

impl Scope {
    /// Frames this scope names: sorted, deduplicated and clamped.
    pub fn resolve(&self, playhead: usize, frame_count: usize) -> Vec<usize> {
        match self {
            Scope::ThisFrame if playhead < frame_count => vec![playhead],
            Scope::ThisFrame => Vec::new(),
            Scope::AllFrames => (0..frame_count).collect(),
            Scope::Frames(picked) => {
                let mut frames: Vec<usize> = picked
                    .iter()
                    .copied()
                    .filter(|i| *i < frame_count)
                    .collect();
                frames.sort_unstable();
                frames.dedup();
                frames
            }
        }
    }

    /// The smallest range covering the scope. Overlays carry a contiguous
    /// range, so a gappy selection widens to span it.
    pub fn span(&self, playhead: usize, frame_count: usize) -> Range<usize> {
        let frames = self.resolve(playhead, frame_count);
        match (frames.first(), frames.last()) {
            (Some(first), Some(last)) => *first..*last + 1,
            _ => 0..0,
        }
    }
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct Document {
    pub frames: Vec<Frame>,
    /// Bottom-to-top. List order *is* z-order; a separate `z` field would be a
    /// second copy of the same fact to keep in sync.
    pub overlays: Vec<Overlay>,
    next_id: u32,
}

impl Document {
    pub fn from_frames(frames: Vec<Frame>) -> Self {
        Document {
            frames,
            ..Default::default()
        }
    }

    pub fn size(&self) -> (u32, u32) {
        self.frames
            .first()
            .map_or((0, 0), |f| f.pixels.dimensions())
    }

    pub fn duration_cs(&self) -> u32 {
        self.frames.iter().map(|f| f.delay_cs as u32).sum()
    }

    /// Re-checked per call, not an import flag: a frame saved back from an
    /// external editor can introduce alpha long after import. Cheap because
    /// each frame answered the question when it was built.
    pub fn has_alpha(&self) -> bool {
        self.frames.iter().any(|f| !f.opaque)
            || self
                .overlays
                .iter()
                .any(|o| !matches!(o.kind, OverlayKind::Shape(_)))
    }

    pub fn overlay(&self, id: OverlayId) -> Option<&Overlay> {
        self.overlays.iter().find(|o| o.id == id)
    }

    pub fn overlay_mut(&mut self, id: OverlayId) -> Option<&mut Overlay> {
        self.overlays.iter_mut().find(|o| o.id == id)
    }

    /// Overlays painting on `frame`, bottom-to-top.
    pub fn overlays_on(&self, frame: usize) -> impl Iterator<Item = &Overlay> {
        self.overlays
            .iter()
            .filter(move |o| !o.hidden && o.range.contains(&frame))
    }

    pub fn add_overlay(
        &mut self,
        name: impl Into<String>,
        kind: OverlayKind,
        transform: Transform,
        range: Range<usize>,
    ) -> OverlayId {
        let id = OverlayId(self.next_id);
        self.next_id += 1;
        self.overlays.push(Overlay {
            id,
            name: name.into(),
            kind,
            range,
            transform,
            opacity: 1.0,
            hidden: false,
        });
        id
    }

    pub fn remove_overlay(&mut self, id: OverlayId) -> Option<Overlay> {
        let i = self.overlays.iter().position(|o| o.id == id)?;
        Some(self.overlays.remove(i))
    }

    /// Raise or lower one step in z-order.
    pub fn move_overlay_z(&mut self, id: OverlayId, up: bool) {
        let Some(i) = self.overlays.iter().position(|o| o.id == id) else {
            return;
        };
        let j = if up { i + 1 } else { i.wrapping_sub(1) };
        if j < self.overlays.len() {
            self.overlays.swap(i, j);
        }
    }

    /// Apply `f` to the overlay on `frames` alone, splitting it around the
    /// rest of its range. An overlay carries one transform for its whole
    /// range, so an edit that covers only part of it has to cut it in two:
    /// the edited frames become their own overlay with its own transform, and
    /// the original keeps the rest. All pieces take the original's place in
    /// the z-order, so nothing under or over the overlay moves relative to
    /// them.
    ///
    /// Returns the id of the overlay the edit landed on — the original's when
    /// it covered everything, a new one otherwise — and how many frames the
    /// edit touched. An empty intersection changes nothing.
    pub fn edit_on_frames(
        &mut self,
        id: OverlayId,
        frames: Range<usize>,
        f: impl FnOnce(&mut Overlay),
    ) -> (OverlayId, usize) {
        let Some(i) = self.overlays.iter().position(|o| o.id == id) else {
            return (id, 0);
        };
        let full = self.overlays[i].range.clone();
        let edit = full.start.max(frames.start)..full.end.min(frames.end);
        if edit.is_empty() {
            return (id, 0);
        }
        if edit == full {
            f(&mut self.overlays[i]);
            return (id, edit.len());
        }
        let (before, after) = (full.start..edit.start, edit.end..full.end);
        let mut piece = self.overlays[i].clone();
        piece.id = OverlayId(self.next_id);
        self.next_id += 1;
        piece.range = edit.clone();
        f(&mut piece);
        let edited = piece.id;
        if before.is_empty() {
            // The edit starts the range: the original keeps only the tail.
            self.overlays[i].range = after;
            self.overlays.insert(i, piece);
        } else {
            self.overlays[i].range = before;
            if after.is_empty() {
                self.overlays.insert(i + 1, piece);
            } else {
                // The edit sits in the middle: the right half is its own too.
                let mut rest = self.overlays[i].clone();
                rest.id = OverlayId(self.next_id);
                self.next_id += 1;
                rest.range = after;
                self.overlays.insert(i + 1, piece);
                self.overlays.insert(i + 2, rest);
            }
        }
        (edited, edit.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    pub fn blank(w: u32, h: u32, n: usize) -> Document {
        Document::from_frames(
            (0..n)
                .map(|_| Frame::new(RgbaImage::new(w, h), 10))
                .collect(),
        )
    }

    /// Regression: the thumbnail and the opacity flag are built once, where the
    /// frame is decoded. Recomputing either per view update froze the UI for
    /// seconds on a few hundred frames.
    #[test]
    fn a_frame_caches_its_thumbnail_and_its_opacity() {
        let opaque = Frame::new(RgbaImage::from_pixel(1920, 1080, Rgba([9, 9, 9, 255])), 4);
        assert_eq!(opaque.thumb.width(), THUMB_W);
        let want = THUMB_W as f32 * 1080.0 / 1920.0;
        assert!(
            (opaque.thumb.height() as f32 - want).abs() <= 0.5,
            "aspect preserved: {} vs {want}",
            opaque.thumb.height()
        );
        assert!(opaque.opaque);

        let holed = Frame::new(RgbaImage::from_pixel(64, 64, Rgba([9, 9, 9, 200])), 4);
        assert!(!holed.opaque);
    }

    /// Regression: `has_alpha` used to scan every pixel of every frame on every
    /// view update, playback ticks included.
    #[test]
    fn has_alpha_reads_the_per_frame_flag() {
        let mut doc = blank(8, 8, 3);
        assert!(doc.has_alpha(), "a blank RgbaImage is transparent");

        doc.frames = (0..3)
            .map(|_| Frame::new(RgbaImage::from_pixel(8, 8, Rgba([1, 2, 3, 255])), 5))
            .collect();
        assert!(!doc.has_alpha());

        doc.frames[1] = Frame::new(RgbaImage::from_pixel(8, 8, Rgba([1, 2, 3, 254])), 5);
        assert!(doc.has_alpha(), "one nearly-opaque pixel still counts");
    }

    /// Regression: `key` and `thumb` are bookkeeping, so they must stay out of
    /// equality or history round-trips start reporting spurious changes.
    #[test]
    fn frames_with_the_same_pixels_are_equal_despite_distinct_keys() {
        let pixels = RgbaImage::from_pixel(4, 4, Rgba([7, 7, 7, 255]));
        let a = Frame::new(pixels.clone(), 5);
        let b = Frame::new(pixels, 5);
        assert_ne!(a.key, b.key, "separately built frames are distinguishable");
        assert_eq!(a, b);
        assert_eq!(a.clone().key, a.key, "a clone is the same frame");
    }

    #[test]
    fn scope_resolves_and_clamps() {
        let doc = blank(4, 4, 10);
        let n = doc.frames.len();
        assert_eq!(Scope::ThisFrame.resolve(3, n), vec![3]);
        assert_eq!(Scope::AllFrames.resolve(3, n), (0..10).collect::<Vec<_>>());
        assert_eq!(Scope::ThisFrame.resolve(99, n), Vec::<usize>::new());
    }

    /// Ctrl+click builds a set, not a run, so the scope has to survive gaps,
    /// repeats and an out-of-order click order.
    #[test]
    fn a_gappy_selection_keeps_its_gaps_but_spans_them_for_an_overlay() {
        let n = blank(4, 4, 10).frames.len();
        let picked = Scope::Frames(vec![7, 2, 2, 40]);
        assert_eq!(
            picked.resolve(0, n),
            vec![2, 7],
            "sorted, deduplicated, clamped"
        );
        assert_eq!(picked.span(0, n), 2..8);
        assert_eq!(Scope::Frames(Vec::new()).span(0, n), 0..0);
    }

    #[test]
    fn a_rotated_box_round_trips_through_its_own_frame() {
        let mut t = Transform::at(10.0, 20.0, 100.0, 40.0);
        t.angle = 0.7;
        let (ix, iy) = t.to_image(10.0, 20.0);
        let (lx, ly) = t.to_local(ix, iy);
        assert!(
            (lx - 10.0).abs() < 0.001 && (ly - 20.0).abs() < 0.001,
            "{lx}, {ly}"
        );
        // The centre is the one point rotation leaves alone.
        let (cx, cy) = t.center();
        let spun = t.to_image(cx, cy);
        assert!((spun.0 - cx).abs() < 0.001 && (spun.1 - cy).abs() < 0.001);
    }

    #[test]
    fn overlay_range_gates_frames() {
        let mut doc = blank(4, 4, 10);
        doc.add_overlay(
            "caption",
            OverlayKind::Text(TextOverlay::default()),
            Transform::at(0.0, 0.0, 10.0, 10.0),
            2..5,
        );
        assert_eq!(doc.overlays_on(1).count(), 0);
        assert_eq!(doc.overlays_on(2).count(), 1);
        assert_eq!(doc.overlays_on(4).count(), 1);
        assert_eq!(doc.overlays_on(5).count(), 0);
    }

    #[test]
    fn z_order_is_list_order() {
        let mut doc = blank(4, 4, 2);
        let a = doc.add_overlay("a", shape(), Transform::at(0.0, 0.0, 1.0, 1.0), 0..2);
        let b = doc.add_overlay("b", shape(), Transform::at(0.0, 0.0, 1.0, 1.0), 0..2);
        doc.move_overlay_z(a, true);
        assert_eq!(
            doc.overlays.iter().map(|o| o.id).collect::<Vec<_>>(),
            vec![b, a]
        );
        doc.move_overlay_z(b, false); // already at the bottom, no-op
        assert_eq!(
            doc.overlays.iter().map(|o| o.id).collect::<Vec<_>>(),
            vec![b, a]
        );
    }

    /// One overlay carries one transform for its whole range, so an edit
    /// scoped to part of that range must split it: the edited frames become
    /// their own overlay, the rest keeps the old transform, and the pieces
    /// take the original's z-order slot.
    #[test]
    fn editing_part_of_a_range_splits_the_overlay() {
        let mut doc = blank(4, 4, 10);
        doc.add_overlay("under", shape(), Transform::at(0.0, 0.0, 1.0, 1.0), 0..10);
        let id = doc.add_overlay("a", shape(), Transform::at(1.0, 1.0, 2.0, 2.0), 0..10);
        doc.add_overlay("over", shape(), Transform::at(0.0, 0.0, 1.0, 1.0), 0..10);
        let moved = Transform::at(9.0, 9.0, 2.0, 2.0);

        let (edited, touched) = doc.edit_on_frames(id, 4..6, |o| o.transform = moved);
        assert_eq!(touched, 2);
        assert_ne!(edited, id, "the edited frames are their own overlay now");

        let shapes: Vec<_> = doc
            .overlays
            .iter()
            .map(|o| (o.name.as_str(), o.range.clone(), o.transform))
            .collect();
        assert_eq!(
            shapes,
            vec![
                ("under", 0..10, Transform::at(0.0, 0.0, 1.0, 1.0)),
                ("a", 0..4, Transform::at(1.0, 1.0, 2.0, 2.0)),
                ("a", 4..6, moved),
                ("a", 6..10, Transform::at(1.0, 1.0, 2.0, 2.0)),
                ("over", 0..10, Transform::at(0.0, 0.0, 1.0, 1.0)),
            ]
        );
        assert_eq!(doc.overlay(edited).map(|o| o.range.clone()), Some(4..6));
    }

    /// An edit at either end of the range leaves one neighbour, not a hole.
    #[test]
    fn editing_the_range_edges_leaves_two_pieces() {
        let moved = Transform::at(9.0, 9.0, 2.0, 2.0);
        let kept = Transform::at(1.0, 1.0, 2.0, 2.0);

        let mut doc = blank(4, 4, 10);
        let id = doc.add_overlay("a", shape(), kept, 2..8);
        let (edited, touched) = doc.edit_on_frames(id, 0..3, |o| o.transform = moved);
        assert_ne!(edited, id);
        assert_eq!((touched, doc.overlays.len()), (1, 2));
        assert_eq!(shapes(&doc), vec![("a", 2..3, moved), ("a", 3..8, kept)]);

        let mut doc = blank(4, 4, 10);
        let id = doc.add_overlay("a", shape(), kept, 2..8);
        let (edited, touched) = doc.edit_on_frames(id, 6..9, |o| o.transform = moved);
        assert_ne!(edited, id);
        assert_eq!((touched, doc.overlays.len()), (2, 2));
        assert_eq!(shapes(&doc), vec![("a", 2..6, kept), ("a", 6..8, moved)]);
    }

    /// The scope covering everything the overlay covers is an in-place edit.
    #[test]
    fn an_edit_over_the_whole_range_stays_in_place() {
        let mut doc = blank(4, 4, 10);
        let id = doc.add_overlay("a", shape(), Transform::at(1.0, 1.0, 2.0, 2.0), 2..8);
        let moved = Transform::at(9.0, 9.0, 2.0, 2.0);

        let (edited, touched) = doc.edit_on_frames(id, 0..10, |o| o.transform = moved);
        assert_eq!((edited, touched), (id, 6));
        assert_eq!(doc.overlays.len(), 1);
        assert_eq!(doc.overlay(id).map(|o| o.transform), Some(moved));
    }

    /// Frames outside the overlay are not the overlay's business.
    #[test]
    fn an_edit_outside_the_range_changes_nothing() {
        let mut doc = blank(4, 4, 10);
        let id = doc.add_overlay("a", shape(), Transform::at(1.0, 1.0, 2.0, 2.0), 2..8);
        let before = doc.clone();

        let (edited, touched) = doc.edit_on_frames(id, 9..11, |o| {
            o.transform = Transform::at(9.0, 9.0, 2.0, 2.0)
        });
        assert_eq!((edited, touched), (id, 0));
        assert_eq!(doc, before, "no intersection, no edit");
    }

    fn shape() -> OverlayKind {
        OverlayKind::Shape(ShapeOverlay {
            shape: Shape::Rect,
            fill: Some([255, 0, 0, 255]),
            stroke: None,
        })
    }

    fn shapes(doc: &Document) -> Vec<(&str, Range<usize>, Transform)> {
        doc.overlays
            .iter()
            .map(|o| (o.name.as_str(), o.range.clone(), o.transform))
            .collect()
    }
}
