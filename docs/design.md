# Design

A GIF editor for Linux. GTK4 + Rust, distributed as a flatpak and as an
AppImage.

This document records the decisions and the reasoning behind them, so that a
decision can be reversed deliberately rather than drifted away from.

## What this is

Two jobs in one app:

1. **Import** any video or an existing GIF as a frame list.
2. **Edit** the frame list along the time axis, then export an optimized GIF.

Recording a screen was the third, and is not built. See
[Recording, and why it is not here](#recording-and-why-it-is-not-here).

Editing means overlays (text, shapes, images), transforms, crop, resize, and
frame-list operations. It does not mean painting. See
[External editor handoff](#external-editor-handoff).

## The core idea

Every existing tool gets one of two halves wrong.

GIMP can open an animated GIF, but each frame is an independent image. Adding a
caption across thirty frames means typing it thirty times. Nothing an edit does
can span time, so the animation is a stack of unrelated pictures that happen to
play in order.

ScreenToGif gets the time axis right and has the frame-list operations people
actually want, but it is Windows-only and its editing model is raster-first.

GIFcurry is written in Haskell for linux, but doesn't have a good user interface,
many GIF features are missing, and simple edits of GIF frames beyond adding text are missing

The design goal here is one sentence: an edit knows which frames it applies to.
Everything below follows from that.

# Development boost
Use the code patterns from Impasto /home/yumeko/DataDisk/Code/ImpastoPaint-public (written in .net gtk4) to write some things here in GTK4 rs (rust), like simple image resizing or adding text

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

The `range` field is the whole difference from GIMP. A caption is one overlay
with `range: 10..40`; you edit it once and thirty frames change.

### The invariant

A frame's composited output is a pure function of its pixels plus the overlays
whose range contains it. Nothing is ever baked in as a side effect.

This is borrowed from Impasto's object-layer system, where the equivalent rule
(an object surface equals the render of its object list) is what makes undo
lossless rather than a raster diff. Preserve it and undo is a pure state machine
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

This control is the product. GIMP has no version of it. Its treatment in the
window is under [Scope and strip](#scope-and-strip).

## Stack

Rust, gtk4-rs, libadwaita, Relm4. Flatpak on the GNOME runtime.

GTK4 was not chosen for the canvas. There is no scene graph and no hit-testing;
a snapshot surface is all you get, and that is equally true of Iced, Slint, and
egui. What GTK4 gives you is everything around the canvas that would otherwise
have to be rebuilt: portal file dialogs, HiDPI, IME, accessibility, and a
flatpak runtime that already ships all of it. egui would make the canvas
somewhat easier and everything else worse. Tauri would drag WebKitGTK into the
flatpak for a canvas that can be painted directly.

Two builds ship. The flatpak carries the GNOME runtime, so which GTK it gets is
a free choice that can move forward on its own schedule. The AppImage cannot
carry glibc, so its build host sets the floor for every machine that runs it;
Ubuntu 24.04 is that host because its GTK 4.14 is exactly the baseline the
bindings are gated at in `Cargo.toml`. Building on anything newer would raise
the glibc requirement and buy nothing, which is how Impasto's AppImage ended up
needing Ubuntu 26.04.

Both builds carry `ffmpeg`, `ffprobe` and `gifsicle`. The GNOME runtime ships
the libav* libraries but not the programs, and `pipeline/video.rs` drives the
programs over pipes; Ubuntu does not install them at all by default. An app
whose import path is a subprocess has to bring the subprocess.

Start as one crate with `core/`, `pipeline/`, and `ui/` modules. Splitting into
separate crates buys nothing while there is one consumer; modules test just as
headlessly. Split when compile times or a second consumer make it worth doing.

## External editor handoff

The app owns time. It does not own a paint engine.

A frame's context menu offers "Edit frame in…", which writes the composited
frame to a temp PNG and hands it to the desktop:

```rust
let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(&frame_png)));
launcher.set_always_ask(true);
launcher.launch(Some(&window), gio::Cancellable::NONE, |_| {});
```

Going through the OpenURI portal rather than spawning a specific binary avoids
`--talk-name=org.freedesktop.Flatpak`, a broad permission Flathub reviewers push
back on. It also means the feature works with Impasto, GIMP, Krita, or whatever
the user already has, for less code than hardcoding one of them.

Watch the temp file with `notify` rather than waiting for process exit, so
someone who saves and keeps editing sees the preview update. Fall back to
reloading on window focus if the watch fails to start.

### Detachment

A frame is exported composited, which is what the user expects to see in the
external editor. On return it is marked `detached`: overlays whose range covers
it skip it from then on.

The tradeoff is real and needs to be visible in the UI. Retyping a caption later
will not update a detached frame, because that frame's pixels are now just
pixels. Show a badge on detached frames in the strip. These are the same
semantics as Impasto's rasterize-on-destructive-touch bridge.

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
writes an mp4 — GNOME's built-in one, OBS, wf-recorder, SimpleScreenRecorder —
and this app imports mp4, webm and animated GIF, resizing a large capture down
on the way in. Building a second recorder buys a user nothing they cannot
already do, and it costs a ScreenCast portal handshake, a PipeWire path, an X11
fallback, a capture-source probe and a setup sheet, all of it in the way of the
editing that is the actual product.

What the design would have been, kept because the reasoning survives the
decision: record to a video file and open it through the normal import path,
never capturing frames straight into the document — that is what makes
ScreenToGif memory-hungry, and it means maintaining two decode paths instead of
one. On Wayland, `ashpd` performs the ScreenCast portal handshake and returns a
PipeWire node id for ffmpeg's `pipewiregrab` source, with GStreamer's
`pipewiresrc` as the fallback; X11 is `-f x11grab`. Region selection would be
the portal's own picker rather than a custom overlay, because GNOME does not
permit arbitrary overlay surfaces, and cursor capture would be the portal's
`cursor_mode: embedded`.

Reversing this means restoring `Caps::record_blocker` and the capture-source
probe removed alongside it, adding `--socket=pipewire` to the manifest, and
putting `Screen Recorder` back in the desktop entry's `Keywords`.

### Import

Decode with ffmpeg to raw RGBA over a pipe (`-f rawvideo -pix_fmt rgba -`),
sized from an `ffprobe` call first. No temp PNG sequence.

Existing GIFs are decoded with the `gif` crate, not ffmpeg. ffmpeg normalizes
toward constant frame rate and discards per-frame delays and disposal methods,
which is exactly the data an editor has to preserve.

Cap import resolution and warn on high frame counts. Frames live in RAM as RGBA
so scrubbing is instant: 500×400 across 100 frames is 80 MB and fine, 1920×1080
across 300 frames is 2.5 GB and is not. A disk cache is worth writing when
someone hits the wall, not before.

### The centisecond problem

GIF delays are stored in centiseconds. 30fps is 3.33cs, which is not
representable, so rounding gives 3cs and a 33.3fps animation that drifts against
the source.

Either snap the import framerate to a value that divides evenly (10, 20, 25, 50)
or distribute the remainder across frames. Pick one and cover it with a test;
the error is invisible until someone reports that a capture "feels wrong."

Browsers also clamp delays below 2cs up to 10cs, so anything above 50fps is
fiction in a GIF and the import UI should say so.

### Export

ffmpeg cannot write the GIF. Its GIF output is effectively constant frame rate,
which discards the per-frame delays that are the reason this app exists. So:

1. RGBA frames plus per-frame delays
2. `color_quant::NeuQuant` for a global palette
3. `gif` crate writes frames with exact delays
4. `gifsicle -O3 --lossy` post-pass

`palettegen`/`paletteuse` were the original plan and were dropped for the reason
above. NeuQuant's global-palette quality holds up well on screen captures, which
are mostly flat UI color and are the dominant input.

## Playback

The frame strip is the scrubber. A GIF is short, thumbnails fit, and clicking a
thumbnail is seeking. Two separate widgets would be inherited habit rather than
a requirement.

Playback reschedules a `glib::timeout_add_local` per frame using that frame's
delay, rather than running a fixed tick with an accumulator. It is less code and
correct by construction.

## The interface

The architecture above constrains the chrome before any of it is designed: the
region picker belongs to the portal, the frame strip is the scrubber, and the
edit scope is the product. This is the window plan that keeps all three.

### Chrome

There is no menu bar. GNOME applications do not ship one (Loupe, Papers, Text
Editor), ScreenToGif's File/Edit/View row is a Windows convention, and the
duties of a "File" menu here are small. The empty main window is the welcome
state, with Open as the one large action plus drag-and-drop anywhere; the
primary menu (hamburger, upper right) holds Open…, Keyboard shortcuts
(Ctrl+?), and About.

```
[ Undo  Redo ]            foo.gif                       [ Export ]  ☰
                      24 frames · 3.0 s
```

`AdwHeaderBar` has three slots: start, a centered title widget, end. Transport
fits in none of them without crowding the title, and the centered slot is the
first casualty when the window narrows, so a timecode parked there vanishes
exactly when space runs out. Play, the timecode, and the fps readout sit in the
footer instead, beside the strip that scrubs them. Showtime and Loupe put
transport in a bottom bar for the same reason.

Undo and redo are headerbar buttons rather than entries under an Edit menu:
history is the most-pressed action in an editor and should not sit behind a
click. Export is the only primary button. The `AdwWindowTitle` subtitle carries
the frame count — document metadata, not a control. Duration stays out of it:
it already lives in the footer's timecode total, and the same number on screen
twice is one more thing to keep in sync.

Numbers that update use the system font with tabular figures
(`font-feature-settings: "tnum"`), not a monospace face. Digits stop jittering
either way, and this keeps a second typeface out of the app.

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

Transform handles follow the behavior recorded under Relationship to Impasto
(opposite-corner anchor, Shift constrains to the source aspect, Ctrl
re-centers), with angle plus un-rotated rect tracked from the first commit.
Dragging snaps to center x/y and canvas edges at roughly 4 px.

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
one more band on the strip that already exists. It is also the one thing GIMP
structurally cannot draw, so it is what a first screenshot should show.

Underneath, the strip still does its other jobs: clicking a thumbnail seeks,
marquee-drag extends the selection, detached frames carry the badge, hover
shows the per-frame delay, drag reorders, right-click opens the frame menu, and
playback autoscrolls.

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

`V T R O A I C` rather than an internally tidy scheme: these are Figma's
letters — V select, R rectangle, O ellipse, T text — the closest thing to a
cross-tool standard that exists. Photoshop's own map (M marquee, U shapes)
agrees with none of them.

Del is bound to whichever surface has focus, since an overlay and a frame range
can both be selected at once. Canvas focus deletes the overlay, strip focus
deletes the frames. That makes a visible focus ring on both surfaces a
requirement rather than a nicety.

The shortcuts window is both the HIG-expected help overlay and the
documentation of this map.

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

## Deferred, with reasons

- **Keyframed overlay motion.** Overlays are static across their range.
  Interpolated movement needs curves, a different history shape, and an editor
  UI. Add it when someone asks for moving text.
- **Per-frame render cost.** An edit currently rebuilds every frame the changed
  overlay covers. Restrict rebuilds to the intersecting range; an overlay
  spanning the whole document still costs a full walk. Painting the visible
  frames first (see States) hides the cost but does not remove it.
- **Disk-backed frame cache.** See the memory budget under Import.
- **Oriented transform handles.** Impasto's handles snap back to an
  axis-aligned bounding box after a rotation, which its own notes call a
  deferred rewrite rather than a polish item. Track angle plus un-rotated rect
  from the start here; it is cheap greenfield and expensive to retrofit.

## Licensing

This software will be MIT. We only use screen to gif and gifcurry for reference, not copy-pasting code.
Pinta and Impasto are MIT, this project is in Rust, only code patterns can be copied.
If we use gifsicle, gifski, etc, AGPL, GPL, might require having a text copy of the license in our software repository for reference.

- Pinta and Impasto are MIT. Patterns ported from them are clean; keep the
  copyright headers on any file that is a direct port.
- ScreenToGif is Ms-PL, which carries terms into derivative works. Behavioral
  reference only, and it stays out of the tree.
- gifsicle is GPL-2 and is called as a subprocess, so it does not reach this
  code.
- gifski and libimagequant are AGPL and GPL respectively, with commercial
  licenses available. Both were rejected in favor of the NeuQuant plus gifsicle
  path; adopting either is a decision about this project's own license.
- `references/` is gitignored for these reasons.

## Relationship to Impasto



Building the GIF editor as an Impasto add-in would reuse its canvas, handles,
history, and translations. Impasto is a Pinta fork and a working GTK4 image editor. Two alternatives to
this project were considered and rejected.

We considered adding animated GIF editing to Impasto but Impasto wouldn't have the room for streamlined GIF operations people expect
(e.g. video convert-to-gif, quickly adding text to all frames)
It needs two host changes the add-in documentation
already flags: file-format registration does not dedupe by extension and can
remove built-in formats, and there is no extension point for dock pads, which a
frame strip requires. 
Impasto's document, as a paint software, is
layer-major with no time axis, so a frame-range overlay model would be bolted
onto a shape that does not have room for it.

Consuming `Pinta.Core` as a library does not work. `PintaCore` is a static
object holding GUI managers, and `DocumentHistory` reaches into
`PintaCore.Chrome`, so the core is not headless and pulls the application in
behind it.

What is taken from Impasto instead is design: the object-list invariant above,
and the transform-handle behavior recorded in its notes (opposite-corner anchor,
Ctrl re-centers, Shift constrains to the source aspect ratio rather than a
square, grip visibility gated on the selection's polygons). Those are bugs
someone already found through live testing. Port the behavior, write the Rust
fresh.

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
