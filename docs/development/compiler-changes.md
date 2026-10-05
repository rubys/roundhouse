# Compiler changes

Read [AGENTS.md](../../AGENTS.md) for invariants. This page helps locate the
owner of a change; [pipeline internals](../pipeline/) and [compiler inputs](../data/)
explain the individual stages.

## Ownership map

Application emission follows Ruby/template ingest → analysis → shared
post-analyze lowering → target emission → packaged project/runtime.
Source consumers (check/LSP/MCP and `emit_preview`) intentionally analyze
without that shared lowering sequence; see [`src/session.rs`](../../src/session.rs).

| Responsibility | Start here |
|---|---|
| Expression IR / application structures | `src/expr.rs`, `src/dialect.rs` |
| Prism → IR | `src/ingest/expr.rs`; other inputs under `src/ingest/` |
| ERB/HAML → Ruby | `src/erb.rs`, `src/haml.rs`, `src/ingest/view.rs` |
| Type / effect inference | `src/analyze/body/mod.rs`, `src/analyze/effects.rs` |
| Method catalog / DB adapter | `src/catalog/`, `src/adapter.rs` |
| Shared lowering / pass order | `src/lower/mod.rs::POST_ANALYZE_PASS_ORDER` |
| Target syntax | `src/emit/` |
| Project assembly / target dispatch | `src/project.rs::target_files` |
| Framework sources / shared transpile driver | `runtime/ruby/`, `src/runtime_loader.rs` |
| Hand-written target glue | `runtime/<target>/` |

Framework behavior belongs once in `runtime/ruby/` or a shared lowering, not
as the same special case in several emitters. The runtime loader shares its
transpile driver through target hooks. New Ruby runtime files also need
registration in `src/project.rs::spinel_files`.

Other entry points: `src/bin/` (CLI/debug tools), `wasm/` (browser compiler),
`editors/vscode/` (LSP client), `tools/compare/` (standalone differential oracle).
`scripts/` drives workflows; `tests/`, `e2e/`, and `tests/browser_smoke/` carry
the regression and browser harnesses.

## Adding an IR variant

1. Declare the variant in `src/expr.rs`. Retain source distinctions needed
   for expression IR round-trip; do not impose whole-app source preservation.
2. Ingest the construct in `src/ingest/expr.rs`. Unsupported shapes must
   produce an honest diagnostic, not disappear.
3. Update both type inference and effect traversal. An exhaustive match
   catches missing variants, not missing recursion or wrong effect propagation.
4. Add a shared lowering when targets need a simpler/common shape. Respect
   declared pass ordering and inspect all downstream walkers of the new nodes.
5. Implement correct target emission or keep the unsupported boundary explicit.
   Ruby expression emission must support snippet IR stability. Other targets
   are not permitted to silently approximate a construct accepted by `check`.
6. Pin ingest, inferred/lowered shape, and semantics with the appropriate
   [tests](testing.md#choosing-tests). Removing an error requires an emitted
   regression in `tests/emit_and_run.rs`, not just fewer diagnostics.

```sh
cargo test --test ingest
cargo run --bin roundhouse-ast -- --round-trip -e '[:a, :b]'
```

## Semantic checks

Preserve evaluation order, short-circuiting, and once-only evaluation. Use
asymmetric values and an effectful operand in regression tests where duplication
or reordering could be hidden. A pass that cannot preserve semantics must leave
an explicit unsupported/residue boundary rather than emit plausible wrong code.

The equivalence claims are different:

- Expression ingest → Ruby emit → ingest checks **IR stability**.
- Whole-app byte-for-byte source round-trip is retired.
- `tests/lowered_ruby_emit.rs` checks lowered structure;
  `tests/spinel_toolchain.rs` exercises native compilation;
  `tests/emit_and_run.rs` exercises emitted Ruby behavior.

A clean analyzer result alone proves none of the last two. Target-only runtime
gaps must remain named; do not claim support merely because another target runs.
