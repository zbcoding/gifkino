// The colour control behind every colour row in the overlay editor: desktop
// `ui/color.rs`. A button showing the colour opens a popover with quick
// colours, a hue/saturation wheel, brightness and opacity sliders and a hex
// box. The angle on the wheel is the hue, the distance from the centre the
// saturation.

import type { Rgba8 } from "../wasm/gifkino_wasm.js";
import { t } from "../i18n";
import { h, icon } from "../ui/dom";

/** Side of the wheel; it sets the popover's width. */
const WHEEL = 168;
const RADIUS = WHEEL / 2;

/** A row of hues over a row of neutrals and darks. */
export const QUICK: Rgba8[] = [
  [255, 60, 60, 255],
  [255, 138, 0, 255],
  [255, 212, 0, 255],
  [60, 208, 112, 255],
  [0, 196, 204, 255],
  [60, 120, 255, 255],
  [138, 76, 255, 255],
  [255, 76, 192, 255],
  [255, 255, 255, 255],
  [200, 200, 200, 255],
  [128, 128, 128, 255],
  [64, 64, 64, 255],
  [0, 0, 0, 255],
  [122, 74, 30, 255],
  [10, 92, 58, 255],
  [20, 40, 90, 255],
];

// Symbolic glyphs after display-brightness-symbolic and view-reveal-symbolic.
const BRIGHTNESS =
  "M8 4.5a3.5 3.5 0 1 1 0 7zM8 3a5 5 0 1 0 0 10A5 5 0 0 0 8 3zM7 0h2v2H7zm0 14h2v2H7zM0 7h2v2H0zm14 0h2v2h-2zM2.1 3.5l1.4-1.4L4.9 3.5 3.5 4.9zm9 9 1.4-1.4 1.4 1.4-1.4 1.4zM2.1 12.5l1.4 1.4 1.4-1.4-1.4-1.4zm9-9 1.4 1.4 1.4-1.4-1.4-1.4z";
const OPACITY =
  "M8 3C4 3 1.2 5.6 0 8c1.2 2.4 4 5 8 5s6.8-2.6 8-5c-1.2-2.4-4-5-8-5zm0 2a3 3 0 1 1 0 6 3 3 0 0 1 0-6zm0 1.5a1.5 1.5 0 1 0 0 3 1.5 1.5 0 0 0 0-3z";

export interface ColorPick {
  /** The button to pack. */
  element: HTMLElement;
  rgba(): Rgba8;
  /** Shows the colour the document holds; never reports back. */
  set(colour: Rgba8): void;
}

/** `onChange` runs for every colour the user picks, and for nothing else. */
export function colorPick(onChange: (colour: Rgba8) => void): ColorPick {
  // HSV is stored rather than read back from RGB, so dragging into the grey
  // centre or the brightness down to black keeps the hue.
  let hsv: [number, number, number] = [0, 0, 0];
  let alpha = 255;
  const rgba = (): Rgba8 => [...hsvToRgb(...hsv), alpha];

  const preview = h("span", { class: "color-preview" });
  const button = h("button", { class: "color-button", type: "button" }, preview);
  const wheel = h("canvas", { class: "color-wheel", width: String(WHEEL), height: String(WHEEL) });
  const value = h("input", { type: "range", min: "0", max: "1", step: "0.01" });
  const opacity = h("input", { type: "range", min: "0", max: "255", step: "1" });
  const hex = h("input", { class: "color-hex", placeholder: "#RRGGBB", size: "9", maxlength: "9" });
  const quick = h("div", { class: "color-quick" });
  const popover = h(
    "div",
    { class: "color-popover", popover: "auto" },
    quick,
    wheel,
    sliderRow(BRIGHTNESS, t("Brightness"), value),
    // Translators: How see-through the colour is, 0 to 255.
    sliderRow(OPACITY, t("Opacity"), opacity),
    hex,
  );
  const element = h("span", { class: "color-pick" }, button, popover);

  const refresh = () => {
    const colour = rgba();
    value.value = String(hsv[2]);
    opacity.value = String(alpha);
    hex.value = hexOf(colour);
    button.title = hexOf(colour);
    paintSwatch(preview, colour);
    drawWheel(wheel, hsv);
  };
  const pick = (next: [number, number, number], nextAlpha: number) => {
    hsv = next;
    alpha = nextAlpha;
    refresh();
    onChange(rgba());
  };

  for (const colour of QUICK) {
    const patch = h("span", { class: "color-patch" });
    paintSwatch(patch, colour);
    const [r, g, b] = colour;
    // Every quick colour is opaque, so taking one keeps the opacity slider's value.
    quick.append(
      h(
        "button",
        {
          class: "color-swatch",
          type: "button",
          title: hexOf(colour),
          onclick: () => pick(rgbToHsv(r, g, b), alpha),
        },
        patch,
      ),
    );
  }

  const pickAt = (event: PointerEvent) => {
    const dx = event.offsetX - RADIUS;
    const dy = event.offsetY - RADIUS;
    pick([hueAt(dx, dy), Math.min(1, Math.hypot(dx, dy) / RADIUS), hsv[2]], alpha);
  };
  wheel.addEventListener("pointerdown", (event) => {
    wheel.setPointerCapture(event.pointerId);
    pickAt(event);
  });
  wheel.addEventListener("pointermove", (event) => {
    if (wheel.hasPointerCapture(event.pointerId)) pickAt(event);
  });
  value.addEventListener("input", () => pick([hsv[0], hsv[1], Number(value.value)], alpha));
  opacity.addEventListener("input", () => pick(hsv, Math.round(Number(opacity.value))));
  hex.addEventListener("keydown", (event) => {
    if (event.key !== "Enter") return;
    const parsed = parseHex(hex.value);
    // Put the colour back rather than leave the box disagreeing with the rest.
    if (!parsed) return refresh();
    pick(rgbToHsv(parsed[0], parsed[1], parsed[2]), parsed[3]);
  });

  button.addEventListener("click", () => {
    const below = button.getBoundingClientRect();
    popover.showPopover();
    const width = popover.offsetWidth;
    popover.style.left = `${Math.max(4, Math.min(innerWidth - width - 4, below.right - width))}px`;
    // Below the button, or above it when the window ends first.
    const fitsBelow = below.bottom + 4 + popover.offsetHeight <= innerHeight;
    popover.style.top = `${fitsBelow ? below.bottom + 4 : Math.max(4, below.top - 4 - popover.offsetHeight)}px`;
  });

  refresh();
  return {
    element,
    rgba,
    set(colour) {
      const current = rgba();
      if (current.every((channel, i) => channel === colour[i])) return;
      hsv = rgbToHsv(colour[0], colour[1], colour[2]);
      alpha = colour[3];
      refresh();
    },
  };
}

function sliderRow(glyph: string, tooltip: string, slider: HTMLInputElement): HTMLElement {
  const image = icon(glyph);
  const label = h("span", { class: "color-slider-icon", title: tooltip }, image);
  return h("div", { class: "color-slider" }, label, slider);
}

/** A flat patch of the colour, over a checkerboard when it is see-through. */
function paintSwatch(patch: HTMLElement, [r, g, b, a]: Rgba8): void {
  const fill = `rgb(${r} ${g} ${b} / ${a / 255})`;
  patch.style.background =
    a < 255
      ? `linear-gradient(${fill}, ${fill}), repeating-conic-gradient(#999 0 25%, #d9d9d9 0 50%) 0 0 / 10px 10px`
      : fill;
}

function drawWheel(canvas: HTMLCanvasElement, [hue, saturation, value]: [number, number, number]) {
  const context = canvas.getContext("2d")!;
  const image = context.createImageData(WHEEL, WHEEL);
  for (let y = 0; y < WHEEL; y++) {
    for (let x = 0; x < WHEEL; x++) {
      const dx = x - RADIUS;
      const dy = y - RADIUS;
      const distance = Math.hypot(dx, dy);
      if (distance > RADIUS) continue;
      const [r, g, b] = hsvToRgb(hueAt(dx, dy), distance / RADIUS, value);
      const at = (y * WHEEL + x) * 4;
      image.data[at] = r;
      image.data[at + 1] = g;
      image.data[at + 2] = b;
      // Fade the outermost pixel, or the rim is a staircase.
      image.data[at + 3] = 255 * Math.min(1, RADIUS - distance);
    }
  }
  context.putImageData(image, 0, 0);
  // The ring marking the colour in hand: light inside dark, visible on both
  // the pale centre and the saturated rim.
  const angle = (hue / 360) * 2 * Math.PI - Math.PI;
  const cx = RADIUS - saturation * RADIUS * Math.cos(angle);
  const cy = RADIUS + saturation * RADIUS * Math.sin(angle);
  context.lineWidth = 1.5;
  context.strokeStyle = "#fff";
  context.beginPath();
  context.arc(cx, cy, 4.5, 0, 2 * Math.PI);
  context.stroke();
  context.strokeStyle = "rgb(0 0 0 / 0.6)";
  context.beginPath();
  context.arc(cx, cy, 6, 0, 2 * Math.PI);
  context.stroke();
}

/** Where the wheel puts a hue, from an offset off its centre. */
export function hueAt(dx: number, dy: number): number {
  return ((Math.atan2(dy, -dx) + Math.PI) / (2 * Math.PI)) * 360;
}

/** Opaque colours are written without their alpha: the six digits people paste. */
export function hexOf([r, g, b, a]: Rgba8): string {
  const byte = (v: number) => v.toString(16).toUpperCase().padStart(2, "0");
  return `#${byte(r)}${byte(g)}${byte(b)}${a === 255 ? "" : byte(a)}`;
}

export function parseHex(text: string): Rgba8 | null {
  const digits = text.trim().replace(/^#+/, "");
  if (!/^([0-9a-f]{6}|[0-9a-f]{8})$/i.test(digits)) return null;
  const byte = (at: number) => parseInt(digits.slice(at, at + 2), 16);
  return [byte(0), byte(2), byte(4), digits.length === 8 ? byte(6) : 255];
}

export function hsvToRgb(hue: number, saturation: number, value: number): [number, number, number] {
  const sector = (((hue % 360) + 360) % 360) / 60;
  const chroma = value * Math.min(1, Math.max(0, saturation));
  const rise = chroma * (1 - Math.abs((sector % 2) - 1));
  const [r, g, b] = [
    [chroma, rise, 0],
    [rise, chroma, 0],
    [0, chroma, rise],
    [0, rise, chroma],
    [rise, 0, chroma],
    [chroma, 0, rise],
  ][Math.min(5, Math.floor(sector))];
  const floor = Math.min(1, Math.max(0, value)) - chroma;
  const byte = (channel: number) => Math.min(255, Math.max(0, Math.round((channel + floor) * 255)));
  return [byte(r), byte(g), byte(b)];
}

export function rgbToHsv(r8: number, g8: number, b8: number): [number, number, number] {
  const [r, g, b] = [r8 / 255, g8 / 255, b8 / 255];
  const value = Math.max(r, g, b);
  const span = value - Math.min(r, g, b);
  let hue = 0;
  if (span !== 0 && value === r) hue = 60 * ((g - b) / span);
  else if (span !== 0 && value === g) hue = 60 * ((b - r) / span + 2);
  else if (span !== 0) hue = 60 * ((r - g) / span + 4);
  const saturation = value === 0 ? 0 : span / value;
  return [((hue % 360) + 360) % 360, saturation, value];
}
