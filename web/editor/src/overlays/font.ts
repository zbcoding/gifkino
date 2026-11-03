// The overlay's font field: desktop `gtk::FontDialogButton`. A button naming
// the Pango description opens a chooser — search, families, faces, preview —
// and Select hands back a description such as "Sans Bold". Weight and slant
// live in the description, so there are no separate bold/italic toggles.

import { t } from "../i18n";
import { h } from "../ui/dom";
import { cssFont, STYLE_WORDS } from "../worker/text";

/** Families every browser resolves, then common installed ones. */
const FAMILIES = [
  "Sans",
  "Serif",
  "Monospace",
  "Cantarell",
  "Inter",
  "DejaVu Sans",
  "Liberation Sans",
  "Noto Sans",
  "Arial",
  "Helvetica",
  "Verdana",
  "Georgia",
  "Times New Roman",
  "Courier New",
  "Impact",
  "Comic Sans MS",
];
const FACES = ["Regular", "Italic", "Bold", "Bold Italic", "Light", "Medium", "Semi-Bold", "Heavy"];
const SAMPLE = "The quick brown fox jumps over the lazy dog.";

interface LocalFont {
  family: string;
  style: string;
}
declare global {
  interface Window {
    queryLocalFonts?: () => Promise<LocalFont[]>;
  }
}

/** "DejaVu Sans Bold Italic" → ["DejaVu Sans", "Bold Italic"]: trailing Pango style words are the face. */
export function splitDescription(description: string): [string, string] {
  const words = description.trim().split(/\s+/).filter(Boolean);
  if (words.length > 1 && /^\d+(\.\d+)?(px)?$/i.test(words[words.length - 1])) words.pop();
  let at = words.length;
  while (at > 1 && words[at - 1].toLowerCase() in STYLE_WORDS) at--;
  return [words.slice(0, at).join(" ") || "Sans", words.slice(at).join(" ")];
}

export interface FontField {
  element: HTMLElement;
  /** Shows the document's font; never reports back. */
  set(description: string): void;
}

export function fontField(onChange: (description: string) => void): FontField {
  let current = "Sans";
  let family = "Sans";
  let face = "";
  const faces = new Map<string, string[]>();
  let localLoaded = false;

  const button = h("button", { type: "button", class: "font-button" });
  const search = h("input", {
    type: "search",
    class: "font-search",
    placeholder: t("Search font name"),
  });
  const list = h("div", { class: "font-list", role: "listbox" });
  const faceSelect = h("select", { class: "font-face" });
  const preview = h("div", { class: "font-preview" }, SAMPLE);
  const select = h("button", { type: "button", class: "suggested" }, t("Select"));
  const cancel = h("button", { type: "button" }, t("Cancel"));
  const popover = h(
    "div",
    { class: "font-popover", popover: "auto" },
    search,
    list,
    faceSelect,
    preview,
    h("div", { class: "font-actions" }, cancel, select),
  );

  const description = () => (face && face !== "Regular" ? `${family} ${face}` : family);
  const showPreview = () => (preview.style.font = cssFont(description(), 20));
  const fillFaces = () => {
    const options = faces.get(family) ?? FACES;
    faceSelect.replaceChildren(...options.map((name) => h("option", { value: name }, name)));
    faceSelect.value = options.includes(face) ? face : options[0];
    face = faceSelect.value;
    showPreview();
  };
  const fillList = () => {
    const query = search.value.trim().toLowerCase();
    const names = [...new Set([...FAMILIES, ...faces.keys()])].filter((name) =>
      name.toLowerCase().includes(query),
    );
    list.replaceChildren(
      ...names.map((name) => {
        const row = h(
          "button",
          {
            type: "button",
            class: name === family ? "font-row selected" : "font-row",
            onclick: () => {
              family = name;
              fillList();
              fillFaces();
            },
          },
          name,
        );
        row.style.fontFamily = cssFont(name, 15).replace(/^15px /, "");
        return row;
      }),
    );
  };
  /** The installed fonts, where the browser will list them (it asks permission once). */
  const loadLocalFonts = async () => {
    if (localLoaded || !window.queryLocalFonts) return;
    localLoaded = true;
    const fonts = await window.queryLocalFonts().catch(() => []);
    for (const font of fonts)
      faces.set(font.family, [...(faces.get(font.family) ?? []), font.style]);
    fillList();
  };

  search.addEventListener("input", fillList);
  faceSelect.addEventListener("change", () => {
    face = faceSelect.value;
    showPreview();
  });
  cancel.addEventListener("click", () => popover.hidePopover());
  select.addEventListener("click", () => {
    popover.hidePopover();
    if (description() !== current) onChange(description());
  });
  button.addEventListener("click", () => {
    [family, face] = splitDescription(current);
    search.value = "";
    fillList();
    fillFaces();
    popover.showPopover();
    const at = button.getBoundingClientRect();
    popover.style.left = `${Math.max(4, at.right - popover.offsetWidth)}px`;
    popover.style.top = `${Math.min(at.bottom + 4, innerHeight - popover.offsetHeight - 4)}px`;
    search.focus();
    void loadLocalFonts();
  });

  const set = (value: string) => {
    current = value;
    button.textContent = value;
    button.style.font = cssFont(value, 15);
  };
  set(current);
  return { element: h("span", { class: "font-field" }, button, popover), set };
}
