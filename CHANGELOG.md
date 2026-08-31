# Changelog

## Unreleased

### Canvas

- The picture's edge is drawn on the canvas, and the margin around it is dimmed.
  The widget is wider or taller than the frame whenever the aspect ratios
  differ, and an overlay dragged into that margin is clipped out of every frame;
  it used to vanish with nothing to say why.
- Selection handles follow Impasto: a blue dot with a white ring at each corner,
  at a constant size whatever the zoom, and a resize cursor that turns with the
  content.
- Alt+drag rotates an overlay, Shift keeps the aspect ratio, and Ctrl resizes
  from the center. Hit tests, corner drags and the outline all read the box in
  its own frame, so a rotated overlay is grabbed where it looks. Hovering a grip
  shows the three modifiers as a tooltip, built from the keymap.
- The rotate cursor is Impasto's own `rotate-handle.svg`, embedded as a 32px
  PNG. GTK has no CSS cursor name for rotation, so it travels as a texture; a
  build that cannot decode it falls back to `grab`.

### Timeline

- Frames sit above the overlay bands rather than below them.
- The band area takes no space until there is an overlay, then grows a row at a
  time to five rows and scrolls behind the expander after that. The canvas keeps
  whatever is left instead of a fixed split.

### Fixes

- Space toggles playback rather than adding an overlay. The window's key
  controller ran in the bubble phase, so whatever held the focus handled the key
  first — and GTK both activates a focused button on Space and parks the initial
  focus on the first focusable widget, which is the Text tool. The controller
  now runs in the capture phase, with text entries, text views and dialogs
  handed their keystrokes back. Toolbar buttons also no longer take focus on
  click, so Enter and the arrow keys keep meaning what the canvas means by them.

### Frame selection

- Ctrl+click adds or removes one frame anywhere in the strip, so a selection no
  longer has to be a run. Delete and duplicate act on the set; reverse acts on
  what it spans.
- Shift+click takes the run between the anchor and the clicked frame, in either
  direction. The anchor survives, so shift-clicking again re-measures from it.

### Keybindings

- `canvas-rotate`, `canvas-keep-aspect` and `canvas-from-center` join
  `keybindings.conf` and the shortcuts window, under a Canvas group that
  captures a held modifier rather than a chord.

### Translations

- German and Japanese drafts for the new strings, flagged `#, fuzzy` for review.
- `scripts/i18n.py` joins a marker call that rustfmt split from its literal.
  Seven msgids had dropped out of the template that way.
