// The left rail (desktop `tool_rail`) and the commands behind it: a click
// places a default-sized overlay under the scope, the canvas handles do the
// rest. Text, then rect/ellipse/arrow on one split button that follows the
// shape used last, then the crop toggle, then snapping apart from the tools.

import type { App } from "../app";
import { t } from "../i18n";
import type { AppState, Store } from "../state";
import { h, icon, tip } from "../ui/dom";
import type { Tool } from "../wasm/gifkino_wasm.js";
import type { Actions } from "./actions";
import { TOOL_LABELS } from "./actions";
import { GRID, PAN_DOWN, toolIcon } from "./icons";
import { CROP_TOOL, cropArmed } from "./local";
import type { Local } from "./local";

type Shape = Local["shapeTool"];
const SHAPES: Shape[] = ["rect", "ellipse", "arrow"];

/** Desktop `keymap::Action` ids and labels for the tool keys. */
const TOOL_COMMANDS: Record<Tool, { id: string; label: string }> = {
  text: { id: "tool-text", label: "Text tool" },
  rect: { id: "tool-rectangle", label: "Rectangle tool" },
  ellipse: { id: "tool-ellipse", label: "Ellipse tool" },
  arrow: { id: "tool-arrow", label: "Arrow tool" },
};

const loaded = (state: AppState) => (state.doc?.frameCount ?? 0) > 0;
const canAdd = (state: AppState) => loaded(state) && state.busy === null;

export function installTools(app: App, local: Store<Local>, actions: Actions): void {
  for (const tool of ["text", "rect", "ellipse", "arrow"] as Tool[]) {
    const { id, label } = TOOL_COMMANDS[tool];
    // Keyboard targets; tools may share a key, which cycles them.
    app.registerCommand({ id, label, enabled: canAdd, run: () => actions.addOverlay(tool) });
    app.registerCommand({
      id: `insert-${tool}`,
      label: TOOL_LABELS[tool],
      menu: "insert",
      enabled: canAdd,
      run: () => actions.addOverlay(tool),
    });
  }
  app.registerCommand({
    id: "tool-crop",
    // Translators: Draws a box on the canvas for cropping or zooming.
    label: "Crop tool",
    enabled: loaded,
    run: () => toggleCropTool(app, local),
  });
  app.registerCommand({
    id: "toggle-snap",
    // Translators: Toggles pulling dragged overlays onto the canvas edges and centre lines.
    label: "Snap to edges and center",
    run: () => local.update({ snap: !local.state.snap }),
  });
  app.registerPanel({
    slot: "rail",
    id: "tools",
    order: 0,
    element: createRail(app, local, actions),
  });
}

/** Desktop `toggle_crop_tool`: arming drops the overlay pick; either way the box goes. */
export function toggleCropTool(app: App, local: Store<Local>): void {
  const arming = !cropArmed(app);
  local.update({ crop: null });
  app.update(arming ? { activeTool: CROP_TOOL, selectedOverlay: null } : { activeTool: null });
}

function railButton(content: Node, onclick: () => void): HTMLButtonElement {
  return h("button", { class: "flat icon-button rail-button", type: "button", onclick }, content);
}

function createRail(app: App, local: Store<Local>, actions: Actions): HTMLElement {
  const text = railButton(toolIcon("text"), () => void app.runCommand("tool-text"));

  const shapeMain = railButton(
    toolIcon(local.state.shapeTool),
    () => void app.runCommand(TOOL_COMMANDS[local.state.shapeTool].id),
  );
  const shapeList = h("div", { class: "shape-popover", popover: "auto" });
  const shapeArrow = railButton(icon(PAN_DOWN), () => {
    shapeList.showPopover();
    const at = shapeArrow.getBoundingClientRect();
    shapeList.style.left = `${at.right + 4}px`;
    shapeList.style.top = `${at.top}px`;
  });
  for (const shape of SHAPES) {
    shapeList.append(
      h(
        "button",
        {
          class: "flat icon-button",
          type: "button",
          title: t(TOOL_LABELS[shape]),
          onclick: () => {
            local.update({ shapeTool: shape });
            shapeList.hidePopover();
            void actions.addOverlay(shape);
          },
        },
        toolIcon(shape),
      ),
    );
  }
  const shapeButton = h("div", { class: "split-button" }, shapeMain, shapeArrow, shapeList);

  const crop = railButton(toolIcon("crop"), () => void app.runCommand("tool-crop"));
  const snap = railButton(icon(GRID), () => void app.runCommand("toggle-snap"));

  const sync = () => {
    const { state } = app;
    const shape = local.state.shapeTool;
    shapeMain.replaceChildren(toolIcon(shape));
    text.title = tip(app, t(TOOL_LABELS.text), "tool-text");
    shapeMain.title = tip(app, t(TOOL_LABELS[shape]), TOOL_COMMANDS[shape].id);
    crop.title = tip(app, t("Crop or zoom: drag a box on the canvas"), "tool-crop");
    snap.title = tip(app, t("Snap to edges and center"), "toggle-snap");
    for (const button of [text, shapeMain, shapeArrow]) button.disabled = !canAdd(state);
    crop.disabled = !loaded(state);
    crop.classList.toggle("active", cropArmed(app));
    snap.classList.toggle("active", local.state.snap);
  };
  sync();
  app.store.subscribe(sync);
  local.subscribe(sync);
  return h("div", { class: "tool-rail" }, text, shapeButton, crop, h("hr", {}), snap);
}
