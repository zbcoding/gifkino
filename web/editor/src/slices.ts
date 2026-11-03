// Feature slices, installed in this order before the layout is built. Each is
// `install(app)` from its own module: add one import and one entry.

import { install as about } from "./about";
import type { App } from "./app";
import { install as frameops } from "./frameops";
import { install as keys } from "./keys";
import { install as overlays } from "./overlays";
import { install as settings } from "./settings";
import { install as strip } from "./strip";
import { install as video } from "./video";

export const slices: ((app: App) => void)[] = [
  frameops,
  strip,
  overlays,
  video,
  keys,
  settings,
  about,
];
