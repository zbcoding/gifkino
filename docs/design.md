# Design

A GIF editor for Linux. GTK4 + Rust, distributed as a flatpak and as an
AppImage.

This public document records the decisions and the reasoning behind them, so that a
decision can be reversed deliberately rather than drifted away from.

Since this document is an overview of the design and this document is public, the focus is on the general structure of this Gifkino software and is intentionally simpler than a detailed writing.

## What this is

Two jobs in one app:

1. **Import** any video or an existing GIF as a frame list.
2. **Edit** the frame list along the time axis, then export an optimized GIF.

Editing means overlays (text, shapes, images), effects, transforms, crop, resize, and
frame-list operations. It does not mean advanced painting. See
[External editor handoff](#external-editor-handoff).

## The name

Gifkino is a German compound built the way `Daumenkino` is — thumb-cinema, the
word for a flipbook — so it reads as "GIF cinema", a place where GIFs are made,
rather than as a kind of file. Leading with "gif" puts the searchable half
first. The app ID is `io.github.zbcoding.Gifkino`.

## Document model

```rust
struct Document {
    frames: Vec<Frame>,        // RGBA + delay, in centiseconds
    overlays: Vec<Overlay>,    // live, re-editable, z-ordered
}

struct Frame {
    pixels: RgbaImage,
    delay_cs: u16,
    detached: bool,            // see External editor handoff
}

struct Overlay {
    kind: OverlayKind,         // Text | Shape | Image
    range: Range<usize>,       // which frames this appears on
    transform: Mat3,
    z: u32,
    opacity: f32,
    hidden: bool,
}
```

The `range` field is what makes this an animation editor rather than a stack of
unrelated pictures. A caption is one overlay with `range: 10..40`; you edit it
once and thirty frames change.

### The invariant

A frame's composited output is a pure function of its pixels plus the overlays
whose range contains it. Nothing is ever baked in as a side effect.

The rule is what makes undo lossless rather than a raster diff: a surface
equals the render of the object list that produced it. Preserve it and undo is
a pure state machine
over the model: stepping to the first history item rebuilds everything, and
forward-back-forward lands on an identical document.

The acceptance test for history is that property, not a list of scenarios. Build
N overlays, apply a random edit script, then assert document equality across
repeated walks in both directions.

### Edit scope

One toolbar control gates every operation: `This frame`, `All frames`, or
`Range`. The first two are always available; `Range` exists only while the
frame strip has a selection to bind to.

- Overlay tools take their `range` from it at creation.
- Overlay edits — a drag, a restyle, a text change — land inside the scope
  too. When the scope covers only part of an overlay's range, the overlay
  splits: the edited frames become their own overlay with its own transform,
  and the rest keeps what it had. One transform per contiguous range is the
  price of the model, and the split is how a one-frame edit stays one frame.
- Frame-list and raster operations apply to the selected frames, wrapped in a
  single compound history item.
- Crop, resize, and flatten default to All frames, because anything else
  corrupts the animation.
- Any edit that touched more than the current frame reports how many frames it
  changed, with Undo in the same toast.

This control is the product. Its treatment in the
window is under [Scope and strip](#scope-and-strip).

## Stack

Rust, gtk4-rs, libadwaita, Relm4. Flatpak on the GNOME runtime.

Two builds ship, Flatpak and Appimage. 

The flatpak carries the GNOME runtime, so which GTK it gets can change. 
The AppImage cannot carry glibc, so its build host matters. Ubuntu 24.04 has GTK 4.14, the baseline.
Both builds also carry `ffmpeg`, `ffprobe` and `gifsicle`, since
the GNOME runtime ships the libav* libraries but not the programs.
Ubuntu does not install them at all, and `pipeline/video.rs` drives the programs over pipes. One
crate to start, with `core/`, `pipeline/` and `ui/` modules.

### Detachment

A frame is exported composited, which is what the user expects to see in the
external editor. On return it is marked `detached`: overlays whose range covers
it skip it from then on.

The tradeoff is real and needs to be visible in the UI. Retyping a caption later
will not update a detached frame, because that frame's pixels are now just
pixels. Show a badge on detached frames in the strip. A destructive touch
rasterizes, and the badge is what says so.

### The line to hold

The value of this split is that no paint engine is ever written. If a brush
starts to look necessary "just for quick touch-ups", the design has failed and
the choice is to either commit to a paint engine or lean harder on the handoff.
Text, shapes, overlay images, transform handles, crop, and resize are the whole
feature set, and each is time-aware in a way a paint tool is not.

## Pipeline

```
import  → ffmpeg → raw RGBA + delays
edit    → overlay model + external handoff
play    → frame strip doubles as scrubber
export  → NeuQuant → gif crate → gifsicle -O3
```

### Recording, and why it is not here

Not built, and not planned. Every desktop already has a screen recorder that
writes an mp4, and this app imports mp4, webm and animated GIF, resizing a
large capture down on the way in. Building a second recorder buys a user nothing they cannot
already do, and it costs a ScreenCast portal handshake, a PipeWire path, an X11
fallback, a capture-source probe and a setup sheet, all of it in the way of the
editing that is the actual product.

### Import

Decode with ffmpeg to raw RGBA over a pipe (`-f rawvideo -pix_fmt rgba -`),
sized from an `ffprobe` call first. No temp PNG sequence.

Existing GIFs are decoded with the `gif` crate, not ffmpeg. ffmpeg normalizes
toward constant frame rate and discards per-frame delays and disposal methods,
which is exactly the data an editor has to preserve.

Cap import resolution and warn on high frame counts.

GIF delays are stored in centiseconds. 30fps is 3.33cs, which is not
representable, so rounding gives 3cs and a 33.3fps animation that drifts against
the source. Either snap the import framerate to a value that divides evenly (10, 20, 25, 50)
or distribute the remainder across frames.

Browsers also clamp delays below 2cs up to 10cs, so GIFs effectively don't go higher than 50 fps.

### Export

ffmpeg cannot write the GIF. Its GIF output is effectively constant frame rate,
which discards the per-frame delays. Instead:

1. RGBA frames plus per-frame delays
2. `color_quant::NeuQuant` for a global palette
3. `gif` crate writes frames with exact delays
4. `gifsicle -O3 --lossy` post-pass

## Playback

The frame strip is the scrubber. A GIF is short, thumbnails fit, and clicking a
thumbnail is seeking.

Playback reschedules a `glib::timeout_add_local` per frame using that frame's
delay, rather than running a fixed tick with an accumulator.

## The interface

### Main window

```
┌────────────────────────────────────────────────────────────────────┐
│ Undo Redo               foo.gif                       Export   ☰   │
│                     24 frames · 3.0 s                              │
├───┬──────────────────────────────────────────────┬─────────────────┤
│ V │                                              │ Properties      │
│ T │                                              │                 │
│ R │              Canvas                          │  contextual     │
│ O │    (checkerboard only under real alpha)      │  page for the   │
│ A │        [ −  100%  +  Fit ]                   │  current        │
│ I │                                              │  selection      │
│ C │                                              │                 │
├───┴──────────────────────────────────────────────┴─────────────────┤
│ ▶  00:01.4 / 00:03.0 · 20 fps        Scope [ This frame | All ]    │
│ ├─◉ text "bug" ────────┤      ├─◉ arrow ─┤        overlay bands    │
│ [1][2][3][▓12▓][▓13▓][▓14▓][15]…    seek · select · badges         │
└────────────────────────────────────────────────────────────────────┘
```

The left rail is Select, Text, Rect, Ellipse, Arrow, Image, Crop. There is no
dedicated zoom or pan tool: pan is middle-drag, zoom is Ctrl+wheel plus the
chip. The canvas opens fit-to-window and carries a hairline border, without
which a white frame bleeds into the light theme background.

Transparency gets a checkerboard only when the document actually has
transparent pixels. GIF alpha is one bit and screen captures are fully opaque,
so a permanent checkerboard would be noise behind the dominant input. The
detection is a lazy re-check whenever frame pixels change, not a one-time
import flag: the external editor handoff is a core path, and a frame saved
back from an external editor as a detached frame can introduce alpha long
after import. The check is a linear scan and costs microseconds at these
sizes.

Transform handles anchor on the opposite corner, Shift constrains to the source
aspect ratio rather than to a square, and Ctrl re-centers, with angle plus
un-rotated rect tracked from the first commit.
Dragging snaps to center x/y and canvas edges at roughly 4 px.

Alt+drag rotates, Shift snaps to 32 steps of a turn, and the modifiers are
rebindable through `keymap::Modal`. `contains()`, `handle_at()` and the outline
all read the pointer through `Transform::to_local`, so hit-testing is done in
the overlay's own space rather than against a screen-space bounding box. The
rotate cursor is a 32 px PNG embedded in the binary: GTK has
no CSS cursor name for rotation and `gdk::Texture` reads PNG but not SVG, so
editing the glyph means re-running `rsvg-convert` (`resources/README.md`). A
build that cannot decode it falls back to `grab`.

Four corner grips, no edge grips yet. Widening a caption without changing its
height still takes a corner drag with Shift off.

The right panel lives in an `AdwOverlaySplitView` with an `AdwBreakpoint` near
900 px, so it collapses to an overlay and gives the canvas the width. Strip
thumbnail size is Ctrl+wheel over the strip; at one fixed size a 300-frame
import is a long horizontal scroll with no overview of it.

Canvas and footer sit in a `GtkPaned`. The footer's height budget is the
transport row, the scope row, the band rows, and the thumbnails, and band
overflow is an open decision below; a draggable divider is one widget now
versus a retrofit after the first real document proves it necessary.

### Scope and strip

The scope control gets one home: directly above the frame strip, because in
Range mode the strip selection is the operand and the two must read as one
unit.

Scope is binary until a range exists. `This frame` and `All frames` are always
present; a third `Range 12–31` segment appears when the strip has a selection,
takes focus at that moment, and disappears when the selection clears. A
permanently disabled segment that can only be reached sideways teaches nobody
anything.

When the selection clears and Range collapses, or a seek lands on one frame,
scope reverts to `This frame` — the least destructive default for the most
scope-sensitive operation, overlay creation — and the accent tint makes the
revert visible rather than silent.

Sticky scope is where this app can hurt someone. Creating a caption under
`All frames` while thinking `This frame` is the mistake people will actually
make, and undo is the only recovery. Two guards, both cheap:

- The scope chip and the strip share one accent, taken from
  `AdwStyleManager` so it follows the user's system accent. `All frames` tints
  every thumbnail's top edge, `This frame` tints only the playhead, and Range
  draws one continuous bar across the selected span with the `Range 12–31`
  label as that bar's left cap. The operand is never a guess.
- The toast after a scope-wide edit names the scope: "Text added to 24 frames ·
  Undo". Silence after an edit that touched everything is the wrong feedback.

One motion, and only this one: while an overlay's geometry is changing under a
wide scope — dragged out, moved, or resized — ghost it live on the neighboring
thumbnails. Moving an existing all-frames caption has the same mismatch as
creating one, so the trigger is any live geometry change, not just drag-out.
It shows the whole premise of the model in the half second before the user
commits, the only animation here that clarifies state rather than decorating
it. The implementation is stamped, not composited: the overlay's transform is
scaled into cached thumbnail pixbufs, visible thumbnails only. Running the
full composite pipeline per motion tick is how this feature earns blame for
jank it did not cause.

#### The strip is the layer list

Overlay bands are stacked above the thumbnails, one row per overlay in z-order,
each band spanning the frames its range covers. The band carries the overlay's
name and an eye toggle in its left cap; dragging an end changes the range,
dragging the body moves it, and clicking selects the overlay. There is no
separate overlay list in the right panel, because that would list every object
twice.

A band click also seeks. The canvas only ever shows the playhead frame, and a
selected overlay can sit outside it; clicking a band moves the playhead to the
range's first frame so the properties panel never edits something the canvas
does not show.

Right-clicking a band acts on that overlay: delete it, or copy it onto frames
it does not cover. The copy has one item, aimed at whatever the strip is
already saying — the frames in scope when more than one is picked, the whole
document otherwise — because "copy this to there" needs no dialog once "there"
is already on screen. A gappy selection gets a piece per run, since an overlay
carries one contiguous range; a piece landing against the original folds back
into it, so copying onto the frame next door widens the band rather than
stacking a second one on top of it.

This is a timeline in shape, but not a second widget to keep in sync — it is
one more band on the strip that already exists. It is also the clearest picture
of the whole model, so it is what a first screenshot should show.

Underneath, the strip still does its other jobs: clicking a thumbnail seeks,
marquee-drag extends the selection, detached frames carry the badge, hover
shows the per-frame delay, drag reorders, right-click opens the frame menu, and
playback autoscrolls.

The selection is a `Vec<usize>`, not a range. Ctrl+click toggles one frame,
Shift+click takes the run from the anchor. Delete and duplicate act on the set;
reverse acts on what the set spans, because reversing a gappy set means
nothing. An overlay still carries one contiguous range, so adding one under a
gappy selection widens to `Scope::span`.

### Properties panel

One contextual page at a time, and properties only now that overlays are listed
on the strip. Overlay selected: kind-specific fields, opacity, raise/lower, and
a Range pair of spinners with a "set from strip selection" button. Frames
selected: count, one delay field applying to the whole selection, delete, and
"Edit frame in…". Nothing selected: document properties — size, resize, crop,
duration, frame count. Build each as an `AdwPreferencesGroup` so the rows come
out consistent for free.

### Export dialog

An AdwDialog with size chips (100% / 480w / 640w / 800w / custom width), speed
(25/50/100/200%, a delay rescale), color count (256/128/64) with a dither
toggle, and loop (forever / N / once).

The size readout runs the real pipeline — NeuQuant, gif crate, gifsicle -O3 —
into memory, debounced ~300 ms after any change, on a worker thread with a
spinner in the readout slot. No amount of debouncing makes a 300-frame encode
safe on the main loop. The comparison reads `2.4 MB → 840 KB`, where the left
side is the source artifact — the mp4 or the GIF that was imported — and stays
fixed while the settings move. A last-encode-versus-new-encode delta churns on
every tweak and reads as noise; the source size is the number "what does this
cost me" is actually asking.

Color count and dither are the two settings nobody can judge from a number, so
they preview on the current frame. The preview quantizes against the same
global-palette path as the export — sampled from a subsample of frames — never
a one-frame local palette, which would flatter the result the export then
fails to match. The dialog warns when the minimum delay falls below 2cs.

### Frame operations

Strip context menu plus a … button in the strip corner: Delete, Duplicate
(freeze-frame), Reverse, "Reduce frame rate…" (dropping every Nth frame with
the delay compensation described under Optimizations), Move delay, and "Edit
frame in…". Nothing else in v1.

Labels stay on the user's side of the screen. "Drop every Nth" is the
implementation talking. The export verb holds steady end to end: the button
says Export, the dialog is titled Export GIF, and the toast says "Exported to
~/Videos/foo.gif · Show in Files".

### States

Every long or failing path needs a screen, and these are the ones that exist.

Import and export both show a cancelable progress page, not a spinner. A
30-second mp4 takes real time to decode, and an indeterminate spinner with no
Cancel is indistinguishable from a hang.

Compositing after a wide edit paints the playhead frame and the strip's visible
thumbnails first, then finishes the rest in the background. The user waits for
two frames instead of two hundred, and it is less plumbing than making a modal
busy state cancelable.

Capabilities are probed once at startup, not at the moment of use: whether
ffmpeg and ffprobe run, and whether gifsicle is present. A missing piece
disables the affected action with the reason attached to it. Discovering that
import is unavailable after picking a file is the worst possible time to find
out.

Failures say what happened and what to do, not an error code.

### Keyboard map

```
Space                play/pause
← →                  step one frame
Shift+← Shift+→      step ten frames
Home End             first/last frame
Ctrl+A               select all frames
Del                  delete the focused surface's selection
V T R O A I C        tools (suppressed while a text field has focus)
+ - F                zoom in / out / fit
Ctrl+Z               undo
Ctrl+Shift+Z Ctrl+Y  redo
Ctrl+E               export
Esc                  cancel tool / deselect
Ctrl+?               shortcuts window
```

`V T R O A I C` rather than an internally tidy scheme: V select, R rectangle,
O ellipse and T text are the letters most drawing tools already use, and muscle
memory beats internal consistency.

Del is bound to whichever surface has focus, since an overlay and a frame range
can both be selected at once. Canvas focus deletes the overlay, strip focus
deletes the frames. That makes a visible focus ring on both surfaces a
requirement rather than a nicety.

The shortcuts window is both the HIG-expected help overlay and the
documentation of this map.

The shortcuts controller runs in the capture phase, and has to: a focused
widget handles a key first, and GTK activates a focused button on Space.
`focus_owns_keys` is the entire exemption list, so a new widget that owns its
own keys — a search entry, an editable list row — has to be added there or it
loses them to the shortcuts. Every widget assertion in `ui::window` sits under
one `gtk_widget_regressions` test, because GTK is single-threaded and `cargo
test` is not; separate `#[test]`s segfault on the second `gtk::init`.

### Visual direction

Stock libadwaita: system light/dark, system accent, system fonts with tabular
figures for anything that counts. This is a tool, not a page — quiet, dense,
utilitarian — and HIG contrast, keyboard navigation, and HiDPI come with the
widgets rather than being rebuilt.

The custom work goes into one signature: the time-selection system, where the
scope control, the strip span, the overlay bands, and the tint on the affected
thumbnails read as one continuous object. Everything else stays plain so that
object is the thing people remember. Deliberately rejected: custom dark chrome
with a single neon accent, glassy cards, and any animation that does not
clarify state.

### Open decisions

- **Project save.** None in v1; a document is ephemeral and export is the
  deliverable. Consequence: overlays die with the window, so closing a
  document that has overlays must warn. A project format waits until someone
  loses work.
- **Overlay band overflow.** A document with a dozen overlays needs more band
  rows than the strip has height for. Scroll the band area, collapse to a
  single merged row past some count, or cap what is shown. Undecided until
  there is a real document to look at.
- **Clipboard paste-in.** `image/gif` clipboard support on Linux is spotty;
  deferred.

## Optimizations

The size of a GIF is determined mostly by inter-frame differencing: encoding
only changed pixels and leaving the rest transparent. `gifsicle -O3` has done
this well for twenty years, and lossy compression and color reduction are
`--lossy=80` and `--colors 128`. Calling it as a subprocess keeps its GPL-2 away
from this codebase.

What has to be written here is the frame-list math:

```rust
/// Delete every Nth frame, adding each dropped frame's delay to its
/// predecessor so total duration is preserved.
fn drop_every_nth(frames: &mut Vec<Frame>, n: usize)
```

Deleting every other frame without compensating delays plays the result at
double speed. That is the bug in every naive implementation, it is pure list
logic, and one assert-based test covers it.

### Work that leaves the main thread

Resampling every frame — resize, and the zoom that refills the canvas from a
box — froze the window when it ran inside `update`. Both now run on a worker.
Each op is split into a pure producer (`resized_frames`, `zoomed_frames`) that
returns `Vec<(usize, Frame)>` and reports progress per frame, and a thin
mutator that keeps its old signature by calling the producer and swapping the
results in. `Frame.pixels` is an `Arc<RgbaImage>` and history already snapshots
the whole document per edit, so handing the worker a `doc.clone()` is pointer
copies and needs no borrowing tricks.

Results are keyed by frame index, which is the constraint everything else
follows from. While a worker is running, any message that would reorder or
resize the frame list is dropped at the top of `update` — that is what
`Msg::changes_frames()` enumerates, and it includes `SetFrameDelay`, since a
produced frame carries the delay from the snapshot it was built from. Overlay
edits stay live: scaling the *current* overlay list on completion preserves
tweaks made mid-work. Results are deliberately not rev-guarded, because a
canvas drag bumps `rev` on every motion event and a rev check would throw away
good work whenever the canvas was touched.

One progress bar in the top bar serves import, resize and zoom, in the welcome
state and with a document open. Export never owns it; an export finishing
mid-import used to hide the bar while the decode kept running. Crop is a copy
of the kept region rather than a resample, so it stays synchronous.

## Deferred, with reasons

- **Keyframed overlay motion.** Overlays are static across their range.
  Interpolated movement needs curves, a different history shape, and an editor
  UI. Add it when someone asks for moving text.
- **Per-frame render cost.** An edit currently rebuilds every frame the changed
  overlay covers. Restrict rebuilds to the intersecting range; an overlay
  spanning the whole document still costs a full walk. Painting the visible
  frames first (see States) hides the cost but does not remove it.
- **Disk-backed frame cache.** See the memory budget under Import.
- **Strip virtualization.** The strip rebuilds one widget per frame whenever the
  frame list changes. Fine at a few hundred frames, a hitch past that. Worth
  doing when someone imports something long enough to notice.
- **Per-frame crop that changes the canvas size.** A GIF has one canvas, so
  frames of different sizes are not something the model can hold. Crop is
  document-wide, and zoom scope covers the per-frame intent by refilling the
  canvas from a box. Building it for real needs a canvas size plus a per-frame
  offset in the model, and every op that touches pixels has to learn about
  both.
- **Oriented transform handles.** Handles that snap back to an axis-aligned
  bounding box after a rotation are a rewrite to fix, not a polish item, so
  angle plus un-rotated rect is tracked from the start. Cheap greenfield,
  expensive to retrofit.

## Translations

`po/` holds `en_US`, `de` and `ja`, all 166 strings, with the de and ja
msgstrs AI-drafted and flagged `#, fuzzy` for review. Another locale is a line
in `po/LINGUAS`, a `scripts/i18n.py merge`, and the msgstrs.

`scripts/i18n.py` extracts msgids from the arguments of `t(...)`, which has two
consequences worth knowing before touching UI strings. Each label needs its own
`t(...)` call — a `t(match …)` hides every arm from the template, and the next
merge treats the missing msgids as obsolete and drops them. And rustfmt moving
a long literal onto its own line under `t(` used to lose the msgid entirely;
`logical_lines` rejoins those now, with the shape asserted in `scripts/i18n.py
selftest`.

`tn()` is a two-form plural, not a Plural-Forms evaluator. That is right for
de, en and ja and wrong for Slavic locales; the first of those needs the
evaluator written, not another call site.

Error text from anyhow — `pipeline/`, ffmpeg failures — reaches toasts
untranslated. Those strings live in `Result` chains rather than in the UI
layer.

## Licensing

The main, free version of this software will be MIT.
If we use gifsicle, gifski, etc, AGPL, GPL, might require having a text copy of the license in our software repository for reference.

## Build order

Each step is verifiable before the next, and the risky parts come first as
tested logic rather than as UI.

1. Frame list, overlay model, and history, as pure logic with unit tests. The
   round-trip property test comes with it.
2. GIF decode and encode through the `gif` crate, verified headless by a
   round-trip that preserves delays and disposal.
3. ffmpeg video import to RGBA frames, verified headless.
4. A Relm4 window painting a decoded frame, plus the frame strip. These are the
   same afternoon.
5. Playback and scrubbing.
6. Overlays and transform handles on the canvas, against the logic from step 1.
7. Edit scope control, wired into overlay creation first.
8. Export path with NeuQuant and the gifsicle pass.
9. Frame-list optimizations.
10. External editor handoff.
