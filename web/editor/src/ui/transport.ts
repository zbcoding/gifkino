// The transport row: play/pause and the clock, then the `transport` slot.

import type { App } from "../app";
import { fps, timecode } from "../format";
import { fill, t } from "../i18n";
import { elapsedCs } from "../state";
import type { Scope } from "../state";
import { commandButton, h, icon } from "./dom";
import { PAUSE, PLAY } from "./icons";

export function createTransport(app: App, slot: HTMLElement): HTMLElement {
  const playIcon = icon(PLAY);
  const play = commandButton(app, "play-pause", playIcon, "icon-button");
  const time = h("span", { class: "time" });

  const sync = () => {
    const { doc, playing } = app.state;
    playIcon.querySelector("path")!.setAttribute("d", playing ? PAUSE : PLAY);
    time.textContent = fill(t("{elapsed} / {total} · {fps} fps"), {
      elapsed: timecode(elapsedCs(app.state)),
      total: timecode(doc?.durationCs ?? 0),
      fps: fps(doc?.frameCount ?? 0, doc?.durationCs ?? 0),
    });
  };
  sync();
  app.store.subscribe(sync);
  return h("div", { class: "transport" }, play, time, h("span", { class: "spacer" }), slot);
}

/** The scope toggle: which frames an edit in "this scope" reaches. */
export function createScopeButtons(app: App): HTMLElement {
  const choices: [Scope, string][] = [
    ["frame", t("This frame")],
    ["all", t("All frames")],
    ["range", t("Range")],
  ];
  const buttons = choices.map(([scope, label]) =>
    h("button", { onclick: () => app.update({ scope }) }, label),
  );
  // The scope buttons are where multi-frame selection is discoverable at all.
  buttons[1].title = [
    t("Every frame in the document"),
    t("Shift+click the strip for a run of frames, Ctrl+click to add to it"),
  ].join("\n");
  const sync = () => {
    const { scope, selection } = app.state;
    choices.forEach(([choice], i) => buttons[i].classList.toggle("active", choice === scope));
    // Range only exists while there is a selection to bind it to.
    buttons[2].hidden = selection.length === 0 && scope !== "range";
  };
  sync();
  app.store.subscribe(sync);
  return h("div", { class: "linked scope" }, ...buttons);
}
