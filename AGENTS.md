# Working on Roundhouse (agents & contributors)

Roundhouse reads Rails source and emits standalone projects in ~12 target
languages, plus an inference engine (LSP/MCP/in-browser IDE) that types Rails
without annotations. This file is the orientation an AI agent or new contributor
needs *before* touching the code: where to look, and the invariants not to break.

**Source of truth for current state is [`RELEASES.md`](RELEASES.md) and CI** —
which targets are live, what each snapshot proves, the known gaps; the
[user guide](docs/guide/README.md) says what each door does today, and the
[bench page](https://rubys.github.io/roundhouse/bench/) carries the numbers.
[`README.md`](README.md) is the landing page. The older docs below are
accurate on *architecture* but may narrate migrations that have since
landed. **When a status claim anywhere disagrees with RELEASES.md or CI,
RELEASES.md and CI win.**

## Start here

| You want… | Read |
|---|---|
| What the project is | [`README.md`](README.md) — the landing page |
| Current state, per snapshot, and the known gaps | [`RELEASES.md`](RELEASES.md) — authoritative with CI |
| Using it (check / editor / MCP / transpile / Spinel) | [`docs/guide/`](docs/guide/README.md) |
| The dev loop, `roundhouse-ast`, adding an IR variant | [`DEVELOPMENT.md`](DEVELOPMENT.md) |
| Pipeline internals (analyze / lower / emit / runtime / verification) | [`docs/pipeline/`](docs/pipeline/) — architecture, not status |
| Compiler inputs (Ruby+ERB, schema/routes/seeds, method catalog, DB adapter) | [`docs/data/`](docs/data/) |
| Why do this at all (the argument, option value) | [`WHY.md`](WHY.md) |
| Why this attempt is different (lineage, the three bets, risks) | [`BETS.md`](BETS.md) |

Pipeline shape: `Ruby AST → analyze (typed IR) → lower (target-neutral IR) →
emit (per-target project + runtime/<target>/ glue)`. Key files are mapped in
DEVELOPMENT.md § "Pipeline at a glance."

## Invariants — do not break these

These are the rules the codebase enforces or depends on. Violating one is a
defect even if the build is green.

1. **Zero *error* diagnostics is the contract.** The subset of Rails we
   transpile is *defined* as "produces no error diagnostics." Warnings are the
   modeling-debt ledger and are expected; **errors are the invariant.** Guarded
   by `tests/real_blog.rs` (`ingests_without_errors` plus
   `type_analysis_coverage`, the zero-error + zero-unresolved-type gate).

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

4. **Ruby emit is lowered-IR-only; Spinel compile-equivalence is the forcing
   function.** Whole-app source-equivalence round-trip was retired (see the
   header of `src/emit/ruby.rs`); the emit-side gates are
   `tests/lowered_ruby_emit.rs` and `tests/spinel_toolchain.rs`.
   Expression-level IR round-trip (`roundhouse-ast --round-trip`, Ruby input
   only) still holds: ingest → emit-ruby → ingest must reach a fixed point.

5. **A new `runtime/ruby/<stem>.rb` must be registered in
   `src/project.rs::spinel_files`** or the Spinel target silently omits it.

6. **Spinel is part of this codebase.** It is Matz's Ruby-to-C compiler at
   `~/git/spinel`, co-developed. Defects → upstream issues/PRs with a minimal
   repro; genuine subset gaps → design around them *honestly* (record the gap,
   don't hide it with a workaround that pretends coverage exists).

## Workflow

- **Committers commit to `main`. No feature branches.** Stage only files
  you changed. End commit messages with the standard `Co-Authored-By`
  trailer.
- **Outside contributors: fork, and open a pull request against `main`.**
  CI runs the full matrix on a PR — every toolchain lane, the DOM compare
  against live Rails, the Spinel lanes — so you do not need every
  toolchain locally; CI is the oracle for the lanes you cannot run.
  Before opening one: `bin/rh fixture` (the test fixtures are generated,
  not checked in — see below), `cargo test --lib` plus the targeted
  integration test for what you touched, and a test that pins the fix.
  A reported repro with a patch in the issue is welcome; the same patch
  as a PR is better, because the lanes you cannot run will run.
- **Fixtures are generated.** `fixtures/real-blog` and `fixtures/store`
  are `.gitignore`d; a fresh clone has neither, and the tests that read
  them fail until `bin/rh fixture` (~60s, needs Ruby and
  `gem install rails`) and `scripts/create-store` have run. Tests reach
  them through `roundhouse::fixtures::real_blog()` / `store()`, which
  say so — with the command — when one is absent.
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
