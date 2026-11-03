// ☰ ▸ About (desktop `dialogs.rs` `about_dialog`): Details, Feedback,
// Licenses. The licences name what the page carries besides Gifkino's own
// code: ffmpeg.wasm (downloaded on the first video import) and Gifsicle.

import cargoToml from "../../../../Cargo.toml?raw";
import iconUrl from "../../../public/images/icon.svg";
import type { App } from "../app";
import { t } from "../i18n";
import { alertDialog } from "../ui/dialog";
import { h } from "../ui/dom";
import { actionRow, group } from "../ui/prefs";

const REPOSITORY_URL = "https://github.com/zbcoding/gifkino";
const ISSUES_URL = "https://github.com/zbcoding/gifkino/issues";
const FEEDBACK_FORM_URL = "https://tally.so/r/A7kKVz";
/** The desktop app's version, so a release bumps `Cargo.toml` and nothing here. */
const VERSION = /^version = "([^"]+)"/m.exec(cargoToml)?.[1] ?? "";

export function install(app: App): void {
  app.registerCommand({ id: "about", label: "About", menu: "app", run: showAbout });
}

/** Desktop `fact_row`: a fact as the row's prominent line under a dim title. */
function factRow(title: string, value: string): HTMLElement {
  const row = actionRow(title, null, value);
  row.classList.add("property");
  return row;
}

/** Desktop `link_row`: the whole row opens `url`. */
function linkRow(title: string, subtitle: string, url: string): HTMLElement {
  const row = actionRow(title, h("span", { class: "dim", "aria-hidden": "true" }, "↗"), subtitle);
  return h(
    "a",
    { class: "pref-row link-row", href: url, target: "_blank", rel: "noopener noreferrer" },
    ...row.childNodes,
  );
}

function showAbout(): void {
  const licenses = group(
    t("Licenses"),
    factRow("Gifkino", "MIT"),
    factRow("FFmpeg (ffmpeg.wasm core)", "GPL-2.0-or-later"),
    factRow("ffmpeg.wasm", "MIT"),
    factRow("Gifsicle", "GPL-2.0"),
  );
  licenses.append(h("p", { class: "pref-description" }, "© 2026 zbcoding"));
  const content = h(
    "div",
    { class: "about" },
    h(
      "div",
      { class: "about-banner" },
      h("img", { src: iconUrl, alt: "", width: "96", height: "96" }),
      h("h1", {}, "Gifkino"),
      h("span", { class: "dim" }, "zbcoding"),
    ),
    group(
      t("Details"),
      factRow(t("Version"), VERSION),
      linkRow(t("Repository"), "github.com/zbcoding/gifkino", REPOSITORY_URL),
    ),
    group(
      t("Feedback"),
      linkRow(
        t("Report an issue on GitHub"),
        // Translators: Why this is the first of the two feedback rows.
        t("Tracked and answered. Needs a GitHub account."),
        ISSUES_URL,
      ),
      linkRow(
        t("Quick feedback form"),
        // Translators: The second feedback row, for a user with no account.
        t("About a minute, no account needed."),
        FEEDBACK_FORM_URL,
      ),
    ),
    licenses,
  );
  void alertDialog({
    title: t("About"),
    content,
    // Translators: Dismisses a dialog that only explains something.
    responses: [{ id: "close", label: t("Close") }],
    close: "close",
  }).answer;
}
