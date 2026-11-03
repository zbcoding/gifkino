// User settings: desktop `settings.rs`. The same flat `key = value` text as
// settings.conf, kept in localStorage under that name.

import { DEFAULT_BUDGETS } from "../frameops/budget";

const MIB = 1024 * 1024;
const STORAGE_KEY = "gifkino/settings.conf";

export interface Settings {
  maxImportBytes: number;
  maxOperBytes: number;
  maxTotalBytes: number;
  /** A po/ catalog name such as `de`; `null` follows the browser's languages. */
  language: string | null;
}

export const defaultSettings: Settings = {
  maxImportBytes: DEFAULT_BUDGETS.import,
  maxOperBytes: DEFAULT_BUDGETS.oper,
  maxTotalBytes: DEFAULT_BUDGETS.total,
  language: null,
};

/** Desktop `settings::entries`: `key = value` lines, both sides trimmed; blanks, `#` comments and junk skipped. */
export function entries(text: string): [string, string][] {
  return text
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line !== "" && !line.startsWith("#") && line.includes("="))
    .map((line) => {
      const at = line.indexOf("=");
      return [line.slice(0, at).trim(), line.slice(at + 1).trim()];
    });
}

/** Desktop `parse_mb`: a positive whole count of megabytes, as bytes. */
function parseMb(value: string): number | null {
  const mb = /^\d+$/.test(value) ? Number(value) : 0;
  return mb > 0 ? mb * MIB : null;
}

const BUDGET_KEYS = {
  max_import_memory_mb: "maxImportBytes",
  max_oper_memory_mb: "maxOperBytes",
  max_total_memory_mb: "maxTotalBytes",
} as const;

/** Desktop `Settings::apply`: anything missing or unparseable keeps its default. */
export function parseSettings(text: string): Settings {
  const settings = { ...defaultSettings };
  for (const [key, value] of entries(text)) {
    if (key === "language") settings.language = value || null;
    if (!(key in BUDGET_KEYS)) continue;
    const field = BUDGET_KEYS[key as keyof typeof BUDGET_KEYS];
    settings[field] = parseMb(value) ?? settings[field];
  }
  return settings;
}

/** The desktop's commented first-run template, carrying `settings`. */
export function settingsText(settings: Settings): string {
  return `# gifkino settings

# Most memory an import may use for decoded frames, in MB.
# Videos that need more are refused until you choose a smaller
# size or a lower frame rate in the import dialog.
max_import_memory_mb = ${settings.maxImportBytes / MIB}

# Most memory one frame operation (a resize, …) may produce, in MB.
max_oper_memory_mb = ${settings.maxOperBytes / MIB}

# Most memory frames may hold in total, in MB: the document plus
# what the operation produces, since undo keeps the old frames.
max_total_memory_mb = ${settings.maxTotalBytes / MIB}

# Interface language, as a po/ catalog name such as de or ja.
# Leave empty to follow the browser's languages.
language = ${settings.language ?? ""}
`;
}

export function loadSettings(): Settings {
  return parseSettings(localStorage.getItem(STORAGE_KEY) ?? "");
}

export function saveSettings(settings: Settings): void {
  localStorage.setItem(STORAGE_KEY, settingsText(settings));
}
