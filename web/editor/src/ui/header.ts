// The header bar: history, the editing menus, the title, ☰ and Export.

import { MENUS } from "../app";
import type { App, MenuName } from "../app";
import { fill, t } from "../i18n";
import { commandButton, h, icon } from "./dom";
import { MENU, REDO, UNDO } from "./icons";

const MENU_TITLES: Record<MenuName, string> = {
  frames: "Frames",
  insert: "Insert",
  // Translators: Header menu with mirror, fade and loop cross-fade.
  effects: "Effects",
  image: "Image",
  optimize: "Optimize",
  file: "",
  app: "",
};

/** The menus the bar shows; `file` and `app` are the ☰ menu's sections. */
const BAR_MENUS = MENUS.filter((menu) => menu !== "file" && menu !== "app");

export function createHeader(app: App): HTMLElement {
  const title = h("div", { class: "title-main" });
  const subtitle = h("div", { class: "subtitle" });
  const popover = h("div", { class: "menu-popover", popover: "auto" });

  const openMenu = (anchor: HTMLElement, menus: readonly MenuName[]) => {
    popover.replaceChildren(...menus.flatMap((menu) => menuSection(app, menu, popover) ?? []));
    const rect = anchor.getBoundingClientRect();
    popover.showPopover();
    const left = Math.min(rect.left, innerWidth - popover.offsetWidth - 4);
    Object.assign(popover.style, { left: `${Math.max(4, left)}px`, top: `${rect.bottom + 4}px` });
  };

  const menuBar = h("nav", { class: "menubar" });
  for (const menu of BAR_MENUS) {
    if (![...app.commands.values()].some((command) => command.menu === menu)) continue;
    const button: HTMLButtonElement = h(
      "button",
      { class: "flat", onclick: () => openMenu(button, [menu]) },
      t(MENU_TITLES[menu]),
    );
    menuBar.append(button);
  }
  const hamburger: HTMLButtonElement = h(
    "button",
    {
      class: "flat icon-button",
      title: t("Main menu"),
      onclick: () => openMenu(hamburger, ["file", "app"]),
    },
    icon(MENU),
  );

  const sync = () => {
    const { doc, fileName } = app.state;
    // Translators: Window title when no file has been opened.
    title.textContent = fileName ?? t("Untitled");
    subtitle.textContent = doc
      ? fill(t("{count} frames · {seconds} s"), {
          count: doc.frameCount,
          seconds: (doc.durationCs / 100).toFixed(1),
        })
      : "";
    document.title = fileName ? `${fileName} — Gifkino` : "Gifkino";
  };
  sync();
  app.store.subscribe(sync);

  return h(
    "header",
    { class: "headerbar" },
    h(
      "div",
      { class: "header-start" },
      h(
        "div",
        { class: "linked" },
        commandButton(app, "undo", icon(UNDO), "icon-button"),
        commandButton(app, "redo", icon(REDO), "icon-button"),
      ),
      menuBar,
    ),
    h("div", { class: "header-title" }, title, subtitle),
    h(
      "div",
      { class: "header-end" },
      hamburger,
      commandButton(app, "export", t("Export"), "suggested"),
    ),
    popover,
  );
}

/** One menu's commands as a section of the popover, or `null` when it has none. */
function menuSection(app: App, menu: MenuName, popover: HTMLElement): HTMLElement | null {
  const commands = [...app.commands.values()].filter((command) => command.menu === menu);
  if (commands.length === 0) return null;
  const items = commands.flatMap((command, i) => {
    const chord = app.shortcutFor(command.id);
    const separator = i > 0 && command.section !== commands[i - 1].section ? [h("hr")] : [];
    return [
      ...separator,
      h(
        "button",
        {
          class: "menu-item",
          disabled: !app.isEnabled(command),
          onclick: () => {
            popover.hidePopover();
            void app.runCommand(command.id);
          },
        },
        h("span", {}, t(command.label)),
        chord && h("span", { class: "accel" }, chord),
      ),
    ];
  });
  return h("div", { class: "menu-section" }, ...items);
}
