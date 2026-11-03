import germanCatalog from "../../../po/de.po?raw";
import { describe, expect, test } from "vite-plus/test";
import { LANGUAGES, parsePo, resolveLocale } from "./i18n";

describe("parsePo", () => {
  test("reads entries and skips the header and untranslated ones", () => {
    const catalog = parsePo(
      'msgid ""\nmsgstr "Content-Type: text/plain; charset=UTF-8\\n"\n\n' +
        '#. Translators: the export button\nmsgid "Export GIF"\nmsgstr "GIF exportieren"\n\n' +
        'msgid "Undo"\nmsgstr ""\n',
    );
    expect([...catalog]).toEqual([["Export GIF", "GIF exportieren"]]);
  });

  // Fuzzy marks an AI draft awaiting review; msgfmt would drop it and nobody would ever see it.
  test("fuzzy entries are used", () => {
    const catalog = parsePo(
      '#. AI-generated translation; human review requested.\n#, fuzzy\nmsgid "Crop all frames…"\nmsgstr "Alle Bilder zuschneiden…"\n',
    );
    expect(catalog.get("Crop all frames…")).toBe("Alle Bilder zuschneiden…");
  });

  test("continuation lines join and escapes unescape", () => {
    const catalog = parsePo(
      'msgid ""\n"Drag a box "\n"on the canvas."\nmsgstr ""\n"Ziehen Sie einen Rahmen\\n"\n"auf der \\"Leinwand\\"."\n',
    );
    expect(catalog.get("Drag a box on the canvas.")).toBe(
      'Ziehen Sie einen Rahmen\nauf der "Leinwand".',
    );
  });

  test("contextual and plural entries are skipped, not mangled", () => {
    const catalog = parsePo(
      'msgctxt "menu"\nmsgid "Open"\nmsgstr "Öffnen"\n\nmsgid "One"\nmsgid_plural "Many"\nmsgstr[0] "Eins"\n\nmsgid "Redo"\nmsgstr "Wiederholen"\n',
    );
    expect([...catalog]).toEqual([["Redo", "Wiederholen"]]);
  });

  // What ships: the German catalog actually translates the menus.
  test("the shipped German catalog parses", () => {
    const de = parsePo(germanCatalog);
    expect(de.get("Undo")).toBe("Rückgängig");
    expect(de.get("Settings…")).toBeTruthy();
  });
});

describe("resolveLocale", () => {
  const available = ["de", "en_US", "ja", "pt_BR"];

  test("a region narrows to its language: de-DE uses de", () => {
    expect(resolveLocale(null, ["de-DE"], available)).toBe("de");
    expect(resolveLocale(null, ["pt-BR"], available)).toBe("pt_BR");
  });

  test("the browser's languages are tried in order", () => {
    expect(resolveLocale(null, ["fr-FR", "ja", "de"], available)).toBe("ja");
    expect(resolveLocale(null, ["en-US", "de"], available)).toBe("en_US");
    expect(resolveLocale(null, ["fr"], available)).toBeNull();
  });

  test("the setting wins, and a setting with no catalog is English", () => {
    expect(resolveLocale("ja", ["de-DE"], available)).toBe("ja");
    expect(resolveLocale("fr", ["de-DE"], available)).toBeNull();
  });

  test("every po/ catalog is bundled", () => {
    expect(LANGUAGES).toEqual(expect.arrayContaining(["de", "en_US", "es", "ja", "pt_BR"]));
  });
});
