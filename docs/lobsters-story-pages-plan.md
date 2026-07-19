# Lobsters story-pages plan — pin route checks, then /s/:short_id

Written 2026-07-19. Self-contained for a fresh session. This is deployment-bench feature
work on the lobsters proving lane, continuing the thread of commits 51f676bc (front page
+ /newest,/active,/recent,/top = 200), c01de3c7 (B2 pool-dispatch locals → /t/tag past
its ArgumentError), and f2fa289e (relation-type-plan R5: /t/tag = 200).
Suggested executor: Fable or Opus with care — L2 is typing-family debugging.

## Sequencing / coordination

Phase L1 is safe to run anytime. Phase L2 touches analysis/view typing — do NOT run it
concurrently with docs/with-adapter-split-plan.md or docs/relation-convergence-plan.md
(both touch `src/analyze/`). Recommended slot: after the split plan, before or after
convergence.

## L0 — Housekeeping (2 minutes)

If `git status` still shows modified-uncommitted `docs/maintainability-refactor-plan.md`
and `docs/relation-type-plan.md` (execution-log lines left by the 07-18/19 sessions),
commit them as a docs commit before starting.

## L1 — Pin the route checks (protects everything already shipped)

**Problem**: the deployment-bench route results — the headline deliverables of three
sessions — have NO committed regression check. `grep -rn '/t/et\|"/t/' tests/ scripts/`
comes up empty; each session verified ad hoc against the seeded app.

- First, discover the harness the prior sessions used: how the lobsters app is emitted
  from the upstream checkout, seeded with THEIR fake_data, and booted/driven (start from
  `scripts/bench-lobsters` and the orchestrator landed in daf69bec; the relation-type-plan
  R5 log describes the verification flow). Record the recipe in this file's log.
- Then commit a repeatable check — a script under `scripts/` (or an `#[ignore]`d,
  CI-invocable test if that fits the existing bench-lane pattern better) asserting, against
  the seeded app: `GET /`, `/newest`, `/active`, `/recent`, `/top` → 200 with 25 stories
  on the front page; `/t/et` → 200 with 7 tagged stories + related-tags box; `/t/aperiam`
  → 200 with 0 stories (truthful — its one story is deleted). Content assertions should be
  cheap and stable (counts / marker strings), not full-page snapshots.
- Wire it wherever the bench lane runs in CI IF that lane exists and is cheap; otherwise
  a documented local script is enough — the point is a one-command re-verification.
Gate: the check passes on current main; commit.

## L2 — /s/:short_id (story page): the view-param-typing blocker

**Known state** (from relation-type-plan R5): the route advanced past three blockers
(`to_ary` flatten, and friends); the remaining one is **`ms.comments.build(...)` in VIEW
position where `ms` is an untyped strict-locals param**. The CollectionProxy constructor
rewrite already exists (`story.comments.build(attrs)` → `Comment.new(attrs…,
story_id: story.id)`, landed in f2fa289e with the `mentions_assoc_constructor` gate in
`src/lower/scope_chain.rs`) — it just can't fire because `ms`'s type is unknown, so the
association read doesn't resolve.

This is the strict-locals view-param-typing family — the same thread as 0c89a969
(strict-locals headers → partial signatures) and c850ebb0 (record binding by first
declared local). Approach:
- Reproduce first: drive `/s/:short_id` on the seeded app, capture the exact failure and
  the partial/param involved. Confirm `ms` (the strict-locals param) lacks a type and why
  — no strict-locals type annotation upstream? annotation present but not threaded to
  this partial? inference from call sites not reaching it?
- Fix at the semantic home, in preference order: (1) if the upstream template declares a
  type in its strict-locals header, thread it (extend the 0c89a969 mechanism); (2) else
  infer from render call sites (the callers pass a typed record — unify param type from
  call sites, the analyzer already has `unify_params_from_call_sites` machinery for
  method params); (3) only if both fail, consider what the c850ebb0 record-binding
  convention can contribute. Do NOT special-case this one partial — the fix should be the
  family fix, whatever member of the family this is.
- Then re-drive the route; peel the next blocker if one surfaces (log each; stop and
  report if a blocker is outside the view-typing family — e.g. a new query gap belongs
  to the relation thread, not this plan).
Deliverable: `/s/:short_id` → 200 with the story, its comments rendered, verified against
seeded ground truth like L1's assertions. Add it to the L1 check. Gates: full
`cargo test --all-targets` diffed vs parent; strong emit harness for any analyzer/lowering
change (real transpile diff, not just emit_preview); every diff explainable.

## L3 — Follow-ons (list only, separate sessions)

Story-page POST flows (comments, votes) if GET lands cleanly; remaining routes; then the
spec tiers T1/T2 adoption from the follow-on-bench thread. Not this session.

## Execution log

(L1 recipe + check location; L2 root cause, family fix description, route evidence,
next-blocker notes)
