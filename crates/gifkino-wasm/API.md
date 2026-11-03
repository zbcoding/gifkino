# gifkino-wasm JavaScript API

The browser editor's handle on `gifkino-core`, the same document code the
desktop app runs. `build.sh` writes `gifkino_wasm.js`, `gifkino_wasm.d.ts` and
`gifkino_wasm_bg.wasm` to `web/editor/src/wasm/`; the `.d.ts` carries every type
named below.

```ts
import init, { Editor, Frames } from "./wasm/gifkino_wasm.js";
await init();                       // Node or a worker with bytes: initSync({ module: bytes })
const editor = new Editor();
editor.setTextRasterizer(drawText);  // Canvas2D, see "Text"
editor.open(Frames.fromGif(new Uint8Array(await file.arrayBuffer()), null));
const frame = editor.composite(0)!;  // new ImageData(frame.data, frame.width, frame.height)
```

## Conventions

- **Calls are synchronous.** A resize or an export of a few hundred frames
  takes seconds; run the module in a Web Worker. Optional callback arguments
  take `null`, not omission.
- **Frame lists** are `Uint32Array | number[]`. They are resolved the way the
  desktop resolves a selection: sorted, deduplicated, indices past the end
  dropped. The desktop's scope maps onto them: "this frame" is
  `[playhead]`, "all frames" is every index, a strip selection is its indices.
- **Window state stays in JavaScript.** Playhead, selection, scope, the picked
  overlay and the crop box are the caller's; methods that need one take it.
- **Edits return what they did.** Every method below that edits returns a
  `Change` (`{ label, framesTouched }`), or a richer object containing one, and
  makes one undo step. `undefined` means nothing changed and no step was
  recorded: an empty or stale frame list, an unknown overlay id, a no-op such as
  a full-canvas crop. `label` is the untranslated desktop history label
  ("Frames deleted"); the desktop toasts `"{change} on {count} frames"` when
  `framesTouched > 1`, and only for edits that reached every frame.
- **Errors throw** a JavaScript `Error`: undecodable bytes, an argument object
  of the wrong shape (bad enum string, missing field), a text rasterizer that
  threw or returned the wrong number of bytes. Nothing is changed by a call
  that throws.
- **Coordinates** are image pixels, floats. Overlay ids are numbers, stable
  across undo and redo.
- `Frames` and `Image` are wasm-owned; call `.free()` on ones you are done
  with, or let the finalizer have them.

## Types

```ts
type Rgba8 = [r, g, b, a];                              // 0–255
interface Transform { x; y; w; h; angle }               // un-rotated box + radians clockwise about its centre; w/h may be negative
type OverlayKind =
  | { type: "text"; text; font; sizePx; color: Rgba8; outline: [Rgba8, width] | null;
      align: "left" | "center" | "right" | "justify"; antialias: boolean }
  | { type: "shape"; shape: "rect" | "ellipse" | "arrow"; fill: Rgba8 | null;
      stroke: [Rgba8, width] | null };
interface Overlay { id; name; kind: OverlayKind; range: { start; end };  // frames start..end-1
                    transform: Transform; opacity; hidden }
interface FrameInfo { delayCs; detached; opaque; key }  // key: equal keys = identical pixels (cache thumbnails by it)
interface Change { label: string; framesTouched: number }
interface OverlayEdit   { change: Change; overlay: number }               // the id now carrying the edit: select it
interface OverlaysAdded { change: Change; overlays: number[]; overlay: number }
interface FramesMoved   { change: Change; at: number }                    // where the moved run starts now
type FitMode = "stretch" | "pad" | "growPad" | "growStretch";
type Tool = "text" | "rect" | "ellipse" | "arrow";
type Grip = "move" | "rotate" | { resize: 0 | 1 | 2 | 3 };               // corners TL, TR, BL, BR
interface Guides { x: "start" | "center" | "end" | null; y: same }        // x names a vertical line
interface ExportSettings { width?: number | null; speed?: number; colors?: number;
                           dither?: boolean; loops?: number | null; lossy?: number }
```

## Frames — decoded pixels on their way in

| Member | |
|---|---|
| `Frames.fromGif(bytes, progress \| null)` | Every frame, full canvas, disposal applied, per-frame delays kept. `progress(decoded)` returning `false` stops the decode there and keeps what decoded. Throws on a broken or frameless GIF. |
| `Frames.fromImage(bytes)` | One still (PNG, JPEG, BMP, TIFF, TGA, ICO, QOI, PNM), delay 10 cs. Throws if undecodable. |
| `new Frames()` + `push(width, height, rgba, delayCs)` | Raw RGBA frames, e.g. video frames decoded with WebCodecs. `rgba` is `width*height*4` bytes or it throws. Use `delaysForFps(fps, count)` for the delays. |
| `length`, `width`, `height` | Count; the first frame's size (0 when empty). |

## Editor

### Document

| Member | |
|---|---|
| `new Editor()` | Empty document. |
| `open(frames)` | Replace the document, clearing undo history (the frame clipboard stays). |
| `needsFit(frames)` | Whether a splice must ask the fit question: the document has a canvas and `frames` are another size. |
| `splice(frames, at, fit)` → `Change` | Insert at `at` (clamped), fitting both sides onto one canvas. Desktop: `at` = `frameCount` for "Add frames", or the frame menu's index; ask the fit question only when `needsFit`, else pass `"stretch"`. One incoming frame takes the delay of the frame before `at`. Label "Frame added"/"Frames added". |
| `width`, `height`, `frameCount`, `durationCs`, `hasAlpha` | |
| `revision` | Bumped by every change to what `composite` draws, undo/redo and live drags included. |
| `frames()` → `FrameInfo[]` | |
| `overlays()` → `Overlay[]` | Bottom to top: list order is z-order. The layer list shows it reversed and filtered to the playhead frame (`range.start <= playhead < range.end`, hidden ones included). |
| `overlay(id)` → `Overlay \| undefined` | |
| `composite(index)` → `Image \| undefined` | Frame with its overlays drawn; calls the text rasterizer. |
| `thumbnail(index)` → `Image \| undefined` | Filmstrip thumbnail fitted to a 72 px box, no overlays. |

`Image`: `width`, `height`, `data` (a fresh `Uint8ClampedArray` copy per read).

### History

`undo()` / `redo()` → `boolean` (whether anything happened). `canUndo`,
`canRedo`, `undoLabel`, `redoLabel` (the step's label, for tooltips). 200
steps are kept.

### Frames

| Method | Desktop action | Label |
|---|---|---|
| `deleteFrames(frames)` | Delete | Frames deleted |
| `cutFrames(frames)` | Cut (copies, then deletes) | Frames cut |
| `copyFrames(frames)` → `number` | Copy; not an edit, returns how many | — |
| `pasteFrames(at)` | Paste; desktop passes `playhead + 1` (0 when empty), then puts the playhead on the last pasted frame and selects the run if it is longer than one | Frames pasted |
| `clipboardLength` | Whether Paste is enabled | — |
| `duplicateFrames(frames)` | Duplicate: each copy lands after its source | Frames duplicated |
| `reverseFrames(frames)` | Reverse the span first..last | Frames reversed |
| `moveFrame(from, to)` | Move frame dialog | Frame moved |
| `moveFrames(frames, gap)` → `FramesMoved` | Strip drag drop on divider `gap` (0 = before the first frame, `frameCount` = after the last). Desktop then selects `at..at+n` and moves the playhead with its frame | Frames moved |
| `nudgeFrames(frames, earlier)` → `FramesMoved` | Move earlier / later: swap the block with its neighbour | Frames moved |
| `setDelay(frames, delayCs)` | Frame delay (min 1 cs) | Delay set |
| `dropEveryNth(n)` | Drop every Nth (n ≥ 2), keeping running time | Frames removed |
| `smartDrop(percent)` | Drop the least-moving `percent` % (max 95), keeping running time | Frames removed |

After any edit, clamp the playhead to `frameCount - 1` and drop selected
indices past the end. Desktop resets the playhead to 0 after both drops.

### Effects and canvas

| Method | Notes | Label |
|---|---|---|
| `mirror(frames, "horizontal" \| "vertical")` | | Frames mirrored |
| `fade(frames, "in" \| "out", colour)` | Needs ≥ 2 frames; desktop offers black, white, transparent | Faded in / Faded out |
| `loopFadeLimit` | Most frames `loopFade` can blend (half the document) | |
| `loopFade(count)` | Dissolve the last `count` frames into the first, then drop the first `count` | Loop cross-faded |
| `crop(x, y, w, h)` | Whole canvas to the box (widened to whole pixels, clipped to the canvas); overlays stay on the picture. Full-canvas box → `undefined` | Cropped |
| `shrink(frames, x, y, w, h)` | "Crop and keep size": keep the box on `frames`, rest transparent | Cropped |
| `zoom(frames, x, y, w, h)` | "Zoom and resize": blow the box up to fill the canvas on `frames` | Zoomed |
| `resize(width, height)` | Every frame; overlays scale. Same size → `undefined` | Resized |

The desktop refuses a resize or splice whose RGBA frames would exceed its
memory budget (`width*height*4*frames` for the result, plus the current frames
kept for undo). These methods do not check; a page that lets the wasm heap
pass 4 GiB loses the module, so apply a budget before calling.

### Overlays

| Method | Notes | Label |
|---|---|---|
| `addOverlay(tool, frames, playhead)` → `OverlaysAdded` | One overlay per contiguous run of `frames`, with the desktop's defaults (text caption along the bottom, shapes filled `[255,60,60,255]` over the middle). `overlay` is the piece on `playhead`, else the first: select it | Text added, Rectangle added, Ellipse added, Arrow added |
| `deleteOverlay(id)` | | Overlay deleted |
| `restackOverlay(id, up, playhead)` | One step up (painted later) or down past the overlay next to it on frame `playhead`'s layer list | Overlay reordered |
| `editText(id, text, frames, playhead)` → `OverlayEdit` | Scoped (below) | Text edited |
| `restyleOverlay(id, kind, frames, playhead)` → `OverlayEdit` | Replace the kind's fields (font, size, colours, outline, align, antialias; fill, stroke). Same value or other type → `undefined`. A playhead off the overlay counts as its first frame; desktop seeks there first | Overlay restyled |
| `copyOverlayTo(id, frames)` | Onto those of `frames` it does not cover, one piece per run; none → `undefined` (desktop says "The overlay is on those frames already") | Overlay copied |
| `overlayAt(frame, x, y)` → `number \| undefined` | Topmost visible overlay under the point, rotation honoured | |

**Scoped overlay edits.** `editText`, `restyleOverlay` and `commitOverlayDrag`
land where `frames` reaches into the overlay's range; when that misses the
playhead but the overlay is on the playhead, on the playhead's frame alone. If
that is not the whole overlay it splits, and `overlay` in the result is the
piece carrying the edit. A band click selects the overlay's whole range as
`frames`, which edits it whole.

**Canvas drags** (desktop `canvas_press` / `canvas_drag` / `canvas_release`):

1. Press: with an overlay selected, a grip within `9.5 / scale` image px
   (`handleAt(transform, x, y, grab)` → corner) starts `{ resize: corner }`; the
   rotate modifier (default Alt) on it starts `"rotate"`. Otherwise
   `overlayAt(playhead, x, y)` picks and starts `"move"` (or `"rotate"`). Keep
   `origin = overlay.transform` and the press point.
2. Motion (ignore until the pointer has moved 0.5 px):
   `editor.dragOverlay(grip, origin, fromX, fromY, toX, toY, snapReach, keepAspect, fromCenter)`
   → `{ transform, guides }`, where `snapReach` is `8 / scale` image px or
   `null` with snapping off, `keepAspect` the Shift modifier (also steps a
   rotation to 1/32 turns), `fromCenter` the Ctrl modifier. Show it with
   `setOverlayTransformLive(id, transform)` (no undo step) and draw `guides`.
3. Release, if it moved: `commitOverlayDrag(id, grip, origin, transform, frames, playhead)`
   → `OverlayEdit`, labelled Overlay moved / rotated / resized.

`scale` is screen pixels per image pixel. `orientedCorners(transform)` gives the
four grip positions (TL, TR, BL, BR) for drawing.

### Export

| Method | |
|---|---|
| `exportGif(settings \| null, progress \| null)` → `Uint8Array` | The GIF file. `null` settings = the desktop export: full size, source speed, 256 colours, no dither, loop forever. Missing fields take those defaults. `width` resizes (height follows), `speed` divides delays, `loops` `null` = forever. `lossy` is ignored: the desktop's gifsicle `-O3`/`--lossy` pass does not run here, so files are larger than the desktop's. `progress(done, total)` after each frame. Throws on an empty document. |
| `estimateSize()` → `number` | Bytes, from encoding 8 spread frames with the default settings and extrapolating — the desktop's size readout (which it measures without gifsicle too). Throws on an empty document. |

## Text

`setTextRasterizer(fn | null)`. `fn(text: TextOverlay, width, height)` returns
`width*height*4` RGBA bytes (`Uint8ClampedArray` or `Uint8Array`) of the
overlay's box, un-rotated: the core places, rotates and blends it. It receives
the text fields of `OverlayKind` (no `type`). Match the desktop's Pango
rendering: `font` is a Pango description ("Sans Bold") at `sizePx` pixels,
wrapped at word or character to the box width, aligned by `align` (justify
under left alignment), the block centred vertically in the box. `outline` is a
round-joined stroke of twice its width along the glyph outlines, painted under
the fill (width 0 draws none). `antialias` off means hard pixel edges. Called
synchronously from `composite`, `exportGif` and `estimateSize`, once per
visible text overlay per frame, so an `OffscreenCanvas` works in a worker.
Without a rasterizer, text draws nothing.

## Free functions

| Function | |
|---|---|
| `handleAt(transform, x, y, grab)` → `number \| undefined` | Corner grip within `grab` of the point. |
| `orientedCorners(transform)` → `[x, y][]` | Grip positions on the image. |
| `fitCanvas(fit, canvasW, canvasH, incomingW, incomingH)` → `[w, h]` | The canvas a splice under `fit` produces, for the fit question's labels. |
| `delaysForFps(fps, count)` → `Uint16Array` | Per-frame delays for video frames, spreading the centisecond remainder so the total keeps time. |

## Not here

Video decoding (ffmpeg on the desktop; use WebCodecs and `Frames.push`), the
gifsicle optimization pass, file dialogs, playback timing (advance the playhead
by each frame's `delayCs * 10` ms), keybindings, and translations of the labels.
