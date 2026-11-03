# Browser editor architecture

The desktop editor (`src/ui/`) ported to plain DOM + Canvas2D. The document
core is `gifkino-core` compiled to wasm (`crates/gifkino-wasm/API.md` is its
contract), running in a Web Worker. This file is the contract for feature
slices.

```bash
vp install
vp run wasm      # crates/gifkino-wasm/build.sh → src/wasm/ (needed by check and dev)
vp dev           # http://localhost:5173/edit/
vp check         # format, lint, type-check
vp test          # Vitest
vp run build     # wasm + bundle → dist/; ../build.sh assembles the site
```

## Modules

| File                 | Role                                                                                                 |
| -------------------- | ---------------------------------------------------------------------------------------------------- |
| `worker/worker.ts`   | Worker entry: one wasm `Editor`, text rasterizer installed.                                          |
| `worker/serve.ts`    | Answers protocol requests against the editor; testable with a stand-in.                              |
| `worker/protocol.ts` | Request/response types, `DocSnapshot`, `FrameSource`, `Picture`.                                     |
| `worker/text.ts`     | `TextRasterizer`: Pango description → CSS font, WordChar wrap, outline under fill.                   |
| `rpc.ts`             | Page-side client: one promise per request, progress callbacks, worker errors reject.                 |
| `state.ts`           | `AppState`, `Store`, pure transitions (`edited`, `opened`, `framesInScope`).                         |
| `app.ts`             | `App`: dispatch, registries, open/export, busy bar, size estimate.                                   |
| `playback.ts`        | Play/pause on per-frame delays.                                                                      |
| `core.ts`            | The shell's own commands: open, export, undo, redo, play-pause; scope toggle; drop; unload guard.    |
| `gifsicle/`          | gifsicle built for WASI (`build.sh`; the `.wasm` is committed) and run on stdin/stdout by `wasi.ts`. |
| `ui/`                | Layout, header + menus, canvas view, transport, doc info, DOM helpers.                               |
| `i18n.ts`            | `t` / `n` / `tn` / `fill`; parses po/*.po (bundled as assets) like desktop `i18n.rs`; `setLanguage`. |
| `keys/`              | Keymap (desktop `keymap.rs`), global shortcuts, the Ctrl+? shortcuts window / keybinding editor.     |
| `settings/`          | Desktop settings.conf (language, memory budgets) and its dialog.                                     |
| `modals.ts`          | Canvas drag modifiers (desktop `keymap::Modal`/`Mods`): ids, defaults, `modsHeld`, `modsLabel`.      |
| `slices.ts`          | The list of slice installers.                                                                        |

## Adding a slice

Write `src/<slice>/index.ts` exporting `install(app: App): void`, then add it to
`slices.ts`. `install` runs once at startup, **before the layout is built**, so
everything is registered there; the layout reads the registries once.

Keep a slice's code in its own directory. Touch shared files only to add a
field or a hook that more than one slice needs, and keep such changes small.

## State and edits

- `app.state` is the current `AppState`; `app.store.subscribe((state, previous) => …)`
  re-renders. Change window state with `app.update({ … })` or
  `app.update((state) => newState)`. State is immutable: replace, never mutate.
- `state.doc` is the latest `DocSnapshot` (`null` = welcome page). It is
  refreshed only by `dispatch`/`open`; never edit it yourself.
- **Every document edit is `await app.dispatch(method, ...args)`**, typed from
  the wasm `Editor` (`dispatch("deleteFrames", framesInScope(app.state))`).
  It runs in the worker, then replaces `state.doc` and clamps playhead,
  selection and selected overlay (`state.edited`). It returns the method's
  result (`Change | undefined`, `OverlayEdit`, …); act on it (select the new
  overlay, move the playhead) with `app.update`. Errors reject.
- Read-only calls (`overlayAt`, `dragOverlay`) use `app.query(method, ...args)`:
  no snapshot, no refresh. The module's free functions (`handleAt`, `contains`,
  `orientedCorners`, `fitCanvas`, `delaysForFps`) run in the worker too, via
  `app.compute(name, ...args)`; the page never loads the wasm itself.
- Live drag previews (`setOverlayTransformLive`) go through `dispatch` so the
  overlays in `state.doc` and the canvas follow.
- `framesInScope(state)` is the desktop's scope: the frame list an edit in the
  current scope touches. Scope is `state.scope` (`frame` / `all` / `range`);
  `range` is `state.selection`, sorted indices the strip owns.
- `state.selectedOverlay`, `state.activeTool` (a string the tool slice names)
  and `state.busy` are shared; `app.withBusy(template, job, cancel?)` drives the busy bar
  (`{done}`/`{total}` placeholders; `cancel` adds its Cancel button).
- `app.composite(i)` / `app.thumbnail(i)` return `Picture`s
  (`new ImageData(p.data, p.width, p.height)`). Cache thumbnails by
  `doc.frames[i].key`.
- Adding frames: `app.stage(source, progress)` decodes a `FrameSource` in the
  worker and returns `{ length, width, height, needsFit }`; then
  `app.request("splice", { at, fit }, { snapshot: true })` and
  `app.update((s) => edited(s, reply.doc!))`. `open` works the same way and is
  wrapped by `app.openFile(file)`. `app.stageFile(file)` is its first half: the
  file through the importers, staged under the import busy bar (what
  `frameops` splices for "Add frames from file…"); `null` when the importer
  was cancelled.
- Any other worker operation: add it to `Ops` in `protocol.ts` and a handler in
  `serve.ts`.

## Seams

| Seam                                                                                | Use                                                                                                                                                                                                                                                                                                                                                                                                                  |
| ----------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `app.registerCommand({ id, label, menu?, section?, enabled?, tooltip?, run })`      | Every user action. `menu` is one of `frames`, `insert`, `effects`, `image`, `optimize` (header bar, in that order) or `file`, `app` (☰ menu). `section` splits a menu with separators. `label` is the untranslated desktop string. `id` is the keybinding target.                                                                                                                                                   |
| `app.runCommand(id)`                                                                | Run one if enabled; a thrown error becomes a toast.                                                                                                                                                                                                                                                                                                                                                                  |
| `commandButton(app, id, content, className)` (`ui/dom.ts`)                          | Button bound to a command: runs it, greys when disabled, tooltip with chord.                                                                                                                                                                                                                                                                                                                                         |
| `app.registerPanel({ slot, id, order, element })`                                   | Mount UI. Slots: `rail` (tool column left of the canvas), `transport` (right end of the play row; scope toggle is order 0), `strip` (frame strip), `bands` (overlay bands, under the strip), `sidebar` (properties, above the doc info).                                                                                                                                                                             |
| `app.registerCanvasLayer({ id, order, draw?, pointer? })`                           | Canvas overlay. `draw(ctx, view, state)` in CSS px; `pointer(event, phase, view)` returns `true` to take the event (highest `order` first; a taken `down` captures the pointer). `view.scale` is screen px per image px; `view.toImage(event.offsetX, event.offsetY)` / `view.toScreen(x, y)` convert. Call `app.redrawCanvas()` after changing what a layer draws without a state change; any state change redraws. |
| `app.registerImporter({ accept, matches, load })`                                   | Open path for other file kinds (video): `load(file)` returns a `FrameSource` (`rgba` frames, transferred), or `null` when the user cancelled. Anything slow (download, decode) runs under the importer's own `withBusy`. Registered importers win over the built-in GIF/image one.                                                                                                                                   |
| `app.shortcutFor = (commandId) => chord \| null`                                    | Set by `keys/`: the first chord bound to the keymap action with that id (keymap action ids are command ids). Read it when rendering, inside a store subscriber: a rebind runs `app.update({})`, so hints rebuilt there move with it.                                                                                                                                                                                 |
| `app.modifiersFor = (modal) => Mods`                                                | Set by `keys/`: the modifiers held during a canvas drag, by desktop `Modal::id` (`canvas-rotate`, `canvas-keep-aspect`, `canvas-from-center`; `MODALS` in `modals.ts`). Defaults Alt, Shift, Ctrl. The canvas reads it on every press and motion, the grip tooltips on every hover.                                                                                                                                  |
| `app.toast(message, { undo? })`                                                     | Transient message. `undo: true` adds the Undo button an edit's toast carries (desktop `Msg::Toast`); leave it off for feedback with nothing to undo (`Msg::Notice`, e.g. "3 frames copied").                                                                                                                                                                                                                         |
| `app.toastChange(change, total?)`                                                   | Desktop `toast_if_document_wide`: toasts "{change} on {count} frames" with Undo when `change.framesTouched >= total` (the frame count before the edit); without `total` always (a splice). `undefined` change: nothing.                                                                                                                                                                                              |
| `alertDialog({ title, body, content, responses, close, default })` (`ui/dialog.ts`) | `adw::AlertDialog` as a modal `<dialog>`; `.answer` resolves with the response id, `.setEnabled(id, on)`.                                                                                                                                                                                                                                                                                                            |
| `ui/prefs.ts`                                                                       | Libadwaita rows for the sidebar and dialogs: `group(title, ...rows)` (`groupTitle` to retitle), `actionRow(title, suffix, subtitle?)` (`setSubtitle`), `suffix(...)`, `spin(min, max, step, value)`, `radioRows(options)`.                                                                                                                                                                                           |
| `t()`, `fill()`, `n()`                                                              | Wrap every user-visible string in a literal `t("…")` (or `n("…")` where it is translated later), so `scripts/i18n.py` extracts it into po/; use the desktop's msgid when one exists. A language switch retranslates built text in place (text nodes, `title`, `placeholder`, `aria-label`); a string filled with values has to be rebuilt in a store subscriber.                                                     |
| `globalThis.gifkino`                                                                | The `App`, for console debugging.                                                                                                                                                                                                                                                                                                                                                                                    |

## Frame-op commands (`frameops/`)

Command ids other slices may run (`app.runCommand`), named after the desktop's
keymap actions / `win.*` actions. Scope-based ones act on `framesInScope`.

| Id                                                                         | Desktop                                                                             |
| -------------------------------------------------------------------------- | ----------------------------------------------------------------------------------- |
| `frame-cut`, `frame-copy`, `frame-paste`                                   | Frames ▸ Cut / Copy / Paste                                                         |
| `frame-delete`, `frame-duplicate`, `frame-reverse`                         | Frames ▸ Delete / Duplicate / Reverse                                               |
| `frame-delay-all`                                                          | Frames ▸ Set delay for all frames…                                                  |
| `frame-delay`                                                              | Frame menu ▸ Set delay… (playhead frame's delay, applies to the scope)              |
| `frame-move-earlier`, `frame-move-later`                                   | Frame menu ▸ Move earlier / later (the scope)                                       |
| `frame-move-to`                                                            | Frame menu ▸ Move to position… (the playhead frame)                                 |
| `frame-insert-file`                                                        | Frame menu ▸ Insert frames from file… (after the playhead)                          |
| `import-more`                                                              | ☰ ▸ Add frames from file… (at the end), with the "Sizes do not match" fit question |
| `delete`                                                                   | Delete key: the selected overlay, else the strip selection                          |
| `mirror-horizontal`, `mirror-vertical`, `fade-in`, `fade-out`, `loop-fade` | Effects menu                                                                        |
| `optimize-resize`                                                          | Image ▸ Resize frames…                                                              |
| `optimize-remove`, `optimize-smart`                                        | Optimize ▸ Remove frames… / Smart remove frames…                                    |

Memory guard: `frameops/budget.ts` holds the desktop's budgets
(`max_import_bytes`, `max_oper_bytes`, `max_total_bytes`) and refuses a resize
or splice over them before the worker is asked; the settings dialog writes
`budgets`, and the wasm heap caps them whatever they say.

Export: `app.exportGif(settings)` encodes in the editor worker, then runs
`gifsicle -O3` (plus `--lossy` when `settings.lossy` > 0) over the file in a
worker of its own (`gifsicle/`), as desktop `export_path` does. A gifsicle that
fails to load or run leaves the unoptimized GIF, downloaded with the same
toast, as on a desktop without gifsicle. The desktop has no export settings UI
(Export GIF… writes `ExportSettings::default()`), so there is none here.

## Strip and bands (`strip/`)

The frame strip (slot `strip`) and overlay bands (slot `bands`) of desktop
`strip.rs`. Clicks, Shift/Ctrl+clicks, band picks and drag-reorder set
`playhead`, `selection`, `scope` and `selectedOverlay`; the Shift+click anchor
is the strip's own and restarts at the first selected frame when another slice
replaces the selection. The right-click menus run the frame-op commands above
and the overlay slice's `overlay-copy-all`, `overlay-copy-selected`,
`overlay-delete`. Keyboard-only commands: `select-all`, `strip-zoom-in`,
`strip-zoom-out`, `strip-zoom-reset` (Ctrl+wheel over the strip zooms too).

## Overlays and crop (`overlays/`)

The rail (text, the rect/ellipse/arrow split button, crop, snap), the canvas
layer (order 10: pick, move, corner resize, rotate, snap guides, crop box), and
the sidebar groups "Overlays on this frame" (order 10), "Properties" (20),
"Overlay" (30) and "Crop / zoom" (40). `state.activeTool === "crop"` is the
crop tool armed; picking an overlay anywhere (`selectedOverlay` set) disarms it.
Escape backs out of the crop tool, else drops the pick.

| Id                                                             | Desktop                                                                     |
| -------------------------------------------------------------- | --------------------------------------------------------------------------- |
| `tool-text`, `tool-rectangle`, `tool-ellipse`, `tool-arrow`    | Tool keys: add that overlay under the scope. Tools may share a key: cycle.  |
| `tool-crop`, `toggle-snap`                                     | Crop tool on/off; snapping to the canvas edges and centre lines on/off      |
| `insert-text`, `insert-rect`, `insert-ellipse`, `insert-arrow` | Insert menu                                                                 |
| `zoom-to-selection`                                            | "Zoom this frame": the crop box blown up on the scope                       |
| `optimize-crop`, `optimize-zoom-all`                           | Image ▸ Crop frames… / Zoom and resize all frames (needs a crop box)        |
| `overlay-copy-all`, `overlay-copy-selected`, `overlay-delete`  | Band menu on the picked overlay: copy to every frame / to the scope, delete |

## Video import (`video/`)

The desktop's `pipeline/video.rs` and import dialog on ffmpeg.wasm's
single-thread core. The first video import asks before downloading the core
(~30 MB) from jsDelivr, or unpkg if that fails, rather than from the canister,
which would pay to store it. `vite.config.ts` pins the URL to the installed
`@ffmpeg/core`'s version and bakes in its wasm's SHA-256; a download that hashes
differently is discarded, and the glue's own reference to the wasm is stripped
so the build does not ship it. The core downloads under the busy bar
and compiles once per page; `ffmpeg.worker.ts` instantiates it and runs
ffprobe and ffmpeg there, the file mounted read-only (WORKERFS) and the RGBA
frames streamed out of a device file as they are written. `plan.ts` is the
desktop's plan arithmetic and trim parsing (`plan_for`, `read_trim`, …), so the
dialog's preview is the decode it runs. Cancel terminates the worker; the next
job starts a fresh one from the compiled module.

## Keys, settings, translations (`keys/`, `settings/`, `i18n.ts`)

One capture-phase `keydown` listener on the window runs the command whose id
is the keymap action the chord fires (desktop `install_shortcuts`); tools
sharing a key cycle. It stands aside while a text field, a `<select>` or a
dialog has focus, and for Cmd chords. `show-shortcuts` (Ctrl+?, ☰) opens the
shortcuts window, which is the keybinding editor; `settings` (☰) opens the
settings dialog. Both save as they change, to localStorage under
`gifkino/keybindings.conf` and `gifkino/settings.conf`, in the desktop's file
formats, so the keybindings file imports and exports between the two.

The catalog loads in `main.ts` before any slice installs: the `language`
setting, else the first of `navigator.languages` with a catalog (`de-DE` →
`de`). Fuzzy entries are used. New strings: `scripts/i18n.py merge`, then
translate them in every po/ catalog (AGENTS.md, Translation & Localization).

## Conventions

- No framework: `h(tag, props, ...children)` builds elements; subscribe and
  set properties to update. Inline `style` attributes are blocked by the CSP;
  set `element.style.*` from script or use classes in `style.css`.
- Site CSP lives in `web/public/_headers` and applies to the whole site.
  Anything new a slice loads (blob workers, `data:` images, CDN) must be added
  there, minimally.
- Tests (`*.test.ts` beside the code) cover logic, not wiring. Import from
  `vite-plus/test`.
