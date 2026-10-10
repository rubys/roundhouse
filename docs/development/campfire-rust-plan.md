# Campfire on Rust: compile-to-working plan

**Status:** the pinned input and diagnostic inventory are reproducible, and
canonical `rubys/roundhouse:main` has been merged locally into the PR branch.
Strict Rust generation still refuses the app before Cargo. The current Rails
path facade and request-cookie transport are tested foundations, not a
Campfire compile improvement: the clean merged-base survey has 2,597 errors;
the dirty working tree has 2,599. Source-level root-cause triage and semantic
implementation work remain open.

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
| PR status | [#688, Draft](https://github.com/rubys/roundhouse/pull/688), OPEN, base `main`; REST inspection on 2026-10-10 confirms remote head `5647c88e3e48e8e3fcfda31b2c761514d873e4af`. Its latest workflow run [38038649634](https://github.com/rubys/roundhouse/actions/runs/38038649634) is red in `compare (rust)`, `smoke (rust)`, two of three unit shards, `compact-required`, and `CI summary`; Campfire compare/conformance and CodeRabbit status are successful, while Campfire smoke and extra-target lanes are skipped. The current local branch has an unpushed canonical-main merge at `4a70cd0e3e4d67004fb247448329a1bab0d754df`; it has no exact-head CI result. Keep Draft and do not merge unless Thomas explicitly instructs otherwise |
| Local integration base | `4a70cd0e3e4d67004fb247448329a1bab0d754df` merges canonical `rubys/roundhouse:main` at `cd8656979a9e9c13eb5a25e0bbcc6da5d793565e` into the PR branch whose parent was `5647c88e3e48e8e3fcfda31b2c761514d873e4af`; the one `tests/support/native_http.rs` conflict preserved upstream session-aware `get()` and the existing explicit-cookie override in `get_with_headers()`. This merge is local and unpushed |
| Campfire | CI pin and checked-out SHA `32b4144b5206304fa8d4c67455a753e2d3c16635` |
| Strict analyzer | `roundhouse check --strict`: exit 0, 0 errors, 407 warnings |
| Strict Rust project generation | Exit 1 before writing a project: 74 unsupported/syntax diagnostics and 109 type diagnostics (183 reported errors total). This is a generator refusal, not a rustc count |
| Survey-only emitted Rust | The clean merge-base and dirty-worktree surveys were emitted separately with `--survey --allow-unsupported`; each generated lock is SHA-256 `b1ae75ef5b9f85e8166d707f9b3babcc0897f6f392c09e2ab99ccd2100d93f55`. The current dirty survey contains 489 files, including 377 Rust source files |
| Historical repeatability and output census | Two pre-merge generations at `609248bc` each emitted 487 files with identical manifest `955ffa2ca632700fe2c0697c7346df29956299b97e964a6ec45a4e17e650c71b` and 2,496 errors / 2,252 fingerprints. Their detailed census is historical, not the current merged-base census. The earlier 1,918-entry post-build hash list is not used as the emitted-file census |
| Survey lib/bin compiler result | On the clean merged base `4a70cd0`, `cargo check --locked --lib --bin app` exits 101 with 2,597 errors / 571 warnings in generated `app`. The current dirty tree exits 101 with 2,599 errors / 572 warnings. The diagnostic inventories have 2,359 and 2,361 fingerprints, respectively; they differ by 111 removed / 113 added groups (118 removed / 120 added error instances). The changed count is **not** a net reduction |
| Survey all-target/test results | The previous 2,496-error all-target result is historical and predates the canonical-main merge; it is not a current all-target Campfire measurement. Roundhouse itself passes `cargo check --locked --all-targets` on the current dirty tree. Do not conflate these separate checks |
| Compiler diagnostic distribution (dirty survey lib/bin) | E0308 824; E0599 682; E0425 560; E0433 134; E0609 71; E0277 57; E0423 50; E0061 49; remaining codes are in `/tmp/rh688-current-4a70-inventory.json`. The reduction in unresolved `Rails` names was largely replaced by unresolved methods on `Rails`; counts and fingerprints are not root causes |
| Historical estimate | ~2,468 was a prior rough figure with unknown scope. Use 2,597 as the clean merged-base survey baseline and 2,599 as the current dirty-tree measurement; neither is strict production support |
| Environment | Debian 12, Linux x86_64; `rustc 1.98.1 (48a229cea 2026-09-01)`, host `x86_64-unknown-linux-gnu`; compiler binary SHA-256 `859254978c0a0402c32f949f6de0d99aee73be8d15f45aac00ae1448aac51e74`; Cargo 1.98.1 binary SHA-256 `da77c8b33849312255ccde3179198ada4c8deb370488d050286146b1d1b27e14`; Roundhouse root lock SHA-256 `206b0c494651b039351ec2a3e6232041e87e4f36232a111ea6086d4a8e8a5981`; generated lock SHA-256 `b1ae75ef5b9f85e8166d707f9b3babcc0897f6f392c09e2ab99ccd2100d93f55` |
| Rails oracle | Not prepared. Required for Rails equivalence, not for compiler diagnostics |
| Exact-head CI | The current remote PR head is still `5647c88`; see the workflow state above. The local merge commit and uncommitted files have no CI evidence. The `ci:rust`/`ci:full` results from earlier heads do not validate this local state |
| Review state | GitHub's review decision was empty at the last check. The latest CodeRabbit review at `0b2323e` left two low-priority performance observations (regex literal recompilation and duplicate analyzer write-site survey), deferred absent concrete evidence. Three subsequent CodeRabbit observations were inspected: bare `turbo_stream` receiver support and regex-case Option-tail behavior are covered by existing implementation/tests; the bare `request` instance-method shadowing guard is present and now has a focused regression. No production-code change was needed for those observations. The browser room-delete journey is still explicitly unverified on Campfire smoke |
| Scratch evidence | Baseline and dirty surveys, Cargo JSON and indexed diagnostic groups live under `/tmp/rh688-base-4a70-*` and `/tmp/rh688-current-4a70-*`; older command outputs and locks remain under `/tmp/rh688-baseline` in the current orb. Large scratch output is not committed |

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
source root-cause map. Strict generation still stops at its 181 reported
front-end diagnostics, before a production Cargo project exists.

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
  Strict generation is blocked by 181 compiler-front-end diagnostics; survey
  lib/bin has 2,496 rustc errors; `--all-targets` adds 165 lib-test errors.
- [ ] **P1.6b** Manually assign root-cause cluster membership to the inventory.
  The error-code distribution above is a prioritization hint, not a root-cause
  classification. Do not report only totals: foundation fixes can expose
  additional methods and increase counts while improving coverage.

## Phase 2 — Triage and dependency waves

Re-sort this table after the baseline. The listed topics are candidates from
prior work, not promises that they remain the dominant errors.

| Wave | Work | Exit condition |
|---|---|---|
| 0 — evidence readiness | Pinned inputs; complete provenance; repeat generation/fingerprint comparison; generated module/class/route/test census; map the 181 strict generator refusals | Baseline can be rerun and compared; refusals have source-level owners, not just counts |
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

Start mapping the 181 strict-generation refusals alongside rustc triage, rather
than waiting for whole-project convergence. In every fresh build, inventory
reachable `todo!`, silent/default/no-op methods, 501 routes, and omitted
methods/modules. A green `cargo check` is not completion if accepted paths
panic, fabricate defaults, or refuse required routes.

### Dependency-linked work packages

Do not activate a package until its prerequisite/interface is recorded here
with a minimal reproducer, owned files, semantic checks, and integration check.

| Package | State | Scope and dependency | Completion evidence |
|---|---|---|---|
| **A0 — typed Rails namespace/application interface** | Partial: typed Rails root/public-path runtime is emitted, app callsites compile in an isolated harness, and Ruby/Rust path behavior tests pass; `Rails.application` and Campfire config are still missing | Integrate the existing typed shared `Rails::Env`, `AppPath`, `Cache`, and `Logger` behavior into Rust once; define a finite application/config contract for app-version fallback, optional git revision/VAPID credentials, routes, and request-vs-no-request protocol/domain. Keep route helpers owned by existing route lowering. No empty `Rails` class or invented defaults. Central generated-module wiring is integrated by the single integration owner. | Campfire source-to-generated mapping for every Rails-root fingerprint; tests for `APP_VERSION`/`GIT_REVISION` fallback, optional credential/VAPID values, request-derived URLs and no-request defaults, root/path behavior, cache behavior, and route access; then re-inventory newly exposed methods and separate sibling namespaces. |
| **A1 — WebPush pool service** | Not started; separate follow-on | Separate from A0’s scalar/config interface: Campfire initializes a callback-bearing `WebPush::Pool`, mutates/replaces it in tests, and calls queue/shutdown. Requires a typed pool/callback/lifecycle contract, not a `Value` callback or dummy pool. | Emitted queue/delivery and invalid-subscription behavior, shutdown/replacement lifecycle, and Campfire push journeys; if deferred, retain an explicit unsupported boundary and do not claim push behavior. |
| **D0 — forwarded-block ABI** | Shared capture runtime home implemented locally; standalone representation prototype compiles; signature-map defect fixed; ABI wiring not started | The 14 capture sites span 13 methods: `ClipboardHelper#button_to_copy_to_clipboard`; `Messages::AttachmentPresentation#inline_media_dimension_constraints` (two branches) and `#lightbox_link`; `MessagesHelper#message_area_tag`, `#messages_tag`, `#message_tag`; `QrCodeHelper#link_to_zoom_qr_code`; `Rooms::InvolvementsHelper#turbo_frame_for_involvement_tag`; `RoomsHelper#link_to_room`, `#link_to_edit_room`; `SearchesHelper#search_results_tag`; `Users::FilterHelper#user_filter_menu_tag`; `Users::ProfilesHelper#web_share_session_button`; `Users::SidebarHelper#sidebar_turbo_frame_tag`. These are optional zero-argument content blocks returning HTML text. `sidebar_turbo_frame_tag` has both block and no-block callers. Related `composer_form_tag`, `profile_form_with`, and `auto_submit_form_with` forward form-builder blocks and are compatibility checks, not part of this narrow contract. Current Rust placeholder/forwarding code is in `src/emit/rust/method.rs` and `src/emit/rust/expr/literal.rs`; `block_refine` is documented as same-class only, while these blocks cross helper/runtime boundaries. A local `rustc` prototype for the selected `Option<Box<dyn FnOnce() -> String + '_>>` shape compiled and ran through two forwarding functions, asserting present/absent output, exactly-once invocation, a borrowed local remaining usable, and a moved non-Clone capture. This proves only the Rust representation, not Roundhouse emission or Campfire support. D0 triage also exposed a separate adjacent defect: ordinary Rust parameter typing rejected a valid signature whenever its `Ty::Fn.params` contained the extra block slot. `collect_param_types` and instance parameter rendering now filter that slot; a focused regression pins the ordinary parameter type and both render paths. A fresh post-fix Campfire survey on `1e5b8288` still has exactly 2,496 errors/2,252 fingerprints and all 14 unresolved `capture` occurrences; this adjacent fix did not move the app's compiler wall. An independent Oracle follow-up confirms the two missing information paths: refinement currently only propagates same-class callees, and call emission has no resolved callee ABI context for choosing `Some`/`None` and moving a forwarded callable. A temporary attempt to put general capture block tests in the shared cross-target test file failed Rust test compilation because its untyped block results became `serde_json::Value`; those probes were moved back to the Ruby-family test lane rather than weakening the shared RBS contract. | Next: introduce only a capture-specific cross-boundary contract/refinement and carry it to both signatures and callsites. Pin conflict/declaration-order/optional cases, then verify module+instance emissions; preserve argument-bearing form-builder blocks as a control. Run an emitted native compile/run before surveying all 14 sites. Do not register all of `view_helpers_ext` or add a Rust-only stub. Keep arbitrary non-String capture semantics and the `message_tag` return discrepancy separately visible. |
| **D1 — residual capture semantics** | Blocked on D0 | Implement only the actual forwarded-capture forms, preserving Rails output/buffer, return/fallback, nested behavior, ordering and escaping/safety semantics. | Executed emitted regression cases compare output and fallback behavior, including nested/evaluation-order cases where Campfire uses them; then re-inventory the 14 callsites rather than assuming all share the same semantics. |
| **F0 — request cookie ownership/transport** | Partial: incoming-header parsing, request-owned queueing, multi-header append and a direct middleware harness are implemented/tested; no Rails `cookies` jar consumes them yet | Establish one request-owned jar shared by controller callbacks/helpers/views as required; parse request cookies; queue writes/deletes; append multiple `Set-Cookie` headers without overwriting flash/other headers. Confirm the production `layout_wrap` lifecycle and response finalization path before wiring the jar. | Concurrent requests prove isolation; controller/layout access sees the same jar; response includes correct multiple cookies and attributes; rejection/unwind behavior is explicit. No process-global jar or per-await clone. |
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

**Replan for the current milestone:** the latest clean/dirty surveys show that
Rails-root wiring alone relocates unresolved `Rails` errors into unresolved
`Rails` methods, and the request-transport foundation alone does not remove
any `cookies` callsite. These are groundwork, not a successful diagnostic
reduction. For the immediate Campfire compile goal, finish the higher-yield
Rails/config and cookie slices before returning to D0 implementation. Oracle's
recommended sequence is retained: first prove the version-header/config path,
then the unsigned permanent `last_room` round trip, then the signed permanent
`session_token` path with a real verifier and fail-closed secret handling.
Keep D0 as an independent workstream; do not mix its callable ABI into the
central Rails/config or request-cookie contracts.

#### Next execution checklist — Rails and cookies

All Campfire paths below refer to the pinned source at
`32b4144b5206304fa8d4c67455a753e2d3c16635` under
`/tmp/rh688-baseline/campfire/`. Check a step only when its completion
evidence is recorded in the ledger. The current local path/transport work is
an initial foundation and does **not** check off any complete Campfire
behavior.

- [x] Re-read the pinned source contracts: `config/initializers/version.rb`,
  `app/controllers/concerns/version_headers.rb`,
  `app/controllers/concerns/tracked_room_visit.rb`,
  `app/controllers/concerns/authentication/session_lookup.rb`, and
  `app/controllers/concerns/authentication.rb`.
- [x] Merge canonical `rubys/roundhouse:main` locally at `4a70cd0`; resolve
  the `native_http.rs` conflict without rewriting history. `cargo
  check --locked --all-targets` passes on the dirty Roundhouse checkout.
- [x] Emit the typed shared Rails path facade and pin isolated Rails path
  behavior. This covers `Rails.root`/`public_path` only, not
  `Rails.application`, config, cache, env, or Campfire headers.
- [x] Add request-owned incoming-cookie parsing and queued `Set-Cookie`
  transport, with first-duplicate preservation, encoded-value preservation,
  concurrent-request isolation, multi-header append and invalid-header
  rejection probes. This is transport only; no Rails jar consumes it.
- [ ] **A0.1 — Rails version oracle:** execute the pinned Campfire version
  header action on Rails for `APP_VERSION` + `GIT_REVISION`, blank
  `APP_VERSION` + present revision, both blank, and absent revision. Record
  exact `X-Version` and `X-Rev` behavior (including whether assigning nil
  removes the header); do not assume it from Rust `Option` conventions.
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
- [ ] **F0.1 — production cookie lifecycle:** verify that the production
  `layout_wrap` owns the same request context used by controller callbacks,
  helpers, views and nested async work; prove multiple response cookies survive
  layout wrapping and existing flash/header behavior. Do not rely only on the
  standalone test middleware, which is not itself the Rails cookie jar.
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
- [ ] Re-run fresh strict generation, survey lib/bin and error fingerprints
  after each integrated slice; explain newly revealed downstream methods.
  Record direct removals, retained/new groups, generated-file changes and
  actual executed behavior tests. Do not forecast one-to-one reductions from
  the historical 119 Rails / 104 cookie diagnostics.

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
| 2026-10-10 (canonical-main merge + Rails/cookie foundations; dirty local tree) | Merge base `4a70cd0e3e4d67004fb247448329a1bab0d754df` (parents PR base `5647c88e3e48e8e3fcfda31b2c761514d873e4af` and canonical `main` `cd8656979a9e9c13eb5a25e0bbcc6da5d793565e`), plus uncommitted source/tests/plan edits | `32b4144b5206304fa8d4c67455a753e2d3c16635` | rustc/Cargo 1.98.1; generated lock SHA-256 `b1ae75ef5b9f85e8166d707f9b3babcc0897f6f392c09e2ab99ccd2100d93f55`; Linux x86_64 | `cargo test --locked --test rails_runtime` (3 tests); ignored isolated transport harness `cargo test --locked --test rust_http_transport request_cookie_transport_is_request_scoped_and_appends_headers -- --ignored --exact --nocapture` (1 test); Roundhouse `cargo check --locked --all-targets`; `roundhouse check --strict`; strict Rust generation; fresh clean-base and dirty-tree survey `cargo check --locked --lib --bin app`; diagnostic inventory comparison; `git diff --check`. Full `cargo fmt --all -- --check` was also run and reports extensive existing rustfmt drift throughout unrelated files; no repo-wide formatter was run | Rails runtime tests 3/3, transport harness 1/1, Roundhouse all-target check passed, strict analyzer 0 errors/407 warnings, diff whitespace check passed. Strict Rust generation remains blocked at 74 unsupported/syntax + 109 type diagnostics. Clean merged-base Campfire survey: 2,597 errors/571 warnings/2,359 groups. Dirty survey: 2,599/572/2,361; inventory comparison: 111 groups / 118 errors removed, 113 groups / 120 errors added. This is not a net reduction or a strict build. Remote PR remains Draft at `5647c88`; its workflow run `38038649634` failed and does not validate local changes. Captures/inventories are in `/tmp/rh688-base-4a70-*` and `/tmp/rh688-current-4a70-*` | Main synced locally; A0 Rails path and F0 request transport foundations tested; A0 config and F0 Rails jar remain open; fresh compiler counts updated |
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
