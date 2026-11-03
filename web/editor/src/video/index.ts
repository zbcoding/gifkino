// Video import (desktop `pipeline/video.rs` and `ui/window/imports.rs`):
// probe, the import dialog, then ffmpeg decoding straight to RGBA frames.

import type { App } from "../app";
import { importBudget } from "../frameops/budget";
import { t } from "../i18n";
import type { FrameSource } from "../worker/protocol";
import { importDialog } from "./dialog";
import { cancelFfmpeg, Cancelled, decodeFrames, ffprobe, inputPath, loadFfmpeg } from "./ffmpeg";
import { FRAMES_DEVICE } from "./ffmpeg-protocol";
import type { ImportPlan, VideoInfo } from "./plan";
import {
  decodeArgs,
  defaultOptions,
  keptRate,
  needsFfmpeg,
  parseProbe,
  planFor,
  planFrames,
  playbackFps,
  sampleArgs,
} from "./plan";

/** gifkino-core `ESTIMATE_SAMPLES`: frames a size estimate encodes. */
const ESTIMATE_SAMPLES = 8;

export function install(app: App): void {
  app.registerImporter({
    accept: "video/*",
    matches: (file) => needsFfmpeg(file.name),
    load: (file) => importVideo(app, file),
  });
}

/** Desktop `plan_import` → `confirm_oversize` → `load`. `null`: cancelled anywhere. */
async function importVideo(app: App, file: File): Promise<FrameSource | null> {
  if (!(await loadFfmpeg(app))) return null;
  const budget = importBudget(false);
  const plan = planFor(await probe(file), defaultOptions(budget));
  const choice = await importDialog({
    name: file.name,
    plan,
    budget,
    liftedBudget: importBudget(true),
    measure: (chosen) => measureGif(app, file, chosen),
  });
  if (!choice) return null;
  try {
    return await decodePlan(app, file, choice.plan);
  } catch (error) {
    if (error instanceof Cancelled) return null;
    throw error;
  }
}

/** Desktop `probe`. */
async function probe(file: File): Promise<VideoInfo> {
  const lines = await ffprobe(file, [
    "-v",
    "error",
    "-select_streams",
    "v:0",
    "-show_entries",
    "stream=width,height,r_frame_rate:format=duration",
    "-of",
    "default=noprint_wrappers=1:nokey=1",
    inputPath(file),
  ]);
  if (lines.length === 0) throw new Error(`ffprobe could not read ${file.name}`);
  return parseProbe(lines, file.name);
}

/** Desktop `import_planned`: exactly this plan, under a cancellable busy bar. */
async function decodePlan(app: App, file: File, plan: ImportPlan): Promise<FrameSource> {
  const expected = planFrames(plan);
  const label =
    expected === null ? t("Importing… {done} frames") : t("Importing… {done} / {total} frames");
  const frames: Uint8Array<ArrayBuffer>[] = [];
  const { decoded, failed } = await app.withBusy(
    label,
    (report) => {
      report(0, expected);
      const job = {
        runs: [decodeArgs(plan, inputPath(file), FRAMES_DEVICE)],
        frameBytes: plan.width * plan.height * 4,
        dropNth: plan.dropNth,
        cap: plan.cap,
      };
      return decodeFrames(file, job, (rgba) => {
        frames.push(rgba);
        report(frames.length, expected);
      });
    },
    cancelFfmpeg,
  );
  // Stopping at the cap is a kept import, not a failure.
  if (failed && frames.length < plan.cap) throw new Error(`ffmpeg failed to decode ${file.name}`);
  if (frames.length === 0) throw new Error(`${file.name} decoded to zero frames`);
  // The drop leaves fewer frames covering the same time: delays from the kept rate.
  const fps = keptRate(plan.fps, plan.dropNth, [decoded, frames.length]);
  const delays = await app.compute("delaysForFps", fps, frames.length);
  return {
    kind: "rgba",
    width: plan.width,
    height: plan.height,
    frames: frames.map((rgba, i) => ({ rgba, delayCs: delays[i] })),
  };
}

/** Desktop `estimate_gif_size`: a spread of sampled frames, encoded and extrapolated. */
async function measureGif(app: App, file: File, plan: ImportPlan): Promise<number> {
  const frameBytes = plan.width * plan.height * 4;
  const samples: Uint8Array[] = [];
  const job = {
    runs: sampleArgs(plan, ESTIMATE_SAMPLES, inputPath(file), FRAMES_DEVICE),
    frameBytes,
    dropNth: null,
    cap: ESTIMATE_SAMPLES,
  };
  await decodeFrames(file, job, (rgba) => samples.push(rgba));
  if (samples.length === 0) throw new Error(`could not read any frames from ${file.name}`);
  const joined = new Uint8Array(samples.length * frameBytes);
  samples.forEach((sample, i) => joined.set(sample, i * frameBytes));
  const delay = Math.min(Math.max(Math.round(100 / Math.max(playbackFps(plan), 1)), 1), 65535);
  const total = planFrames(plan) ?? plan.cap;
  return app.compute("estimateGifSize", plan.width, plan.height, joined, delay, total);
}
