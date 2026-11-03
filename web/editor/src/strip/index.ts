// The frame strip and the overlay bands under it (desktop strip.rs), plus the
// keyboard-only strip commands: select all and the strip zoom.

import "./strip.css";
import type { App } from "../app";
import { createBands } from "./bands";
import { STRIP_ZOOM_STEP } from "./geometry";
import { selectAllFrames } from "./selection";
import { createStrip, Picker } from "./strip";

export function install(app: App): void {
  const picker = new Picker(app);
  const strip = createStrip(app, picker);
  app.registerPanel({ slot: "strip", id: "strip", order: 0, element: strip.element });
  app.registerPanel({
    slot: "bands",
    id: "bands",
    order: 0,
    element: createBands(app, strip, picker),
  });

  app.registerCommand({
    id: "select-all",
    label: "Select all frames",
    run: () => app.update(selectAllFrames),
  });
  const zooms: [string, string, number][] = [
    ["strip-zoom-in", "Zoom in the frame strip", STRIP_ZOOM_STEP],
    ["strip-zoom-out", "Zoom out the frame strip", 1 / STRIP_ZOOM_STEP],
    // `0` is the reset to 1×.
    ["strip-zoom-reset", "Reset the frame strip zoom", 0],
  ];
  for (const [id, label, factor] of zooms) {
    app.registerCommand({ id, label, run: () => strip.zoomBy(factor) });
  }
}
