//! Undo as a pure state machine over the model.
//!
//! Every edit snapshots the document. That is only affordable because frame
//! pixels are `Arc`-shared, so a snapshot of 300 frames is 300 pointer copies
//! plus the overlay list. Nothing is ever a raster diff, which is what makes
//! stepping to the first history item rebuild everything exactly.

use super::model::Document;

const LIMIT: usize = 200;

/// What an edit did, for the toast that names the scope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub label: String,
    pub frames_touched: usize,
}

impl Change {
    /// "Text added on 24 frames", or just the label when only one frame moved.
    ///
    /// Translated here rather than at the toast: the label is a marked literal
    /// (`i18n::n`) chosen where the edit happens, and this is the only place
    /// that knows whether the count belongs in the sentence.
    pub fn message(&self) -> String {
        let label = crate::i18n::lookup(&self.label);
        if self.frames_touched > 1 {
            crate::i18n::fill(
                // Translators: Toast after an edit. {change} is a past-tense edit name such as "Frames deleted".
                crate::i18n::t("{change} on {count} frames"),
                &[
                    ("change", &label),
                    ("count", &self.frames_touched.to_string()),
                ],
            )
        } else {
            label
        }
    }
}

pub struct Editor {
    pub doc: Document,
    past: Vec<(Change, Document)>,
    future: Vec<(Change, Document)>,
}

impl Editor {
    pub fn new(doc: Document) -> Self {
        Editor {
            doc,
            past: Vec::new(),
            future: Vec::new(),
        }
    }

    /// Run `f` against the document as one tracked step. `frames_touched` is
    /// what the toast reports; a wide edit that stays silent is the wrong
    /// feedback.
    pub fn edit<T>(
        &mut self,
        label: impl Into<String>,
        frames_touched: usize,
        f: impl FnOnce(&mut Document) -> T,
    ) -> (Change, T) {
        let change = Change {
            label: label.into(),
            frames_touched,
        };
        self.past.push((change.clone(), self.doc.clone()));
        if self.past.len() > LIMIT {
            self.past.remove(0);
        }
        self.future.clear();
        let out = f(&mut self.doc);
        (change, out)
    }

    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }

    /// Label of the step undo would reverse, for the button tooltip.
    pub fn undo_label(&self) -> Option<&str> {
        self.past.last().map(|(c, _)| c.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.future.last().map(|(c, _)| c.label.as_str())
    }

    pub fn undo(&mut self) -> bool {
        let Some((change, doc)) = self.past.pop() else {
            return false;
        };
        let current = std::mem::replace(&mut self.doc, doc);
        self.future.push((change, current));
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some((change, doc)) = self.future.pop() else {
            return false;
        };
        let current = std::mem::replace(&mut self.doc, doc);
        self.past.push((change, current));
        true
    }
}

#[cfg(test)]
mod tests {
    use image::RgbaImage;

    use super::*;
    use crate::core::model::{Frame, OverlayKind, Shape, ShapeOverlay, TextOverlay, Transform};

    /// Deterministic noise; a real generator is a dependency for no gain here.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self, n: usize) -> usize {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (self.0 >> 33) as usize % n.max(1)
        }
    }

    fn seeded() -> Editor {
        let doc = Document::from_frames(
            (0..24)
                .map(|i| Frame::new(RgbaImage::new(4, 4), 3 + (i % 2) as u16))
                .collect(),
        );
        Editor::new(doc)
    }

    fn shape() -> OverlayKind {
        OverlayKind::Shape(ShapeOverlay {
            shape: Shape::Rect,
            fill: Some([9, 9, 9, 255]),
            stroke: None,
        })
    }

    /// Run a random script of edits, then assert the document is a pure
    /// function of the history position, in both directions and repeatedly.
    #[test]
    fn history_round_trips_losslessly() {
        let mut ed = seeded();
        let initial = ed.doc.clone();
        let mut rng = Lcg(0x5eed);

        for step in 0..60 {
            let n = ed.doc.frames.len().max(1);
            match rng.next(7) {
                0 => {
                    let start = rng.next(n);
                    let end = (start + 1 + rng.next(4)).min(n);
                    ed.edit("Add text", end - start, |d| {
                        d.add_overlay(
                            format!("text {step}"),
                            OverlayKind::Text(TextOverlay {
                                text: format!("caption {step}"),
                                ..Default::default()
                            }),
                            Transform::at(rng_f(step), 4.0, 20.0, 8.0),
                            start..end,
                        );
                    });
                }
                1 => {
                    ed.edit("Add shape", n, |d| {
                        d.add_overlay("rect", shape(), Transform::at(1.0, 1.0, 3.0, 3.0), 0..n);
                    });
                }
                2 => {
                    if let Some(id) = ed.doc.overlays.first().map(|o| o.id) {
                        ed.edit("Delete overlay", 1, |d| {
                            d.remove_overlay(id);
                        });
                    }
                }
                3 => {
                    if let Some(id) = ed.doc.overlays.last().map(|o| o.id) {
                        ed.edit("Lower overlay", 1, |d| d.move_overlay_z(id, false));
                    }
                }
                4 if n > 2 => {
                    let start = rng.next(n - 1);
                    ed.edit("Delete frames", 1, |d| d.delete_frames(start..start + 1));
                }
                5 => {
                    let i = rng.next(n);
                    ed.edit("Duplicate frame", 1, |d| d.duplicate_frame(i));
                }
                _ => {
                    ed.edit("Set delay", n, |d| d.set_delay(0..n, 4 + (step % 3) as u16));
                }
            }
        }

        let final_doc = ed.doc.clone();
        let depth = {
            let mut depth = 0;
            while ed.undo() {
                depth += 1;
            }
            depth
        };
        assert!(depth > 20, "the script should have produced real history");
        assert_eq!(
            ed.doc, initial,
            "stepping to the first item rebuilds everything"
        );

        while ed.redo() {}
        assert_eq!(
            ed.doc, final_doc,
            "redo to the end lands on the same document"
        );

        // forward → back → forward is identical, repeatedly
        for _ in 0..3 {
            let mut marks = Vec::new();
            while ed.undo() {
                marks.push(ed.doc.clone());
            }
            for expected in marks.iter().rev().skip(1) {
                assert!(ed.redo());
                assert_eq!(&ed.doc, expected);
            }
            while ed.redo() {}
            assert_eq!(ed.doc, final_doc);
        }
    }

    fn rng_f(step: usize) -> f32 {
        (step % 17) as f32
    }

    #[test]
    fn a_new_edit_clears_the_redo_branch() {
        let mut ed = seeded();
        ed.edit("Add shape", 1, |d| {
            d.add_overlay("a", shape(), Transform::at(0., 0., 1., 1.), 0..1);
        });
        ed.undo();
        assert!(ed.can_redo());
        ed.edit("Add shape", 1, |d| {
            d.add_overlay("b", shape(), Transform::at(0., 0., 1., 1.), 0..1);
        });
        assert!(!ed.can_redo());
        assert_eq!(ed.doc.overlays.len(), 1);
    }

    #[test]
    fn toast_names_the_scope() {
        let c = Change {
            label: "Text added".into(),
            frames_touched: 24,
        };
        assert_eq!(c.message(), "Text added on 24 frames");
        let c = Change {
            label: "Text added".into(),
            frames_touched: 1,
        };
        assert_eq!(c.message(), "Text added");
    }
}
