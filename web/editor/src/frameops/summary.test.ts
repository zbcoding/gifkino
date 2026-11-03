import { describe, expect, test } from "vite-plus/test";
import { resizeFits, spliceFits } from "./budget";
import { frameScopeSummary, pairedDimension, smartDropCount } from "./summary";

describe("frameScopeSummary", () => {
  test("names the frames in scope the way the desktop sidebar does", () => {
    expect(frameScopeSummary("frame", [3])).toBe("Frame");
    expect(frameScopeSummary("all", [0, 1, 2])).toBe("All 3 frames selected");
    expect(frameScopeSummary("range", [2, 3, 4])).toBe("Frames 3–5 selected");
    expect(frameScopeSummary("range", [0, 2, 9])).toBe("Frames 1, 3, 10 selected");
    expect(frameScopeSummary("range", [0, 2, 4, 6, 8, 10, 12, 14, 16])).toBe("9 frames selected");
  });
});

test("pairedDimension keeps the ratio inside the size field's range", () => {
  expect(pairedDimension(240, 480, 320)).toBe(160);
  expect(pairedDimension(10, 100, 100)).toBe(16);
  expect(pairedDimension(8000, 100, 200)).toBe(8192);
  expect(pairedDimension(50, 0, 10)).toBe(50);
});

test("smartDropCount caps at 95 %", () => {
  expect(smartDropCount(10, 30)).toBe(3);
  expect(smartDropCount(20, 100)).toBe(19);
  expect(smartDropCount(3, 5)).toBe(0);
});

test("the memory guard refuses what the wasm heap cannot hold", () => {
  // 4096² × 4 × 40 = 2.5 GiB new, fine for the desktop's 4 GiB per
  // operation, but old + new must also fit the 4 GiB heap.
  expect(resizeFits(4096, 4096, 4096, 4096, 40)).toBe(false);
  expect(resizeFits(480, 320, 960, 640, 100)).toBe(true);
  expect(spliceFits(1920, 1080, 400)).toBe(true);
  expect(spliceFits(8192, 8192, 20)).toBe(false);
});
