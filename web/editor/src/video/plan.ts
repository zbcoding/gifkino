// Desktop `pipeline/video.rs` planning and the import dialog's trim parsing
// (`ui/window/imports.rs`): what an import would decode, settled from the
// probe alone so the dialog can say what it costs before anything decodes.

export interface VideoInfo {
  width: number;
  height: number;
  fps: number;
  durationS: number | null;
}

/**
 * What part of the input to take. A frame index counts at the source's own
 * rate, and both ends are inclusive.
 */
export type Trim =
  | { kind: "seconds"; start: number; end: number | null }
  | { kind: "frames"; first: number; last: number | null };

export interface ImportOptions {
  /** Long edge cap when no `target` is given. */
  maxWidth: number | null;
  /** Explicit output size, free to change the aspect ratio. */
  target: [number, number] | null;
  fps: number | null;
  trim: Trim | null;
  /** Drop one frame in every N on the way in; under two keeps every frame. */
  dropNth: number | null;
  /** Ceiling on the decoded pixel data. */
  maxBytes: number;
}

/** Desktop `ImportOptions::default` with the budget the caller runs under. */
export function defaultOptions(maxBytes: number): ImportOptions {
  return { maxWidth: 1280, target: null, fps: null, trim: null, dropNth: null, maxBytes };
}

export interface ImportPlan {
  source: VideoInfo;
  width: number;
  height: number;
  fps: number;
  /** Where the decode starts, and how long it runs; `null` runs to the end. */
  startS: number;
  spanS: number | null;
  dropNth: number | null;
  /** Frames the memory budget buys at this size. */
  cap: number;
}

/** Desktop `parse_rate`: ffprobe's `30000/1001` or a plain number. */
export function parseRate(text: string): number {
  const [n, d] = text.trim().split("/");
  if (d === undefined) return Number(n) || 0;
  const denominator = Number(d) || 0;
  return denominator === 0 ? 0 : (Number(n) || 0) / denominator;
}

/**
 * Desktop `probe`: ffprobe's `width`, `height`, `r_frame_rate`, `duration`
 * lines (`default=noprint_wrappers=1:nokey=1`).
 */
export function parseProbe(lines: string[], name: string): VideoInfo {
  const [w, h, rate, duration] = lines.map((line) => line.trim());
  const width = Math.trunc(Number(w)) || 0;
  const height = Math.trunc(Number(h)) || 0;
  if (width <= 0 || height <= 0) throw new Error(`${name} has no video stream`);
  const fps = rate === undefined ? 0 : parseRate(rate);
  const seconds = duration ? Number(duration) : NaN;
  return {
    width,
    height,
    fps: fps > 0 ? fps : 25,
    durationS: Number.isFinite(seconds) ? seconds : null,
  };
}

/** Desktop `VideoInfo::estimated_frames`. */
export function estimatedFrames(source: VideoInfo, fps: number): number | null {
  return source.durationS === null ? null : Math.max(Math.round(source.durationS * fps), 1);
}

/** Frames the source holds at its own rate: desktop `source_frames`. */
export function sourceFrames(source: VideoInfo): number | null {
  return estimatedFrames(source, source.fps);
}

/** Desktop `Trim::resolve`: start, and seconds taken (`null` = to the end). */
export function resolveTrim(trim: Trim, source: VideoInfo): [number, number | null] {
  const rate = source.fps > 0 ? source.fps : 25;
  const [start, end] =
    trim.kind === "seconds"
      ? [Math.max(trim.start, 0), trim.end]
      : [trim.first / rate, trim.last === null ? null : (trim.last + 1) / rate];
  const clamp = (s: number) =>
    source.durationS === null ? Math.max(s, 0) : Math.min(Math.max(s, 0), source.durationS);
  const from = clamp(start);
  return [from, end === null ? null : Math.max(clamp(end) - from, 0)];
}

function thinning(dropNth: number | null): number | null {
  return dropNth !== null && dropNth >= 2 ? dropNth : null;
}

/** Frames left after dropping one in every `n`: frame 0 stays, every Nth goes. */
export function keptFrames(decoded: number, dropNth: number | null): number {
  const n = thinning(dropNth);
  return n === null ? decoded : decoded - Math.floor(decoded / n);
}

/** The most decoded frames whose kept share still fits in `kept`. */
export function decodedForKept(kept: number, dropNth: number | null): number {
  const n = thinning(dropNth);
  return n === null ? kept : Math.floor((kept * n) / (n - 1));
}

/** The rate kept frames play at so they cover the same stretch of time. */
export function keptRate(
  fps: number,
  dropNth: number | null,
  counts: [decoded: number, kept: number] | null,
): number {
  const n = thinning(dropNth);
  if (n === null) return fps;
  if (counts && counts[0] > 0) return (fps * counts[1]) / counts[0];
  return (fps * (n - 1)) / n;
}

function secondsTaken(source: VideoInfo, startS: number, spanS: number | null): number | null {
  if (spanS !== null) return spanS;
  return source.durationS === null ? null : Math.max(source.durationS - startS, 0);
}

/** ffmpeg's scalers and every encoder downstream want even dimensions. */
export function even(value: number): number {
  return Math.max(Math.round(value), 2) & ~1;
}

/** The standard heights that fit inside the source, at its aspect ratio. */
export function sizePresets(source: VideoInfo): [number, number][] {
  return [720, 540, 480, 360, 270]
    .filter((h) => h < source.height)
    .map((h) => [even((source.width * h) / Math.max(source.height, 1)), h]);
}

/**
 * Desktop `plan_for`. Over budget, the automatic rate thins rather than the
 * clip being cut short; an explicit rate is honoured and refused if too big.
 */
export function planFor(source: VideoInfo, options: ImportOptions): ImportPlan {
  let [width, height] = options.target
    ? [even(options.target[0]), even(options.target[1])]
    : [source.width, source.height];
  if (!options.target && options.maxWidth !== null && width > options.maxWidth) {
    height = even((options.maxWidth / width) * height);
    width = options.maxWidth;
  }
  const frameBytes = Math.max(width * height * 4, 1);
  const cap = Math.max(Math.floor(options.maxBytes / frameBytes), 1);
  const [startS, spanS] = options.trim ? resolveTrim(options.trim, source) : [0, null];
  const dropNth = thinning(options.dropNth);
  const plannedSeconds = secondsTaken(source, startS, spanS);
  let fps = options.fps ?? source.fps;
  if (options.fps === null && plannedSeconds !== null && plannedSeconds > 0.5) {
    const affordable = decodedForKept(cap, dropNth);
    fps = Math.min(fps, Math.max(affordable / plannedSeconds, 2));
  }
  return { source, width, height, fps: Math.max(fps, 1), startS, spanS, dropNth, cap };
}

/** Frames the decoder hands over before the drop: what `-frames:v` counts. */
export function decodedFrames(plan: ImportPlan): number | null {
  const seconds = secondsTaken(plan.source, plan.startS, plan.spanS);
  return seconds === null ? null : Math.max(Math.round(seconds * plan.fps), 1);
}

/** Frames this plan lands; `null` with no duration and no trim end. */
export function planFrames(plan: ImportPlan): number | null {
  const decoded = decodedFrames(plan);
  return decoded === null ? null : keptFrames(decoded, plan.dropNth);
}

export function planBytes(plan: ImportPlan): number | null {
  const frames = planFrames(plan);
  return frames === null ? null : frames * plan.width * plan.height * 4;
}

/** More frames than the budget buys: the import is refused, not trimmed. */
export function overBudget(plan: ImportPlan): boolean {
  const frames = planFrames(plan);
  return frames !== null && frames > plan.cap;
}

/** The rate the imported frames play at, the drop taken out. */
export function playbackFps(plan: ImportPlan): number {
  const decoded = decodedFrames(plan);
  const counts: [number, number] | null =
    decoded === null ? null : [decoded, keptFrames(decoded, plan.dropNth)];
  return keptRate(plan.fps, plan.dropNth, counts);
}

/**
 * Desktop `import_planned`'s decode ceiling: `cap` counts kept frames, so the
 * drop is added back. A backstop for streams with no duration.
 */
export function decodeCap(plan: ImportPlan): number {
  return plan.dropNth === null ? plan.cap : decodedForKept(plan.cap, plan.dropNth) + 1;
}

/** Desktop `ffmpeg_rgba`: decode `path` from `startS` to raw RGBA in `output`. */
export function ffmpegRgba(path: string, startS: number, options: string[], output: string) {
  const seek = startS > 0 ? ["-ss", startS.toFixed(3)] : [];
  return [
    "-v",
    "error",
    ...seek,
    "-i",
    path,
    ...options,
    "-f",
    "rawvideo",
    "-pix_fmt",
    "rgba",
    output,
  ];
}

/** Desktop `import_planned`'s ffmpeg arguments for exactly this plan. */
export function decodeArgs(plan: ImportPlan, path: string, output: string): string[] {
  const options: string[] = [];
  if (plan.spanS !== null) options.push("-t", plan.spanS.toFixed(3));
  const filters: string[] = [];
  if (plan.width !== plan.source.width || plan.height !== plan.source.height) {
    filters.push(`scale=${plan.width}:${plan.height}`);
  }
  if (Math.abs(plan.fps - plan.source.fps) > 0.01) filters.push(`fps=${plan.fps}`);
  if (filters.length > 0) options.push("-vf", filters.join(","));
  options.push("-frames:v", String(decodeCap(plan)));
  return ffmpegRgba(path, plan.startS, options, output);
}

/**
 * Desktop `sample_frames`: one seek-and-decode per sample, spread across what
 * the plan takes, at its output size.
 */
export function sampleArgs(plan: ImportPlan, count: number, path: string, output: string) {
  const seconds = secondsTaken(plan.source, plan.startS, plan.spanS);
  const spread = seconds !== null && seconds > 0.5 ? seconds : null;
  const options = ["-frames:v", "1", "-vf", `scale=${plan.width}:${plan.height}`];
  return Array.from({ length: Math.max(count, 1) }, (_, i) => {
    const at =
      spread === null
        ? plan.startS + i / Math.max(plan.fps, 1)
        : plan.startS + (spread * (i + 0.5)) / count;
    return ffmpegRgba(path, at, options, output);
  });
}

// ---- The trim fields ----

/** What the two trim fields say: `invalid` never falls back to the whole clip. */
export type TrimInput = { kind: "whole" } | { kind: "set"; trim: Trim } | { kind: "invalid" };

/** A frame number, as typed. */
export function parseIndex(text: string): number | null {
  const trimmed = text.trim();
  return /^\d+$/.test(trimmed) ? Number(trimmed) : null;
}

/** `12.5`, `0:01.12`, `1:02:03`; rounded to the centisecond a GIF can hold. */
export function parseSeconds(text: string): number | null {
  const trimmed = text.trim();
  if (!trimmed) return null;
  const parts = trimmed.split(":");
  if (parts.length > 3) return null;
  let total = 0;
  for (const part of parts) {
    const piece = part.trim();
    // What Rust's `f64::from_str` takes, less the infinities and negatives.
    if (!/^\+?(\d+\.?\d*|\.\d+)(e[+-]?\d+)?$/i.test(piece)) return null;
    total = total * 60 + Number(piece);
  }
  return Math.round(total * 100) / 100;
}

/** Desktop `read_trim`: blank is the whole clip, one end alone is open at the other. */
export function readTrim(
  inFrames: boolean,
  start: string,
  end: string,
  source: VideoInfo,
): TrimInput {
  const [from, to] = [start.trim(), end.trim()];
  if (!from && !to) return { kind: "whole" };
  return inFrames ? readFrameTrim(from, to, source) : readSecondsTrim(from, to, source);
}

function readFrameTrim(from: string, to: string, source: VideoInfo): TrimInput {
  const frames = sourceFrames(source);
  const within = (n: number) => frames === null || n <= Math.max(frames - 1, 0);
  const first = from ? parseIndex(from) : 0;
  if (first === null || !within(first)) return { kind: "invalid" };
  const last = to ? parseIndex(to) : null;
  if (to && (last === null || last <= first || !within(last))) return { kind: "invalid" };
  return { kind: "set", trim: { kind: "frames", first, last } };
}

function readSecondsTrim(from: string, to: string, source: VideoInfo): TrimInput {
  const within = (s: number) => source.durationS === null || s <= source.durationS;
  const start = from ? parseSeconds(from) : 0;
  if (start === null || !within(start)) return { kind: "invalid" };
  const end = to ? parseSeconds(to) : null;
  if (to && (end === null || end <= start || !within(end))) return { kind: "invalid" };
  return { kind: "set", trim: { kind: "seconds", start, end } };
}

/** `HH:MM:SS.mmm`, what the trim fields are measured against. */
export function timestamp(seconds: number): string {
  const ms = Math.round(Math.max(seconds, 0) * 1000);
  const pad = (n: number, width = 2) => String(n).padStart(width, "0");
  return `${pad(Math.floor(ms / 3_600_000))}:${pad(Math.floor(ms / 60_000) % 60)}:${pad(
    Math.floor(ms / 1000) % 60,
  )}.${pad(ms % 1000, 3)}`;
}

/** A second in the seconds field; ten frames in the frames field. */
export const TRIM_STEP_S = 1;
export const TRIM_STEP_FRAMES = 10;

/** Desktop `stepped_trim`: a blank field steps from the end it stands for; clamped inside the file. */
export function steppedTrim(
  text: string,
  forward: boolean,
  inFrames: boolean,
  isEnd: boolean,
  source: VideoInfo,
): string {
  if (inFrames) {
    const frames = sourceFrames(source);
    const last = frames === null ? null : Math.max(frames - 1, 0);
    const current = parseIndex(text) ?? (isEnd ? (last ?? 0) : 0);
    const stepped = Math.max(current + (forward ? TRIM_STEP_FRAMES : -TRIM_STEP_FRAMES), 0);
    return String(last === null ? stepped : Math.min(stepped, last));
  }
  const end = source.durationS ?? Number.MAX_VALUE;
  const current = parseSeconds(text) ?? (isEnd ? end : 0);
  const stepped = current + (forward ? TRIM_STEP_S : -TRIM_STEP_S);
  return timestamp(Math.min(Math.max(stepped, 0), end));
}

/** The trim fields' placeholders: the file's own first and last moment or frame. */
export function trimExamples(inFrames: boolean, source: VideoInfo): [string, string] {
  const frames = sourceFrames(source);
  if (inFrames) return ["0", frames === null ? "1234" : String(Math.max(frames - 1, 0))];
  return [timestamp(0), source.durationS === null ? "00:01:23.450" : timestamp(source.durationS)];
}

/**
 * Desktop `needs_ffmpeg`: what is neither a GIF nor a still image goes to
 * ffmpeg, including formats that can animate (WebP, APNG).
 */
export function needsFfmpeg(name: string): boolean {
  const extension = /\.([^.]+)$/.exec(name)?.[1]?.toLowerCase() ?? "";
  return extension !== "gif" && STILL_EXTENSIONS[extension] !== true;
}

/** Desktop `is_still_image`. */
const STILL_EXTENSIONS: Record<string, true> = {
  png: true,
  jpg: true,
  jpeg: true,
  bmp: true,
  tif: true,
  tiff: true,
  tga: true,
  ico: true,
  qoi: true,
  ppm: true,
  pgm: true,
  pbm: true,
  pnm: true,
};
