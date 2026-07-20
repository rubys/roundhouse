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

### 2026-07-19 — Step 1 map (re-surveyed against b30fb5fc)

`with_adapter` is now `src/analyze/mod.rs:89–1574` (~1486 LOC). Structure:

- **89–100** setup: `classes: HashMap<ClassId,ClassInfo>` (the output), `module_include_map`.
- **101–396** per-model loop. Shared/threaded state: `self_ty`, `array_of_self`, the
  `instantiate` closure (wraps `instantiate_return_kind`), `module_include_map`, `AR_CATALOG`.
  Genuinely stateful — **stays in the loop** (Step 3).
- **398–1566** post-loop registration, in emit order (each region does `classes.insert`/`entry`):
  1. 410–423 `ActiveRecord::Base` — AR
  2. 434–442 `ActionController::Base` (`.helpers` proxy) — controller
  3. 444–556 `ActiveModel::Validations`/`Model` + `ActiveModel::Errors` class — activemodel
  4. 557–578 `CollectionProxy` — AR
  5. 580–601 individual `ActiveModel::Error` class — activemodel
  6. 603–655 DB adapter class (`adapter.class_name()`) — AR/adapter
  7. 656–714 `Arel` / `Arel::Table` / `Attribute` / `Node` / `SelectManager` — AR
  8. 716–768 `form_builder` + `ActionController::Collector` — view
  9. 770–958 **`action_view` accumulator** → `classes.insert(ActionView::Base)` @958.
     form_with, flat helpers, `tag`, flash accessors, `flash`, `json`, route helpers
     (view side), kaminari, params, simple_form, helper-fold. Depends on
     `route_helper_names` (built @779) + `form_builder_ty`. — view
  10. 960–1009 `ActionDispatch::Flash::FlashHash` class — view
  11. 1011–1034 `Rails` singleton — stdlib
  12. 1036–1057 `Time` singleton — stdlib
  13. 1059–1067 `Date`/`DateTime` — stdlib
  14. 1070–1142 stdlib singletons (SecureRandom/File/Dir/Math/CGI/ERB::Util/Digest/URI/Set) — stdlib
  15. 1144–1156 `GEM_CATALOG` fold — stdlib
  16. 1158–1277 `ApplicationController` surface (route helpers @1188, devise @1224) — controller
  17. 1279–1291 user RBS sidecars — misc (leave in orchestrator)
  18. 1293–1338 library classes (route helpers @1323) — library
  19. 1340–1461 ActionMailer classes — library
  20. 1463–1491 ActiveJob classes — library
  21. 1492–1543 Sidekiq workers — library
  22. 1544–1566 controllers registration — controller
- **1568–1573** `Self { classes, inferred_params: {}, adapter, concern_folded: {} }`.

**Cross-domain shared value:** `route_helper_names: Vec<String>` (built @779 via
`flatten_routes` + a `path_candidate` closure) is consumed by regions 9, 16, 18.
→ Extract as a free fn `route_helper_names(app) -> Vec<String>`, call once in the
orchestrator, pass `&[String]` to view/controller/library extractions. `flash_ty` and
`form_builder_ty` are inline `Ty::Class`/`block_fn` literals — reconstruct in-place where a
region needs them (byte-identical), no threading.

**Extraction plan (one commit each, gated):** stdlib.rs (11–15 + `register_stdlib_class`),
activemodel.rs (3,5), ar.rs (1,4,6,7), routes.rs (`route_helper_names` fn), view.rs (8,9,10),
controllers/library grouped last (2,16,18,19,20,21,22). Regions 17 + per-model loop stay in
orchestrator.

### Per-commit gate
Byte-identical emit harness in scratchpad: baseline captured at b30fb5fc (all 13 targets on
fixtures/real-blog, fixed output path). `gate.sh` rebuilds + re-transpiles + `diff -rq`.
Determinism verified. Plus `cargo test --all-targets` vs baseline snapshot (91 binaries,
0 failed at baseline).

### 2026-07-19 — Step 2 EXECUTED (7 commits, all gates green)

`Analyzer::with_adapter` went from ~1486 LOC to **388 LOC** (per-model loop + 7 registry
calls + RBS sidecars + controller-class registration + `Self{}`). New module family
`src/analyze/registry/` (1275 LOC across 8 files). Every commit: byte-identical emit across
all 13 targets + `cargo test --all-targets` clean.

- `0b0a528c` step 2a — `registry::stdlib` (Rails/Time/Date + stdlib singletons + GEM_CATALOG;
  moved `register_stdlib_class` helper along).
- `2bfa5d97` step 2b — `registry::activemodel` (Validations/Model + Errors + Error). CollectionProxy
  (AR) left inline between the two former activemodel spans; distinct keys ⇒ order-neutral.
- `0e920462` step 2c — `registry::ar` (ActiveRecord::Base + CollectionProxy + AdapterInterface + Arel family).
- `7c401789` step 2d — `registry::routes::route_helper_names(app) -> Vec<String>` (the cross-domain value).
- `94ce936e` step 2e — `registry::view` (FormBuilder, Collector, ActionView::Base accumulator, FlashHash);
  `block_fn` promoted to shared `registry::block_fn` free fn (was inline closure used by view + controller).
- `da4f1496` step 2f — `registry::library` (library classes, ActionMailer, ActiveJob, Sidekiq).
- `25994cb6` step 2g — `registry::controllers` (ActionController::Base + ApplicationController incl. Devise fold).
  ActionController::Base regrouped adjacent to ApplicationController; runs after `view::register`
  (Devise fold mutates ActionView::Base). Region 22 (per-app controller-class registration) LEFT INLINE —
  depends on mod-private `controller_includes` + on ApplicationController inserted first.

**Threading design:** each `register` takes `&mut HashMap<ClassId, ClassInfo>` plus explicit
`app` / `&[String] route_helper_names` params (no struct-field widening). `flash_ty`/
`form_builder_ty` reconstructed in-place where needed (inline `Ty` literals). `route_helper_names`
bound once in the orchestrator, passed to view/controllers/library.

### Step 3 — per-model loop: JUDGMENT = LEAVE INLINE
The `for model in &app.models` loop (~294 LOC) accumulates a single `cls: ClassInfo` across ~12
sub-passes threading `self_ty`, `array_of_self`, the `instantiate` closure (borrows `model.name`),
`module_include_map`, `scope_names`, and `includes`. This is the genuinely-shared-state region the
plan flagged as the risk area. Clean sub-seams exist (class-query surface, schema attrs, associations,
concern-DSL fold) but extracting them buys little over the achieved 388-LOC orchestrator and adds
closure/borrow-threading risk. Left inline per plan ("when in doubt, leave it").

### Steps 4 / 5 — DEFERRED (optional, time-gated)
Sibling free-fn extractions (render/effects/diagnose) and insert-run table-ification not pursued this
session; independent of the relation-convergence sequencing unblock. Step 2 (the code-motion this plan
existed to do) is complete, so docs/relation-convergence-plan.md is now unblocked.
