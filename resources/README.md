# resources

`rotate-handle.svg` is Impasto's rotate cursor, taken verbatim from
`Pinta.Resources/icons/hicolor/scalable/actions/rotate-handle.svg`. It is the
non-symbolic icon on purpose: a symbolic one is recoloured to a single colour by
GTK, which costs the white halo that keeps the glyph readable over any frame.

`rotate-handle.png` is that file at 32×32, which is what the binary embeds. GTK
has no CSS cursor name for rotation, so the glyph has to travel as a texture,
and `gdk::Texture::from_bytes` reads PNG but not SVG. Re-render it after editing
the SVG:

```bash
rsvg-convert -w 32 -h 32 resources/rotate-handle.svg -o resources/rotate-handle.png
```

## Tool icons


`icons/scalable/actions/*-symbolic.svg` are the left-rail tool glyphs (text,
rectangle, ellipse, arrow, crop). They are original artwork, not ported from
Impasto/Pinta: Impasto's own icon set mixes in third-party glyphs (e.g. its
`tool-rectangle`/`tool-ellipse`/crop icons are Material Design Icons, Apache-2.0)
with unclear per-file provenance, so copying its paths verbatim would need
per-icon license auditing. We only reused the *convention* documented in
`ImpastoPaint-public/ICONS.md`: `viewBox="0 0 24 24"`, single `fill="#bebebe"`,
filled paths with no stroke, bold shapes edge-to-edge (roughly 1–23). That
`#bebebe` fill is the magic color GTK recolors `-symbolic` icons by — GTK, not
the SVG, decides the on-screen color (theme foreground, hover, insensitive,
high contrast), so it must stay exactly that value.

`icons.gresource` is those five SVGs compiled with `glib-compile-resources`,
which `src/ui/window.rs` embeds with `include_bytes!` and registers at startup
(`gio::resources_register` + `IconTheme::add_resource_path`) so
`gtk::Button::from_icon_name("tool-text-symbolic")` finds them like any stock
Adwaita icon. There is no build-time compile step (no `build.rs`); like
`rotate-handle.png`, re-run this by hand after editing an SVG:

```bash
glib-compile-resources --sourcedir=resources/icons resources/icons/icons.gresource.xml \
  --target=resources/icons/icons.gresource
```
