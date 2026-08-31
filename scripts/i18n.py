#!/usr/bin/env python3
"""Extract and merge gettext catalogs for gif-editor.

Impasto drives this with `make updatepotfiles` and `make updatepot`; there is no
Makefile here, so the same two jobs live in one script:

    scripts/i18n.py potfiles   # rescan src/ and rewrite po/POTFILES.in
    scripts/i18n.py pot        # rewrite po/messages.pot from the marked strings
    scripts/i18n.py merge      # fold the template into every po/LINGUAS locale
    scripts/i18n.py check      # report untranslated and stale entries
    scripts/i18n.py selftest   # check the line joining and the escaping

xgettext is not used: it has no Rust mode, and driving its C parser over Rust
mis-reads raw strings and lifetimes. The markers are `t("…")` and `n("…")`,
which is a regex away.
"""

import os
import re
import subprocess
import sys
from datetime import datetime, timezone

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PO = os.path.join(ROOT, "po")
POT = os.path.join(PO, "messages.pot")

# t("…") translates; n("…") only marks a literal for extraction, for strings
# defined far from where they are shown. Both take one plain string literal.
MARKER = re.compile(r'\b(?:i18n::)?[tn]\(\s*"((?:[^"\\]|\\.)*)"')
# tn("one", "many", n) — both halves are separate entries.
PLURAL = re.compile(r'\btn\(\s*"((?:[^"\\]|\\.)*)"\s*,\s*"((?:[^"\\]|\\.)*)"')
NOTE = re.compile(r'//\s*Translators:\s*(.*)')
# rustfmt puts a long literal on its own line under the call that marks it, so a
# line ending in a bare `t(` has to be joined with the next one or the msgid
# disappears from the template.
OPEN_MARKER = re.compile(r'\b(?:i18n::)?(?:tn|t|n)\(\s*$')


def logical_lines(text):
    """[(line_number, text)] with Rust's `\\`-newline continuations joined.

    A marked string may be wrapped across source lines; the msgid is what Rust
    sees, which is the joined form with the next line's indentation dropped.
    The reported line number stays the one the string starts on.
    """
    out = []
    pending, start = None, 0
    for i, line in enumerate(text.splitlines(), start=1):
        body = line.rstrip()
        joined = line.lstrip() if pending is not None else line
        current = (pending or "") + (joined[:-1] if body.endswith("\\") else joined)
        if pending is None:
            start = i
        if (body.endswith("\\") and '"' in line) or OPEN_MARKER.search(current):
            pending = current
        else:
            out.append((start, current))
            pending = None
    if pending is not None:
        out.append((start, pending))
    return out


def rust_sources():
    out = []
    for base, dirs, files in os.walk(os.path.join(ROOT, "src")):
        dirs[:] = [d for d in dirs if d not in {"target"}]
        for name in sorted(files):
            if name.endswith(".rs"):
                out.append(os.path.relpath(os.path.join(base, name), ROOT))
    return sorted(out)


def potfiles():
    """Files that contain at least one marked string."""
    keep = [
        p
        for p in rust_sources()
        if any(MARKER.search(line) for _, line in logical_lines(read(p)))
    ]
    body = (
        "# Files with translatable strings, one per line.\n"
        "# Regenerate with: scripts/i18n.py potfiles\n"
    ) + "".join(f"{p}\n" for p in keep)
    write(os.path.join(PO, "POTFILES.in"), body)
    return keep


def read(rel):
    with open(os.path.join(ROOT, rel), encoding="utf-8") as f:
        return f.read()


def write(path, text):
    with open(path, "w", encoding="utf-8") as f:
        f.write(text)


ESCAPES = {"n": "\n", "t": "\t", "r": "\r", '"': '"', "\\": "\\"}


def unescape(raw):
    """Undo the backslash escapes, one character at a time.

    Not `codecs.unicode_escape`: that decodes through latin-1, so every
    non-ASCII character in a msgid — · × ≈ … ▸ — comes back mojibaked.
    """
    if "\\" not in raw:
        return raw
    out, chars = [], iter(raw)
    for c in chars:
        if c != "\\":
            out.append(c)
            continue
        nxt = next(chars, "\\")
        out.append(ESCAPES.get(nxt, nxt))
    return "".join(out)


def quote(value):
    out = value.replace("\\", "\\\\").replace('"', '\\"').replace("\t", "\\t")
    # gettext convention: a string with newlines starts with an empty line and
    # breaks after each \n, so a diff of one line is one line.
    if "\n" in value:
        parts = out.split("\n")
        lines = [f'"{p}\\n"' for p in parts[:-1]]
        if parts[-1]:
            lines.append(f'"{parts[-1]}"')
        return '""\n' + "\n".join(lines)
    return f'"{out}"'


def extract():
    """{msgid: {"refs": [...], "notes": [...]}} in first-seen order."""
    found = {}
    for rel in potfiles():
        lines = logical_lines(read(rel))
        for index, (lineno, line) in enumerate(lines):
            ids = [unescape(m.group(1)) for m in MARKER.finditer(line)]
            for m in PLURAL.finditer(line):
                ids += [unescape(m.group(1)), unescape(m.group(2))]
            if not ids:
                continue
            # A `// Translators:` comment applies to the marked strings on the
            # next code line, which is where translators expect to find it.
            note = None
            for back in range(index - 1, max(index - 4, -1), -1):
                hit = NOTE.search(lines[back][1])
                if hit:
                    note = hit.group(1).strip()
                    break
                if lines[back][1].strip() and not lines[back][1].strip().startswith("//"):
                    break
            for msgid in dict.fromkeys(ids):
                entry = found.setdefault(msgid, {"refs": [], "notes": []})
                ref = f"{rel}:{lineno}"
                if ref not in entry["refs"]:
                    entry["refs"].append(ref)
                if note and note not in entry["notes"]:
                    entry["notes"].append(note)
    return found


HEADER = '''# Translation template for gif-editor.
# Copyright (C) {year}
# This file is distributed under the same license as the gif-editor package.
#
# PO translation metadata:
# Translations must include a descriptive translator note.
# AI-generated translations are tagged with a `#, fuzzy` flag so review tools
# treat them as needing editing (unfinished) until a human reviews them.
# Required entry order:
#   #. Translators: Describe what this string does for translators.
#   #. AI-generated translation; human review requested.
#   #, fuzzy
#   msgid "..."
#   msgstr "..."
#
#, fuzzy
msgid ""
msgstr ""
"Project-Id-Version: gif-editor\\n"
"Report-Msgid-Bugs-To: \\n"
"POT-Creation-Date: {stamp}\\n"
"PO-Revision-Date: YEAR-MO-DA HO:MI+ZONE\\n"
"Last-Translator: FULL NAME <EMAIL@ADDRESS>\\n"
"Language-Team: LANGUAGE <LL@li.org>\\n"
"Language: \\n"
"MIME-Version: 1.0\\n"
"Content-Type: text/plain; charset=UTF-8\\n"
"Content-Transfer-Encoding: 8bit\\n"
'''


def write_pot():
    entries = extract()
    stamp = datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M+0000")
    out = [HEADER.format(year=datetime.now().year, stamp=stamp)]
    for msgid, meta in entries.items():
        out.append("")
        for note in meta["notes"]:
            out.append(f"#. Translators: {note}")
        out.append("#: " + " ".join(meta["refs"]))
        out.append(f"msgid {quote(msgid)}")
        out.append('msgstr ""')
    write(POT, "\n".join(out) + "\n")
    return entries


def linguas():
    path = os.path.join(PO, "LINGUAS")
    if not os.path.exists(path):
        return []
    with open(path, encoding="utf-8") as f:
        return [
            line.strip()
            for line in f
            if line.strip() and not line.strip().startswith("#")
        ]


def parse_po(path):
    """{msgid: (msgstr, [comment lines])}, good enough to merge with."""
    entries, comments = {}, []
    msgid = msgstr = None
    target = None
    with open(path, encoding="utf-8") as f:
        for raw in f:
            line = raw.strip()
            if not line:
                if msgid is not None:
                    entries[msgid] = (msgstr or "", comments)
                msgid = msgstr = target = None
                comments = []
            elif line.startswith("#"):
                comments.append(line)
            elif line.startswith("msgid "):
                if msgid is not None:
                    entries[msgid] = (msgstr or "", comments)
                    comments = []
                msgid, msgstr, target = unquote_po(line[6:]), "", "id"
            elif line.startswith("msgstr "):
                msgstr, target = unquote_po(line[7:]), "str"
            elif line.startswith('"'):
                if target == "id":
                    msgid += unquote_po(line)
                elif target == "str":
                    msgstr += unquote_po(line)
    if msgid is not None:
        entries[msgid] = (msgstr or "", comments)
    entries.pop("", None)
    return entries


def unquote_po(raw):
    raw = raw.strip()
    if raw.startswith('"') and raw.endswith('"'):
        raw = raw[1:-1]
    return unescape(raw)


def merge():
    entries = write_pot()
    for lang in linguas():
        path = os.path.join(PO, f"{lang}.po")
        existing = parse_po(path) if os.path.exists(path) else {}
        head = po_header(path, lang)
        out = [head.rstrip("\n")]
        for msgid, meta in entries.items():
            translated, comments = existing.get(msgid, ("", []))
            out.append("")
            for note in meta["notes"]:
                out.append(f"#. Translators: {note}")
            # The AI marker and the fuzzy flag are the translator's; the notes
            # and the source references come from the code and are regenerated.
            for c in comments:
                if c.startswith("#. AI-generated"):
                    out.append(c)
            out.append("#: " + " ".join(meta["refs"]))
            for c in comments:
                if c.startswith("#,"):
                    out.append(c)
            out.append(f"msgid {quote(msgid)}")
            out.append(f"msgstr {quote(translated)}")
        write(path, "\n".join(out) + "\n")
        stale = [k for k in existing if k not in entries]
        if stale:
            print(f"{lang}: dropped {len(stale)} obsolete entries")
    return entries


def po_header(path, lang):
    if os.path.exists(path):
        with open(path, encoding="utf-8") as f:
            head = []
            for line in f:
                if line.strip() == "" and head:
                    break
                head.append(line)
            return "".join(head)
    stamp = datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M+0000")
    return (
        HEADER.format(year=datetime.now().year, stamp=stamp)
        .replace('"Language: \\n"', f'"Language: {lang}\\n"')
        .replace("Translation template for gif-editor.", f"{lang} translation for gif-editor.")
    )


def check():
    entries = extract()
    print(f"{len(entries)} translatable strings")
    problems = 0
    for lang in linguas():
        path = os.path.join(PO, f"{lang}.po")
        if not os.path.exists(path):
            print(f"{lang}: no catalog")
            problems += 1
            continue
        existing = parse_po(path)
        missing = [k for k in entries if not existing.get(k, ("", []))[0]]
        stale = [k for k in existing if k not in entries]
        fuzzy = sum(1 for _, (_, c) in existing.items() if any(x.startswith("#,") and "fuzzy" in x for x in c))
        print(
            f"{lang}: {len(entries) - len(missing)}/{len(entries)} translated, "
            f"{fuzzy} awaiting review, {len(stale)} obsolete"
        )
        if missing:
            problems += 1
            for m in missing[:5]:
                print(f"    missing: {m!r}")
    return 1 if problems else 0


def selftest():
    """The line joining and the escaping are the two things worth checking."""
    joined = logical_lines('let x = t("one \\\n   two");\nlet y = 1;\n')
    assert joined[0] == (1, 'let x = t("one two");'), joined
    assert joined[1] == (3, "let y = 1;"), joined

    # rustfmt moves a long literal under its marker; the msgid is still the
    # string the marker wraps.
    wrapped = logical_lines('    t(\n        "one \\\n         two",\n    ),\n')
    assert wrapped[0] == (1, '    t("one two",'), wrapped
    assert MARKER.search(wrapped[0][1]), wrapped

    # A call that merely ends in a word ending with t or n is not a marker.
    assert not OPEN_MARKER.search("    let img = RgbaImage::new("), "new("
    assert not OPEN_MARKER.search("    let s = format("), "format("

    # Round trip through the file, which is the contract that matters: a
    # msgid full of · × ≈ … has to come back as itself.
    samples = ["plain", 'with "quotes"', "two\nlines", "a\\backslash", "dot · x ≈ y ▸ z"]
    body = "".join(f"msgid {quote(v)}\nmsgstr {quote(v.upper())}\n\n" for v in samples)
    path = "/tmp/_i18n_selftest.po"
    open(path, "w", encoding="utf-8").write(body)
    parsed = parse_po(path)
    os.remove(path)
    for v in samples:
        assert v in parsed, (v, list(parsed))
        assert parsed[v][0] == v.upper(), (v, parsed[v])
    print("ok")


if __name__ == "__main__":
    command = sys.argv[1] if len(sys.argv) > 1 else "check"
    if command == "potfiles":
        print("\n".join(potfiles()))
    elif command == "pot":
        print(f"{len(write_pot())} strings -> po/messages.pot")
    elif command == "merge":
        print(f"{len(merge())} strings merged into {len(linguas()) or 0} catalogs")
    elif command == "check":
        sys.exit(check())
    elif command == "selftest":
        selftest()
    else:
        print(__doc__)
        sys.exit(2)
