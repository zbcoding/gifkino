// The desktop's `pipeline/video.rs` and trim-field tests, on the port: the
// preview the import dialog shows has to be the decode it runs.

import { describe, expect, test } from "vite-plus/test";
import type { ImportOptions, VideoInfo } from "./plan";
import {
  decodeArgs,
  defaultOptions,
  needsFfmpeg,
  overBudget,
  parseProbe,
  parseRate,
  parseSeconds,
  planBytes,
  planFor,
  planFrames,
  playbackFps,
  readTrim,
  resolveTrim,
  sampleArgs,
  steppedTrim,
  timestamp,
} from "./plan";

const MIB = 1024 * 1024;
/** Desktop `ImportOptions::default`: the budget its own tests plan against. */
const options = (overrides: Partial<ImportOptions> = {}): ImportOptions => ({
  ...defaultOptions(1200 * MIB),
  ...overrides,
});
const video = (
  width: number,
  height: number,
  fps: number,
  durationS: number | null,
): VideoInfo => ({
  width,
  height,
  fps,
  durationS,
});

describe("probe", () => {
  test("frame rates parse", () => {
    expect(parseRate("30/1")).toBe(30);
    expect(parseRate("30000/1001")).toBeCloseTo(29.97, 2);
    expect(parseRate("25")).toBe(25);
    expect(parseRate("0/0")).toBe(0);
  });

  test("ffprobe's four lines become the video, a missing rate becomes 25", () => {
    expect(parseProbe(["320", "240", "30/1", "2.000000"], "a.mp4")).toEqual(video(320, 240, 30, 2));
    expect(parseProbe(["320", "240", "0/0", "N/A"], "a.mp4")).toEqual(video(320, 240, 25, null));
    expect(() => parseProbe(["", ""], "a.mp3")).toThrow("a.mp3 has no video stream");
  });

  test("what is neither a GIF nor a still goes to ffmpeg", () => {
    expect(needsFfmpeg("clip.MP4")).toBe(true);
    expect(needsFfmpeg("anim.webp")).toBe(true);
    expect(needsFfmpeg("a.gif")).toBe(false);
    expect(needsFfmpeg("photo.JPG")).toBe(false);
    expect(needsFfmpeg("constructor")).toBe(true);
  });
});

describe("plan", () => {
  test("a long clip loses frame rate, not its ending", () => {
    const plan = planFor(video(1920, 1080, 60, 120), options());
    expect([plan.width, plan.height]).toEqual([1280, 720]);
    expect(planBytes(plan)!).toBeLessThanOrEqual(1200 * MIB);
    expect(plan.fps).toBeGreaterThanOrEqual(2);
    expect(plan.fps).toBeLessThan(60);
    expect(overBudget(plan)).toBe(false);
    expect(playbackFps(plan)).toBe(plan.fps);
  });

  test("a short clip keeps its frame rate", () => {
    const plan = planFor(video(640, 480, 30, 5), options());
    expect(plan.fps).toBe(30);
    expect(planFrames(plan)).toBe(150);
  });

  test("the previewed rate is the rate the kept frames play at", () => {
    const plan = planFor(video(64, 64, 10, 1), options({ dropNth: 3 }));
    expect(planFrames(plan)).toBe(7);
    expect(playbackFps(plan)).toBe(7);
  });

  test("an endless clip is capped but unknowable", () => {
    const plan = planFor(video(320, 240, 25, null), options());
    expect(plan.fps).toBe(25);
    expect(planFrames(plan)).toBeNull();
    expect(overBudget(plan)).toBe(false);
  });

  test("a trim resolves to the same span either way, both ends inclusive", () => {
    const info = video(640, 480, 25, 100);
    expect(resolveTrim({ kind: "seconds", start: 10, end: 20 }, info)).toEqual([10, 10]);
    const [start, span] = resolveTrim({ kind: "frames", first: 234, last: 1234 }, info);
    expect(start).toBe(234 / 25);
    expect(Math.round(span! * 25)).toBe(1001);
    expect(resolveTrim({ kind: "seconds", start: 90, end: null }, info)).toEqual([90, null]);
    expect(resolveTrim({ kind: "seconds", start: 90, end: 500 }, info)).toEqual([90, 10]);
  });

  test("the plan counts the trim and the drop", () => {
    const info = video(320, 240, 30, 600);
    const trim = { kind: "seconds", start: 60, end: 70 } as const;
    const plan = planFor(info, options({ fps: 30, trim, dropNth: 3 }));
    expect([plan.startS, plan.spanS]).toEqual([60, 10]);
    expect(planFrames(plan)).toBe(200);
    expect(playbackFps(plan)).toBe(20);
    for (const keepAll of [null, 1]) {
      const whole = planFor(info, options({ fps: 30, dropNth: keepAll }));
      expect(whole.dropNth).toBeNull();
      expect(planFrames(whole)).toBe(18000);
    }
  });

  test("dropping frames buys a clip that would not fit", () => {
    const info = video(640, 360, 30, 120);
    const budget = options({ fps: 30, maxBytes: 210 * MIB });
    expect(overBudget(planFor(info, budget))).toBe(true);
    const trim = { kind: "seconds", start: 0, end: 10 } as const;
    const plan = planFor(info, { ...budget, trim, dropNth: 2 });
    expect(planFrames(plan)).toBe(150);
    expect(overBudget(plan)).toBe(false);
  });

  test("the drop buys the automatic rate more frames", () => {
    const info = video(1280, 720, 60, 120);
    const plain = planFor(info, options());
    for (const n of [2, 3, 4, 7]) {
      const dropped = planFor(info, options({ dropNth: n }));
      expect(dropped.fps / plain.fps).toBeCloseTo(n / (n - 1), 2);
      expect(overBudget(dropped)).toBe(false);
    }
  });

  test("the decode runs exactly the plan: seek, span, scale, rate, ceiling", () => {
    const trim = { kind: "seconds", start: 2, end: 4 } as const;
    const plan = planFor(video(1920, 1080, 30, 6), options({ fps: 15, trim }));
    expect(decodeArgs(plan, "/input/a.mp4", "/frames")).toEqual([
      "-v",
      "error",
      "-ss",
      "2.000",
      "-i",
      "/input/a.mp4",
      "-t",
      "2.000",
      "-vf",
      "scale=1280:720,fps=15",
      "-frames:v",
      String(plan.cap),
      "-f",
      "rawvideo",
      "-pix_fmt",
      "rgba",
      "/frames",
    ]);
    const untouched = planFor(video(320, 240, 30, 2), options());
    expect(decodeArgs(untouched, "/i", "/f")).not.toContain("-vf");
  });

  test("samples spread across the trimmed span at the planned size", () => {
    const trim = { kind: "seconds", start: 10, end: 18 } as const;
    const plan = planFor(video(640, 480, 30, 60), options({ trim }));
    const seeks = sampleArgs(plan, 8, "/i", "/f").map((args) => args[args.indexOf("-ss") + 1]);
    expect(seeks).toEqual([
      "10.500",
      "11.500",
      "12.500",
      "13.500",
      "14.500",
      "15.500",
      "16.500",
      "17.500",
    ]);
  });
});

describe("trim fields", () => {
  const info = video(640, 480, 25, 12);

  test("times read as seconds, mm:ss or h:mm:ss, to the centisecond", () => {
    expect(parseSeconds("12.5")).toBe(12.5);
    expect(parseSeconds("0:01.123")).toBe(1.12);
    expect(parseSeconds("1:02:03")).toBe(3723);
    for (const bad of ["", "abc", "-1", "1:2:3:4", "0x10", "1:"])
      expect(parseSeconds(bad)).toBeNull();
    expect(timestamp(83.45)).toBe("00:01:23.450");
  });

  test("blank is the whole clip; anything outside the file or backwards is invalid", () => {
    expect(readTrim(false, " ", "", info)).toEqual({ kind: "whole" });
    expect(readTrim(false, "2", "", info)).toEqual({
      kind: "set",
      trim: { kind: "seconds", start: 2, end: null },
    });
    expect(readTrim(true, "", "299", info)).toEqual({
      kind: "set",
      trim: { kind: "frames", first: 0, last: 299 },
    });
    for (const [start, end] of [
      ["5", "5"],
      ["5", "3"],
      ["13", ""],
      ["", "12.5"],
      ["x", ""],
    ]) {
      expect(readTrim(false, start, end, info)).toEqual({ kind: "invalid" });
    }
    expect(readTrim(true, "", "300", info)).toEqual({ kind: "invalid" });
  });

  test("steps start from the end a blank field stands for and stay inside the file", () => {
    expect(steppedTrim("", true, false, false, info)).toBe("00:00:01.000");
    expect(steppedTrim("", false, false, true, info)).toBe("00:00:11.000");
    expect(steppedTrim("11.5", true, false, true, info)).toBe("00:00:12.000");
    expect(steppedTrim("", true, true, false, info)).toBe("10");
    expect(steppedTrim("295", true, true, true, info)).toBe("299");
    expect(steppedTrim("4", false, true, false, info)).toBe("0");
  });
});
