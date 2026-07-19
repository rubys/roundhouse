# Relation convergence plan — one representation, effects to terminal (R7 items 1-pricing, 2, 4)

Written 2026-07-19. Follow-on to docs/relation-type-plan.md (R1–R6 executed, commits
33ebdd74..ac4e438d; read its Execution log first — especially the R5 finding and the
post-plan `ac4e438d` dual-representation fix). Self-contained; re-verify file:line refs.
Suggested executor: Fable — phases C1/C2 involve semantic judgment on typing diffs.

## Sequencing (read first)

Run **AFTER** docs/with-adapter-split-plan.md lands — that plan restructures
`src/analyze/mod.rs`, which this plan then modifies. Do not run concurrently with it, nor
with docs/lobsters-story-pages-plan.md phase L2 (which also touches analysis/view typing).
Phases P0a/P0b below are independent and can run first in any case.

## Why this work

The Relation[T] plan's additive staging left TWO representations of a relation:
- inline chains starting at `Model.where(...)` → `Array<Self>`-typed (legacy)
- chains starting at a scope / relation-returning class method / assoc → `Ty::Relation{of}`

The `ac4e438d` incident (CI-caught Mastodon regression) proved the seam between them is a
live hazard class: a chain mixing representations (`Account.with_username` stayed Array,
`.with_domain` flipped Relation) fell into a delegation direction the machinery didn't
cover, and the harvest poisoned a method to Untyped. Both directions are now shimmed and
regression-tested, but every future seam needs both shims forever. Convergence deletes the
class. It also unblocks effects-to-terminal (R7 item 4): the two receiver-blind catalog
consumers currently filter the Relation context out specifically because of the dual
representation.

## Settled context — do not relitigate

All five settled decisions of docs/relation-type-plan.md stand, especially:
- Erasure-first: folded chains emit direct SQL, results `Array[T]`/`T?`. Convergence
  changes the *intermediate* typing only; fold behavior and terminal result types are
  invariant.
- No runtime relation classes for strict targets; no compile-time splice in this plan
  (P0a only PRICES it).

## Ground rules

- Commit to main, one commit per numbered step; never `git add -A`; never pipe cargo.
- Strong emit harness mandatory (this is analysis+lowering work; `emit_preview` skips
  post-analyze lowerings): `emit_preview` snapshot AND real `roundhouse` transpile of
  real-blog across all targets, diffed against a parent-commit worktree build, per step.
- `cargo test --all-targets` baseline diffed against parent before claiming regression.

## P0a — Price the splice (report only, no build)

Classify the 48 `dynamic_relation` residue sites in lobsters (the R6 ledger; construct id
`dynamic_relation`, reason `unspecialized_relation_chain`). Run the analyzer over the same
upstream-lobsters tree the R5/R6 session used (see relation-type-plan Execution log; the
deployment-bench checkout), collect the diagnostics, and classify each site:
- **param-less** (scope/chain fully determined by the model — splice-foldable with plain
  contribution recording), vs
- **parameterized** (needs contribution-with-holes, prepared-statement shaped — aligns
  with the ROUNDHOUSE_PARAM_BINDS cache design).
Also note per-site whether it's on a benchmarked route. Deliverable: a table in this
file's Execution log + a one-paragraph recommendation. This converts R7 item 1 into a
costed decision for Sam. **Do not build anything.**

## P0b — Capture Ufuk's challenge evidence for RubyConf (20 minutes, do before C1)

The two challenge gaps now work: relation→class delegation (`Story.recent.for_user(u)`)
and scope body-return inference. Capture the evidence while the machinery is exactly as
tested: run the challenge's example shapes through the analyzer (`roundhouse-mcp type_at`
against a small fixture, or adapt `tests/relation_typing.rs`'s MapVfs app) and save the
type readouts (input source + inferred types) as a note/slide asset in the talk repo at
`~/git/rubyconf-2026`. Match that repo's existing asset conventions; commit there, not
here. Capturing BEFORE C1 means the evidence reflects verified-landed behavior even if
C1 uncovers surprises.

## C1 — Converge inline-chain typing onto Ty::Relation

Retype chain *starts* on class receivers (`Model.where/order/joins/...` in
`src/analyze/body/send.rs` and the chainable registrations in the analyze registry) from
`Array<Self>` to `Relation { of }`. With R4a/R4b machinery already dispatching on
Relation receivers, downstream chain hops and terminals should flow through the existing
Relation path; the Array-representation relation branch (`array_method`'s relation arms,
`relation_return_on_array_repr`, and BOTH dual-representation delegation shims including
`ac4e438d`'s) then becomes dead — delete it in the same series, keeping
`relation_typed_scope_delegates_on_array_representation_receiver` retargeted or retired
with a note.

**The gate is emit-neutrality on fixtures, and it is a real hypothesis, not a formality.**
Two specific risk zones:
1. **Fold parity**: every chain that folded to SQL must still fold byte-identically —
   `try_build_arel` and its callers may key off `Array`-typing today; teach them
   `Relation{of}` receivers as equivalent BEFORE the retype commit so the series never
   has a broken intermediate state.
2. **Unfolded inline chains on strict targets**: any inline chain that today emits via
   Array-typing without folding would, post-convergence, hit the R1 unsupported-Relation
   emit diagnostic. If the fixture diff shows NEW strict-target errors, that is the dual
   representation having HIDDEN real gaps — STOP, list the sites in the log, and report
   to Sam rather than papering over (options are his: map Relation to the array repr in
   that emitter, or accept the error as truthful). Blog fixtures are expected to show
   zero such sites (everything folds); treat any nonzero as a finding.

Also expected and fine (document, don't fight): the `dynamic_relation` ledger counts
change — the residue pass counts Relation-typed chain heads, and convergence widens the
candidate set. Re-run blog (expect 0 still) and lobsters counts, record both.

Commit sequence suggestion: (1) arel/fold Relation-receiver parity, (2) retype chain
starts + delete dead Array-relation branch + test updates, (3) ledger re-run + doc.

## C2 — Effects to terminal for Relation chains

With one representation, finish what R2/R4 deferred (see catalog comments and the R2
log entry): remove the Relation-context filters in the two receiver-blind consumers
(`SqliteAdapter::classify_ar_method`, `Analyzer::is_builder_chain` — re-locate by name),
and move query-execution effects from builder steps to the terminal step for
Relation-typed chains. Builders become effect-free; terminals carry the Db effect.
Gate: effects tests (update only where the new placement is strictly more precise —
list each in the commit message), full suite, strong emit harness. If the effects tests
reveal consumers depending on builder-step effects in ways that aren't cheap to migrate,
STOP C2, log the dependency map, leave the filters in place — the plan's value is C1.

## Out of scope
Building the splice (P0a prices it only). Tier-2 branch enumeration. Runtime relation
classes. R7 items 5/6 (assoc-context revisit, strict-lane story) — decisions, not work.
`with_adapter` restructuring (done by the split plan before this runs).

## Execution log

(P0a classification table + recommendation; P0b asset pointer; C1 parity evidence,
deleted-shim list, ledger re-counts; C2 effects-migration list or stop verdict)
