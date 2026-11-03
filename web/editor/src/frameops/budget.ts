// The desktop's memory guard (`settings.rs` budgets, `actions.rs`
// `resize_fits_budget(s)`). The wasm core does not check, and a heap that
// passes 4 GiB loses the module, so the page refuses before asking.

const MIB = 1024 * 1024;

/** Desktop `Settings` defaults: `max_import_bytes`, `max_oper_bytes`, `max_total_bytes`. */
export const budgets = {
  import: 4096 * MIB,
  oper: 4096 * MIB,
  total: 8192 * MIB,
};

// ponytail: wasm32's 4 GiB address space holds the old frames, the new ones
// and everything else, so the desktop's 8 GiB total can never be reached
// here; capping old + new at 4 GiB keeps a refused resize a toast instead of
// a dead worker. A settings slice that makes the budgets editable keeps this.
const WASM_HEAP = 4096 * MIB;

/** RGBA bytes `frames` frames of `w × h` hold: desktop `resize_bytes`. */
export function frameBytes(w: number, h: number, frames: number): number {
  return w * h * 4 * frames;
}

/** Desktop `resize_fits_budget`: the landing frames of a splice under the import budget. */
export function spliceFits(w: number, h: number, frames: number): boolean {
  return frameBytes(w, h, frames) <= Math.min(budgets.import, WASM_HEAP);
}

/**
 * Desktop `resize_fits_budgets`: the new frames within the operation budget,
 * and old plus new within the total, since undo keeps the old ones.
 */
export function resizeFits(cw: number, ch: number, w: number, h: number, frames: number): boolean {
  const before = frameBytes(cw, ch, frames);
  const after = frameBytes(w, h, frames);
  return after <= budgets.oper && before + after <= Math.min(budgets.total, WASM_HEAP);
}
