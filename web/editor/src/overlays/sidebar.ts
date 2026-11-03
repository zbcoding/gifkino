// The sidebar groups this slice owns, in the desktop's order under the frame
// group: "Overlays on this frame" (the layer list), "Properties" (the text of
// a text overlay) and "Overlay" (the picked overlay's style). Desktop
// `layer_list`, `text_panel`, `overlay_editor` and their `sync_*` in view.rs.

import type { App } from "../app";
import { fill, t, tn } from "../i18n";
import { framesInScope } from "../state";
import type { AppState } from "../state";
import { h, icon } from "../ui/dom";
import { group, spin, suffix } from "../ui/prefs";
import type {
  OverlayKind,
  Rgba8,
  ShapeOverlay,
  TextAlign,
  TextOverlay,
} from "../wasm/gifkino_wasm.js";
import type { Actions } from "./actions";
import { editingOverlay, stackedOverlays } from "./actions";
import { colorPick } from "./color";
import { fontField } from "./font";
import { ALIGN, CLOSE, GO_DOWN, GO_UP, TRASH } from "./icons";
import { cropArmed } from "./local";

const DEFAULT_SHAPE_COLOUR: Rgba8 = [255, 60, 60, 255];

export function installSidebar(app: App, actions: Actions): void {
  app.registerPanel({
    slot: "sidebar",
    id: "layers",
    order: 10,
    element: createLayerList(app, actions),
  });
  const editor = createOverlayEditor(app, actions);
  app.registerPanel({ slot: "sidebar", id: "text", order: 20, element: editor.text });
  app.registerPanel({ slot: "sidebar", id: "overlay", order: 30, element: editor.overlay });
}

const multiFrame = (state: AppState) => framesInScope(state).length > 1;

/**
 * The overlays on the frame on screen, topmost first, each with its two
 * z-order steps and a quick delete acting on its own row. Hidden while the
 * scope names several frames: which overlays sit on "the" frame has no single
 * answer then.
 */
function createLayerList(app: App, actions: Actions): HTMLElement {
  const rows = h("div", { class: "pref-group layer-list", role: "listbox" });
  const panel = group(t("Overlays on this frame"));
  panel.querySelector(".pref-group")!.replaceWith(rows);
  let shown = "";
  const step = (glyph: string, tooltip: string, enabled: boolean, run: () => void) => {
    const button = h(
      "button",
      {
        type: "button",
        class: "flat icon-button",
        title: tooltip,
        onclick: (event: Event) => {
          event.stopPropagation();
          run();
        },
      },
      icon(glyph),
    );
    button.disabled = !enabled;
    return button;
  };
  const sync = () => {
    const { state } = app;
    const stacked = stackedOverlays(state);
    panel.hidden = multiFrame(state) || cropArmed(app) || stacked.length === 0;
    const key = stacked.map((overlay) => `${overlay.id}:${overlay.name}`).join("|");
    if (key !== shown) {
      shown = key;
      const bottom = stacked.length - 1;
      rows.replaceChildren(
        ...stacked.map(({ id, name }, row) =>
          h(
            "div",
            {
              class: "pref-row layer-row",
              role: "option",
              "data-id": String(id),
              onclick: () => actions.pick(id),
            },
            h("span", { class: "pref-title" }, name),
            h(
              "span",
              { class: "pref-suffix" },
              step(GO_UP, t("Move overlay up"), row > 0, () => void actions.restack(id, true)),
              step(
                GO_DOWN,
                t("Move overlay down"),
                row < bottom,
                () => void actions.restack(id, false),
              ),
              step(CLOSE, t("Delete overlay"), true, () => void actions.deleteOverlay(id)),
            ),
          ),
        ),
      );
    }
    for (const row of rows.children as HTMLCollectionOf<HTMLElement>) {
      const selected = Number(row.dataset.id) === state.selectedOverlay;
      row.classList.toggle("selected", selected);
      row.setAttribute("aria-selected", String(selected));
      // The pick may have come from the canvas or a band, naming a row scrolled
      // past: the shortest scroll of the list alone (desktop `show_row`).
      if (selected && row.offsetTop < rows.scrollTop) rows.scrollTop = row.offsetTop;
      const below = row.offsetTop + row.offsetHeight - rows.clientHeight;
      if (selected && below > rows.scrollTop) rows.scrollTop = below;
    }
  };
  sync();
  app.store.subscribe(sync);
  return panel;
}

interface OverlayEditor {
  text: HTMLElement;
  overlay: HTMLElement;
}

function createOverlayEditor(app: App, actions: Actions): OverlayEditor {
  const restyleText = (change: (text: TextOverlay) => Partial<TextOverlay>) =>
    actions.restyle((kind) => (kind.type === "text" ? { ...kind, ...change(kind) } : kind));
  const restyleShape = (change: (shape: ShapeOverlay) => Partial<ShapeOverlay>) =>
    actions.restyle((kind) => (kind.type === "shape" ? { ...kind, ...change(kind) } : kind));
  let kind: OverlayKind | undefined;

  // "Properties": the text itself.
  const entry = h("input", { type: "text", class: "overlay-text", placeholder: t("Overlay text") });
  entry.addEventListener("input", () => void actions.editText(entry.value));
  const textRow = row(t("Text"), entry);
  const textGroup = group(t("Properties"), textRow);

  // "Overlay": the style.
  const font = fontField((description) => void restyleText(() => ({ font: description })));
  const size = spin(4, 512, 1, 32);
  size.input.addEventListener("change", () => void restyleText(() => ({ sizePx: size.value() })));

  const alignButtons = (["left", "center", "right", "justify"] as TextAlign[]).map((align) => {
    const tips = {
      left: t("Left"),
      center: t("Centered"),
      right: t("Right"),
      // Translators: Text alignment that stretches every line but the last to both margins.
      justify: t("Justified"),
    };
    const button = h(
      "button",
      {
        type: "button",
        class: "icon-button",
        title: tips[align],
        onclick: () => void restyleText(() => ({ align })),
      },
      icon(ALIGN[align]),
    );
    return { align, button };
  });

  const antialias = h("input", { type: "checkbox", role: "switch" });
  antialias.addEventListener(
    "change",
    () => void restyleText(() => ({ antialias: antialias.checked })),
  );

  // Fill: a switch for shapes, where a fill is optional, and the colour.
  const fillOn = h("input", { type: "checkbox", role: "switch" });
  const fillColour = colorPick((colour) => {
    if (kind?.type === "text") void restyleText(() => ({ color: colour }));
    else void restyleShape(() => ({ fill: fillOn.checked ? colour : null }));
  });
  fillOn.addEventListener(
    "change",
    () => void restyleShape(() => ({ fill: fillOn.checked ? fillColour.rgba() : null })),
  );

  // A width of zero is how the outline and the stroke are turned off.
  const widthPair = (emit: (pair: [Rgba8, number]) => void) => {
    const width = spin(0, 40, 1, 0);
    width.input.classList.add("narrow");
    const colour = colorPick((picked) => emit([picked, width.value()]));
    width.input.addEventListener("change", () => emit([colour.rgba(), width.value()]));
    return { width, colour };
  };
  const outline = widthPair((pair) => void restyleText(() => ({ outline: pair })));
  const stroke = widthPair((pair) => void restyleShape(() => ({ stroke: pair })));

  const fontRow = row(t("Font"), font.element);
  const sizeRow = row(t("Size"), size.input);
  const alignRow = row(
    t("Align"),
    h("span", { class: "linked" }, ...alignButtons.map((a) => a.button)),
  );
  // Translators: The interior colour of a shape, or the colour of the glyphs for text.
  const fillRow = row(t("Fill"), suffix(fillOn, fillColour.element));
  // Translators: The contrasting edge drawn behind text so it stays readable over any image.
  const outlineRow = row(t("Outline"), suffix(outline.width.input, outline.colour.element));
  // Translators: Smoothing of glyph edges. Off gives hard pixel edges, for pixel-art captures.
  const antialiasRow = row(t("Smooth edges"), antialias);
  // Translators: The outline of a shape, as distinct from the outline behind text.
  const strokeRow = row(t("Stroke"), suffix(stroke.width.input, stroke.colour.element));
  const textRows = [fontRow, sizeRow, alignRow, outlineRow, antialiasRow];

  const overlayGroup = group(
    t("Overlay"),
    fontRow,
    sizeRow,
    alignRow,
    fillRow,
    outlineRow,
    antialiasRow,
    strokeRow,
  );
  // Deleting belongs with the controls that edit it; destructive, so red and apart.
  const trash = h(
    "button",
    {
      type: "button",
      class: "flat icon-button destructive",
      title: t("Delete overlay"),
      onclick: () => void app.runCommand("overlay-delete"),
    },
    icon(TRASH),
  );
  const heading = overlayGroup.querySelector(".pref-heading")!;
  const description = h("p", { class: "pref-description" });
  const headingRow = h("div", { class: "pref-heading-row" });
  heading.replaceWith(headingRow);
  headingRow.append(heading, trash);
  overlayGroup.querySelector(".pref-group")!.before(description);

  // The selection, scope and revision the "Applies to" note was last measured for.
  let measured = "";
  const syncDescription = async (state: AppState, id: number) => {
    const frames = framesInScope(state);
    const key = `${state.doc?.revision}:${id}:${frames.join(",")}:${state.playhead}`;
    if (key === measured) return;
    measured = key;
    const spread = await app.query("overlayEditCount", id, frames, state.playhead);
    if (key !== measured) return;
    description.textContent = fill(
      // Translators: Shown under the delay field while multiple frames are selected; changing it applies to all of them at once.
      tn("Applies to {count} selected frame", "Applies to {count} selected frames", spread),
      { count: spread },
    );
    description.hidden = spread <= 1;
  };

  const sync = () => {
    const { state } = app;
    const selected = editingOverlay(state);
    kind = selected?.kind;
    const isText = kind?.type === "text";
    const isShape = kind?.type === "shape";
    overlayGroup.hidden = !kind;
    textGroup.hidden = multiFrame(state) && !isText;
    textRow.hidden = !isText;
    for (const row of textRows) row.hidden = !isText;
    strokeRow.hidden = !isShape;
    fillOn.hidden = !isShape;
    if (kind?.type === "text") syncText(kind);
    if (kind?.type === "shape") syncShape(kind);
    if (selected) void syncDescription(state, selected.id);
  };
  const syncText = (text: TextOverlay) => {
    // Never under the cursor: a keystroke still in flight would be undone.
    if (document.activeElement !== entry && entry.value !== text.text) entry.value = text.text;
    font.set(text.font);
    size.set(text.sizePx);
    fillColour.set(text.color);
    outline.colour.set(text.outline?.[0] ?? [0, 0, 0, 255]);
    outline.width.set(text.outline?.[1] ?? 0);
    for (const { align, button } of alignButtons)
      button.classList.toggle("active", align === text.align);
    antialias.checked = text.antialias;
  };
  const syncShape = (shape: ShapeOverlay) => {
    fillOn.checked = shape.fill !== null;
    fillColour.set(shape.fill ?? DEFAULT_SHAPE_COLOUR);
    stroke.colour.set(shape.stroke?.[0] ?? DEFAULT_SHAPE_COLOUR);
    stroke.width.set(shape.stroke?.[1] ?? 0);
  };
  sync();
  app.store.subscribe(sync);
  return { text: textGroup, overlay: overlayGroup };
}

/**
 * An `adw::ActionRow` that is not activatable: a `div`, not `prefs.actionRow`'s
 * `label`, so a click on the title never presses the first control in it.
 */
function row(title: string, content: Node): HTMLElement {
  return h(
    "div",
    { class: "pref-row" },
    h("span", { class: "pref-titles" }, h("span", { class: "pref-title" }, title)),
    content,
  );
}
