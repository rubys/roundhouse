# Maintainability refactor plan

Written 2026-07-18 from a four-way structural survey of the codebase (emitters, lowering/IR,
analyzer/types/catalog, tests/runtime/glue). This plan is designed to be executed by a fresh
Claude session with no prior context. All file:line references were verified on main at the
time of writing; re-verify with grep before editing — line numbers drift.

## Ground rules (non-negotiable)

1. **Every phase is behavior-neutral unless explicitly flagged otherwise.** This is a
   refactor, not a feature or bugfix pass. The gate for "neutral" is: emitted output for the
   fixture apps is byte-identical before and after (see Verification below), and the test
   suite passes identically.
2. **Commit to main, one commit per numbered step.** No feature branches. Stage only files
   you changed (`git add <specific files>`, never `git add -A` — the repo has untracked
   strays like `proc_capture.c`, `bench/probes/`, `.DS_Store` that must not be committed).
3. **Each phase is independently valuable and a legal stopping point.** If a step turns out
   to be non-neutral or bigger than described, STOP that step, write up what you found in
   a short note at the bottom of this file under "Execution log", and move to the next
   independent step. Do not force it.
4. **Never pipe cargo output** (no `cargo test 2>&1 | tail`); run it plainly. For any change
   that adds/removes enum variants, run `cargo test --all-targets` (unit tests hide there).
5. Some `#[ignore]`d tests fail on main by design (feature-gap baselines). Before claiming
   a regression, check whether the failure pre-exists on the parent commit.

## Verification harness (set up once, reuse every step)

Baseline snapshot before any edits:

```sh
cargo build --tests
cargo test            # record pass/fail counts; some ignored tests are expected
```

Emit-neutrality check — use the `emit_preview` binary (`src/bin/emit_preview.rs`) against the
in-repo fixture apps. Before starting, generate a baseline tree per target into a scratch
dir; after each step, regenerate and `diff -r`. Look at how `tests/lowered_ruby_emit.rs` and
`scripts/compare` drive emission if `emit_preview`'s CLI is unclear. A refactor step is done
only when the diff is empty (or the step is flagged behavior-affecting below).

For emitter-touching steps additionally run the relevant target's local check
(e.g. crystal changes → the crystal toolchain test / `scripts/compare crystal`) when the
toolchain is installed; skip gracefully if not.

---

## Phase 1 — Zero-risk deletions and dedup (small, do first)

### 1.1 Delete the dead query algebra
`src/query.rs` defines `enum Query` + `enum Predicate` — a full relational algebra with
**zero references** anywhere except `pub mod query;` in `src/lib.rs:36`. The live query
subsystem is `src/lower/arel/` (`ir.rs` has its own `Predicate`; `visitor.rs`, `build.rs`)
plus `src/schema.rs`. Two `Predicate` enums actively mislead readers.

- Re-verify deadness: `grep -rn "crate::query\|query::" src/ tests/` must show only the
  `lib.rs` mod decl (and any hits that are actually `arel` or local idents).
- Delete `src/query.rs`, remove the mod decl. `cargo build --tests`, full test run, commit.

### 1.2 One shared `residue()` constructor
The helper `fn residue(expr, reason) -> Diagnostic` building
`DiagnosticKind::LowerResidue { pass, construct, reason }` is copy-pasted in six lowering
passes: `src/lower/update_kwargs.rs:47`, `errors_add.rs:43`, `job_class_side.rs:188`,
`create_block.rs:39`, `mailer_class_side.rs:158`, `send_dispatch.rs:82`.

- Add one shared constructor (natural home: `src/lower/mod.rs` or a helper on
  `Diagnostic` in `src/diagnostic.rs`) taking the pass name explicitly:
  `residue(pass: &str, expr: &Expr, reason: ...)`.
- Replace all six copies. The rendered diagnostic text must be unchanged (there are tests
  asserting on diagnostics). Commit.

### 1.3 Reconcile the two target enums
`src/profile.rs:66 enum Target` has 8 variants and is missing `Kotlin`, `Swift`, `CSharp`,
which exist in `src/project.rs:40 enum BuildTarget` (14 variants).

- **Judgment step, not blind sync**: first determine whether `profile::Target` is
  deliberately a subset (profiling lanes only). Read its consumers.
- If deliberate: add a doc comment on `profile::Target` cross-referencing `BuildTarget` and
  stating the subset is intentional, so the next reader doesn't "fix" it.
- If accidental drift: add the missing variants and follow the compiler errors
  (`cargo test --all-targets` per ground rule 4). Commit either way.

### 1.4 Catalog the env-var gates
10 live `ROUNDHOUSE_*` env vars exist with zero documentation: `ROUNDHOUSE_APP_ROOT`,
`ROUNDHOUSE_ASSETS_DIR`, `ROUNDHOUSE_BASE`, `ROUNDHOUSE_INGEST_SURVEY`,
`ROUNDHOUSE_PARAM_BINDS`, `ROUNDHOUSE_GO_V2`, `ROUNDHOUSE_GO_V2_MODELS`,
`ROUNDHOUSE_RUST_V2`, `ROUNDHOUSE_RUST_V2_LEGACY`, `ROUNDHOUSE_RUST_V2_EMIT_TESTS`,
`ROUNDHOUSE_ELIXIR_V1`. (`docs/pipeline/emit.md:228` references a
`ROUNDHOUSE_<TARGET>_VIEW_THIN` pattern that matches no code — fix or delete that mention.)

- `grep -rn 'ROUNDHOUSE_' src/ scripts/` to get the authoritative current list (it may have
  grown since this was written).
- Write `docs/env-gates.md`: one table — var, default, effect, one-line "when to remove"
  lifecycle note for the `_V1/_V2/_LEGACY` migration toggles. Docs-only commit.

---

## Phase 2 — Give `Ty` its behavior (`src/ty.rs`, currently 131 lines, ~19 variants)

There are ~337 open-coded `matches!(… Ty:: …)` sites across `src/`, and the lattice
operations live scattered outside the type.

### 2.1 Add predicate methods
Add to `impl Ty` (names indicative):
- `is_unknown()` → `Var { .. } | Untyped` (≥6 exact-match sites exist)
- `is_open()` → `Var { .. } | Bottom` (≥6 sites)
- `is_stringish()` → `Str | Sym`
- `is_scalar()` → `Int | Float | Bool | Str | Sym`

**THE TRAP — read carefully**: the existing `matches!` clusters have *inconsistent
membership* (some sites use `Untyped | Var | Nil`, others add `Bottom`, others omit `Nil`).
The inconsistency may be load-bearing per site. Migration rule: **only replace a `matches!`
site whose variant set is exactly identical to the predicate's definition.** Do NOT
"normalize" a site to the nearest predicate — that is a silent behavior change. Collect the
near-miss sites (same cluster ± one variant) into a list appended to this file's Execution
log for Sam to review later; leave their code untouched.

Migrate in a few commits (e.g. one per predicate), full test run + emit-diff each.

### 2.2 One `union_of`
Two independent implementations exist: `src/analyze/body/mod.rs:1232 union_of` (with
`union_many:1345`, `peel_nilable:1119`, `canonicalize_variants:1341`) and
`src/rbs.rs:691 union_of`. Related scattered nil ops: `strip_nil` (`src/analyze/mod.rs:4611`),
`can_be_nil` / `nil_verdict` (`src/ide.rs:168,183`).

- **First diff the two implementations semantically.** If they behave identically, move the
  body/mod.rs version (it is the more complete one) onto `impl Ty` (or a `ty::lattice`
  module) and delegate both call sites. If they differ, document the difference in the
  Execution log and make each original call site keep its exact prior semantics — a wrapper
  per caller if needed. Do not pick a winner silently.
- Move `strip_nil`, `peel_nilable`, `canonicalize_variants` to `ty.rs` as pure code motion.
  Leave `ide.rs`'s `can_be_nil`/`nil_verdict` alone for now (they are query-layer verdicts,
  not lattice ops).
- Gate: full test run (rbs tests especially) + emit-diff neutral. Commit.

---

## Phase 3 — Shared emitter helpers (collapse the 9× copies)

Model everything here on the pattern that already works: `src/emit/shared/add.rs` —
`classify_add(lhs, rhs) -> AddCase` where a shared classifier makes the *decision* and each
target renders its own syntax. Never move syntax into shared code; move decisions.

Migrate **one target per commit** with an emit-diff gate each time. If a target's migration
is not byte-neutral, stop that target, log it, continue with the others.

### 3.1 `split_trailing_kwargs` helper
`ExprNode::Hash { entries, kwargs: bool }` (`src/expr.rs:242`) already flags kwarg hashes,
but the trailing-kwargs split is re-implemented in at least: `csharp/expr.rs:1577`,
`elixir2/expr.rs:1634`, `crystal/expr.rs` (multiple), `kotlin/expr.rs:262`,
`go2/expr.rs:2642`, swift. Add `shared::args::split_trailing_kwargs(args) -> (positional,
Option<kwarg_entries>)` and migrate each site.

### 3.2 String-interpolation walker
Nine private copies of `emit_string_interp(parts: &[InterpPart])`: `crystal/expr.rs:762`,
`ruby/expr.rs:394`, `elixir2/expr.rs:2974`, `kotlin/expr.rs:979`, `csharp/expr.rs:977`,
`swift/expr.rs:1345`, inline at `typescript/expr.rs:558`, `go2/expr.rs:438`,
`python/expr.rs:752`. The deltas are delimiter (`#{}`, `${}`, `\()`) and escape table.
Add `shared::interp::render(parts, open, close, escape: impl Fn) -> String` (or a small
config struct) where the *escape fn stays per-target* (see 3.3). Migrate target by target.

### 3.3 Do NOT unify `escape_str`
Near-identical copies exist (`swift/expr.rs:1299`, `kotlin/expr.rs:905`,
`csharp/expr.rs:909`) but they differ in exactly the characters that are target-semantic
(`$`, `\b`/`\f`, control-char fallbacks). Unifying them behind a config invites subtle
breakage for near-zero LOC win. Leave them; the interp walker in 3.2 takes the escape fn as
a parameter.

### 3.4 Shared binary-operator dispatch
The binop block is byte-identical (comment included) in `swift/expr.rs:2333`,
`kotlin/expr.rs:1510`, `csharp/expr.rs:1367`, each followed by the same
`<<`/`push` append special-case (also `go2/expr.rs:1220`). Add a shared classifier
(`shared::ops::classify_binop(method, receiver, args) -> BinopCase` with cases like
`NativeInfix(op)`, `Append`, `NotBinop`) and migrate the three identical targets first.

### 3.5 Adoption of existing arithmetic classifiers — flag, don't force
`classify_add`/`classify_sub`/`classify_mul` (`src/emit/shared/`) are consumed by only
6 of 10 targets; swift/kotlin/csharp/crystal/go2 re-decide string-vs-numeric inline.
**This step is potentially behavior-affecting**: adopting the classifier may *change*
emitted output where the inline version was wrong (e.g. string-concat vs numeric-add on
untyped operands). Per target: adopt, emit-diff; if the diff is non-empty, inspect whether
each change is a correction. If yes, note it in the commit message as a behavior change and
run that target's toolchain check locally. If unclear, revert that target and log it.

### 3.6 (Optional, large) Variant-split the monolith emitters
House rule says emitters >2K LOC split by IR variant with `try_*` helpers; only `rust2/expr/`
follows it (`mod.rs`, `control.rs`, `literal.rs`, `assign.rs`, `send/…`, `decide/…`).
Monoliths: `go2/expr.rs` (3590), `typescript/expr.rs` (3506), `elixir2/expr.rs` (3067),
`swift/expr.rs` (2626). This is pure code motion — no logic edits — one target per commit,
mirroring rust2's module layout. Do this LAST within phase 3, and only if time permits;
skip entirely rather than half-split one target.

---

## Phase 4 — Method-registry unification (highest value, most care)

Today a Rails method is known in up to four parallel places. Canonical smell — `pluck`:
`AR_CATALOG` entry (`src/catalog/mod.rs:343`), the separate `is_query_builder_method` match
(`catalog/mod.rs:898`), an imperative insert loop in `Analyzer::with_adapter`
(`src/analyze/mod.rs:233`), and a hardcoded return-type arm in
`src/analyze/body/send.rs:921` (one of ~147 string-literal arms in `dispatch`,
send.rs:238). Plus `GEM_CATALOG` (`src/catalog/gems.rs:89`) and RBS overlays with different
shapes. Goal end-state: `CatalogedMethod` in `AR_CATALOG` is the single source of truth for
builtin AR-method facts; `send.rs` and `with_adapter` *read* it.

Do this incrementally — never a big-bang rewrite:

### 4.1 Make `is_query_builder_method` derive from `AR_CATALOG`
It duplicates chain/receiver info already in the table. Replace the match with a table
lookup. If any method is in the match but not the table (or vice versa), that delta IS the
finding — add the missing table entries so behavior is identical, and log the delta.

### 4.2 `send.rs` reads return kinds from the catalog
Pick a small cluster of methods that exist in both `AR_CATALOG` and `send.rs` match arms
(start with the query projections: `pluck`, `pick`, `ids`, …). For each: confirm the
catalog's `return_kind` reproduces exactly what the match arm computes, then delete the arm
and route through a catalog lookup at the top of `dispatch`. One cluster per commit, full
`cargo test --test analyze` (tests/analyze.rs is 103KB — the main behavioral net) +
emit-diff per commit. Expect to leave a long tail of arms whose logic is genuinely
contextual (arg-dependent types) — that's fine; the goal is that *simple* facts live in the
table and only *irreducibly contextual* logic stays in code.

### 4.3 Split `with_adapter` (src/analyze/mod.rs:89–1585, ~1496 LOC)
Pure code motion first: split the one giant function into per-domain builder functions in a
new `src/analyze/registry/` module family — `ar.rs` (AR class/instance + chainables),
`activemodel.rs` (Dirty/Validations/Errors), `view.rs` (form builders, view-context self,
flat helpers), `routes.rs` (URL helpers), `stdlib.rs` — each taking
`&mut HashMap<ClassId, ClassInfo>`. `with_adapter` becomes a short orchestrator. NO
table-ification in this step — just motion. Commit.

### 4.4 (Optional follow-on) Table-ify the moved builders
Where a builder function is a run of uniform `methods.insert(name, ty)` calls, convert to a
`const` table + loop, matching `CatalogedMethod`'s shape where possible. Only do the uniform
runs; leave conditional registration logic as code.

While in `analyze/mod.rs`, the same pure-code-motion treatment applies to its other
squatters when convenient (each its own commit): view/partial resolution free fns
(~lines 3827–5157 → `analyze/render.rs`), effects subsystem (3045–3792 →
`analyze/effects.rs`), diagnostics walker (5159–5553 → `analyze/diagnose.rs`),
`inferred_types` IDE support (5554–5729 → near `ide.rs`). Do NOT attempt to split
`run_typing_passes` (1797–3044) — it is order-sensitive multi-pass orchestration; extraction
there is a design task for Sam, not a mechanical one.

---

## Phase 5 — `Session` bootstrap facade

The ingest → `Analyzer::new` → `analyze` → diagnostics dance is copy-pasted at 7 entry
points: `src/project.rs:2070`, `src/mcp.rs:162` (via `Server::analyze`, mcp.rs:74),
`src/bin/roundhouse.rs:226`, `src/lsp.rs:479` (via `run_analysis`, lsp.rs:460),
`src/bin/dump_ir.rs:96`, `src/bin/emit_preview.rs:15`, `src/bin/roundhouse-check.rs:102`.
There is an agreed-but-unbuilt `roundhouse analyze` CLI that must NOT become copy #8.

- Extract one facade (suggested: `src/session.rs`, `Session::open(root, …) -> AnalyzedApp`
  or similar) capturing the common sequence. The entry points differ in details (LSP has a
  VFS overlay + background thread; MCP re-ingests per call) — the facade should cover the
  common core and take the variation as parameters, not absorb the LSP threading model.
- Migrate the 4 bin/ entry points first (simplest), then project.rs, then mcp.rs, then
  lsp.rs. One or two entry points per commit, behavior-neutral.
- Do NOT build incrementality/caching into this — that's future work; the facade is just
  the seam where it will later live.

## Phase 6 — Target-registration fan-out

Adding a runtime/ruby stem currently means editing up to 9 hand-maintained tables in
`src/runtime_loader.rs` (2016 lines): `TYPESCRIPT_RUNTIME:274`, `CRYSTAL_RUNTIME:437`,
`RUST_RUNTIME:568`, `KOTLIN_RUNTIME:901`, `CSHARP_RUNTIME:1135`, `SWIFT_RUNTIME:1296`,
`GO_RUNTIME:1474`, `ELIXIR_RUNTIME:1676`, `PYTHON_RUNTIME:1866` — 76 `RuntimeEntry`
literals, most `include_str!`-ing the same `runtime/ruby/*.rb`+`.rbs` pairs. (Note:
`src/runtime_src.rs` is the shared parser half — it does NOT duplicate the loader; leave it.)

- Introduce one shared manifest: a single table of runtime units
  (stem → rb/rbs `include_str!` pair), plus a per-target list of *stem names* (or
  include/exclude deltas from a default set). Derive the 9 tables from it. The per-target
  variation that exists today (some targets omit units, `python_units_subset:2005`) must be
  expressible — check every table's delta from the union before designing the manifest.
- Gate: the generated per-target unit lists must be exactly identical to the current tables
  (write a temporary assertion test comparing old vs new lists, then delete the old tables).
- **Defer, do not do**: unifying `spinel_files`/`ruby_runtime_files`/`jruby_runtime_files`
  in `project.rs` — those encode deliberate per-lane patches tied to active lobsters bench
  work; touching them risks in-flight work.

### 6.1 CI matrix conversion (independent, cheap)
`.github/workflows/ci.yml` (~82KB, 46 jobs, zero `matrix:` blocks) hand-clones 12
`compare-<target>` jobs (ci.yml:775–1101) and 12 `smoke-<target>` jobs (1170–1522) that all
just call `scripts/compare <target>` / `scripts/smoke`. Convert each family to one job with
a `strategy.matrix.target` axis + per-target toolchain-install steps keyed off matrix
`include` entries. Keep the `toolchain-*` / `framework-tests-*` jobs as-is for now (their
setup steps genuinely differ more). CI-only commit; expect to validate on the next push
(per repo practice, don't block on watching CI — check back on the run once).

## Phase 7 — Lowering-layer cleanups

### 7.1 Codify pass ordering
`apply_post_analyze_lowerings` (`src/lower/mod.rs:103-131`) is a hand-ordered list of ~20
`apply_*` calls; ordering constraints live only in prose ("AFTER send_dispatch, by
contract" — mod.rs:126, duplicated at `duration.rs:15`; more at `send_dispatch.rs:45`,
`typed_store.rs:145`). Minimal fix, no pass-manager framework: represent the pipeline as a
list of `(name, fn, runs_after: &[name])` entries and assert the declared order is
satisfied (debug assertion or test). The prose comments then point at one authority.

### 7.2 Options struct for the telescoping lowerer entry
`lower_controllers_with_arel_views_assocs_and_routes`
(`src/lower/controller_to_library/mod.rs:173`) has 8 positional params, grown one wrapper
per feature. Introduce a params struct; collapse the wrapper chain if the intermediate
arities have no external callers.

### 7.3 Python emitter re-classifies view helpers — behavior-affecting, flag it
`src/emit/python/view.rs:530,661,872` re-runs `classify_view_helper` at emit time instead of
consuming lowered IR, so a newly added view helper silently no-ops on Python until someone
remembers Python's extra match arm. Investigate routing Python through the lowered form
other targets consume. **This will likely change Python output (that's the point).** Treat
as its own mini-project: understand why Python diverged (its view emit is split
app-side into `controller/model/route/view`), make the change, run python compare/toolchain
tests, commit with the behavior change described. If the divergence is deep, log findings
and leave for Sam.

### 7.4 Deduplicate route-helper lowering — investigate first
Both `controller_to_library/rewrites.rs` (`rewrite_redirect_to:1132`,
`rewrite_route_helpers:1299`, `polymorphic_path:1261`) and `view_to_library`
(`helpers.rs:279 emit_url_arg`, `mod.rs:3144-3157 route_helpers_call`) independently lower
the record-as-URL idiom to `RouteHelpers.<x>_path(...)`. Extract a shared
`src/lower/route_helper.rs` consumed by both. The two sides have real contextual
differences (controller has redirect semantics; view has form/url-arg contexts) — share the
record→path-name resolution core, not the whole rewrite. Emit-diff gate.

### 7.5 `view_to_library/mod.rs` extraction (pure code motion)
At ~3287 lines with sibling modules already established (`helpers.rs`, `form_builder.rs`,
`partial.rs`, `walker.rs`, `predicates.rs`). Move, one commit each:
- ivar→local rewriting (~mod.rs:2480-2917) → `ivar_rewrite.rs`
- partial/render-edge graph (~mod.rs:2079-2362) → merge into `partial.rs`
- framework/db stub insertion (~mod.rs:701-1262) → `stubs.rs`
- reader-fact derivation (~mod.rs:2918-3017) → `reader_facts.rs`. NOTE for the log, do not
  fix here: these derive nilability/bool/association facts into flat name-sets keyed by
  attribute name only (no model scoping) — same-named attributes on different models alias.
  That is a known modeling gap; moving the code must not change it.

Line ranges above predate commits c850ebb0 and later work in this file — re-locate the
clusters by function name (`rewrite_ivars_to_locals`, `render_partial_keys`,
`insert_db_stub`, `nilable_scalar_reader_names`), not by line number.

---

## Explicitly out of scope (do not touch)

- `src/diagnostic.rs` — the one well-factored subsystem; leave it.
- `src/lower/functionalize/` gating — correctly gated at the elixir2 emit boundary.
- `run_typing_passes` decomposition — design work, not mechanical.
- Incrementality/caching for LSP/MCP — future work behind the Phase 5 facade.
- `spinel_files` / ruby / jruby runtime-file fns in project.rs — active bench work.
- Cross-target runtime conformance harness — worthwhile but a design conversation first.
- `escape_str` unification (see 3.3).

## Suggested execution order and stopping points

Phases 1 and 2 first (small, high signal). Then 3.1–3.4, then 4.1–4.3. Phases 5–7 are
independent of each other; pick by available time. 3.6 and 4.4 are optional fillers.
Every numbered step is a legal stopping point.

## Execution log

(append findings, skipped steps, near-miss `matches!` sites, and behavior deltas here)

### Session summary (2026-07-18)

**Done + pushed** (all behavior-neutral; emit byte-identical across 9 targets × 2 fixtures
verified per step, full `cargo test --all-targets` green):
`1.1` `1.2` `1.3` `1.4` · `2.1` (4 predicates, 34 sites) `2.2` · `3.1` `3.2` `3.4`
· `4.1`†  · `7.1` `7.2`.

**Directive honored, no code:** `3.3` (do-not-unify `escape_str` — respected throughout 3.2).

**Findings / blocked (documented, no forced change):**
- `4.1`† — premise false: `is_query_builder_method` is a narrower curation, NOT catalog-
  `chain`-derivable (would broaden by 26 methods = lowering behavior change). Added guard
  doc + test.
- `4.2` — blocked on catalog modeling relation receivers + missing `ReturnKind` variants.
- near-miss `matches!` sites for `2.1` predicates (listed above) left for Sam.

**Deferred (larger / behavior-affecting / needs toolchains — each a focused session):**
`3.5` (arith-classifier adoption, behavior-affecting), `3.6` (monolith variant-split),
`4.3` + analyze/mod.rs squatter extractions (largest code-motion), `5` (Session facade —
thin dedup, dominated by per-entry-point error-handling variation), `6` (runtime manifest +
`6.1` CI matrix — CI-only, not locally verifiable), `7.3` (Python view helpers — behavior-
affecting), `7.4` (route-helper dedup — investigate-first), `7.5` (view_to_library motion).

Verification harness used: `emit_preview` snapshot of every target×fixture into a scratch
tree, `diff -r` against a baseline after each step (ruby target — not in emit_preview —
covered by `lowered_ruby_emit`'s 100 tests).

### Phase 1 (2026-07-18) — all four steps done, behavior-neutral

- **1.1** Deleted `src/query.rs` + its `pub mod`/`pub use` in lib.rs. Confirmed zero
  live refs (the `arel` `JoinKind` is a distinct type). Tests pass.
- **1.2** Added `crate::lower::residue_diagnostic(pass, construct, span, reason, message)`;
  the six residue-emitting passes delegate to it. The plan's suggested
  `residue(pass, expr, reason)` signature didn't fit — two passes key off a `MethodDef`
  span, not an `Expr`, and all six have distinct message text — so the shared core takes
  `construct`, `span`, and the pre-formatted `message` as params. Rendered text unchanged;
  removed now-unused `DiagnosticKind` imports from the six files. Emit-neutral.
- **1.3** `profile::Target`: judged **accidental drift**, not a deliberate subset (Go/
  Python/Elixir/etc. are present but have no deployment profile constructed either; the
  enum's own doc says it mirrors `src/emit/`). Added `Kotlin`, `Swift`, `CSharp` + a doc
  note distinguishing it from `project::BuildTarget`. Inert (never matched exhaustively).
- **1.4** Wrote `docs/env-gates.md`. Finding: `GO_V2`, `GO_V2_MODELS`, `RUST_V2`,
  `RUST_V2_LEGACY` are **vestigial** — their gates were removed when the new path became
  unconditional; they survive only in comments and are safe to delete. `ELIXIR_V1` is
  still live in `scripts/compare`. Fixed the `ROUNDHOUSE_<TARGET>_VIEW_THIN` mention in
  `docs/pipeline/emit.md` (matched no code → the real `<TARGET>_V2` convention).

### Phase 2 (2026-07-18)

- **2.1 `is_unknown()` (`Var | Untyped`)** — added to `impl Ty`; migrated 11 exact boolean
  `matches!` sites (ide.rs:2225; analyze/mod.rs 2454/2517/2577/2935; rust2/library.rs:697;
  swift/expr.rs:512; go2/library.rs 1290/1303/1304/1307). Emit-neutral, analyze+rbs pass.

  **Near-miss sites left untouched (for Sam)** — same `Var|Untyped` cluster + one extra
  variant; NOT folded into the predicate (would be a silent membership change):
  - `src/lsp.rs:621` — `Var | Untyped | Bottom` (+Bottom)
  - `src/emit/kotlin/library.rs:1164` — `Untyped | Var | Nil` (+Nil)
  - `src/emit/swift/library.rs:1199` — `Nil | Untyped | Var` (+Nil)
  - `src/emit/csharp/library.rs:1226` — `Untyped | Var | Nil` (+Nil)

  Also left as-is (match *arms*, not boolean `matches!` — converting to a guard would be a
  readability regression): rbs.rs:799, ide.rs:186, kotlin/ty.rs:80, crystal/ty.rs:70,
  swift/ty.rs:77, csharp/ty.rs:96, swift/library.rs:1282, kotlin/library.rs:1093.

- **2.1 `is_scalar()` (`Int | Float | Bool | Str | Sym`)** — added to `impl Ty`; migrated
  5 direct-`&Ty` boolean sites (crystal/expr.rs:131 — the `is_non_nilable_primitive` helper
  body now delegates; rust2/expr/send/coerce.rs:68; ty_coerce_insertion.rs 483/509/557).
  Emit-neutral, analyze passes.

  **Left for optional follow-up (equivalent-but-restructuring)** — `Some(<scalar set>)`
  matches! sites that would migrate via `.is_some_and(|t| t.is_scalar())` (provably
  equivalent, but restructures the Option handling rather than a 1:1 swap):
  rust2/expr/control.rs:561, rust2/expr/send/coerce.rs:235, coerce.rs:261.
  Left as match-arms (not boolean): kotlin/expr.rs:442, csharp/expr.rs:483, go2/expr.rs:1638.

- **2.1 `is_stringish()` (`Str | Sym`)** — added to `impl Ty`; migrated 7 direct-`&Ty`
  boolean sites (str_color.rs:589; rust2/expr/send/coerce.rs — cast_inner, both param_ty,
  and the `**key` hash-key check; ty_coerce_insertion.rs:455; go2/expr.rs:2114). The many
  remaining `Ty::Str | Ty::Sym` occurrences are match-*arms* (rendering a type string) or
  `Some(Ty::Str | Ty::Sym)` Option-wrapped predicates — left as-is. Emit-neutral, analyze
  passes.

  **Phase 2.1 complete** — all four predicates (`is_unknown`, `is_open`, `is_scalar`,
  `is_stringish`) landed. Total 34 exact boolean sites migrated across 4 commits.

- **2.2 union_of** — **the two implementations are semantically DIFFERENT; NOT unified**
  (per the plan's "if they differ" branch). Finding:
  - `analyze::body::union_of(a: Ty, b: Ty)` is a *normalizing binary lattice join*:
    Bottom-drop, pointwise Hash/Array container merge, flatten + dedup nested unions,
    canonical variant sort. `union_many` folds a `Vec` through it. Its `==`-based fixpoint
    convergence and the lattice-law tests depend on the canonicalization.
  - `rbs::union_of(variants: Vec<Ty>)` was a *bare non-normalizing constructor* — return
    the sole variant, else `Ty::Union { variants }` verbatim. It's the RBS **parse**
    direction and must reproduce exactly what the author wrote (round-trip printing +
    not silently rewriting a declared signature). Normalizing here would be a behavior bug.

    Actions taken (all behavior-neutral): renamed `rbs::union_of` → **`union_or_single`**
    with a doc stating it is deliberately not the lattice join (kills the misleading name
    collision); added a cross-ref note. Did NOT mass-move the 49-call-site lattice
    `union_of`/`union_many`/`push_union_variants` onto `impl Ty` — that move was contingent
    on the two being identical, and the churn/risk isn't justified given they diverge; they
    keep their home in `analyze::body`.
  - **Pure code motion done**: moved `peel_nilable`, `strip_nil`, `canonicalize_variants`
    to `impl Ty` in `ty.rs` (each had exactly one caller). `ide.rs`'s `can_be_nil`/
    `nil_verdict` left alone as instructed. Lattice-law + rbs + analyze tests pass, emit
    byte-identical.

### Phase 3 (2026-07-18)

- **3.1 `split_trailing_kwargs`** — new `src/emit/shared/args.rs`:
  `split_trailing_kwargs(args) -> (&[Expr], Option<&[(Expr, Expr)]>)` on the exact
  `split_last` + `Hash { kwargs: true }` decision. Adopted in csharp, swift, kotlin,
  elixir2 (`emit_call_args` / `unpack_kwargs_with`), one commit each, emit-neutral.
  **go2 excluded** — its trailing-Hash handling is per-callee (`try_expand_render_kwargs`
  etc., indexing `args[1]` and matching `Hash { .. }` with any `kwargs` flag), a different
  shape that the helper doesn't model; left as-is.

- **3.2 interp walker** — new `src/emit/shared/interp.rs`: `render(parts, delims,
  emit_expr, escape)` with an `InterpDelims { open_quote, close_quote, expr_open,
  expr_close }` config. Adopted in crystal, ruby (byte-identical pair), kotlin, csharp,
  elixir2. Escape stays per-target (passed as a closure/fn ref) per 3.3.
  **swift excluded** — its Expr arm wraps optionals in `RhString.s(...)` (type-directed,
  not a plain delimiter). **go2 excluded** — `emit_interp_appends` appends to a variable
  rather than building a quoted literal. Both keep their own walker.

- **3.3** No action (a "do NOT unify `escape_str`" directive) — honored throughout 3.2:
  every migrated target keeps its own escape fn/closure.

- **3.4 binop dispatch** — new `src/emit/shared/ops.rs`: `classify_binop(method) ->
  BinopCase { NativeInfix(op), Append, NotBinop }`. Adopted in swift/kotlin/csharp — the
  byte-identical infix-operator `matches!` + `<<`/`push` append pair becomes one `match`;
  each target still renders its own append (`.append`/`.add`/`.Add`). Classified on
  `method` alone (arity + receiver stay enforced by the enclosing `(Some(r), 1)` guard,
  which the rest of each block needs anyway). go2's append (`go2/expr.rs:1220`, inline
  `&& args.len() == 1`) left as-is — not in the shared block shape. Emit byte-identical.

- **3.5 / 3.6 DEFERRED** — 3.5 (adopt `classify_add/sub/mul` in the 4 inline targets) is
  explicitly behavior-affecting: it needs per-change correction analysis and per-target
  toolchain runs (several toolchains aren't installed here). 3.6 (variant-split the monolith
  emitters) is optional pure code motion. Both left for a later pass; proceeding to Phase 4
  (highest value) per the plan's recommended order.

### Phase 4 (2026-07-18)

- **4.1 — the premise is FALSE; NOT a pure duplication.** `is_query_builder_method` does
  NOT duplicate the catalog's `chain` field. Measured: all 13 methods it recognizes have a
  Builder/Terminal `AR_CATALOG` entry, but the catalog's Builder/Terminal set has **26
  more** (`find`, `find_by`, `count`, `sum`, `average`, `maximum`, `minimum`, `exists?`,
  `pick`, `take`, `page`, `async_count`, `having`, `preload`, `eager_load`,
  `left_outer_joins`, `references`, `reorder`, `rewhere`, `unscope`, `readonly`, `reselect`,
  `extending`, `merge`, …). The predicate is a deliberately *narrower curation* — the
  relation-chain surface the lowerer walks (`chain.rs`, `controller/send.rs`'s `QueryChain`
  classification). Deriving it from `chain ∈ {Builder, Terminal}` would reclassify those 26
  Sends as query chains — a real lowering behavior change, not a refactor.
  **Action (behavior-neutral):** left the predicate as-is; added a doc comment warning
  against the trap and a unit test (`query_builder_methods_are_all_cataloged`) guarding the
  one true invariant (subset ⊆ catalog Builder/Terminal), so a catalog rename can't silently
  desync it. Did not add a marker field to `CatalogedMethod` — that would re-encode the
  hand-curated list across ~80 struct literals for a single consumer; low value, deferred
  as a design call for Sam.

- **4.2 — BLOCKED on catalog modeling; NOT done.** The suggested cluster (`pluck`, `pick`,
  `ids`) can't be routed through the catalog today:
  - The send.rs arms for `pluck`/`pick`/`ids`/`count`/`exists?`/… at send.rs:895–943 live
    in the **relation-receiver** branch (dispatch on a `Ty::Array<elem>` receiver, e.g.
    `Model.where(...).pluck`). The catalog's `ReceiverContext` is Class/Instance only — it
    explicitly does NOT model Relation receivers yet (the enum comment says they "will join
    as the analyzer gains Relation<T>"). So there is no catalog entry in the matching
    receiver context to look up.
  - `pluck`/`pick`/`sum`/`average` have `return_kind: None` in the catalog (their shapes —
    `Array<Untyped>`, `Untyped` — aren't even expressible in the current `ReturnKind` enum,
    which has no `ArrayOfUntyped`/`Untyped` variant). So the catalog can't reproduce the
    arm's computed type.
  - The Class-receiver catalog methods that DO have return_kinds (`all`, `first`, `last`,
    `count`, `exists?`) are already consumed by `with_adapter` (analyze/mod.rs:114–137) into
    `class_methods`; send.rs's string arms are the relation/contextual fallback the registry
    doesn't cover. There's no redundant *class-receiver* arm to delete.

    Doing 4.2 requires first extending the catalog (`ReceiverContext::Relation`, new
    `ReturnKind` variants) — feature work beyond a behavior-neutral refactor. Left for Sam
    as a design task. No code change.

- **4.3 + analyze/mod.rs squatter extractions DEFERRED** — `with_adapter` is
  analyze/mod.rs:89–1585 (per-model loop 101–404 with shared `self_ty`/`instantiate` state,
  then ~1180 lines of post-loop ActiveModel/form-builder/routes/stdlib registration). The
  extraction is pure code motion in principle but the single largest, most interleaved
  mechanical task in the plan; it (and the render/effects/diagnose free-fn extractions)
  warrant a dedicated focused session with careful incremental verification rather than the
  tail of this one. Not attempted here.

### Phase 7 (2026-07-18)

- **7.2 params struct** — added `LowerControllerOptions<'a>` (`#[derive(Default)]`, all
  feature fields default to off) to `controller_to_library`. The 8-positional-param
  `lower_controllers_with_arel_views_assocs_and_routes` now takes `(controllers, extras,
  opts)`; body unchanged (destructures opts at the top). The wrapper chain is KEPT — every
  intermediate arity has external callers (tests, emit/\*, dump_ir), so the plan's collapse
  condition isn't met. Cryptic trailing `&[], None, false` at the two direct call sites
  (the `_and_assocs` wrapper + emit/ruby.rs) become named fields. Emit byte-identical,
  ruby + controller tests pass.

- **7.1 pass ordering** — added `const POST_ANALYZE_PASS_ORDER: &[(&str, &[&str])]` in
  `lower/mod.rs` as the single authority for the post-analyze pipeline's order + `runs_after`
  constraints (the only real one: `duration` after `send_static_dispatch`; `typed_store`'s
  ordering note is a *different* pipeline — model synthesis — and out of scope). Soundness
  checked by a `debug_assert!` on pipeline entry + two unit tests
  (`post_analyze_pass_order_is_sound_topologically`, `..._names_are_unique`). Repointed the
  scattered prose ("AFTER send_dispatch, by contract" in mod.rs/duration.rs/send_dispatch.rs)
  at the const. Dropped the `fn` from the entry tuple the plan sketched — the passes have
  heterogeneous signatures (`Vec<Diagnostic>` vs `()`, some take `registry`), so a uniform
  fn table needs wrappers for no benefit; the list's job is ordering, not dispatch. Emit
  byte-identical.
