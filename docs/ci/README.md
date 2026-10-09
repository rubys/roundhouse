# CI for contributors

Use this page to understand a PR's checks, request broader validation, or
read a failure. Local commands live in [development/testing.md](../development/testing.md).
The workflows and their tests own implementation details, not this handbook.

## What runs

Coverage is a ladder with **two gates**. The planner (`scripts/ci-plan.py`)
chooses jobs from labels and changed paths; draft and ready PRs use the same
policy. Strategic priority: **Ruby first, then Spinel**; the seven extra
languages are async ledger work.

| Gate | What must be green | What may stay red |
|---|---|---|
| **Merge-gate** | Ruby floor (`BASE`) plus any lanes a **focus label** made required for that run | Extra-language ledger, advisory Spinel on main/Full, unselected skips |
| **Ledger / Full** | Honest visibility of every selected target | Does **not** hold merge by itself — `compare-extra` / `smoke-extra` are advisory unless focused |

A green merge-gate is **not** proof that Crystal/Go/Swift/… all pass. Read
advisory jobs and the scheduled Full cycle for the multi-target ledger.

| State | What runs |
|---|---|
| **Draft or ready**, no special label | Path-selected coverage on the Ruby floor |
| **Draft or ready** + `ci:spinel` | Ruby floor plus the **CORE** Spinel lane (required for that run) |
| **Draft or ready** + focus label(s) | Ruby floor plus **only** the selected focus lanes (required) |
| **Draft or ready** + `ci:full` | Full validation (all targets, WASM, Writebook, Spinel); extras stay advisory |
| **Push to canonical `main`** | Ruby floor plus the full Spinel suite (advisory); extra-language SDKs wait for the schedule |
| **Scheduled / manual Full validation** | Full validation (the extra-language ledger and publication cycle) |

PRs without a special label run a Ruby floor: fixture preparation, unit
tests, Store analysis, the CRuby comparison against Rails, and Campfire
conformance/comparison. Four unit shards cover all package test targets in
bounded batches; ignored integrations need selected toolchain lanes.
Shard 0 of `unit` also runs each `gems/roundsnap/test/*_test.rb` in its own
MRI process (`ruby -Ilib:test "$file"`): extra positional arguments are ARGV,
not additional test files, so one invocation cannot load the suite. Missing
files fail the step; they are not skipped green. Those gem tests ride the
Ruby floor (including compiler/runtime diffs) because `unit` is in `BASE`.
`gems/roundsnap/` and `src/roundsnap.rs` stay on that floor — they do not
fan out extras or Spinel11. There is no `campfire-roundsnap` job and no
`ci:roundsnap` focus label. The default `roundsnap_delivery` target runs
real-blog emit + boot + the four emitted model/controller suites, plus
source-frame assertions for a model and ERB view. It uses subprocess flags,
not global environment mutation, and fails when MRI or fixtures are absent.
The existing `campfire-conformance` job additionally emits and boots the
pinned Campfire app through Roundsnap without app/runtime `.rb`, then runs
its full emitted suite without stubs. Its per-file results and per-test
failures must match the plain Ruby lane on the same app, MRI, gems and
compiler; the plain lane's existing conformance floor still applies.
The standalone `campfire-roundsnap` script remains a boot-only local smoke.
Framework and toolchain suites also run inside comparison jobs, not
necessarily as standalone checks.
**Spinel is not part of `BASE`.**

That floor is the merge claim for ordinary analyzer, lowerer, and runtime
work: the Ruby shape runs, and Campfire still matches Rails. Crystal, Go,
Swift, Kotlin, C#, Elixir, Python, Rust, TypeScript, WASM, and Writebook do
**not** start on that path unless the diff owns them or a maintainer applies
`ci:full`. When path ownership or Full selects the seven extras, their
`compare-extra` / `smoke-extra` jobs are **advisory** (visible, not
merge-blocking) unless a focus label made them required. JRuby stays with
Ruby-family path ownership (`src/emit/ruby.rs`, interpreter-only runtime
files, proven `src/project.rs` bodies) or `ci:jruby`. Spinel starts when the
diff owns it, via `ci:spinel` (CORE, required), or via `ci:full` / main push
(full suite, mostly advisory). Extra-language failures after merge are a
scheduled-ledger item, not a reason to block the next Ruby PR.

Selected lanes start once their inputs are ready, without waiting for unit
tests to pass. Campfire consumes an independently built same-run debug compiler.
Speculative work may therefore finish even when a unit shard fails; the final
gate still requires all selected non-advisory checks, including the unit matrix.

Additional checks are selected from the changed inputs. Target-specific
changes select owning lanes. Shared emit (`src/emit/shared/`), the compare
harness, and the smoke harness still select every extra target they own.
CI files, Cargo manifests, `src/project.rs`, and unknown changed inputs do
**not** fan out extra-language SDKs — they stay on the Ruby floor (unknown
inputs keep Spinel too). Apply `ci:full` when a workflow edit must prove
`compare-extra` / `smoke` steps. Changes only to CI contract tests retain
the Ruby floor. Analyzer/lowerer changes do not automatically select every
target. CLI help and other `src/bin/roundhouse.rs` edits stay on the Ruby
floor.
The planner diffs the PR merge tree (or the PR head) against its base,
includes both sides of a rename, and does not expand to extra SDKs when
the trees cannot be identified. A newer main than the event's `base.sha`
is not unknown input.
See the run's **plan** job for its selected jobs and reasons.

Changing draft status does not restart checks or change coverage. `ci:draft`
has no effect. Stacked labels prefer the broader lane:
`ci:full` > any focus labels (`ci:<lang>` / `ci:extras` / `ci:jruby` /
`ci:spinel`, unioned on `BASE`) > path ownership. Documentation-only PRs
still receive checks; changes to the rendered user guide also select
site/browser coverage.

### Focus labels (narrow fix rounds)

When Full validation (or a `ci:full` PR) shows red lanes, apply focus labels
so fix rounds only queue the lanes under repair — and those lanes are
**required** (not advisory) for that run:

| Label | Selects (required while focused) |
|---|---|
| `ci:crystal` … `ci:python` | That language's `compare-extra` **and** `smoke-extra` (+ `build-site` / archives / `archive-results`) |
| `ci:extras` | All seven of the above |
| `ci:jruby` | `compare-jruby` + floor `smoke` jruby (+ site/archives) |
| `ci:spinel` | CORE Spinel (`spinel-build`, `spinel-toolchain`, `spinel-compare`) + `spinel-framework` (+ site/archives). Not the heavy Campfire Spinel suite (`campfire-spinel-*`); not folded into `ci:extras`. |

**Narrow semantics:** with any focus label set and `ci:full` **not** set, the
plan is the Ruby floor (`BASE`) plus only the selected focus lanes. Path
ownership does not expand the plan. Unrelated extras / WASM / rust·ts
`compare` / Writebook / full Spinel11 stay off. `ci:full` still wins as the
full ledger (extras remain advisory there).

**Rust vs TypeScript:** selecting one no longer forces both `compare` matrix
legs — each rides an independent plan `compare` list.

**Flow:** Full red → set focus label(s) → fix rounds → remove focus labels →
Full again. Do not treat a focus-green PR as multi-target support without a
subsequent Full (or `ci:full`) pass.

The focus `ci:*` labels are defined on the GitHub repo; if a fork is missing
them, create labels with those exact names (color may match other `ci:*`
labels) so applying them on a PR is possible.

### Fork PRs and “Approve and run”

In-repo CI uses ordinary `pull_request` (not `pull_request_target`) with a
read-only `GITHUB_TOKEN` (`contents: read`). There is **no** workflow `if:`
that skips forks or first-time contributors; fork PRs take the same plan path
as same-repo PRs. Secrets are not passed to untrusted fork code.

If Actions still shows **Approve and run** / waiting for approval on a fork
PR, that gate is a **GitHub org or repo Actions setting**, not workflow logic.
A maintainer with admin access turns it down under:

**Settings → Actions → General → Fork pull request workflows from outside collaborators**

Choose **Require approval for first-time contributors only** or **Don't
require approval for all outside collaborators** (org owners may also set
this at the organization Actions policy). Workflow edits cannot remove that
prompt.

Pushes to canonical `main` run the Ruby floor plus the full Spinel suite
and cancel a superseded SHA on the same ref. They do **not** run Crystal,
Go, Swift, Kotlin, C#, Elixir, Python, Rust, TypeScript, WASM, Writebook,
or the extra-language smoke matrix. Extra-target red is follow-up work on
the four-hour scheduled Full validation cycle, not a merge gate for later
Ruby PRs. That schedule remains the extra-language ledger, publication
path, and floating-pin catch-up.

## Request full, Spinel, focus, or fresh validation

- **Extra-language fix rounds:** apply one or more focus labels (`ci:swift`,
  `ci:go`, …, or `ci:extras`) on a draft or ready PR. Runs the Ruby floor plus
  only those `compare-extra` and `smoke-extra` lanes as **required**. Prefer
  this over `ci:full` when repairing a few red extras after Full validation.
- **JRuby fix rounds:** `ci:jruby` — `compare-jruby` + smoke jruby, required.
- **Spinel-focused CI:** apply `ci:spinel` on a draft or ready PR. Runs the
  Ruby floor plus the CORE Spinel lane as **required**; skips Crystal/Go/… SDKs,
  WASM, Writebook, and the heavy Campfire Spinel suite. Prefer this over
  `ci:full` when only the native/Ruby-family lane matters. Multiple focus
  labels union on `BASE`.
- **More coverage:** ask a maintainer to apply `ci:full` to a ready or draft PR. The
  label triggers a full run of the current PR merge tree and keeps full
  coverage on later pushes. Extra-language lanes stay advisory on Full so the
  ledger can be red without holding merge. A comment requesting full is not
  itself a trigger.
- **Fresh execution:** select **Re-run all jobs** on the desired run.
  Selected PR checks may otherwise reuse successful execution evidence on
  identical inputs. Full coverage alone does not disable that reuse.
- **A newer head:** needs a new run. Reruns retain the original SHA and
  coverage; rerunning an old compact run neither tests the new head nor
  expands its matrix.
- **Manual full validation:** Actions → **Full validation** → **Run workflow**.
  Leave `publish` unchecked. This executes freshly on the chosen ref; a
  branch-head dispatch is not a substitute for a PR merge-tree check.

Superseded PR runs cancel. Push-to-main Ruby+Spinel runs also cancel a
superseded SHA; the scheduled full-ci lock does not. Neither dependency-cache hits nor
restored fixture source are test results; check the job summary for any
explicitly reused execution evidence. The compact and summary gates run only
when `plan` succeeded and the workflow has not been cancelled. Their explicit
`!cancelled()` status check overrides GitHub's implicit `success()`, so skipped
or failed dependencies still reach the result evaluator. Cancelling the workflow
stops the gates instead of scheduling `always()` work in a superseded run.
A missing plan output is not a valid successful selection.

## Read results honestly

`CI summary` reports selected non-advisory checks that failed, skipped, were
cancelled, or are missing. Unselected skips are expected. The `archive-results`
job is packaging evidence, not a validation receipt: abandoned or failed
reports do not fail the summary gate (Pages assembly still requires a
successful report when publishing). It is informational: the workflow does
not impose branch protection or decide when to merge.

Read advisory jobs and raw step outcomes too. `continue-on-error` can hide a
Spinel failure in the overall conclusion. A green summary is not proof that
every target passed, and a missing compiler/archive can block dependent checks
without those checks having executed. Spinel failures can originate in
Roundhouse, its runtime/RBS/packaging, or upstream; establish the cause before
attributing it. Do not add workarounds just to hide advisory failures.

For a failing lane, inspect its logs and retained reports/repro artifacts,
then run the owning local harness. Do not regenerate corpus baselines or
broaden comparison masks merely to turn CI green.

## Publication is separate

PR checks never deploy Pages. Pushes to canonical `main` run Ruby+Spinel
without publication. The four-hour scheduled cycle on canonical
`rubys/roundhouse` main is what runs the extra-language matrix and requests
publication. Manual publication is opt-in on canonical main.

Pages requires the compact publication floor (Ruby plus any selected
Rust/TypeScript lanes) and verified same-run assembly before deployment.
Main advancing while Full validation is still running does **not** abort
publish: deploy ships that run's assembled artifact for its validated SHA.
It does **not** require all extra/advisory lanes to pass. Failed archives may
be useful repro downloads, not validated output. The published
`ci/archive-results.json` reports archive presence and validation separately.
Evidence applies to exact bytes: testing a TGZ does not certify its sibling
ZIP/JSON.

CLI binary releases are different: the tag-triggered cargo-dist
[release workflow](../../.github/workflows/release.yml) creates GitHub Releases;
it does not inherit the Pages validation guards.

## Changing CI

Read the owner and its executable contract before editing:

| Concern | Source | Tests |
|---|---|---|
| Coverage and execution | [ci.yml](../../.github/workflows/ci.yml), [ci-plan.py](../../scripts/ci-plan.py), [ci-unit-tests.py](../../scripts/ci-unit-tests.py) | `tests/ci_plan_test.py`, `tests/ci_plan_focus_test.py`, `tests/ci_policy_workflow.rs`, `tests/workflow_yaml_parses.rs` |
| Toolchain selection | [ci.yml](../../.github/workflows/ci.yml), [`.ruby-version`](../../.ruby-version), [bin/rh](../../bin/rh) | `tests/ci_toolchain_workflow.rs`, `tests/rh_verify.rs` |
| Receipt reuse | [ci-reuse.py](../../scripts/ci-reuse.py) | `tests/ci_reuse_test.py` |
| Fixture caching | [generate-fixture in ci.yml](../../.github/workflows/ci.yml) | `tests/ci_fixture_workflow.rs` |
| Archive evidence and Pages | [ci-archive-evidence.py](../../scripts/ci-archive-evidence.py), [full-ci.yml](../../.github/workflows/full-ci.yml) | `tests/ci_policy_workflow.rs` |

Run the relevant suites with `cargo test --test <stem>`; Python tests can run
directly with `python3 -B tests/ci_reuse_test.py -v`, for example. Detailed
cache keys, receipt fingerprints, resource sampling, and GC-mode witnesses
belong beside their implementation and tests, not in a parallel prose spec.
