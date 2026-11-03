// Glyphs for the rail and the overlay editor. The tool glyphs are the
// desktop's own (resources/icons, 24×24); the rest are 16×16 stand-ins for
// the Adwaita symbolic icons the desktop names beside each.

import type { Tool } from "../wasm/gifkino_wasm.js";
import { icon } from "../ui/dom";

const TOOL_GLYPHS: Record<Tool | "crop", string> = {
  text: "M3,3 L21,3 L21,7 L14,7 L14,21 L10,21 L10,7 L3,7 Z",
  rect: "M3,3 L21,3 L21,21 L3,21 Z M6,6 L6,18 L18,18 L18,6 Z",
  ellipse:
    "M12,1.5 C17.8,1.5 22.5,6.2 22.5,12 C22.5,17.8 17.8,22.5 12,22.5 C6.2,22.5 1.5,17.8 1.5,12 C1.5,6.2 6.2,1.5 12,1.5 Z M12,5 C8.13,5 5,8.13 5,12 C5,15.87 8.13,19 12,19 C15.87,19 19,15.87 19,12 C19,8.13 15.87,5 12,5 Z",
  arrow: "M6.13,20.13 L15.77,10.5 L17.89,12.62 L21,3 L11.38,6.11 L13.5,8.23 L3.87,17.87 Z",
  crop: "M2,2 L10,2 L10,4.5 L4.5,4.5 L4.5,10 L2,10 Z M22,22 L14,22 L14,19.5 L19.5,19.5 L19.5,14 L22,14 Z",
};

export function toolIcon(tool: Tool | "crop"): SVGSVGElement {
  return icon(TOOL_GLYPHS[tool], "0 0 24 24");
}

/** view-grid-symbolic */
export const GRID =
  "M1 1h6v6H1zm2 2v2h2V3zm6-2h6v6H9zm2 2v2h2V3zM1 9h6v6H1zm2 2v2h2v-2zm6-2h6v6H9zm2 2v2h2v-2z";
/** pan-down-symbolic, the split button's arrow */
export const PAN_DOWN = "M4 6h8l-4 5z";
/** go-up-symbolic */
export const GO_UP = "M8 3 2 9l1.4 1.4L8 5.8l4.6 4.6L14 9z";
/** go-down-symbolic */
export const GO_DOWN = "M8 13 2 7l1.4-1.4L8 10.2l4.6-4.6L14 7z";
/** window-close-symbolic */
export const CLOSE =
  "M4.2 2.8 8 6.6l3.8-3.8 1.4 1.4L9.4 8l3.8 3.8-1.4 1.4L8 9.4l-3.8 3.8-1.4-1.4L6.6 8 2.8 4.2z";
/** user-trash-symbolic */
export const TRASH = "M6 1h4v1h4v2H2V2h4zM3 5h10l-1 10H4zm3 2v6h1V7zm3 0v6h1V7z";
/** format-justify-{left,center,right,fill}-symbolic */
export const ALIGN = {
  left: "M1 2h14v2H1zm0 4h9v2H1zm0 4h14v2H1zm0 4h9v2H1z",
  center: "M1 2h14v2H1zm3 4h8v2H4zm-3 4h14v2H1zm3 4h8v2H4z",
  right: "M1 2h14v2H1zm5 4h9v2H6zm-5 4h14v2H1zm5 4h9v2H6z",
  justify: "M1 2h14v2H1zm0 4h14v2H1zm0 4h14v2H1zm0 4h9v2H1z",
} as const;
