# AGENTS.md

This file provides guidance to agents like Claude Code (claude.ai/code) when working with code in this repository.

These rules apply to every task in this project unless explicitly overridden or the task is very simple.

Bias: caution over speed on non-trivial work. Use judgment on trivial tasks.
Bias: user testing the application - more code writing, adding regression tests to automate app testing, and checking commit history, less opening the app for screenshots

## Use Git for version management - use worktrees, because two or more agents can code at the same time
`main` is the shared trunk, kept checked out in the primary repo directory — the
checkout that git worktrees are added from, named for the project rather than for
a branch. Never commit directly onto `main`, never work from a detached HEAD, and
never do task edits in the primary checkout — it stays on `main` so
`cargo run` and `cargo test` there always exercise the landed tip.

**One worktree per agent per task.** Multiple agents can edit the same files for different
features at once, each isolated in its own checkout on its own branch:
- `git worktree add <path> -b <branch> main` before touching any file. Do all edits, builds,
  and commits inside that worktree.
- Commit only the files your task changed. Never sweep unrelated changes in — stage explicit
  paths (`git add <path>`), not `git add -A`.
- Every change must land on `main` before the task is complete. Prefer small, focused commits.

**Rebase onto the current `main` tip immediately before landing, not whenever you branched.**
This failure mode already bit us once: one fix was silently deleted because a second, stale
branch's diff clobbered it while landing its own unrelated change. To land your worktree's branch:
1. From your worktree: `git fetch` then `git rebase main` so your branch replays on the actual
   current tip. Two agents that edited the same file resolve their overlap here, as conflicts.
2. Skim `git diff main...HEAD` — if it touches a file another commit changed since you branched,
   read that file's current state; don't trust your branch's copy.
3. Build + run tests on the rebased branch.
4. Land from the primary checkout (it owns `main`, so a worktree can't `git switch main`):
   `git -C <primary> merge --ff-only <branch>`. The rebase in step 1 guarantees a fast-forward;
   if it isn't one, someone else landed first — re-run from step 1.
5. Clean up: `git worktree remove <path>` then `git branch -d <branch>`.

If two branches race to land, the second to rebase sees the collision in step 2 — resolve it
there, never `git push --force` over the other's work. If anything unexpected turns up in
`git status`/`git log`, investigate with read-only commands first; don't `git reset --hard`
or `git checkout --` over changes you didn't make.

**Landing goal.** The required end state for every code task:
- The commit is reachable from `main`, and the primary checkout on `main` builds and tests.
- The task's worktree is removed and its branch deleted.



# General

## Cyclomatic Complexity
### Prevent AI slop code. Ask: Would a human software developer want to read this?
Count the:
- branching paths
- loops
- catch blocks
- chained && || and other similar expressions

Ways to write and refactor:
Guard clauses. Invert conditions, return early, kill nesting.
Extract function. Each extracted piece gets a name that says what, not how. Names are documentation.
Lookup table / map instead of if-else or switch chains.
Named predicates. if(isEligibleForThis(Thing)) beats a 4-clause boolean.
Polymorphism / strategy for switch-on-type. Only when the switch appears in 2+ places.
Flatten loops reasonably. Extract loop body, use continue instead of nested if.

Don't optimize for this metric like it's the only score. A dense one-liner hiding 6 branches is worse than the honest if-chain it replaced.


## Variable Naming
Jane Street house style inspired by OCAML descriptive tranformation
### Avoid minting new names if you can
### Names should be informative about function, descriptive or mnemonic
### Less common systems should have more descriptive names
### Avoid churn
### Scope-Based Length: Names should be long and descriptive (e.g., credit_card_expiration) for variables referenced across multiple files or spanning entire modules, while shorter names are acceptable for local variables within small functions.
### Lexical Consistency: The firm advocates for using a single lexeme for similar operations, such as using create_hashtable and create_rbtree rather than mixing verbs like build or make. This allows programmers to guess function existence without documentation.
### Uniform Interfaces: In their Core library, types have dedicated modules (e.g., Int, Float) with standardized function names like to_string and of_string, and exception-throwing variants are consistently suffixed with _exn (e.g., Map.find_exn).
### Argument Order: Functions within a module typically place the primary type argument (t) first (e.g., Map.find) to facilitate partial application and maintain uniformity across data structures.


## Think Before Coding
State assumptions explicitly. If uncertain, ask rather than guess.
Present multiple interpretations when ambiguity exists.
Push back when a simpler approach exists.
Stop when confused. Name what's unclear.
Before adding code, read exports, immediate callers, shared utilities.
"Looks orthogonal" is dangerous. If unsure why code is structured a way, ask.

## Simplicity First
Minimum code that solves the problem. Nothing speculative.
No features beyond what was asked. No abstractions for single-use code.
Test: would a senior engineer say this is overcomplicated? If yes, simplify.

## Surgical Changes
Touch only what you must. Clean up only your own mess.
Don't "improve" adjacent code, comments, or formatting.
Don't refactor what isn't broken. Match existing style.

## Goal-Driven Execution
Define success criteria. Loop until verified.
Don't follow steps. Define success and iterate.
Strong success criteria let you loop independently.

## Token budgets are not advisory
Per-task: 4,000 tokens. Per-session: 30,000 tokens.
If approaching budget, summarize and start fresh.
Surface the breach. Do not silently overrun.

## Surface conflicts, don't average them
If two patterns contradict, pick one (more recent / more tested).
Explain why. Flag the other for cleanup.
Don't blend conflicting patterns.

## Tests verify intent, not just behavior
Tests must encode WHY behavior matters, not just WHAT it does.
A test that can't fail when business logic changes is wrong.

## Checkpoint after every significant step
Summarize what was done, what's verified, what's left.
Don't continue from a state you can't describe back.
If you lose track, stop and restate.

## Fail loud
"Completed" is wrong if anything was skipped silently.
"Tests pass" is wrong if any were skipped.
Default to surfacing uncertainty, not hiding it.

## Writing comments, documentation, runbooks or skills
### Specific details will rot
Bias towards instructions that are version-agnostic and outcome-based.

### Capable models do their best work when given tools and discretion
Over-specification degrades them.

### Assume the agent is already smart
Add only what it doesn't have. Do not recite CLAUDE.md or other common information.

### Model context is valuable. Be concise
No history, no stories, no "why we rejected the alternative."

### Omit, don't litigate
Say what a thing is; don't spend context negating what it isn't. Frame the general rule so edge cases fall out, and simply don't build what you don't want.

### Update documents, don't add to them when something changes
Growing line count should reflect a larger underlying system, not accumulated amendments.

### Favor clean domain separation
Duplicating the same information across many files or skills increases the surface area for rot and makes the knowledge base heavier for marginal gain. Refactor and reorganize.

### Legibility over edge-case cleverness
Tools are read and driven by frontier models — a tool that's intuitive to use and whose diagnostic you can trust beats one that silently handles a rare case but is hard to reason about.

# Code Search with Semble

Use `semble search` to find code by describing what it does or naming a symbol/identifier, if that would be faster than grep or ripgrep:

```bash
semble search "authentication flow" ./my-project
semble search "save_pretrained" ./my-project
semble search "save model to disk" ./my-project --top-k 10
```

Use `semble find-related` to discover code similar to a known location (pass `file_path` and `line` from a prior search result):

```bash
semble find-related src/auth.py 42 ./my-project
```

`path` defaults to the current directory when omitted; git URLs are accepted.

If `semble` is not on `$PATH`, use `uvx --from "semble[mcp]" semble` in its place.

### Workflow

1. Start with `semble search` to find relevant chunks.
2. Inspect full files only when the returned chunk is not enough context.
3. Optionally use `semble find-related` with a promising result's `file_path` and `line` to discover related implementations.
4. Use grep only when you need exhaustive literal matches or quick confirmation of an exact string.


## Before Submitting Code

1. **Run tests**
2. **Check formatting**
3. **Keep diffs rebase-friendly**: Small, focused changes; don't touch naming or file structures
4. **Avoid renaming** — If a refactor needs naming changes, coordinate with upstream first or defer it
7. **Keep commit messages generic** — Use "reference implementation" rather than trademarked names

## Translation & Localization

Standard gettext `.po` catalogs in `po/`, the same format Impasto uses. The
differences are that the msgids are Rust string literals rather than C# ones,
and that the app reads the `.po` directly — there is no `msgfmt` step and no
`.mo`. Compiling to `.mo` only ever bought lookup speed, which a few hundred
strings do not need, and it would cost a build step plus an install prefix this
app does not have yet.

- `t("Export GIF")` translates. The msgid is the US English string, so an
  untranslated build reads correctly instead of showing keys.
- `n("Frames deleted")` marks a literal without translating it there — gettext's
  `N_`. For strings defined far from where they are shown: the action labels in
  `keymap.rs`, the history labels stored in a `Change`, the capability blockers
  in `caps.rs`. Translate at the point of display with `t(…)` or `lookup(…)`.
- `tn(one, many, count)` picks between two msgids. It is not a Plural-Forms
  evaluator, so it is right for languages with one plural form and wrong for
  Slavic ones; the first such locale needs a real evaluator, not a new call
  site.
- `fill(t("Exported to {path} · {size} KB"), &[("path", …), ("size", …)])`
  interpolates. `format!` needs a literal and a translated string is never one.
  Placeholders are named so a translator can reorder the sentence; a test
  asserts every catalog keeps the placeholders its msgid has.
- Never build a string by concatenating translated fragments, and never
  lowercase one: German capitalizes its nouns.

Workflow — Impasto's `make updatepotfiles` and `make updatepot` are one script
here, since there is no Makefile:

```bash
scripts/i18n.py potfiles   # rescan src/ and rewrite po/POTFILES.in
scripts/i18n.py pot        # rewrite po/messages.pot from the marked strings
scripts/i18n.py merge      # fold the template into every po/LINGUAS locale
scripts/i18n.py check      # what is untranslated, unreviewed, or obsolete
scripts/i18n.py selftest   # the line joining and the escaping
```

`xgettext` is not used: it has no Rust mode, and its C parser mis-reads Rust
raw strings and lifetimes. The extractor understands `\`-continued literals, so
a long msgid may still wrap across source lines.

Locale comes from the `language` key in `settings.conf`, then `LANGUAGE`,
`LC_ALL`, `LC_MESSAGES`, `LANG`. `de_DE` falls back to `de.po`. Set
`GIFKINO_PO_DIR` to point at a catalog directory while testing; otherwise the
source tree's `po/` is found by walking up from the executable, so `cargo run`
picks up an edit with no install.

- AI-generated translations must be marked in each PO entry with `#. AI-generated translation; human review requested.`
- Add a descriptive `#. Translators: ...` note immediately above the AI-generated marker, so the entry reads `Translators` note, then AI-generated marker, then `msgid`/`msgstr`, and carry a `#, fuzzy` flag so review tools treat the entry as unfinished
- Unlike `msgfmt`, the runtime **uses** fuzzy entries rather than dropping them.
  Here fuzzy means "AI draft, not yet reviewed", and a draft nobody can see is a
  draft nobody will ever correct.

## Keybindings
User can set custom keybindings for everything. They're saved in a keybindings file.
When user updates keybindings, tooltips update too, so that the keybinding is shown in that tool tip.
If user adds keybinding to feature that has None default keybinding, the hint tooltip still updates and adds that new keybinding.
Keybinding menu usually shows user when duplicates have been set (red highlight as warning), but some buttons, like the tool menu buttons,
use the same key to cycle through the tool menu buttons.

How this is built here: `src/keymap.rs` owns `Action`, `Chord` and `Keymap`,
plus `Modal` and `Mods` for the modifiers held during a canvas drag (rotate,
keep aspect, resize from center) — a chord needs a key, and those are the
modifier alone. Every action lives in `keymap::ACTIONS`; adding one means an
entry there and an arm in `window::message_for`. Every canvas modifier lives in
`keymap::MODALS` and is read where the drag is handled. The file is `~/.config/gifkino/keybindings.conf`
— flat `action = Ctrl+Z` text rather than JSON, because the whole map is two
dozen readable lines and serde would be a dependency for what `split_once('=')`
already does. Import and export in the shortcuts window are a file copy.

Tooltips are built with `Keymap::tip("Undo", Action::Undo)`, never typed with
the accelerator in the string, which is what makes them follow a rebind.
`Keymap::conflicts` is what the editor paints red; it excludes tool-on-tool
clashes, which `install_shortcuts` turns into a cycle instead.

Menu items carry theirs as the GMenuItem "accel" attribute, built by
`window::frame_item` from `Chord::accel` — GTK looks a menu accelerator up in a
`GtkApplication` accel table or a shortcut controller in the item's own tree,
and ours are in neither, so it has to be handed over. `Chord::accel` speaks
`gtk_accelerator_parse` syntax (`<Control>z`), which needs GDK's own casing of
the key name where a chord stores it lowercased. `Msg::SetKeymap` drops the
cached frame list so `update_view` rebuilds the strip, and with it the popover
that carries the menu: that is what makes a rebind move the menu's hints.

Ctrl+? opens `window::shortcuts_dialog`, which is both the shortcuts window and
the keybinding editor: one list that shows what a key does and lets it be
changed, rather than two screens saying the same things.

## Tests
Writing regression tests is good.
Check for scripts to do tests in parallel like scripts/test-all-parallel.sh
Tests groups should be split into sections so running a group of relevant tests takes 1 minute max - no 10 minute long test runs for minor changes.


Unless noted, focus more on feature addition, less on time consuming verification and checking, because I'm checking the software as features are added

# Project Overview

this gif editor should import video to gif, gif editing, quick gif image frame edits like Adding text, adding overlay images, and streamlined gif operations like crop, resize, optimize, drop every 1 in N frames. Screen recording is out of scope: other software records mp4, and this app imports it.

## Quick Build & Run

Rust + GTK4 + libadwaita. One crate, three modules: `core/` (document model,
history, compositing), `pipeline/` (GIF and video I/O), `ui/` (window, Pango
text). `cargo` resolves the Rust side; the GTK stack comes from the system or
the flatpak runtime.

```bash
cargo run                  # welcome state
cargo run -- path/to.gif   # open a GIF or video directly
cargo test                 # whole suite, well under a minute
cargo test core::          # model, history, frame-list math, compositing
cargo test pipeline::      # GIF round-trip, ffmpeg probe, export settings
cargo fmt && cargo clippy
```

Everything except `ui::text` runs headless, so the model and pipeline are
testable without a display. `ui::text` needs a font stack, not a display.

External binaries are looked up at startup by `pipeline::caps`, never at the
moment of use:

- **ffmpeg / ffprobe** — required to import anything that is not a GIF.
  Without them the Open action is disabled with the reason attached.
- **gifsicle** — optional. Its absence skips the `-O3` pass; the export is a
  larger but valid GIF.

A missing capability disables its action rather than failing mid-task, so a
build with no ffmpeg still opens GIFs and exports them.

## Rebase Strategy & Constraints

Weigh a change against the conflict it will cause; do not treat a one-line edit there as free.

**Mark simplifications with `ponytail:` comments** that name the cost ceiling and upgrade path. This signals intentional shortcuts, not ignorance.

**Translations move in lockstep with their msgids.** Rebranding a user-facing string means editing the `msgid` and every `msgstr` that renders the old name across all of `po/*.po`. A changed `msgid` with an untouched `msgstr` silently orphans that translation and the string falls back to English. Watch for collisions with an existing entry (duplicates make `msgfmt` reject the file — merge rather than rename into one) and for inflected or transliterated forms (`Pinty`, `Pinto`, `Пинта`, `பிண்டா`) that a literal search misses. Verify with `msgfmt -c` over every catalogue.


# Changes checklist
Update changelog.md
Add translations
Check keybindings and key shortcuts