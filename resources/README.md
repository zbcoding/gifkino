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
