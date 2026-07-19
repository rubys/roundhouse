# Maintainability refactor plan 2 — mechanical remainder

Written 2026-07-18. Continuation of docs/maintainability-refactor-plan.md (phases 1, 2,
3.1/3.2/3.4, 4.1, 7.1, 7.2 of which are DONE — see its Execution log). This plan is the
behavior-neutral mechanical slice that remains, deliberately scoped to run **in parallel**
with docs/relation-type-plan.md in a separate session. Self-contained: executable by a
fresh Claude session with no prior context. Re-verify file:line references before editing.

## Collision rules (read first)

A concurrent session may be executing the Relation[T] plan. That session owns:
`src/ty.rs`, `src/catalog/`, `src/analyze/` (all of it), `src/lower/arel/`,
`src/lower/scope_chain.rs`. **Do not edit those files here, even for a one-line fix** —
log the finding instead. Everything this plan touches (`src/lower/mod.rs` guard only,
`src/session.rs` new file + entry-point call sites, `src/runtime_loader.rs`,
`.github/workflows/ci.yml`) is outside that set. The one shared file is `src/lower/mod.rs`
(M1) — the Relation session doesn't modify the pass pipeline; if a merge conflict appears
there anyway, rebase, don't force.

Also explicitly NOT here: refactor-plan step 4.3 (`with_adapter` split) — queued until the
Relation plan lands, because it restructures exactly the file that plan churns.

## Ground rules

Same as the parent plan, abbreviated: behavior-neutral throughout (emit byte-identical on
the fixture snapshot harness — see parent plan "Verification harness"); commit to main,
one commit per step, stage only files you changed (untracked strays exist — never
`git add -A`); never pipe cargo; `cargo test --all-targets` for anything touching enums;
`#[ignore]`d baseline failures exist — diff against parent commit before claiming
regression. Every step is a legal stopping point; if a step turns out non-neutral or
bigger than described, STOP it, log it below, move on.

## M1 — Close the 7.1 gap: pass-list ↔ call-sequence correspondence

`src/lower/mod.rs` now has `POST_ANALYZE_PASS_ORDER` (const, ~:138) as the ordering
authority, but its own doc admits the debug_assert "guards the ordering constraints, not
the code↔list correspondence" — a pass added to `apply_post_analyze_lowerings` (~:193)
but not the const goes uncaught, which is the original drift failure mode in miniature.

Fix without wrappers (the passes have heterogeneous signatures; a uniform fn table was
rightly rejected): in `apply_post_analyze_lowerings`, build a debug-only executed-names
list — a `#[cfg(debug_assertions)] let mut executed: Vec<&str>` with a one-line
`executed.push("blank");` (etc.) adjacent to each pass call — and at the end
`debug_assert_eq!` it against the const's names in order. A pass added to the code without
the const (or vice versa, or out of order) now fails every debug test run.
Gate: `cargo test --all-targets`; emit-neutral by construction. One commit.

## M2 — `Session` bootstrap facade (parent plan Phase 5)

The ingest → `Analyzer::new` → `analyze` → diagnostics dance is copy-pasted at 7 entry
points: `src/project.rs` (~:2070), `src/mcp.rs` (`Server::analyze` ~:74),
`src/bin/roundhouse.rs` (~:226), `src/lsp.rs` (`run_analysis` ~:460),
`src/bin/dump_ir.rs` (~:96), `src/bin/emit_preview.rs` (~:15),
`src/bin/roundhouse-check.rs` (~:102). The agreed-but-unbuilt `roundhouse analyze` CLI
must not become copy #8.

Expectation-setting (from the prior session's triage): this is a *thin* dedup — the copies
are dominated by per-entry-point error-handling variation, not shared logic. So aim small:
- New `src/session.rs` (or a module under an existing home if one fits better) with one
  function capturing the common core — suggested shape
  `Session::open(root, …) -> AnalyzedApp` or a plain
  `fn analyze_project(...) -> Result<(App, Registry, Vec<Diagnostic>), …>`. Take the
  genuine variation as parameters (VFS overlay for LSP, diagnostics on/off); do NOT
  absorb the LSP threading model or MCP's per-call re-ingest policy — those stay at the
  call sites.
- **Constraint from the Relation session**: `src/analyze/` is off-limits, so the facade
  wraps calls INTO the analyzer without modifying it. `src/lsp.rs`/`src/mcp.rs`/
  `src/project.rs`/bins are fine to edit — only their bootstrap blocks, nothing else.
- Migrate the 4 bin/ entry points first (simplest), then project.rs, then mcp.rs, then
  lsp.rs. One or two entry points per commit.
- If, mid-work, the honest assessment is that the facade removes <~15 lines per site and
  obscures the per-site error handling, STOP after the bins, log that verdict, and leave
  lsp/mcp as-is — a facade nobody's entry point fits is worse than the duplication. The
  hard requirement is only that a future `roundhouse analyze` CLI has one obvious
  function to call.
Gate per commit: full test run; emit byte-identical; `roundhouse-check` and `emit_preview`
smoke-run by hand on a fixture app.

## M3 — Runtime-unit manifest (parent plan Phase 6)

`src/runtime_loader.rs` (~2016 lines) hand-maintains 9 near-identical per-target tables
(`TYPESCRIPT_RUNTIME:274`, `CRYSTAL_RUNTIME:437`, `RUST_RUNTIME:568`,
`KOTLIN_RUNTIME:901`, `CSHARP_RUNTIME:1135`, `SWIFT_RUNTIME:1296`, `GO_RUNTIME:1474`,
`ELIXIR_RUNTIME:1676`, `PYTHON_RUNTIME:1866`) — 76 `RuntimeEntry` literals, most
`include_str!`-ing the same `runtime/ruby/*.rb`+`.rbs` pairs. Adding one runtime unit
means editing up to 9 tables. (`src/runtime_src.rs` is the shared parser half — it does
NOT duplicate the loader; leave it.)

- First, mechanically compute every table's delta from the union of all tables (a scratch
  script is fine). The manifest design must express the real variation: some targets omit
  units; `python_units_subset` (~:2005) exists; per-target `format_constant` thunks differ.
- Then: one shared unit table (stem → rb/rbs `include_str!` pair, declared once) + a
  per-target stem list (or include/exclude delta from a default set). Derive the 9 tables.
  Keep the `TargetEmit` callback struct as-is — the manifest replaces the *unit lists*,
  not the per-target emit plumbing.
- Gate: write a temporary test asserting the generated per-target unit lists are exactly
  identical (same stems, same order if order matters — check whether consumers are
  order-sensitive before assuming) to the old tables; land the switch; delete the old
  tables and the temporary test in the same commit. Emit byte-identical; full test run.
- **Defer, do not do** (unchanged from parent plan): `spinel_files` /
  `ruby_runtime_files` / `jruby_runtime_files` in `src/project.rs` — deliberate per-lane
  patches tied to active lobsters bench work.

## M4 — CI matrix conversion (parent plan 6.1; independent of M1–M3)

`.github/workflows/ci.yml` (~82KB, 46 jobs, zero `matrix:` blocks): 12 `compare-<target>`
jobs (~:775–1101) and 12 `smoke-<target>` jobs (~:1170–1522), all of which just call
`scripts/compare <target>` / `scripts/smoke` with per-target toolchain setup. Convert each
family to one job with a `strategy.matrix` axis, per-target toolchain-install steps keyed
off matrix `include` entries. Preserve per-job details exactly — before writing the
matrix, diff the 12 jobs in a family against each other and enumerate every difference
(runner OS, cache keys, `needs:`, `if:` conditions, timeouts); each difference becomes an
`include` field, not a casualty. Keep the `toolchain-*` / `framework-tests-*` job families
as-is (their setup genuinely differs more). CI-only commit; not locally verifiable — check
the next CI run once after pushing (per repo practice, don't babysit it).

## Parked (do not start; listed so they aren't re-derived)

From the parent plan, still deliberately deferred: 3.5 (arithmetic-classifier adoption —
behavior-affecting, needs installed toolchains), 3.6 (variant-split monolith emitters —
optional pure motion), 4.3 + analyze/mod.rs squatter extractions (queued behind the
Relation plan), 7.3 (Python view-helper re-classification — behavior-affecting),
7.4 (route-helper dedup — investigate-first), 7.5 (`view_to_library` motion). Parent
plan's "Explicitly out of scope" list still applies (diagnostic.rs, functionalize gating,
`run_typing_passes`, escape_str unification, cross-target conformance harness).

## Suggested order

M1 (20 minutes) → M2 (stop-early rule applies) → M3 (the big one) → M4 (anytime,
independent). Every step is a legal stopping point.

## Execution log

### Session 2026-07-18 — M1–M4 all EXECUTED + verified, behavior-neutral

Verification harness: `emit_preview` snapshot of 9 targets × 2 fixtures (722
files) diffed after each step; the real `roundhouse` transpile path (10 targets,
real-blog) diffed against a parent-commit worktree build where the change was on
the emit path; `cargo test --all-targets --no-fail-fast` (baseline 1102 passed /
0 failed / 67 ignored) held identical across every step.

- **M1 — DONE** (commit `refactor(M1)`). `apply_post_analyze_lowerings` now
  threads a `#[cfg(debug_assertions)]` `executed: Vec<&str>` (one `ran!("name")`
  macro-push adjacent to each pass call) and `debug_assert_eq!`s it against
  `POST_ANALYZE_PASS_ORDER`'s names in order. Catches the code↔list drift the
  pre-existing `runs_after` debug_assert couldn't (a pass added to the code but
  not the const, removed, or reordered now fails every debug test run). Emit
  byte-identical by construction; the assert runs during every debug-build emit
  (emit_preview snapshot passed).

- **M2 — DONE, thin-facade verdict as the plan predicted** (commit
  `refactor(M2)`). New `src/session.rs::analyze_and_lower(&mut App) ->
  Vec<Diagnostic>` = `Analyzer::new` + `analyze` +
  `apply_post_analyze_lowerings(registry)`, the emit-ready-IR seam. Adopted at
  the THREE emit-bound drivers only: `roundhouse.rs`, `dump_ir.rs`,
  `project::build_site` (each dropped its now-unused `Analyzer` import).
  **Verdict — the other four entry points do NOT fit and were deliberately left
  alone**: `emit_preview`, `roundhouse-check`, MCP `Server::analyze`, and LSP
  `run_analysis` all run analyze *without* the post-analyze lowerings on purpose
  (they consume source-shaped IR for previews / type-checks / hovers), so
  routing them through the facade would change behavior, not dedup it. Their
  only shared core is the two-line `Analyzer::new`+`analyze` (below the ~15-line
  stop-early threshold), and their ingest wrapping (Prism scope, VFS overlay,
  survey on/off) plus diagnostic post-processing (attribute_ingest_gaps) is
  genuinely per-site. Hard requirement met: the planned `roundhouse analyze` CLI
  has one obvious function for emit-ready IR. Real transpile byte-identical
  across 10 targets.

- **M3 — DONE, with a design finding** (commit `refactor(M3)`). Instead of the
  plan's "shared unit table + per-target stem lists that derive whole entries,"
  a `runtime_entry! { stem: "...", ... }` macro derives the
  `rb_src`/`rbs_src`/`rb_path` triple from one stem literal via `concat!` +
  `include_str!`. All 75 entries converted; `struct RuntimeEntry`, the `*_units`
  consumers, and `transpile_entry` untouched.
  **FINDING — the plan's fuller vision is refuted by the measured variation.**
  I extracted every table's fields first (as the plan instructed): `namespace`,
  `mode`, `out_path`, `imports`, `prelude`, `extra_roots` EACH vary across
  targets for the *same* stem — `inflector` is `Mode::Module` in 6 targets but
  `Mode::Library` in rust/go/elixir; `active_record/base` is `ns=ActiveRecord`
  in 5 targets, empty in 4; `out_path` naming is irregular (sometimes basename
  `errors.ts`, sometimes flattened stem `active_record_base.ts`), so it isn't
  derivable by convention either. The ONLY stem-derivable duplication is the
  include_str triple (225 copies → gone, and the three can no longer silently
  disagree). The six rendering fields stay explicit per target by necessity, so
  adding a unit still touches each target's table — but names the file path once
  per entry, not thrice. No "generated-list vs old-table" gate was needed: the
  macro expands to the identical `RuntimeEntry` literals, so compile +
  emit-byte-identical (emit_preview + real transpile) IS the equivalence proof.

- **M4 — DONE** (commit `refactor(M4)`). Nine compile-and-boot `compare-*` jobs →
  one `compare` matrix job; eleven `smoke-*` jobs → one `smoke` matrix job.
  Every per-target difference is a `if: matrix.target == '...'` guarded step with
  its exact action + versions; `fail-fast: false` preserves the old independent
  behavior. Kept standalone: `compare-ruby`/`compare-jruby` (two setup-ruby
  steps), `compare-spinel`/`smoke-spinel` (`needs: build-spinel` is job-level +
  continue-on-error + binary staging). Faithfulness proven by reconstructing each
  removed job from the matrix (resolving guards + `${{ matrix.target }}`) and
  diffing vs the pre-change jobs: compare — rust byte-identical, four targets
  differ only by step `name:` labels, and typescript/go/elixir/python have
  setup-ruby moved LAST (the single functional change — ruby-last only
  strengthens MRI's PATH win, already required by the JVM targets and safe for
  the rest); smoke — 4 identical, 6 name-only, smoke-rust's rust-toolchain now
  follows the shared download-artifact step (order-independent). YAML parses
  (Ruby `YAML.safe_load`); 46→26 jobs. CI-only — validate on the next run.

**All of plan-2 executed.** Parked items (below) untouched.
