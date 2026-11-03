import { expect, test } from "vite-plus/test";
import { cornerCursor } from "./canvas";

// The glyph has to point along the rotated edge, or the cursor promises a
// resize in a direction the drag will not go.
test("the resize cursor turns with the box", () => {
  expect(cornerCursor(0, 0)).toBe("nw-resize");
  expect(cornerCursor(3, 0)).toBe("se-resize");
  // A quarter turn clockwise carries the top-left corner round to the top right.
  expect(cornerCursor(0, Math.PI / 2)).toBe("ne-resize");
  // Half a turn puts it opposite itself, and the glyph is symmetric.
  expect(cornerCursor(0, Math.PI)).toBe("se-resize");
  // Corners never borrow an edge glyph, whatever the angle.
  for (let corner = 0; corner < 4; corner++) {
    for (let step = 0; step < 24; step++) {
      expect(cornerCursor(corner, step * 0.3).length).toBeGreaterThan("n-resize".length);
    }
  }
});
