# CI for contributors

Use this page to understand a PR's checks, request broader validation, or
read a failure. Local commands live in [development/testing.md](../development/testing.md).
The workflows and their tests own implementation details, not this handbook.

## What runs

Coverage is a ladder. The planner (`scripts/ci-plan.py`) chooses jobs from
labels and changed paths; draft and ready PRs use the same policy:

| State | What runs |
|---|---|
| **Draft or ready**, no special label | Path-selected coverage on the Ruby floor |
| **Draft or ready** + `ci:spinel` | Ruby floor plus the full Spinel suite; no other language SDKs |
| **Draft or ready** + `ci:full` | Full validation (all targets, WASM, Writebook, Spinel) |
| **Push to canonical `main`** | Ruby floor plus the full Spinel suite; extra-language SDKs wait for the schedule |
| **Scheduled / manual Full validation** | Full validation (the extra-language ledger and publication cycle) |

PRs without a special label run a Ruby floor: fixture preparation, unit
tests, Store analysis, the CRuby comparison against Rails, and Campfire
conformance/comparison. Four unit shards cover all package test targets in
bounded batches; ignored integrations need selected toolchain lanes. Framework
and toolchain suites also run inside comparison jobs, not necessarily as
standalone checks.

That floor is the merge claim for ordinary analyzer, lowerer, and runtime
work: the Ruby shape runs, and Campfire still matches Rails. Crystal, Go,
Swift, Kotlin, C#, Elixir, Python, JRuby, Rust, TypeScript, WASM, and
Writebook do **not** start on that path unless the diff owns them or a
maintainer applies `ci:full`. JRuby stays with Ruby-family path ownership
(`src/emit/ruby.rs`, interpreter-only runtime files, proven `src/project.rs`
bodies). Spinel starts when the diff owns it, or via `ci:spinel` / `ci:full`.
Extra-language failures after merge are a scheduled-ledger item, not a
reason to block the next Ruby PR.

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
has no effect. Stacked labels prefer the broader lane: `ci:full` > `ci:spinel`.
Documentation-only PRs still receive checks; changes to the rendered user guide
also select site/browser coverage.

Pushes to canonical `main` run the Ruby floor plus the full Spinel suite
and cancel a superseded SHA on the same ref. They do **not** run Crystal,
Go, Swift, Kotlin, C#, Elixir, Python, Rust, TypeScript, WASM, Writebook,
or the extra-language smoke matrix. Extra-target red is follow-up work on
the four-hour scheduled Full validation cycle, not a merge gate for later
Ruby PRs. That schedule remains the extra-language ledger, publication
path, and floating-pin catch-up.

## Request full, Spinel, or fresh validation

- **Spinel-focused CI:** apply `ci:spinel` on a draft or ready PR. Runs the
  Ruby floor plus every Spinel job; skips Crystal/Go/Swift/… SDKs, WASM, and
  Writebook. Prefer this over `ci:full` when only the native/Ruby-family lane
  matters.
- **More coverage:** ask a maintainer to apply `ci:full` to a ready or draft PR. The
  label triggers a full run of the current PR merge tree and keeps full
  coverage on later pushes. A comment requesting it is not itself a trigger.
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
| Coverage and execution | [ci.yml](../../.github/workflows/ci.yml), [ci-plan.py](../../scripts/ci-plan.py), [ci-unit-tests.py](../../scripts/ci-unit-tests.py) | `tests/ci_policy_workflow.rs`, `tests/workflow_yaml_parses.rs` |
| Toolchain selection | [ci.yml](../../.github/workflows/ci.yml), [`.ruby-version`](../../.ruby-version), [bin/rh](../../bin/rh) | `tests/ci_toolchain_workflow.rs`, `tests/rh_verify.rs` |
| Receipt reuse | [ci-reuse.py](../../scripts/ci-reuse.py) | `tests/ci_reuse_test.py` |
| Fixture caching | [generate-fixture in ci.yml](../../.github/workflows/ci.yml) | `tests/ci_fixture_workflow.rs` |
| Archive evidence and Pages | [ci-archive-evidence.py](../../scripts/ci-archive-evidence.py), [full-ci.yml](../../.github/workflows/full-ci.yml) | `tests/ci_policy_workflow.rs` |

Run the relevant suites with `cargo test --test <stem>`; Python tests can run
directly with `python3 -B tests/ci_reuse_test.py -v`, for example. Detailed
cache keys, receipt fingerprints, resource sampling, and GC-mode witnesses
belong beside their implementation and tests, not in a parallel prose spec.
