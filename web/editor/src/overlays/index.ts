// Overlays slice: the tool rail, the canvas layer that selects, moves,
// resizes and rotates overlays, the overlay editors in the sidebar, and the
// crop tool. See ARCHITECTURE.md for the commands other slices run.

import type { App } from "../app";
import type { AppState } from "../state";
import { createActions } from "./actions";
import { installCanvas } from "./canvas";
import { installCrop } from "./crop";
import { createLocal } from "./local";
import { installSidebar } from "./sidebar";
import { installTools } from "./tools";
import "./overlays.css";

const picked = (state: AppState) => state.selectedOverlay !== null && state.busy === null;

export function install(app: App): void {
  const local = createLocal(app);
  const actions = createActions(app, local);
  installTools(app, local, actions);
  installCanvas(app, local);
  installSidebar(app, actions);
  installCrop(app, local);

  // The band menu's actions on the picked overlay (desktop `band_actions`).
  app.registerCommand({
    id: "overlay-copy-all",
    label: "Copy overlay to all frames",
    enabled: picked,
    run: () => actions.copySelected(true),
  });
  app.registerCommand({
    id: "overlay-copy-selected",
    label: "Copy overlay to selected frames",
    enabled: picked,
    run: () => actions.copySelected(false),
  });
  app.registerCommand({
    id: "overlay-delete",
    label: "Delete overlay",
    enabled: picked,
    run: () => actions.deleteOverlay(app.state.selectedOverlay!),
  });
}
