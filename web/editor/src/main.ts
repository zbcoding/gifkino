import "./style.css";
import { App } from "./app";
import { installCore } from "./core";
import { startPlayback } from "./playback";
import { createClient } from "./rpc";
import type { Port } from "./rpc";
import { slices } from "./slices";
import { createLayout } from "./ui/layout";

const worker = new Worker(new URL("./worker/worker.ts", import.meta.url), { type: "module" });
const app = new App(createClient(worker as Port));

installCore(app);
for (const install of slices) install(app);

document.body.append(createLayout(app));
startPlayback(app.store);
app.startEstimates();

// Console handle for debugging: `gifkino.dispatch("duplicateFrames", [0])`.
Object.assign(globalThis, { gifkino: app });
