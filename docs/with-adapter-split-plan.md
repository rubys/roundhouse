# with_adapter split plan — analyze/mod.rs code motion (refactor 4.3)

Written 2026-07-19. Self-contained for a fresh session; re-verify all file:line refs by
grep/function-name before editing — this file has churned recently (R3/R4a of
docs/relation-type-plan.md landed scope seeds and extracted `instantiate_return_kind`).
Suggested executor: Opus — this is pure code motion with a byte-identical gate.

## Why now / sequencing (read first)

This is refactor-plan step 4.3 (docs/maintainability-refactor-plan.md), queued until the
Relation[T] plan landed — it has (R1–R6 done, commits 33ebdd74..ac4e438d). It must land
**BEFORE** docs/relation-convergence-plan.md executes: that plan modifies the scope-seed
and registration logic that lives in `with_adapter`, and motion-then-modify beats rebasing
a 1500-line split over live changes. Do not run the two concurrently.

## Ground rules

- **Behavior-neutral, pure code motion. No logic edits, no renames beyond module paths,
  no table-ification** (that's optional step 4 below, separately gated).
- Commit to main, one commit per numbered step; stage only files you changed (untracked
  strays exist — never `git add -A`).
- Gate per commit: `cargo test --all-targets` identical to parent commit, PLUS the strong
  emit harness — `with_adapter` is analysis code, and `emit_preview` **skips post-analyze
  lowerings**, so also diff real `roundhouse` transpile output (all targets, real-blog)
  against a parent-commit worktree build. Byte-identical or revert.
- Never pipe cargo. Some `#[ignore]`d tests fail on main by design — diff against parent
  before claiming regression.
- If any step turns out to require a logic change to disentangle shared state, STOP that
  step, log it below, move on.

## Step 1 — Survey the current shape (no edits)

`Analyzer::with_adapter` was `src/analyze/mod.rs:89–1585` (~1496 LOC) at last survey:
a per-model loop (~:101–404) threading shared `self_ty`/`instantiate` state, then
~1180 lines of post-loop registration (ActiveModel Dirty/Validations/Errors, CollectionProxy,
adapter classes, ActionView form builders, view-context self, route URL helpers, flat view
helpers, flash, stdlib). Re-map the boundaries by reading the function top to bottom and
listing the extraction seams and every piece of shared mutable state each region touches
(the per-model loop's closures are the risk area — R4a already extracted
`instantiate_return_kind` from one; look at how that was done and match it). Record the
map in the Execution log.

## Step 2 — Extract the post-loop registration domains (the bulk)

New module family `src/analyze/registry/` — suggested split, adjust to the Step-1 map:
- `ar.rs` — AR class/instance methods, Arel entry points, chainable query-builder
  registration (includes the `AR_CATALOG`-consuming loops)
- `activemodel.rs` — Dirty / Validations / Errors / has_secure_password
- `view.rs` — form builders, view-context self, flat view helpers, flash accessors
- `routes.rs` — route URL helpers
- `stdlib.rs` — `register_stdlib_class` + friends (these free fns live later in the file;
  move them with their callers)

Each extracted fn takes `&mut HashMap<ClassId, ClassInfo>` (plus whatever explicit params
the Step-1 map shows — pass state explicitly rather than widening struct fields).
`with_adapter` becomes a short orchestrator calling them in the original order. One
commit per domain, gate each.

## Step 3 — The per-model loop

Likely stays in `with_adapter` (it's the part with genuinely shared state). If the
Step-1 map shows clean sub-seams (e.g. scope seeding, association registration as
separable passes over one model), extract them as `registry/model.rs` helpers called
from inside the loop — but only where the state threading stays explicit and mechanical.
When in doubt, leave it; log the judgment.

## Step 4 (optional) — Sibling squatter extractions in analyze/mod.rs

Same pure-motion treatment, one commit each, only if time permits (function names, not
stale line numbers, are the locator):
- View/partial resolution free fns (`interpret_render_call`, `resolve_partial_path`,
  `extract_partial_render_sites`, …) → `analyze/render.rs`
- Effects subsystem (`collect_effects`, `visit_effects`, `contribute_send_effect`, …) →
  `analyze/effects.rs`
- Diagnostics walker (`diagnose`, `diagnose_with_coverage`, `diagnose_expr`) →
  `analyze/diagnose.rs`
- `inferred_types` / `collect_types_expr` → alongside `ide.rs`'s consumers

**Do NOT touch `run_typing_passes`** — order-sensitive multi-pass orchestration, a design
task for Sam (unchanged verdict from the parent plan).

## Step 5 (optional) — Table-ify uniform insert runs (parent plan 4.4)

Only where an extracted builder is a run of uniform `methods.insert(name, ty)` calls:
convert to a `const` table + loop. Skip anything conditional. This is the only step that
changes code shape beyond motion — gate extra carefully.

## Out of scope
Any behavior change; `run_typing_passes`; the Relation-convergence work (separate plan,
runs after this); `src/catalog/` (only consumed, not restructured).

## Execution log

(step-1 map, per-commit gates, judgments, skipped seams)
