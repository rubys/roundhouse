# Experimental Core Ruby input

`bin/rh materialize` is a **host-only, opt-in research input**. Execute a trusted
Ruby manifest, materialize a declared instance-method cut into ordinary Ruby,
then use Roundhouse's existing ingest, analysis, lowering and emission unchanged.
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
the emit-side analysis/lowering sequence. This experiment changes neither.
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
are retained. Reachable `super` definitions retain separate module owners and
lookup order; shadowed bodies without a reachable `super` are not imported.
This is a syntactic bounded closure, **not** dead-code pruning from observed
test traces. Receiver-dependent calls on other objects are not followed by it.

Closure locals become explicit shared instance cells. Shared lexical slots stay
shared across methods and instances; different factories stay separate; later
rebinding is preserved. Boot values are not substituted into every body.

Root cuts intentionally omit class-side generator registries and unrelated
framework methods. They do **not** preserve the original class's complete ABI,
inheritance, object identity, constructor protocol or reflection surface.
Inherited superclass initializers/runtime methods are not imported. A non-default
omitted initializer plus any instance-variable access in the reachable cut now
refuses export rather than producing uninitialized receiver state. Stateless
leaves remain admissible, with the omitted initializer in provenance; this is
not preservation of its side effects. The conservative guard may reject an API
that could establish its own state. Default Object-based explicit setup such as
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
Only nil/booleans/integers/frozen strings are snapshotted, not arbitrary mutable
heaps. Source/eval lexical scope, mutable-string and identity semantics are not
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
complex parameters, general constant/global/class-variable scopes, late dynamic
generation and general reflected dispatch are outside the bounded cut.

The same real Rails generator with **default `...` forwarding** is separately
captured and passed to strict Roundhouse checking. It retains the real error:
`forwarding destination's declaration cannot be verified`. The fixed-arity
positive case does not rewrite this negative input or suppress its diagnostic.

No full callback scheduler, error collection, ActiveRecord AttributeSet,
association/DB state, native extension translation, heap snapshot, runtime
method-missing, forwarding fix or parameter-signature fix is claimed.

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
the real constructor and its reachable state-producing helpers for the failed
Writebook Current cut, preserving separate instances, defaults and nil reset.
Do not inject a hand-written `@attributes = {}` to get its contract green.
Then try ordinary Ruby specialization of statically proven dispatch/class
bindings, with overrides, visibility and once-only evaluation preserved, before
attempting the real AR attribute representation. Those are shared Ruby/runtime
problems, not invitations to add one more rule for each Rails DSL.

Before other target languages, address forwarding-declaration and method-
signature stamping gaps. After one actual generator family passes its original/
Core/emitted contract, compare against the existing synthesizer and remove only
the responsibility proved redundant. A provenance-checked Core cache for
non-booting editor/check consumers is a separate prerequisite for deletion.

## Three real applications: controlled rounds, not full-app coverage

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
