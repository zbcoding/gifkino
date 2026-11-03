// Libadwaita's preference widgets in plain DOM, for the sidebar and dialogs:
// `PreferencesGroup`, `ActionRow`, `SpinButton`, radio rows.

import { h } from "./dom";

/** An `adw::PreferencesGroup`: an optional bold title over rows in one rounded card. */
export function group(title: string | null, ...rows: Node[]): HTMLElement {
  const heading = h("h3", { class: "pref-heading" }, title ?? "");
  heading.hidden = !title;
  return h(
    "section",
    { class: "pref-section" },
    heading,
    h("div", { class: "pref-group" }, ...rows),
  );
}

/** The title element of a `group`, for a heading that changes (`set_title`). */
export function groupTitle(section: HTMLElement): HTMLElement {
  return section.querySelector(".pref-heading")!;
}

/** An `adw::ActionRow`: title (and subtitle) left, `suffix` right. */
export function actionRow(title: string, suffix: Node | null, subtitle?: string): HTMLElement {
  const sub = h("span", { class: "pref-subtitle" }, subtitle ?? "");
  sub.hidden = !subtitle;
  return h(
    "label",
    { class: "pref-row" },
    h("span", { class: "pref-titles" }, h("span", { class: "pref-title" }, title), sub),
    suffix,
  );
}

/** A row's subtitle element, for one that changes (`set_subtitle`); empty hides it. */
export function setSubtitle(row: HTMLElement, text: string): void {
  const sub = row.querySelector<HTMLElement>(".pref-subtitle")!;
  sub.textContent = text;
  sub.hidden = !text;
}

/** Controls side by side in a row's suffix: a spin and its unit. */
export function suffix(...children: (Node | string)[]): HTMLElement {
  return h("span", { class: "pref-suffix" }, ...children);
}

export interface Spin {
  input: HTMLInputElement;
  /** The field as a whole number inside its range, as `gtk::SpinButton::value` reads. */
  value(): number;
  set(value: number): void;
}

/** A `gtk::SpinButton` over whole numbers `min..=max`; a typed value snaps into range on commit. */
export function spin(min: number, max: number, step: number, value: number): Spin {
  const input = h("input", {
    type: "number",
    class: "spin",
    min: String(min),
    max: String(max),
    step: String(step),
  });
  const clamp = (v: number) => Math.min(max, Math.max(min, Math.round(v)));
  const read = () => {
    const typed = Number(input.value);
    return input.value !== "" && Number.isFinite(typed) ? clamp(typed) : min;
  };
  input.addEventListener("change", () => (input.value = String(read())));
  const set = (v: number) => (input.value = String(clamp(v)));
  set(value);
  return { input, value: read, set };
}

/**
 * Radio rows in one group, the first picked: the desktop's `CheckButton`
 * group in `ActionRow` prefixes. `picked()` is the index of the active one.
 */
export function radioRows(options: { title: string; subtitle?: string }[]): {
  element: HTMLElement;
  picked(): number;
} {
  const name = `radio-${Math.random().toString(36).slice(2)}`;
  const radios = options.map((_, i) => h("input", { type: "radio", name, checked: i === 0 }));
  const rows = options.map((option, i) => {
    const row = actionRow(option.title, null, option.subtitle);
    row.prepend(radios[i]);
    return row;
  });
  return {
    element: group(null, ...rows),
    picked: () =>
      Math.max(
        0,
        radios.findIndex((radio) => radio.checked),
      ),
  };
}
