# CI coverage and conservative PR-local reuse

## Compact PR checks and full validation

`.github/workflows/ci.yml` runs a compact floor on PRs and main pushes:

- Fixture generation and `unit` (`cargo test --all-targets`, including emitted
  Ruby execution tests and the debug-profile bench emission checks).
- Store analysis, Ruby/Rust/TypeScript comparisons against live Rails.
- TypeScript SharedWorker browser tests, Campfire conformance, and Campfire
  comparison including its model/database differential.

These are nine validation executions, plus three small orchestration jobs
(`plan`, `compact-required`, `ci-summary`). Drafts select only fixture and
unit validation. **`CI summary` is informational:** it reports missing,
skipped, cancelled or failed selected non-advisory checks as red. Unselected
jobs may skip; advisory failures remain separate signals. This workflow does
not configure branch protection or require the summary to pass before merging;
maintainers decide when to merge. Publication still requires the compact floor
and verified assembly.

The planner compares the actual PR merge tree with its base, not just the last
commit. Renames/deletions retain both ownership sets. Unknown diff identity
expands to full validation. Documentation-only PRs still get the summary
status instead of being left pending by workflow-level path filters.

The CI helpers use Python's standard library, following the existing receipt
helper, without additional Python packages.

| Changed inputs | Additional coverage |
|---|---|
| Target emitter file **or directory**, target runtime, toolchain/framework test | Owning comparison and archive smoke; existing embedded framework/toolchain suites stay intact |
| Ruby emitter | Ruby/JRuby owners plus advisory native Spinel core (`build-spinel`, `toolchain-spinel`, `compare-spinel`) |
| `runtime/ruby/`, native `runtime/spinel/`, or a focused Spinel test | Advisory native core plus the relevant focused Spinel framework suite |
| Interpreter-only `db_jruby` / `markly_jruby` or `scaffold/ruby_overlay` | Interpreted Ruby/JRuby owners, not native fanout |
| Spinel scaffold packaging | Advisory native core plus the native archive smoke |
| Shared emitter helpers (`src/emit/shared/`) | Full validation across target languages |
| Shared analyzer or lowerer | Compact floor; no mandatory Spinel lane on an ordinary PR; reviewers can request `ci:full` for broad/risky changes |
| `wasm/` | WASM build and IDE/playground/studio browser verification |
| Site/guide sources | Site/archive build and WASM verification, without publishing |
| Shared compare, framework, archive, or E2E harness | The checks owned by that harness |
| Proven body-only edits in `src/project.rs`'s interpreted Ruby/JRuby builders | Ruby/JRuby comparison and archive smokes, plus Writebook inventory |
| Proven body-only edits in its shared Ruby/Spinel builders | Ruby/JRuby owners plus all native Spinel/Campfire consumers and Writebook inventory; no unrelated target or WASM fanout |
| Cross-target packaging, CI policy/workflows/planner, Cargo/build/toolchain policy, unknown new target | Full validation |

Project assembly is narrowed only when the base and event trees differ solely
inside the bodies of `ruby_runtime_files`, `jruby_runtime_files`,
`ruby_family_runtime_files`, `spinel_files` or `spin_shape`. Signatures and
every byte outside those bodies must remain identical. Shared helpers,
constants, dispatch, new/deleted functions, mode changes and unrecognized
source shapes still select full validation. This deliberately conservative
recognizer is not a Rust parser: raw strings (`r`, `br`, `cr`) within builder
bodies and block comments retain full coverage. Other changed paths and
`ci:full` can still expand the combined plan; no last-commit or PR-title inference
is used.

### Requesting broader or fresh validation

For broad/risky changes, or when targeted coverage is insufficient, request
**`ci:full`**. Applying labels requires upstream repository triage access or
higher; fork contributors and their agents without that access should ask a
maintainer to apply the label. A request in a PR comment alone does not trigger
CI. The label must exist in the upstream repository before it can be applied.

- The PR must be **ready for review**: drafts retain fixture + unit only,
  even with `ci:full`.
- Applying the label starts a new full-matrix run of the current **PR merge
  tree**, without a new commit. Superseded PR runs are cancelled.
- Further pushes retain full coverage while the label remains set.
- Removing it starts a run with automatic coverage selection. Policy/shared
  emitter changes can still select full coverage without the label.
- Full **coverage** does not disable conservative PR execution-receipt reuse.
  For fresh execution, ask a maintainer to select **Re-run all jobs** on the
  full-matrix run. A rerun retains that run's original SHA and event coverage;
  rerunning an older compact run does not expand coverage or test a newer head.

A manual **Actions → Full validation → Run workflow** dispatch validates the
chosen ref freshly. A branch-head dispatch is not a replacement for the PR
merge-tree check. Leave **publish** unchecked for validation only.

### Validation is not publication

`full` selects coverage; `publish` separately authorizes publication work.
Applying `ci:full` never enables `publish`.

| Trigger | Publication behavior |
|---|---|
| PR, including `ci:full`, or ordinary main push | Validation/artifacts only; no deploy |
| Manual Full validation, `publish=false` (default) | Fresh validation only; no deploy |
| Manual Full validation, `publish=true` | Accepted only on canonical `rubys/roundhouse` main; guarded publication |
| Four-hour schedule on canonical main | Fresh full validation plus guarded publication |

The shared validator and PR jobs have no Pages/OIDC deployment privileges and
no deploy job. Only the separate full-workflow deploy job receives those
permissions. Publication requests outside canonical main and schedule/manual
events are rejected. Publication still requires the compact floor, verified
assembly and a live-main SHA check; the detailed archive contract follows below.

### Current-run artifacts and the full cycle

Targeted archive smokes use `roundhouse --archives rust,go` (selected
`browse/<target>.{json,tgz,zip}` outputs) and do not build WASM, demos, website
assets or unrelated archives. The same archive writers power `--site`, which
keeps the complete developer-facing build. Full publication and smoke consume
the same producer bytes, without later re-emission.

`.github/workflows/full-ci.yml` checks canonical main at **00:17, 04:17,
08:17, 12:17, 16:17 and 20:17 UTC**, bundling full validation and publication.
Every cycle executes freshly; validation results are not cached. Spinel master
is resolved once per run, and evidence records the compiler revision actually
used. Its lanes remain advisory. Ordinary build caches and the conservative PR
execution receipts described below are unchanged.

Archive producers upload with `always()`, preserving any files already produced
if a later producer step fails. The outcome report names the source SHA, run and
attempt, actual byte size/hash, and applicable Spinel provenance, and classifies
each archive `passed`, `reused`, `failed`, `unverified`, or `not-selected`.
Validation is byte-specific: a TGZ smoke does not certify sibling ZIP or JSON
bytes. A Spinel source archive contains source, not a built compiler; its native
smoke witness therefore records the compiler revision separately. Likewise a
Docker producer's compiler revision is distinct provenance from a native
archive consumer's compiler revision.

Publication requires the **compact** floor and assembled site, not passing all
extra-target or advisory lanes: failed or unverified archives can still be
useful repros, but are not described as validated. Assembly waits for the same
run's outcome report, copies only that run's archives, and verifies the actual
copied bytes against the report hashes. It never rebuilds from a newer main.
The published sidecar at `ci/archive-results.json` separately records whether
each archive is present for download. Older-attempt witnesses remain visible
but are conservatively `unverified` on a rerun, not evidence that it passed.
Deployment guards are unchanged: the deployment lock rechecks that canonical
main still equals the validated SHA; lookup failure or a superseded snapshot
prevents publication. A new main commit racing the final check cannot be made
atomic with Pages deployment.
Started background runs finish; only the newest pending request is retained.
Extra comparison/smoke matrices use `max-parallel: 2`, GC uses 1. These bound
fanouts, **not** total repository concurrency or a guaranteed PR runner priority.

## Reuse within selected PR checks

`scripts/ci-reuse.py` can reuse **executed, successful** checks from the same
pull request. The allowlist is `store-check`, `writebook-inventory`, Rust archive
smoke, SharedWorker browser smoke, and the Rust inflector framework suite.
Unit tests, fixture generation, artifact producers, DOM comparisons, other
selected toolchain lanes and all selected main-branch checks execute freshly.
Routing changes the required coverage, not the execution-receipt trust rules.

## What must match

For the two source checks, the fingerprint combines:

- The actual checkout's Git merge-tree entries: paths, modes and blob IDs. It
  includes compiler sources, every runtime, build configuration, lockfiles,
  scripts, workflow definitions, shared test support, fixtures and unknown
  paths. Only unrelated top-level `tests/*.rs` integration-test binaries are
  excluded. Writebook's own `tests/writebook.rs` remains included.
- Every file, directory, permission and byte in the actual downloaded
  Writebook or generated store tree. Tar/file modification times are not
  inputs; source text, migration names and generated credentials are **not**
  normalized. Unsupported file types or symlinks disable reuse.
- The observed hosted runner image/architecture, installed Rust/Cargo/C
  compiler versions, build flags and downloaded action code. This compares
  actual resolved inputs, not the strings `stable` or `ubuntu-latest`.

Both commands use `--locked`. They run Rust analysis/emit inventory, not Rails,
external Ruby, or a target-language runtime, and install no apt/gem/npm
dependencies. The native binary's commit stamp is provenance, not an input to
these checks: neither check exercises `--version` or MCP server information.
SHA-stamped WASM and its consumers are **not** eligible.

Any shared compiler change invalidates both jobs. A change only to an unrelated
Rust integration test can reuse Writebook. Store reuse will often miss because
fresh Rails generation changes its actual contents; that is intentional, not a
reason to pretend identical generator scripts produced identical inputs.

## Downstream consumers

These checks use **fresh output identity**, not producer source identity. The
producer always runs; a compiler edit can reuse a target's validation only if
its actual consumed output is identical. Harness, workflow, Cargo configuration,
shared test support and installation-policy changes still invalidate reuse.
Python 3.11+ is required for parsing the generated Cargo manifests and locks.

| Consumer | Preparation before lookup | Validation on a miss |
|---|---|---|
| `smoke (rust)` | Resolve Cargo and E2E npm dependencies in a **separate** archive extraction; install the README's Chromium payload | Extract the original archive again, execute its README blocks unchanged, and retain the existing test-count floors |
| `browser-smoke-typescript` | Fresh emission, generated npm dependency resolution, Vite build, harness/browser installation | Existing `npm run test-only`, including fresh Vite preview and all browser tests |
| Rust inflector in `compare (rust)` | Fresh framework ingestion/emission and `cargo generate-lockfile` | Inner `cargo test --locked`; require an actual successful emitted inflector test, not just hand-written runtime tests |

Consumer fingerprints include emitted source/configuration, generated locks,
and (for SharedWorker) **built `dist` bytes**. Rust archive identity also includes
the original `.tgz` bytes. Browser consumers include applicable installed npm
trees, actual Node/npm versions, installed OS package revisions, sqlite3,
and the complete explicitly selected browser payload. Only root-level browser
`.links` install/GC bookkeeping is excluded; browser executables, resources and
installation markers remain inputs. Source symlinks still disable reuse;
installed tools may contain only witnessed, internal file symlinks.

Before issuing any receipt, recompute its complete witness, including external
source trees and the environment for Store and Writebook. Rust smoke
compares the **pristine validation extraction's** resulting locks, modules and
source against the resolver extraction, never copying prepared dependencies
into validation. Resolution drift or unexpected source changes suppress the
receipt, not the real test result. This preserves README coverage: a missing
install command is not rescued by preparation. Like the existing smoke lane,
this proves README execution with prepared system/browser prerequisites, not a
completely cold browser install.

Rust smoke accepts only the audited Build/Setup/Test/E2E command shape and
pinned Playwright manifest. Dependency preparation uses `scripts/smoke
--extract-blocks` so it audits the same parser as real README execution.
Unknown README commands, external/path Cargo
sources, global Cargo configuration, external database/server overrides,
browser/test-selection overrides or unsupported file types execute normally.
Dependency preparation remains real work even on a hit; the saved work is
compilation and validation, not fabricated freshness. This assumes ordinary
trusted package-manager/CI behavior, not attestation against transient side
effects or malicious workflow authors.

Receipts are scoped to the exact physical matrix job as well as the logical
consumer. Inflector reuse never suppresses the Rust DOM comparison or other
framework suites. Its receipt is created only after **inner actual execution**;
the outer harness returning success on a hit cannot create a new receipt.

### Remaining downstream lanes — audited, not blanket-skipped

| Family | Why it still executes / prerequisite for safe reuse |
|---|---|
| Other archive smoke targets (Crystal, Kotlin, Swift, C#, TypeScript, Go, Elixir, Python, Ruby/JRuby) | Each README resolves a different dependency closure during execution. Capture its fresh resolved closure and toolchain, preserve pristine README validation, and compare the validation witnesses before enabling each lane. |
| DOM compare matrix, Ruby/JRuby compare | Also consumes the live Rails oracle, Bundler closure, generated assets and target dependency resolution. An unchanged transpiled archive alone is insufficient. |
| Other framework/toolchain suites | Emit projects internally and resolve dependencies during testing. Need per-harness output boundaries and actual-execution floors, not a producer-source filter. |
| IDE/WASM browser | WASM intentionally embeds the current commit SHA. Do not normalize away a changed consumer input to force a hit. |
| Spinel framework/toolchain/archive lanes | Need fresh emitted source plus the actual unpinned Spinel binary, `spin` package closure, C toolchain and native-library identities. Advisory failures remain signals, never reusable success. |
| Campfire CRuby/Spinel compare, model differential and conformance | Include pinned Campfire source, Rails/gem oracle closure, assets, Redis/DB scenarios, native packages, Spinel binary and GC mode as applicable. Current harnesses still resolve mutable external inputs. |
| Campfire Docker smoke | Build the current Docker context and resolve its mutable base/apt closure before fingerprinting the actual runnable image. Archive identity alone cannot certify that image. |
| Site/archive producers, assembly and publication | Must produce current-run outputs and provenance; site content also fetches live bench data. They are not reused validation results. |

## Evidence, not just a green run

After actual validation succeeds, the job uploads an execution receipt for its
exact run attempt. Writebook's receipt bundle includes the two generated
reports. A future run checks the receipt's fingerprint and PR/repository/
workflow/job identity, then independently queries GitHub's attempt-specific
jobs API to verify the completed job and every required validation step.
Raw step outcomes must also have been successful. A successful job in an
otherwise failed or cancelled workflow can be reused; failed, skipped,
cancelled, neutral, ambiguous or masked failures cannot.

On a hit, the job's summary links the original executed job. Writebook reports
are restored as bounded, explicitly named **data**, then uploaded under the
normal artifact name in the current run. No old executable or built binary is
restored. A reused job never produces another execution receipt; there are no
chains of "passed because an earlier job was reused."

Receipts expire after seven days. Lookup examines at most 20 recent runs of
this PR's branch, paginates artifact/job lists and has a 90-second API budget.
Missing/expired/malformed evidence, API permission failures, timeouts or absent
environment metadata execute the original check. Receipt bookkeeping cannot
fail validation. This uses ordinary `pull_request` with job-local
`actions: read`, never privileged `pull_request_target`. Receipts do not contain
tokens or credentials and are not accepted across PRs or into main.

This is ordinary GitHub CI provenance, not cryptographic attestation against
an author who can modify the PR's workflow itself.

## Further load reductions and cancellation

Superseded PR runs still cancel; main runs that have started still finish.
Completed successful jobs with receipts remain reusable even if another job
later fails or the overall run is cancelled. An unfinished or cancelled job is
not proof of successful validation. Keeping old full runs alive indiscriminately
would add concurrent work without proving the newest merge-tree inputs.
Stage-aware cancellation needs a separate controller to inspect the previous
run; GitHub's concurrency expression cannot inspect that run's job progress.
This PR does not add privileged cancellation machinery.

A separate measured follow-up may evaluate sharing the `unit` job's Debug
binary with Campfire conformance/compare. This change does not copy Cargo target
directories, share images, introduce Bazel, or claim a measured queue-time
improvement. Any sharing proposal still needs compatible toolchains/build flags,
native-library ABI, current-commit provenance, and proof at each consumer.

Matrix `fail-fast: false` still preserves cross-target diagnostic coverage;
advisory lanes remain signals rather than reasons to cancel independent checks.
Receipt reuse does not bypass selected checks' dependency or failure handling.
The Campfire Docker recipe no longer downloads a separate Dockerfile frontend:
its ordinary multi-stage instructions use the bundled frontend and COPY
preserves the archive's executable boot mode. Base images and apt packages
still resolve freshly, and the real Docker smoke remains enabled.

## Forcing a fresh check and extending the allowlist

Rerun checks bypass matching receipts. Choose **Re-run all jobs** to execute
the entire selected graph freshly, rather than only failed or individual jobs.
The rerun keeps the original SHA, ref and coverage; the fresh attempt may
produce new evidence. Draft, `needs` and main-branch behavior is unchanged.

Before adding another job, audit its complete input contract, including
generated artifacts, framework tests, executable README blocks and the actual
versions of external packages/toolchains it resolves. An unresolved mutable
input makes it ineligible. Do not substitute a PR-wide changed-files filter or
the last head-commit diff for merge-tree identity. Keep artifact producers fresh
unless current-run outputs and their provenance can be preserved honestly.

Run `python3 -B tests/ci_reuse_test.py -v` and
`cargo test --test workflow_yaml_parses` when changing this policy. The Rust
workflow tests execute the Python adversarial suite, so normal unit CI gates it.
For coverage routing, archive evidence or full-workflow changes, also run
`cargo test --test ci_policy_workflow`; it executes the planner and archive
evidence suites and checks the publication boundaries.
