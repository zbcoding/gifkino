import { expect, test } from "vite-plus/test";
import { DEFAULT_MODIFIERS, modsHeld, modsLabel } from "./modals";

const keys = (ctrlKey: boolean, shiftKey: boolean, altKey: boolean) => ({
  ctrlKey,
  shiftKey,
  altKey,
});

// Desktop `Mods::held`: a superset still matches, so Alt+Shift rotates and
// keeps the aspect at once; an unbound modifier must never fire, or clearing
// a binding would turn every drag into a rotation.
test("canvas modifiers match supersets and never fire unbound", () => {
  const rotate = DEFAULT_MODIFIERS["canvas-rotate"];
  expect(modsHeld(rotate, keys(false, true, true))).toBe(true);
  expect(modsHeld(rotate, keys(false, true, false))).toBe(false);
  expect(modsHeld({ ctrl: false, shift: false, alt: false }, keys(true, true, true))).toBe(false);
  expect(modsHeld({ ctrl: true, shift: true, alt: false }, keys(true, false, false))).toBe(false);
});

test("modifiers read the way the desktop writes them", () => {
  expect(modsLabel(DEFAULT_MODIFIERS["canvas-rotate"])).toBe("Alt");
  expect(modsLabel({ ctrl: true, shift: true, alt: true })).toBe("Ctrl+Alt+Shift");
});
