# The browser IDE

[rubys.github.io/roundhouse/ide/](https://rubys.github.io/roundhouse/ide/)
is the analyzer compiled to WebAssembly, running in a Web Worker
behind a Monaco editor. It needs nothing installed: open the page,
pick an app, hover. It is the fastest way to see what roundhouse
knows about a Rails codebase — and, through *open folder…*, to see
what it knows about yours, without the code leaving your machine.

## What it does

The page comes preloaded with real apps — the Rails Guides store, the
blog fixture, Lobsters, Campfire and Mastodon — selectable from the
**app** menu. Switching re-analyzes in place; Mastodon's 1,100-odd
files take about two and a half seconds. Then, in any file:

- **hover** — the inferred type at the cursor, inside ERB and HAML
  templates as well as Ruby.
- **completion** — typed members, scopes, column keyword arguments and
  instance variables from the last completed analysis.
- **markers** — the diagnostics, with the same three-way reading as
  [`check.md`](check.md): errors are findings, warnings are the
  coverage ledger, `info` notes mean "roundhouse can't see this yet",
  not "your code is wrong".
- **F12 / ⇧F12** (or ⌘-click) — go to definition and find references.
  An instance variable read in a view resolves through the
  controller→view channel: F12 lands on the write in the feeding
  action, ⇧F12 lists every feeder's writes and the reads in the
  sibling templates.
- **⌘P** — fuzzy file and class picker. **⌘⇧R** — related files, from
  the inferred render graph and include edges rather than filename
  conventions. **⌘⇧T** — the request trace for the action under the
  cursor, the same chain the editor's CodeLens and the MCP's
  `traceroute` produce.
- **coverage** — the ingest-gap punch list for the current app.
- **copy summary** — one pasteable line: app · files · errors ·
  warnings · coverage notes · ingest gaps · gem census ·
  `roundhouse <version>@<commit>`.

Edits re-analyze in the worker, debounced; queries answer from the
previous snapshot while the next one runs.

## Your own app: open folder…

The last entry in the app menu is **open folder…**. Point it at a Rails
checkout on your disk and the page analyzes that instead.

Nothing is uploaded. The files are read through the browser's File
API in the tab and handed to the analyzer worker, which is also in the
tab; the status line says *read from your disk, nothing uploaded*
while a local app is open, and there is no server on the other end
that could receive anything. The page is static.

In Chromium-based browsers (Chrome, Edge, Arc, Brave) the page gets a
directory picker and keeps the handle, so **↻** re-reads the tree after
you edit in your real editor. Other browsers get a folder-upload
dialog, which enumerates the whole tree before the page filters it —
an app with a large `node_modules/` beside `app/` opens noticeably
faster through the picker, which walks only the directories the
analyzer reads (the same inclusion rules the shipped bundles were
built with).

**copy summary** is the number to post. It names the analyzer build
that produced it, so a line pasted into an issue or a chat can be
compared against a later run of the same app on a newer snapshot, and
the counts are the same ones `roundhouse check` prints — an IDE run
and a command-line run on the same checkout agree.

## Limits

The IDE is the analyzer, not the compiler: there is no transpile or
compile from the browser. It is also a snapshot of the analyzer at the
site's last deploy, which tracks `main` rather than a dated release;
the summary line's commit says which. For an editor that follows your
files as you work, [`editor.md`](editor.md); for an agent,
[`mcp.md`](mcp.md).
