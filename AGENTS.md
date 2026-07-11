# Working on Roundhouse (agents & contributors)

Roundhouse reads Rails source and emits standalone projects in ~12 target
languages, plus an inference engine (LSP/MCP/in-browser IDE) that types Rails
without annotations. This file is the orientation an AI agent or new contributor
needs *before* touching the code: where to look, and the invariants not to break.

**Source of truth for current state is [`README.md`](README.md)** — which
targets are live, benchmark numbers, what works today. It is kept current; the
older docs below are accurate on *architecture* but may narrate migrations that
have since landed. **When a status claim anywhere disagrees with README or CI,
README and CI win.**

## Start here

| You want… | Read |
|---|---|
| What the project is / current state / the numbers | [`README.md`](README.md) — fresh, authoritative |
| The dev loop, `roundhouse-ast`, adding an IR variant | [`DEVELOPMENT.md`](DEVELOPMENT.md) |
| Pipeline internals (analyze / lower / emit / runtime / verification) | [`docs/pipeline/`](docs/pipeline/) — architecture, not status |
| Compiler inputs (Ruby+ERB, schema/routes/seeds, method catalog, DB adapter) | [`docs/data/`](docs/data/) |
| Why do this at all (the argument, option value) | [`WHY.md`](WHY.md) |

Pipeline shape: `Ruby AST → analyze (typed IR) → lower (target-neutral IR) →
emit (per-target project + runtime/<target>/ glue)`. Key files are mapped in
DEVELOPMENT.md § "Pipeline at a glance."

## Invariants — do not break these

These are the rules the codebase enforces or depends on. Violating one is a
defect even if the build is green.

1. **Zero *error* diagnostics is the contract.** The subset of Rails we
   transpile is *defined* as "produces no error diagnostics." Warnings are the
   modeling-debt ledger and are expected; **errors are the invariant.** Guarded
   by `tests/real_blog.rs` (`ingests_without_errors`, and the diagnostics
   assertions around it).

2. **Features land once, in a shared home — never duplicated per target.** New
   framework behavior belongs in `runtime/ruby/` (transpiled to every target) or
   in a `src/lower/` pass, not copied into N emitters. If you find yourself
   editing the same logic in two emitters, it belongs in the lowerer.

3. **`runtime/ruby/` method bodies must be fully typed and statically
   resolvable.** Enforced by
   `tests/runtime_src_integration.rs::every_runtime_method_body_is_fully_typed`.
   No `method_missing`, no subclassing built-ins, no type-erasing bags — the
   inference engine has to resolve every body. Non-void methods end in a read.
   Quick self-check: emit Rust and `cargo check`.

4. **Ruby emit is the round-trip forcing function.** `src/emit/ruby.rs` must be
   the exact inverse of `src/ingest.rs` (round-trip identity is tested on the
   fixtures). Other targets may approximate until a fixture sharpens them; Ruby
   may not.

5. **A new `runtime/ruby/<stem>.rb` must be registered in
   `src/project.rs::spinel_files`** or the Spinel target silently omits it.

6. **Spinel is part of this codebase.** It is Matz's Ruby-to-C compiler at
   `~/git/spinel`, co-developed. Defects → upstream issues/PRs with a minimal
   repro; genuine subset gaps → design around them *honestly* (record the gap,
   don't hide it with a workaround that pretends coverage exists).

## Workflow

- **Commit to `main`. No feature branches.** Stage only files you changed.
  End commit messages with the standard `Co-Authored-By` trailer.
- **Test cycle:** `cargo build --tests` + the targeted test for what you
  touched + a round-trip check (`roundhouse-ast --round-trip`). Use
  `cargo test --all-targets` at milestones. Real-toolchain tests are
  `#[ignore]`-gated (`cargo test --test <target>_toolchain -- --ignored`); CI
  runs each in its own job.
- **CI is deliberately not uniformly gating.** The core `cargo test` job gates.
  The ~5 `continue-on-error: true` jobs track upstream Spinel and other moving
  toolchains on purpose — **red there is a signal to read, not a regression to
  shim away.** Don't add workarounds just to make an advisory job green.

## The actual goal

The endpoint is not "does the fixture compile." It is a **per-target ledger of
how much of Rails transpiles** — the honest unsupported list, driven down over
time. Don't trade that real goal for a locally reachable one. The proving lanes
today are the real-blog fixture (every target, DOM-equivalent to Rails on every
push) and lobsters/Mastodon on the inference + Spinel path (see README).
