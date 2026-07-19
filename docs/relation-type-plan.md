# Relation[T] plan — analysis-side relation typing, erasure-first

Written 2026-07-18. Self-contained: executable by a fresh Claude session with no prior
context. Re-verify all file:line references with grep before editing — they drift.

## Why this work (strategic anchor)

Three active threads converge on one modeling gap — the analyzer has no relation type
(relations are approximated as `Ty::Array { elem }`; see `src/analyze/body/send.rs:866`
"Relation chain methods preserve Array<Self>"):

1. **Lobsters deployment bench** (the proving lane): `/t/tag` is past its ArgumentError and
   now blocked on the assoc-returns-Relation query gap (`tag.stories` + further chaining) —
   same family as `/s/:short_id`. This plan's phase R5 is that fix.
2. **Ufuk's Relation[T] challenge (RubyConf)**: (a) relation→class delegation
   (`Story.recent.for_user(u)` — scopes callable on relations), (b) scope body-return
   inference. Phases R3/R4.
3. **Catalog registry unification**: refactor step 4.2 (docs/maintainability-refactor-plan.md)
   is blocked on `ReceiverContext::Relation` + missing `ReturnKind` variants. Phase R2/R4.

## Design decisions — SETTLED, do not relitigate

These were decided with Sam 2026-07-18:

1. **`Relation[T]` is a first-class analysis-time type, not a runtime object.** Roundhouse
   specializes queries: statically-visible chains fold at their terminal into direct SQL
   (`src/lower/arel/build.rs try_build_arel`) and results are typed `Array[T]` / `T?`. That
   stays. The relation type exists so the *analyzer* can reason across method boundaries;
   it is erased by specialization in emitted code. Futamura framing: arel/relation
   machinery is interpreter machinery and must not survive into the specialized program
   except where the program is irreducibly dynamic.
2. **Representation: a dedicated `Ty::Relation { of: ClassId }` variant** (not
   `Ty::Class { id, args }`). Precedent: `Ty::Time`'s doc comment in `src/ty.rs` — a
   first-class variant forces every exhaustive match to make an explicit per-target
   choice; a target that meets a reachable `Relation` at emit must route to an
   `Unsupported` diagnostic, never silently degrade.
3. **Three emit tiers**: (1) fully-static chain (after splicing through scopes/assocs) →
   direct SQL, `Array[T]` result — the default and the goal; (2) branch-dependent but
   enumerable → future work, NOT this plan; (3) irreducibly dynamic (predicates accumulated
   in loops) → **residue diagnostic, NOT a runtime relation class**. Do not emit
   per-model relation classes; the ledger (how many tier-3 sites exist in
   lobsters/Mastodon) is the deliverable that would justify them later.
4. **Staging is additive, not big-switch.** Inline chains that type as `Array` and fold
   today keep doing so untouched. `Ty::Relation` is introduced only where `Array` fails:
   scope return types, class-method-returning-relation bodies, association-returns-relation.
   A chain is `Relation`-typed throughout iff it *starts* from such a call; chains starting
   at `Model.where(...)` stay `Array`-typed as today. Mixing within one chain is coherent:
   chain methods preserve the receiver's representation. Converging the two representations
   later is phase R7 — a decision for Sam, not this session.
5. **Terminal method result types must not change.** `first` → `T?`, `count` → `Int`,
   `pluck` → `Array[...]`, `to_a`/iteration → `Array[T]` — downstream view typing depends
   on these. Whatever the chain's intermediate representation, terminals produce exactly
   the types they produce today.

## Ground rules

- Commit to main, one commit per numbered step; stage only files you changed (repo has
  untracked strays — never `git add -A`).
- This is feature work: analysis output is *expected* to improve (fewer Var/Untyped, new
  chains folding). The gates are: (a) no regression — every fixture chain that folded to
  SQL before still folds to the same SQL; (b) `cargo test --all-targets` green, with test
  expectation updates only where the new type is strictly more precise; (c) emit-diff
  reviewed per step — diffs must be explainable as intended improvements.
- Emit-diff harness: snapshot `emit_preview` output for all targets × fixtures before
  starting; `diff -r` after each step. **CAVEAT (learned in plan-2 M2): `emit_preview`
  runs analyze WITHOUT the post-analyze lowerings** — it previews source-shaped IR — so
  for this plan (which changes analysis and lowering) it under-covers. Use the stronger
  harness from plan-2's execution log: ALSO diff the real `roundhouse` transpile output
  (all targets, real-blog fixture) against a parent-commit worktree build, per step.
- COORDINATION: a parallel session may be executing docs/maintainability-refactor-plan-2.md
  (Session facade, runtime manifest, CI). That plan is forbidden from touching this plan's
  files. This session owns: `src/ty.rs`, `src/catalog/`, `src/analyze/`, `src/lower/arel/`,
  `src/lower/scope_chain.rs`, model-registry metadata. Do not start refactor-plan step 4.3
  (`with_adapter` split) from here either — it's queued until after this plan lands.
- Known baseline: some `#[ignore]`d tests fail on main by design. Diff against parent
  commit before claiming regression.

## Phases

### R1 — `Ty::Relation { of: ClassId }` variant (neutral: unreachable)
Add the variant to `src/ty.rs` with a doc comment in the house style (model it on
`Ty::Time`'s: why first-class, what targets must do on reachability — Unsupported
diagnostic, never silent). Follow the exhaustive-match fallout everywhere
(`cargo test --all-targets` — enum-variant rule):
- Lattice: `union_of`/`union_many`/`canonicalize_variants` (`src/analyze/body/mod.rs`,
  plus the helpers now on `impl Ty`) — `Relation{of: A} ∪ Relation{of: A}` = itself;
  differing `of` → `Union` (do not invent subtyping across models here).
- The `is_*` predicates on `impl Ty` are unaffected (Relation is none of
  unknown/open/scalar/stringish).
- RBS rendering (`src/rbs.rs`), ide/lsp type rendering (`render_ty`), and each emitter's
  ty-mapping module (`emit/*/ty.rs`): render as `Relation[T]` in RBS/IDE output; in
  emitters, route to the Unsupported-diagnostic path (should be unreachable this phase).
Gate: emit byte-identical (variant is constructed nowhere yet). Commit.

### R2 — Catalog groundwork (neutral: unconsumed)
`src/catalog/mod.rs` already documents this future: comments at :95-96 ("Relation and
Association receiver contexts will join…"), :136-138, :385, :986.
- Add `ReceiverContext::Relation` (and decide, with a doc note, whether Association folds
  into it or stays future — recommend fold in: an association read *is* a relation whose
  base predicate is the FK match).
- Extend `ReturnKind` so the relation-branch arms in `send.rs:866-943` become expressible:
  needs at minimum element-nilable (`first`/`take`), array-of-element (`to_a`), relation
  (builders), plus the shapes noted blocked in refactor-plan 4.1/4.2 findings
  (`Array<Untyped>` for `pluck`, `Untyped` for `pick`). Name them for what they are, not
  per-method.
- Add catalog entries under the Relation context for the chain surface (`where`, `order`,
  `limit`, `not`, …) and terminals (`first`, `count`, `exists?`, `pluck`, …). Populate
  from what `send.rs`'s relation branch actually handles — the arms are the spec.
- CAUTION from refactor finding 4.1: `is_query_builder_method` (`catalog/mod.rs:885`,
  guarded by test `query_builder_methods_are_all_cataloged`) is a deliberately narrower
  curation. Adding Relation-context entries must not change that predicate; keep its
  guard test green.
Gate: emit byte-identical (nothing consumes the new context yet). Commit.

### R3 — Scope and class-method return inference (first behavior change)
Ufuk gap (b) + the `Relation` type's first producers.
- A model class-method or `scope` whose body is/ends in a query-builder chain infers
  return `Ty::Relation { of: Self }`. Self-substitution at inclusion: a scope defined in a
  concern included by N models instantiates `of` per includer — reuse the existing
  `instantiate`/`self_ty` machinery in `Analyzer::with_adapter`
  (`src/analyze/mod.rs`, per-model loop ~:101-404).
- Scope *declarations* (`scope :recent, -> { … }`) likely lower through
  `src/lower/scope_chain.rs` — read it first; it may already hold partial machinery.
- Calling a scope on the class (`Story.recent`) now types as `Relation { of: Story }`.
Gate: analyze tests (update expectations where a `Var`/`Untyped`/`Array` became
`Relation` — each update is evidence of the improvement, list them in the commit message).
Emit diff: chains *consuming* scope results may newly fold or newly hit the R1 diagnostic;
inspect each. If a previously-working fixture path regresses to a diagnostic, the chain
handling in R4 is needed first — reorder locally rather than forcing. Commit per coherent
slice.

### R4 — Dispatch on Relation receivers (Ufuk gap (a) + refactor 4.2 realized)
In `src/analyze/body/send.rs`, a send whose receiver types `Relation { of }`:
1. Chain/terminal builtins: resolve via the R2 catalog entries (this finally does
   refactor-plan 4.2 — the string arms for the relation branch migrate to catalog
   lookups; keep arms whose types are genuinely arg-dependent, per that plan's guidance).
   Builders preserve `Relation { of }`; terminals return the SAME types the Array-branch
   arms return today (settled decision #5).
2. Class-side delegation: if the method isn't a builtin, look up `of`'s class-side surface
   (scopes + class methods returning relations) in the class registry — `Story.recent
   .for_user(u)` resolves `for_user` against Story. Only delegate the relation-returning
   class-side surface; do not forward arbitrary class methods (that would be Rails'
   method_missing semantics — statically resolve or diagnose).
Also: catalog comment `:385` says builder effects change "once Relation<T> typing lands" —
move query-execution effects to the terminal step for Relation-typed chains ONLY if the
effects tests make it cheap; otherwise log it as a follow-on rather than churning the
effects system mid-plan.
Gate: analyze tests + emit diff reviewed. Commit per slice.

### R5 — Fold through the boundary: predicate contributions + arel splice (the payoff)
The self-describing-IR move: when the lowerer knows a scope's or association's predicate,
the IR records it (same pattern as the model-association registry).
- Scope definitions: record the predicate/ordering contribution of the body chain in
  model-registry metadata at lowering time.
- Association reads returning relations (`tag.stories`): contribution = the FK predicate,
  already derivable from the association registry.
- `try_build_arel` (`src/lower/arel/build.rs`) learns to resolve a `Relation`-typed call
  in receiver position by splicing the callee's recorded contribution into the chain it's
  building, then folding at the terminal exactly as today → one SQL statement, result
  `Array[T]`, relation type erased.
Deliverable: **lobsters `/t/tag` renders (HTTP 200)** via the deployment-bench route checks
used for the front-page work (see commits around 51f676bc / c01de3c7 for the harness; the
route was previously failing on the assoc-returns-Relation gap). Also re-check
`/s/:short_id` — same gap family.
Gate: blog fixtures — every previously-folded chain emits the SAME SQL (string-compare
the emitted queries); lobsters route check; full test run. Commit.

### R6 — Tier-3 residue ledger (small, closes the loop)
Chains the splice cannot ground (dynamic composition — predicates in loops, relations
escaping into un-analyzable positions) get a structured diagnostic: reuse
`DiagnosticKind::Unsupported { construct: "dynamic_relation", … }` or `LowerResidue` via
`crate::lower::residue_diagnostic` — one mechanism, greppable construct id. Run the
counts on blog (expect 0) and lobsters (expect a small handful — search is the likely
site) and record them in the Execution log below. **Do not build a runtime query builder
or per-model relation classes** — the counts are the input to that decision, which is
Sam's.

### R7 — NOT for this session: decision points for Sam
Record in the log, do nothing:
- Converge inline-chain typing (`Model.where(...)` chains currently `Array`-typed) onto
  `Ty::Relation` — kills the dual representation; gate would be emit-neutrality.
- Tier-2 enumerable branch shapes (finite branch combinations → N prepared statements,
  runtime flag select — aligns with the param-binds shape cache).
- Effects-to-terminal completion if skipped in R4.
- Whether Association context stays folded into Relation (per R2 choice).

## Out of scope
Runtime relation classes (settled decision #3). Refactor-plan 4.3 / `with_adapter` split
(queued behind this plan). Any file owned by maintainability-refactor-plan-2.md. Tier-2
enumeration. The kwarg-forwarding strict-target gap (separate thread).

## Execution log

Executed 2026-07-18, one session. Commits `33ebdd74..bb94d85b` on main. Harness: plan-2's
strong form — 9-target × 2-fixture `emit_preview` snapshot PLUS 12-target real
`roundhouse` transpile of real-blog, `diff -r` against a pre-plan baseline after every
phase.

### R1 — `33ebdd74` (byte-identical ✓, suite green ✓)
Variant + doc comment modeled on `Ty::Time`. Nine exhaustive-match sites (8 emitter ty
renderers + ide.rs) plus two catch-all sites that would have silently absorbed it
(typescript's `_ => "any"`, rbs.rs print_ty's `_ => "untyped"`). Emitters route to a new
`emit::diagnostics::unsupported_relation_ty` (Error severity + non-compiling placeholder,
the `unsupported_time_ty` pattern); consumer-facing renderers (ide, rbs print_ty) show
`Relation[T]` (print_ty documents the Time-style round-trip asymmetry). Lattice needed no
code — structural equality + Debug-sort already give same-of ⊔ same-of = itself and
differing-of → flat Union; locked by adding Relation to the lattice-law test universe + a
dedicated no-cross-model-subtyping test.

### R2 — `b75ba844` (byte-identical ✓)
`ReceiverContext::Relation` (Association FOLDED IN, per the recommendation — doc note on
the variant says when to revisit). `ReturnKind` + `RelationOfSelf`/`ArrayOfInt`/
`ArrayOfUntyped`/`Untyped`. ~70 Relation-context entries; the send.rs arms were the spec.
**Neutrality trap found and handled**: the two receiver-blind NAME-based consumers
(`SqliteAdapter::classify_ar_method`, `Analyzer::is_builder_chain`) search `lookup_any`
across contexts — new Relation-only names (`to_a`, `page`, `merge`, …) would have shifted
effect classification. Both filter the Relation context out until effects-to-terminal
lands (see R7). Guard test `relation_context_mirrors_send_rs_relation_branch` pins the
surface. `is_query_builder_method` untouched, its guard test green.

### R4a (machinery, reordered before R3 per the plan's own guidance) — `32c8d5ca`
(byte-identical ✓)
Dispatch arm for `Ty::Relation`: catalog builtins via shared `instantiate_return_kind`
(extracted from the with_adapter closure; Self = element model under Relation context) →
class-side delegation of the relation-returning surface only (Relation-typed entries, or
Array-of-model entries re-wrapped to preserve the receiver's representation; NO arbitrary
class-method forwarding) → `array_method` Enumerable fallback. `block_params_for` +
`multiassign_target_ty` treat Relation as Array<of>. Catalog `arel` entry corrected to
`ClassRef(Arel::SelectManager)` — the live Array-representation behavior is the send.rs
intercept, not the dead Untyped arm.

### R3 — `a5c5efc4` (byte-identical ✓ — fixtures declare scopes but consume none at
emit-visible positions)
Producers: scope seeds and class-method harvest flip to `Relation { of: Self }` when the
body TAIL is a builder chain. New classifier `body_tail_yields_relation` — the *typing*
twin of `scope_chain::mentions_bare_chain_start` (mention-anywhere qualifies for `__rel`
threading; tail-position-only qualifies for the return type). Conservative: terminal
tails, block-taking hops, cross-model constant roots keep the legacy `Array<Self>` seed.
Class-method flip lives in `harvest_returns_to_registry` (the relation type is introduced
at the method boundary; inside the body the inline chain stays Array-typed — settled
decision #4). Expectation updates: NONE — no existing test pinned scope-call result
types. Evidence instead: new `tests/relation_typing.rs` (9 tests, inline MapVfs app):
scope → Relation, chain preserve, delegation (`Story.recent.for_user(1)`), terminals
exact (first → T?, count → Int, to_a → Array[T]), sibling-scope root, conservative
non-flip, view block-param typing downstream.

### R4b — `593fba1a` (byte-identical ✓) — refactor-plan 4.2 realized
`array_method`'s relation-branch string arms migrated to Relation-context catalog lookups
via `relation_return_on_array_repr` (builders preserve `Array<elem>`; `SelfOrNil` uses
`union_of` so union elements flatten). Kept as code: `model` and `arel` (genuinely
elem-dependent — union-element cases inexpressible as a ReturnKind).

### R5 — `f2fa289e` — **lobsters /t/tag = HTTP 200** ✓
**Finding: the plan misdiagnosed the blocker.** `/t/tag` was NOT blocked on
assoc-returns-Relation splicing — `Tag.related` is a *parameterized* scope
(`Arel.sql("COUNT(*) desc")` ordering + a relation-valued predicate), which correctly
lives on the runtime-Relation path, where three runtime/lowering gaps bit:
1. `column_predicate`: a **Relation used as a where VALUE** stringified
   (`taggings.story_id = '#<Relation>'`) instead of rendering Rails' subquery form
   `IN (SELECT …)`. Fixed in `runtime/ruby/active_record/relation.rb` (shared home, all
   ruby-family lanes).
2. `group(:id)` rendered unqualified → "ambiguous column name" under the `joins`. Fixed:
   symbol group columns qualify with the relation's table (Rails renders
   `GROUP BY "tags"."id"`).
3. Rails' array-delegation surface: `to_ary` (lets `[@story, relation].flatten` splice —
   the /s/:short_id crash), block-form `filter`, set ops `&`/`|`/`-`. Added to the
   runtime + its RBS; `Array#&`/`#|` also gained analyzer typing (were `Var`).
Also landed (scope_chain lowering): **owner-typed association resolution** in
`assoc_owner_seed` (peels `T | Nil`; disambiguates assoc names declared on several
models — `comments` on Story AND User) and the **CollectionProxy constructor rewrite**
`story.comments.build(attrs)` → `Comment.new(attrs…, story_id: story.id)` (+
`mentions_assoc_constructor` gate so ctor-only bodies enter the rewrite; regression test
in relation_typing.rs).
Verification: `/t/et` = 200 with all 7 tagged stories + related-tags box (subquery SQL
verified against seeded fake_data ground truth); `/t/aperiam` = 200 (0 stories is
truthful — its one story is deleted); `/`, `/newest` still 200 with 25 stories. SQL
parity: emit diff vs baseline = the runtime relation.rb/.rbs files only; every app-code
SQL fold byte-identical across 12 targets. Live `framework_tests_ruby` 5/5. Untyped
ceiling 190 → 195 (documented — the new surface is record-typed like `to_a`).
**The compile-time splice (scope contributions + `try_build_arel` receiver-position
resolution) was NOT built** — the deliverable didn't need it, and building it against a
corpus whose scope call sites are overwhelmingly parameterized would have been
speculative. It moves to R7 as a decision. `/s/:short_id` advanced past three blockers;
its remaining one is `ms.comments.build` in VIEW position where `ms` is an untyped
strict-locals param — the view-param-typing family
(project_lobsters_followon_deployment_bench ledger), not this plan's.

### R6 — `bb94d85b` (byte-identical ✓)
New `relation_residue` post-analyze pass (last in POST_ANALYZE_PASS_ORDER, pure ledger):
one `LowerResidue` warning per Relation-typed chain head, construct **`dynamic_relation`**,
reason `unspecialized_relation_chain`. Counts: **real-blog 0, tiny-blog 0** (as
expected), **upstream lobsters 48** chain heads (models 18, controllers ~20, jobs 3,
misc; scope-heavy corpus — `Story.base/tagged/top`, `Tag.related`, pagination chains).
All 48 execute correctly on the runtime Relation in the ruby-family lanes today; the
count is the tier-2/tier-3 decision input. Bigger than the plan's "small handful" guess —
which strengthens, not weakens, the case for R7's splice decision.

### R7 — decision points for Sam (recorded, not done)
1. **Build the tier-1 splice after all?** 48 lobsters sites is a real population.
   Recording scope predicate contributions + `try_build_arel` receiver-position
   resolution would fold the *param-less* subset; the parameterized subset
   (`tagged(user, tags)`) needs contribution-with-holes (prepared-statement shaped —
   aligns with the param-binds cache). The ledger can now price this precisely:
   grep `dynamic_relation` and classify by param-ness.
2. **Converge inline-chain typing** (`Model.where(...)` chains currently Array-typed)
   onto `Ty::Relation` — kills the dual representation; gate = emit-neutrality.
3. **Tier-2 enumerable branch shapes** (finite branch combinations → N prepared
   statements, runtime flag select — aligns with ROUNDHOUSE_PARAM_BINDS shape cache).
4. **Effects-to-terminal** for Relation-typed chains: skipped in R4 per plan guidance
   (the two receiver-blind consumers still filter the Relation context; catalog comment
   updated to say so). Cheap to do once the effects tests are in hand.
5. **Association context stays folded into Relation** (R2 choice) — revisit only if a
   CollectionProxy-only method (`<<` on an association) needs cataloging.
6. Strict-target story for the 48: today each site would ERROR at strict emit (by
   design, R1). Options when a strict lobsters lane matters: splice (1), or a per-target
   runtime Relation (settled decision #3 says NOT without this ledger justifying it).
