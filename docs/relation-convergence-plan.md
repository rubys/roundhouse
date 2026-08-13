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

(P0b asset pointer; C1 parity evidence, deleted-shim list, ledger re-counts; C2
effects-migration list or stop verdict)

### P0a — splice pricing, measured 2026-08-13

Measured at roundhouse `9518913c`, lobsters `9e849fd4`, once-campfire `2aa4141`.

**How to reproduce.** `roundhouse-check` does NOT surface these: it runs `analyze` +
`diagnose` and deliberately skips the post-analyze lowerings (see the four skip-listed
entry points documented in `src/session.rs`), and the ledger runs inside
`apply_post_analyze_lowerings` (`src/lower/mod.rs:397`). Only the transpile / site /
dump_ir drivers emit it. The construct id `dynamic_relation` does not appear in the
rendered text — grep the message:

```
./target/release/roundhouse --target ruby --allow-unsupported <APP> -o <TMP> 2>&1 \
  | grep -c 'relation chain stays dynamic'
```

Counts are target-independent (post-analyze pass): `--target rust` reports the same 50
for lobsters. Each run is ~1s.

| app | sites |
|---|---|
| fixtures/real-blog, tiny-blog, roda-blog | 0 |
| lobsters | **50** (R6 recorded 48; +2 drift, expected per C1's note) |
| once-campfire | **24** (never previously measured) |

Other local checkouts fail *ingest* before the ledger runs, so they yield no number:
mastodon (`AliasMethodNode`), writebook (`MatchRequiredNode`), fizzy (`private` inside
`class << self`), showcase (`ClassVariableWriteNode`).

**lobsters — 50.** By layer: controllers 26, models 18, jobs 6. By element type: Story 22,
User 6, Tag 6, Comment 4, ModActivity 3, Message 3, Invitation 2, SavedStory/Link/
HiddenStory/Hat 1 each. By file: `home_controller.rb` 11, `story.rb` 9,
`messages_controller.rb` 3, then `tag.rb`/`comment.rb`/`prefill_page_cache_job.rb`/
`notify_comment_job.rb`/`story_urls_controller.rb`/`stories_controller.rb`/
`signup_controller.rb` 2 each, and 13 files with 1. By chain-head method: `where` 8,
`limit` 5, `load` 4, `active` 4, `select` 3, `order` 3, then `tagged`/`hottest`/
`for_presentation`/`by` 2 each and 15 singletons.

**once-campfire — 24.** By layer: controllers 15, models 9. By element type: User 17,
Message 4, Membership 3. By file: `message/pagination.rb` 4, `room/message_pusher.rb` 3,
`accounts/bots_controller.rb` 3, then 2s and 1s across 10 more. By chain-head method:
`ordered` 7, `active` 5, `active_bots` 3, then 9 singletons.

**Param-ness.** Classified by whether the chain head takes a runtime argument. lobsters
≈30 parameterized / ≈20 param-less; once-campfire ≈3 / ≈21. Treat the lobsters split as
±3 — a handful of heads take literal-only arguments (`Story.hottest(nil, [])` in
`prefill_page_cache_job.rb:13-14`, `story.rb:88`'s `limit(STORIES_PER_PAGE)`) and fold
like param-less sites, so the boundary is a judgment call. The two apps are shaped
oppositely and that is the finding:

- **once-campfire is almost entirely param-less scope chaining**: `User.active`,
  `.ordered`, `User.active_bots`, and `Message::Pagination`'s
  `scope :first_page, -> { ordered.first(PAGE_SIZE) }`. Plain contribution recording
  would fold nearly all 24.
- **lobsters' bulk is parameterized and sits on the benchmarked routes**: 11 of the 50 are
  `home_controller`'s `paginate Story.<scope>(@user, filtered_tags.map(&:id))` family
  (`active` :16, `hidden` :30, `hottest` :41, `newest` :83, `newest_by_user` :123,
  `recent` :139, `saved` :154, `categories` :184, `tagged` :206 and :234,
  `Tag.related(@tag)` :210) — i.e. the main query of `/`, `/active`, `/newest`,
  `/recent`, `/saved`, `/hidden`, `/categories`, `/t/:tag`. Add
  `messages_controller`'s `Message.inbox(@user).load` ×2 / `outbox` ×1 and
  `users_controller.rb:22`'s `ModActivity.user(@showing_user).order(...).limit(20)`.
  **Param-less splicing buys nothing on the lobsters benchmark surface**; those routes
  need contribution-with-holes (prepared-statement shaped, per
  `project_param_binds_prototype`) or a different mechanism entirely.

**Correction to relation-type-plan R7 item 6 — the strict-lane premise is stale.** R7
asserts each residue site "would ERROR at strict emit (by design, R1)". It does not.
Strict runs (no `--allow-unsupported`):

| app / target | verdict |
|---|---|
| lobsters, rust **and** crystal (identical) | `0 unsupported/syntax error(s), 111 type error(s)` — `send_dispatch_failed` 70, `ivar_unresolved` 29, `incompatible_binop` 12 |
| once-campfire, rust | `0 unsupported/syntax error(s), 66 type error(s)` — `send_dispatch_failed` 45, `ivar_unresolved` 21 |

Only 2 lobsters errors mention Relation (`load_async` on Relation[Comment],
`stories_controller.rb:177`; `partition` on Relation[Moderation],
`views/stories/show.html.erb:86`) and 5 in campfire (`new`, `partition` ×2,
`find_by_transfer_id`, `authenticate_by`, all on Relation[User]) — and those are *catalog*
gaps (method not known on the Relation context), not the dynamic chains themselves. The
50/24 chains pass strict emit. `unsupported_relation_ty` fires only where a Relation type
is actually *rendered*, which these sites mostly avoid.

What happens instead is silent: `home_controller#active`'s whole
`@stories, @show_more = get_from_cache(active: true) { paginate Story.active(...) }`
emits on rust2 as `/* TODO rust2: ExprNode::Discriminant(28) */`. Related and probably
the same root: `push_scope_methods` / `push_scope_variants`
(`src/lower/model_to_library/mod.rs:819, 891`) are called *only* from
`src/emit/ruby/library.rs:614, 715`, so a `scope :recent` produces **no method at all** on
any strict target. The residue ledger is therefore not currently functioning as the
forcing function it was designed to be, and the strict-target ledger under-reports. That
is a separate finding from this plan (emitter truthfulness), not a C1/C2 item.

For scale context, lobsters emits 120 `lower_residue` warnings total; `dynamic_relation`
is 50 of them, the rest being `respond_to` arm drops (45), `job_class_side` option drops
(13), reflective `send` (3), and permitted-field / association-writer notes.

**Recommendation.** Build the splice for the param-less subset — it is cheap, it is plain
contribution recording, and it retires ~21 of once-campfire's 24 sites, which matters
because campfire is the live target milestone. Do *not* build contribution-with-holes off
the back of it: lobsters' parameterized bulk is the benchmark surface, and the honest
alternative there is per-model monomorphization of the runtime Relation — generating
`Relation` as a per-model `LibraryClass` in the shared lowering (the `model_to_library`
pattern), which deletes exactly the two things that keep `runtime/ruby/active_record/
relation.rb` ruby-family-only (`@model` held as a dynamic class value, and
`Array[untyped] @records`, the slow-shape rule #1 violation on the spinel AOT lane).
Settled decision #3 deferred per-model relation classes until a ledger justified them;
this is that ledger, but the trigger for acting on it should be evidence, not the count
alone. That evidence is now in — see the allocation measurement below: **monomorphization
is a portability item, not a benchmark item.** It waits for a strict target (crystal) to
become a lane driven to green, not for a lobsters performance push.

### P0a addendum — allocation composition, both AOT lanes, measured 2026-08-13

The open question above ("does the dynamic relation path cost the spinel AOT lane, or only
portability?") was posed because the published benchmarks show a much larger AOT dividend
on blog than on lobsters: blog `/articles` Spinel 9,153 vs emitted-Ruby 3,108 req/s
(2.94×) and `/articles/1.json` 21,708 vs 5,474 (3.97×), against lobsters' whole-sequence
AOT 92.35 ms vs roundhouse-YJIT 115.98 ms (1.26×). Blog carries 0 residue sites and
lobsters 50, eleven of them the main query of the benchmarked home routes — so "the
untyped runtime Relation is what costs AOT its advantage" was the obvious hypothesis.

**Method.** `SPINEL_ALLOC_REPORT` (spinel `docs/profiling.md`) dumps at `atexit`, so the
blog lane — normally driven over HTTP by wrk — cannot be measured as shipped. Copied
`build/transpiled-blog-spinel` to a scratchpad tree, added a `bench_driver.rb` mirroring
`scripts/lobsters-spinel-driver.rb` (same `Tep::Request` construction, same
`Db.with_connection { Main.dispatch(req, res) }` seam `MainApp#dispatch` uses), spliced a
`--bench` branch into `main.rb` ahead of the server start, and rebuilt with `make build`.
Both lanes are therefore measured in-process at the identical seam. Lobsters ran its
normal parity + verify + 2 warmup + 3 timed passes (~664 dispatches); blog ran its 5
routes × 128 (640 dispatches, all 200). Spinel `79249df8a232`.

| | lobsters AOT | blog AOT |
|---|---|---|
| dispatches | ~664 | 640 |
| objects / bytes | 1,661,801 / 138.8 MB | 201,464 / 13.6 MB |
| **per request** | **2,503 obj / 214 KB** | **315 obj / 21.7 KB** |
| avg response | 35.1 KB | 3.2 KB |
| **alloc per response byte** | **6.2×** | **6.9×** |
| `ActiveRecord::Relation` | 5,857 (0.35% obj, 0.52% byt) | **0** |
| poly containers (`Array`/`Hash`, no elem type) | 18,173 (1.09% obj, 0.57% byt) | 781 (0.39% obj, 0.43% byt) |
| mono containers (`Array(T)`/`Hash(K,V)`) | 326,546 (19.7% obj, 15.4% byt) | 50,349 (25.0% obj, 22.8% byt) |
| `String` | 59.3% obj, 68.6% byt (95 MB) | 55.0% obj, 62.2% byt |

**The ledger prediction holds and the hypothesis does not.** Blog allocates *zero*
Relation objects — 0 residue sites means nothing ever reaches the runtime Relation,
confirmed at the allocator. Lobsters allocates 5,857 (≈8.8 per request). But the
poly-container tax is not there: untyped containers are 1.09% of objects / 0.57% of bytes
on lobsters against 0.39% / 0.43% on blog, a delta of 0.7 points of objects and 0.14
points of bytes. Untyped SHAPES — the part of the dynamic path that AOT specifically
cannot specialize and YJIT recovers by observing runtime types — are that 0.57% of bytes,
and nothing that small moves a lane ratio from 2.9× to 1.26×.

What the report shows instead is **scale**: lobsters allocates 8× the objects and 10× the
bytes per request, at essentially the *same* amplification per byte of output (6.2× vs
6.9×). Lobsters is not allocating wastefully; it does an order of magnitude more work per
request, into a collector whose mark is live-set proportional. That is the
Amdahl-plus-heap-size reading, now measured rather than argued, and it is consistent with
[[project_lobsters_per_route_medians_measure_gc_arrival]]'s finding that the per-route
table largely measures collection arrival.

Limits of this measurement, in decreasing order of how much they could change the reading:
- **Allocation, not live heap.** Mark cost tracks the live set; these are cumulative
  allocation counters. Relation objects are per-request and short-lived, so they are
  unlikely to be over-represented in the live set, but the report cannot show that.
- **55 unnamed `scan_0x…` rows** on lobsters (7.5% obj, 1.5% byt) — cross-TU scan
  callbacks, unresolvable without a `--profile` symbol map (`nm` on the shipped binary
  does not line up: the report's runtime addresses land in libsqlite3 text). Even if all
  of them were poly containers the totals stay under 9% of objects and 2% of bytes.
- The blog driver is not the published harness and the blog fixture DB holds 3 articles.
  These are composition numbers, not throughput numbers, and must not be quoted against
  the published page — see [[feedback_pin_benchmarks_not_ledgers]].

### Per-SITE attribution — the dynamic path is ~10% of bytes, not ~1%

The by-TYPE table above answers the AOT-vs-YJIT question but is the wrong denominator for
"what does the dynamic path cost at all": it counts only what the path allocates *of
distinctive types* (Relation objects, poly containers) and misses everything it allocates
of ordinary ones. `Array(String)` alone is 10.7% of lobsters objects, and the obvious
question — where does it come from — is what forced this pass.

Re-run with `SPINEL_ALLOC_SITES=1` (~346 dispatches: parity + verify + 1 warmup + 1 timed;
per-request composition matches the longer run, 2,578 vs 2,503 obj/req, so the two are
consistent). Of 96,244 `Array(String)` allocations: `sp_Relation_to_a` 18.6%,
`sp_User_s_instantiate` 9.6%, the `_preload_dispatch`/`_preload_batch_*` family ~17%,
`sp_Relation_where` 2.3% — **over 40% from the dynamic relation path.** The preload family
counts as dynamic-only: the static arel path lowers `includes` into inline preload
statements, so those per-model methods exist solely for chains reaching `Relation#to_a`.

Aggregated by frame across all types (892,137 objects / 73.6 MB):

| band | objects | bytes |
|---|---|---|
| runtime Relation + preload frames | 10.47% | 7.88% |
| `select_rows` row Hashes | 3.36% | 0.44% |
| `*Row` intermediates (UserRow, StoryRow, …) | 0.95% | 1.63% |
| **directly attributable to the dynamic path** | **~15%** | **~10%** |
| static hydration (`from_stmt`), for comparison | 1.21% | 1.84% |

Plus part of the 10.31% of bytes under `instantiate`/`from_raw` — though some of that is
the model objects themselves, which either path allocates.

The mechanism is a clean dichotomy in the emitted code, and it is the specialization
difference made concrete:
- **folded**: `User.from_stmt(stmt)` — `Db.column_int(stmt, 0)`, `Db.column_text_opt(stmt,
  1)`, … straight into typed fields. One object per row.
- **dynamic**: `select_rows(sql)` → a `Hash[String, untyped]` per row →
  `UserRow.from_raw(row)` → `User.from_row(...)`. Three objects per row plus a per-column
  reparse.

On blog the query IS the controller: `Article` allocations attribute to
`sp_ArticlesController_index` with the hydrate loop inlined, and no `from_stmt` frame
exists at all. That also means blog cannot be compared band-for-band here — its zeros in
these bands are an inlining artifact. (The zero *Relation objects* in the by-type table is
not an artifact; it is solid.) Frame attribution also folds inlined callees into the
caller, so "under `Relation#to_a`" includes its inlined `to_sql` fragment assembly and the
Relation's own six accumulator-array ivars.

**Correction to the earlier "Strings are unattributed" caveat** (which came from a
2026-07-31 profiling note): this spinel build DOES attribute String allocations per site —
822 String site rows. The caveat is retired.

**Consequence for the recommendation above — one half stands, one half does not.** The
un-defer trigger for per-model monomorphization is still a strict target becoming a driven
lane, NOT lobsters performance: the dynamic path's cost is paid identically by the same
emitted Ruby on CRuby+YJIT and on spinel AOT, so removing it helps both lanes and does not
move the ratio. What does NOT stand is the footprint claim: the dynamic path is ~10% of
allocated bytes, not ~1%, so specializing it (by folding more chains or by monomorphizing
the Relation) is a real LANE-SHARED win of the same family as the 2026-07-16 `to_a`
memoization fix that took the sequence down 20%. Priced as an optimization it is worth
more than the first pass said; priced as an AOT-gap closer it is still worth nothing.
