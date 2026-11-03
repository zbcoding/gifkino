// Modifiers held during a canvas drag: desktop `keymap::Modal` / `Mods`. A
// chord needs a key; these are the modifier alone, so they are not commands.
// `app.modifiersFor(modal)` reads them; the keybindings slice replaces it.

/** Desktop `Modal::id`, also the keybindings file key. */
export type Modal = "canvas-rotate" | "canvas-keep-aspect" | "canvas-from-center";

/** Empty means unbound, which is never satisfied. */
export interface Mods {
  ctrl: boolean;
  shift: boolean;
  alt: boolean;
}

/** Desktop `keymap::MODALS`, in order, with their untranslated labels. */
export const MODALS: { id: Modal; label: string }[] = [
  // Translators: Held while dragging an overlay on the canvas.
  { id: "canvas-rotate", label: "Rotate instead of move" },
  { id: "canvas-keep-aspect", label: "Keep the aspect ratio" },
  { id: "canvas-from-center", label: "Resize from the center" },
];

/** The desktop's default `Keymap` modals: Alt, Shift, Ctrl. */
export const DEFAULT_MODIFIERS: Record<Modal, Mods> = {
  "canvas-rotate": { ctrl: false, shift: false, alt: true },
  "canvas-keep-aspect": { ctrl: false, shift: true, alt: false },
  "canvas-from-center": { ctrl: true, shift: false, alt: false },
};

/** Whether `mods` are all down. A superset still matches, so Alt+Shift rotates and constrains. */
export function modsHeld(
  mods: Mods,
  event: { ctrlKey: boolean; shiftKey: boolean; altKey: boolean },
): boolean {
  const bound = mods.ctrl || mods.shift || mods.alt;
  return (
    bound &&
    (!mods.ctrl || event.ctrlKey) &&
    (!mods.shift || event.shiftKey) &&
    (!mods.alt || event.altKey)
  );
}

/** Desktop `Mods::display`: "Ctrl+Alt+Shift" order. */
export function modsLabel(mods: Mods): string {
  return [mods.ctrl && "Ctrl", mods.alt && "Alt", mods.shift && "Shift"].filter(Boolean).join("+");
}
