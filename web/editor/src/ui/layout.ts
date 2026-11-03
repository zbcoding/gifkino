// The window: header, busy bar, then either the welcome page or the editor
// (rail + canvas over the footer's transport, strip and bands; sidebar on the
// right). Slots are filled from `app.panels`, sorted by `order`.

import type { App, Slot } from "../app";
import { fill, t } from "../i18n";
import { createCanvasView } from "./canvas";
import { commandButton, h } from "./dom";
import { createHeader } from "./header";
import { createDocInfo } from "./sidebar";
import { createTransport } from "./transport";
import iconUrl from "../../../public/images/icon.svg";

export function createLayout(app: App): HTMLElement {
  const slot = (name: Slot) => h("div", { class: `slot slot-${name}` });
  const slots: Record<Slot, HTMLElement> = {
    rail: slot("rail"),
    transport: slot("transport"),
    strip: slot("strip"),
    bands: slot("bands"),
    sidebar: slot("sidebar"),
  };
  for (const panel of [...app.panels].sort((a, b) => a.order - b.order)) {
    panel.element.dataset.panel = panel.id;
    slots[panel.slot].append(panel.element);
  }

  const welcome = h(
    "section",
    { class: "welcome" },
    h("img", { src: iconUrl, alt: "", width: "128", height: "128" }),
    h("h1", {}, t("Open Image")),
    h("p", {}, t("Open a video or GIF to start editing")),
    commandButton(app, "open", t("Open…"), "pill suggested"),
  );

  const editor = h(
    "section",
    { class: "editor" },
    h(
      "div",
      { class: "editor-main" },
      h("div", { class: "canvas-row" }, slots.rail, createCanvasView(app)),
      h(
        "footer",
        { class: "footer" },
        createTransport(app, slots.transport),
        slots.strip,
        slots.bands,
      ),
    ),
    h("aside", { class: "sidebar" }, slots.sidebar, createDocInfo(app)),
  );

  const root = h(
    "div",
    { class: "window" },
    createHeader(app),
    createBusyBar(app),
    h("main", { class: "content" }, welcome, editor),
    createToasts(app),
  );
  const sync = () => {
    const loaded = app.state.doc !== null;
    welcome.hidden = loaded;
    editor.hidden = !loaded;
  };
  sync();
  app.store.subscribe(sync);
  return root;
}

/** The progress row under the header, shown while a job holds `state.busy`. */
function createBusyBar(app: App): HTMLElement {
  const bar = h("progress", { max: "1" });
  const label = h("span", {});
  const cancel = h(
    "button",
    { class: "flat", onclick: () => app.state.busy?.cancel?.() },
    t("Cancel"),
  );
  const row = h("div", { class: "busy" }, bar, label, cancel);
  const sync = () => {
    const { busy } = app.state;
    row.hidden = !busy;
    if (!busy) return;
    label.textContent = fill(busy.label, { done: busy.done, total: busy.total ?? "" });
    cancel.hidden = !busy.cancel;
    // A bar without `value` is indeterminate: the pulsing kind.
    if (busy.total) bar.value = busy.done / busy.total;
    else bar.removeAttribute("value");
  };
  sync();
  app.store.subscribe(sync);
  return row;
}

/**
 * `app.toast`: a message at the bottom that fades after a few seconds. An
 * edit's toast offers Undo (desktop `Msg::Toast`); feedback with nothing to
 * undo must not (`Msg::Notice`), or it would undo some earlier edit.
 */
function createToasts(app: App): HTMLElement {
  const host = h("div", { class: "toasts", role: "status" });
  app.toast = (message, options) => {
    const toast = h("div", { class: "toast" }, message);
    if (options?.undo) {
      const undo = () => {
        toast.remove();
        void app.runCommand("undo");
      };
      toast.append(h("button", { class: "flat", onclick: undo }, t("Undo")));
    }
    host.append(toast);
    setTimeout(() => toast.remove(), 4000);
  };
  return host;
}
