// Desktop `confirm_oversize` (ui/window/imports.rs): every video stops here
// to pick the size, rate, trim, frame drop and this import's memory budget,
// with a live preview of what they land and a measured GIF size.

import { fill, t } from "../i18n";
import { alertDialog } from "../ui/dialog";
import { h } from "../ui/dom";
import { actionRow, group, setSubtitle, spin, suffix } from "../ui/prefs";
import type { Spin } from "../ui/prefs";
import type { ImportPlan, VideoInfo } from "./plan";
import {
  defaultOptions,
  overBudget,
  planFor,
  planFrames,
  readTrim,
  sizePresets,
  steppedTrim,
  trimExamples,
} from "./plan";
import { extent, importBody, planSummary } from "./summary";

/** Desktop `MEASURE_DEBOUNCE`: settings settle before ffmpeg is spent on a size. */
const MEASURE_DEBOUNCE_MS = 5000;

export interface ImportChoice {
  plan: ImportPlan;
  /** The budget the plan was measured against: the configured one, or the lifted one. */
  budget: number;
}

export interface ImportDialogOptions {
  name: string;
  /** The automatic plan: its size is the recommendation. */
  plan: ImportPlan;
  budget: number;
  /** The budget "Ignore the memory limit" lifts this import to. */
  liftedBudget: number;
  /** The GIF size of `plan`, in bytes (desktop `estimate_gif_size`). */
  measure(plan: ImportPlan): Promise<number>;
}

export async function importDialog(options: ImportDialogOptions): Promise<ImportChoice | null> {
  const { plan: recommended, budget } = options;
  const source = recommended.source;

  // "Import at": the plan's own size first, then the presets, then Custom.
  const sizes: [number, number][] = [
    [recommended.width, recommended.height],
    ...sizePresets(source).filter(
      ([w, ht]) => w !== recommended.width || ht !== recommended.height,
    ),
  ];
  const presets = h(
    "select",
    {},
    ...sizes.map(([w, ht], i) => {
      const size = `${w} × ${ht}`;
      return h(
        "option",
        { value: String(i) },
        i === 0 ? fill(t("{size} (recommended)"), { size }) : size,
      );
    }),
    h("option", { value: "custom" }, t("Custom…")),
  );
  const width = spin(16, Math.max(source.width, 16), 2, recommended.width);
  const height = spin(16, Math.max(source.height, 16), 2, recommended.height);
  const lock = h("input", { type: "checkbox", role: "switch", checked: true });
  // A preset is picked to start with, so the fields it fills start locked.
  for (const field of [width.input, height.input, lock]) field.disabled = true;
  const rate = spin(
    1,
    Math.min(Math.max(source.fps, 1), 50),
    1,
    Math.max(Math.floor(recommended.fps), 1),
  );

  const advanced = advancedRows(source);
  const summary = h("p", { class: "import-summary" });
  const content = h(
    "div",
    {},
    h("p", { class: "alert-body" }, ...bodyNodes(options.name, recommended)),
    summary,
    group(
      null,
      actionRow(t("Import at"), presets),
      // Translators: Abbreviation for pixels, shown after a pair of size fields.
      actionRow(t("Size"), suffix(width.input, "×", height.input, t("px"))),
      actionRow(t("Keep aspect ratio"), lock),
      // Translators: Abbreviation for frames per second.
      actionRow(t("Frame rate"), suffix(rate.input, t("fps"))),
      advanced.element,
    ),
  );
  const dialog = alertDialog({
    title: overBudget(recommended) ? t("This video is large") : t("Import video"),
    content,
    responses: [
      { id: "cancel", label: t("Cancel") },
      { id: "import", label: t("Import"), suggested: true },
    ],
    close: "cancel",
    default: "cancel",
  });
  content.closest("dialog")?.classList.add("wide");

  let chosen: ImportChoice = { plan: recommended, budget };
  // A measurement belongs to the settings it was taken at.
  let generation = 0;
  const refresh = () => {
    const startedAt = ++generation;
    const trim = advanced.trim();
    const chosenBudget = advanced.ignoreLimit.checked ? options.liftedBudget : budget;
    const plan = planFor(source, {
      ...defaultOptions(chosenBudget),
      target: [width.value(), height.value()],
      fps: rate.value(),
      trim: trim.kind === "set" ? trim.trim : null,
      dropNth: advanced.dropNth.value(),
    });
    chosen = { plan, budget: chosenBudget };
    summary.textContent = planSummary(plan, null);
    const fits = !overBudget(plan) && trim.kind !== "invalid";
    dialog.setEnabled("import", fits);
    if (!fits || planFrames(plan) === null) return;
    setTimeout(() => {
      if (generation !== startedAt) return;
      options.measure(plan).then(
        (bytes) => {
          if (generation === startedAt) summary.textContent = planSummary(plan, bytes);
        },
        (error: unknown) => {
          if (generation !== startedAt) return;
          const reason = fill(t("Could not measure the size: {error}"), { error: String(error) });
          summary.textContent = `${planSummary(plan, null)}\n${reason}`;
        },
      );
    }, MEASURE_DEBOUNCE_MS);
  };

  // Only Custom lets the size fields and the aspect lock be edited; a preset
  // writes its own pair into both.
  const syncPreset = () => {
    const pair = sizes[Number(presets.value)];
    if (pair) {
      width.set(pair[0]);
      height.set(pair[1]);
    }
    for (const field of [width.input, height.input, lock]) field.disabled = pair !== undefined;
    refresh();
  };
  presets.addEventListener("change", syncPreset);
  linkAspect(width, height, lock, source, refresh);
  for (const field of [rate.input, advanced.dropNth.input]) {
    field.addEventListener("input", refresh);
    field.addEventListener("change", refresh);
  }
  advanced.onChange(refresh);
  syncPreset();

  const answer = await dialog.answer;
  generation += 1;
  return answer === "import" ? chosen : null;
}

/** The opening line, the file name in bold; the over-budget advice under it. */
function bodyNodes(name: string, plan: ImportPlan): (Node | string)[] {
  const { opening, advice } = importBody(plan);
  const [before, rest = ""] = opening.split("<b>");
  const [bold, after = ""] = rest.split("</b>");
  const nodes: (Node | string)[] = [before, h("b", {}, fill(bold, { name })), after];
  if (advice) nodes.push(h("br"), h("br"), advice);
  return nodes;
}

/**
 * Desktop `link_aspect_fields` over the source's ratio, with the peer snapped
 * to the fields' even step. Re-locking re-anchors the height on the width.
 */
function linkAspect(
  width: Spin,
  height: Spin,
  lock: HTMLInputElement,
  source: VideoInfo,
  refresh: () => void,
) {
  const paired = (edited: number, editedOther: number, fixedOther: number) => {
    const value = editedOther === 0 ? edited : (edited * fixedOther) / editedOther;
    return 16 + Math.round((value - 16) / 2) * 2;
  };
  const link = (edited: Spin, peer: Spin, editedOther: number, fixedOther: number) => {
    const relink = () => {
      const typed = Number(edited.input.value);
      if (lock.checked && edited.input.value !== "" && typed >= 0) {
        peer.set(paired(typed, editedOther, fixedOther));
      }
      refresh();
    };
    edited.input.addEventListener("input", relink);
    edited.input.addEventListener("change", relink);
  };
  link(width, height, source.width, source.height);
  link(height, width, source.height, source.width);
  lock.addEventListener("change", () => {
    if (lock.checked) height.set(paired(width.value(), source.width, source.height));
    refresh();
  });
}

/**
 * Desktop `advanced_import_rows`: which part of the file comes in, how many
 * of its frames are kept, and how much memory this one import may spend.
 */
function advancedRows(source: VideoInfo) {
  const units = h(
    "select",
    {},
    h("option", { value: "seconds" }, t("Seconds")),
    h("option", { value: "frames" }, t("Frames")),
  );
  const inFrames = () => units.value === "frames";
  const start = h("input", { type: "text", class: "trim-field", spellcheck: "false" });
  const end = h("input", { type: "text", class: "trim-field", spellcheck: "false" });
  const setExamples = () => {
    [start.placeholder, end.placeholder] = trimExamples(inFrames(), source);
  };
  setExamples();
  const changed: (() => void)[] = [];
  const notify = () => changed.forEach((listener) => listener());
  const trimRow = (title: string, field: HTMLInputElement, isEnd: boolean) => {
    const step = (forward: boolean) => {
      field.value = steppedTrim(field.value, forward, inFrames(), isEnd, source);
      notify();
    };
    const steppers = h(
      "span",
      { class: "linked" },
      // Translators: Tooltip on the trim field's minus button. The step is a second, or ten frames.
      h(
        "button",
        { type: "button", title: t("One step earlier"), onclick: () => step(false) },
        "−",
      ),
      // Translators: Tooltip on the trim field's plus button.
      h("button", { type: "button", title: t("One step later"), onclick: () => step(true) }, "+"),
    );
    field.addEventListener("input", notify);
    return actionRow(title, suffix(field, steppers));
  };
  units.addEventListener("change", () => {
    setExamples();
    notify();
  });

  const dropNth = spin(1, 60, 1, 1);
  const ignoreLimit = h("input", { type: "checkbox", role: "switch" });
  ignoreLimit.addEventListener("change", notify);

  const element = h(
    "details",
    { class: "pref-expander" },
    h("summary", { class: "pref-row" }, t("Advanced")),
    actionRow(t("This video"), h("span", { class: "dim selectable" }, extent(source))),
    helpedRow(
      t("Trim by"),
      // Translators: Help for the trim fields. A GIF's delays are whole centiseconds, so hundredths are the finest a cut can be.
      t(
        "Leave both blank for the whole clip. Seconds read 0:01.12 — a GIF holds hundredths, so that is as fine as a cut gets. Frames counts frame numbers, both ends included.",
      ),
      units,
    ),
    trimRow(t("From"), start, false),
    trimRow(t("To"), end, true),
    helpedRow(
      t("Drop one frame in every"),
      t(
        "1 keeps every frame. More drops one frame in every N on the way in, so those frames never cost memory and the ones kept still cover the same stretch of time.",
      ),
      // Translators: The unit after the frame-drop field: "Drop one frame in every 3 frames".
      suffix(dropNth.input, t("frames")),
    ),
    helpedRow(
      t("Ignore the memory limit"),
      t(
        "Frames are held uncompressed in memory while you edit, which is what the limit counts. This lets one import past it; the configured limit is left alone.",
      ),
      ignoreLimit,
    ),
  );

  return {
    element,
    dropNth,
    ignoreLimit,
    onChange: (listener: () => void) => changed.push(listener),
    /** The trim the fields say; every non-empty field turns red while it does not parse. */
    trim() {
      const trim = readTrim(inFrames(), start.value, end.value, source);
      for (const field of [start, end]) {
        field.classList.toggle("error", trim.kind === "invalid" && field.value.trim() !== "");
      }
      return trim;
    },
  };
}

/** Desktop `helped_row`: a question mark after the title that shows what the row does. */
function helpedRow(title: string, explanation: string, control: Node): HTMLElement {
  const row = actionRow(title, control);
  const help = h("button", { type: "button", class: "help flat", "aria-label": t("Help") }, "?");
  help.addEventListener("click", (event) => {
    event.preventDefault();
    const shown = row.querySelector(".pref-subtitle")!.textContent !== "";
    setSubtitle(row, shown ? "" : explanation);
  });
  const titleLabel = row.querySelector(".pref-title")!;
  const heading = h("span", { class: "help-heading" });
  titleLabel.replaceWith(heading);
  heading.append(titleLabel, help);
  return row;
}
