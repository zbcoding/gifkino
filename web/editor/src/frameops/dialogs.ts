// The desktop's frame-op dialogs (src/ui/window/dialogs.rs, imports.rs
// `fit_dialog`). Each resolves with what Apply chose, or `null` on Cancel.

import { byteSize } from "../format";
import { fill, t } from "../i18n";
import type { FitMode, Rgba8 } from "../wasm/gifkino_wasm.js";
import { alertDialog } from "../ui/dialog";
import type { DialogResponse } from "../ui/dialog";
import { h } from "../ui/dom";
import { actionRow, group, radioRows, spin, suffix } from "../ui/prefs";
import { resizeFits, frameBytes } from "./budget";
import { pairedDimension, plural } from "./summary";

const cancel: DialogResponse = { id: "cancel", label: t("Cancel") };
const apply: DialogResponse = { id: "apply", label: t("Apply"), suggested: true };

/** A dialog that only explains something: one Close button. */
function explain(title: string, body: string): Promise<null> {
  // Translators: Dismisses a dialog that only explains something.
  const close = { id: "close", label: t("Close") };
  return alertDialog({ title, body, responses: [close], close: "close" }).answer.then(() => null);
}

/** Title, body and one spin row; resolves with the spin's value on Apply. */
async function spinDialog(options: {
  title: string;
  body: string;
  row: string;
  range: [min: number, max: number, step: number];
  value: number;
  unit?: string;
  ok?: DialogResponse;
  enterApplies: boolean;
}): Promise<number | null> {
  const [min, max, step] = options.range;
  const field = spin(min, max, step, options.value);
  const content = group(
    null,
    actionRow(options.row, options.unit ? suffix(field.input, options.unit) : field.input),
  );
  const ok = options.ok ?? apply;
  const answer = await alertDialog({
    title: options.title,
    body: options.body,
    content,
    responses: [cancel, ok],
    close: "cancel",
    default: options.enterApplies ? ok.id : undefined,
  }).answer;
  return answer === ok.id ? field.value() : null;
}

/** Desktop `optimize_dialog(Remove)`: drop one frame in every N. */
export function removeFramesDialog(): Promise<number | null> {
  return spinDialog({
    title: t("Remove frames"),
    body: t(
      "Delete one frame out of every N. Total duration is preserved, so the result plays at the same speed with fewer frames in it.",
    ),
    row: t("Remove 1 frame in every"),
    range: [2, 60, 1],
    value: 2,
    // Translators: Unit after the "remove 1 frame in every N" field.
    unit: t("frames"),
    enterApplies: true,
  });
}

/** Desktop `optimize_dialog(Smart)`: drop the least-moving share of frames. */
export function smartRemoveDialog(): Promise<number | null> {
  return spinDialog({
    title: t("Smart remove frames"),
    body: t(
      "Drop the frames that change the least, so a still section thins out before a moving one does. Duration is preserved.",
    ),
    row: t("Remove this share of frames"),
    range: [5, 90, 5],
    value: 30,
    unit: "%",
    enterApplies: true,
  });
}

/** Desktop `delay_scope_dialog`: the scope's delay, seeded from the frame on screen. */
export function delayScopeDialog(scope: number[], delayCs: number): Promise<number | null> {
  return spinDialog({
    title: t("Frame delay"),
    // Translators: GIF stores frame delays in centiseconds.
    body: t("How long this frame is held, in hundredths of a second."),
    row:
      scope.length === 1
        ? fill(t("Frame {number}"), { number: scope[0] + 1 })
        : plural("{count} selected frame", "{count} selected frames", scope.length),
    range: [1, 65535, 1],
    value: Math.max(1, delayCs),
    enterApplies: false,
  });
}

/** Desktop `delay_all_dialog`: every frame's delay at once, seeded from the first. */
export function delayAllDialog(count: number, delayCs: number): Promise<number | null> {
  return spinDialog({
    title: t("Set delay for all frames"),
    // Translators: GIF stores frame delays in centiseconds.
    body: t("How long every frame is held, in hundredths of a second."),
    row: fill(t("All {count} frames"), { count }),
    range: [1, 65535, 1],
    value: Math.max(1, delayCs),
    enterApplies: false,
  });
}

/** Desktop `move_frame_dialog`: a 1-based target position for frame `from`; resolves 0-based. */
export async function moveFrameDialog(from: number, count: number): Promise<number | null> {
  const to = await spinDialog({
    title: t("Move frame"),
    body: t("Where should this frame go?"),
    row: fill(t("Frame {number}"), { number: from + 1 }),
    range: [1, count, 1],
    value: from + 1,
    ok: { id: "apply", label: t("Move"), suggested: true },
    enterApplies: false,
  });
  return to === null ? null : Math.min(to - 1, count - 1);
}

/** Desktop `FADE_COLOURS`, in the dialog's order; the first is the default. */
const FADE_COLOURS: [label: string, colour: Rgba8][] = [
  // Translators: A fade color.
  ["Black", [0, 0, 0, 255]],
  // Translators: A fade color.
  ["White", [255, 255, 255, 255]],
  // Translators: A fade color: the frames fade to see-through.
  ["Transparent", [0, 0, 0, 0]],
];

/**
 * Desktop `fade_dialog`: the colour the `count` frames in scope fade from or
 * to. With fewer than two there is nothing to ramp across, so it says how to
 * pick more instead.
 */
export async function fadeDialog(fade: "in" | "out", count: number): Promise<Rgba8 | null> {
  const title =
    fade === "in"
      ? // Translators: Dialog title; the frames in scope brighten in from a solid color.
        t("Fade in")
      : // Translators: Dialog title; the frames in scope sink out to a solid color.
        t("Fade out");
  if (count < 2) {
    return explain(
      title,
      // Translators: Shown when Fade in/out is picked with only one frame in scope.
      t(
        "Select the frames to fade across in the strip first: Shift+click for a run, or pick All frames.",
      ),
    );
  }
  const body =
    fade === "in"
      ? // Translators: {count} is a number of frames, at least 2.
        t("The {count} frames in scope climb from a solid color to the full picture.")
      : // Translators: {count} is a number of frames, at least 2.
        t("The {count} frames in scope sink from the full picture to a solid color.");
  const colours = radioRows(
    FADE_COLOURS.map(([label], i) => ({
      title: t(label),
      subtitle:
        i === 2
          ? // Translators: Subtitle on the Transparent fade color.
            t("A GIF pixel is either opaque or clear, so this fade shows as dithering.")
          : undefined,
    })),
  );
  const answer = await alertDialog({
    title,
    body: fill(body, { count }),
    content: colours.element,
    responses: [cancel, apply],
    close: "cancel",
    default: "apply",
  }).answer;
  return answer === "apply" ? FADE_COLOURS[colours.picked()][1] : null;
}

/** Desktop `loop_fade_dialog`: how many frames the loop cross-fade blends, up to `limit`. */
export function loopFadeDialog(limit: number): Promise<number | null> {
  // Translators: Dialog title for blending the end of the animation into its start.
  const title = t("Cross-fade the loop");
  if (limit === 0) {
    // Translators: Shown when the document has a single frame.
    return explain(title, t("A loop cross-fade needs at least two frames."));
  }
  return spinDialog({
    title,
    // Translators: Body of the loop cross-fade dialog.
    body: t(
      "The last frames dissolve into the first ones, so the end plays into the start without a cut. The first frames are then removed, since the dissolve already shows them: the animation gets that many frames shorter.",
    ),
    // Translators: How many frames at each end the loop cross-fade blends.
    row: t("Frames to blend"),
    range: [1, limit, 1],
    value: Math.min(limit, 10),
    enterApplies: true,
  });
}

/**
 * Desktop `resize_dialog`: a new size for every frame, the two fields linked
 * by "Keep aspect ratio", with the memory it will take; Apply is off while
 * that is over the budget.
 */
export async function resizeDialog(
  cw: number,
  ch: number,
  frames: number,
): Promise<[number, number] | null> {
  const width = spin(16, 8192, 1, cw);
  const height = spin(16, 8192, 1, ch);
  const lock = h("input", { type: "checkbox", role: "switch", checked: true });
  const summary = h("p", { class: "alert-summary" });
  const content = h(
    "div",
    {},
    group(
      null,
      // Translators: Abbreviation for pixels, shown after a pair of size fields.
      actionRow(t("New size"), suffix(width.input, "×", height.input, t("px"))),
      actionRow(t("Keep aspect ratio"), lock),
    ),
    summary,
  );
  const dialog = alertDialog({
    title: t("Resize"),
    body: t("Scale every frame, and the overlays with them."),
    content,
    responses: [cancel, apply],
    close: "cancel",
    default: "apply",
  });
  const refresh = () => {
    const [w, h] = [width.value(), height.value()];
    const fits = resizeFits(cw, ch, w, h, frames);
    const lines = [fill(t("{size} in memory"), { size: byteSize(frameBytes(w, h, frames)) })];
    if (!fits) lines.push(t("That resize would use more memory than the configured limit."));
    summary.textContent = lines.join("\n");
    dialog.setEnabled("apply", fits);
  };
  // Desktop `link_aspect_fields`: typing in one field recomputes the other
  // from the ratio the dialog opened at, live, while the lock is on.
  const link = (
    edited: typeof width,
    peer: typeof width,
    editedOther: number,
    fixedOther: number,
  ) =>
    edited.input.addEventListener("input", () => {
      const typed = Number(edited.input.value);
      if (lock.checked && edited.input.value !== "" && typed >= 0) {
        peer.set(pairedDimension(Math.round(typed), editedOther, fixedOther));
      }
      refresh();
    });
  link(width, height, cw, ch);
  link(height, width, ch, cw);
  for (const field of [width, height]) field.input.addEventListener("change", refresh);
  // Re-locking re-anchors the ratio: the height follows the width back onto it.
  lock.addEventListener("change", () => {
    if (lock.checked) height.set(pairedDimension(width.value(), cw, ch));
    refresh();
  });
  refresh();
  return (await dialog.answer) === "apply" ? [width.value(), height.value()] : null;
}

const FIT_MODES: FitMode[] = ["stretch", "pad", "growPad", "growStretch"];

/** Desktop `fit_labels`: what each fit does, in terms of the two sizes. */
function fitLabels(mode: FitMode, canvas: string, arriving: string): [string, string] {
  switch (mode) {
    case "stretch":
      return [
        t("Stretch what is coming in"),
        // Translators: {canvas} is a size like "480 × 320".
        fill(t("Fills {canvas}, changing its aspect ratio."), { canvas }),
      ];
    case "pad":
      return [
        t("Scale what is coming in to fit"),
        fill(t("Keeps its aspect ratio inside {canvas}; transparency fills the rest."), { canvas }),
      ];
    case "growPad":
      return [
        t("Resize this animation to fit"),
        // Translators: {arriving} is the size of the file being added.
        fill(
          t(
            "The canvas becomes {arriving}; every existing frame keeps its aspect ratio, with transparency around it.",
          ),
          { arriving },
        ),
      ];
    case "growStretch":
      return [
        t("Resize this animation and stretch it"),
        fill(t("The canvas becomes {arriving}; every existing frame is stretched to it."), {
          arriving,
        }),
      ];
  }
}

/**
 * Desktop `fit_dialog` ("Sizes do not match"): frames arriving at another
 * size than the canvas, and the four ways to make them one canvas.
 */
export async function fitDialog(
  file: string,
  canvas: [number, number],
  arriving: [number, number],
): Promise<FitMode | null> {
  const [canvasText, arrivingText] = [canvas, arriving].map(([w, h]) => `${w} × ${h}`);
  const options = radioRows(
    FIT_MODES.map((mode) => {
      const [title, subtitle] = fitLabels(mode, canvasText, arrivingText);
      return { title, subtitle };
    }),
  );
  const answer = await alertDialog({
    title: t("Sizes do not match"),
    body: fill(
      // Translators: {file} is a filename; {arriving} and {canvas} are sizes like "800 × 600".
      t("{file} is {arriving}, and this animation is {canvas}. Pick how they fit together."),
      { file, arriving: arrivingText, canvas: canvasText },
    ),
    content: options.element,
    // Translators: Confirms the fit chooser and splices the frames in.
    responses: [cancel, { id: "add", label: t("Add frames"), suggested: true }],
    close: "cancel",
    default: "add",
  }).answer;
  return answer === "add" ? FIT_MODES[options.picked()] : null;
}
