// The text rasterizer the wasm core calls for every visible text overlay:
// Canvas2D standing in for the desktop's Pango layout (src/ui/text.rs).

import type { TextOverlay } from "../wasm/gifkino_wasm.js";

/** Pango style words, lowercased, and the CSS `font` keyword each becomes. */
const STYLE_WORDS: Record<string, string> = {
  italic: "italic",
  oblique: "oblique",
  "small-caps": "small-caps",
  thin: "100",
  "ultra-light": "200",
  "extra-light": "200",
  light: "300",
  "semi-light": "350",
  "demi-light": "350",
  book: "380",
  normal: "",
  regular: "",
  roman: "",
  medium: "500",
  "semi-bold": "600",
  "demi-bold": "600",
  bold: "bold",
  "ultra-bold": "800",
  "extra-bold": "800",
  heavy: "900",
  black: "900",
  "ultra-condensed": "ultra-condensed",
  "extra-condensed": "extra-condensed",
  condensed: "condensed",
  "semi-condensed": "semi-condensed",
  "semi-expanded": "semi-expanded",
  expanded: "expanded",
  "extra-expanded": "extra-expanded",
  "ultra-expanded": "ultra-expanded",
};

const GENERIC_FAMILIES: Record<string, string> = {
  sans: "sans-serif",
  "sans-serif": "sans-serif",
  serif: "serif",
  monospace: "monospace",
  mono: "monospace",
};

/**
 * A Pango font description ("Sans Bold Italic 12") as a CSS `font` value at
 * `sizePx`. The description's own size is ignored, as on the desktop, which
 * sets an absolute size.
 */
export function cssFont(description: string, sizePx: number): string {
  const words = description.trim().split(/\s+/);
  if (words.length > 1 && /^\d+(\.\d+)?(px)?$/i.test(words[words.length - 1])) words.pop();
  const style: string[] = [];
  while (words.length > 1 && words[words.length - 1].toLowerCase() in STYLE_WORDS) {
    style.unshift(STYLE_WORDS[words.pop()!.toLowerCase()]);
  }
  const families = words
    .join(" ")
    .split(",")
    .map((family) => family.trim())
    .filter(Boolean)
    .map((family) => GENERIC_FAMILIES[family.toLowerCase()] ?? JSON.stringify(family));
  if (families.length === 0) families.push("sans-serif");
  return [...style.filter(Boolean), `${sizePx}px`, families.join(", ")].join(" ");
}

export interface Line {
  text: string;
  /** Last line of its paragraph: never justified. */
  last: boolean;
}

/**
 * Pango's WordChar wrapping: break between words, and inside a word only when
 * it does not fit a line by itself. Newlines start paragraphs.
 */
export function wrap(text: string, width: number, measure: (s: string) => number): Line[] {
  return text.split("\n").flatMap((paragraph) => {
    const lines = wrapParagraph(paragraph, width, measure);
    return lines.map((line, i) => ({ text: line, last: i === lines.length - 1 }));
  });
}

function wrapParagraph(paragraph: string, width: number, measure: (s: string) => number): string[] {
  const lines: string[] = [];
  let line = "";
  for (const word of paragraph.match(/\s*\S+\s*/g) ?? [""]) {
    if (measure((line + word).trimEnd()) <= width) {
      line += word;
      continue;
    }
    if (line) lines.push(line.trimEnd());
    line = "";
    for (const char of word) {
      if (line && measure((line + char).trimEnd()) > width) {
        lines.push(line.trimEnd());
        line = "";
      }
      line += char;
    }
  }
  lines.push(line.trimEnd());
  return lines;
}

/** Where each word of `line` starts so the words span `width`. */
function justify(line: string, width: number, measure: (s: string) => number) {
  const words = line.split(/\s+/).filter(Boolean);
  const inked = words.reduce((sum, word) => sum + measure(word), 0);
  const gap = words.length > 1 ? (width - inked) / (words.length - 1) : 0;
  let x = 0;
  return words.map((word) => {
    const placed = { word, x };
    x += measure(word) + gap;
    return placed;
  });
}

const css = ([r, g, b, a]: readonly number[]) => `rgba(${r}, ${g}, ${b}, ${a / 255})`;

let canvas: OffscreenCanvas | null = null;

/** `TextRasterizer`: the overlay's un-rotated box as `width*height*4` RGBA bytes. */
export function rasterizeText(
  overlay: TextOverlay,
  width: number,
  height: number,
): Uint8ClampedArray {
  if (width === 0 || height === 0) return new Uint8ClampedArray(0);
  canvas ??= new OffscreenCanvas(1, 1);
  canvas.width = width;
  canvas.height = height;
  const ctx = canvas.getContext("2d", { willReadFrequently: true })!;
  ctx.font = cssFont(overlay.font, overlay.sizePx);
  ctx.textBaseline = "alphabetic";
  const measure = (s: string) => ctx.measureText(s).width;

  const metrics = ctx.measureText("Mg");
  const ascent = metrics.fontBoundingBoxAscent;
  const lineHeight = ascent + metrics.fontBoundingBoxDescent;
  const lines = wrap(overlay.text, width, measure);
  // Centred vertically, but a block taller than the box keeps its first line.
  const top = Math.max(0, (height - lines.length * lineHeight) / 2);

  const paint = (draw: (text: string, x: number, y: number) => void) => {
    lines.forEach((line, i) => {
      const y = top + i * lineHeight + ascent;
      if (overlay.align === "justify" && !line.last) {
        for (const { word, x } of justify(line.text, width, measure)) draw(word, x, y);
        return;
      }
      const spare = width - measure(line.text);
      const x = { left: 0, justify: 0, center: spare / 2, right: spare }[overlay.align];
      draw(line.text, x, y);
    });
  };

  // Outline first, then the fill over every glyph, as Pango's stroke_preserve + fill.
  const [outlineColor, outlineWidth] = overlay.outline ?? [[0, 0, 0, 0], 0];
  if (outlineWidth > 0) {
    ctx.strokeStyle = css(outlineColor);
    ctx.lineWidth = outlineWidth * 2;
    ctx.lineJoin = "round";
    paint((text, x, y) => ctx.strokeText(text, x, y));
  }
  ctx.fillStyle = css(overlay.color);
  paint((text, x, y) => ctx.fillText(text, x, y));

  const pixels = ctx.getImageData(0, 0, width, height).data;
  if (!overlay.antialias) hardenEdges(pixels);
  return pixels;
}

/**
 * Canvas text is always antialiased; snapping alpha to 0 or 255 is the
 * nearest Canvas2D gets to Cairo's ANTIALIAS_NONE.
 */
function hardenEdges(pixels: Uint8ClampedArray) {
  for (let i = 3; i < pixels.length; i += 4) pixels[i] = pixels[i] < 128 ? 0 : 255;
}
