import { describe, expect, test } from "vite-plus/test";
import { bandAt, cellPitch, gapAt, nextStripZoom, packRows, thumbSize } from "./geometry";

describe("strip geometry", () => {
  test("thumbnails fit the 72 px box on their long side, like the core's", () => {
    expect(thumbSize(400, 300)).toEqual([72, 54]);
    expect(thumbSize(1, 1000)).toEqual([1, 72]);
  });

  test("the spacing between cells is not zoomed, or the bands slide off their frames", () => {
    expect(cellPitch(72, 2)).toBe(148);
  });

  test("a drop lands in the gap whose side of the cell's midpoint the pointer is on", () => {
    // Cells 72 wide at pitch 76: cell 1 spans 76..148, midpoint 112.
    expect(gapAt(111, 76, 72, 5)).toBe(1);
    expect(gapAt(113, 76, 72, 5)).toBe(2);
    expect(gapAt(-20, 76, 72, 5)).toBe(0);
    expect(gapAt(10_000, 76, 72, 5)).toBe(5);
  });

  test("zoom steps clamp to their bounds and 0 resets", () => {
    expect(nextStripZoom(2.9, 1.2)).toBe(3);
    expect(nextStripZoom(0.21, 1 / 1.2)).toBe(0.2);
    expect(nextStripZoom(2.5, 0)).toBe(1);
  });
});

describe("band rows", () => {
  test("an overlay takes the lowest row free over its frames, not its list position", () => {
    const rows = packRows([
      { start: 0, end: 3 },
      { start: 2, end: 5 },
      { start: 5, end: 8 },
      { start: 3, end: 6 },
    ]);
    expect(rows).toEqual([0, 1, 0, 2]);
  });

  test("a click finds the band on that row and frame among overlays sharing a row", () => {
    const band = (id: number, start: number, end: number) => ({
      id,
      name: "",
      range: { start, end },
      row: 0,
      selected: false,
    });
    const bands = [band(1, 0, 2), band(2, 4, 6)];
    expect(bandAt(bands, 4 * 76 + 10, 5, 76)).toBe(2);
    expect(bandAt(bands, 3 * 76, 5, 76)).toBeNull();
    expect(bandAt(bands, 10, 20, 76)).toBeNull();
  });
});
