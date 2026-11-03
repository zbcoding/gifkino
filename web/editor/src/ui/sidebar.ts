// The sidebar's foot: canvas size, counts and the export size estimate.

import type { App } from "../app";
import { byteSize } from "../format";
import { fill, t } from "../i18n";
import type { AppState } from "../state";
import { h } from "./dom";

export function createDocInfo(app: App): HTMLElement {
  const summary = h("div", {});
  const estimate = h("div", {});
  const sync = () => {
    const { doc } = app.state;
    if (!doc) return;
    summary.textContent = fill(t("{width} × {height} · {frames} frames · {overlays} overlays"), {
      width: doc.width,
      height: doc.height,
      frames: doc.frameCount,
      overlays: doc.overlays.length,
    });
    estimate.textContent = estimateText(app.state);
  };
  sync();
  app.store.subscribe(sync);
  return h("div", { class: "doc-info" }, summary, estimate);
}

/** Desktop `sync_doc_info`: a stale figure stays up, marked as updating. */
export function estimateText({ doc, estimate }: AppState): string {
  if (!estimate) {
    // Translators: Shown while the exported GIF size is still being measured.
    return doc && doc.frameCount > 0 ? t("sizing…") : "";
  }
  const size = byteSize(estimate.bytes);
  return estimate.revision === doc?.revision
    ? fill(t("≈ {size} as a GIF"), { size })
    : fill(t("≈ {size} as a GIF (updating…)"), { size });
}
