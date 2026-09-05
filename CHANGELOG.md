# Changelog

## 0.1.0 — 2026-09-05

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

- Ctrl+wheel over the strip zooms the thumbnails; Ctrl+Up / Ctrl+Down and
  Ctrl+plus / Ctrl+minus do the same from the keyboard, and Ctrl+0 resets. The
  overlay bands scale with the thumbnails so their columns stay aligned. Zoom
  out to sweep a long document, in to pick a frame precisely.
- The strip's horizontal scrollbar sits in its own strip of space rather than
  floating over the bottom of the thumbnails and their frame numbers.
- Frames sit above the overlay bands rather than below them.
- The band area takes no space until there is an overlay, then grows a row at a
  time to five rows and scrolls behind the expander after that. The canvas keeps
  whatever is left instead of a fixed split.

### Scope

- Overlay edits now follow the scope control. Dragging, resizing or restyling a
  caption with `This frame` active touches only that frame: the overlay splits
  there, the edited frames becoming their own overlay with its own transform
  while the rest keeps what it had. An `All frames` drag still moves the whole
  thing, and the toast names which it was.
- Clicking a thumbnail seeks and drops the scope back to `This frame`. Sticky
  `All frames` past a click is how a one-frame drag turned into a hundred-frame
  edit; the chip and the strip tint make the revert visible.

### Fixes

- Adding a frame from an image works at all. The `image` dependency was built
  with no format features, so every PNG, JPEG and everything else answered
  "Could not read that image" — there was no decoder compiled in for any of
  the formats the file filter offered.
- Duplicating a frame no longer throws the timeline to another spot. The
  scroller's viewport follows the keyboard focus by default, and closing the
  frame menu — a popover parented to the strip, so a duplicate, delete or paste
  moves the focus out of it — had it scrolling off to wherever the focus landed,
  usually the first frame. Nothing in the strip takes the focus, so the viewport
  no longer chases it.
- Space toggles playback rather than adding an overlay. The window's key
  controller ran in the bubble phase, so whatever held the focus handled the key
  first — and GTK both activates a focused button on Space and parks the initial
  focus on the first focusable widget, which is the Text tool. The controller
  now runs in the capture phase, with text entries, text views and dialogs
  handed their keystrokes back. Toolbar buttons also no longer take focus on
  click, so Enter and the arrow keys keep meaning what the canvas means by them.
- Clicking a thumbnail showed the frame that was actually clicked, but
  highlighted the wrong one — the border always landed one frame behind. The
  frame's right-click popover has to be parented to the strip to show itself,
  and GTK counts a `set_parent()`'d popover as a real child right along with
  the thumbnails, so the playhead/scope/selection walk over the strip's
  children was off by one from the start. The walk now skips anything that
  is not an actual thumbnail cell.

### Toolbar

- Rectangle, ellipse and arrow share one split button in the left rail: the
  icon and a click both follow whichever shape was used last, and its dropdown
  opens a flyout to pick a different one. Text keeps its own button.

### Notifications

- A toast now only interrupts for an edit that reached every frame the
  document had — a resize, a document-wide crop, a zoom or shape edit scoped
  to "All frames". Anything narrower (a caption tweak, a single frame's delay,
  one frame deleted) already shows in the strip and the canvas, so the popup
  added nothing but noise.
- A toast for something with nothing to undo — the count a frame copy
  reports — carries no Undo button, which would have landed on whatever edit
  came before it.

### Frame operations

- "Set delay for all frames…" joins the frame menu, for setting every frame's
  delay at once instead of finding there is no menu entry for it.
- Drag a thumbnail onto another to reorder the timeline. A frame's own context
  menu adds "Move earlier", "Move later" and "Move to position…" for the same
  thing without a pointer.
- A frame's context menu can also add a frame decoded from an image file,
  spliced in right after it and resized to the canvas if it does not already
  match.
- Ctrl+X, Ctrl+C and Ctrl+V cut, copy and paste frames, on the frame scope:
  one frame, a selection, or the whole document. A paste lands directly after
  the frame on screen and keeps the clipboard, so the same run can go in
  twice; the clipboard holds frames with their delays rather than going
  through the system one, which has no format that carries a GIF frame's
  timing. Both frame menus and a frame's context menu list all three.

### Overlays

- The sidebar's layer list reads topmost-first, the way the layers stack on
  the canvas. It used to be bottom-up, so the row at the bottom was the
  overlay painted on top.
- Each row carries its own up and down carets and an X: a step through the
  z-order past the layer shown next to it, and a delete that acts on that
  row rather than on the selection. The carets are insensitive at the ends of
  the list. The red trash button in the overlay editor still deletes the
  selected overlay.
- The layer list scrolls past six layers and never shrinks below three, and
  the properties panel around it scrolls once the window is too short for
  it. A frame with a dozen overlays used to push the overlay editor, the
  crop buttons and the document summary off the bottom with no way to reach
  them. Picking a layer on the canvas or in the strip scrolls its row into
  view, so the selection is never off-screen.

### Import

- "Add frames from file…" in the File menu, and "Insert frames from file…" in a
  frame's own context menu, splice another file's frames into the timeline —
  images, GIFs and videos alike, at the end or directly after the frame that
  was right-clicked. Two clips can be mixed into one.
- A file that is not the canvas size asks how the two should fit rather than
  silently stretching what comes in. Four answers: stretch what is coming in,
  scale it to fit with transparency around it, or grow the canvas to the file
  and fit or stretch every frame already in the document onto it. Overlays
  follow the frames they were drawn on through all four. The resampling runs
  off the main thread with the same progress bar a resize uses, so growing a
  300-frame document to a bigger canvas does not freeze the window.
- Still images decode through the same entry point as GIFs and videos, so they
  splice in on a build with no ffmpeg and get progress and the fit chooser
  like everything else.

### Crop and zoom

- The three canvas-tool buttons are disabled until a crop box has actually
  been drawn, rather than as soon as nothing else is busy. Each explains
  itself in its own tooltip instead of a paragraph shared under all three.
- Zoom always acts on the frame on screen, not the scope control, which is
  what its label says: "Zoom and resize this frame only". "Crop and keep
  size" follows the scope like the frame-operations menu does instead:
  whichever frames it names — the frame on screen, a selection, or every
  frame. (It used to hardcode the frame on screen regardless of scope,
  same as zoom; selecting "All frames" and drawing a crop box still only
  cropped one. Fixed, and a regression test locks the contract in.)
- The new crop mode keeps the cropped region at its own size and place instead
  of scaling it back up to fill the canvas, blanking the rest of the frame to
  transparent — a frame can look smaller than the others without the document
  model needing a per-frame canvas size.
- "Crop all frames" and the per-frame crop mode are threaded like resize and
  zoom: a large document no longer freezes the window while every frame is
  copied, and the progress bar shows how far it has gotten.

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
- A frame's right-click menu shows the shortcut beside each item it shares with
  the keymap — delete, duplicate, cut, copy, paste — and a rebind moves what it
  shows, the same as the tooltips. An unbound action shows nothing rather than
  an empty column.

### Translations

- German and Japanese drafts for the new strings, flagged `#, fuzzy` for review.
- `scripts/i18n.py` joins a marker call that rustfmt split from its literal.
  Seven msgids had dropped out of the template that way.
- German and Japanese drafts for the frame-operations, import, and crop/zoom
  strings above, flagged `#, fuzzy` for review.

### Optimize

- "Crop all frames…" joins the other optimize dialogs: a box with X, Y, width
  and height fields for the same document-wide crop the canvas tool applies.
  German and Japanese drafts for the new strings are flagged `#, fuzzy`.

### Long jobs

- Resizing no longer freezes the app. The per-frame resample runs on a worker
  thread, the progress bar (now in the toolbar, visible with a document open)
  counts frames, and the result lands as one undoable step when it finishes.
- Zooming a scope is threaded the same way. While either runs, the app stays
  interactive: overlays keep drawing and can still be edited (deleting a
  selected overlay included); frame-moving edits and the actions that open
  dialogs on top of them grey out until the work lands.
- Frame work guards against the edge cases: a resize is refused when its RGBA
  output would exceed the configured memory limit, a worker failure reports a
  toast instead of leaving the progress bar up, crop and zoom pad frames that
  are smaller than the canvas instead of panicking, and a crop box that starts
  in the canvas margin now selects only the pixels inside the image.
- Applying the crop-all dialog unchanged (full canvas) is a no-op rather than
  an empty "Cropped" undo step, and its width/height fields stay consistent
  with X/Y as the box is edited. Dialog response buttons are translated.
- GIF imports now count frames in the progress bar as they decode; the GIF
  header carries no frame total, so the bar pulses rather than filling.
- A running import can be cancelled from an X beside the bar. The stop flag
  ends the decode between frames for both pipelines — the ffmpeg child is
  killed, the GIF decoder stops reading — and nothing is loaded; a cancelled
  import is not a failure, so it says nothing.

### Name

- The application is called **Gifkino**, and the crate, the binary, the
  application ID, the config directory and the catalog directory all spell it
  the same way. `Gifkino` is a closed compound built the way `Daumenkino` is,
  so it reads as a name rather than as the phrase "a GIF editor"; the German
  form would hyphenate after the initialism (`GIF-Kino`), which a binary name
  cannot use.
- The window title is now the literal `Gifkino` rather than a translated
  string. A proper noun has no translation, and the old msgid let a locale
  rename the application — `de` shipped "GIF-Editor", `ja` "GIF エディター".
  "GIF" and "editor" stay searchable through the desktop entry's
  `GenericName`, `Keywords` and the metainfo summary, which do get translated.
- Old state does not carry over: settings move from
  `~/.config/gif-editor/settings.conf` to `~/.config/gifkino/`, and the same
  for `keybindings.conf`. Nothing has shipped, so there is no migration path
  and none is written.

### Packaging

- The app ships as a flatpak and as an AppImage, built by
  `.github/workflows/release.yml` and attached to a `v*` tag. Both carry
  ffmpeg, ffprobe and gifsicle, so import and the optimized export work on a
  machine that has none of them installed.
- The application ID is `io.github.zbcoding.Gifkino`, and the icon resource
  prefix moved with it. The first ID, `io.github.gif_editor`, named a GitHub
  account that does not exist, which Flathub review rejects.
- The flatpak builds on the GNOME 50 runtime. Only GTK 4.14 and libadwaita 1.5
  APIs are called, gated by the feature flags in `Cargo.toml`, so the runtime
  can move forward without the code following.
- The AppImage is built on Ubuntu 24.04, whose GTK is exactly the 4.14 baseline
  the bindings target. That puts its glibc floor at 2.39 and its ceiling
  nowhere: it runs on 24.04 and everything newer.
- Added a desktop entry, AppStream metainfo, an application icon and an MIT
  `LICENSE`.
- The application icon is three stacked image cards under a K for kino, drawn
  as `xdg/io.github.zbcoding.Gifkino.svg`: the flatpak installs it to
  `hicolor/scalable/apps` and the AppImage build rasterizes it at 512, 256,
  128, 64 and 48. The K is stroked geometry rather than `<text>`, because
  nothing rasterizing the icon is guaranteed the font.
- `scripts/install-user.sh` installs the desktop entry, metainfo and icon into
  `~/.local/share` and symlinks `~/.local/bin/gifkino` at the release build,
  with `--uninstall` to undo it. A development build run straight out of
  `target/` gets a placeholder icon in the taskbar: the desktop matches a
  window by its app_id to an installed desktop file, and from there to an
  installed icon, so with nothing installed there is nothing to match.
- An AppImage run installs its own desktop entry and icons into
  `~/.local/share` on first launch, so the window carries this application's
  name and icon instead of a placeholder. `--uninstall-desktop` removes them,
  `--install-desktop` does it on demand, and `GIFKINO_NO_DESKTOP_INTEGRATION`
  turns the first-run step off. Moving or renaming the AppImage rewrites the
  entry on the next run rather than leaving one that launches nothing.
  Installing it from the application is a deviation from the AppImage
  convention, where the image only carries these files and an integrator
  (AppImageLauncher, appimaged, a software centre) copies them onto the host.
  Following the convention alone is what leaves a double-clicked AppImage with
  a placeholder, so the first run covers for a missing integrator and defers to
  a present one: an `appimagekit_*` entry for this application means the
  integrator owns it and the app adds nothing, rather than putting Gifkino in
  the menu twice.

  A Wayland client can hand its icon straight to the compositor, but GTK only
  implements xdg-toplevel-icon from 4.20 and the AppImage bundles Ubuntu
  24.04's GTK 4.14, so that route is closed to it - and the compositor cannot
  read the icon inside the AppDir, being a different process. X11 sessions
  never had the problem: GTK 4.14 sets `_NET_WM_ICON` from the icon theme,
  which resolves in-process because AppRun puts the AppDir's share tree on
  `XDG_DATA_DIRS`. The flatpak never had it either, since flatpak exports the
  entry and the icon to the host itself.
- The AppImage's `.DirIcon` is a real 256px PNG rather than a symlink to the
  scalable icon, which is what linuxdeploy leaves behind whatever
  `--icon-file` says. It is the icon a file manager draws for the `.AppImage`
  file itself, and the AppDir specification asks for a PNG in a standard size
  because nothing reading it is obliged to rasterize an SVG. The build fails if
  it is ever not a plain PNG again, since the cost is only a wrong icon on one
  file and no build would otherwise notice.
- The desktop entry's `Exec` carries a plain `%f`. flatpak's exporter is what
  writes the `@@ %f @@` file-forwarding markers: it rewrites the line, adds
  `--file-forwarding` to the command it generates, and refuses to install an
  app that ships the markers itself ("Invalid Exec argument @@"). Carrying them
  also handed a literal `@@` to the binary as the file to open for anyone whose
  entry came from `scripts/install-user.sh`, which copies it to the user prefix
  verbatim.
- The icon's own notes sit inside its `<svg>` element rather than above it.
  gdk-pixbuf identifies an image by sniffing the head of the file for the
  opening tag — 256 bytes on Ubuntu 24.04 — so a comment block in front of it
  made `appstreamcli compose` read a valid icon as an unrecognized format and
  fail the flatpak build ten minutes in. Every direct librsvg consumer,
  `rsvg-convert` in the AppImage build included, had taken the same file
  happily, which is why only one of the two packages ever complained.
