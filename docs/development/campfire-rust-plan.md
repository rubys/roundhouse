# Campfire on Rust: compile-to-working plan

**Current working state (2026-10-10, latest local verification):** PR #688 is
OPEN and Draft at published head `74dce7306cdc70eb3af7f4068a8c7af677d894a2`,
base `main` (verified with `gh pr view`). The reported status rollup contains
CodeRabbit SUCCESS only; there are no required workflow checks on this head.
Do not mark ready or merge. Canonical main `aaff26906c66300a7ff0a3d1b1d96e7a391786e9`
has been fetched and is merged in the local worktree, but the merge commit has
not yet been created. The worktree has staged and unstaged changes; no current
local changes are validated by GitHub CI. `cargo check --locked --all-targets`
passes. Focused Rust iteration tests pass 3/3, including a custom `empty?`
predicate regression; Cache-Control integration tests pass 4/4, with one
native-Spinel test explicitly ignored, and the runtime unit file passes 40 runs
/ 48 assertions. The ignored real-blog Rust Cargo gate
passes 1/1, compiling and executing its generated project after correcting
CacheControlStore constructor initialization, custom predicate dispatch, and
optional-array iteration. These fixes restore the real-blog gate; they do not
reduce the Campfire compiler wall. On the pinned Campfire revision
`edbc779f4dfc9b26c36310881711ddc976a9dfc8`, fresh strict analysis is 0 errors /
460 warnings. Fresh strict Rust generation exits 1 with 132 unsupported/syntax
+ 113 type errors (245 frontend diagnostics) and refuses `Data.define` before
writing a project. Therefore **there is no current-pin Campfire Cargo/rustc
error count**. The strongest comparable result remains historical: 2,577
errors / 575 warnings / 2,339 fingerprints on older Campfire `32b4144b` at
Roundhouse `863dd24b`; do not report it as a current count. Next is the narrow
nominal `ContentKey` Data factory path through the real heterogeneous cache-key
consumer and `is_a?`; keep the generation gate until emitted behavior is
compiled and executed.

**Historical snapshot (superseded 2026-10-10):** PR #688
was verified OPEN and Draft at `83be1d4054d5e2ae4f289d0409f382188d3a6aff`, based on
`main`; the local branch includes canonical-main merge `030052820334dbcc9883f4763c43cd6e3e2f3e39`.
Do not merge or mark ready. The exact-head full CI run
[38063332818](https://github.com/rubys/roundhouse/actions/runs/38063332818)
is red: unit shards 0 and 2, Rust compare, Rust smoke, compact-required, and
the overall summary failed; Campfire compare and conformance passed, while
Campfire smoke and several other lanes were skipped. CodeRabbit's SUCCESS
context says review was skipped because this is Draft, so there is no current
substantive bot review. Local targeted checks now pass for the previously red
Data-block method journey (`data_define_block_methods_belong_to_the_data_class`),
the Data factory nominal/member inference test, the corrected ActionController
emitter assertion, and `cargo check --locked --all-targets`. The Data failure
was caused by a synthesized Data member reader harvesting an unknown
constructor argument plus the synthetic nil-initialized ivar as `Var | Nil`;
the analyzer now leaves that reader's pre-seeded gradual return intact only
when the harvested return has no informative core beyond Nil. Concrete call-site
types remain harvestable. The prior current-pin count (132 unsupported/syntax
 113 type diagnostics = 245) was reproduced after the Data inference fix at
the current code state; the fix repairs the focused Data-method failure but
does not move the strict-generation boundary or reduce that count. There is
still no verified Cargo-error count for Campfire pin
`edbc779f4dfc9b26c36310881711ddc976a9dfc8`. The CI-generated real-blog Rust
source contained stale `CacheControlStore`/`self.extras` code absent from the
current tracked runtime and fixture. The exact ignored Rust test passes at
`c20ec808` in a detached clean worktree with a fresh `CARGO_TARGET_DIR` and
the exact CI fixture artifact (SHA-256
`6199e048b38b866790c5956868b9e1960497e2ddded3d2b67e6b994cad06350a`). The
compare lane's `emit_preview` plus generated release-app build also passes;
the resulting local `action_controller_base.rs` contains neither stale name.
The CI failure is therefore not reproduced; its generated source/build-input
provenance remains unresolved. Code
fixes are published in `756ec909`; `83be1d40` is the subsequent docs-only
refresh. The check snapshot at `83be1d40` had only CodeRabbit SUCCESS, whose
description explicitly says review was skipped because the PR is Draft; no
workflow check-runs had posted at that time. Next: compare CI's restored
fixture/build inputs with the local clean-target reproduction, then continue
the compiler inventory by source-level root cause. Keep the PR Draft and do
not merge.

**Historical snapshot (superseded 2026-10-10):** PR #688 was OPEN and Draft at published head
`f228175fde7d8dacbf7bbbbe3045b986a6e92f28`, based on `main`; no merge or
ready transition is authorized. Canonical `main` at `5d3d144e` is merged
locally in `863dd24b`; a later canonical-main integration is present in local
merge commit `02938da8` (base `c210f226`). The working tree currently has
uncommitted edits to the Rust `HeaderStore` String ownership fix, its emitter
regression assertion, and this plan; the published head and CI results do not
include them yet. Local targeted tests and `cargo check --locked --all-targets`
pass for these edits, but the Campfire database differential has not been
rerun. The current
Campfire pin is `edbc779f4dfc9b26c36310881711ddc976a9dfc8`. A fresh strict Rust
generation attempt at the published head exits before writing project files:
it reports 132 unsupported/syntax and 113 type errors (245 total), then
refuses `Data.define`. The allow-unsupported survey path also stops at that
same project gate. There is no Cargo error count for this current pin. The
latest required `ci:rust` run [38049749293](https://github.com/rubys/roundhouse/actions/runs/38049749293)
is **red**, not a green baseline; see its exact-head failures in the current
snapshot and newest evidence row. The last comparable emitted-project survey
remains older Campfire `32b4144b` at `863dd24b`: 2,577 errors / 575 warnings /
2,339 fingerprints. The exact older-pin source at `898b4606` had 2,599 / 572 /
2,361; the changed inventory removes 23 groups/errors and adds one, for 22
fewer errors overall after the mainline integration. Do not attribute this
cross-commit movement to the cookie-lifecycle harness. The next design gate
is nominal Data identity through Campfire's heterogeneous flattened cache-key
consumer; implementing a Data struct alone is not enough to remove the
generation refusal. Then execute Rails config/header and unsigned-cookie
slices.

**Current local integration (2026-10-10):** local merge commit
`030052820334dbcc9883f4763c43cd6e3e2f3e39` joins PR parent
`ee9a718f077b19d0651266d634045031cd2f156f` and canonical `main`
`9249df4dce36dbc33401fc79bb90f41b34537c2c`. The merge had no unresolved
conflicts. Four follow-up files remain modified: this plan, the Rust model
insertion shim and assertion, and the view partial inference correction.
`cargo check --locked --all-targets` passed after the merge; focused tests
passed for Data factory analysis (15), route block constraints (7), route
segment matching (5), view ivar/local collision (3), `real_blog` (6), and the
ignored emitted real-blog Rust Cargo gate (1). The separate ignored
forwarded-block Cargo regression was rerun and still fails with five generated
Rust errors; it is explicitly not a passing check. On Campfire
`edbc779f4dfc9b26c36310881711ddc976a9dfc8`, strict Rust generation reports 132
unsupported/syntax plus 113 type diagnostics (245 total), then refuses
`Data.define` before writing a project. Current-pin Cargo errors therefore
remain unavailable. The 2,577-error / 575-warning / 2,339 fingerprint
measurement is historical, for Campfire `32b4144b` at Roundhouse `863dd24b`;
it must not be compared directly with the current 245 front-end diagnostics.
The local integration and follow-up fixes have not been pushed.

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
| PR status | [#688, Draft](https://github.com/rubys/roundhouse/pull/688), OPEN, base `main`, published head `74dce7306cdc70eb3af7f4068a8c7af677d894a2` (verified with `gh pr view` on 2026-10-10). The reported rollup contains CodeRabbit SUCCESS only; no required workflow checks are reported on this head. Do not treat this as substantive review or current CI validation. Keep Draft and do not merge |
| Local integration base | Canonical `main` `aaff26906c66300a7ff0a3d1b1d96e7a391786e9` is merged locally; `MERGE_HEAD` remains present while conflict resolutions and follow-up edits are staged/unstaged. The merge commit is not yet created. `cargo check --locked --all-targets` and the emitted real-blog Rust Cargo gate pass on the working tree; see the newest ledger row |
| Campfire target | Current canonical CI pin `edbc779f4dfc9b26c36310881711ddc976a9dfc8`; earlier measured comparison pin `32b4144b5206304fa8d4c67455a753e2d3c16635` |
| Strict analyzer | On current pin `edbc779f`, `roundhouse check --strict` exits 0 with 0 errors / 460 warnings. This does not imply Rust generation or Cargo success |
| Strict Rust generation / current pin | Fresh production generation exits 1 before writing files after reporting 132 unsupported/syntax and 113 type errors (245 total), including `FragmentCache::ContentKey = Data.define(:digest) { def cache_key = digest }`. `--survey --allow-unsupported` also exits at `rust: Data.define is not supported; use Ruby or Spinel`. This is an explicit target representation gate, not a rustc count. Current-pin Cargo inventory is unavailable until the factory and its nominal cache-key consumer have a sound Rust representation |
| Last emitted Rust project | Older pin `32b4144b` at merged Roundhouse head `863dd24b`: survey emitted 488 files and Cargo lib/bin check exits 101 with 2,577 errors / 575 warnings / 2,339 fingerprints. This remains the comparison project until `edbc779f` generation is unblocked |
| Survey-only emitted Rust | The exact `863dd24b` survey of older pin `32b4144b` used `--survey --allow-unsupported`; it emitted 488 files, including 377 Rust source files (489 files after the captured `Cargo.lock` was added for checking). Generated lock SHA-256 is `b1ae75ef5b9f85e8166d707f9b3babcc0897f6f392c09e2ab99ccd2100d93f55` |
| Historical repeatability and output census | Two pre-merge generations at `609248bc` each emitted 487 files with identical manifest `955ffa2ca632700fe2c0697c7346df29956299b97e964a6ec45a4e17e650c71b` and 2,496 errors / 2,252 fingerprints. Their detailed census is historical, not the current merged-base census. The earlier 1,918-entry post-build hash list is not used as the emitted-file census |
| Survey lib/bin compiler result | Clean prior merged base `4a70cd0` on `32b4144b`: 2,597 errors / 571 warnings / 2,359 fingerprints. Code-bearing `898b4606` on `32b4144b`: 2,599 / 572 / 2,361. Current merged `863dd24b` on `32b4144b`: 2,577 / 575 / 2,339. Comparing `898b4606` with `863dd24b` removes 23 groups/errors and adds one; net 22 fewer. This is the observed integrated-head delta, not proof those groups share a root cause or that any feature is complete |
| Survey all-target/test results | The previous 2,496-error all-target result is historical and predates the canonical-main merge; it is not a current Campfire measurement. Roundhouse source at `863dd24b` passes `cargo check --locked --all-targets`. Do not conflate this source check with generated Campfire lib/bin Cargo results |
| Compiler diagnostic distribution (older pin survey lib/bin) | E0308 822; E0599 672; E0425 554; E0433 133; E0609 71; E0277 56; E0423 49; E0061 49; remaining codes in `/tmp/rh688-863-32b-inventory.json`. The reduction in unresolved `Rails` names has largely moved into unresolved Rails method/API errors; compiler-code totals are not root causes |
| Historical estimate | ~2,468 was a prior rough figure with unknown scope. Use 2,597 as the clean merged-base `32b4144b` baseline, 2,577 as the current merged `32b4144b` comparison, and leave current-pin `edbc779f` Cargo count unavailable until generation succeeds |
| Environment | Debian 12, Linux x86_64; `rustc 1.98.1 (48a229cea 2026-09-01)`, host `x86_64-unknown-linux-gnu`; compiler binary SHA-256 `859254978c0a0402c32f949f6de0d99aee73be8d15f45aac00ae1448aac51e74`; Cargo 1.98.1 binary SHA-256 `da77c8b33849312255ccde3179198ada4c8deb370488d050286146b1d1b27e14`; Roundhouse root lock SHA-256 `206b0c494651b039351ec2a3e6232041e87e4f36232a111ea6086d4a8e8a5981`; generated lock SHA-256 `b1ae75ef5b9f85e8166d707f9b3babcc0897f6f392c09e2ab99ccd2100d93f55` |
| Rails oracle | Partial probe on `32b4144b`: standalone Action Controller dispatch loaded the version initializer and `VersionHeaders` callback; five environment cases returned 200. Full pinned Rails boot and on-wire nil `X-Rev` behavior remain unverified |
| Exact-head CI | Published head `ee9a718f` has 0 check-runs; its only status is CodeRabbit SUCCESS explicitly because review was skipped for a draft. The red `ci:rust` observations on `f228175` / `0b2323e`, skipped Campfire smoke, and canonical-main comparison run on `c210f226` belong to older heads and are not current exact-head validation |
| Review state | CodeRabbit's latest substantive report observed here covers `0b2323e`, not published head `ee9a718f` or local merge `03005282`. Earlier findings have follow-up confirmations as addressed; low-priority performance observations remain deferred. No review covers the current local edits. The Campfire browser room-delete journey remains explicitly unverified |
| Scratch evidence | Older-pin generation and JSON inventory are under `/tmp/rh688-863-32b-*`; latest-pin strict check and failed generation captures are `/tmp/rh688-edbc779-check.out` and `/tmp/rh688-edbc779-generate.stderr`. Previous baseline captures remain under `/tmp/rh688-baseline` and `/tmp/rh688-898-*`; do not commit generated output or scratch logs |

The PR already contains substantial Rust Campfire work, including app/helper
class emission, namespace-aware controller structure, reachable inherited
methods, request-context and `process_action` wiring, and explicit unsupported
route handling. Treat historical labels such as “app classes missing,”
“cookies,” “capture,” and “STI helper” as **questions to re-triage**, not as a
current residual-error inventory. The Oracle inspection confirmed that
request/task metadata and `process_action` dispatch already exist; new work
must extend the present lifecycle instead of rebuilding those foundations.

**Interpretation guard:** the two captures establish deterministic survey
output for the measured inputs; they do not complete P0.2 provenance or the
source root-cause map. Strict generation on the current Campfire pin stops at
245 front-end diagnostics before a production Cargo project exists. The older
181-diagnostic observation is historical only.

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

- [x] **P0.1** Fetch/check out ONCE Campfire at exactly
  `32b4144b5206304fa8d4c67455a753e2d3c16635`; verify `git rev-parse HEAD` (or
  verify the archive’s recorded pin and content manifest when there is no
  `.git`). Do not silently substitute Campfire `main`.
- [ ] **P0.2** Record Roundhouse SHA, clean/dirty status, `rustc -vV`, Cargo
  version, host/target, OS, root and generated `Cargo.lock` hashes, compiler
  binary provenance/hash, relevant environment/feature flags, and the exact
  command line. Pin Rust to the PR’s declared **1.98.1** initially; any
  intentional toolchain change is a separately reviewed plan update. The
  repeated run’s OS and compiler binary hashes are now recorded; the original
  baseline worktree’s clean/dirty state and its exact relevant environment /
  feature-flag snapshot are not established. Leave this item open until those
  baseline-specific details are captured or explicitly marked unrecoverable.
- [x] **P0.3** Finish the generated-file, module/class/route/test census. The
  repeatable 487-file pre-Cargo manifest and source/category counts are recorded
  in the snapshot table. Keep credentials, session cookies, and authentication
  state out of logs and committed artifacts.
- [ ] **P0.4** Prepare the Rails oracle separately for comparison work. Record
  its Campfire pin and pristine database state. Do not block compiler-only
  inventory on Redis/oracle setup.
- [x] **P0.5** Run the production CLI path into fresh directories, separately:
  strict `roundhouse check --strict "$APP"`, strict Rust project generation, and (only
  for inspection) `--allow-unsupported` generation. Preserve the strict
  diagnostics. At this PR head `--allow-unsupported` changes diagnostic
  severities, so its warning count or exit status is not a zero-error proof.
- [x] **P0.6** If project generation refuses before producing the app, record
  that exact refusal and stop calling the result a rustc baseline. Do not
  bypass a project-level gate and present direct `rust::emit` output as the
  production CLI path.
- [x] **P0.7** Generate twice from the same pinned app and equivalent
  Roundhouse code (the intervening commit was docs-only); compare normalized
  Cargo diagnostic fingerprints and generated-file census. Both outputs had
  identical 487-file manifests and 2,252 fingerprints for 2,496 errors. Keep
  documenting any nondeterminism if later changes introduce it.

## Phase 1 — Build the rustc inventory

- [x] **P1.1** Run Cargo in the survey-emitted project against the captured
  generated lockfile and record each lane independently:

  ```sh
  cargo check --locked --lib --bin app --message-format=json
  cargo check --locked --all-targets --message-format=json
  cargo test --locked --no-run --message-format=json
  ```

  `cargo test --locked --no-run` also failed with the same 2,496 lib and 165
  lib-test errors. Add `cargo build --locked --release` once check gates make
  it practical. Strict production generation remains blocked before Cargo.
- [x] **P1.2** Parse Cargo JSON diagnostics (not rendered-text grep) into a
  machine-readable inventory. Preserve full message, error code when present,
  package/target kind, generated path, enclosing item, primary and child spans,
  expansion data, and useful source excerpt. The standard-library tool is
  [`scripts/campfire-rust-diagnostics.py`](../../scripts/campfire-rust-diagnostics.py);
  synthetic regressions are in
  [`tests/campfire_rust_diagnostics_test.py`](../../tests/campfire_rust_diagnostics_test.py).
- [x] **P1.3** Define stable fingerprints using target kind, generated module /
  enclosing item, compiler code, and normalized message/operation. Keep exact
  line numbers in raw evidence, but do not make line numbers the identity.
  Normalize temp paths only; retain meaningful type and signature details. The
  indexed output has 2,252 groups for 2,496 lib errors. Groups are not cause
  clusters.
- [x] **P1.4** Separate application errors from third-party/dependency/build
  environment failures; record whether compilation completed and its exit
  status so partial output, network failure, or termination cannot look like
  improvement. Cargo's own summary attributes these compile errors to the
  generated `app` crate; other crates were dependency build steps, not reported
  as error owners. Preserve each command's exit code with its capture.
- [ ] **P1.5** Manually verify each cluster with representative generated code
  and trace it back through Ruby source → typed IR → lowered library IR → Rust
  signature/import/runtime. Mark causes **confirmed** or **suspected**; retain
  unclassified errors rather than forcing a category.
- [x] **P1.6a** Record raw counts by strict/survey lane and Cargo target scope.
  The original 2026-10-09 baseline was 181 front-end diagnostics, 2,496
  survey lib/bin errors, and 165 additional lib-test errors. The current pin
  `edbc779f` is blocked by 245 front-end diagnostics and has no Cargo count;
  2,577/575/2,339 remains an older-pin comparison at `32b4144b` / `863dd24b`.
- [ ] **P1.6b** Manually assign root-cause cluster membership to the inventory.
  The error-code distribution above is a prioritization hint, not a root-cause
  classification. Do not report only totals: foundation fixes can expose
  additional methods and increase counts while improving coverage.

## Phase 2 — Triage and dependency waves

Re-sort this table after the baseline. The listed topics are candidates from
prior work, not promises that they remain the dominant errors.

| Wave | Work | Exit condition |
|---|---|---|
| 0 — evidence readiness | Pinned inputs; complete provenance; repeat generation/fingerprint comparison; generated module/class/route/test census; map the current 245 strict generator refusals (the 181 count is the original baseline) | Baseline can be rerun and compared; refusals have source-level owners, not just counts |
| 1 — contracts and independent foundations | Typed Rails application/config contract; return-preserving forwarded-block ABI; request-owned cookie transport contract; Rails oracle and Rust translated-test lane preparation | Interfaces and ownership are frozen; small semantic repros prove the foundation without pretending dependent behavior is complete |
| 2 — dependent semantics | Residual capture; signing and signed/permanent cookie views; session/login/logout; separately typed SignedId/InvalidSignature behavior; confirmed STI and narrow expression clusters | Each package meets its dependency contract and has executed semantic/security tests; fresh inventory is re-triaged |
| 3 — whole-project convergence | Remaining strict-generation refusals and production/generated-test Cargo errors; reachable unsupported/panic/default/501 audit | Fresh strict generation, production Cargo check/build, and generated-test compile all pass without lost output |
| 4 — application acceptance | Native execution, Campfire tests, Rails differential, browser journeys and security boundaries | Declared Rust Campfire workflows pass; remaining gaps are explicit and bounded |

### Confirmed first-wave clusters and dependency gates

Counts below are from the captured survey inventory, not from strict
production output. A confirmed missing generated symbol identifies a compiler
root cause; it does not by itself establish the complete feature contract.

| Status | Cluster | Evidence and dependency decision |
|---|---|---|
| **Confirmed; split by API contract** | Rails namespace references: 119 E0433 references to `Rails` | The generated project exports no `Rails` root, but the references cover multiple services and must not be treated as one config bag: `Rails.application` and its source-backed app/VAPID config, routes and URL helpers; request-derived protocol/domain; `Rails.root`, `Rails.env`, `Rails.cache`, and `Rails.logger`; plus `Rails::HTML5::SafeListSanitizer`. The shared `runtime/ruby/rails.rb` and `.rbs` already define typed Env/AppPath/Cache/Logger and a deliberately limited Application, but that runtime is not integrated into the Rust output. First reuse/register the typed shared surface and make the app-specific configuration boundary explicit. Keep sanitizer and callback-bearing WebPush-pool work as separate contracts; never add an untyped catch-all Rails object. |
| **Confirmed; ABI prerequisite (shared runtime placement fixed locally)** | Forwarded helper-block capture: 14 E0425 unresolved `capture` references in 13 generated methods | Pinned source mapping shows optional zero-argument HTML-content blocks forwarded through TagBuilder helpers. Survey Rust emits required `Box<dyn FnOnce()>`, then attempts `.clone().is_none()` and `capture(__blk)`: absence is impossible, the closure result is erased, and `FnOnce` cannot be cloned/invoked this way. The prior runtime omission is addressed locally: `capture` and its unchanged general RBS contract now live in registered `action_view/view_helpers`, not the Rust-excluded `view_helpers_ext`. The extension remains excluded because it contains other unsupported methods. This adds the callee implementation but does not close the block ABI or establish Campfire output; do not claim the 14 callsites fixed. Generic direct `capture` tests stay on the Ruby-family overlay lane until Rust can type that general callable contract; the D0 native fixture must separately prove the narrow String-returning ABI. `MessagesHelper#message_tag` also emits a `serde_json::Value` return in survey mode; keep that separate signature discrepancy visible rather than letting it hide either root cause. |
| **Confirmed; transport and security contract open** | Controller cookies: 104 E0425 unresolved `cookies()` calls | These are callsites for a missing request-scoped jar/accessor and response Set-Cookie path, not 104 independent features. Plain, signed, permanent, and delete operations are represented. Signed behavior depends on a real secret/config boundary and Rails-compatible verification. Session APIs, SignedId/InvalidSignature names, and Rails encrypted `_campfire_session` are distinct; do not count or claim them as fixed by a cookie accessor. |
| **Open; re-triage after producers** | STI/polymorphic routes and remaining E0308/E0599 groups | Prior estimates are questions, not work packages. Activate only on a current representative failure traced through model identity, lowering, route generation, or method signature. |

**Working-tree update after the captured `1e5b8288` baseline:** moving
`capture` into the registered shared runtime changes all 14 unresolved-name
diagnostics at those callsites into concrete block/capture type failures, but
the survey compiler count rises from 2,496 to 2,510. This is an explanatory
transition, not a compile improvement: the optional forwarded-block ABI is
still missing, and no Campfire behavior claim follows from resolving a name.
See the latest evidence-ledger row for exact counts and commands.

**D0 regression scaffold (local, not yet a passing feature):**
`tests/rust_toolchain.rs::forwarded_optional_string_block_runs_through_two_edges`
now generates a synthetic Ruby class with a nil-guarded terminal `capture`,
two `&block` forwarding edges, and a generated-crate executable check for
absence, exactly-once execution, borrowed state, and a moved non-`Clone`
capture. A rerun at Roundhouse
`cecdec8b71f3b10825d9e6158c45c381aa76d6e2` failed in the generated crate at
`cargo test --test forwarded_optional_block`: five Rust errors remain (E0599
for treating the required `Box<dyn FnOnce()>` as optional; E0271 because that
callback returns unit where shared `ViewHelpers::capture` requires
`serde_json::Value`; and three E0308 mismatches across the method's
`String`/`Value` result and direct closure-versus-boxed callsite). This is the
intended pre-fix reproduction, not a passing regression test and not support
evidence. The emitted methods still return `serde_json::Value`, and no
Campfire compiler-wall reduction is established. A focused Oracle review
(2026-10-09) recommends adapting only a proven D0 String-returning callable at
the terminal capture boundary as
`FnOnce() -> String` → `FnOnce() -> serde_json::Value::String`, leaving the
general shared RBS/runtime contract unchanged. The lowered nil guard must
preserve `None` as empty content; direct unguarded capture must not gain an
invented empty-block fallback. The next D0 patch must make this fixture pass
before auditing the 14 Campfire callsites.

Continue mapping the current 245 strict-generation refusals alongside rustc
triage; the earlier 181 count is the original baseline only. In every fresh build, inventory
reachable `todo!`, silent/default/no-op methods, 501 routes, and omitted
methods/modules. A green `cargo check` is not completion if accepted paths
panic, fabricate defaults, or refuse required routes.

### Dependency-linked work packages

Do not activate a package until its prerequisite/interface is recorded here
with a minimal reproducer, owned files, semantic checks, and integration check.

#### Execution order and safe parallelism

- **Serial baseline gate:** current pinned Campfire `edbc779f` cannot produce a
  Rust project because of `Data.define`. Keep that gate explicit and first in
  the production-baseline path; do not use survey-only internals to report a
  Cargo count. The Data representation and the `CachedResponses` nominal
  identity check must be solved together or remain explicitly unsupported.
- **Parallel contract work:** while the Data fix is developed, A0 and F0 may
  independently prepare test fixtures and pin their own source/API contracts.
  Do not claim either as Campfire support until current-pin production
  generation and emitted execution are available. A0 owns the typed app-config
  and version-header contract; F0 owns the request-bound plain-cookie jar and
  transport semantics. A0/F0 must not edit the same central Rust app/module
  wiring concurrently; one integrator owns `src/emit/rust.rs`,
  `src/project.rs`, the shared result matrix, and the combined Campfire
  inventory.
- **Oracle prerequisite for F0:** the currently installed global Rails is
  8.1.4, while the pinned Campfire lockfile resolves Rails 8.2.0.alpha; the
  pinned app's `bundle check` currently fails on missing gems. Existing
  committed compatibility vectors cover signed/encrypted cookie wires, but do
  not establish the plain/permanent jar's missing-versus-empty behavior or
  default attributes. Do not infer these from the Ruby-family `CookieJar` or
  the different global Rails version. Obtain those cases from the pinned Rails
  oracle before changing behavior.
- **Integration order:** after isolated A0/F0 tests pass, integrate one owner
  at a time, then run combined Rust tests and a fresh strict generation/survey.
  Reassign dependencies from the resulting diagnostics before opening the
  next implementation wave. Keep signed `session_token` and encrypted
  `_campfire_session` outside F0.

| Package | State | Scope and dependency | Completion evidence |
|---|---|---|---|
| **A0 — typed Rails namespace/application interface** | Partial: typed Rails root/public-path runtime is emitted and path behavior tests pass. Ingest already lifts app config into `app.rails_application`, and config-reader lowering rewrites reads to that application object; Rust currently filters out `Rails.application` and does not emit the lifted `Rails::Application` methods. | Reuse the existing typed config lift/lowering and finite app-config contract. First vertical slice is source-backed `app_version` plus nullable `git_revision`; preserve the initializer's exact `.presence` fallback and do not bake the transpiler's environment into output. Do not add an untyped config bag or dummy defaults. Keep WebPush pool callbacks and unrelated Rails services separate. Central generated-module wiring is integrated by one owner. | Trace each selected callsite through source → lifted reader → emitted method → response header. Test presence/blank/absent cases in fresh processes, nil initialization separately from “not initialized,” the actual before-action and HTTP response behavior, then regenerate and classify the Rust diagnostic delta. |
| **A1 — WebPush pool service** | Not started; separate follow-on | Separate from A0’s scalar/config interface: Campfire initializes a callback-bearing `WebPush::Pool`, mutates/replaces it in tests, and calls queue/shutdown. Requires a typed pool/callback/lifecycle contract, not a `Value` callback or dummy pool. | Emitted queue/delivery and invalid-subscription behavior, shutdown/replacement lifecycle, and Campfire push journeys; if deferred, retain an explicit unsupported boundary and do not claim push behavior. |
| **D0 — forwarded-block ABI** | Shared capture runtime home implemented locally; standalone representation prototype compiles; signature-map defect fixed; ABI wiring not started | The 14 capture sites span 13 methods: `ClipboardHelper#button_to_copy_to_clipboard`; `Messages::AttachmentPresentation#inline_media_dimension_constraints` (two branches) and `#lightbox_link`; `MessagesHelper#message_area_tag`, `#messages_tag`, `#message_tag`; `QrCodeHelper#link_to_zoom_qr_code`; `Rooms::InvolvementsHelper#turbo_frame_for_involvement_tag`; `RoomsHelper#link_to_room`, `#link_to_edit_room`; `SearchesHelper#search_results_tag`; `Users::FilterHelper#user_filter_menu_tag`; `Users::ProfilesHelper#web_share_session_button`; `Users::SidebarHelper#sidebar_turbo_frame_tag`. These are optional zero-argument content blocks returning HTML text. `sidebar_turbo_frame_tag` has both block and no-block callers. Related `composer_form_tag`, `profile_form_with`, and `auto_submit_form_with` forward form-builder blocks and are compatibility checks, not part of this narrow contract. Current Rust placeholder/forwarding code is in `src/emit/rust/method.rs` and `src/emit/rust/expr/literal.rs`; `block_refine` is documented as same-class only, while these blocks cross helper/runtime boundaries. A local `rustc` prototype for the selected `Option<Box<dyn FnOnce() -> String + '_>>` shape compiled and ran through two forwarding functions, asserting present/absent output, exactly-once invocation, a borrowed local remaining usable, and a moved non-Clone capture. This proves only the Rust representation, not Roundhouse emission or Campfire support. D0 triage also exposed a separate adjacent defect: ordinary Rust parameter typing rejected a valid signature whenever its `Ty::Fn.params` contained the extra block slot. `collect_param_types` and instance parameter rendering now filter that slot; a focused regression pins the ordinary parameter type and both render paths. A fresh post-fix Campfire survey on `1e5b8288` still has exactly 2,496 errors/2,252 fingerprints and all 14 unresolved `capture` occurrences; this adjacent fix did not move the app's compiler wall. An independent Oracle follow-up confirms the two missing information paths: refinement currently only propagates same-class callees, and call emission has no resolved callee ABI context for choosing `Some`/`None` and moving a forwarded callable. A temporary attempt to put general capture block tests in the shared cross-target test file failed Rust test compilation because its untyped block results became `serde_json::Value`; those probes were moved back to the Ruby-family test lane rather than weakening the shared RBS contract. | Next: introduce only a capture-specific cross-boundary contract/refinement and carry it to both signatures and callsites. Pin conflict/declaration-order/optional cases, then verify module+instance emissions; preserve argument-bearing form-builder blocks as a control. Run an emitted native compile/run before surveying all 14 sites. Do not register all of `view_helpers_ext` or add a Rust-only stub. Keep arbitrary non-String capture semantics and the `message_tag` return discrepancy separately visible. |
| **D1 — residual capture semantics** | Blocked on D0 | Implement only the actual forwarded-capture forms, preserving Rails output/buffer, return/fallback, nested behavior, ordering and escaping/safety semantics. | Executed emitted regression cases compare output and fallback behavior, including nested/evaluation-order cases where Campfire uses them; then re-inventory the 14 callsites rather than assuming all share the same semantics. |
| **F0 — request cookie ownership/transport** | Partial: transport and standalone middleware tests are implemented; a new ignored harness exercises `server::production_router`, the same assembly used by production `start`, and passes handler/layout context sharing, HTML/redirect/non-HTML finalization, existing `Set-Cookie` preservation, and concurrent-request isolation. No Rails `cookies` jar consumes the transport yet. | Retain the request-owned queue and outer drain. Before reusing the shared Ruby jar, correct/resolve its currently documented deviations: missing reads return `""` rather than Rails `nil`, and bare-cookie HttpOnly defaults on whereas pinned Rails defaults it off. | Executed production-stack test proves handler/layout read the same inbound cookie, both can queue response cookies, redirect and non-HTML paths skip layout but drain handler writes, existing headers survive, and concurrent clients do not leak values. Rails oracle must pin missing/empty distinction and attributes before implementing only Campfire's nullable plain/permanent operations. Keep signing/session separate. |
| **F1 — signing and secret boundary** | Blocked on A0 `secret_key_base` decision | Reuse shared Ruby semantics/verifier representation and native Rust crypto adapters; do not fork Rails cookie format or accept empty/missing production secrets. | Cross-language Rails-minted ↔ Rust-minted vectors, malformed/tampered/wrong-key/wrong-name/purpose/expiry rejection, missing-secret fail-closed behavior, and request isolation. |
| **F2 — signed/permanent cookie views** | Blocked on F0 + F1 | Implement Campfire-used plain, signed, permanent, options, write, read and delete behavior over the same request jar. | Emitted app sends and consumes cookies across requests; attributes/expiry are asserted; a removed compiler diagnostic has passing behavioral coverage. |
| **F3 — session and login/logout** | Blocked on F2 + separately typed session/model/error contracts | Rails encrypted `_campfire_session` is separate from `session_token`; scope it as its own package if Campfire requires it. | Login via Rails-compatible signed token, authenticated follow-up, invalid/tampered/expired rejection, logout invalidation, CSRF/no-write-on-rejection, and concurrent-user isolation. |
| **F4 — SignedId/InvalidSignature** | Not started; separate from cookie transport | Requires actual model, purpose, expiry and error contracts. | Rails compatibility vectors plus emitted create/verify/reject execution for relevant Campfire models; distinct type/error behavior is tested. |
| **E — STI/routes** | Not re-triaged on this survey; hold | Activate only for a verified remaining `link_to @record`/polymorphic route repro after checking persisted/hydrated subtype identity and route bridge. | New/edit/namespace/routeless-subclass cases as applicable, with emitted URL execution; don't infer success from a helper's type-check. |
| **C — residual compiler clusters** | Not classified globally; inventory by source signature first | Assign narrow, confirmed E0308/E0599/etc. causes only; exclude central files owned by another package. | Before/after representative semantics and no missing modules/methods/tests in a fresh census. |

### First substantial reduction target: request/auth vertical slice

Fresh measurement at Roundhouse `ab04ceb67a065096e63bcfd89acec21aa5b961a6`
and Campfire `32b4144b5206304fa8d4c67455a753e2d3c16635` found 2,510 Cargo
errors in survey-only lib/bin output. This is the classification starting
point, not a production build. Largest diagnostic-code totals are E0308 833,
E0599 575, E0425 456, and E0433 255; these codes are too broad to treat as
root causes. More useful first buckets are 119 “cannot find type `Rails`”,
104 “cannot find function `cookies`”, 33 “expected function, found module
`session`”, 51 `unwrap` calls on `serde_json::Value`, and 23 references to
the undefined `RoundhouseUnsupportedRelation` marker. These counts overlap
larger code totals but are separate diagnostic messages; they must not be
summed as independent fixes or promised as one-to-one reductions.

The best first candidate for a few-hundred-error reduction is a vertically
verified request/auth slice, not a generic E0308 cast pass: (1) trace the
119 Rails references to the precise typed Rails namespace/root/config
contracts already present in shared runtime code; (2) define request-owned
cookie state and lifecycle before implementing the 104 cookie callsites;
(3) trace the
repeated session/auth controller errors back through lowering and the
`resume_session`/`session` signatures; and (4) keep signed-cookie crypto and
secret configuration fail-closed. The Rails and cookie counts alone total
223 diagnostics, but a correct implementation may expose additional errors
or remove cascading ones, so this is an opportunity estimate, not a forecast.
The session/unwrap/type mismatches may belong to this slice only after
source-to-emitted tracing confirms it. Never add an empty Rails value, global
cookie jar, `Value`-typed callback, default secret, or no-op login path to
chase the number.

Before the first implementation batch: [ ] store the full JSON diagnostic
inventory and normalized fingerprints for this exact head; [ ] for each
candidate cluster, map generated spans back to Campfire source and classify
direct versus cascading diagnostics; [ ] choose the smallest complete
request/auth behavior with an emitted regression that checks valid, rejected,
and cross-request-isolation cases; [ ] rerun fresh strict generation and
survey lib/bin after the change; [ ] report actual removed/retained/new
fingerprints and the executed behavioral cases. Continue the next verified
cluster if the first complete slice does not approach 500 net fewer errors;
do not weaken diagnostics or count lost output as progress. D0 remains a
correctness prerequisite for 14 HTML block sites, but is not by itself a
500-error lever.

### Oracle review and current decision

The Oracle review on 2026-10-09 recommends an evidence-readiness batch first,
then the narrow **D0 return-carrying block ABI** as the first feature batch.
The 14 `capture` calls are downstream of a closure contract that currently
erases a forwarded block to `Box<dyn FnOnce()>`; they cannot be repaired by a
capture helper that returns empty text or guesses the block result. D0 is not
completion of the Campfire capture callsites.

**Replan after canonical main moved the Campfire pin:** Oracle recommends
working on current CI input `edbc779f` first, rather than quietly treating the
older `32b4144b` pin as current. The current input strictly analyzes with zero
errors but cannot emit Rust because `FragmentCache::ContentKey` uses
`Data.define`. Ingest and analysis already recognize the literal factory and
carry its methods; the missing contract is a target representation. Implement
only statically representable immutable Data records with constructor/readers
and source-defined methods, preserving explicit errors for unsupported forms
and uses. In particular, `CachedResponses` asks whether a flattened
heterogeneous cache key includes `FragmentCache::ContentKey`; resolving the
factory name without preserving nominal identity would silently break cache
versioning. Until that consumer has a correct representation, keep that use
explicitly unsupported and do not claim fragment-cache behavior.

Once current-pin survey output exists, re-triage its fresh fingerprints.
Then proceed with a separate typed Rails version/header slice, followed by
unsigned permanent `last_room`, then a real fail-closed Rails-compatible
verifier for signed permanent `session_token`. Keep encrypted
`_campfire_session` separate. The previous `32b4144b` inventory is a control,
not a current-pin denominator. D0 remains independent; do not mix its
callable ABI into Data, Rails/config or request-cookie contracts.

#### Next execution checklist — Rails and cookies

The primary current-pin checkout is `/tmp/campfire-edbc779/`
(`edbc779f4dfc9b26c36310881711ddc976a9dfc8`); the historical comparison
checkout is `/tmp/rh688-baseline/campfire/`
(`32b4144b5206304fa8d4c67455a753e2d3c16635`). The Rails/cookie source audit
below was first performed on the older pin and must be rechecked on the current
pin before claiming source parity. Check a step only when its completion
evidence is recorded in the ledger. Existing path and transport work are
foundations only and do **not** check off complete Campfire behavior.

- [x] Re-read the pinned source contracts: `config/initializers/version.rb`,
  `app/controllers/concerns/version_headers.rb`,
  `app/controllers/concerns/tracked_room_visit.rb`,
  `app/controllers/concerns/authentication/session_lookup.rb`, and
  `app/controllers/concerns/authentication.rb`.
- [x] Merge canonical `rubys/roundhouse:main` at `4a70cd0` and again at
  `5d3d144e` in merge commit `863dd24b`; preserve history and resolve the sole
  `native_http.rs` conflict by retaining both automatic session propagation
  and explicit-cookie override. `cargo check --locked --all-targets` passes.
- [x] Emit the typed shared Rails path facade and pin isolated Rails path
  behavior. This covers `Rails.root`/`public_path` only, not
  `Rails.application`, config, cache, env, or Campfire headers.
- [x] Add request-owned incoming-cookie parsing and queued `Set-Cookie`
  transport, with first-duplicate preservation, encoded-value preservation,
  concurrent-request isolation, multi-header append and invalid-header
  rejection probes. This is transport only; no Rails jar consumes it.
- [ ] **A0.1 — Rails version oracle:** partial standalone callback probe
  completed: first-present `APP_VERSION` / `GIT_REVISION` / `"0"` determines
  `app_version`; `git_revision` is copied directly, so blank remains `""` and
  absent remains nil. Five cases produced HTTP 200 and the response map kept
  `x-rev => nil` when absent. The locked Campfire Rails boot and actual wire
  serialization of that nil header remain unverified because Bundler cannot
  find the pinned Rails checkout; complete this in the Rails-enabled oracle
  environment before choosing Rust header behavior.
- [ ] **A0.2 — source-backed application config:** expose only the typed
  configuration values required by those Campfire callsites, preserving
  `APP_VERSION.presence || GIT_REVISION.presence || "0"` and nullable
  `git_revision`. Do not add an untyped config bag or sample defaults. Carry
  the data from initializer inputs into the app's generated Rails application
  representation with once-only initialization semantics.
- [ ] **A0.3 — emitted version-header vertical slice:** compile and execute
  the actual generated controller/header path for the A0.1 matrix; compare
  headers with the Rails oracle. Then measure strict-generation changes and
  a fresh Rust survey inventory. A quieter analyzer or shifted error code is
  not acceptance.
- [x] **F0.1 — production cookie lifecycle:** extracted
  `server::production_router`, called by both `server::start` and the ignored
  isolated test harness. `cargo test --locked --test
  rust_server_cookie_lifecycle -- --ignored --nocapture` passes an assembled
  production-stack test: handler/layout read the same incoming cookie, both
  queue writes, HTML preserves an existing `Set-Cookie`, redirects and
  non-HTML skip layout but drain handler writes, and concurrent requests stay
  isolated. This establishes transport ownership/finalization only, not jar
  serialization semantics or Rails cookie behavior.
- [ ] **D-Data.1 — current-pin declaration audit:** exact source use is
  `FragmentCache::ContentKey = Data.define(:digest) do ... cache_key ... end`
  in `app/models/fragment_cache.rb`; `CachedResponses` also uses
  `is_a?(FragmentCache::ContentKey)` on a heterogeneous flattened cache key.
  Confirm constructor, member, block and consumer behavior against Rails; do
  not count this input as Cargo-measurable yet.
- [ ] **D-Data.2 — finite Rust Data representation:** reuse existing ingest and
  analysis metadata; carry nominal class identity, ordered members, inferred
  member types and factory block methods through lowering to Rust. Do not
  derive `Default`, expose mutable fields, invent missing values or accept
  unsupported factory shapes. Preserve the `is_a?`/heterogeneous-key
  distinction or retain an explicit unsupported diagnostic for that use.
- [ ] **D-Data.3 — Data execution and gates:** add production-path emitted Rust
  compile/run coverage for the actual ContentKey constructor, `digest` and
  `cache_key` behavior, plus negative cases for unsupported forms and identity
  checks. Verify `is_a?` evaluates its receiver and has correct nominal
  behavior before admitting Campfire's cache consumer. Then record strict
  generation, generated Cargo check and fresh error fingerprints on both pins.
  Successful survey generation alone is an inventory milestone, not feature
  completion.

**D-Data emission probe (2026-10-10, local-only at `183a2aa4`):** a temporary
integration test sent the pinned declaration shape through ingest, analysis,
and `emit::rust::emit` directly (intentionally bypassing the production gate),
then was removed. The emitted parent class contains only a `TODO` for the
`ContentKey` constant; the nominal `ContentKey` class is an empty
`#[derive(Clone, Default)]` struct, and its `cache_key` is emitted as
`fn(&self) -> serde_json::Value` whose body calls an undefined free `digest()`.
This probe establishes why deleting the project gate would expose broken
output. It does not compile/run the emitted app or establish Cargo-count
availability. The gate remains until constructor/member storage, reader
typing, the `cache_key` receiver, and the heterogeneous nominal identity path
are implemented and executed together.

**D-Data progress (2026-10-10):** the block-form library class retains
`DataFactory` provenance and source-ordered members; ingest also materializes
required positional initialization and read-only readers in the typed class
IR, while preserving authored methods and allowing authored reader overrides.
The analyzer infers the fixture's `digest` and `cache_key` as `String` from
the actual constructor arguments. Ruby emission filters only these synthetic
IR methods so native `Data.define` still supplies its constructor/readers; a
regression checks the emitted source. The Rust emitter now gives Data-origin
classes private member storage and omits the invalid `Default` derivation; a
test compiles and executes the directly emitted `ContentKey` record through
its constructor, generated readers and authored `cache_key` method. That test
bypasses project admission intentionally: the owner constant binding and
heterogeneous nominal identity remain unimplemented, so the production Rust
refusal stays required until the Campfire consumer is compiled and executed.
Oracle recommended this dependency order rather than tackling erasure and
identity before a nominal record exists. Validation on the worktree based on
`1031b440`: `data_factory_constants` passes 15/15, including direct `rustc`
compile-and-run of the emitted record; `cargo check --locked --all-targets`
passes with existing warnings, documentation references pass 1/1, and
whitespace/test-file formatting checks pass. This is isolated record evidence,
not a current-pin Campfire Cargo count or cache-consumer result. Repository-wide
`rustfmt --check` is not clean because these large existing source files
contain prior formatting drift; no broad formatting rewrite was made.

- [ ] **F0.2 — unsigned permanent `last_room`:** implement the smallest typed
  jar operation used by `TrackedRoomVisit`, including Rails-compatible
  request parsing, write/read/delete and permanent-cookie attributes. Exercise
  a room visit followed by a second request and compare the cookie/header with
  Rails. Invalid and missing values must fail through the app's existing
  fallback (`default_room`), not a fabricated ID.
- [ ] **F1/F2 — signed permanent `session_token`:** only after F0 and the
  source-backed secret/config contract are frozen, implement Rails-compatible
  signing, purpose/name handling, expiry and verification. Test Rails-to-Rust
  and Rust-to-Rails vectors plus tampered, malformed, wrong-key, expired and
  missing-secret cases. Missing production secrets must fail closed.
- [ ] **F3 — authentication journey:** only after signed-cookie semantics,
  exercise login, authenticated follow-up, invalid-token rejection and logout
  on the emitted app. Keep Rails encrypted `_campfire_session` separate if
  Campfire routes actually require it; signed `session_token` is not an
  encrypted Rails session.
- [ ] Re-run fresh strict analysis/generation, survey lib/bin and error
  fingerprints for both Campfire pins after each integrated slice; explain
  newly revealed downstream methods. Record direct removals, retained/new
  groups, generated-file changes and executed behavior tests. Do not forecast
  one-to-one reductions from historical Rails/cookie diagnostic counts.

**Parallel execution boundary:** parallelize pinned Rails oracle/test-matrix
preparation and cookie wire-format/request-isolation tests in separate orbs;
these can proceed without touching shared emitter files. Keep one integration
owner for `src/emit/rust.rs`, `src/runtime_loader.rs`, `src/project.rs`,
generated exports and dependency templates. Keep one F0 owner for
`runtime/rust/http.rs` and `runtime/rust/server.rs`. Do not run concurrent
feature edits against those shared files; integrate each tested contract in a
small batch, refresh the survey, then reassign. D0 ABI work can be prepared in
parallel on its own analysis/emitter/test files after the owners are frozen.

A follow-up Oracle decision on the now-confirmed 13-method map selects a
narrow optional, boxed, return-carrying callable representation for these
zero-argument HTML blocks: conceptually
`Option<Box<dyn FnOnce() -> String + '_>>`. The `'_` lifetime is intentional:
the view closure may borrow locals, and the closure may own non-`Clone`
captures. Literal blocks are boxed only when entering this ABI; forwarded
options move unchanged; omitted blocks become `None`; terminal capture
consumes the callable once and keeps its returned string. This is a proposed
representation to prove with a small executable fixture before propagating
it—not a blanket ABI for all Ruby blocks.

Implementation constraints from that review:

- Carry callable shape and optionality as distinct contract metadata through
  resolved call edges. `Ty::Fn.block: None` means there is no block slot in
  that signature; it must not be overloaded to mean the source-level block
  can be absent. Conflicting forwarding targets remain unsupported rather
  than using first-target-wins refinement.
- Resolve the same contract at both method definitions and callsites,
  including omitted-block calls. Module and instance method paths must agree.
  Do not globally box closures: typed argument-bearing form-builder blocks
  retain their argument/result types and current semantics.
- Inspect/narrow only the forwarded capture operation whose source proves a
  String-valued block result. The shared `ActionView::ViewHelpers.capture`
  accepts arbitrary block results and returns an empty String for non-String
  results; do not globally narrow its RBS contract or change ordinary capture
  semantics to fit this subset.
- Nil checks borrow (`is_none()`), forwarding moves, and terminal invocation
  consumes. No `.clone()`, no fabricated `'static`, and no accidental second
  invocation. Verify behavior where blocks are mutually exclusive as well as
  the absent case.

**D0 classifier and emission boundary (Oracle follow-up, 2026-10-09):**

- Implement one Rust-local `block_abi` analysis over the final Rust-owned
  `LibraryClass` set. Use a canonical method key containing the full namespace,
  `MethodReceiver`, and method name. Do not key contracts by leaf class name
  or bare method name: the existing global class-method registry collapses
  namespaces and receiver kinds for ordinary dispatch, which is too lossy for
  an ownership-changing ABI.
- Have analysis and emission share the same conservative call resolver.
  Initially admit only uniquely resolved same-owner calls, explicit
  namespaced class-method calls with known receiver kind, and uniquely
  registered global helpers. Preserve helper shadowing and dispatch
  precedence. Leave ambiguous helpers/leaf constants, unknown receivers,
  unresolved inheritance/overrides, and dynamic calls unsupported.
- Seed candidates from source-structured optional nil-guarded capture, not a
  method-name allowlist. Propagate contracts to callers to a fixed point only
  when the incoming block is forwarded unchanged and all uses are accounted
  for. Reject reassignment, alias/storage/return, explicit yield/invocation,
  opaque expressions, deferred nested captures, unproven recursion, and
  loops that consume the block. Count consumers per execution path: mutually
  exclusive terminal captures can pass, while sequential double-consumption
  or use-after-move cannot.
- A nil-guarded call to general `capture` is not by itself proof that every
  caller's block returns String. Require positive evidence at each admitted
  boundary (zero-argument String-producing literals or already-proven D0
  bindings); if evidence conflicts or is unknown, do not classify that edge.
  Preserve the arbitrary-result RBS/runtime contract and report excluded
  Campfire paths explicitly.
- Feed proven return types and contract facts consistently into definitions,
  class/global dispatch registries, call-expression effective types, and
  existing coercion/ownership decisions. A block contract does not imply a
  String return automatically. Apply Rust-effective facts to Rust-owned IR
  before coercion insertion; do not mutate shared analyzed input or change
  shared runtime signatures. Static-safe implementation functions and their
  instance wrappers must share the same signature contract and move forwarded
  options unchanged.
- At D0 callsites append `None` for omitted blocks, move a proven option
  unchanged for forwarding, box only a proven zero-argument String lambda,
  and preserve explicit nil as `None`. Keep generic `attach_block` behavior
  for all non-D0 calls. At the terminal capture only, consume once and adapt
  `String` to `serde_json::Value::String`; do not invent an empty fallback
  for an unguarded block. Avoid generic `Ty::Fn` or `param_types` as a place
  to smuggle optionality, since those paths can reintroduce narrowing or
  cloning.
- Add rejection tests as well as the positive native fixture: conflicting
  targets, namespace/receiver collisions, non-String or argument-bearing
  blocks, double consumption, use-after-consumption, loops, deferred capture,
  and ambiguous helper calls. Check D0 call-expression coercion in an
  argument/interpolation context, not just function tail. Keep the existing
  form-builder block as a negative-control compatibility test and test the
  generic capture runtime with both String and non-String results.

**D0 proof order:** (1) contract/refinement tests for two forwarding edges,
declaration-order independence, optionality, and conflict rejection; (2)
module/instance emission tests for present, absent, forwarded and borrowed
closures; (3) a native compile-and-run fixture asserting exact returned HTML,
one invocation, absent invocation count zero, a live borrow after the call,
and a non-Clone capture; (4) a required argument-bearing form-builder block
control; (5) a minimal real-blog overlay if shared lowering changes; then
(6) fresh Campfire generation and inspection of all 14 sites. Only after this
proof should the broader D0 implementation be dispatched/integrated.

**D0 progress (2026-10-10):** [x] Added a test-only conservative seed/forwarder
recognizer probe for a direct optional nil-guarded `capture` terminal embedded
in a string interpolation, plus two same-owner forwarding edges. It rejects
duplicate method identities, extra block reads, unknown targets, and deferred
forwarding under a nested lambda. The focused two-test suite passes. This probe
is intentionally not compiled into production and does not prove caller block
result types, resolve cross-owner callsites, establish per-path consumption,
or change emitted Rust; it is scaffolding only, not D0 completion or Campfire
progress.

[x] Extended the ignored emitted-Rust acceptance fixture with an actual Ruby
source callsite whose literal block returns `String`; this will exercise the
positive callsite evidence needed by the eventual ABI classifier rather than
relying only on a Rust-authored callback. Re-ran
`forwarded_optional_string_block_runs_through_two_edges` from dirty worktree
based on `83f6d67a`. The generated crate fails with five errors: optionality
checking tries to clone `Box<dyn FnOnce()>`; the terminal's closure is unit-
returning where shared `capture` requires `Value`; guard branches and inferred
method return disagree between `String` and `Value`; and the newly included
source callsite passes a closure where the current method signature requires
`Box<dyn FnOnce()>`. This is a more informative pre-fix failure, not a Campfire
inventory change.

The pinned Campfire source audit at `32b4144b5206304fa8d4c67455a753e2d3c16635`
shows that the 14 listed sites have zero-argument HTML-content blocks; the
two `sidebar_turbo_frame_tag` calls without a block are real optional `None`
cases, and the two dimension-constraint capture calls are mutually exclusive.
Form-builder block forwarding remains a negative control. The next vertical
slice is the structurally guarded sidebar frame method, with the actual
generated view callsites proving content-block shape and omission. Implement
one exact method/callsite resolver and carry its decision through signature,
call, and terminal consumption; do not change generic `capture` or claim the
other 12 methods by association.

**D0 callsite-proof refinement (2026-10-10; Oracle advice checked against
local sources):** the emitted test now runs `session::analyze_and_lower`,
which is the same shared post-analysis lowering facade used by emit-bound
drivers. It still uses a hand-authored guarded `capture` helper, so it does
not yet exercise the Campfire `tag_builder` rewrite or an ERB template.
`tag_builder::capture_call` creates the guarded terminal for a forwarded
helper block; `capture_inline` flattens literal `capture` lambdas and may
stringify the final block value. Therefore, a String result on that lowered
callee is not enough to prove the original caller's block result.

Require per-callsite evidence: inspect all Lambda arity fields directly
(`Ty::Fn.params` is synthesized empty and cannot prove zero arity), require an
exact String return rather than `Untyped`/union evidence, and reject
unmodeled early exits such as `return`, `break`, and `next`. For template
callbacks, require a terminal `StringBuilderResult` tied to that callback's
own `StringBuilderInit`; a descendant-only hint is insufficient. The hints
and `Ty::Str` prove string value shape, not SafeBuffer/HTML-safety semantics;
preserve the existing escaping and `.html_safe` paths, and do not expand the
support claim to Rails SafeBuffer equivalence. Missing, ambiguous, or
conflicting callsite evidence means no specialized ABI. See the generated
fixture and pipeline evidence in the latest ledger row.

`cargo check --lib` passes without warnings after isolating the probe under
`cfg(test)`. `rustfmt --check` passes on the new probe, while a direct check
through `rust.rs` still reports pre-existing formatting drift across the
emitter tree; no broad formatting was applied.

**D0 classifier soundness follow-up (2026-10-10):** an Oracle audit found
four unsafe-positive paths in the initial test-only classifier: calls in
deferred lambda bodies were skipped; a single guarded terminal could be inside
a loop or follow an implicit `yield`/`super`; assignment could replace the
block without adding a Var read; and a same-named inherited call could be
silently ignored as a non-candidate target. Tightened the probe to inspect
calls in deferred lambdas (but never treat a nested forwarded Var as proof of
the enclosing method's block), reject loop/yield/super/rebinding flows, and
invalidate a candidate on unresolved candidate-named targets. Added negative
tests for deferred non-String callsites, repeated/implicit/rebound consumption,
and unresolved inherited dispatch. Focused classifier tests initially passed
6/6. The first audit also found that the classifier recognized a bare method
named `capture` without proving that it resolves to the shared
`ActionView::ViewHelpers.capture` implementation. A subsequent local-only
refinement requires the owner resolver to identify that exact class method and
requires exactly one matching method in the analysis inventory; a regression
rejects a local `capture` override and a missing owner. The classifier suite
was 7/7 before the latest hardening. A follow-up Oracle review found two more
prerequisites: candidate call resolution must honor local-method shadowing
before global-helper fallback, matching Rust emission precedence; and
forwarding candidates must reject implicit yield, super, rebinding, and other
unsupported block flow in the forwarded send's arguments, not only in terminal
methods. The test-only classifier now applies both guards and has regressions
for a local instance `capture` shadowing the registered framework helper,
`yield` in forward-call arguments, and block rebinding before the forward. The
focused classifier suite passes 9/9. A separate structural return-shape probe
now follows the real tail across the terminal, same-owner forwarding chain,
and caller; it rejects a method whose guarded capture is followed by a
non-String tail and accepts a verified string-builder tail. This is still only
test scaffolding: it does not inspect `App::rbs_signatures`, and explicit
authored return contracts (including `untyped`) must veto any future Rust
return override. Production wiring is still untouched.
The emitted-crate test was rerun at
`cecdec8b71f3b10825d9e6158c45c381aa76d6e2` and remains a red reproduction
(five generated Cargo errors). Before wiring, separately prove effective
method returns and authored RBS provenance; the classifier's callback-flow
proof alone cannot authorize a `String` return. Production analysis must use
the complete final Rust-owned inventory and preserve dispatch identity at emit
time. No Campfire inventory was rerun, so there is no compiler-wall delta or
support claim.

**Immediate D0 implementation boundary (reconfirmed at `bdb0651b`, 2026-10-10):**
the fresh ignored emitted-project test still fails with five errors, all in the
synthetic `BlockForwardingProbe` crate. The generated source identifies three
independent ABI mismatches: (1) the guarded optional block parameter is emitted
as required `Box<dyn FnOnce()>`, and the guard consequently emits an invalid
`.clone().is_none()`; (2) the body and the shared terminal disagree about
`String` versus `serde_json::Value`; and (3) the source literal closure is
passed directly where the forwarding method expects a boxed trait object.
Do not start by changing the generic block placeholder or `ViewHelpers::capture`:
that would broaden the contract beyond the proven zero-argument HTML subset.
The first production slice must instead carry one conservative, canonical
method contract into both definition and call emission, with separate plans
for required versus optional slot and return shape. The optionality guard must
borrow (`is_none()`), forwarding must move the `Option` without cloning, and
only eligible literal closures may become `Some(Box::new(...))`. Preserve the
ordinary argument-bearing form-builder block path as a negative control.

Before editing emitter behavior, turn the test-only analysis into a production
plan builder with these gates: [ ] full owner + receiver + method identity;
[ ] complete final Rust-owned class inventory, including route/import/view,
controller, and fixture/helper classes; [ ] identical resolution precedence
for classifier and emitter, with local shadowing before global helper lookup;
[ ] source-proven String return shapes and callsite evidence; [ ] an authored
RBS return-contract veto (including explicit `untyped`) while allowing only
compatible declarations; [ ] duplicate/ambiguous methods and unresolved
inheritance excluded. Then apply that plan symmetrically to static-safe
implementation functions and instance wrappers. Keep the native generated
crate regression red until all paths work; the classifier's 9/9 unit results
alone are not a feature gate.

For cookies, existing request/task metadata and `process_action` dispatch are
already present. F0 must extend that request lifecycle and decide whether the
same jar is available through outer layout rendering before deciding when
pending writes are drained. F1 must reject missing secret configuration rather
than accept an empty-key fallback. Rails’ encrypted `_campfire_session` is
separate from the signed `session_token` cookie. This ordering allows a
cookie-transport contract to be designed independently, while signed auth
remains gated on A0’s typed secret/config decision and F1 compatibility tests.

### Parallel ownership and batch protocol

Use one integration owner for shared/central wiring. Parallelize design,
isolated semantic tests, pinned oracle/test-lane preparation, and genuinely
disjoint implementation; serialize edits to central interfaces.

- **Integration owner:** `src/emit/rust.rs`, `src/project.rs`,
  `src/runtime_loader.rs`, generated module exports, registries, and dependency
  templates. No other worker edits these files during an active integration
  batch.
- **D0/D1 owner:** block analysis/refinement, `src/emit/rust/method.rs`,
  narrowly named block/closure expression files and capture-specific tests.
  Exclude those files from other active feature assignments.
- **A0 contract owner:** trace Campfire callsites and propose typed contracts
  and isolated behavior tests. Hand central emission/module wiring to the
  integration owner; do not invent dynamic configuration placeholders.
- **F0/F1/F2 owner:** shared cookie semantics plus request transport/ownership
  and dedicated request/crypto tests. One owner controls `runtime/rust/http.rs`
  and `runtime/rust/server.rs` during the cookie batch; no second request-state
  implementation in parallel.
- **G evidence owner:** repeatability/census, strict-refusal mapping, pinned
  Rails oracle and Rust translated-test lane. No feature-file edits.
- **E/C owners:** only after a current repro and explicit file ownership; do
  not allocate all errors of a compiler code to one worker.

**Batch 0 — evidence readiness:** complete P0.2/P0.3/P0.7, update the ledger,
map the strict refusals and pin the oracle/test-lane gap. Keep the existing
baseline intact; all fresh emissions go to fresh directories.

**Batch 1 — Rails/config and cookie vertical slices:** establish the A0
version-header oracle and typed configuration boundary, then prove the F0
request lifecycle before implementing unsigned permanent `last_room`. Start
F1/F2 signed-token work only after the source-backed secret boundary is
settled. Prepare D0 ABI work in parallel only on disjoint files; its
integration waits for the A0/F0 shared-file batch to finish. Do not mark
cookies or Rails application support complete because a method name resolves.

For each package, create a small tracking entry (issue or this plan) before
dispatch: [ ] cause and fingerprint membership confirmed; [ ] dependencies and
owned files frozen; [ ] minimal regression fails before and executes after;
[ ] shared runtime is fully typed and removed errors have behavior evidence;
[ ] focused semantic and applicable real-blog checks pass; [ ] fresh strict
generation/refusals recorded; [ ] fresh survey lib/bin, all-targets and
test-no-run inventories compared, with removed/retained/new fingerprints and
census changes explained; [ ] claim boundary and remaining dependencies
recorded. Early Campfire Cargo inventories are expected to remain red; they
are measurements, not required green gates. Fresh generation must precede
checking its generated project.

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
| 2026-10-10 (canonical-main integration and CacheControlStore Rust gate repair; local working tree) | Published base `74dce7306cdc70eb3af7f4068a8c7af677d894a2` + staged merge of canonical `main` `aaff26906c66300a7ff0a3d1b1d96e7a391786e9` and unstaged fixes | Current pin `edbc779f4dfc9b26c36310881711ddc976a9dfc8` | rustc/Cargo 1.98.1, Linux x86_64; generated real-blog project lock is fixture-managed | `cargo test --locked --test rust_each_iteration`; `cargo test --locked --test cache_control_header`; `ruby -Iruntime/ruby runtime/ruby/test/action_controller/cache_control_test.rb`; ignored generated real-blog `cargo test --locked --test rust_toolchain real_blog_cargo_test_passes -- --ignored --exact --nocapture`; `cargo check --locked --all-targets`; fresh strict analyzer and Rust generation on Campfire | Rust iteration tests 3/3; Cache-Control integration tests 4/4 with 1 native-Spinel test ignored; CacheControlStore runtime unit suite 40 runs / 48 assertions; generated real-blog Rust Cargo gate 1/1; all-target check passed. The prior real-blog failure is fixed by initializing fields directly in `new`, routing custom `empty?` calls to the emitted predicate name while preserving typed Array `is_empty`, and using indexed loops for the nullable-array iteration emitter mismatch. Strict Campfire analysis is 0 errors / 460 warnings. Strict Rust generation remains blocked at 132 unsupported/syntax + 113 type errors (245 total) on `Data.define`; no generated Cargo project or current-pin rustc count. PR #688 remains OPEN/Draft at published `74dce730`; GitHub reported CodeRabbit SUCCESS only and no required checks on that head. Canonical main has been merged locally but merge commit is not yet created. | Restored the real-blog Rust compiler/execution forcing gate; no Campfire count delta claimed; keep the Data factory gate until nominal cache-key behavior is proven |
| 2026-10-10 (Data member inference repair; published at `756ec909`, docs refreshed at `83be1d40`) | Code batch `756ec909929f9165ade62d45179ee1c431a779ba`; docs-only `83be1d4054d5e2ae4f289d0409f382188d3a6aff` | Current CI pin `edbc779f4dfc9b26c36310881711ddc976a9dfc8` | rustc/Cargo 1.98.1; Linux x86_64; root lock unchanged | `cargo test --locked --test emit_and_run data_define_block_methods_belong_to_the_data_class -- --nocapture`; `cargo test --locked --test data_factory_constants block_data_classes_retain_ordered_members_as_nominal_origin -- --exact`; `cargo test --locked --test rust_toolchain real_blog_controller_identity_values_match_rails -- --ignored --nocapture --exact`; `cargo test --locked --lib action_controller_runtime_emit_typechecks_hotspots -- --nocapture`; `cargo check --locked --all-targets`; strict `cargo run --locked --bin roundhouse -- --target rust /tmp/campfire-edbc779-clean -o /tmp/rh688-83be-strict`; clean-target c20 identity test and `emit_preview` + generated release build with exact CI fixture; `git diff --check`; PR status | The Campfire Data block runtime journey passes 1/1 after excluding synthetic Data member-reader harvests whose inferred type has no informative core beyond Nil; the constructor-typed member inference regression passes 1/1. The fixed-head ActionController identity test passes locally 1/1 and the emitter assertion passes 1/1. Roundhouse `cargo check --locked --all-targets` and `git diff --check` pass. Strict generation at the updated code state exits 1 with exactly 132 unsupported/syntax and 113 type diagnostics (245 total), still refusing `Data.define` before project emission; no current-pin Cargo count is available. The prior exact-head run [38063332818](https://github.com/rubys/roundhouse/actions/runs/38063332818) remains red on unit shards 0/2, Rust compare/smoke and `compact-required`; Campfire compare/conformance pass and Campfire smoke is skipped. We downloaded the exact CI fixture artifact (SHA-256 `6199e048b38b866790c5956868b9e1960497e2ddded3d2b67e6b994cad06350a`); at c20 with this fixture and a fresh Cargo target, the identity test passed 1/1 and the compare lane's emitted release app built successfully. The local `action_controller_base.rs` SHA-256 is `6fa7b2ea02c1cbaadcbb9dd96c9746f13d53f2bfa2d2e51b86314c5e7fffe7db`; it contains neither `CacheControlStore` nor `self.extras`. The CI failure remains unresolved and not reproducible. PR was verified OPEN/Draft at `fb2144e7`; only CodeRabbit SUCCESS was present and review was explicitly skipped for Draft. | Fixes the Data method journey's `String + nil` error without weakening its test; strict-generation boundary and CI discrepancy remain open |
| 2026-10-10 (canonical-main integration + local Rust check repairs; unpublished working tree) | Merge `030052820334dbcc9883f4763c43cd6e3e2f3e39` plus four uncommitted files | Current CI pin `edbc779f4dfc9b26c36310881711ddc976a9dfc8` | rustc/Cargo 1.98.1; root lock SHA-256 `206b0c494651b039351ec2a3e6232041e87e4f36232a111ea6086d4a3e8a5981`; Linux x86_64 | `cargo check --locked --all-targets`; `cargo test --locked --test rust_toolchain --test route_path_decoding`; ignored emitted-real-blog gate `cargo test --locked --test rust_toolchain real_blog_cargo_test_passes -- --ignored --exact --nocapture`; strict current-pin generation; `git diff --check` | Canonical main merged locally with no unresolved conflicts. All-target check passed; regular Rust toolchain tests 3/3 and route-path tests 8/8 passed (7 ignored); emitted real-blog Rust Cargo test 1/1 passed. Local generated-model assertion confirms `_insert_row` fills timestamps and uses the raw adapter insertion path; route matcher fixture was updated to its four-argument API; view partial inference now has an explicit collected vector type. Latest strict generation still reports 245 frontend diagnostics (132 unsupported/syntax + 113 type) and refuses `Data.define` before project emission, so the current-pin Rust/Cargo error count is unavailable. The ignored forwarded optional String block regression remains red with five generated Rust errors; this integration is not a Campfire compile milestone. `cargo check` emits existing unused-code warnings. | Mainline integration locally checked; current compiler boundary and D0 failure remain accurately open |
| 2026-10-10 (Data factory Rust-emitter probe; temporary test removed) | `183a2aa4` plus local plan update | Current CI pin `edbc779f4dfc9b26c36310881711ddc976a9dfc8`; minimal source reproduces `FragmentCache::ContentKey` | rustc/Cargo 1.98.1; root lock SHA-256 `206b0c494651b039351ec2a3e6232041e87e4f36232a111ea6086d4a3e8a5981` | Temporary `cargo test --locked --test rust_data_factory_probe -- --nocapture`; direct `roundhouse::emit::rust::emit` on a minimal ingested Data factory; probe file deleted after inspection | Probe harness passed 1/1 and printed the actual emitted classes: owner `ContentKey` binding is a TODO; generated class has no fields or constructor and derives `Default`; `cache_key` is emitted with an untyped `serde_json::Value` result and unresolved bare `digest()` call. This source inspection bypassed project validation intentionally and is not an emitted-crate compile/run or a feature test. The production `Data.define` refusal is necessary; no Rust support or current-pin Cargo count is claimed. | D-Data.2/3 remain blocked; implement complete nominal constructor/member/method and heterogeneous identity flow before removing gate |
| 2026-10-10 (local Rust ownership fix + exact-head CI triage; published at `da57db7f`) | `da57db7f789e851078e8f55fd70f22e0628a8a24`; local changes include `runtime/ruby/action_controller/base.rb`, `src/emit/rust/library.rs`, and this plan | Current CI pin `edbc779f4dfc9b26c36310881711ddc976a9dfc8` | rustc/Cargo 1.98.1; root lock SHA-256 `206b0c494651b039351ec2a3e6232041e87e4f36232a111ea6086d4a3e8a5981` | `cargo test --locked --test data_factory_constants`; `cargo test --locked --lib action_controller_runtime_emit_typechecks_hotspots -- --nocapture`; `cargo test --locked --test docs_references doc_path_references_resolve -- --nocapture`; ignored generated Rust tests `real_blog_controller_identity_values_match_rails` and `inflector_test_passes_under_rust`; `cargo check --locked --all-targets`; exact-head `ci:rust` run [38049749293](https://github.com/rubys/roundhouse/actions/runs/38049749293) and canonical-main full run [38049081004](https://github.com/rubys/roundhouse/actions/runs/38049081004) log inspection | Data tests 12/12; focused action-controller assertion 1/1; docs reference 1/1; generated real-blog Rust identity test 1/1; generated Rust Inflector test 1/1; Roundhouse all-target check passed with existing warnings. Local fix converts the stored header key to `String`, matching `Vec<String>` and the current `store_value` delegation; stale local assertions/path references were corrected. The PR run predates these edits and is red as detailed above. The exact `rooms.messages_count` difference (Rails 1, emit 2) is also present on canonical main, so it is not established as a PR regression, but remains an unresolved semantic mismatch. Exploratory ignored `ac_base` generated-Rust test still fails in generated test-harness compilation; the Ruby counterpart could not run because required gems are absent. Campfire smoke and post-edit differential were not rerun; there is no current-pin Cargo inventory or browser-smoke evidence. | Rust ownership fix and focused gates verified locally; preserve the Data generation gate; investigate the shared baseline mismatch and run Campfire smoke |
| 2026-10-10 (exact-head `ci:rust` diagnosis + current-pin gate refresh) | Published `f228175fde7d8dacbf7bbbbe3045b986a6e92f28`; local changes after that head are uncommitted | Current CI pin `edbc779f4dfc9b26c36310881711ddc976a9dfc8` | rustc/Cargo 1.98.1; root lock SHA-256 `206b0c494651b039351ec2a3e6232041e87e4f36232a111ea6086d4a3e8a5981` | `gh pr view 688`; failed-job logs for run [38049749293](https://github.com/rubys/roundhouse/actions/runs/38049749293); `cargo test --test data_factory_constants`; strict Rust generation and `--survey --allow-unsupported` generation; focused `action_controller_runtime_emit_typechecks_hotspots` probe | PR remains OPEN/Draft at `f228175`. CI failures: stale `set_index` assertion, stale server-module path in the plan, `Vec<String>::push(&str)` in Rust compare/smoke, and `rooms.messages_count` Rails-vs-emit mismatch (1 vs 2); `campfire-conformance` passed, while Campfire smoke was skipped. Data factory tests pass 12/12. Current-pin production and survey generation both stop at Data; strict generation reports 132 unsupported/syntax + 113 type errors (245 total), so no current-pin Cargo count. The local action-controller probe confirmed the existing assertion was stale because validation lives in `store_value`; it also exposed the owned-String push gap. Oracle recommends keeping the current gate until Data's nominal identity survives the actual heterogeneous `flatten`/`is_a?` cache-key consumer. Local source/test/doc edits fix the compile bug and stale assertions/references, but have not yet passed the emitted Rust runtime test or Campfire differential; the CI run predates these edits. | Re-triage Data boundary; address exact-head CI regressions and verify the DB differential before publication |
| 2026-10-10 (canonical-main merge + current-pin generation gate + comparison rebaseline) | `863dd24bd436cc9096fb4e9822c8bca1e77e7bc9` | Current CI pin `edbc779f4dfc9b26c36310881711ddc976a9dfc8`; comparison pin `32b4144b5206304fa8d4c67455a753e2d3c16635` | rustc/Cargo 1.98.1; generated lock SHA-256 `b1ae75ef5b9f85e8166d707f9b3babcc0897f6f392c09e2ab99ccd2100d93f55`; Linux x86_64 | Canonical `main` fetch/merge at `5d3d144e`; `cargo check --locked --all-targets`; ignored cookie lifecycle and transport harnesses; emitted-Ruby ordinalize regression; strict `roundhouse check --strict` on `edbc779f`; exact current-pin Rust survey attempt; fresh `32b4144b` survey and generated `cargo check --locked --lib --bin app --message-format=json`; Cargo JSON indexing and inventory comparison against `898b4606` | Merge conflict in `tests/support/native_http.rs` resolved retaining both session defaulting and explicit-cookie override; all-target check passed; production cookie lifecycle 1/1, transport 1/1, ordinalize emitted-Ruby test 1/1. Current-pin strict analyzer: 0 errors / 460 warnings. Current-pin Rust survey exits 1 before writing output: `Data.define` explicitly unsupported, so Cargo count unavailable. Older-pin survey at `863dd24b` emits 488 files and Cargo lib/bin exits 101 with 2,577 errors / 575 warnings / 2,339 fingerprints. Against `898b4606`/older pin (2,599/572/2,361), 23 fingerprint groups/errors disappear and one appears; net count −22, not yet root-cause classified. Canonical main moved `CAMPFIRE_SHA` to `edbc779f`; update all future scope labels and rebaseline per pin. No PR push was performed in this batch. | Main merge checked locally; current-pin Data blocker identified; older-pin comparison baseline refreshed; A0/F0 behavior remains open |
| 2026-10-10 (production cookie-stack regression + exact-head survey) | `898b4606ba85382e816976eaf8ee79121c5b18cc` | `32b4144b5206304fa8d4c67455a753e2d3c16635` | rustc/Cargo 1.98.1; generated lock SHA-256 `b1ae75ef5b9f85e8166d707f9b3babcc0897f6f392c09e2ab99ccd2100d93f55`; Linux x86_64 | `cargo test --locked --test rust_server_cookie_lifecycle -- --ignored --nocapture`; `cargo test --locked --test rust_http_transport -- --ignored --nocapture`; `cargo check --locked --all-targets`; `rustfmt --edition 2024 --check runtime/rust/server.rs tests/rust_server_cookie_lifecycle.rs`; `git diff --check`; exact-head survey `cargo run --locked --bin roundhouse -- --target rust --survey --allow-unsupported /tmp/rh688-baseline/campfire -o /tmp/rh688-898-survey`; generated `cargo check --locked --lib --bin app --message-format=json`; diagnostic indexing and fingerprint comparison | Production-stack harness passed 1/1; transport harness passed 1/1; Roundhouse all-target check passed with existing warnings; changed Rust files pass rustfmt and whitespace checks. The pinned Campfire survey emitted 488 files (489 including Cargo.lock); generated app check exited 101 with 2,599 errors / 572 warnings / 2,361 fingerprints. Comparing to `33c1bc0` shows identical file census and identical diagnostic fingerprints/counts; only the generated server module changed. This is not a compiler-wall reduction or a working build. Separate Rails callback probe remains partial; full pinned Rails boot and nil-header wire behavior are unverified. Exact-head GitHub REST after publication confirmed Draft/open, no workflow checks, and CodeRabbit review skipped. | F0.1 checked; A0.1 partial; A0 config and Rails CookieJar/signing/session remain open |
| 2026-10-10 (canonical-main merge + Rails/cookie foundations) | `33c1bc0f8509447b5d988e0a05d8712ac8cae277` (includes merge `4a70cd0e3e4d67004fb247448329a1bab0d754df`, parents PR base `5647c88e3e48e8e3fcfda31b2c761514d873e4af` and canonical `main` `cd8656979a9e9c13eb5a25e0bbcc6da5d793565e`) | `32b4144b5206304fa8d4c67455a753e2d3c16635` | rustc/Cargo 1.98.1; generated lock SHA-256 `b1ae75ef5b9f85e8166d707f9b3babcc0897f6f392c09e2ab99ccd2100d93f55`; Linux x86_64 | `cargo test --locked --test rails_runtime` (3 tests); ignored isolated transport harness `cargo test --locked --test rust_http_transport request_cookie_transport_is_request_scoped_and_appends_headers -- --ignored --exact --nocapture` (1 test); Roundhouse `cargo check --locked --all-targets`; `roundhouse check --strict`; strict Rust generation; fresh clean-base and dirty-tree survey `cargo check --locked --lib --bin app`; diagnostic inventory comparison; `git diff --check`. Full `cargo fmt --all -- --check` was also run and reports extensive existing rustfmt drift throughout unrelated files; no repo-wide formatter was run | Rails runtime tests 3/3, transport harness 1/1, Roundhouse all-target check passed, strict analyzer 0 errors/407 warnings, diff whitespace check passed. Strict Rust generation remains blocked at 74 unsupported/syntax + 109 type diagnostics. Clean merged-base Campfire survey: 2,597 errors/571 warnings/2,359 groups. Survey output from the committed source: 2,599/572/2,361; inventory comparison: 111 groups / 118 errors removed, 113 groups / 120 errors added. This is not a net reduction or a strict build. PR was published in Draft state; no exact-head workflow/check-run had executed at inspection time. Captures/inventories are in `/tmp/rh688-base-4a70-*` and `/tmp/rh688-current-4a70-*` | Main synced and published; A0 Rails path and F0 request transport foundations tested; A0 config and F0 Rails jar remain open; fresh compiler counts updated |
| 2026-10-09 (pre-inventory CI observation) | `3b6d1b7576036382f82aa936fef8bcbd5b65272c` | `32b4144b5206304fa8d4c67455a753e2d3c16635` | PR pin 1.98.1; no local Cargo lock measurement yet | CI run [37986454230](https://github.com/rubys/roundhouse/actions/runs/37986454230): Rust compare, Campfire compare/conformance passed at last inspection; Campfire browser smoke and extra-target jobs were skipped | No local compiler inventory in that observation; superseded by the baseline row below | Historical only |
| 2026-10-09 (survey baseline) | `3b6d1b7576036382f82aa936fef8bcbd5b65272c` | `32b4144b5206304fa8d4c67455a753e2d3c16635` | rustc/Cargo 1.98.1, host `x86_64-unknown-linux-gnu`; root lock SHA-256 `206b0c494651b039351ec2a3e6232041e87e4f36232a111ea6086d4a8e8a5981`; generated lock SHA-256 `b1ae75ef5b9f85e8166d707f9b3babcc0897f6f392c09e2ab99ccd2100d93f55` | Strict `roundhouse check --strict`: exit 0, 0 errors, 404 warnings. Strict Rust generation: exit 1 before project output (72 unsupported/syntax + 109 type diagnostics). Survey `--allow-unsupported` output: Cargo `check --locked --lib --bin app` exit 101, 2,496 errors/555 warnings; `check --all-targets` adds 165 lib-test errors; `test --no-run` fails on those targets. Full captures and exits in `/tmp/rh688-baseline` | 2,252 fingerprints for lib/bin; not strict production support. Generated survey project did not provide Cargo.lock; captured lock was generated and held constant. Repeat generation/census pending. | P0.1, P0.5–P0.6, P1.1–P1.4, P1.6a |
| 2026-10-09 (repeatability check) | `609248bcf7f51c94d56f91fbdaaf6675dd5b71fe` (docs-only difference from baseline code SHA `3b6d1b7`) | `32b4144b5206304fa8d4c67455a753e2d3c16635` | Same captured generated lock SHA-256 `b1ae75ef5b9f85e8166d707f9b3babcc0897f6f392c09e2ab99ccd2100d93f55`; Roundhouse binary reported `2026.9.18 (609248bc)` | Two fresh survey generations with `--survey --allow-unsupported`; generated-file manifests compared before Cargo. Both then ran `cargo check --locked --lib --bin app --message-format=json` with a shared Cargo target cache | Both generated 487 files with identical manifest SHA-256 `955ffa2ca632700fe2c0697c7346df29956299b97e964a6ec45a4e17e650c71b`; both Cargo runs exit 101 with 2,496 errors, 2,252 groups and no malformed JSON; normalized fingerprint+count lists identical. Outputs/captures in `/tmp/rh688-repeat` | P0.3, P0.7 |
| 2026-10-09 (signature-map fix inventory) | `1e5b828895dcafaa687334756ed0f8c908c27451` | `32b4144b5206304fa8d4c67455a753e2d3c16635` | rustc/Cargo 1.98.1; reused generated lock SHA-256 `b1ae75ef5b9f85e8166d707f9b3babcc0897f6f392c09e2ab99ccd2100d93f55` and baseline Cargo target cache; fresh generated project in `/tmp/rh688-1e5b8288-survey` | Fresh `roundhouse --target rust --survey --allow-unsupported` then `cargo check --locked --lib --bin app --message-format=json` | Exit 101, 2,496 errors, 2,252 fingerprints, 555 warnings; fingerprint+count inventory exactly matches baseline (0 groups removed/added/changed). The 14 unresolved `capture` occurrences in 13 groups are unchanged. This narrow parameter-map fix is correct but does not reduce the Campfire wall; no support claim. Capture JSON/inventory in `/tmp/rh688-1e5b8288-cargo.json` and `/tmp/rh688-1e5b8288-inventory.json` | D0 adjacent signature-map defect covered; D0 ABI remains open |
| 2026-10-09 (shared capture runtime placement, working tree based on `1e5b8288`) | Parent `1e5b828895dcafaa687334756ed0f8c908c27451` plus uncommitted changes | `32b4144b5206304fa8d4c67455a753e2d3c16635` | rustc/Cargo 1.98.1; reused generated lock SHA-256 `b1ae75ef5b9f85e8166d707f9b3babcc0897f6f392c09e2ab99ccd2100d93f55`; dirty worktree | `ruby -Iruntime/ruby runtime/ruby/test/action_view/view_helpers_ext_test.rb`; `cargo test --locked --test runtime_src_integration every_runtime_method_body_is_fully_typed`; fresh survey generation and `cargo check --locked --lib --bin app --message-format=json`; generated Rust framework test with `--ignored`, before and after moving direct generic capture probes | CRuby suite passed 21 tests / 38 assertions; runtime typed-body gate passed 1/1. Fresh survey Cargo check: exit 101, 2,510 errors / 554 warnings (baseline `1e5b8288`: 2,496 / 555). Diagnostic-set comparison by code/message/source span: 14 `capture` E0425s and one clone diagnostic disappeared; 14 E0271 callable-result mismatches, 14 E0308 branch mismatches, and one relocated clone diagnostic appeared. This is not a net compiler improvement; it replaces unresolved capture names with concrete evidence that the current closure is `FnOnce() -> ()` where the runtime expects a value, while the survey output still fails. Rust framework harness failed with 24 generated-test compilation errors before probe relocation and 21 after; the three removed errors were from those new probes. Remaining failures include fixture/type-shape mismatches whose baseline status was not tested, so the Rust framework lane remains red. CRuby syntax checks and `git diff --check` passed. | Shared runtime placement locally implemented; capture ABI is now more directly localized; D0 behavior remains unproven |
| 2026-10-10 (D0 source-callsite regression expansion; dirty worktree) | `83f6d67acd5b4e38dc4a045f4c8cac20ccee20a9` plus local test/plan/recognizer changes | `32b4144b5206304fa8d4c67455a753e2d3c16635` | rustc/Cargo 1.98.1; current Roundhouse checkout `pr688-prep`; generated scratch project at `/tmp/roundhouse-rust-check-optional-string-forwarded-block` | `cargo test --lib emit::rust::block_abi::tests -- --nocapture`; `cargo check --lib`; ignored emitted-Rust test `forwarded_optional_string_block_runs_through_two_edges` with the source-authored `render_html` literal-block callsite; `git diff --check` | ABI probe unit tests passed 2/2; library check passed; `git diff --check` passed. The emitted-project test intentionally remains red (exit 101): five app compile errors, including the new source literal closure not matching the current required `Box<dyn FnOnce()>` signature, plus the pre-existing optionality, closure result, and `String`/`Value` mismatches. This reproduces the pre-fix path and expands evidence; no feature behavior is fixed and no Campfire inventory was rerun. | D0 positive source-callsite fixture added; production ABI integration and Campfire claim remain open |
| 2026-10-10 (D0 proof-path check; dirty worktree) | `3fdeda76496c31a3b2e45d4d0fcf444ccaea5cc4` plus local test/plan edits | `32b4144b5206304fa8d4c67455a753e2d3c16635` | rustc/Cargo 1.98.1; emitted scratch project `/tmp/roundhouse-rust-check-optional-string-forwarded-block` | Re-ran the ignored D0 compile/run test after switching fixture generation to `session::analyze_and_lower`; inspected lowering order and lambda typing in `src/session.rs`, `src/lower/mod.rs`, `src/lower/tag_builder.rs`, `src/lower/capture_inline.rs`, `src/lower/view_to_library/walker.rs`, and `src/analyze/body/mod.rs` | The full emitted test still fails at Cargo with the same five app errors; shared lowerings do not change this synthetic handwritten `capture` fixture. This run proves the shared lowering path is exercised but does not exercise Campfire tag-builder rewrites, view templates, escaping, or a real Campfire compile improvement. Local source confirms `Ty::Fn.params` is empty for analyzed lambdas; lambda arity must be read from its own parameter fields. No new feature behavior is claimed. | Production-path repro harness verified; no ABI integration yet |
| 2026-10-10 (D0 reproduction refreshed at `bdb0651b`) | `bdb0651b6f4dd81515ad3de3bb7509edd340213a` | `32b4144b5206304fa8d4c67455a753e2d3c16635` | rustc/Cargo 1.98.1; focused test generated fresh project under `/tmp/roundhouse-rust-check-optional-string-forwarded-block` | `CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_PROFILE_TEST_STRIP=symbols cargo test --locked --test rust_toolchain forwarded_optional_string_block_runs_through_two_edges -- --ignored --exact --nocapture` | Exit 101 after emitted app compilation: five errors remain—one E0599 for `.clone().is_none()` on required `Box<dyn FnOnce()>`, one E0271 because the closure returns `()` instead of `Value`, and three E0308 mismatches across String/Value returns and direct closure/boxed callback. The test did not reach behavior assertions. No production behavior changed, no Campfire survey was run, and no compiler-wall improvement is established. | Confirms exact D0 pre-implementation failure at current code head |

| Date | Roundhouse SHA | Campfire SHA | Toolchain / locks | Commands and executed scope | Result / artifact links | Checklist updated |
|---|---|---|---|---|---|---|
| 2026-10-09 (review-observation verification; local worktree based on `aecb0b82`) | Base `aecb0b8216a823748eb752e72aa674f90be81515` plus one request-shadowing regression test and plan update | `32b4144b5206304fa8d4c67455a753e2d3c16635` | rustc/Cargo 1.98.1; existing repo lock | `cargo test --locked --lib bare_request_intrinsic_respects_instance_method_shadowing`; `cargo test --locked --lib regex_case_uses_option_branches_only_in_an_option_return_tail`; `cargo test --locked --lib turbo_stream_collection_as_local_seeds_the_actual_partial_contract`; `cargo test --locked --test turbo_stream_views`; `git diff --check` | Request-shadow regression passed 1/1 for both intrinsic and shadowed paths; regex Option-tail test passed 1/1; bare Turbo Stream analyzer regression passed 1/1; `turbo_stream_views` passed 15/15. The initial request test exposed an incorrect expected rendering (`request()` instead of `self.request()`); corrected expectation passed. Direct `rustfmt --check` is not clean: it reports unrelated existing formatting drift through the module tree; no formatter was run to avoid broad changes. These results verify existing review fixes on this local base, not exact-head CI or D0. | Three later CodeRabbit observations verified; request shadowing now has direct regression coverage |

### Latest review observations

The three CodeRabbit observations checked against the `aecb0b82` worktree
are verified by the tests named in the current snapshot and by the newly
added `bare_request_intrinsic_respects_instance_method_shadowing` regression.
The first request-shadow test run caught a wrong test expectation
(`request()` instead of the actual `self.request()`); the corrected test
passes. Direct `rustfmt --check` remains unavailable as a clean gate because
it reports pre-existing formatting drift in adjacent Rust emitter modules;
no formatter was run, and `git diff --check` passes. These are local checks,
not exact-head CI or Campfire D0 evidence.

### Reproduction command template

Fill in the actual paths and save the complete output in the evidence
artifact—not just this command template—before claiming a baseline:

```sh
APP=/path/to/once-campfire-at-32b4144b5206304fa8d4c67455a753e2d3c16635
OUT=/tmp/campfire-rust-<roundhouse-sha>
roundhouse check --strict "$APP"
roundhouse --target rust "$APP" -o "$OUT"
(cd "$OUT" && cargo check --locked --lib --bin app --message-format=json)
(cd "$OUT" && cargo check --locked --all-targets --message-format=json)
(cd "$OUT" && cargo test --locked --no-run --message-format=json)
```

If strict generation refuses before producing `$OUT`, preserve that result,
triage the refusal at its source, and do not substitute a weakened run as the
baseline.
