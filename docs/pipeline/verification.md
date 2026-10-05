# Verification

Different checks prove different claims. No single green suite proves that
arbitrary Rails code transpiles correctly. Local test commands live in
[development/testing.md](../development/testing.md); GitHub selection and
publication live in [CI](../ci/README.md).

## Evidence layers

| Layer | Claim on the tested input | Important limit |
|---|---|---|
| Ingest and analysis | Source was retained and expressions typed without error diagnostics | No proof the emitted program runs |
| IR identity | Serialization or expression Ruby round-trip retains IR | No whole-app source equality or behavioral proof |
| Framework tests | Source/transpiled runtime satisfies the exercised assertions | Coverage differs by target and selected subset |
| Toolchain | Emitted project builds and selected tests execute | Compilation alone is not Rails equivalence |
| Live DOM/JSON comparison | Emitted responses match a Rails oracle after defined normalization | Only exercised requests and state; not every side effect |
| Archive smoke | Actual archive can execute its README commands and test floors | Does not certify sibling archive formats or arbitrary apps |
| Browser and app conformance | Dynamic interactions or upstream test scenarios work | Floors/subsets are not whole-app conformance |

## Ingest, typing, and execution

[`tests/real_blog.rs`](../../tests/real_blog.rs) guards zero errors and zero
unresolved types on the generated blog. The framework typing gate is
`tests/runtime_src_integration.rs::every_runtime_method_body_is_fully_typed`.
The CRuby source suite (`tests/runtime_ruby_unit.rs`) and transpiled
`framework_tests_<target>` suites test the runtime itself. Some harnesses skip
when prerequisites are absent; some targets exercise only a subset. Read what
actually executed.

Removing an error diagnostic claims emitted support. Pin that claim with
[`tests/emit_and_run.rs`](../../tests/emit_and_run.rs): the overlaid construct
must analyze cleanly **and** execute correctly in the emitted Ruby app.
Other target behavior still needs its own relevant evidence.

## Three meanings of round-trip

`tests/roundtrip.rs` checks `App → JSON → App`; `tests/runtime_src_roundtrip.rs`
and `roundhouse-ast --round-trip` check Ruby expression/method IR stability.
The CLI round-trip accepts Ruby input, not ERB.

Whole-app source equivalence is retired. Application Ruby emission consumes
lowered IR: `tests/lowered_ruby_emit.rs` checks that shape and
`tests/spinel_toolchain.rs` exercises native compile-equivalence. Neither
serialization identity nor a stable expression implies correct runtime output.

## Differential oracle

[`tools/compare/`](../../tools/compare/) compares live Rails and emitted
responses. HTML element trees and text must match; attribute order and comments
are ignored. Defined masks handle legitimately variable values such as tokens
and asset fingerprints. JSON comparison is structural, with timestamp precision
normalization. The exact normalization lives beside the comparator.

`scripts/compare` drives the fixture comparison; the
[user guide](../guide/verifying.md) explains applying the oracle to another app.
Do not extend masks for a translation bug. Translated application tests alone
are weaker: translating the code and its expectations incorrectly can still
produce agreement.

## Packaging and dynamic behavior

[`scripts/smoke`](../../scripts/smoke) extracts an archive and executes its
README shell blocks rather than maintaining another per-target recipe. It
enforces executed-test floors, so a successful empty test run is insufficient.

[`e2e/`](../../e2e/) tests browser behavior such as Turbo updates, Action Cable,
validation, and styles. [`tests/browser_smoke/`](../../tests/browser_smoke/)
exercises SharedWorker and browser compiler surfaces.

`scripts/campfire-suite` runs the pinned app's own tests and reports a worklist;
CI owns its conformance floors. Inventory-only corpus gates, such as
[Writebook](../writebook.md), do not run the app and must not be described as
compilation, runtime, or UI conformance.
