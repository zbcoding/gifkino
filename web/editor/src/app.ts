// The one object every slice gets in `install(app)`: state, the worker, and the
// registries the shell's layout reads. See ARCHITECTURE.md.

import { runGifsicle } from "./gifsicle";
import { fill, t } from "./i18n";
import { DEFAULT_MODIFIERS } from "./modals";
import type { Modal, Mods } from "./modals";
import type { Client, RequestOptions } from "./rpc";
import { edited, initialState, opened, Store } from "./state";
import type { AppState } from "./state";
import type {
  EditorMethod,
  FreeFunction,
  FreeFunctions,
  FrameSource,
  MethodArgs,
  MethodResult,
  Picture,
  Staged,
} from "./worker/protocol";
import type { Change, ExportSettings } from "./wasm/gifkino_wasm.js";

/** Header menus, in the desktop's order; `file` and `app` live in the ☰ menu. */
export const MENUS = ["frames", "insert", "effects", "image", "optimize", "file", "app"] as const;
export type MenuName = (typeof MENUS)[number];

export interface Command {
  /** Stable name: the keybinding target. */
  id: string;
  /** Untranslated desktop label; shown through `t`. */
  label: string;
  /** Translated tooltip when it says more than the label ("Undo Frames deleted"). */
  tooltip?: (state: AppState) => string;
  /** Menu the command appears in; none for keyboard-only commands. */
  menu?: MenuName;
  /** Menu items with different sections are split by a separator. */
  section?: string;
  /** Greyed out (and not run) when this says no. Default: always enabled. */
  enabled?: (state: AppState) => boolean;
  run: () => unknown;
}

/** Layout regions slices fill; see ARCHITECTURE.md for where each sits. */
export type Slot = "rail" | "transport" | "strip" | "bands" | "sidebar";

export interface Panel {
  slot: Slot;
  id: string;
  /** Ascending within the slot. */
  order: number;
  element: HTMLElement;
}

/**
 * Where the image sits on the canvas overlay. Screen coordinates are CSS
 * pixels from the overlay canvas's top-left: a pointer event's `offsetX/Y`.
 */
export interface View {
  /** Screen pixels per image pixel. */
  scale: number;
  /** The image's top-left corner on the overlay canvas. */
  left: number;
  top: number;
  toImage(screenX: number, screenY: number): [number, number];
  toScreen(imageX: number, imageY: number): [number, number];
}

export type PointerPhase = "down" | "move" | "up";

export interface CanvasLayer {
  id: string;
  /** Ascending: higher draws later and sees pointer events first. */
  order: number;
  /** Draw in CSS pixels; `ctx` is already scaled for the device pixel ratio. */
  draw?(ctx: CanvasRenderingContext2D, view: View, state: AppState): void;
  /** Return `true` to take the event; the rest of the stack then never sees it. */
  pointer?(event: PointerEvent, phase: PointerPhase, view: View): boolean;
}

export interface Importer {
  /** Added to the file picker's `accept`. */
  accept: string;
  matches(file: File): boolean;
  /**
   * Read `file` into frames; `null` when the user cancelled. Anything slower
   * than reading the bytes runs under its own `withBusy`.
   */
  load(file: File): Promise<FrameSource | null>;
}

const builtinImporter: Importer = {
  accept: "image/*,.gif",
  matches: () => true,
  async load(file) {
    const bytes = new Uint8Array(await file.arrayBuffer());
    const isGif = file.type === "image/gif" || /\.gif$/i.test(file.name);
    return { kind: isGif ? "gif" : "image", bytes };
  },
};

/** Debounce before measuring the export size again: the desktop's 5 s. */
const ESTIMATE_DEBOUNCE_MS = 5000;

export class App {
  readonly store = new Store<AppState>(initialState);
  readonly commands = new Map<string, Command>();
  readonly panels: Panel[] = [];
  readonly layers: CanvasLayer[] = [];
  readonly importers: Importer[] = [];
  /** Keybinding seam: the chord shown beside a command, e.g. "Ctrl+Z". */
  shortcutFor: (commandId: string) => string | null = () => null;
  /** Keybinding seam: the modifiers that turn a canvas drag into a rotate, keep-aspect or from-centre drag. */
  modifiersFor: (modal: Modal) => Mods = (modal) => DEFAULT_MODIFIERS[modal];
  /** Set by the canvas view: repaint the overlay layers. */
  redrawCanvas: () => void = () => {};
  /** Set by the layout: show a transient message; `undo` adds the edit toast's Undo button. */
  toast: (message: string, options?: { undo?: boolean }) => void = () => {};

  /**
   * Desktop `toast_if_document_wide`: an edit earns a toast only when it
   * reached every frame the document had, `total` being the count before the
   * edit. Without `total` it always toasts (a splice). `undefined`: no change.
   */
  toastChange(change: Change | undefined, total?: number): void {
    if (!change || change.framesTouched < Math.max(total ?? 0, 1)) return;
    this.toast(changeMessage(change), { undo: true });
  }

  readonly request: Client;

  constructor(request: Client) {
    this.request = request;
  }

  get state(): AppState {
    return this.store.state;
  }

  update(change: Partial<AppState> | ((state: AppState) => AppState)): void {
    this.store.update(change);
  }

  // ---- Registries: call during `install`, before the layout is built. ----

  registerCommand(command: Command): void {
    if (this.commands.has(command.id)) throw new Error(`Command ${command.id} registered twice`);
    this.commands.set(command.id, command);
  }

  registerPanel(panel: Panel): void {
    this.panels.push(panel);
  }

  registerCanvasLayer(layer: CanvasLayer): void {
    this.layers.push(layer);
  }

  registerImporter(importer: Importer): void {
    this.importers.push(importer);
  }

  isEnabled(command: Command): boolean {
    return command.enabled?.(this.state) ?? true;
  }

  /** Run a command by id; failures become a toast rather than a lost promise. */
  async runCommand(id: string): Promise<void> {
    const command = this.commands.get(id);
    if (!command || !this.isEnabled(command)) return;
    try {
      await command.run();
    } catch (error) {
      this.toast(error instanceof Error ? error.message : String(error));
    }
  }

  // ---- The document ----

  /**
   * Every document edit goes through here: run `Editor[method]` in the worker,
   * then refresh `state.doc` and clamp the indices that point into it.
   */
  async dispatch<M extends EditorMethod>(
    method: M,
    ...args: MethodArgs<M>
  ): Promise<MethodResult<M>> {
    const { value, doc } = await this.request("call", { method, args }, { snapshot: true });
    this.update((state) => edited(state, doc!));
    return value as MethodResult<M>;
  }

  /** A read-only `Editor` call (`overlayAt`, `dragOverlay`, …): no refresh. */
  async query<M extends EditorMethod>(method: M, ...args: MethodArgs<M>): Promise<MethodResult<M>> {
    const { value } = await this.request("call", { method, args });
    return value as MethodResult<M>;
  }

  /** One of the module's free functions: `orientedCorners`, `handleAt`, `contains`, `fitCanvas`, `delaysForFps`, `estimateGifSize`. */
  async compute<F extends FreeFunction>(
    name: F,
    ...args: Parameters<FreeFunctions[F]>
  ): Promise<ReturnType<FreeFunctions[F]>> {
    const { value } = await this.request("compute", { name, args });
    return value as ReturnType<FreeFunctions[F]>;
  }

  async composite(index: number): Promise<Picture | undefined> {
    return (await this.request("composite", { index })).value;
  }

  async thumbnail(index: number): Promise<Picture | undefined> {
    return (await this.request("thumbnail", { index })).value;
  }

  /** Decode `source` in the worker and hold it for `open` or `splice`. */
  async stage(source: FrameSource, report?: (decoded: number) => void) {
    const options: RequestOptions = { transfer: sourceBuffers(source), progress: report };
    return (await this.request("stage", { source }, options)).value;
  }

  /**
   * Read `file` through the first importer that takes it and stage its
   * frames under the import busy bar: the first half of `openFile`, and what
   * "Add frames from file…" splices. `null`: the importer was cancelled.
   */
  async stageFile(file: File): Promise<Staged | null> {
    const importer = this.importers.find((candidate) => candidate.matches(file)) ?? builtinImporter;
    const source = await importer.load(file);
    if (!source) return null;
    return this.withBusy(t("Importing… {done} frames"), (report) =>
      this.stage(source, (decoded) => report(decoded, null)),
    );
  }

  /** Replace the document with `file`. */
  async openFile(file: File): Promise<void> {
    if (!(await this.stageFile(file))) return;
    const { doc } = await this.request("open", {}, { snapshot: true });
    this.update((state) => opened(state, doc!, file.name));
  }

  /** Every importer's `accept`, for the file picker. */
  get accept(): string {
    return [...this.importers, builtinImporter].map((importer) => importer.accept).join(",");
  }

  /** Encode the document, optimize it with gifsicle and download it, named after the opened file. */
  async exportGif(settings: ExportSettings | null = null): Promise<void> {
    const { doc, fileName } = this.state;
    if (!doc || doc.frameCount === 0) return;
    const name = `${fileName?.replace(/\.[^.]*$/, "") || "export"}.gif`;
    // Translators: Progress while the document is being written out as a GIF; both values are frame counts.
    const bytes = await this.withBusy(t("Exporting… {done} / {total} frames"), async (report) => {
      report(0, doc.frameCount);
      const reply = await this.request("exportGif", { settings }, { progress: report });
      return optimized(reply.value, settings?.lossy ?? 0);
    });
    download(new Blob([bytes], { type: "image/gif" }), name);
    this.update({ dirty: false });
    this.toast(
      fill(t("Exported to {path} · {size} KB"), {
        path: name,
        size: Math.round(bytes.length / 1024),
      }),
    );
  }

  /**
   * Show the busy bar while `job` runs; `label` is a template over `{done}`
   * and `{total}`. `cancel` adds the bar's Cancel button.
   */
  async withBusy<T>(
    label: string,
    job: (report: (done: number, total: number | null) => void) => Promise<T>,
    cancel?: () => void,
  ): Promise<T> {
    const report = (done: number, total: number | null) =>
      this.update({ busy: { label, done, total, cancel } });
    report(0, null);
    try {
      return await job(report);
    } finally {
      this.update({ busy: null });
    }
  }

  /** Keep `state.estimate` measured, debounced after edits like the desktop. */
  startEstimates(): void {
    let timer: number | undefined;
    this.store.subscribe((state, previous) => {
      if (state.doc?.revision === previous.doc?.revision) return;
      clearTimeout(timer);
      if (!state.doc || state.doc.frameCount === 0) return;
      const revision = state.doc.revision;
      // A fresh document has no figure at all yet, so it measures at once.
      const wait = state.estimate ? ESTIMATE_DEBOUNCE_MS : 0;
      timer = setTimeout(() => void this.measure(revision), wait);
    });
  }

  private async measure(revision: number): Promise<void> {
    if (this.state.doc?.revision !== revision) return;
    // ponytail: measuring blocks the worker (~8 frames encoded), so a frame
    // composite queued behind it waits; a second worker would avoid that.
    const bytes = await this.query("estimateSize").catch(() => null);
    if (bytes === null || this.state.doc?.revision !== revision) return;
    this.update({ estimate: { revision, bytes } });
  }
}

/** The buffers a `FrameSource` owns, moved to the worker instead of copied. */
function sourceBuffers(source: FrameSource): Transferable[] {
  if (source.kind === "rgba") return source.frames.map((frame) => frame.rgba.buffer);
  return [source.bytes.buffer];
}

/**
 * Desktop `export_path`'s gifsicle pass. A gifsicle that will not load or run
 * leaves the unoptimized file, which is still a valid GIF, as the desktop does
 * when gifsicle is not installed.
 */
async function optimized(gif: Uint8Array<ArrayBuffer>, lossy: number) {
  try {
    return await runGifsicle(gif, lossy);
  } catch (error) {
    console.warn("Exporting without gifsicle:", error);
    return gif;
  }
}

export function download(blob: Blob, name: string): void {
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = name;
  link.click();
  // The download starts asynchronously; revoking at once can cancel it.
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
}

/** Desktop `change_message`: "Text added on 24 frames", or the bare label for one frame. */
export function changeMessage(change: Change): string {
  const label = t(change.label);
  if (change.framesTouched <= 1) return label;
  // Translators: Toast after an edit. {change} is a past-tense edit name such as "Frames deleted".
  return fill(t("{change} on {count} frames"), { change: label, count: change.framesTouched });
}
