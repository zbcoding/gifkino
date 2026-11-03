// The import dialog's words (desktop `ui/window/imports.rs`): the opening
// line about the file and the live preview of what the settings land.

import { byteSize } from "../format";
import { fill, t } from "../i18n";
import type { ImportPlan, VideoInfo } from "./plan";
import { overBudget, planBytes, planFrames, playbackFps, sourceFrames, timestamp } from "./plan";

/** `m:ss`. */
export function clock(seconds: number): string {
  const s = Math.floor(Math.max(seconds, 0));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

/** `m:ss.cc`, back to `m:ss` when there is no fraction to show. */
export function clockCs(seconds: number): string {
  const cs = Math.round(Math.max(seconds, 0) * 100);
  if (cs % 100 === 0) return clock(cs / 100);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${Math.floor(cs / 6000)}:${pad(Math.floor(cs / 100) % 60)}.${pad(cs % 100)}`;
}

/**
 * Desktop `import_body`: what the file is, and why it will not fit when it
 * will not. `opening` keeps `<b>{name}</b>` for the caller to set in bold.
 */
export function importBody(plan: ImportPlan): { opening: string; advice: string | null } {
  const src = plan.source;
  const length =
    src.durationS === null
      ? t("has no duration the container will admit to")
      : fill(t("runs {duration}"), { duration: clock(src.durationS) });
  const opening = fill(
    // Translators: Pango markup: keep the <b> tags. {length} is "runs 1:30" or the no-duration phrase.
    t("<b>{name}</b> {length} at {width}×{height}, {fps} fps."),
    { length, width: src.width, height: src.height, fps: src.fps.toFixed(0) },
  );
  const advice = overBudget(plan)
    ? t(
        "That is more than will fit in memory at full size. Pick a smaller size or rate, or open Advanced to trim it, drop frames, or ignore the limit for this import.",
      )
    : null;
  return { opening, advice };
}

/** Desktop `advanced_import_rows`' extent: the file's length in both trim units. */
export function extent(source: VideoInfo): string {
  const frames = sourceFrames(source);
  if (source.durationS === null || frames === null) {
    return t("no duration the container will admit to");
  }
  // Translators: The length of the file being imported. {time} is HH:MM:SS.mmm.
  return fill(t("{time} · {frames} frames"), { time: timestamp(source.durationS), frames });
}

/**
 * Desktop `plan_summary`: the rate the frames play at, the span the trim
 * leaves, the memory they hold and the measured GIF size (`null` = still
 * measuring).
 */
export function planSummary(plan: ImportPlan, measured: number | null): string {
  const played = playbackFps(plan);
  const rate =
    played < plan.source.fps - 0.01
      ? fill(t("{fps} fps, down from {source}"), {
          fps: Math.max(played, 1).toFixed(0),
          source: plan.source.fps.toFixed(0),
        })
      : fill(t("{fps} fps"), { fps: played.toFixed(0) });
  const frames = planFrames(plan);
  if (frames === null) {
    // Translators: {rate} is a phrase such as "4 fps, down from 60"; it opens the sentence.
    return fill(t("{rate}. The container gives no duration, so there is no count to preview."), {
      rate,
    });
  }
  let span = "";
  if (plan.spanS !== null) {
    // Translators: The part of the file being imported, as two timecodes.
    const range = fill(t("{from} to {to}"), {
      from: clockCs(plan.startS),
      to: clockCs(plan.startS + plan.spanS),
    });
    span = ` · ${range}`;
  } else if (plan.startS > 0) {
    span = ` · ${fill(t("from {from} on"), { from: clockCs(plan.startS) })}`;
  }
  const bytes = planBytes(plan);
  const memory =
    bytes === null ? "" : ` · ${fill(t("{size} in memory"), { size: byteSize(bytes) })}`;
  const head = fill(t("{frames} frames at {rate}"), { frames, rate });
  if (overBudget(plan)) {
    const refusal = t(
      "That is more memory than an import may use. Choose a smaller size or a lower frame rate.",
    );
    return `${head}${span}${memory}\n${refusal}`;
  }
  const gif =
    measured === null
      ? t("measuring the GIF size…")
      : fill(t("{size} as a GIF"), { size: byteSize(measured) });
  return `${head}${span}${memory} · ${gif}`;
}
