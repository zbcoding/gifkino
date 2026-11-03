// The desktop's `adw::AlertDialog` as a modal <dialog>: a heading, a body,
// optional form rows, and a row of responses. Escape answers `close`; Enter in
// a field answers `default` when that response is enabled.

import { h } from "./dom";

export interface DialogResponse {
  id: string;
  label: string;
  /** `adw::ResponseAppearance::Suggested`: the blue button. */
  suggested?: boolean;
}

export interface DialogOptions {
  title: string;
  body?: string;
  /** `set_extra_child`: rows under the body. */
  content?: Node;
  responses: DialogResponse[];
  /** The answer when the dialog is dismissed (Escape). */
  close: string;
  /** The answer Enter gives. */
  default?: string;
}

export interface Dialog {
  /** Resolves with the picked response id once the dialog closes. */
  answer: Promise<string>;
  /** `set_response_enabled`. */
  setEnabled(id: string, enabled: boolean): void;
}

export function alertDialog(options: DialogOptions): Dialog {
  const buttons = new Map<string, HTMLButtonElement>();
  const dialog = h(
    "dialog",
    { class: "alert-dialog" },
    h("h2", {}, options.title),
    options.body ? h("p", { class: "alert-body" }, options.body) : null,
    options.content,
  );
  const row = h("div", { class: "alert-responses" });
  const { promise: answer, resolve } = Promise.withResolvers<string>();
  const finish = (id: string) => {
    dialog.close();
    dialog.remove();
    resolve(id);
  };
  for (const response of options.responses) {
    const button = h(
      "button",
      { class: response.suggested ? "suggested" : "", onclick: () => finish(response.id) },
      response.label,
    );
    buttons.set(response.id, button);
    row.append(button);
  }
  dialog.append(row);
  dialog.addEventListener("cancel", (event) => {
    event.preventDefault();
    finish(options.close);
  });
  dialog.addEventListener("keydown", (event) => {
    if (event.key !== "Enter" || !(event.target instanceof HTMLInputElement)) return;
    const button = options.default ? buttons.get(options.default) : undefined;
    if (!button || button.disabled) return;
    event.preventDefault();
    // Commit the typed value first, the way a spin button commits on Enter.
    event.target.dispatchEvent(new Event("change"));
    button.click();
  });
  document.body.append(dialog);
  dialog.showModal();
  dialog.querySelector<HTMLInputElement>("input")?.select();
  return {
    answer,
    setEnabled: (id, enabled) => {
      const button = buttons.get(id);
      if (button) button.disabled = !enabled;
    },
  };
}
