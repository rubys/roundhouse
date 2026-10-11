# Experimental Core Ruby input

`bin/rh materialize` is a **host-only, opt-in research input**. Execute a trusted
Ruby manifest, materialize a declared instance-method cut into ordinary Ruby,
then use Roundhouse's existing ingest, analysis, lowering and emission pipeline.
Round four also corrects a shared source-keyword ABI bug in that pipeline:
ordinary Ruby keyword declarations and verified call packets stay native keywords.
No new IR variant, alternate type checker, per-gem DSL recognizer or target
runtime is introduced. JSON is provenance/test reporting, not an IR input.

## Try it

Prerequisites: the repository-pinned Rust toolchain, MRI 3.4 with development
headers, `cc`, `make`, Prism 1.9.0; the Rails fixture also needs
ActiveModel/ActiveSupport 8.1.4. Nothing here
automatically installs gems. Output directories must be new and their parents
must exist.

```sh
cargo build --locked --jobs 4 --bin roundhouse --bin dump_ir
source_dir="$PWD/tools/native-observer"
observer_dir=$(mktemp -d)
(cd "$observer_dir" && ruby "$source_dir/extconf.rb" && make)
export RUBYLIB="$observer_dir${RUBYLIB:+:$RUBYLIB}"
ruby tools/native-observer/contract.rb
bin/rh materialize --trust-boot tools/boot-to-core/rails_input.rb -o /tmp/rails-core
target/debug/roundhouse check --strict /tmp/rails-core
target/debug/roundhouse --target ruby /tmp/rails-core -o /tmp/rails-core-emitted
```

The input project contains `app/`, `lib/core.rb`, optional `sig/input*.rbs`, and
`materialization.json`. The analyzer reads normal source and RBS, **not** that
JSON file. The source project is separate from the application and is never
written back into it. `check`, LSP and WASM do not boot applications.

## Where it plugs in, and what it could replace

The seam is **before source ingestion**, not a new branch in the analyzer:

```text
Trusted app/gem generation
          |
          v
bin/rh materialize -> ordinary lib/core.rb (+ existing boundary RBS)
          |
          v
ingest_app -> LibraryClass/Expr -> existing analyzer -> existing lowering -> emit
```

`src/ingest/app.rs` already reads ordinary classes/modules under `lib/` through
`ingest_library_classes`; `src/session.rs::analyze_and_lower` already provides
the emit-side analysis/lowering sequence. The exporter uses both existing
stages; the shared source-keyword correction changes their argument handling,
not this pipeline seam.
Ruby Core is a source-level common representation, **not** the final flattened
typed IR: the existing compiler still has to infer types and lower it.

The radical simplification would be to stop translating each generator's
declaration into hand-synthesized methods. The executed ActiveModel case
obtains `name_describe` and its alias from the original generator, without a
recognizer for that DSL. Bodies, private helpers and module ownership flow
through the same ordinary-method input as the unknown closure generator.

| Existing responsibility | Candidate saving / remaining obligation |
|---|---|
| `src/ingest/model.rs` declaration recognition | Generated ordinary bodies could bypass recognition for an admitted generator cut; associations/validation metadata still have runtime consumers. |
| `src/lower/model_to_library/mod.rs` accessor/macro synthesis | Candidate for replacement **only** when original generated bodies and their dependencies compile and execute correctly. AttributeSet, native calls and mutable registries are not solved here. |
| `src/lower/typed_store.rs`, `has_json.rs`, `secure_password.rs`, attachment helpers | Follow-up candidates, not executed support claims. Their serialization, hashing and storage behavior must still exist in emitted output. |
| `src/lower/model_to_library/associations.rs`, `validations.rs` | DB operations, association proxies and callback/validation scheduling are semantics, not just code generation. No deletion is justified by the current body cuts. |
| `src/lower/controller_to_library/mod.rs` | Materialized methods alone do not replace HTTP dispatch, filter order/halting, request/response state or template rendering. |

**No production ingestion/lowering code is deleted in this draft.** The
demonstrated saving is *no additional DSL-specific compiler code* for the
bounded generators, not thousands of existing lines proved redundant.
Next, replace one existing synthesized accessor family behind a differential
original/Core/emitted contract; only then remove its recognizer/synthesizer.

Across dedicated delegate, enum, accessor, CurrentAttributes and class-attribute
builders, a **rough 2–3k LOC candidate corridor** is plausible after successful
contracts. This is a responsibility-level estimate, not measured deletable
code or a percentage of the compiler. It excludes whole model/controller
files and persistence/scheduler/cast/runtime semantics. The generic Ruby
machinery needed to replace those builders may cost more than it saves.

Concrete homes for that estimate are `src/ingest/delegate.rs` and its declaration/
model gates, the enum expansion in `src/ingest/model.rs`, accessor builders in
`src/ingest/library_class.rs` and `src/lower/model_to_library/{markers,schema}.rs`,
`src/ingest/current_attributes.rs`, and `src/ingest/class_attribute.rs`. Their
selected synthesis/declaration/helper ranges total roughly 2.8k physical lines
with overlap removed, including comments and parsing/gating code. That is an
upper candidate corridor, not a measured net saving; shared helpers and static
consumers can prevent removal. `src/lower/current_set.rs` and
`src/lower/enum_symbols.rs` rewrite call sites, not generator definitions.

There is another constraint: check/LSP/WASM must keep working **without boot**.
An optional emission input cannot justify deleting their static recognizers.
That requires a separately designed, provenance-checked generated-source cache
for source consumers, or retaining the static fallback. The strategic benefit
is sharing generator handling across future DSLs, not a guaranteed net LOC cut.

Run the full bounded comparison and inspect commands, stderr and results:

```sh
ruby tools/boot-to-core/try_input.rb /tmp/core-input-results
ruby tests/rh_materialize_test.rb
ruby tests/rh_verify_test.rb
cargo test --locked --jobs 4 --test lowered_ruby_emit -- --test-threads=1
```

`try_input.rb` exits zero only for the declared bounded contracts **and** the
expected strict rejection of its forwarding counterexample. It does not call
the whole Rails surface supported. It executes unchanged compiler output in
fresh processes, without adding the original generator or Rails to that output.

## Manifest contract

A manifest is executable Ruby. Load dependencies before the bounded capture
phase; enable capture **before the first relevant generation**, not after a gem
has warmed its method cache. Declare public instance-method roots and existing
boundary signatures. Relative signature paths are resolved against the manifest.

```ruby
require_relative "my_generator"
BootToCore.capture { MyGenerator.install(MyClass) }
BootToCore.input(
  roots: { MyClass => [:entrypoint] },
  signatures: ["domain.rbs"]
)
```

The mechanism captures string `class_eval`/`module_eval`, source-backed
`define_method` Procs, aliases, and `Method`/`UnboundMethod` copies. The native
observer preserves MRI's caller visibility and explicit-callable precedence;
the earlier Ruby wrapper did not. Installed-definition snapshots and instruction
sequence checks plus independently recorded replacement identities preserve
known final definitions; ambiguous associations stay opaque. ISeq equality is
not closure-environment identity. Alias snapshots must equal the saved source
definition, or preserve an independently captured replacement. Copies retain original lexical
provenance, not just their changed installation owner.
Ordinary source definitions and literal accessors are recovered by source site.

Capture limitations become **opaque records**, not exceptions in the original
boot. A syntactically reachable opaque definition still refuses the entire
export before output creation, including calls on an unexecuted branch.
The materialization report distinguishes opaque definitions, observer failures,
admitted methods and scalar constant reads; these are not support counts.

Selection starts with effective definitions and follows every direct self-call
in every syntactic branch, including private helpers. Own/included initializers
are retained. Stateful cuts additionally follow the actual inherited Ruby
initializer and its reachable helpers; native or opaque dependencies still refuse
the entire export. Reachable `super` definitions retain separate module owners and
lookup order; shadowed bodies without a reachable `super` are not imported.
This is a syntactic bounded closure, **not** dead-code pruning from observed
test traces. Receiver-dependent calls on other objects are not followed by it.

Closure locals become explicit shared instance cells. Shared lexical slots stay
shared across methods and instances; different factories stay separate; later
rebinding is preserved. Boot values are not substituted into every body.
Symbol cells and deeply frozen Array/Hash graphs are admitted, preserving shared
collection nodes and freezing. Mutable descendants, cycles and special Hash
default/identity semantics refuse export. Namespace shells preserve class/module
kind; named module roots are invoked with an extended object. Parameter syntax
and defaults remain source-backed, including nested block-local scope depth.

Root cuts intentionally omit class-side generator registries and unrelated
framework methods. They do **not** preserve the original class's complete ABI,
inheritance, object identity, constructor protocol or reflection surface.
An inherited initializer is followed when a reachable cut reads/writes instance
variables; it is never replaced with hand-written receiver storage. Stateless
leaves remain admissible, with the omitted initializer in provenance; this is
not preservation of its side effects. Default Object-based explicit setup such as
`prime` in the fixture remains available. State accessed through other objects
or class registries is not proven complete by this syntactic guard.

## Executed cases

Verified on canonical main fetched for this integration, with rebuilt binaries:
Ruby 3.4.8, Prism 1.9.0, ActiveModel/ActiveSupport 8.1.4.

| Input | Original / Core / actual emitted Ruby |
|---|---|
| Unknown eval + closure generator | 22 assertions per fresh process; 14 methods, 6 cells |
| Real ActiveModel attribute generator + callback-body cut | 34 assertions per fresh process; 12 methods |

The first contract covers asymmetric arithmetic, overrides, shared and isolated
closures, post-boot state, later rebinding, Unicode, false/nil and truthiness
including `true`. Its RBS return domain includes `true`, unlike the original
prototype's incomplete fallback declaration.

The real generator uses its actual fixed-arity `parameters: ""` option, passes
method bodies through Rails' code cache/UnboundMethod installation, and creates
an attribute alias. The exporter has no rule for the `_describe` DSL name.
Its private `attribute_describe` helper is discovered, not manually rooted.

The callback root is obtained from the actual ActiveModel registry. Its body and
the private worker reached by `review` are exported, with explicit scalar state.
The contract covers both predicate branches, negative inputs and repeated
mutations. The original's real `valid?` scheduler is also executed on valid and
invalid inputs as a reference, but **is not exported or replaced**. Core invokes
the extracted bodies explicitly; this is not a `valid?`/HTTP/database contract.

Both strict checks report zero errors/warnings. Direct lowered-IR inspection
finds 88/88 concrete expression types for the closure case and 46/46 for the
Rails case **with explicit input RBS**. Types are not inferred from sampled
runtime values. Ten parameterized method signatures in the first case and two
in the second still contain untyped parameters, despite concrete expression
types. This is not a fully typed cross-target ABI claim.

## Boundaries and safety

Only trusted, controlled, **single-threaded** fixtures may use this prototype.
`--trust-boot` is consent to execute Ruby, not sandbox isolation. Boot may perform
arbitrary external effects; use disposable configuration with no production
credentials, data or secret-bearing state. Pre-boot CLI validation prevents
accidental overwrite (including dangling symlinks); it cannot undo boot effects.

Binding slot aliases are identified by a temporary sentinel write followed by
restoration in `ensure`. This is not safe concurrent application instrumentation.
Only nil/booleans/integers/Symbols/frozen strings and admitted deeply frozen
Array/Hash graphs are snapshotted, not arbitrary mutable heaps.
Source/eval lexical scope, mutable-string and identity semantics are not
generally solved; **this is not a sound admission checker for arbitrary Ruby**.
It is not deterministic/reproducible full Rails boot or a Bundler dependency
closure exporter.

Ordinary source methods may snapshot unqualified scalar constants from proven
direct lexical scopes. Copies use the original scope, not the destination
class's same-named constant. The lexical scope is reconstructed from names,
not captured as MRI CREF: this lane requires **no lexical namespace path
rebinding from definition creation through export**, **sealed scalar bindings
after boot**, and unchanged source files throughout observation/export (parsed
trees are cached). Sealing a replacement namespace only after boot is insufficient:
an old method can still retain its original outer module. A frozen value does
not prove a sealed binding. Obvious owner/path mismatches reject scalar reads;
the no-rebinding precondition is not generally verified. Unloaded autoload, mutable values, ancestor/top-level fallback,
uncertain eval/Proc scopes and qualified/dynamic paths stay unsupported; relevant
reflective mutations and constant writes are refused. Later mutation outside
the declared cut is outside its contract, not magically preserved. `defined?`
is refused before value substitution can change its operand classification.

Public CLI tests refuse mutable captures, native bodies, uncaptured generated
definitions, uncaptured overrides, collector failures, missing inherited
receiver construction and aliased `super`. Reentrant aliases and same-site
different-capture Procs are exercised through exported execution. Aliased super is an existing
Ruby-emitter boundary: cloning the alias changes nameless-super lookup, so the
new rooted lane refuses it rather than silently claiming support. Prepend,
implicit/block-local parameter declarations, general constant/global/class-variable scopes, late dynamic
generation and general reflected dispatch are outside the bounded cut.

The same real Rails generator with **default `...` forwarding** is separately
captured and passed to strict Roundhouse checking. It retains the real error:
`forwarding destination's declaration cannot be verified`. The fixed-arity
positive case does not rewrite this negative input or suppress its diagnostic.

No full callback scheduler, error collection, ActiveRecord AttributeSet,
association/DB state, native extension translation, heap snapshot, runtime
method-missing or general forwarding support is claimed. The ordinary keyword
correction preserves Ruby's defaults, false/nil, evaluation order and
unknown-key ArgumentError. A bare positional Hash is not promoted to keywords.
Unverified destinations and non-Ruby native-keyword ABIs remain error boundaries.
This is deliberately stricter than the earlier selector-only keyword-rest
repair: an untyped receiver can no longer borrow another class's keyword ABI
just because their method names match. Its source packet remains intact, but
the cut is not strict-clean until receiver lookup is proved.

## Real-blog: whole-app attempt versus incremental integration

```sh
# Requires the generated fixture and its installed bundle.
ruby tools/boot-to-core/try_blog.rb /tmp/core-blog-results
```

The runner boots/tests a disposable copy of the **complete** real-blog fixture,
attempts its real model/controller roots under capture, then separately adds
the unknown generator's materialized Core to an unchanged copy of that app.
The latter exercises ordinary `lib/` ingestion without changing existing Rails
semantics: all four emitted model/controller suites run (21 tests), followed by
the generator's 22 assertions against the emitted application's real `main.rb`.
The original Rails reference runs 21 tests / 54 assertions without skips.

The initial observer aborted **during Rails initialization**, while
ActiveSupport aliased native `Time#to_time`. Round two now lets that boot and
eager load finish; root export then refuses `Article#valid?` as inherited runtime
outside the declared cut. This is genuine observer progress, not a callback/DB
implementation. No output project is created for the failed whole-app lane.
Exit zero from `try_blog.rb` means the incremental contract passed and the
separate whole-app limitation was recorded, **not** full app support.

CI's `unit` shard 0 installs the pinned Prism/ActiveModel controls, builds the
native observer and installs the fixture's own locked bundle, executes the
observer/CLI tests and both comparison runners,
and uploads command logs/results as `core-ruby-input-evidence`. Other shards
and existing gates are unchanged. A local legacy CI adapter that reports
workflow drift is not a replacement for that hosted execution.

## Earlier experiment

`run.rb` and `rails_probe.rb` retain the earlier micro/control workflow. The
original exporter stopped at UnboundMethod; the reused generic exporter now
handles captures/copies but that unsliced legacy Rails probe still encounters
unsupported class/runtime state. Use `try_input.rb` for the new declared-root
integration and its explicit forwarding boundary, not the old runner as a
whole-Rails success gate.

## Recommended next cut

Keep this an optional source materializer. The next experiment should import
the real class-attribute/default machinery reached by Writebook Current's
inherited constructor, preserving separate instances, defaults and nil reset.
That constructor is now followed, but its opaque defaults generator still
refuses. Do not inject a hand-written `@attributes = {}` to get its contract green.
Then try ordinary Ruby specialization of statically proven dispatch/class
bindings, with overrides, visibility and once-only evaluation preserved, before
attempting the real AR attribute representation. Those are shared Ruby/runtime
problems, not invitations to add one more rule for each Rails DSL.

Before other target languages, address forwarding-declaration and method-
signature stamping gaps. After one actual generator family passes its original/
Core/emitted contract, compare against the existing synthesizer and remove only
the responsibility proved redundant. A provenance-checked Core cache for
non-booting editor/check consumers is a separate prerequisite for deletion.

## Earlier three-app rounds (one to three), not full-app coverage

Three independently owned orbs used the same compiler control (canonical
83b6b145), three exact materializer snapshots and unchanged application sources/
lockfiles. Campfire used the pre-Ruby-4 revision, Writebook its installed 3.4.7
runtime, and Mastodon the actual CI app revision under supported MRI 3.4.8 rather
than its 4.0.5 lock metadata. No application source or dependency was rewritten.
The reusable manifests/contracts/runners are in `campfire/`, `writebook/` and
`mastodon/`; environment/version/audit logs remain private experiment evidence.

| Actual final cut | Original / Core / emitted Ruby | Important exclusion or refusal |
|---|---|---|
| Campfire's three notification/mention-selector leaves | 3 assertions per fresh process, strict-clean | AR constructors/state omitted; actual mention content-type constant is an unfrozen String and refuses export. |
| Writebook `QrCodeLink#url` + own initializer | 4 per process, strict-clean | No signing, secrets, QR rendering, HTTP or DB. |
| Writebook generated `Current#user/user=` | 5 original assertions; round two Core/emitted fail | Now refuses omitted inherited initializer and `@attributes` before output. A prior clean check was not support. |
| Mastodon HTTP request-target and response/limit cuts | 5 per cut/process, strict-clean | Addressable URI is an explicit external gem input, not translated or stubbed. |
| Mastodon real `ASCIIFolding#fold` | 10 per process, strict-clean | Two original frozen scalar string bindings; Unicode/non-mutation boundaries exercised. |
| Mastodon full eager capture → same ordinary request-target cut | 5 per process, strict-clean | Final capture completes in 24.265s; 10,940 opaque definitions and 2,234 opaque eval events stay unadmitted. |

Round one failed early in framework initialization/generation. Round two
records unavailable definitions as opaque and uses the native observer, letting
boot/generation reach requested roots. Round three fixes reentrant alias/body
association, scalar `defined?`, missing inherited receiver construction and a
Mastodon-discovered swallowed TERM. Definition/installation indexes and parsed
source caching remove repeated work under the unchanged-source precondition;
the single 24.265s result is not a comparative performance benchmark.

Generated enum/attribute/delegate roots still refuse real `public_send`,
`_read_attribute` or association/runtime dependencies. Zero Rails DSL families
or production compiler/runtime lines are proved replaceable by these app cuts.
Writebook's original whole-app strict ledger remains 29 errors/258 warnings with
identical diagnostic identities/multiplicities; no baseline was updated. Other
apps' first exporter refusals are not compiler diagnostics: there is no Core
project to check when export fails.

The initial Writebook references exercised 44 assertions, the final Campfire
reference 16 including Rack `/up=200`, and Mastodon's generated-reference lane
15. These are distinct bounded original-app contracts, not complete application
test suites or emitted HTTP equivalence. Final native interpreter/YJIT controls
pass on each app's Ruby, including cancellation/unwind controls. The latest CLI
is 9 tests/99 assertions locally, in Campfire and Mastodon; Writebook runs
9/93 with one explicit missing ActiveModel 8.1.4 fixture skip under its unchanged
8.2 Git bundle. No full Rails/gem, native-target or cross-target claim follows.

## Round four: four real apps, original/Core/actual emitted execution

Four independently owned orbs tested the exact same 22-file materializer snapshot
(`cdf501542d7b6a386cabedc059975d672187bd4b2bca234c9f0b5b4e1ec35f28`).
Application sources, lockfiles and original dependencies stayed unchanged.
The initial matrix retained each orb's unchanged compiler binary; the Mastodon
keyword failure additionally drove a separately built shared-compiler comparison.
The reusable scripts and bounded exclusions are recorded in each app's reports.
These are selected method contracts, not full application or HTTP conformance.

| New passing cut | Assertions in each original / Core / emitted process | What became executable |
|---|---:|---|
| [Campfire](campfire/ROUND4.md) `Opengraph::Location` | 16 / 16 / 16 | Namespaced literal accessors with its original initializer; setter identity and nil/non-nil inputs. |
| Campfire `ApplicationPlatform` | 25 / 25 / 25 | Original inherited constructor, private accessor and `match?`; asymmetric/mixed user agents. |
| [Writebook](writebook/ROUND4.md) `ArrangementHelper#arrangement_actions` | 2 / 2 / 2, both capture modes | Real module receiver, full 16-event mapping and mutable returned-String isolation. An ordinary method, not a generated family. |
| [Lobsters](lobsters/RESULTS.md) `ApplicationHelper` pagination | 7 / 7 / 7 | Public module method with boundary/asymmetric pagination inputs. |
| Lobsters `StoriesPaginator` | 5 / 5 / 5 | Original optional-positional constructor and literal accessors. |
| [Mastodon](mastodon/RESULTS.md) `InteractionPolicy::SubPolicy` predicates | 9 / 9 / 9 | Four actual generated methods, four Symbol closure cells and the original frozen `POLICY_FLAGS` Hash. |
| Mastodon plain namespaced `missing?` | 3 / 3 / 3 | Ordinary namespaced method, not another generated family. |
| Mastodon `Admin::SystemCheck::Message` | 12 / 12 / 12 with keyword candidate | Actual optional-keyword constructor; unchanged compiler instead failed `expected true, got {critical: true}` after a clean strict check. |

Every passing cut is strict-clean (zero errors/warnings) and runs unchanged
emitted Ruby in a fresh process. Prior leaf, QR, Search, scalar/accessor and
full-captured Search controls remain. Location and SubPolicy are two app-level
generated-family demonstrations, **not two new DSL-specific recognizers**.
No production generator/runtime LOC is proved removable; net deleted LOC is zero.

The negative ledger remains executable evidence: Current reaches the real
inherited initializer but refuses opaque class-attribute defaults/callbacks;
AR attributes/associations still require their actual stores; enum dispatch
still requires `public_send`; generated delegates retain their qualified rescue
constant/runtime refusal. Lobsters CandidateId retains the Utils/RNG/class-side
boundary and Graph reaches `respond_to?`, rather than receiving invented state.
Refused cuts produce no Core project. Empty-controller emitted bootstrap failure
is separately retained, not reclassified as whole-app support.

Native controls must run the **child** VM under YJIT: use
`RUBYOPT=--yjit ruby tools/native-observer/contract.rb` and inspect `yjit: true`
in its child records; outer `ruby --yjit` alone does not propagate through
`RbConfig.ruby`. Interpreter/YJIT, privacy and unwind controls remain distinct
from the three-way app contracts. CLI controls are now 11 tests/119 assertions;
Writebook retains its explicit missing-ActiveModel-fixture skip (11/113).
No full Rails, cross-target, performance or framework-replacement claim follows.

### Round-four shared-compiler verification before upstream integration

The final ten-file production diff has SHA256
`85e2725bc2695d21141d263c7f8d12c0d0dc8db36179ed752681b6b2161abc93`.
The independent Mastodon comparison applies that exact diff to a separate
canonical checkout, preserving the original failing binary and all earlier
candidates. Its six unchanged contracts pass 12/5/5/10/9/3 assertions in each
original/Core/emitted process, with all seven strict gates clean. All 29 actual
Ruby child records report YJIT enabled. Original app/dependencies, contracts and
all six Core/model outputs remain unchanged across the final candidate controls.

Native source keywords retain their declared ABI: `critical: false` no longer
becomes `critical = false`. The discriminating caller uses a **local positional
Hash**, not just an inline literal. It retains Ruby's ArgumentError; its native
keyword sibling still returns true. The first four-file candidate failed that
control, and its failure remains preserved separately.

Broad verification additionally reproduced and fixed two ownership regressions:
non-singleton Concern templates consumed by class-body macro expansion are not
callable module singletons, and an untyped nested test-helper constructor needs
the existing lexical namespace resolver, not a constant-assignment lookup.
Actual singleton bodies and every surviving includer copy remain checked;
qualified calls to an invented template singleton still refuse. The existing
Authentication macro and carried-helper emitted tests pass without relaxed
expectations.

Local checks on Rust 1.98.1 before merging canonical main:

- `cargo test --locked --jobs 4 --no-fail-fast -- --test-threads=1 --skip test_backtraces_retain_library_and_integration_source_locations`:
  **5,046 passed, zero failed, 245 ignored**, one separately checked backtrace gate.
- `CARGO_PROFILE_TEST_DEBUG=line-tables-only CARGO_PROFILE_TEST_STRIP=none cargo test --locked --jobs 4 --test ci_policy_workflow test_backtraces_retain_library_and_integration_source_locations -- --exact --test-threads=1`:
  **one passed** with library/integration source locations retained.
- Rebuilt `roundhouse`/`dump_ir`, then reran `try_input.rb`: micro **22** and Rails
  **34** assertions per original/Core/emitted process, strict-clean; the
  unsupported forwarding control still refuses.
- Reran `try_blog.rb`: original **21** Rails tests and emitted incremental
  **21** blog tests plus **22** generator assertions pass. Whole-app export
  refuses **Article#caller**, creates no Core, and is not counted as support.
- CLI **11/119**, verifier **18/284**, and interpreter/actual-child-YJIT native
  controls (**56** assertions per baseline/inactive/active mode) pass.

The ignored SDK/external-corpus gates and hosted CI have not been promoted to
passed checks. The existing jbuilder unused-assignment warning remains. These
results do not authorize publication, merging, or deleting production lowering.

### Verification after canonical upstream integration

Canonical main at [636feb11](https://github.com/rubys/roundhouse/commit/636feb116c3dfb24e9821b8729f54a52c1b77a05)
was merged without rebasing contributor commits in
[8ee72483](https://github.com/thomasklemm/roundhouse/commit/8ee724836cd3a4512c719e2a19c855b7b0065f25).
Fresh checks on that combined compiler tree, using the same commands above:

- Default Cargo suite: **5,067 passed, zero failed, 246 ignored**, with the
  backtrace gate filtered and separately **passed** under debug symbols.
- Rebuilt both binaries and reran the micro **22** and Rails **34** contracts
  in original/Core/emitted processes; strict checks remain clean. Expression
  types have zero missing/unresolved entries; the reported parameter-signature
  gaps remain **10/2**, identical to the pre-integration receipts, not zero.
- Incremental real-blog again passes **21** original Rails tests, **21** emitted
  blog tests and **22** generator assertions. Whole-app export still refuses
  **Article#caller** and produces no Core project; forwarding still refuses.

The existing jbuilder warning and upstream Futamura unused-mut warning remain.
The independently pinned four-app results above were not rerun on this merged
compiler and retain their exact earlier snapshots. Hosted CI and ignored SDK
coverage remain separate from these local checks.
