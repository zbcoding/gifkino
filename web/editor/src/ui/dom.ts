// Plain-DOM helpers shared by the shell and the slices.

import type { App } from "../app";
import { t } from "../i18n";

type Props = Record<string, string | boolean | ((event: Event) => void)>;
type Child = Node | string | null | undefined | false;

/**
 * Create an element. Props starting with `on` attach listeners (`onclick`);
 * `true` sets an empty attribute, `false` omits it; anything else is an attribute.
 */
export function h<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  props: Props = {},
  ...children: Child[]
): HTMLElementTagNameMap[K] {
  const element = document.createElement(tag);
  for (const [name, value] of Object.entries(props)) {
    if (typeof value === "function") element.addEventListener(name.slice(2), value);
    else if (value !== false) element.setAttribute(name, value === true ? "" : value);
  }
  for (const child of children) if (child) element.append(child);
  return element;
}

/** An inline SVG icon from a 16×16 path, coloured by `currentColor`. */
export function icon(path: string): SVGSVGElement {
  const svgNs = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(svgNs, "svg");
  svg.setAttribute("viewBox", "0 0 16 16");
  svg.setAttribute("class", "icon");
  svg.setAttribute("aria-hidden", "true");
  const shape = document.createElementNS(svgNs, "path");
  shape.setAttribute("d", path);
  svg.append(shape);
  return svg;
}

/** Tooltip text with the command's chord, so a rebind moves the hint. */
export function tip(app: App, text: string, commandId: string): string {
  const chord = app.shortcutFor(commandId);
  return chord ? `${text} (${chord})` : text;
}

/**
 * A button that runs a registered command, greyed while the command is
 * disabled, with the command's tooltip.
 */
export function commandButton(
  app: App,
  commandId: string,
  content: Child,
  className = "",
): HTMLButtonElement {
  const command = app.commands.get(commandId);
  if (!command) throw new Error(`No command ${commandId}`);
  const button = h(
    "button",
    { class: className, onclick: () => void app.runCommand(commandId) },
    content,
  );
  const sync = () => {
    button.disabled = !app.isEnabled(command);
    button.title = tip(app, command.tooltip?.(app.state) ?? t(command.label), commandId);
  };
  sync();
  app.store.subscribe(sync);
  return button;
}
