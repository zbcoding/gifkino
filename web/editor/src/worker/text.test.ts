import { describe, expect, test } from "vite-plus/test";
import { cssFont, wrap } from "./text";

describe("cssFont reads Pango descriptions the desktop stores", () => {
  test("the default caption font", () => {
    expect(cssFont("Sans Bold", 32)).toBe("bold 32px sans-serif");
  });

  test("style words, a named family and the description's own size, which is ignored", () => {
    expect(cssFont("DejaVu Serif Semi-Bold Italic 12", 20)).toBe('600 italic 20px "DejaVu Serif"');
  });

  test("a family whose last word is not a style keeps every word", () => {
    expect(cssFont("Noto Sans Mono", 10)).toBe('10px "Noto Sans Mono"');
  });

  test("a bare style word is a family, not an empty description", () => {
    expect(cssFont("Bold", 10)).toBe('10px "Bold"');
  });

  test("family lists keep their fallbacks", () => {
    expect(cssFont("Cantarell, Sans 11", 14)).toBe('14px "Cantarell", sans-serif');
  });
});

describe("wrap breaks like Pango's WordChar mode", () => {
  // One unit per character: widths are character counts.
  const measure = (s: string) => s.length;
  const texts = (text: string, width: number) =>
    wrap(text, width, measure).map((line) => line.text);

  test("between words while words fit", () => {
    expect(texts("one two three", 7)).toEqual(["one two", "three"]);
  });

  test("inside a word only when it cannot fit a line by itself", () => {
    expect(texts("hi abcdefghij", 4)).toEqual(["hi", "abcd", "efgh", "ij"]);
  });

  test("newlines start paragraphs, and only each paragraph's last line is final", () => {
    const lines = wrap("aa bb\ncc", 2, measure);
    expect(lines).toEqual([
      { text: "aa", last: false },
      { text: "bb", last: true },
      { text: "cc", last: true },
    ]);
  });

  test("an empty paragraph still takes a line", () => {
    expect(texts("a\n\nb", 5)).toEqual(["a", "", "b"]);
  });
});
