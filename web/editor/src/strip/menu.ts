// A right-click popover over registered commands: the strip's frame menu and
// the bands' overlay menu. Styled like the header's menus.

import type { App } from "../app";
import { h } from "../ui/dom";

export interface MenuItem {
  /** Translated text. */
  label: string;
  command: string;
  /** Show the command's chord, as the desktop does for the clipboard and list items. */
  accel?: boolean;
}

export interface ContextMenu {
  element: HTMLElement;
  /** Open at the pointer with `sections` split by separators. */
  open(event: MouseEvent, sections: MenuItem[][]): void;
}

export function createContextMenu(app: App): ContextMenu {
  const element = h("div", { class: "menu-popover", popover: "auto" });

  const item = ({ label, command, accel }: MenuItem) => {
    const registered = app.commands.get(command);
    const chord = accel ? app.shortcutFor(command) : null;
    return h(
      "button",
      {
        class: "menu-item",
        disabled: !registered || !app.isEnabled(registered),
        onclick: () => {
          element.hidePopover();
          void app.runCommand(command);
        },
      },
      h("span", {}, label),
      chord && h("span", { class: "accel" }, chord),
    );
  };

  const open = (event: MouseEvent, sections: MenuItem[][]) => {
    element.replaceChildren(
      ...sections.map((section) => h("div", { class: "menu-section" }, ...section.map(item))),
    );
    element.showPopover();
    // Opens downward from the pointer unless that runs off the window, which
    // from a strip at the bottom it usually does.
    const { offsetWidth: width, offsetHeight: height } = element;
    const left = Math.max(4, Math.min(event.clientX, innerWidth - width - 4));
    const below = event.clientY + height <= innerHeight - 4;
    const top = Math.max(4, below ? event.clientY : event.clientY - height);
    Object.assign(element.style, { left: `${left}px`, top: `${top}px` });
  };

  return { element, open };
}
