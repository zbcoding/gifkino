// Readouts shared with the desktop (src/ui/window/view.rs, imports.rs).

import { fill, t } from "./i18n";

/** `MM:SS.d`, the transport's clock. */
export function timecode(cs: number): string {
  const minutes = Math.floor(cs / 6000);
  const seconds = Math.floor(cs / 100) % 60;
  const tenths = Math.floor((cs % 100) / 10);
  return `${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}.${tenths}`;
}

/** Frames per second over the whole document, rounded; 0 without a duration. */
export function fps(frameCount: number, durationCs: number): number {
  return durationCs === 0 ? 0 : Math.round((frameCount * 100) / durationCs);
}

/** Binary units: whole KB and MB, GB to one decimal. */
export function byteSize(bytes: number): string {
  if (bytes >= 2 ** 30) return fill(t("{n} GB"), { n: (bytes / 2 ** 30).toFixed(1) });
  if (bytes >= 2 ** 20) return fill(t("{n} MB"), { n: (bytes / 2 ** 20).toFixed(0) });
  return fill(t("{n} KB"), { n: (bytes / 1024).toFixed(0) });
}
