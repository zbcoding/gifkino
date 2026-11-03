// Pure wording and arithmetic the frame-op UI shares with the desktop.

import { fill, t } from "../i18n";
import type { Scope } from "../state";

/** `tn` without a catalog: English plural by count. */
export function plural(one: string, many: string, count: number): string {
  return fill(t(count === 1 ? one : many), { count });
}

/**
 * Desktop `frame_scope_summary`: the sidebar heading over the delay field,
 * "Frame" for one frame in scope, else which frames are.
 */
export function frameScopeSummary(scope: Scope, inScope: number[]): string {
  if (inScope.length <= 1) return t("Frame");
  if (scope === "all") {
    // Translators: Sidebar heading while every frame is in scope.
    return fill(t("All {count} frames selected"), { count: inScope.length });
  }
  const first = inScope[0];
  const last = inScope[inScope.length - 1];
  if (last - first + 1 === inScope.length) {
    // Translators: Sidebar heading for a run of selected frames. Frame numbers are 1-based and inclusive.
    return fill(t("Frames {first}–{last} selected"), { first: first + 1, last: last + 1 });
  }
  if (inScope.length <= 8) {
    const numbers = inScope.map((i) => i + 1).join(", ");
    // Translators: Sidebar heading for a non-contiguous set of selected frames; {numbers} is a comma-separated, 1-based list.
    return fill(t("Frames {numbers} selected"), { numbers });
  }
  // Translators: Sidebar heading when too many non-contiguous frames are selected to list by number.
  return plural("{count} frame selected", "{count} frames selected", inScope.length);
}

/**
 * Desktop `paired_dimension`: the other side of a size that keeps the
 * `editedOther`:`fixedOther` ratio, clamped to a size field's 16–8192.
 */
export function pairedDimension(edited: number, editedOther: number, fixedOther: number): number {
  const paired = editedOther === 0 ? edited : Math.round((edited * fixedOther) / editedOther);
  return Math.min(8192, Math.max(16, paired));
}

/** Desktop `smart_drop`: how many frames a smart drop of `percent` removes (never over 95%). */
export function smartDropCount(total: number, percent: number): number {
  return Math.floor((total * Math.min(percent, 95)) / 100);
}
