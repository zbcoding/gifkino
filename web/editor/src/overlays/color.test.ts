import { describe, expect, test } from "vite-plus/test";
import { hexOf, hsvToRgb, hueAt, parseHex, QUICK, rgbToHsv } from "./color";

describe("colour picker", () => {
  // The picker stores HSV and hands back RGB, so a colour the document set has
  // to come out as the same bytes it went in as, or showing it would edit it.
  test("every byte survives the trip through HSV", () => {
    for (const [r, g, b] of QUICK) expect(hsvToRgb(...rgbToHsv(r, g, b))).toEqual([r, g, b]);
    for (let c = 0; c <= 255; c++) expect(hsvToRgb(...rgbToHsv(c, c, c))).toEqual([c, c, c]);
  });

  // The cursor's placement is the inverse of the wheel's shading; if the two
  // disagree the marker sits on a colour that is not the one in hand.
  test("the wheel maps a hue to the pixel it was drawn at", () => {
    for (const hue of [0, 45, 120, 200, 359]) {
      const angle = (hue / 360) * 2 * Math.PI - Math.PI;
      const hit = hueAt(-42 * Math.cos(angle), 42 * Math.sin(angle));
      expect(Math.abs(hit - hue)).toBeLessThan(0.001);
    }
  });

  test("hex round-trips and rejects the rest", () => {
    expect(parseHex("#FF3C3C")).toEqual([255, 60, 60, 255]);
    expect(parseHex("ff3c3c80")).toEqual([255, 60, 60, 128]);
    expect(hexOf([255, 60, 60, 255])).toBe("#FF3C3C");
    expect(hexOf([255, 60, 60, 128])).toBe("#FF3C3C80");
    for (const bad of ["", "#12345", "#GGGGGG", "#FF3C3C3", "rgb(1,2,3)"]) {
      expect(parseHex(bad)).toBeNull();
    }
  });
});
