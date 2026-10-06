# docs/ — map

The user guide is [`guide/`](guide/README.md) — install, and one door
each for analyzing, transpiling and compiling a Rails app. Everything
below is for people working on roundhouse itself.

Use the current code and executed checks for current behavior;
[`RELEASES.md`](../RELEASES.md) records dated release snapshots. Architecture
references explain design, while working plans are point-in-time proposals.

## Working on Roundhouse

- [`development/`](development/README.md) — setup and local workflow;
  [test selection](development/testing.md), [debugging](development/debugging.md),
  and [compiler changes](development/compiler-changes.md).
- [`ci/`](ci/README.md) — understand checks, request full/fresh validation,
  and distinguish validation from publication. Implementation details stay
  in the workflows, scripts, and their tests.
- [`../AGENTS.md`](../AGENTS.md) — invariants and agent task navigation.

## Compiler inputs — [`data/`](data/)

- [`ruby-and-erb.md`](data/ruby-and-erb.md) — Ruby + template ingest:
  Prism, the ERB/HAML compile-to-Ruby seam, surface preservation,
  strict vs survey mode.
- [`schema-routes-seeds.md`](data/schema-routes-seeds.md) — the
  declarative inputs: schema (and migration fallback), routes, seeds,
  fixtures, importmap, RBS sidecars.
- [`catalog.md`](data/catalog.md) — the method catalog: one IDL-shaped
  table for the Active Record surface, plus the gem catalog.
- [`adapter.md`](data/adapter.md) — the `DatabaseAdapter` trait:
  effect classification and async coloring per database backend.

## Pipeline internals — [`pipeline/`](pipeline/)

- [`analyze.md`](pipeline/analyze.md) — type + effect inference.
- [`lower.md`](pipeline/lower.md) — target-neutral lowerings and the
  post-analyze pass pipeline.
- [`emit.md`](pipeline/emit.md) — per-target emitters and the shared
  emit machinery.
- [`runtime.md`](pipeline/runtime.md) — the two-layer runtime
  (per-target primitives + transpiled framework Ruby), including the
  semantic-divergence ledger.
- [`verification.md`](pipeline/verification.md) — how we know the
  output is correct: evidence layers and their limits, not CI job topology.
- [`bytecode.md`](pipeline/bytecode.md) — experimental bytecode
  target; parked, test-only.

## Reference

- [`writebook.md`](writebook.md) — pinned external-corpus inventory and its limits.
- [`private-inertia-sorbet-corpus.md`](private-inertia-sorbet-corpus.md) — private Rails 8.1 + Inertia + Sorbet forcing corpus (themes only; not a support claim).

## Working plans

Point-in-time design documents; each records its own status at the
top. When one completes, it moves to [`archive/`](archive/).

- [`python-overlay-plan.md`](python-overlay-plan.md)
- [`relation-convergence-plan.md`](relation-convergence-plan.md)
- [`relation-type-plan.md`](relation-type-plan.md)
- [`maintainability-refactor-plan-2.md`](maintainability-refactor-plan-2.md)
- [`with-adapter-split-plan.md`](with-adapter-split-plan.md)
- [`lobsters-story-pages-plan.md`](lobsters-story-pages-plan.md)
- [`roda-sequel-plan.md`](roda-sequel-plan.md)

## [`archive/`](archive/)

Completed or superseded plans, kept as historical design records
(browser demo, the rust/kotlin/swift/csharp emitter migrations, the
jbuilder lowerer, maintainability refactor phase 1, and the
rust-migration spike crates). Accurate about the decisions they made;
not maintained against the current tree.
