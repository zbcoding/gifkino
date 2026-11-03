import { expect, test } from "vite-plus/test";
import { defaultSettings, parseSettings, settingsText } from "./conf";

const MIB = 1024 * 1024;

// The file is hand-editable (and importable from the desktop), so a bad line
// must fall back to the default rather than break the editor.
test("unparseable lines fall back to the defaults", () => {
  expect(parseSettings("max_import_memory_mb = 512").maxImportBytes).toBe(512 * MIB);
  expect(parseSettings("  max_oper_memory_mb=64  ").maxOperBytes).toBe(64 * MIB);
  expect(parseSettings("max_total_memory_mb = 10240").maxTotalBytes).toBe(10240 * MIB);
  expect(parseSettings("language = de").language).toBe("de");
  expect(parseSettings("language =").language).toBeNull();
  for (const junk of [
    "",
    "# max_import_memory_mb = 512",
    "max_import_memory_mb = 0",
    "max_import_memory_mb = lots",
    "max_total_memory_mb = -5",
    "nonsense",
    "other_key = 4",
  ]) {
    expect(parseSettings(junk), junk).toEqual(defaultSettings);
  }
});

// The template is the documentation for the file, so it has to read back as what it says.
test("the written file reads back as the settings it came from", () => {
  expect(parseSettings(settingsText(defaultSettings))).toEqual(defaultSettings);
  const custom = {
    maxImportBytes: MIB,
    maxOperBytes: 2 * MIB,
    maxTotalBytes: 3 * MIB,
    language: "ja",
  };
  expect(parseSettings(settingsText(custom))).toEqual(custom);
});
