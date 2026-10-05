# Working on Roundhouse (agents & contributors)

Roundhouse reads Rails source and emits standalone projects in ~12 target
languages, plus an inference engine (LSP/MCP/in-browser IDE) that types Rails
without annotations. This file is the orientation an AI agent or new contributor
needs *before* touching the code: where to look, and the invariants not to break.

Current implementation and executed CI are authoritative for behavior.
[`RELEASES.md`](RELEASES.md) records dated snapshot claims, not a live main
status page. The [user guide](docs/guide/README.md) describes product usage;
the [bench page](https://rubys.github.io/roundhouse/bench/) carries measurements.

## Start here

| You want… | Read |
|---|---|
| What the project is | [`README.md`](README.md) — the landing page |
| Release snapshots and their known gaps | [`RELEASES.md`](RELEASES.md) |
| Using it (check / editor / MCP / transpile / Spinel) | [`docs/guide/`](docs/guide/README.md) |
| Set up a checkout | [`docs/development/README.md`](docs/development/README.md) |
| Choose tests or use `bin/rh verify` | [`docs/development/testing.md`](docs/development/testing.md) |
| Inspect AST, IR, or emitted output | [`docs/development/debugging.md`](docs/development/debugging.md) |
| Change IR, lowering, runtime, or an emitter | [`docs/development/compiler-changes.md`](docs/development/compiler-changes.md) |
| Read/request hosted checks | [`docs/ci/README.md`](docs/ci/README.md) |
| Pipeline internals (analyze / lower / emit / runtime / verification) | [`docs/pipeline/`](docs/pipeline/) — architecture, not status |
| Compiler inputs (Ruby+ERB, schema/routes/seeds, method catalog, DB adapter) | [`docs/data/`](docs/data/) |
| Why do this at all (the argument, option value) | [`WHY.md`](WHY.md) |
| Why this attempt is different (lineage, the three bets, risks) | [`BETS.md`](BETS.md) |

Pipeline shape: `Ruby AST → analyze (typed IR) → lower (target-neutral IR) →
emit (per-target project + runtime/<target>/ glue)`. The
[ownership map](docs/development/compiler-changes.md#ownership-map) locates each stage.

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

6. **A diagnostic you remove is a claim that the emitted program runs.**
   Invariant 1 makes an error mean "not supported yet," so a change that
   takes a construct from error to clean is a claim that it is now
   supported, and supported means the *output* works, not that `check`
   is quiet. Typing a call without a runtime behind it turns "reported
   unsupported" into "silently broken," which is worse than where it
   started. Pin such a change with `tests/emit_and_run.rs`: overlay the
   construct onto real-blog, and the harness asserts both halves, zero
   errors *and* the emitted Ruby test passing. A diagnostic-count test
   alone does not prove support. If the runtime half is out of scope,
   leave the error in place and ledger the gap.

7. **Spinel is part of this codebase.** It is Matz's Ruby-to-C compiler at
   `~/git/spinel`, co-developed. Defects → upstream issues/PRs with a minimal
   repro; genuine subset gaps → design around them *honestly* (record the gap,
   don't hide it with a workaround that pretends coverage exists).

## Workflow

- **Committers commit to `main`. No feature branches.** Stage only files
  you changed. End commit messages with the standard `Co-Authored-By`
  trailer.
- **Outside contributors: fork, and open a pull request against `main`.**
  Include the repro, a regression test, and actual verification results.
- Prepare the [fixtures and dependencies](docs/development/README.md#setup)
  before testing. Iterate with focused suites; follow the
  [test cycle](docs/development/testing.md) before committing.
- A local preview/subset is not full CI or merge approval. Report missing SDK
  coverage and request [broader checks](docs/ci/README.md) for broad/risky changes.
- Do not suppress diagnostics, comparison differences, or advisory failures
  merely to make a check green. Establish the failing input and cause first.

## The actual goal

The endpoint is not "does the fixture compile." It is a **per-target ledger of
how much of Rails transpiles** — the honest unsupported list, driven down over
time. Don't trade that real goal for a locally reachable one. Inspect the
relevant input, target, and executed gate before making a support claim.
