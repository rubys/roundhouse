# Mastodon boot → bounded Core Ruby: controlled rounds and keyword comparison

**Round-four result:** one newly executable generated family — four actual
`InteractionPolicy::SubPolicy` predicates — passes original/Core/actual emitted
Ruby. Prior 5/5/10-assertion controls remain passing; namespace-only `missing?`
passes 3 assertions but is not a generated family. The real optional-keyword
`Admin::SystemCheck::Message` cut passes strict and original/Core (12 each),
but the unchanged compiler's emitted Ruby fails: expected `true`, got
`{critical: true}`. **The separately supplied keyword compiler candidate fixes
that exact regression: 12/12/12, with all five previous lanes still passing.**
The final five-file candidate also preserves ArgumentError for a local positional
Hash where the earlier four-file candidate incorrectly promoted it to keywords;
the five-file section records this differential and actual child YJIT verification.
**Latest ten-file ownership candidate (85e272):** the same six lanes and local-Hash control
still pass, with 29/29 actual child YJIT true and no differing observations or
emitted model bytes. Its separate exact-source receipt is in the last section.
Earlier evidence and source/bundle/compiler controls remain separately retained.
**The preceding c6be6 control was not the final production snapshot.** The parent full
Rust suite subsequently found a nested Authentication compile-time filter-macro
ownership regression and supplied this ownership correction. Broader verification
remains parent-owned; neither Mastodon comparison is full-suite approval.

## Final result (round three)

**Real frozen Mastodon boots under observation and exports a declared ordinary
method cut after full eager capture. Three small cuts execute identically in
original/Core/emitted Ruby. Generated Rails cuts still refuse inherited runtime.**
This is not whole-app materialization, a complete ABI, or Rails/gem coverage.
All rounds use compiler `83b6b1458adde2b2db728507fcabc0c2c0557612`, the unchanged
binary, app pin, locked bundle and disposable Unix-socket services below.

| Final lane | Original/Core/emitted, each fresh | Strict result |
|---|---|---|
| Ordinary HTTP request target | 5 assertions each | 0 errors/warnings/gaps |
| Literal response/limit accessors | 5 assertions each | 0 errors/warnings/gaps |
| **Full eager capture → same HTTP request-target cut** | 5 assertions each | 0 errors/warnings/gaps; original no-RBS manifest retained |
| **Actual `ASCIIFolding#fold`** | 10 assertions each | 0 errors/warnings/gaps |
| Real enum/attributes/delegate/settings reference | 15 original assertions | No exported generated Rails cut; explicit root refusals |

The single final full-capture attempt completed in **24.265 seconds, exit 0**,
with a 120-second TERM limit and 10-second hard-kill fallback. It observed
10,940 opaque definition records and 2,234 opaque eval events, with **zero
deferred native collector failures**. Only `HttpSignatureDraft#request_target`
and its original initializer are selected; opaque definitions outside this cut
are neither emitted nor declared supported. Unrelated boot refusal has genuinely
become bounded cut execution, not whole-app execution.

Native contracts pass in interpreter and YJIT modes: 56 assertions in each of
baseline/inactive/active, original observations equal, plus TERM/Interrupt/
SystemExit/throw propagation controls. The CLI contract passes **9 tests,
99 assertions, 0 failures/errors/skips** with explicit `LANG=C.UTF-8`. Its first
clean-environment run had two Unicode decoding errors under default US-ASCII;
that failure and the diagnosed locale correction are retained, not hidden.
ActiveModel/ActiveSupport 8.1.4 were actually available outside the app bundle,
so the CLI Rails fixture was executed, not skipped. Mastodon's frozen bundle
still uses 8.1.3; all 256 loaded-gem fingerprints match the initial run.

Final reusable files: `run.rb`, `input.rb`, `contract.rb`, three RBS files,
`round3.json`, and this report. `round3-opaque.json.gz` retains observable
definition identity tuples and multiplicities (2,982 distinct tuples), not
generated source. Anonymous owners remain `null`, so these labels do not claim
unique structural identity; eval event identities omit process-local object IDs.
Raw 5.5 MB evidence and Core/emitted files remain at
`/tmp/mastodon-round3-final-evidence/`, outside the small parent delivery bundle.
Earlier `round1.json`/`round2.json` and exact probe archives are separate.

## Initial result (round one; preserved evidence)

**The real pinned application boots. Two small app cuts preserve behavior through
Core Ruby and actual Roundhouse-emitted Ruby. The Rails/generated-method cuts
do not materialize with the initial exporter.** This is not a whole-app snapshot,
Rails coverage result, HTTP test, or cross-target support claim.

| Experiment | Original | Core | Emitted Ruby | Strict diagnostics |
|---|---|---|---|---|
| `HttpSignatureDraft#request_target` (+ original initializer) | 5 assertions | 5 identical assertions | 5 identical assertions | 0 parse errors, 0 errors, 0 warnings, 0 survey gaps |
| `ResponseWithLimit#response/#limit` (+ original initializer) | 5 assertions | 5 identical assertions | 5 identical assertions | 0 parse errors, 0 errors, 0 warnings, 0 survey gaps |
| Real enum, attribute casts, setting accessors, delegate | 15 assertions | Not exported | Not emitted | Exporter boundaries below, **not** compiler acceptance |

The ordinary method builds the HTTP-signature request-target string. Inputs
distinguish absent query from empty query, preserve escaping and verb case, and
exclude fragments. It does **not** sign anything or make a request. All three
lanes use real `Addressable::URI` objects: the URI implementation is an explicit
external gem boundary, described in `ordinary.rbs`, not translated or replaced.
The accessor cut distinguishes two objects, positive/negative limits and Unicode
strings. Its methods are recovered from the app's literal `attr_reader` source;
this does not establish support for a previously unknown Rails generator.

Each comparison uses fresh processes and literal expectations independently
written in `contract.rb`. Emitted execution loads the unchanged emitted
`app/models/*.rb` files, not the app source or Rails generators. This exercises
the emitted classes, **not** the emitted project's server/runtime scaffold.
Explicit input RBS is supplied; strict-clean is not a claim that every inferred
expression or parameter signature is concrete.

Reusable files: `input.rb` is the manifest and failure observer, `contract.rb`
contains independent assertions, `ordinary.rbs`/`accessor.rbs` declare the input
boundaries, and `run.rb` drives isolated fresh processes. `round1.json` is the
compact sanitized evidence (selected nine gem fingerprints). Full raw output,
all loaded-gem fingerprints, emitted/Core files and command logs are retained
at `/tmp/mastodon-round1-final-evidence/` in this orb.

## Pins and provenance

- Actual CI Mastodon pin: `163f96cee4dea23365bff9b433871e68d20d9ee7`, from
  `.github/workflows/ci.yml:55`. Codeload archive SHA256:
  `e08dad7245a77858da2f4a6cbef15ca9313121cb5cda04345cb93274c8e2a981`.
- Controlled compiler: canonical `rubys/roundhouse:main` commit
  `83b6b1458adde2b2db728507fcabc0c2c0557612`, isolated at
  `/tmp/mastodon-compiler`. Refreshed canonical was already newer
  (`7a2d3de66ffccf5e53ac5d282c42ada3639c1685`); it was **not** substituted.
  The original fork checkout and its local work were preserved.
- Supplied exporter archive SHA256, verified before extraction:
  `0607454626deed7338d3403684ba92c9294429bd3f80437543bb972f3275bc26`.
  `capture.rb`: `92019e78f4645a3d6540f5a7386b69df73d251ad79d09adf9918e71b354b21ba`.
  `materialize.rb`: `e6109775fa113a4b25a4b32b6f704fa17b3d9605b89e505b39e236e1d755d5c2`.
- Built `roundhouse` binary SHA256:
  `d082f35d6fe41752b6044600ce12a483174ca50531caa60dc61980568ae156a1`.
  `cargo build --locked --jobs 4 --bin roundhouse --bin dump_ir` succeeded;
  the pinned compiler emits one pre-existing `unused_assignments` warning in
  `jbuilder_to_library`. No compiler/runtime changes were made.
- Real frozen bundle: Bundler **4.0.13**, Rails/ActiveRecord/ActiveModel/
  ActiveSupport **8.1.3**, Prism **1.9.0**, Addressable **2.9.0**.
  Runtime: MRI **3.4.8**, accepted by the real Gemfile (`>=3.3.0,<4.1.0`).
  The app's `.ruby-version` and lock metadata say **4.0.5**; this experiment is
  **not** a run on that exact MRI version. No lockfile was regenerated.
- Gemfile SHA256:
  `6ebd4f731e32108f6c9120fee828f38aa4d14071e4dd0c2fc3146f074b972a03`.
  Gemfile.lock SHA256:
  `9873bbb4bf26682ad1a0773d964ea7a74515d998ef80da3d56eec8fe10fe6aa9`.

The runner records selected app-source and probe hashes, both exporter hashes,
binary hash, every loaded gem version/gemspec hash, and deterministic per-gem
hashes over installed Ruby/native files. Tree hashing uses sorted relative paths,
a NUL separator, each file's SHA256, and newline-joined entries. These are
installed-code fingerprints, not hashes of distribution gem archives or of
native system libraries. A second extraction compared with
`diff -qr --no-dereference --exclude=tmp` found **no source differences**.

## Boot and safety

Frozen installation first failed at `idn-ruby 0.1.5` because `-lidn` was absent.
Installing local `libidn-dev` resolved that exact failure. Bundle completion:
154 Gemfile dependencies, 251 installed gems, excluding development, test,
opentelemetry and optional PAM groups. No Gemfile or app source patches.

The first production boot reported the three required AR encryption variables.
Disposable non-production values resolved that gate; eager loading then failed
while `AccountsIndex` read `Account` schema without a DB. A local PostgreSQL 15
cluster and Redis were practical, so this was **not** treated as a final blocker.
Loading bare `db/schema.rb` first failed at absent `timestamp_id(text)`, then at
Scenic views. Loading the real `Mastodon::Snowflake` implementation and
`Scenic.load` supplied the app's original schema dependencies; the complete
original schema then loaded successfully. No seed data, migrations of shared
data, workers, web server, mail, federation or signing operations were run.

Boot and every child comparison run under `sudo unshare -n -- env -i`, with
explicit disposable configuration only. IP networking is unavailable; the
disposable DB/cache are reachable through local Unix sockets. Production eager
load remains enabled, mail delivery is `test`, ES/Prometheus are disabled, and
no inherited `OTEL_*`, credentials, production env files or secrets are supplied.
`require "./config/environment"` completed with `MASTODON_BOOT_OK`.
The original generated-reference contract mutates unsaved model objects only.
Boot's DB/cache configuration is not an exported heap or a database contract.

## Actual capture/root boundaries

The manifest activates capture via `TracePoint(:class, :end)` at the selected
original app class's first declaration, retaining it through calls into gem
generators. It does not reload, rename, subclass or rewrite the app classes.
Full capture also runs separately. App-specific diagnostic observers only log
and re-raise failures; they never admit, skip or rewrite an exporter input.

| Lane | Actual first failing input |
|---|---|
| Full boot capture | AS `LoggerSilence` `mattr_accessor :silencer`: eval `def self.silencer; @@silencer; end;def silencer; @@silencer; end` at `logger_silence.rb:12`; exporter rejects mixed class/instance definitions. This app stops **before** the parent's Campfire native `Time#to_time` alias. |
| `LoginActivity#otp?`, early capture | Installing `BrowserDetection` callbacks touches inherited `__class_attr___callbacks`, AS `class_attribute.rb:15`, via `silence_redefinition_of_method`; inherited visibility/copy provenance is missing. The run aborts **before the enum declaration**, so this is not a captured-enum result. |
| `User#external/#external=`, early capture | Devise's `class_attribute :devise_modules`, `devise/models/authenticatable.rb:64`, generates `class << self` plus instance copy-on-write readers/predicates; rejected mixed eval. The app attribute declaration is not reached under capture. |
| `StatusEdit#local?`, early capture | Nested `PreservedMediaAttachment < ActiveModelSerializers::Model` triggers inherited `_validators` class-attribute copies; `__class_attr__validators` has no recoverable definition at AS `class_attribute.rb:15`. The delegate declaration is not reached under capture. |
| `UserSettings#always_send_emails` | Its actual `define_method` is captured, but direct-self closure reaches `UserSettings#[]`, whose `self.class` is inherited runtime outside the cut. The class registry stores mutable setting objects/default Procs; fixing `#class` alone would not export that state. |
| `Fasp::Capability#enabled/#enabled=`, early capture | `ActiveModel::Model`/callbacks generate mixed class/instance `__callbacks` eval, AS `callbacks.rb:70`; rejected before attribute extraction. |

Two additional **declaration-only capture controls** use app-specific
`TracePoint(:line)` ranges at the pinned original source (`LoginActivity:21`,
`StatusEdit:41–42`). They capture the actual first DSL execution while excluding
earlier class setup, without replaying a DSL or changing its parameters:

- `enum_generation`: the real `otp?` Proc is **captured**. Root closure then
  rejects `LoginActivity#public_send` as inherited runtime outside the cut.
  The actual body is `public_send(:"#{name}_for_database") == value`; dynamic
  dispatch and AR's database-value attribute representation are real remaining
  semantics, not just missing generated names.
- `delegate_generation`: the real forwarding `StatusEdit#local?` body is
  **captured**. Its direct-self closure reaches the original generated `status`
  reader in `StatusEdit::GeneratedAssociationMethods`, at
  `activerecord/associations/builder/association.rb:104`, whose earlier eval was
  not captured. Association lookup/cache, nil/error behavior and forwarding are
  not made ordinary scalar state by capturing the delegate's name/body.

Separate **root-only negative controls**, after successful original boot and
without capture, establish independent boundaries rather than confusing boot
failure with a missing method:

- `LoginActivity#otp?` exists, arity zero, defined by the real Proc at
  `activerecord/enum.rb:307`; the source extractor cannot recover its uncaptured
  environment. It is not supported merely because Ruby can call it.
- `StatusEdit#local?` exists at `app/models/status_edit.rb:41`, with real
  `*`, `**`, `&` forwarding formals. The uncaptured eval body is not recoverable
  from the app declaration. Capturing it would still require forwarding support
  and the association's runtime/heap, not just a generator-name rule.
- `User` attributes are lazily generated; the control invokes the real
  `User.define_attribute_methods` outside capture before inspecting the root.
  The resulting `User::GeneratedAttributeMethods#external` exists at
  `activemodel/attribute_methods.rb:273`, but its uncaptured generated source
  is not recoverable; the control stops there, not at a fabricated missing root.
- `Fasp::Capability` readers/writers exist, but the exporter independently
  rejects namespaced class owners. No alias changes a Ruby class's real name.

No Core project or strict compiler result exists for rejected lanes. Error text,
failing eval source/hash, source locations and short stacks are retained in the
run's `*_materialize.stderr` and `results.json`. No diagnostic is suppressed.

## DSL inventory and possible simplification

This is a family inventory of actual pinned app code, **not** measured support
coverage or removable LOC. Compiler owner names below refer to the controlled
commit; boundaries were checked against its actual source. The table describes
the initial experiment; the round-four section records its actual execution delta.

| Actual Mastodon family/example | Current Roundhouse owner | What could disappear; what remains/moves |
|---|---|---|
| Literal accessors (`ResponseWithLimit`, many filters) | `src/ingest/library_class.rs::synth_attr_reader/synth_attr_writer` | Can arrive as ordinary `def`; this probe verifies that path, but static accessor expansion is already simple. |
| ActiveModel/AR `attribute` (`Fasp::Capability`, `User`, confirmation forms) | `src/lower/active_model_model.rs`; `model_to_library/markers.rs::attribute_api_decls/push_attribute_api_methods`; `schema.rs::push_schema_methods` | Per-DSL accessor/constructor synthesis might be replaced if generated bodies and state become representable. AR `AttributeSet`, casting objects, defaults, dirty tracking and constructors remain. The pinned virtual-attribute matcher accepts only two args, not arbitrary `default:`. |
| `enum` (LoginActivity string labels; Account/DomainBlock integer labels; prefixed/suffixed policies) | `src/ingest/model.rs::enum_declaration/expand_enum_decl` | Label/predicate/bang-method name synthesis is a candidate only after actual captured execution. Class mapping hashes, native/dynamic dispatch, attribute database values and relation scopes remain. No enum passed this probe. |
| `delegate` (`StatusEdit`, `Mention`, suggestion objects; prefix/allow_nil elsewhere) | `src/ingest/delegate.rs::lower_delegates`; `delegate_declarations.rs` | Generated forwarding source could replace delegate-specific synthesis. Once-only receiver evaluation, nil errors, arbitrary forwarding and association calls move to ordinary Ruby/compiler/runtime responsibilities, not away. |
| `class_attribute` (`Trends::Base`, ActivityPub serializer context maps, inherited Rails metadata) | `src/ingest/class_attribute.rs::expand` | Concern-carrier rewriting may be unnecessary in a sufficiently general lane. Inheritance/copy-on-write, late subclass mutation, mutable registries and singleton lookup still need semantics. Current support is bounded Concern carriers, not full class_attribute. |
| Callbacks, validations, associations/scopes (models/concerns) | `src/ingest/model.rs::parse_callback/parse_validates/parse_association`; `model_to_library/markers.rs::push_callback_methods`, `validations.rs`, `associations.rs`; `runtime/ruby/active_record/base.rb` | Named callback wrappers might materialize, but callback scheduling, conditions, abort behavior, errors, queries, transactions and DB adapters remain. This run does not export them. |
| Custom settings/namespace/inverse aliases (`UserSettings::DSL`, `Setting`, `Namespace`) | Ordinary computed `define_method` is not general execution support; `src/ingest/model_macros.rs` specializes only bounded literal concern macros | Captured generated names could avoid bespoke future recognizers. The class-side mutable definition graph, default lambdas, type casting and post-boot changes still need general Ruby/heap support. |
| Closure-heavy custom macros (`Remotable` attachment downloads; account fields; `InteractionPolicy::SubPolicy` predicates) | `model_macros.rs` leaves computed/nested/side-effectful cases unexpanded | Definition templates might be obtained at boot; network delivery, native parsing/crypto/image processing, constant Hashes, optional formals and dynamic `public_send` do not vanish. Not executed here. |
| `store_accessor` (`Admin::ActionLog#recorded_changes` JSONB) | `src/lower/controller_to_library/params_wrapper.rs` only harvests wrapper names; no general accessor synthesis found | Boot can expose generated names, not automatically export JSON mutation, dirty state or AR serialization. Do not count a nonexistent implementation as removable. |
| Serializer `attributes`/context DSL, Devise, Paperclip, rate limits | Gem boot, app custom modules; adjacent compiler models do not imply whole-gem support | Mutable gem objects and native/service adapters dominate the remaining contract. The installed bundle boot is not evidence of their translation. |

**Conclusion:** materialization can move generated-definition discovery to the
real booted gems and feed the unchanged Core-Ruby compiler. This sample proves
two tiny cuts, not removal of Rails lowering. Many apparent simplifications
instead move work into general Ruby semantics, heap/state capture, forwarding,
reflection or explicit native/service adapters. No LOC-removal estimate is made.

## Reproduce / improvement round

For the initial run, restore the probe files from `round1-probe.tgz` into an
isolated copy of this directory; the final `run.rb` now expects the round-three
native observer. Do not substitute the final scripts into the initial recipe.

Run from the Roundhouse checkout; all `/tmp` paths are disposable orb-local state:

```sh
git fetch --quiet https://github.com/rubys/roundhouse.git main
git worktree add --detach /tmp/mastodon-compiler 83b6b1458adde2b2db728507fcabc0c2c0557612
sha256sum core-ruby-input-source.tgz  # compare exact hash above first
tar -xzf core-ruby-input-source.tgz -C /tmp/mastodon-compiler
curl -fsSL https://codeload.github.com/mastodon/mastodon/tar.gz/163f96cee4dea23365bff9b433871e68d20d9ee7 -o /tmp/mastodon-source.tgz
mkdir /tmp/mastodon-source
tar -xzf /tmp/mastodon-source.tgz -C /tmp/mastodon-source --strip-components=1
gem install bundler -v 4.0.13 --no-document
sudo apt-get install -y libidn-dev postgresql-15 postgresql-contrib
```

With working directory `/tmp/mastodon-source`:

```sh
BUNDLE_FROZEN=true BUNDLE_WITHOUT=development:test:opentelemetry:pam_authentication bundle _4.0.13_ install --jobs 4
mkdir -p /tmp/mastodon-pg /tmp/mastodon-redis
/usr/lib/postgresql/15/bin/initdb -D /tmp/mastodon-pg/data --auth=trust --no-locale -U mastodon_probe
amp orb service start mastodon-pg --command "/usr/lib/postgresql/15/bin/postgres -D /tmp/mastodon-pg/data -k /tmp/mastodon-pg -p 55439 -c listen_addresses=''"
amp orb service start mastodon-redis --command "redis-server --port 0 --unixsocket /tmp/mastodon-redis/redis.sock --unixsocketperm 777 --save '' --appendonly no"
/usr/lib/postgresql/15/bin/createdb -h /tmp/mastodon-pg -p 55439 -U mastodon_probe mastodon_probe
BUNDLE_FROZEN=true BUNDLE_WITHOUT=development:test:opentelemetry:pam_authentication ruby -rbundler/setup -ractive_record -rtsort -rscenic -e 'Scenic.load; module Mastodon; end; ActiveRecord::Base.establish_connection(adapter: "postgresql", host: "/tmp/mastodon-pg", port: 55439, username: "mastodon_probe", database: "mastodon_probe"); load "lib/mastodon/snowflake.rb"; Mastodon::Snowflake.define_timestamp_id; load "db/schema.rb"; Mastodon::Snowflake.ensure_id_sequences_exist'
```

With working directory `/tmp/mastodon-compiler`, build the compiler as above.
With working directory the Roundhouse checkout:

```sh
ruby tools/boot-to-core/mastodon/run.rb /tmp/mastodon-source /tmp/mastodon-compiler /tmp/mastodon-round1-final-evidence
```

Runner output must be new. It asserts the controlled compiler HEAD and unchanged
tracked compiler/runtime sources, boots the original app, records hashes and
exact commands/safe environment, runs the two three-way contracts, and reports
each other lane's materialization/strict outcome separately. It never installs
dependencies or manages services/DBs itself. A zero runner exit means only its
declared bounded positive contracts succeeded; it is **not** green whole-app CI.

Original app, bundle, compiler binary and disposable services remain in this orb
for the next exact exporter snapshot. A second snapshot must be hash-verified;
the unchanged compiler/app/bundle controls must stay pinned. Compare boundary
identities and inputs, not just counts. No commit, push, PR, merge or deployment
was performed; only app-specific probe files were written.

## Round two: opaque capture and scalar constants

Snapshot `core-ruby-input-round2.tgz` was verified before selective extraction:
SHA256 `9f1fcbc80f1ef1076740980b4ca82288f3800e24820f3b68dec6d948140f95c9`.
Capture SHA256 `557a5f56e5788cce1ae96149635c8da1d5a8ef568e8018339b0193f84a66005d`;
materializer `89e2e18aeeacb6c4b129a3b55ac383ae6c2005fbe3191661b7a8800d9b066ec6`.
Native C source `060cc07589fcae4bb0070a268d0c06663aeb99a948705b31d55c6960bf46028a`;
local extension `0cf4d19bf460ce2273a4fd1ec46787a589a4cd39d66082b7429a61ea059262c6`.

Both previous positives remained strict-clean and three-way identical. Early
class capture now completed the actual boot and reached the requested generated
methods, instead of aborting on mixed eval or inherited class_attribute wrappers:

| Same original app lane | Round-one first boundary | Round-two/three reachable root boundary |
|---|---|---|
| `LoginActivity#otp?` | inherited class_attribute copy during callback setup | `LoginActivity#public_send`, inherited runtime |
| `User#external/#external=` | Devise mixed eval before declaration | `User#_read_attribute`, inherited AR runtime |
| `StatusEdit#local?` | nested AMS class_attribute copy | `StatusEdit#association`, inherited AR runtime |
| `Fasp::Capability#enabled/#enabled=` | mixed callbacks eval | namespaced class-owner boundary |
| `UserSettings#always_send_emails` | inherited `#class` | same boundary; mutable settings registry still outside scope |

Declaration-only enum and delegate controls and all four no-capture root controls
retain their earlier refusal identities. These are actual generated bodies,
not replayed or simplified DSLs. No per-DSL admission rules or app patches were
added. Opaque records are not removed errors or coverage successes.

The additional real app method is `ASCIIFolding#fold`, used by HashtagNormalizer.
Its two direct lexical String constants are frozen by the original source.
The independent literal expectations distinguish accented upper/lowercase,
letters deliberately absent from the table, combining accents, non-Latin text,
empty input and nonmutation of the input. The exact same manifest against the
saved initial exporter boots and refuses `body contains Prism::ConstantReadNode`;
rounds two and three export it and pass 10 assertions per original/Core/emitted
process. Provenance records name `ASCIIFolding::NON_ASCII_CHARS` and
`ASCIIFolding::EQUIVALENT_ASCII_CHARS`; no mutable values, autoload, namespaced
lookup fallback or late rebinding are admitted.
The final lexical contract assumes no namespace-chain rebinding since definition
creation and sealed scalar bindings after boot; it is a precondition, not a
general mutation detector or namespace/heap snapshot.

Round-two full capture was **incomplete**, not a boot or root success. The first
attempt exceeded its TERM timeout and was stopped after diagnosis; the retained
bounded rerun used a 10-second hard-kill fallback and ended with signal 9 and no
observation summary. The runner returns exit 1 for that incomplete capture,
despite the bounded positives passing. Thirty-second diagnostic samples show
capture walking Proc source and scanning installation arrays around RDF's
stdlib DelegateClass setup; they do not establish a performance percentage or
the dominant cost. More importantly, a direct clean-environment native probe
confirmed swallowed TERM: a collector sending TERM to itself returned with
`NativeDefineMethodObserver.failures == [[..., SignalException]]` instead of
terminating. This was reported to the parent and fixed generically in round three.
Interpreter/YJIT's original 56-assertion observer contract had not tested that
signal invariant, so its earlier pass did not disprove the defect.

`round2.json` retains diagnostic identities, observable multiplicities, exact
commands, source/binary/gem controls, the old scalar refusal and profile samples.
The raw bounded run remains `/tmp/mastodon-round2-final-evidence/`; the earlier
manually stopped run remains `/tmp/mastodon-round2/`. Neither is substituted by
final green evidence.

## Round three: final controls and remaining responsibilities

Snapshot `core-ruby-input-round3.tgz` SHA256:
`f213a414bbef19582710e82f7052d9298de478cb8b9cdbff9b3b1101cf2d3127`.
Only the archive's listed generic files were installed into the isolated compiler
worktree; owned app probes and prior evidence were preserved. No compiler or
runtime implementation file changed. Final identities:

| File | SHA256 |
|---|---|
| `capture.rb` | `bc3db5a5af1c733e0379f7452fab48cf9faa16f262f2b44110c2bdd41248bd36` |
| `materialize.rb` | `e4f5bd01e2f475a6b6f3436d6a97417b3efc35cdbd1b22f6f10cb5d67f21f709` |
| Native `observer.c` | `3c15a5af257b789c9bda219b2b6e4b7cddf29bb31e77a464b207e6f1b798b6c3` |
| Native contract | `b0fcda653521edb0176e95ebe33013a6ef9aee0c6bbfcfeef2c6269a35b800ee` |
| Local extension, actual MRI 3.4.8 | `0ffdeff87c28d6f7c1cf24ed869807d00e81c0dc000f5a8cef5f15878e5642ec` |

The native signal probe is part of the latest contract and now passes. Cached
unchanged Prism source and indexed definition/installation identities allow the
one bounded full capture to finish. This is an observed successful run, not a
controlled speed benchmark attributing a percentage to either change.

Targeted root refusals and their deeper contracts remain: AR attribute storage,
type casts/defaults/dirty state, dynamic enum dispatch/database representation,
association lookup/cache and DB adapters, mutable class registries and inherited
initialization. The narrow declaration-only delegate still cannot recover its
earlier association-reader eval. Namespaced owners and mutable/native state
remain explicit boundaries. Round three also refuses omitted non-default
inherited initialization when the cut touches receiver ivars; this does not
silently turn a Rails model into an Object-initialized scalar object.

The inventory above therefore supports only a **qualitative** simplification:
discover definitions by running real generators, then reuse ordinary Ruby
analysis. It verifies replacing literal accessor discovery and scalar lexical
reads in these cuts, not removing enum/delegate/ActiveRecord lowering today.
The remaining work moves into ordinary Ruby forwarding/lookup/initialization,
closure/heap modeling, mutation and native/service adapters. There is no
removable-LOC estimate, whole-gem coverage claim, or cross-target execution claim.
Campfire and the other app are owned by the parent's independent probes.

## Final rerun commands and delivery

Inspect the round-three archive listing and verify its SHA256 above before
extracting its listed generic members into `/tmp/mastodon-compiler`; do not
extract into the owned Mastodon probe directory. The frozen bundle/services and
compiler setup remain as described in the initial recipe. With working directory
`/tmp/core-observer-round3` (created beforehand):

```sh
env -i HOME="$HOME" PATH="$PATH" ruby /tmp/mastodon-compiler/tools/native-observer/extconf.rb
make
```

With working directory `/tmp/mastodon-compiler`:

```sh
env -i HOME="$HOME" PATH="$PATH" RUBYLIB=/tmp/core-observer-round3 ruby tools/native-observer/contract.rb
env -i HOME="$HOME" PATH="$PATH" RUBYLIB=/tmp/core-observer-round3 RUBYOPT=--yjit ruby tools/native-observer/contract.rb
env -i HOME="$HOME" PATH="$PATH" LANG=C.UTF-8 RUBYLIB=/tmp/core-observer-round3 ruby tests/rh_materialize_test.rb --verbose
```

With working directory the Roundhouse checkout:

```sh
ruby tools/boot-to-core/mastodon/run.rb /tmp/mastodon-source /tmp/mastodon-compiler /tmp/mastodon-round3-final-evidence
```

Use a **new** output path for any authorized later rerun. The final runner always
allowlists only the supplied observer build as `RUBYLIB` in every original and
manifest child, keeps all original cases, records elapsed seconds/exit/signals,
and does not treat a missing observation summary as zero collector failures.
Exact command argv, clean environment, hashes and per-lane outputs are in
`round3.json`; full raw evidence and services stay available in this orb.

The delivery bundle contains only `tools/boot-to-core/mastodon/` probe files,
this inventory/report and compact round evidence (including small compressed
opaque tuples). No app source archive, gems, compiler/native binaries or generated
projects are included. Round-one and round-two exact probe archives are retained
for reproduction, not baseline updates. All work is **local and uncommitted**;
no push, PR, merge, deployment or external-state publication occurred.

## Follow-up iteration: exact bodies and contract readiness

The parent is improving published PR #788 on its task branch; this orb retains
the exact round-three generic source and compiler control as its initial
comparison. No parent compiler/generic change was copied or implemented here.
Mastodon source, lock/dependencies and disposable services remain unchanged;
all 256 loaded-gem fingerprints and the compiler binary match round three.

`inspect.rb` observes one original safe eager boot with round-three capture,
then inspects **45 effective method definitions**. `iteration4-bodies.json`
contains actual source/eval/Proc bodies, owners, formals, original source-file
hashes and a narrow scalar closure-value allowlist. No DSL was replayed; opaque
native accessors remain opaque. The dependency chains are:

- **Fasp namespace:** `enabled(...)` is
  `self.attribute("enabled", ...)`; `enabled=(value)` calls
  `_write_attribute('enabled', value)`. Inherited initialization creates
  `self.class._default_attributes.deep_dup`, then calls `super`. The reads reach
  `@attributes.fetch_value(name) → self[name].value`; writes reach
  `@attributes.write_from_user → with_value_from_user`. Real type objects/defaults,
  the AttributeSet and initialization are necessary. Boolean casts distinguish
  empty String → nil from false tokens/zero → false. Namespace syntax alone is
  insufficient, and no scalar/fake AR store replaces these semantics.
- **Enum:** captured body is
  `{ public_send(:"#{name}_for_database") == value }`, with actual `name` an
  **unfrozen** String `"authentication_method"` and `value` the frozen String
  `"otp"`. The generated database reader calls
  `self.attribute_for_database("authentication_method")`. Neither captured name
  nor registry is frozen/copied to sidestep the current boundary.
- **StatusEdit:** `local?(...)` evaluates `status` once into `_`, forwards to
  `_.local?(...)`, rescues `::NoMethodError`, and raises the actual
  `::ActiveSupport::DelegationError` only for its nil-target case. The `status`
  reader calls `association(:status)`, the deprecation guard with `__method__`,
  then `association.reader`. Inherited association lookup consults the original
  reflection/cache and constructs `reflection.association_class.new(self,
  reflection)`. That is not exported by supplying a replacement reader/store.
- **Safe non-AR delegates:** WebPushRequest has a plain constructor/literal
  receiver accessor and the same generated forwarding/rescue shape. A
  diagnostic-only `super`/rethrow probe proves the round-three rewrite first
  rejects **`WebPushRequest#endpoint → ::Kernel`**, not an AR-store dependency.
  The body also requires `::NoMethodError`, `::ActiveSupport::DelegationError`,
  ordinary exception propagation and `...` forwarding. Error paths are retained.
- **AMS namespaced POROs:** Suggestion's actual prefixed `account_id` forwards
  `account.id`. Translation and PreservedMediaAttachment native accessors come
  from `ActiveModelSerializers::Model.attributes → attr_accessor(*names)` at
  AMS `model.rb:45`, not literal declarations readable by round three. Their
  original inherited initializer constructs attributes/errors and calls super.
- **Custom generated predicates:** InteractionPolicy::SubPolicy captures Symbol
  `key` and executes `@bitmap.anybits?(POLICY_FLAGS[key])` against the original
  frozen Hash. No per-key constant substitution, app edit or state shortcut is
  proposed. `missing?` is a separate ordinary namespace control, not a new
  generated family.

Independently literal original contracts, each in a fresh process:

| Case | Original assertions | Important distinctions |
|---|---:|---|
| `fasp_contract` | 16 | nil/empty/false/zero/one casts, defaults, String IDs, two instances |
| `safe_delegate` | 21 | real unsaved subscriptions, false/nil results, live receiver mutation, independent instances, nil-target vs false-target errors, positional/keyword arity propagation |
| `prefixed_delegate` | 14 | real Account receivers, prefixed `account_id`, nil IDs, nil/false receiver errors, native accessor mutation/isolation |
| `namespace_attributes` | 12 | Translation AMS native accessors, false vs nil, Unicode, independent setters, original nil constructor |
| `media_delegate` | 12 | real MediaAttachment receivers, local/remote, nil IDs, nil/false receiver errors, description isolation |
| `policy_generated` | 9 | seven literal asymmetric flag vectors plus independent instances |
| `policy_plain` | 3 | zero/nonzero instance state; **not counted as generated coverage** |

The existing generated-reference 15 assertions and ordinary/accessor/scalar
5/5/10 assertions still pass unchanged. Native interpreter/YJIT contracts retain
56 assertions per baseline/inactive/active and pass all signal/nonlocal unwind
controls. Ruby syntax and generated RBS parsing also pass.

Core/emitted contracts explicitly retain original Rails **only as the external
receiver/gem environment**. Before loading output they remove the original
exported target class constant, then require each selected generated root's
source location to be output-owned. No `allocate`, ivar seeding, monkey-patched
receivers or original target-method fallback is used. Wrong-output controls
loading the unrelated ordinary Core cut fail with NameError for both safe and
prefixed delegates, rather than passing using original application methods.
Invalid-receiver tests exercise Ruby errors outside the RBS valid receiver ABI.
This protocol cannot be called standalone Rails/AR support.

`iteration4-reference.json` records all original observations and the two
output-isolation negative controls. `iteration4-baseline.json` records strict
three-way passes for only the previous ordinary/accessor/scalar cuts, unchanged
enum/association refusals, and current refusals for all seven new cases. All
targeted boot observation summaries have zero collector failures. The safe
delegate refuses `ConstantPathNode`; the six namespace cases refuse class
ownership before their deeper runtime/closure/accessor constraints. **Newly
executable generated families = 0 on this starting baseline.** A green harness
exit describes the previous positive controls, not these new refused cases.

`run.rb` accepts explicit trailing lane names, so the next exact snapshot can
reuse this identical bounded matrix without another full capture:

```sh
ruby tools/boot-to-core/mastodon/run.rb /tmp/mastodon-source /tmp/mastodon-compiler /tmp/mastodon-iteration4-next ordinary accessor constant enum delegate fasp_contract safe_delegate prefixed_delegate namespace_attributes media_delegate policy_generated policy_plain
```

Native source and generic snapshot updates remain parent-owned and must be
hash-verified before this rerun. The runner's compiler guard remains the exact
old control until a separately identified compiler comparison is requested.
Only successful strict original/Core/**actual emitted** observations can increase
the generated-family ledger; extra captured methods, namespace boot or plain
`missing?` execution cannot. Readiness files are local and uncommitted, with no
push/publication, and the orb remains available for the next snapshot.

## Round four: unchanged-compiler breadth result

The exact 22-member `core-breadth-round4.tgz` was path/hash verified and installed
only into the isolated compiler worktree. Archive SHA256:
`cdf501542d7b6a386cabedc059975d672187bd4b2bca234c9f0b5b4e1ec35f28`.
`round4-snapshot.json` records every installed member's hash. Capture SHA256
`dcbf2c0610c6271692266387b97a5e74e9ea0ce9e69070120d377f13a5790a6b`;
materializer `34f9b72abad9c762113311f02d3cc67af6cd02a611499d2247fa49bfc380a05b`.
Native source/build are unchanged from round three. All 256 loaded-gem
fingerprints match both rounds one and three. Every selected original app file,
including the newly selected Message constructor, matches the pinned CI archive.

| Actual cut | Original / Core / actual emitted assertions | Counted outcome |
|---|---|---|
| HTTP request target | 5 / 5 / 5 | Prior ordinary control retained |
| ResponseWithLimit accessors | 5 / 5 / 5 | Prior literal-accessor control retained |
| ASCIIFolding scalar constants | 10 / 10 / 10 | Prior scalar/native control retained |
| **SubPolicy generated predicates** | **9 / 9 / 9** | **One new generated family** |
| SubPolicy ordinary `missing?` namespace control | 3 / 3 / 3 | New plain cut, not another generated family |
| Admin::SystemCheck::Message readers/constructor | 12 / 12 / **failure** | Not supported on unchanged compiler |

All six materialized cuts are strict-clean: zero parse errors/errors/warnings,
gap-attributed notes or survey gaps. The final matrix still exits **1**, status
`positive_lane_failed`, because the Message **emitted execution** fails. The
first prepared matrix without this extra ABI control exits 0 and remains
separate in `round4-first-run.json`; it must not replace this failure evidence.

The SubPolicy cut executes the real app's original generator body:

```ruby
define_method(:"#{key}?") do
  @bitmap.anybits?(POLICY_FLAGS[key])
end
```

Four immutable Symbol closure cells and the original frozen five-entry
`InteractionPolicy::POLICY_FLAGS` Hash are represented in ordinary Core; no
per-key rewrite, alternate app, receiver seeding or new freeze is introduced.
Its actual initializer remains `@bitmap = bitmap`, with no omitted initializer.
Literal vectors at 0, 2, 4, 8, 16, 6 and 18 distinguish the four bit positions,
combined masks and independent instances. Four methods are **one family**, not
four families; `missing?` is source-written. This remains Ruby-target execution,
not a cross-target native `Integer#anybits?` or full InteractionPolicy claim.
Constant snapshots retain the sealed/unchanged-source namespace preconditions.

### Deeper root refusals, not coverage

Every bounded lane boots under observation with zero deferred collector failures.
Original-only assertions for the five refused PORO/delegate cases were rerun
unchanged (16/21/14/12/12); the existing generated-reference 15 also still passes.

| Lane | Round-three first refusal | Exact round-four boundary |
|---|---|---|
| Fasp::Capability typed attributes | Namespaced class owner | `Fasp::Capability#class`, inherited runtime |
| Suggestion prefixed delegate | Namespaced class owner | Native `#account` accessor, missing source at AMS `model.rb:45` |
| Translation attributes | Namespaced class owner | Native `#text` accessor, same AMS origin |
| PreservedMediaAttachment delegate | Namespaced class owner | Native `#media_attachment` accessor, same AMS origin |
| WebPushRequest safe delegate | `ConstantPathNode` | Same body boundary (`::Kernel`/exception namespace path), no error-path omission |
| LoginActivity enum | `#public_send` | Same inherited dynamic dispatch; original captured unfrozen String remains unfrozen |
| StatusEdit full class capture | `#association` | Reaches real association reader's `#__method__`, still inherited runtime |
| StatusEdit declaration-only delegate | Unobserved association-reader eval | Same GeneratedAssociationMethods `#status` source refusal |
| UserSettings | `#class` | Same inherited runtime; mutable settings registry not exported |

The first matrix supplies full enum/delegate observations; the final matrix
also reruns their declaration-only negative controls and UserSettings. Opaque
identities/multiplicities remain in both JSON files, separate from collector
failures. No additional full capture was attempted in round four; round three's
one bounded full capture remains historical evidence, not a new claim.

### Genuine optional-keyword ABI failure

The real `app/lib/admin/system_check/message.rb` defines four literal readers
and assigns four receiver ivars in:

```ruby
def initialize(key, value = nil, action = nil, critical: false)
```

SoftwareVersionCheck and MediaPrivacyCheck actually call it with `critical: true`.
Only the safe constructor/readers are exercised; no system check is run.
The independent contract tests the default, explicit true/false, positional
values, nil and independent objects. Core preserves `critical: false`; emitted
`app/models/admin/system_check.rb` instead declares `critical = false` and
receives the external keyword Hash as its fourth positional argument. The exact
failure is `expected true, got {critical: true}`. This verifies the parent's ABI
warning against a real consumer, rather than speculating from source alone.

The unchanged compiler/binary is still the exact original control. Strict/Core
acceptance is not sufficient, and this cut is not included in the supported
ledger. `round4.json` retains its source/Core/emitted hashes and failure/exit.

### Responsibility/savings delta

The new success eliminates the need for a **new app-specific SubPolicy macro
recognizer** in this cut: actual `define_method` names, lexical Symbol bindings
and immutable constant lookup become ordinary input. It does not remove a
measured existing lowering implementation or justify a LOC savings number.
AMS accessor observation, namespace exception lookup, forwarding/nil errors,
AR AttributeSet/association runtime, mutable registries and native adapters still
require general semantics. Fasp's namespace improvement exposes initialization
and class state; it does not make the typed attributes executable. Only the
selected Ruby cuts count, with original Rails objects explicitly external where
the contracts declare them. Other apps' inventories remain parent-owned.

Native interpreter/YJIT contracts pass 56 assertions per baseline/inactive/active
and all TERM/Interrupt/SystemExit/throw controls. CLI: **11 tests, 119 assertions,
0 failures/errors/skips**; real external AM/AS 8.1.4 fixture executed, while the
app remains on 8.1.3. The private-callback Ruby-wrapper negative control fails as
intended and is not hidden. Initial wrong-working-directory extraction and the
pre-install CLI log were not used as installed round-four evidence.

Reproduce this exact unchanged comparison from the Roundhouse checkout, using
a new output directory (same existing frozen bundle and Unix-socket services):

```sh
ruby tools/boot-to-core/mastodon/run.rb /tmp/mastodon-source /tmp/mastodon-compiler /tmp/mastodon-round4-final-matrix ordinary accessor constant policy_generated policy_plain keyword_message fasp_contract safe_delegate prefixed_delegate namespace_attributes media_delegate enum_generation delegate_generation settings
```

Expected exit is **1** until the separately authorized compiler candidate is
selected. Raw evidence remains `/tmp/mastodon-round4-final-matrix/` and
`/tmp/mastodon-round4-final-evidence/`; compact identity/commands/assertions and
refusal multisets are in `round4.json` and `round4-first-run.json`. All earlier
JSON/probe archives remain separate and unchanged.

## Separate shared-compiler keyword candidate comparison

This is **not another unchanged-compiler result**. The parent supplied
`core-keyword-round4.patch`, SHA256
`f970c0d2393bccbfe3b8541311eebc15f8b30856974072b585fc628ce515988e`.
It passed `git apply --check` in a new isolated detached worktree based on the
same canonical control. Before application, all four changed Rust files were
verified byte-for-byte identical to the parent's published source at
[`4480841`](https://github.com/thomasklemm/roundhouse/commit/4480841589a8fedca066822f6730bc3959abd316).
The resulting full source diff equals the supplied patch **byte-for-byte**;
no app, gems, contract, RBS, exporter, runtime or lock file was altered.

| Exact changed file | Candidate scope |
|---|---|
| `src/ingest/library_class.rs` | Retain ordinary source keyword declarations |
| `src/analyze/forwarding.rs` | Select native packets only for verified source keyword destinations; refuse unverifiable destinations |
| `src/lower/forwarding.rs` | Carry that refusal policy through lowering |
| `src/project.rs` | Report unverified native keyword parameter ABI for non-Ruby targets |

The decisive declaration change in the supplied patch is:

```{ .diff file="src/ingest/library_class.rs" }
// @annotation The constructor's external Ruby keyword must remain a keyword, rather than accepting its Hash as a positional boolean.
-    ingest_library_method_with_keywords(def, owner, file, false)
+    ingest_library_method_with_keywords(def, owner, file, true)
// @endannotation
```

Built with a separate target directory, not over the original executable:

```sh
env -i HOME="$HOME" PATH="$PATH" CARGO_TARGET_DIR=/tmp/mastodon-keyword-target cargo +1.98.1 build --locked --bin roundhouse --jobs 2
```

Working directory `/tmp/mastodon-keyword-compiler`; build exit 0, 2m17s. One
unused-assignment warning in unchanged `src/lower/jbuilder_to_library/mod.rs`
is retained. The locked toolchain is Rust 1.98.1. Exact identities:

| Executable | SHA256 |
|---|---|
| Original unchanged compiler | `d082f35d6fe41752b6044600ce12a483174ca50531caa60dc61980568ae156a1` |
| Separate keyword candidate | `2a7934849744fc124f52bb931d0f08359f5a3e0c8da5a00e9f620e17c1631e33` |

The **identical 12-assertion Message contract now passes original/Core/actual
emitted**. Its emitted constructor is `critical: false`, not `critical = false`.
All five previously passing lanes also pass unchanged: ordinary 5, accessor 5,
scalar 10, generated policy 9, ordinary policy 3, each per fresh process.
All six cuts have zero strict parse errors/errors/warnings/notes/gaps, zero
collector failures and three-way identical observations. Runner exit **0**:
`bounded_contracts_verified_with_explicit_boundaries`.

All six **Core outputs are byte-identical to the failing baseline's Core
outputs**. Contract/manifest/RBS hashes, original app and lock hashes, all 256
gem fingerprints and native build hashes match. The original compiler worktree
and binary are untouched; the baseline emitted failure remains in `round4.json`.
Only the app-owned runner gained an explicit candidate opt-in with exact-patch
hash/diff guards and a separate executable path. Its default still rejects any
compiler source difference. A candidate-without-opt-in negative control rejects
before output creation, rather than silently treating this as the old compiler.

From the Roundhouse checkout, with a **new** output path:

```sh
MASTODON_PROBE_COMPILER_PATCH=/home/user/workspace/repo/core-keyword-round4.patch MASTODON_PROBE_COMPILER_BINARY=/tmp/mastodon-keyword-target/debug/roundhouse ruby tools/boot-to-core/mastodon/run.rb /tmp/mastodon-source /tmp/mastodon-keyword-compiler /tmp/mastodon-keyword-round4-final-evidence keyword_message ordinary accessor constant policy_generated policy_plain
```

`keyword-round4.json` records the full exact patch, four before/after source
hashes, build command/lock/toolchain/log identity, both binary hashes, commands,
observations and controls. Raw outputs stay in
`/tmp/mastodon-keyword-round4-final-evidence/`. Non-Ruby ABI diagnostics, source
packet/super/forwarding combinations and full hosted/compiler CI are **not**
claimed by this bounded app rerun; broader verification remains parent-owned.

The additional executable Message cut uses the already demonstrated literal
reader family; fixing its constructor ABI does **not** increase the new generated
family count beyond **one**. Enum/delegate/AR mutable state and native accessor
refusals remain the unchanged-compiler breadth ledger above, not new successes.
The final compact delivery includes this report, reusable owned scripts/RBS,
exact body inventory, round-four before/after evidence and all prior compact
round evidence. No application archives, gems, binaries or giant generated
projects are included. Everything remains local/uncommitted/unpublished, and
the orb, original control, candidate build and disposable services remain
available for a later authorized comparison.

## Final five-file production-diff comparison

The supplied `core-keyword-round4-final.patch` SHA256 is
`6bbf7c7ee8c8bf84a20998ff62c01b54cb450cb0e64386274520449761bdb212`.
It passed `git apply --check` in a **third isolated checkout** at canonical
83b6, separate from both earlier compiler controls. All five base files,
including `src/lower/kwsplat.rs`, match the parent's published 4480841 source
byte-for-byte. The exact final source diff hashes to the supplied patch hash.
The earlier four-file receipt, original compiler, four-file binary and previous
delivery bundle are unchanged. Final changed source identities:

| File | Final SHA256 |
|---|---|
| `src/analyze/forwarding.rs` | `dbc22dbec013caa11c6c2b384392ea28fcda08489e5824ea8c1c0f7b3ff05975` |
| `src/ingest/library_class.rs` | `179f22213af75f609c1525ee2fe9737457d6cfd3c04c2874d7238009807d2959` |
| `src/lower/forwarding.rs` | `ea93200319e066c21ec7299923dcb80134734348a06419a6810771bc78ee5d2a` |
| `src/lower/kwsplat.rs` | `e15add2cec4d532aa02844c5b057dde8f1c77cd52a1d07a6986e2db314e4fd5c` |
| `src/project.rs` | `ca33b7e19ff5a9585d9768c49495d2d9be912f3bcb7395063b62b9dcc0c1581a` |

Built in `/tmp/mastodon-keyword-final-compiler` with clean environment and
`CARGO_TARGET_DIR=/tmp/mastodon-keyword-final-target cargo +1.98.1 build --locked
--bin roundhouse --jobs 2`. Build exit 0, 2m09s; the same warning in unchanged
jbuilder source is retained. Final binary SHA256:
`d961dffa3cf6f46eb2e646780f6a96824472f033e71f166e63cbc3f7d1a20cb9`.
The original and four-file hashes remain in the preceding receipt/table.

All **six unchanged** contracts remain strict-clean and three-way identical:
Message 12, ordinary 5, accessors 5, scalar 10, generated policy 9, plain policy 3,
each in fresh original/Core/actual-emitted processes. Runner exit **0**;
44 assertions per execution mode. All six Core source bytes, app/lock/source
pins, original contract/manifest/RBS hashes, all 256 gem fingerprints and native
build match the preceding comparison. No original contract was edited.

### Discriminating bare positional Hash control

An explicit **caller fixture**, not another app/DSL substitute, invokes the
real Message constructor through a local Hash:

```ruby
payload = {critical: true}
Admin::SystemCheck::Message.new(:bare, nil, nil, payload)
```

The positive sibling uses `new(:native, nil, nil, critical: true)`. Original
and Core raise **ArgumentError** for the former and return critical **true** for
the latter. Both compiler candidates are strict-clean, but their actual emitted
caller executions differ:

| Compiler | Bare local Hash | Native keyword call | Control exit |
|---|---|---|---|
| Earlier four-file candidate | Incorrectly returns critical true | true | **1** |
| Final five-file candidate | **ArgumentError** | true | **0** |

The earlier candidate emits `critical: payload.fetch(:critical, false)`; the
final candidate preserves the positional `payload`. An inline Hash literal
passes on **both** candidates, so that first control alone did not demonstrate
the new guard. The reusable owned runner now uses the discriminating local-read
shape. It was rerun through the public runner with `keyword_message` plus the
extra control and exits 0. The caller fixture is not counted as generated app
coverage, nor is it a change to any original app/gem method or receiver store.

### Actual child YJIT and harness evidence

The six-lane run records **29 actual Ruby children**, each reporting
`RubyVM::YJIT.enabled? == true` from inside that process at exit. The final
local-Hash public-runner check records another **9 actual children**, all true;
the original/Core/four-file/five-file differential also records actual child
runtime. The clean environments do not inherit private RUBYOPT or secrets.
`--yjit` is added to each Ruby child explicitly, and the runner refuses a missing
or false child observation when requested. Rust compiler commands are not
mislabelled as Ruby/YJIT workers.

An initial probe incorrectly required global JSON 3.0.2 before Bundler, while the
real lock requires 2.20.0. That harness failure is retained separately; moving
the probe's require **inside at_exit**, after the original bundle activation,
fixed it without changing dependencies or source. The two successful later
runs preserve the full loaded-gem fingerprint. This was not an app boot defect.

With working directory the Roundhouse checkout, using a **new** output directory:

```sh
MASTODON_PROBE_COMPILER_PATCH=/home/user/workspace/repo/core-keyword-round4-final.patch MASTODON_PROBE_COMPILER_BINARY=/tmp/mastodon-keyword-final-target/debug/roundhouse MASTODON_PROBE_YJIT=1 MASTODON_PROBE_BARE_HASH=1 ruby tools/boot-to-core/mastodon/run.rb /tmp/mastodon-source /tmp/mastodon-keyword-final-compiler /tmp/mastodon-keyword-final-reproduce keyword_message ordinary accessor constant policy_generated policy_plain
```

`keyword-final-round4.json` contains the exact five-file diff, before/after
source hashes, all three binary identities, build/lock/log receipt, six-lane and
local-control runner receipts, differential emitted callers/observations, actual
child runtime records and the diagnosed instrumentation failure. Earlier
`keyword-round4.json` and all original failure evidence remain unchanged.
Raw runs stay in `/tmp/mastodon-keyword-final-rerun-safe/`,
`/tmp/mastodon-keyword-final-local-runner/` and `/tmp/mastodon-bare-local-final/`.
Only the owned runner/report and compact evidence changed; nothing was committed
or published. No whole-app/non-Ruby/full-CI claim is made. Parent compiler-suite
and backtrace results are separate from these executed bounded app controls.

## Ten-file verified-breadth compiler comparison

The supplied `core-keyword-round4-verified-breadth.patch` SHA256 is
`8ad268408b2a0bf641e2ec4db0b5589991bf9c0534e4d6dae7c392a2dca2f97a`.
Exactly ten Rust files were inspected; every canonical83b6 base file matches
the parent's published 4480841 source byte-for-byte. `git apply --check` passed
in `/tmp/mastodon-keyword-breadth-compiler`, a separate fourth compiler checkout.
Its complete resulting source diff equals the supplied patch byte-for-byte.
The generic 22-member snapshot, runtime, Cargo lock and original controls are
unchanged. The owned runner only added this exact patch hash to its allowlist;
its whole-diff check and unchanged default guard remain enforced.

Built separately with clean environment and locked Rust 1.98.1:

```sh
CARGO_TARGET_DIR=/tmp/mastodon-keyword-breadth-target cargo +1.98.1 build --locked --bin roundhouse --jobs 2
```

Build exit 0, 2m23s; the same unused-assignment warning in unchanged jbuilder
source remains recorded. New compiler binary SHA256:
`8d692e1d456996c66de813c95f52b96dba48b4dc9bf249a9c66a6f7f93c0c97a`.

| Unchanged app lane | Original/Core/actual emitted, each | Strict |
|---|---|---|
| Message constructor/readers | 12 assertions | zero diagnostics |
| HTTP request target | 5 | zero diagnostics |
| ResponseWithLimit accessors | 5 | zero diagnostics |
| ASCIIFolding scalar/native control | 10 | zero diagnostics |
| SubPolicy generated predicates | 9 | zero diagnostics |
| SubPolicy plain namespace control | 3 | zero diagnostics |

Runner exit **0**, `bounded_contracts_verified_with_explicit_boundaries`;
44 assertions per main execution mode. The unchanged local positional-Hash
caller control also passes original/Core/actual emitted: **ArgumentError** for
the local positional payload, **true** for the valid `critical: true` call.
All seven strict checks have zero parse errors/errors/warnings/notes/gaps.
No observer failures occur. Actual YJIT is **true in all 29 Ruby children**.

**Differing observations against the five-file candidate: none.** All six Core
source hashes, all emitted Ruby model-file hashes and all three-way assertion
records match byte-for-byte/value-for-value. The local-Hash driver, emitted model
files and observable results match the five-file public-runner control too.
Original app/lock/contract/manifest/RBS hashes, all 256 gem fingerprints, native
source/build and every generic snapshot member match. The previous original,
four-file and five-file binaries, receipts, failures and bundles remain intact.

The exact ten changed files and their before/after SHA256 values are recorded in
`keyword-breadth-round4.json`, together with the full patch, all four binary
identities, locked build receipt, exact clean child commands, strict output,
actual child runtime, emitted model hashes and equality controls. Full raw
results remain `/tmp/mastodon-keyword-breadth-evidence/`.

Reproduce from the Roundhouse checkout with a new output directory:

```sh
MASTODON_PROBE_COMPILER_PATCH=/home/user/workspace/repo/core-keyword-round4-verified-breadth.patch MASTODON_PROBE_COMPILER_BINARY=/tmp/mastodon-keyword-breadth-target/debug/roundhouse MASTODON_PROBE_YJIT=1 MASTODON_PROBE_BARE_HASH=1 ruby tools/boot-to-core/mastodon/run.rb /tmp/mastodon-source /tmp/mastodon-keyword-breadth-compiler /tmp/mastodon-keyword-breadth-reproduce keyword_message ordinary accessor constant policy_generated policy_plain
```

This demonstrates that the broader compiler batch preserves these app cuts and
the discriminating positional-Hash contract. It does **not** independently cover
concern carriers, singleton/conflicting-includer negatives, controller/test-helper
constructor scopes or form wrappers; those broader tests remain parent-owned.
No new generated family is counted, no original contracts/app/gems were changed,
and no full-app/non-Ruby/hosted-CI claim is made. Work remains local, uncommitted
and unpublished; all compiler controls, evidence and disposable services remain
available for another explicitly authorized comparison.

## c6be6 ten-file source-module template guard candidate comparison

The exact `core-keyword-round4-final-breadth.patch` SHA256 is
`c6be6d458fe6b36be652007e65e5630fcdbb47767129300c08b5ea020d235c65`.
It passed `git apply --check` in a new isolated canonical83b6 checkout,
`/tmp/mastodon-keyword-final-breadth-compiler`. All ten base files again match
published4480841 byte-for-byte; the resulting full source diff equals the patch
byte-for-byte. Against the preserved ten-file candidate, **only
`src/analyze/forwarding.rs` differs**. Its new SHA256 is
`a7adb0a053d18aa798fc75e74613422350a4df23491b05fe12bca3b347d73358`;
the other nine after-hashes remain identical to the preceding receipt.

For the incremental build, the previous target cache was copied with
`cp -a --reflink=auto` into a **separate** target directory; no prior executable
was rebuilt in place. The clean-environment locked build command was:

```sh
CARGO_TARGET_DIR=/tmp/mastodon-keyword-final-breadth-target cargo +1.98.1 build --locked --bin roundhouse --jobs 2
```

Build exit 0, 1m32s, with the same warning in unchanged jbuilder source. New
binary SHA256:
`e603b007e656f4a9baa539459bc4392447d5851e5593a464e4c415058eb760e3`.
The preserved preceding ten-file binary remains
`8d692e1d456996c66de813c95f52b96dba48b4dc9bf249a9c66a6f7f93c0c97a`.

The **unchanged six lanes** still pass 12/5/5/10/9/3 assertions in each fresh
original/Core/actual-emitted mode; 44 per main execution mode, runner exit **0**.
The same local positional-Hash caller still raises **ArgumentError** in all
three modes, and its native keyword sibling returns **true**. All seven strict
checks have zero parse errors/errors/warnings/notes/gaps; all collector failure
lists are empty. **All 29 actual Ruby children report YJIT true.**

**Differing observations against the preceding ten-file candidate: none.**
All six Core files, every emitted Ruby model file, all assertion records and
the local-Hash driver/model bytes match. App/lock/contract/manifest/RBS, all 256
gem fingerprints, native source/build and all generic22 members match.
The previous ten-file receipt and bundle remain hash-identical and untouched;
all earlier binaries/failures/receipts are retained separately too.

The owned runner only adds the exact new patch opt-in, retaining all existing
hash/diff guards. `keyword-final-breadth-round4.json` records all ten source
before/previous/after hashes, the sole-file delta, exact patch, cache-copy/build
argv, lock/log receipt, both latest binary identities, strict and three-way
observations, emitted model hashes, actual child YJIT and preservation controls.
Raw evidence remains `/tmp/mastodon-keyword-final-breadth-evidence/`.

Reproduce from the Roundhouse checkout, using a new output directory:

```sh
MASTODON_PROBE_COMPILER_PATCH=/home/user/workspace/repo/core-keyword-round4-final-breadth.patch MASTODON_PROBE_COMPILER_BINARY=/tmp/mastodon-keyword-final-breadth-target/debug/roundhouse MASTODON_PROBE_YJIT=1 MASTODON_PROBE_BARE_HASH=1 ruby tools/boot-to-core/mastodon/run.rb /tmp/mastodon-source /tmp/mastodon-keyword-final-breadth-compiler /tmp/mastodon-keyword-final-breadth-reproduce keyword_message ordinary accessor constant policy_generated policy_plain
```

This verifies preservation of the Mastodon cuts, not independent coverage of the
parent's Factory/Product template-admission regression or full default suite.
Those gates remain parent-owned. The parent subsequently reported a nested
Authentication compile-time filter-macro ownership regression in the full Rust
suite; this receipt is a preserved candidate control, not final production
approval. No app, original contract or generic exporter was changed; no new
generated family is counted. Work remains local/uncommitted, with no publication
or new threads, and all controls remain available for the next exact patch.

## Exact template-root ownership candidate comparison (85e272)

The supplied `core-keyword-round4-ownership-final.patch` is exactly 36,299 bytes,
SHA256 `85e2725bc2695d21141d263c7f8d12c0d0dc8db36179ed752681b6b2161abc93`.
It passed `git apply --check` in a new detached canonical83b6 checkout,
`/tmp/mastodon-keyword-ownership-compiler`; the complete resulting source diff
equals the patch byte-for-byte. All ten base files match published4480841.
Against c6be6 only `src/analyze/forwarding.rs` differs, new SHA256
`de2a3cf501d282cbff3fcd9ce75432a188cb2144e7c70f3aba3d94fc7f01173c`.
The supplied change tracks exact non-singleton template expression roots and
uses `ConstResolver.namespace` for untyped source-constant resolution. These are
parent-owned compiler changes, not app-specific rules.

The preceding target cache was copied with `cp -a --reflink=auto` into
`/tmp/mastodon-keyword-ownership-target`. The separate clean-environment build
used `cargo +1.98.1 build --locked --bin roundhouse --jobs 2`, exit **0** in
1m32s, with the same warning in unchanged jbuilder source. Binary SHA256:
`cbfb637d4e6b2f7868ce5a78c32da6c72ffb83b9bd7d7644659b35aaf4613f86`.
The prior c6be6 binary remains
`e603b007e656f4a9baa539459bc4392447d5851e5593a464e4c415058eb760e3`.

The same runner exited **0**, `bounded_contracts_verified_with_explicit_boundaries`:

| Unchanged lane | Original | Core | Actual emitted |
|---|---:|---:|---:|
| Message keyword constructor | 12 | 12 | 12 |
| Ordinary request-target method | 5 | 5 | 5 |
| ResponseWithLimit accessors | 5 | 5 | 5 |
| Scalar constant reader | 10 | 10 | 10 |
| SubPolicy generated predicates | 9 | 9 | 9 |
| SubPolicy plain missing? | 3 | 3 | 3 |

All six lanes are strict-clean; the local-Hash control is the seventh clean
strict gate. All have zero parse errors/errors/warnings/notes/gaps, and all
collector failure lists are empty. In each fresh original/Core/actual-emitted
process, the same bare local positional Hash raises **ArgumentError** and the
native keyword sibling returns **true**. **29/29 actual Ruby children report
YJIT enabled**, including materialization and original generated-reference checks.

**No differing contract observations or Core/emitted model bytes against c6be6.**
The local-Hash driver/helper/model bytes also match. App/lock/manifest/contracts/
RBS, all 256 loaded-gem fingerprints, native source/build and all generic22 files
remain identical. All five previous binaries remain hash-identical. The c6be6
receipt and archive remain unchanged, as do earlier separately retained receipts.
The only runner change is the new exact patch allowlist entry; its full-source
diff guard remains active for this and all previous opt-ins.

`keyword-ownership-round4.json` retains the exact ten-file diff, before/c6be6/after
source hashes, compiler/build/cache/log/lock identities, runtime and gem pins,
three-way assertions, strict output, Core/emitted hashes, all child YJIT records
and preservation controls. Raw output remains
`/tmp/mastodon-keyword-ownership-evidence/`; no generated projects or binaries are
included in the compact owned bundle.

Reproduce from the Roundhouse checkout with a new output directory:

```sh
MASTODON_PROBE_COMPILER_PATCH=/home/user/workspace/repo/core-keyword-round4-ownership-final.patch MASTODON_PROBE_COMPILER_BINARY=/tmp/mastodon-keyword-ownership-target/debug/roundhouse MASTODON_PROBE_YJIT=1 MASTODON_PROBE_BARE_HASH=1 ruby tools/boot-to-core/mastodon/run.rb /tmp/mastodon-source /tmp/mastodon-keyword-ownership-compiler /tmp/mastodon-keyword-ownership-reproduce keyword_message ordinary accessor constant policy_generated policy_plain
```

No additional family is counted. This bounded comparison does not independently
exercise the parent's Authentication macro/Factory ownership regressions or full
Rust suite. Those gates and production acceptance remain parent-owned. No app,
gem, contract or generic22 edits, publication or new threads; work is local and
uncommitted, with the orb and every prior control available for further comparison.
