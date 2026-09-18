# Roundhouse — user guide

Roundhouse reads a Rails application — unmodified source, no
annotations, no `bundle install`, no boot, no database — and does one
of three things with what it learns. Pick the door you came for.

## Analyze

Whole-program type and effect inference over your app, surfaced as an
editor server, an agent tool, and a command-line checker. *What's the
type here? Can it be nil? Which filters run before this action, and
which query will be N+1?*

- [`check.md`](check.md) — `roundhouse check`: the diagnostics and how
  to read them.
- [`editor.md`](editor.md) — the LSP in VS Code (or any LSP client).
- [`mcp.md`](mcp.md) — the MCP server for Claude Code, Cursor, and
  other agents.
- [`ide.md`](ide.md) — the in-browser IDE, which analyzes a folder on
  your disk without uploading it.

## Transpile

The same analysis, fed to one of a dozen emitters: a standalone project
in Rust, Go, TypeScript, Crystal, Elixir, Kotlin, Swift, Python, C#,
or Ruby, with its own tests and no Rails at runtime.

- [`transpile.md`](transpile.md) — `roundhouse --target`: producing a
  project and running it.
- [`targets.md`](targets.md) — every target, its toolchain, and how
  far it is tested.
- [`rails-coverage.md`](rails-coverage.md) — which parts of Rails come
  through, and where each target stops.
- [`verifying.md`](verifying.md) — the DOM-equivalence oracle: proving
  the emitted app renders what Rails renders.

## Compile

The Ruby-shape emit compiled ahead of time to a native binary by
[Spinel](https://github.com/matz/spinel). One executable, no
interpreter, a SQLite file beside it by default.

- [`spinel.md`](spinel.md) — building, running, tuning and deploying
  the binary.

## Before any of them

- [`install.md`](install.md) — the snapshot binaries, and building from
  source when they don't fit.
- [`../../RELEASES.md`](../../RELEASES.md) — what each dated snapshot
  contains.

## Elsewhere

This guide is for using roundhouse. The architecture — how the
pipeline is built, what each pass does, why the runtime is split the
way it is — lives one directory up in [`docs/`](../README.md), and
contributor setup in [`DEVELOPMENT.md`](../../DEVELOPMENT.md). Status
claims in this guide describe the snapshot they ship with; where the
guide and CI disagree, CI wins.
