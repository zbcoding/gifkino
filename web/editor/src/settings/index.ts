// The settings dialog over desktop settings.conf: interface language and the
// three memory budgets. The desktop edits the file by hand; a browser has no
// file to edit, so the same keys get a dialog, saved as they change.

import type { App } from "../app";
import { budgets } from "../frameops/budget";
import { LANGUAGES, n, setLanguage, t } from "../i18n";
import { alertDialog } from "../ui/dialog";
import { h } from "../ui/dom";
import { actionRow, group, spin } from "../ui/prefs";
import { loadSettings, saveSettings } from "./conf";
import type { Settings } from "./conf";

const MIB = 1024 * 1024;

/** Feed the budgets to the memory guard. */
function applyBudgets(settings: Settings): void {
  budgets.import = settings.maxImportBytes;
  budgets.oper = settings.maxOperBytes;
  budgets.total = settings.maxTotalBytes;
}

/** A catalog's name in its own language: `de` → "Deutsch". */
function autonym(language: string): string {
  const tag = language.replace("_", "-");
  return new Intl.DisplayNames([tag], { type: "language" }).of(tag) ?? language;
}

export function install(app: App): void {
  let settings = loadSettings();
  applyBudgets(settings);
  const change = (edit: Partial<Settings>) => {
    settings = { ...settings, ...edit };
    saveSettings(settings);
    applyBudgets(settings);
  };

  app.registerCommand({
    id: "settings",
    // Translators: Main menu item opening the language and memory settings.
    label: n("Settings…"),
    menu: "app",
    run: () => openSettingsDialog(app, settings, change),
  });
}

function openSettingsDialog(
  app: App,
  settings: Settings,
  change: (edit: Partial<Settings>) => void,
): void {
  const language = h(
    "select",
    {},
    // Translators: Language choice that follows the languages set in the browser.
    h("option", { value: "" }, t("Browser default")),
    ...LANGUAGES.map((name) => h("option", { value: name }, autonym(name))),
  );
  language.value = settings.language ?? "";
  language.addEventListener("change", async () => {
    change({ language: language.value || null });
    await setLanguage(language.value || null);
    // Strings filled with values are rebuilt by store subscribers.
    app.update({});
  });

  const budgetRow = (title: string, subtitle: string, field: keyof Omit<Settings, "language">) => {
    const budget = spin(1, 1024 * 1024, 256, settings[field] / MIB);
    budget.input.addEventListener("change", () => change({ [field]: budget.value() * MIB }));
    return actionRow(title, budget.input, subtitle);
  };

  void alertDialog({
    title: t("Settings"),
    content: h(
      "div",
      { class: "settings-editor" },
      // Translators: Settings row choosing the interface language.
      group(null, actionRow(t("Language"), language)),
      group(
        // Translators: Settings group of the memory limits, in megabytes.
        t("Memory limits"),
        budgetRow(
          // Translators: Memory limit row: one import.
          t("Per import"),
          // Translators: Explains the per-import memory limit.
          t("Most memory an import may use for decoded frames, in MB."),
          "maxImportBytes",
        ),
        budgetRow(
          // Translators: Memory limit row: one frame operation such as a resize.
          t("Per operation"),
          // Translators: Explains the per-operation memory limit.
          t("Most memory one frame operation (a resize, …) may produce, in MB."),
          "maxOperBytes",
        ),
        budgetRow(
          // Translators: Memory limit row: everything held at once.
          t("In total"),
          // Translators: Explains the total memory limit.
          t(
            "Most memory frames may hold in total, in MB: the document plus what the operation produces, since undo keeps the old frames.",
          ),
          "maxTotalBytes",
        ),
      ),
    ),
    // Translators: Dismisses a dialog that only explains something.
    responses: [{ id: "close", label: t("Close") }],
    close: "close",
  }).answer;
}
