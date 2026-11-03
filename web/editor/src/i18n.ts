// Translation seam. Msgids are the desktop's US English strings (po/), so an
// untranslated build reads correctly. `t` is the identity until a catalog loads.

export function t(msgid: string): string {
  return msgid;
}

/** Fill named `{placeholders}`; translators may reorder them. */
export function fill(template: string, values: Record<string, string | number>): string {
  return template.replace(/\{(\w+)\}/g, (whole, name: string) =>
    name in values ? String(values[name]) : whole,
  );
}
