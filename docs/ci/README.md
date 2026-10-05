# CI for contributors

Use this page to understand a PR's checks, request broader validation, or
read a failure. Local commands live in [development/testing.md](../development/testing.md).
The workflows and their tests own implementation details, not this handbook.

## What runs

Coverage is a ladder. The planner (`scripts/ci-plan.py`) chooses jobs from
the PR draft state, labels, and changed paths:

| State | What runs |
|---|---|
| **Draft**, no CI label | Nothing selected — build and review without runners |
| **Draft** + `ci:draft` | Fixture preparation and unit shards only |
| **Draft or ready** + `ci:spinel` | Ruby floor plus the full Spinel suite; no other language SDKs |
| **Draft or ready** + `ci:full` | Full validation (all targets, WASM, Writebook, Spinel) |
| **Ready** (non-draft), no special label | Path-selected coverage on the Ruby floor |

Ready PRs without a special label run a Ruby floor: fixture preparation, unit
tests, Store analysis, the CRuby comparison against Rails, and Campfire
conformance/comparison. Four unit shards cover all package test targets in
bounded batches; ignored integrations need selected toolchain lanes. Framework
and toolchain suites also run inside comparison jobs, not necessarily as
standalone checks.

That floor is the merge claim for ordinary analyzer, lowerer, and runtime
work: the Ruby shape runs, and Campfire still matches Rails. Crystal, Go,
Swift, Kotlin, C#, Elixir, Python, JRuby, Rust, TypeScript, WASM, Writebook,
and Spinel do **not** start on that path unless the diff owns them or a
maintainer applies `ci:full` / `ci:spinel`. Extra-language failures after merge
are a main ledger, not a reason to block the next Ruby PR.

Selected lanes start once their inputs are ready, without waiting for unit
tests to pass. Campfire consumes an independently built same-run debug compiler.
Speculative work may therefore finish even when a unit shard fails; the final
gate still requires all selected non-advisory checks, including the unit matrix.

Additional checks are selected from the changed inputs. Target-specific
changes select owning lanes; shared emit, build, packaging, and CI-policy
implementation changes can select full coverage. Changes only to CI contract
tests retain the Ruby floor rather than expanding to every target.
Analyzer/lowerer changes do not automatically select every target: request
full coverage when the risk warrants it. CLI help and other
`src/bin/roundhouse.rs` edits stay on the Ruby floor.
The planner diffs the PR merge tree (or the PR head) against its base,
includes both sides of a rename, and expands only when the trees cannot be
identified. A newer main than the event's `base.sha` is not unknown input.
See the run's **plan** job for its selected jobs and reasons.

Drafts stay idle until `ci:draft`, `ci:spinel`, or `ci:full` is applied.
Marking a PR ready-for-review leaves the draft idle path and runs the normal
ready planner. `ci:full` / `ci:spinel` also work while the PR is still a draft.
Documentation-only ready PRs still receive checks; changes to the rendered user
guide also select site/browser coverage.

Pushes to canonical `main` run full validation and cancel a superseded SHA
on the same ref. Extra-target red on that run is follow-up work on main,
not a merge gate for later Ruby PRs. The four-hour scheduled cycle remains
the publication and floating-pin catch-up.

## Request full, Spinel, or fresh validation

- **Slim draft CI:** apply `ci:draft` on a draft PR (fixture + unit only).
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

Superseded PR runs cancel. Push-to-main full runs also cancel a superseded
SHA; the scheduled full-ci lock does not. Neither dependency-cache hits nor
restored fixture source are test results; check the job summary for any
explicitly reused execution evidence.

## Read results honestly

`CI summary` reports selected non-advisory checks that failed, skipped, were
cancelled, or are missing. Unselected skips are expected. It is informational:
the workflow does not impose branch protection or decide when to merge.

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

PR checks never deploy Pages. Pushes to canonical `main` run full validation
without publication. The four-hour scheduled cycle on canonical
`rubys/roundhouse` main is what requests publication. Manual publication is
opt-in on canonical main.

Pages requires the compact publication floor (Ruby plus any selected
Rust/TypeScript lanes), verified same-run assembly, and a live-main SHA
check before deployment. It does **not** require all extra/advisory lanes
to pass. Failed archives may be useful repro downloads, not validated output.
The published `ci/archive-results.json` reports archive presence and validation
separately. Evidence applies to exact bytes: testing a TGZ does not certify its
sibling ZIP/JSON. A commit racing the last main check is not atomic with deploy.

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
