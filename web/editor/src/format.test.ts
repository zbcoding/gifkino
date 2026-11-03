import { expect, test } from "vite-plus/test";
import { byteSize, fps, timecode } from "./format";
import { estimateText } from "./ui/sidebar";
import { initialState } from "./state";
import { docWith } from "./test-support";

test("the transport clock reads minutes, seconds and tenths", () => {
  expect(timecode(0)).toBe("00:00.0");
  expect(timecode(6789)).toBe("01:07.8");
});

test("fps is the document's average rate, rounded, with no rate for no duration", () => {
  expect(fps(4, 40)).toBe(10);
  expect(fps(3, 10)).toBe(30);
  expect(fps(0, 0)).toBe(0);
});

test("sizes switch unit at a binary megabyte", () => {
  expect(byteSize(1023 * 1024)).toBe("1023 KB");
  expect(byteSize(2 ** 20)).toBe("1 MB");
  expect(byteSize(3 * 2 ** 30)).toBe("3.0 GB");
});

test("an estimate from an older revision stays up, marked as updating", () => {
  const doc = docWith([10], { revision: 5 });
  const state = { ...initialState, doc, estimate: { revision: 4, bytes: 2048 } };
  expect(estimateText(state)).toBe("≈ 2 KB as a GIF (updating…)");
  expect(estimateText({ ...state, estimate: { revision: 5, bytes: 2048 } })).toBe(
    "≈ 2 KB as a GIF",
  );
  expect(estimateText({ ...state, estimate: null })).toBe("sizing…");
});
