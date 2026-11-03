// Translations: desktop `i18n.rs`. Msgids are the desktop's US English strings
// and the catalogs are the repository's po/*.po, copied into the build as
// assets and parsed as they are, fuzzy entries included.

/** Every po/ catalog, by file name, as a build asset URL. */
const CATALOG_URLS: Record<string, string> = Object.fromEntries(
  Object.entries(
    import.meta.glob<string>("../../../po/*.po", { query: "?url", import: "default", eager: true }),
  ).map(([path, url]) => [path.replace(/^.*\/|\.po$/g, ""), url]),
);

/** The catalog names a language setting may hold: `de`, `en_US`, `ja`, … */
export const LANGUAGES = Object.keys(CATALOG_URLS).sort();

let catalog = new Map<string, string>();

/** Translate. An untranslated msgid is itself, so a missing catalog still reads correctly. */
export function t(msgid: string): string {
  return catalog.get(msgid) ?? msgid;
}

/** Mark a literal for extraction without translating it here: gettext's `N_`, desktop `n`. */
export function n(msgid: string): string {
  return msgid;
}

/** Fill named `{placeholders}`; translators may reorder them. */
export function fill(template: string, values: Record<string, string | number>): string {
  return template.replace(/\{(\w+)\}/g, (whole, name: string) =>
    name in values ? String(values[name]) : whole,
  );
}

/**
 * Desktop `tn`: pick between two msgids by `count`. Right for languages with
 * one plural form; Slavic ones need a real Plural-Forms evaluator.
 */
export function tn(one: string, many: string, count: number): string {
  return t(count === 1 ? one : many);
}

/**
 * Desktop `parse_po`: comments, `msgid`/`msgstr` and their continuation
 * lines. Fuzzy entries are kept (here fuzzy means an AI draft awaiting
 * review); the header and untranslated entries are not entries; an entry with
 * `msgctxt` or `msgid_plural` is skipped rather than half-read.
 */
export function parsePo(text: string): Map<string, string> {
  const entries = new Map<string, string>();
  let id = "";
  let msg = "";
  let inMsgstr = false;
  let skip = false;
  const flush = () => {
    if (!skip && id && msg) entries.set(id, msg);
    id = msg = "";
    skip = false;
  };
  for (const raw of text.split("\n")) {
    const line = raw.trim();
    if (line === "") {
      flush();
      inMsgstr = false;
    } else if (line.startsWith("msgid ")) {
      // A msgctxt precedes its msgid, so its flag belongs to the entry starting here.
      const carried: boolean = skip;
      flush();
      skip = carried;
      inMsgstr = false;
      id = unquote(line.slice(6));
    } else if (line.startsWith("msgstr ")) {
      inMsgstr = true;
      msg = unquote(line.slice(7));
    } else if (line.startsWith("msgctxt ") || line.startsWith("msgid_plural ")) {
      skip = true;
    } else if (line.startsWith('"')) {
      if (inMsgstr) msg += unquote(line);
      else id += unquote(line);
    }
  }
  flush();
  return entries;
}

const ESCAPES: Record<string, string> = { n: "\n", t: "\t", r: "\r" };

/** Strip the quotes and undo gettext's escapes. */
function unquote(raw: string): string {
  const trimmed = raw.trim();
  const inner = /^".*"$/.test(trimmed) ? trimmed.slice(1, -1) : trimmed;
  return inner.replace(/\\(.?)/g, (_, c: string) => ESCAPES[c] ?? (c || "\\"));
}

/** Desktop `normalize_locale` + `candidates`: `de-DE.UTF-8` tries `de_DE`, then `de`. */
function candidates(locale: string): string[] {
  const name = locale.split(/[.@]/)[0].replace("-", "_");
  const base = name.split("_")[0];
  return base === name ? [name] : [name, base];
}

/**
 * The catalog to load: the setting's, else the first of the browser's
 * languages that has one. A set language with no catalog is English, as on
 * the desktop, rather than falling through to the browser's choice.
 */
export function resolveLocale(
  setting: string | null,
  preferred: readonly string[],
  available: readonly string[],
): string | null {
  const wanted = setting ? candidates(setting) : preferred.flatMap(candidates);
  return wanted.find((name) => available.includes(name)) ?? null;
}

/**
 * Load the catalog for `setting` (see `resolveLocale`) and retranslate the
 * page already built in the previous language.
 */
export async function setLanguage(setting: string | null): Promise<void> {
  const locale = resolveLocale(setting, navigator.languages, LANGUAGES);
  const next = locale ? parsePo(await (await fetch(CATALOG_URLS[locale])).text()) : new Map();
  const previous = catalog;
  catalog = next;
  document.documentElement.lang = (locale ?? "en").replace("_", "-");
  retranslate(document.body, previous, next);
}

const TRANSLATED_ATTRIBUTES = ["title", "placeholder", "aria-label"];

/**
 * Swap every text node and tooltip that is a translation in `from` for the
 * same msgid in `to`. ponytail: most of the page is built once, with `t()`
 * at build time, and this is what lets the language change without a reload.
 * Strings filled with values (`{count} frames`) are not msgids; they are
 * rebuilt by their store subscribers when the caller nudges the store.
 */
function retranslate(root: HTMLElement, from: Map<string, string>, to: Map<string, string>): void {
  const msgids = new Map([...from].map(([msgid, translated]) => [translated, msgid]));
  const swap = (text: string) => {
    const msgid = msgids.get(text) ?? text;
    return to.get(msgid) ?? msgid;
  };
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const swapped = swap(node.nodeValue ?? "");
    if (swapped !== node.nodeValue) node.nodeValue = swapped;
  }
  for (const element of root.querySelectorAll("[title], [placeholder], [aria-label]")) {
    for (const name of TRANSLATED_ATTRIBUTES) {
      const value = element.getAttribute(name);
      if (value) element.setAttribute(name, swap(value));
    }
  }
}
