# docs/ — map

Architecture references live in the two subdirectories; the loose files
at this level are working plans. Status discipline: these docs describe
*architecture*, not day-to-day status — when a status claim here
disagrees with [`README.md`](../README.md) or CI, README and CI win
(see [`AGENTS.md`](../AGENTS.md)).

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
  output is correct: the test layers and the CI gate topology.
- [`bytecode.md`](pipeline/bytecode.md) — experimental bytecode
  target; parked, test-only.

## Reference

- [`env-gates.md`](env-gates.md) — every `ROUNDHOUSE_*` environment
  variable the codebase reads.

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
