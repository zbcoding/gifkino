// Feature slices, installed in this order before the layout is built. Each is
// `install(app)` from its own module: add one import and one entry.

import type { App } from "./app";

export const slices: ((app: App) => void)[] = [];
