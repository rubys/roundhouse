# Campfire on Rust: compile-to-working plan

**Status:** baseline not yet reproduced; plan established; feature work is gated
on an exact, repeatable compiler-error inventory.

**Scope:** the ONCE Campfire revision pinned below, emitted as a Rust project by
Roundhouse. “Rust compiles” and “Campfire works” are deliberately separate
milestones. The first is necessary, not sufficient, for the second.

**Tracking rule:** check a box only with the evidence named in that item or in
the acceptance matrix. Record the Roundhouse SHA, Campfire SHA, toolchain,
commands, actual executed counts, logs/artifact links, and result in the
[evidence ledger](#evidence-ledger). A patch existing, a diagnostic count
falling, or a test being selected is not completion evidence.

## Current snapshot

| Input | Snapshot / status |
|---|---|
| Roundhouse PR | [#688, Draft](https://github.com/rubys/roundhouse/pull/688), head `3b6d1b7576036382f82aa936fef8bcbd5b65272c` when this plan was authored |
| Campfire | CI pin `32b4144b5206304fa8d4c67455a753e2d3c16635`; no checkout is present in the current orb |
| Historical compiler-error estimate | ~2,468, previously reported, but **not verified** against the current pin/head; do not use as the baseline or progress denominator |
| Rails oracle | Not prepared in the current orb. It is required for Rails equivalence, not for collecting rustc diagnostics |
| Local Rust toolchain | `cargo` and `rustc` are installed; record full versions/host from the actual baseline run |
| Current exact-head CI observation | Run [37986454230](https://github.com/rubys/roundhouse/actions/runs/37986454230), head `3b6d1b7576036382f82aa936fef8bcbd5b65272c`: Rust compare and Campfire compare/conformance passed in the last inspection; Rust smoke was still in progress; Campfire browser smoke was skipped by the focused selection. Recheck before relying on this snapshot |
| Current end-to-end compiler inventory | Not available. Full strict generation and Cargo diagnostics have not yet been captured from the pinned app in this orb |

The PR already contains substantial Rust Campfire work, including app/helper
class emission, namespace-aware controller structure, reachable inherited
methods, request-context and `process_action` wiring, and explicit unsupported
route handling. Treat historical labels such as “app classes missing,”
“cookies,” “capture,” and “STI helper” as **questions to re-triage**, not as a
current residual-error inventory.

## Non-negotiable correctness rules

- Zero **error** diagnostics is the support boundary. Removing an error claims
  that the emitted behavior works; do not silence the diagnostic to make the
  output compile.
- Put Rails/framework behavior once in typed `runtime/ruby/` or shared
  `src/lower/`; use Rust emission/runtime glue for Rust-specific representation
  and native boundaries, not a second Rails implementation.
- Preserve evaluation order, short-circuiting, once-only evaluation, mutation,
  ownership, and request isolation. Do not “fix” errors by dropping code,
  widening everything to `Value`, cloning every receiver, adding unchecked
  defaults, disabling generated tests, or adding broad ignores.
- Keep unsupported behavior explicit. `todo!`, no-op/default behavior, and
  HTTP 501 routes are not successful implementations merely because rustc
  accepts them.
- A check is evidence only for the input, generated files, target kind, and
  behavior it actually exercised. Record skipped, ignored, selected, and
  executed tests separately.

## Phase 0 — Freeze scope and establish inputs

- [ ] **P0.1** Fetch/check out ONCE Campfire at exactly
  `32b4144b5206304fa8d4c67455a753e2d3c16635`; verify `git rev-parse HEAD` (or
  verify the archive’s recorded pin and content manifest when there is no
  `.git`). Do not silently substitute Campfire `main`.
- [ ] **P0.2** Record Roundhouse SHA, clean/dirty status, `rustc -vV`, Cargo
  version, host/target, OS, root and generated `Cargo.lock` hashes, compiler
  binary provenance/hash, relevant environment/feature flags, and the exact
  command line. Pin Rust to the PR’s declared **1.98.1** initially; any
  intentional toolchain change is a separately reviewed plan update.
- [ ] **P0.3** Record generated-file hashes and module/class/route/test census.
  Keep credentials, session cookies, and authentication state out of logs and
  committed artifacts.
- [ ] **P0.4** Prepare the Rails oracle separately for comparison work. Record
  its Campfire pin and pristine database state. Do not block compiler-only
  inventory on Redis/oracle setup.
- [ ] **P0.5** Run the production CLI path into fresh directories, separately:
  strict `roundhouse check "$APP"`, strict Rust project generation, and (only
  for inspection) `--allow-unsupported` generation. Preserve the strict
  diagnostics. At this PR head `--allow-unsupported` changes diagnostic
  severities, so its warning count or exit status is not a zero-error proof.
- [ ] **P0.6** If project generation refuses before producing the app, record
  that exact refusal and stop calling the result a rustc baseline. Do not
  bypass a project-level gate and present direct `rust::emit` output as the
  production CLI path.
- [ ] **P0.7** Generate twice from the same inputs and compare normalized
  diagnostic fingerprints and generated-file census. Explain any
  nondeterminism before assigning feature work.

## Phase 1 — Build the rustc inventory

- [ ] **P1.1** Run Cargo in the generated project against the preserved lockfile
  and record each lane independently:

  ```sh
  cargo check --locked --lib --bin app --message-format=json
  cargo check --locked --all-targets --message-format=json
  cargo test --locked --no-run --message-format=json
  ```

  Add `cargo build --locked --release` once check gates make it practical.
- [ ] **P1.2** Parse Cargo JSON diagnostics (not rendered-text grep) into a
  machine-readable inventory. Preserve full message, error code when present,
  package/target kind, generated path, enclosing item, primary and child spans,
  expansion data, and useful source excerpt.
- [ ] **P1.3** Define stable fingerprints using target kind, generated module /
  enclosing item, compiler code, and normalized message/operation. Keep exact
  line numbers in raw evidence, but do not make line numbers the identity.
  Normalize temp paths only; retain meaningful type and signature details.
- [ ] **P1.4** Separate application errors from third-party/dependency/build
  environment failures; record whether compilation completed and its exit
  status so partial output, network failure, or termination cannot look like
  improvement.
- [ ] **P1.5** Manually verify each cluster with representative generated code
  and trace it back through Ruby source → typed IR → lowered library IR → Rust
  signature/import/runtime. Mark causes **confirmed** or **suspected**; retain
  unclassified errors rather than forcing a category.
- [ ] **P1.6** Publish baseline raw counts by lane plus root-cause cluster
  membership. Do not report only total errors, since a foundation fix can
  expose additional methods and increase the total while improving coverage.

## Phase 2 — Triage and dependency waves

Re-sort this table after the baseline. The listed topics are candidates from
prior work, not promises that they remain the dominant errors.

| Wave | Work | Exit condition |
|---|---|---|
| 0 — evidence | Pinned input, strict CLI behavior, repeatable Cargo JSON inventory and source mapping | Baseline can be rerun and fingerprints compared |
| 1 — structural producers | Project assembly, emitted modules/classes, namespace/import resolution, constructors, inherited dispatch, method registries | Representative producer failures fixed; downstream cascades re-inventoried |
| 2 — independent semantic clusters | Rust value/coercion/ownership representation; remaining block/capture shapes; STI/polymorphic routes; cookies/session/auth only after request/class contracts are established | Each cluster has a verified cause, minimal repro, owner, semantic tests, and no overlapping central-file edits |
| 3 — whole-project convergence | Remaining production and generated-test target errors; newly exposed clusters; removal of temporary diagnostics only when behavior is complete | Strict fresh generation, production Cargo check/build, and generated tests compile |
| 4 — application acceptance | Native execution, Campfire tests, Rails differential, browser product journeys and security boundaries | Declared Rust Campfire workflows pass; remaining gaps are explicit and bounded |

### Candidate clusters to re-validate

- [ ] **Triage app/Rails classes and runtime first.** The PR already emits
  application classes; find exact missing or incorrectly typed symbols and
  distinguish producer omissions from downstream import/type cascades.
- [ ] **Triage authentication/cookies as a correctness and security cluster.**
  Do not stub cookies, signing, secret-key configuration, or session lookup:
  Campfire login depends on them. Validate malformed, tampered, expired and
  cross-purpose credentials, CSRF rejection, and no-write-on-rejection.
- [ ] **Triage capture/block failures from the actual residual forms.** Shared
  capture/block lowerings already exist. Add only the semantics actually
  missing; cover nested buffers, return/fallback behavior, output order,
  escaping/safety, and exactly-once evaluation.
- [ ] **Triage STI `link_to @record` against hydrated/persisted subtype
  behavior.** The PR already has subtype-aware route work; identify whether any
  remaining failure is route selection, model identity/hydration, helper
  typing, or something else. Include namespace, new/edit, and routeless-subclass
  cases as applicable.
- [ ] Inventory reachable `todo!`, silent/default/no-op methods, 501 routes,
  and un-emitted methods alongside compiler diagnostics. A green `cargo check`
  is not completion if reachable accepted paths still panic, return fabricated
  defaults, or refuse required routes.

### Parallel work package contract

Parallelize **confirmed root causes**, not arbitrary error codes or files.
Before a worker starts, put the repro, verified cause, expected semantics,
owned files, dependencies, focused checks, and integration check in the issue
or this plan. One worker owns shared/central files such as
`src/emit/rust.rs`, `src/emit/rust/expr/mod.rs`, `src/emit/rust/library.rs`,
`src/runtime_loader.rs`, and `src/project.rs` at a time; other workers send a
small proposed interface/change for that owner to integrate.

Potential streams after Phase 1 (activate only if inventory supports them):

- [ ] **A — structural emission and project assembly:** one writer for Rust
  project assembly/central emitter files; tests for every generated production
  module and route reference.
- [ ] **B — class/inheritance contracts:** class/lowering ownership such as
  `src/lower/rust_inheritance.rs` plus its isolated tests, coordinated with A
  before any shared registry/interface change.
- [ ] **C — Rust expression representation:** a named non-overlapping subset
  of `src/emit/rust/expr/`, type/ownership decisions, or method coercions,
  based on the actual cluster—not all E0308/E0599 diagnostics as one package.
- [ ] **D — shared capture/block behavior:** only after the residual syntax is
  proven; own its `src/lower/`/`runtime/ruby/` files and semantic test files.
- [ ] **E — route/STI behavior:** shared route/model lowering and dedicated
  tests, coordinated at the integration boundary with A.
- [ ] **F — cookies/auth behavior:** runtime and isolated native/request
  boundary tests after A establishes the required request/app interface.
- [ ] **G — independent test/oracle preparation:** pinned source, Rails oracle,
  dependency/setup diagnosis and acceptance-test gap inventory; do not edit
  A–F feature files.

For each integrated batch: run its focused semantic tests, Rust real-blog gate,
`cargo check --locked --all-targets`, then fresh Campfire generation and
fingerprint comparison. Update this plan and commit the batch before starting
more work that depends on it.

## Phase 3 — Acceptance ladder

Mark each claim separately. Broader claims require all lower-level evidence;
no single green job substitutes for the rest.

| Claim | Required evidence |
|---|---|
| Baseline reproducible | Exact inputs/toolchain/locks, generation result or explicit refusal, raw JSON/logs, repeatable fingerprints, module/class/route/test census |
| Cluster fixed | Minimal repro fails before and passes after; root cause traced; semantic regression executes; fresh full-app inventory rechecked |
| Supported source analyzes cleanly | Strict analyzer has zero errors and applicable unresolved-type gates pass; warnings remain separately accounted |
| Rust generation is strict-clean | Full production CLI Rust generation succeeds without `--allow-unsupported`; project/lowering/emitter errors are included |
| Runtime behavior is shared and typed | Runtime-method typing integration passes; runtime/source tests pass; emitted target executes the case; `tests/emit_and_run.rs` covers removed errors where applicable |
| Production Rust project compiles | Fresh generated app passes locked lib+bin check and release build; expected modules/routes remain present |
| Generated test surface compiles | `--all-targets` check and `cargo test --no-run` pass with test generation enabled; report selected, generated, ignored, and executed counts |
| Campfire own tests run on Rust | A Rust-capable lane actually executes named translated Campfire tests and records selected/retained/executed/passed/failed/ignored counts and floors. Existing Ruby/Spinel `campfire-suite` does not satisfy this |
| Rails differential passes | `scripts/campfire-compare --target rust` passes both Rails/emitted walks and all enabled page/frame comparisons from the pinned pristine state; record masks/job posture |
| Browser journeys pass | `scripts/campfire-e2e --target rust` builds the fresh emit and actually runs Playwright journeys; verify assets/network/console, first-run, login/logout, rejected requests, product journeys and two-client updates |
| Campfire works on Rust (declared scope) | All applicable gates above pass; route/feature scope and remaining unsupported behavior are explicitly listed; no accepted behavior is replaced by a stub/501 |

`tests/rust_toolchain.rs` is useful but its helper directly calls
`rust::emit(&app)`. It does not by itself prove the production CLI, shared
post-analysis lowering, project assembly, or a complete Campfire app. Keep
both focused harness evidence and full CLI evidence.

## Phase 4 — Revisit, update, and close items

- [ ] After each integrated worker batch, update cluster fingerprints, status,
  owner, evidence, newly exposed work and dependencies in this plan.
- [ ] Re-plan at every dependency-wave boundary, and immediately if a cause is
  disproved, a shared ABI/pass order changes, workers contend for a central
  file, compilation success reveals runtime failure, or the Campfire/toolchain/
  lockfile pin changes.
- [ ] Before calling the compile milestone, review whether any error reduction
  came from lost modules/methods/routes/tests, diagnostic downgrades, or
  unsupported defaults; confirm strict generation and all production/test
  Cargo lanes on a fresh tree.
- [ ] Before calling the working-app milestone, run the acceptance ladder and
  audit negative authentication/request-isolation behavior. Record residual
  unsupported cases instead of expanding the claim.
- [ ] Keep this PR Draft unless Thomas explicitly instructs otherwise. A green
  compile, completed checklist, push, or validation run does not authorize
  marking it ready or merging.

## Evidence ledger

Append one row per baseline or integration batch. Link logs/JSON as CI or
review artifacts; avoid committing large generated projects or sensitive data.

| Date | Roundhouse SHA | Campfire SHA | Toolchain / locks | Commands and executed scope | Result / artifact links | Checklist updated |
|---|---|---|---|---|---|---|
| 2026-10-09 | `3b6d1b7576036382f82aa936fef8bcbd5b65272c` | `32b4144b5206304fa8d4c67455a753e2d3c16635` | PR pin 1.98.1; generated lock/baseline not captured | Exact-head CI run `37986454230`: Rust compare, Campfire compare/conformance passed at last inspection; Rust smoke in progress; Campfire browser smoke skipped | No fresh Campfire rustc inventory; historical ~2,468 estimate remains unverified | P0–P4 open |

### Reproduction command template

Fill in the actual paths and save the complete output in the evidence
artifact—not just this command template—before claiming a baseline:

```sh
APP=/path/to/once-campfire-at-32b4144b5206304fa8d4c67455a753e2d3c16635
OUT=/tmp/campfire-rust-<roundhouse-sha>
roundhouse check "$APP"
roundhouse --target rust "$APP" -o "$OUT"
(cd "$OUT" && cargo check --locked --lib --bin app --message-format=json)
(cd "$OUT" && cargo check --locked --all-targets --message-format=json)
(cd "$OUT" && cargo test --locked --no-run --message-format=json)
```

If strict generation refuses before producing `$OUT`, preserve that result,
triage the refusal at its source, and do not substitute a weakened run as the
baseline.
