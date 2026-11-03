// The sidebar's "Frame" group (desktop `frame_panel` + `sync_frame_delay`):
// a heading naming the frames in scope and the delay of the frame on screen,
// which an edit applies to every frame in scope.

import type { App } from "../app";
import { t } from "../i18n";
import { framesInScope } from "../state";
import { actionRow, group, groupTitle, setSubtitle, spin } from "../ui/prefs";
import { frameScopeSummary, plural } from "./summary";

export function createFramePanel(app: App): HTMLElement {
  const delay = spin(1, 65535, 1, 1);
  // Translators: Per-frame hold time, in centiseconds (1/100 s).
  const row = actionRow(t("Delay (cs)"), delay.input);
  const panel = group(t("Frame"), row);

  // `change` is the spin's commit (Enter, arrows, focus-out). Focus leaves the
  // field before a thumbnail click moves the playhead, so a typed delay lands
  // on the frames it was typed for, as the desktop's `commit_typed` ensures.
  delay.input.addEventListener("change", async () => {
    const frames = framesInScope(app.state);
    const cs = delay.value();
    const doc = app.state.doc;
    // A spin only reports a value that differs from the one it showed: the
    // frame on screen's delay.
    if (!doc || frames.length === 0 || doc.frames[app.state.playhead]?.delayCs === cs) return;
    const change = await app.dispatch("setDelay", frames, cs);
    app.toastChange(change, doc.frameCount);
  });

  const sync = () => {
    const { doc, playhead, busy, playing } = app.state;
    const count = doc?.frameCount ?? 0;
    // Playback rewrites this every tick; typing into it would land on
    // whichever frame the next tick shows.
    delay.input.disabled = count === 0 || busy !== null || playing;
    const frame = doc?.frames[playhead];
    if (frame && document.activeElement !== delay.input) delay.set(frame.delayCs);
    const inScope = framesInScope(app.state);
    groupTitle(panel).textContent = frameScopeSummary(app.state.scope, inScope);
    setSubtitle(
      row,
      inScope.length > 1
        ? // Translators: Shown under the delay field while multiple frames are selected; changing it applies to all of them at once.
          plural(
            "Applies to {count} selected frame",
            "Applies to {count} selected frames",
            inScope.length,
          )
        : "",
    );
  };
  sync();
  app.store.subscribe(sync);
  return panel;
}
