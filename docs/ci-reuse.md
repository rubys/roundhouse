# Conservative PR-local CI reuse

`scripts/ci-reuse.py` can reuse **executed, successful** checks from the same
pull request. The allowlist is `store-check`, `writebook-inventory`, Rust archive
smoke, SharedWorker browser smoke, and the Rust inflector framework suite.
Unit tests, fixture generation, artifact producers, DOM comparisons, other
toolchain lanes and all main-branch checks still execute freshly.

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

The next substantial opportunity is sharing **current-run** native compiler
builds, not declaring more outputs unchanged without a dependency witness.
In [run 36998514579](https://github.com/rubys/roundhouse/actions/runs/36998514579),
the unit Cargo step took 309 seconds and the two TypeScript toolchain/framework
steps took 210 seconds together. Those are whole-step timings, not measurements
of compiler cost alone. A shared build needs consistent profiles/toolchains,
native-library ABI, current commit provenance and coverage of each consumer's
actual build flags. It should be measured and proved separately.

Matrix `fail-fast: false` still preserves cross-target diagnostic coverage;
advisory lanes remain signals rather than reasons to cancel independent checks.
Existing `needs` gates and normal step failure handling are unchanged. The
Campfire Docker recipe no longer downloads a separate Dockerfile frontend:
its ordinary multi-stage instructions use the bundled frontend and COPY
preserves the archive's executable boot mode. Base images and apt packages
still resolve freshly, and the real Docker smoke remains enabled.

## Forcing a fresh check and extending the allowlist

GitHub's **Re-run jobs** always executes these checks freshly, even if a matching
receipt exists. The fresh attempt may produce new evidence. Existing draft,
`needs` and main-branch behavior is unchanged.

Before adding another job, audit its complete input contract, including
generated artifacts, framework tests, executable README blocks and the actual
versions of external packages/toolchains it resolves. An unresolved mutable
input makes it ineligible. Do not substitute a PR-wide changed-files filter or
the last head-commit diff for merge-tree identity. Keep artifact producers fresh
unless current-run outputs and their provenance can be preserved honestly.

Run `PYTHONDONTWRITEBYTECODE=1 python3 tests/ci_reuse_test.py -v` and
`cargo test --test workflow_yaml_parses` when changing this policy. The Rust
workflow tests execute the Python adversarial suite, so normal unit CI gates it.
