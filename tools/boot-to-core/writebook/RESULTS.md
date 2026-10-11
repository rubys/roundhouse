# Writebook boot → Core Ruby: controlled rounds

**Latest: [round four](ROUND4.md) executes the real ArrangementHelper method
across original/Core/emitted Ruby; QR is unchanged, Current/AR remain refused.**
This adds an ordinary app-method positive, not a newly executable generated family.
[Round three](ROUND3.md) retains the earlier safe Current refusal and controls.
The report below retains rounds one and two; their evidence is unchanged.
The [next-iteration preparation](NEXT.md) adds exact Current dependency bodies
and 54 passing original assertions, but no new exporter support claim.

**Result: real locked-app boot works; no Rails generator replacement is proved.**
Round two advances from observation-time refusal to root refusal. Real generated
Current accessors now materialize and check cleanly, but **both standalone Core
and actual emitted Ruby fail** because inherited constructor state is missing.
Only the ordinary `QrCodeLink#url` accessor cut has matching original/Core/emitted
execution (four assertions each). The nine initial original contracts still pass
the same 44 assertions; a new actual lightbox-policy contract adds three original
assertions but no scalar/export success. Meaningful-app-method three-way
equivalence has **not been achieved**. No production compiler/runtime, generic
exporter, CI, Gemfile or app source was edited; only supplied snapshots were
extracted in the isolated compiler worktree. No external writes/publication.

## Inputs and integrity

| Input | Exact pin / SHA256 |
|---|---|
| Compiler | [canonical initial commit](https://github.com/rubys/roundhouse/commit/83b6b1458adde2b2db728507fcabc0c2c0557612) |
| Writebook | [basecamp/writebook pin](https://github.com/basecamp/writebook/commit/f3fadd21907ad9b18cb23800d971c2cc25045e2a) |
| Rails Git dependency | [locked Rails commit](https://github.com/rails/rails/commit/3a4961048ad251b50991ae83135d760a8a9e8ae3) |
| Useragent Git dependency | [locked Useragent commit](https://github.com/basecamp/useragent/commit/433ca320a42db1266c4b89df74d0abdb9a880c5e) |
| Initial exporter upload | `0607454626deed7338d3403684ba92c9294429bd3f80437543bb972f3275bc26` |
| Round-two exporter upload | `9f1fcbc80f1ef1076740980b4ca82288f3800e24820f3b68dec6d948140f95c9` |
| Downloaded Writebook archive | `6a00399cc14097d2fff457ed710e46f88daf4904d09dc60ad3ccf8a3c71c79ae` |
| Gemfile | `757cf42f13fa10f34453562cdc11bc084ec0429a0adb0e5d8d4406cc33dac025` |
| Gemfile.lock | `7eb8e5056f245c69c2f066d22d3be33b17130118a81800b9f492cb6e0e55db4d` |
| Built roundhouse binary | `80515e3a6837da1fd1195e7928360891b91bb3328b617958617d0ad01920e252` |

Canonical main was explicitly fetched; it had advanced to
[this commit](https://github.com/rubys/roundhouse/commit/7a2d3de66ffccf5e53ac5d282c42ada3639c1685).
The experiment instead uses an isolated detached worktree at the requested
initial commit, not local main or fork origin/main. Source audit compares all
490 original archive files byte-for-byte: **zero changed/missing files**.
Snapshot `bin/rh`, `capture.rb` and `materialize.rb` also match the upload exactly.
The only tracked worktree difference is the uploaded `bin/rh`, not an experiment
edit. See private delivery evidence `evidence/source-audit.json` and
`evidence/round1.json`.

Runtime: exact app Ruby 3.4.7, Prism 1.9.0, Rails 8.2.0.alpha at the lock's Git
revision, libvips 8.14.1, SQLite 3.53.2, Rust 1.98.1. Frozen installation resolves
128 locked gems. Installation uses Bundler 2.5.18 from `BUNDLED WITH`; the direct
Ruby runner's Bundler setup is 2.6.9. Both use the unchanged frozen lock; no
resolver substitution or alternate Rails release. Gems are local disposable
dependencies, not exported runtimes.

## Round one: what executed

Each original contract boots the real app and loads its real schema into a
separate local SQLite file. Subprocesses use `unsetenv_others: true`, test storage,
dummy secret-key mode and no inherited production credentials. Boot/eager load
prints `BOOT_OK`. This is disposable configuration, **not network sandboxing**.
Core/emitted processes do not load Bundler, Rails or the original app; the
contract explicitly rejects their presence. Emitted source is not patched.

| Actual app API | Original assertions | Core / emitted | Discriminating inputs |
|---|---:|---|---|
| `Section#body`, `markable`, `searchable_content` | 5 | blocked / not produced | two instances, nil, Unicode, empty reassignment |
| `Current#user`, `user=` | 5 | blocked / not produced | distinct instance state, nil reassignment |
| `Access#level`, `reader?`, `editor?` | 8 | blocked / not produced | both enum labels, symbol/string assignment, invalid label preserves old state |
| `Section#title` delegate from Leafable | 4 | blocked / not produced | different actual Leaf targets, mutation of one target |
| `Page#body`, `body=`, `markable` | 5 | blocked / not produced | actual Markdown association, distinct contents, blank/default |
| `Leaf#previous` through Positionable | 3 | blocked / not produced | real persisted siblings at scores −7, 13, 41; first has no previous |
| `Leaf#slug` | 4 | blocked / not produced | punctuation, empty fallback, Unicode transliteration |
| `QrCodeLink#url` + own initializer | 4 | **4 / 4 pass** | different URLs, Unicode fragment, empty string, first instance unaffected |
| `EmbedProvider#allows?` | 6 | blocked / not produced | host mismatch, segment-prefix mismatch, traversal, encoded slash, case |

Positioning creates actual Book/Section/Leaf records, then updates scores through
AR to establish an independent ordering oracle; no fake Relation or attribute
store is substituted. Values in `contract.rb` are independent literal expected
results, not obtained from the exporter/compiler. These are bounded app-specific
contracts, not Writebook's whole test suite.

The positive QR cut excludes `signed`, `from_signed`, verifier/purpose checks,
Rails key generation and secrets, controllers/HTTP, QR rendering, all model/DB
state, Current/request state and non-String inputs. A temporary constant-return
regression failed the **second-instance** URL assertion with exit 1, confirming
that the contract detects sample-value substitution; the temporary file is gone.

## Round one: capture failure provenance

The manifest offers **focused observation** (real framework boot first, capture
before selected model autoload/lazy accessor generation) and an **unobserved
control** (boot/generation complete before selection). Neither replays declarations
or replaces app methods. Diagnostic-only wrappers print metadata on failure and
re-raise the same exception; they do not change admission or wrap eval.

* Full boot observation, after dependency/Bundler setup, fails at
  `ActiveSupport::Logger`: `mattr_reader` eval from `logger_silence.rb:12`, via
  `attribute_accessors.rb:73`, contains an ordinary and a singleton `DefNode`.
  `record_eval` requires only receiverless instance definitions. **It never
  reaches Campfire's native Time alias failure** in this setup.
* Focused Current capture fails while lazily loading its real superclass:
  `ActiveSupport::CurrentAttributes` callback `class_attribute` eval from
  `callbacks.rb:70` contains `SingletonClassNode` and `CallNode`, before the app's
  `attribute :session, :user` generates its accessors.
* All six focused AR cuts stop during model subclass initialization. The failing
  copied/aliased method is `__class_attr_defined_enums` on
  `#<Class:ActiveRecord::Base>`, source `active_support/class_attribute.rb:15`.
  `Enum#inherited` calls the class_attribute writer, whose silence-redefinition
  alias has uncaptured inherited-wrapper provenance. This is earlier than the
  selected app generator, **not evidence that enum/delegate bodies were exported**.
* Unobserved controls fail at actual generated source sites: Section `body` and
  Access `level` at `active_model/attribute_methods.rb:273`; Current `user` at
  `current_attributes.rb:124`; Section `title` at `leafable.rb:11`; Page `body` at
  `action_text_has_markdown.rb:8`; Positionable `other_positioned_siblings` at
  `positionable.rb:27`; Leaf `title` at the AR accessor source. Missing Proc/eval
  provenance remains explicit, not reconstructed from a hand-written substitute.
* EmbedProvider selection pulls in its **actual own initializer** and rejects
  its inherited Kernel `Array` call as runtime outside the declared cut. Keyword
  constructor parameters, mutable arrays/attribute set, constant reads and
  return/regex paths are further source obligations, not passed gates.

Full capture initially also encountered RubyGems gemspec eval when dependency
setup was inside observation. Moving **Bundler setup only** before observation
is consistent with the exporter manifest contract, and exposes the Logger
boundary above; it does not warm Rails/app generators or relax capture. The
subsequent same-snapshot runs reproduce the blocked cuts and QR equivalence.

No failure is skipped to produce a green lane. Native method translation,
mutable closure heaps, class registries, scalar constant reads and uncaptured
Proc provenance are **not demonstrated**. Most are not reached because earlier
capture barriers abort. Boot loading native SQLite/Vips/Redcarpet successfully
does not export them. No native Spinel output or other target execution claimed.

## Strict diagnostics: identities, not totals

`roundhouse check --strict` on the unmodified app exits 1: 0 parse errors,
**29 errors, 258 warnings**, 0 survey gaps. Every severity/code/location/message
line and its multiplicity is retained in `round1.json.app_strict`.
The QR Core strict check exits 0 with zero errors/warnings and an empty diagnostic
multiset; this is a **different bounded input**, not 29 app errors fixed.
All other Core inputs were never produced, so they have **no strict result**.

The existing `tests/writebook.rs` survey inventory was also run at the exact
compiler commit. It **fails its checked-in baseline comparison**. Its independent
survey report has 19 Error, 120 Warning and 148 Info occurrences, plus three
ingest gaps; survey attribution differs from the strict CLI and must not be mixed
with it. Private `evidence/diagnostics-inventory.json` and
`evidence/inventory-baseline-delta.json`
are retained. Changes include 150 added/146 removed diagnostic occurrences,
one added `content_security_policy` ingest gap, five removed lowering warnings,
and no Ruby/Spinel emit-inventory changes. The main failure log names changed
gap-attributed view identities and two `gradual_untyped` occurrences. No baseline
was refreshed or diagnostics suppressed. This inventory test does not exercise
the exporter, and is not a new exporter-caused regression claim.

Private `evidence/qr.ir.json` has 3/3 expression types concretely String
with explicit input RBS. `url` has a String-returning signature, but `initialize`
still has **no method signature** in dumped IR. Zero check diagnostics therefore
does not establish a fully typed cross-target constructor ABI.

## Round two: the same app, compiler, bundle and contracts

Private `evidence/source-audit-round2.json` verifies every intended uploaded
file against the new archive, all 490 app files unchanged, and no production
compiler/runtime differences. The compiler binary hash above is identical.
Private `evidence/comparison-round2.json` verifies all nine
original observation arrays identical and the **entire strict diagnostic identity
multiset identical**, with no added/removed occurrences. Initial evidence and the
initial probe sources remain separately retained, not overwritten.

| Same focused cut | Round-one boundary | Round-two outcome |
|---|---|---|
| Full observed app boot, then Access roots | Logger mixed string eval abort | boot/eager-load/schema/accessor generation completes; root refuses `Access#_read_attribute` |
| Section attributes | inherited class_attribute wrapper during class creation | root refuses `Section#_read_attribute` |
| Current accessors | superclass callback mixed eval | two real copied generated methods materialize; strict 0 errors/0 warnings; Core **and emitted fail** |
| Access enum | inherited class_attribute wrapper | root refuses `Access#_read_attribute`; not enum execution proof |
| Leafable title delegate | inherited class_attribute wrapper | root refuses `Section#association` |
| Page Markdown | inherited class_attribute wrapper | root refuses `Page#association` |
| Positionable.previous | inherited class_attribute wrapper | root refuses `Leaf#send`; dynamic selection is not specialized away |
| Leaf.slug | inherited class_attribute wrapper | root refuses `Leaf#_read_attribute` |
| QR url | three-way pass | unchanged three-way pass, 4 assertions each |
| EmbedProvider.allows? | inherited `Array` constructor call | unchanged refusal |

Unobserved controls still refuse the actual generated source locations; round two
adds owner attribution, not invented Proc/eval provenance. Exact failure identities
**and multiplicities** for both rounds are in the comparison file; compiler
diagnostics and materializer failures are separate inventories.

### Missing inherited state, not an observer or compiler crash

Current's materialization inventory records `user` and `user=` as `method_copy`
from the real anonymous generated-attribute module. Its Core source reads/writes
`@attributes[:user]`, but exports a new `Current` without its superclass or an
initializer. The locked `ActiveSupport::CurrentAttributes#initialize`, source
lines 205–207, actually sets `@attributes = resolve_defaults`. Both independent
executions fail at the **first assertion**, before any write:

```text
BootOwner1#user: undefined method '[]' for nil (NoMethodError)
```

Private `evidence/current.round2.core.rb`, `evidence/current.round2.emitted-class.rb`,
`evidence/current.round2.emitted-methods.rb` and
`evidence/decisive-round2.log` preserve the failure. No attribute store,
initializer patch, manual heap seeding or changed contract hides it. Clean strict
checking is not executable support. Defaults, inherited construction, callbacks,
thread-local singleton state and reset behavior remain necessary responsibilities.

### Additional natural constant-dependent app case

`HtmlScrubber#scrub` reaches the real private `remove_foreign_actions`, whose
unqualified frozen `LIGHTBOX_ACTION` gates permitted actions. The original
contract runs actual Loofah/Nokogiri sanitization: preserve the allowed action on
an anchor, remove a foreign action, remove the same allowed action on a details
element. All three pass. The declared scrub root refuses inherited
`HtmlScrubber#scrub` (`super`) before visiting this lexical constant; constructor
Rails/Loofah namespaces, mutable allowlists and native DOM state are also excluded
obligations, not scalar successes. **Zero scalar provenance events** were reached
in these app cuts. This does not contradict the generic scalar contract; it shows
that Writebook's real dependency graph prevents an end-to-end scalar-positive cut.

Full observation now retains 6,459 final definition records, including 1,948
opaque records: 1,087 conservative installation-provenance records, 406 mixed-eval
records, 410 method-copy records and 45 ambiguous-Proc records. There are also
500 opaque-eval events (events and final definitions are different counts).
Native `Time#to_time` copies, mixed class_attribute evals and unresolved Proc
locations remain explicit opaque metadata. They were not translated or removed.
"Callback replacement or unobserved method_added" is an unresolved provenance
reason, **not evidence that a callback replacement occurred**. The app contracts
pass with the observer, and every attempted cut has **zero collector failures**.
All metadata/reasons are retained in private `evidence/round2.json`.

The extension was built against this app's actual Ruby **3.4.7**. Both interpreter
and YJIT native contracts pass 56 assertions in each baseline/inactive/active
mode, with 18 active notifications; the Ruby-wrapper privacy negative control
fails as intended. Private `evidence/native.round2.log` and
`evidence/native.round2-yjit.log` retain exact observations.
CLI checks: **7 runs, 69 assertions, 0 failures/errors, 1 skip**. The skip requires
ActiveModel/ActiveSupport 8.1.4 absent from this unchanged Rails 8.2 Git bundle;
no alternate gems were installed. An initial US-ASCII JSON decode failure was
diagnosed and rerun with explicit `LANG=C.UTF-8`, not skipped. See
private `evidence/cli.round2.log`. This is not the parent's 75-assertion fixture run.

## App generation inventory → compiler responsibilities

Private `evidence/dsl-sites.json` enumerates 141 source calls in 13 selected
generation/framework families across app/lib/config Ruby. These are **source
sites**, including reusable generator bodies/hooks, not counts of executed
expansions or all Rails DSLs. ERB and route expansion are not inventoried here.
The owner lines below were inspected in the exact controlled compiler worktree.
`ingest/` and `lower/` entries below are under `src/`.

| Family / sites | Writebook examples | Recognizer / synthesizer owners (compiler-relative) |
|---|---|---|
| Attributes / 5 | Current `attribute`, QR/Embed readers, Page cattr, Markdown mattr | `ingest/current_attributes.rs:43,92,204`; ordinary/cattr/mattr accessors `ingest/library_class.rs:2095`; visibility `ingest/visibility.rs:574`; schema accessors `lower/model_to_library/schema.rs:60,996,1334` |
| Enum / 5 | Book theme, Leaf status, Access level, Edit action, User role | `ingest/model.rs:1085 enum_declaration`, `:1186 expand_enum_decl` |
| Delegate / 4 | Leaf search, Leafable title; two delegated_type declarations | `ingest/delegate.rs:154 expand_delegates`, `ingest/model_delegate.rs:15`; `ingest/delegated_type.rs:56,243` |
| Position / 1, generation / 4 | Leaf macro; three define_method bodies plus Markdown class_eval | `ingest/model_macros.rs:25,572`; `ingest/model_macros/class_eval.rs` (bare Markdown claimed separately) |
| Markdown / 1 | Page body | `lower/plain_text_attr.rs:83,103,170,199`; named storage and autosave remain AR work |
| Associations / 17 | book/leaves, polymorphic leafables, Markdown has_one | `ingest/model.rs:2492 parse_association`; `lower/model_to_library/associations.rs:97` |
| Scopes / 20 | published, active, positioned, before/after, includes | `ingest/model.rs:2259 parse_scope`; `lower/model_to_library/mod.rs:1342` plus scope-chain/Relation lowering |
| Callbacks / 46 | model lifecycle and controller before/after actions | model `ingest/model.rs:2003 parse_callback`, `lower/model_to_library/markers.rs:1271`; controller `ingest/controller.rs:623 parse_filter_call`, `ingest/library_class.rs:3561 ingest_concern_filters`, `lower/controller_to_library/process_action.rs:120` dispatcher |
| Framework hooks / 32 | Concern included/class_methods, on_load, to_prepare | `ingest/app.rs:2008 splice_concerns_into_models`, `:2189 splice_concern_class_methods_into_includers`, `:2450 splice_concerns_into_controllers`; narrow `ingest/on_load_reopen.rs:65` fingerprints and `ingest/app.rs:7823 initializer_statements`; not a full boot-hook graph |
| Storage / 3 | Book cover, Picture variant, Markdown uploads | `lower/attached.rs:568,745`; `lower/attachment_model.rs`; `runtime/ruby/active_storage.rb` |
| Security / 2 | User password, Session token | `lower/secure_password.rs:42`; `lower/secure_token.rs:110`; BCrypt/token runtime remains necessary |
| Serialization / 1 | Account embed_providers JSON | `lower/serialize.rs:24 serialize_decls` claims JSON only; `lower/model_to_library/schema.rs:1048,1064,1084` String-slot/JsonColumn accessors; `analyze/mod.rs:8525` gradual reader/writer typing; no exported contract |

**Savings estimate: zero framework responsibilities proved replaceable today.**
The QR result only recovers an ordinary literal accessor, which Roundhouse already
knows how to ingest. Round two proves recovery of two real generated Current
bodies, **not removal of its recognizer/synthesizer**: the existing Current owner
also constructs singleton instances, models storage as ordinary ivars, generates
class forwarding and models thread-local/reset behavior, none recovered by this
cut. The 141-site inventory regenerates byte-identically in round two. Conditional
candidates are source specialization/body construction for delegate, enum
predicates, Current accessors and bounded concern
define_method/class_eval. The existing Positionable handler is already **generic
bounded macro specialization**, not a Positionable-name rule. An exporter would
replace that specialization only after captured Symbol/dynamic-send semantics,
visibility and runtime selection are proved. A `send(parent)` body preserved
literally is not a static specialization win.

Even if those bodies materialize, schema type/conversion rules, SQL/Relation and
association state, ActiveRecord AttributeSet, constructors, callbacks/transactions,
Current's thread/request reset, autosave/dependent destroy, native adapters,
authorization, RBS/API stamping, diagnostics and target runtimes remain necessary.
No percentage of `model.rs` or `model_to_library` is justified by these cuts.
No code was deleted and no line-count saving is asserted.

## Reuse / another controlled round

Prerequisite commands (all local; retain the downloaded app's MIT license):

```sh
git fetch https://github.com/rubys/roundhouse.git main
git worktree add --detach /tmp/writebook-probe-compiler 83b6b1458adde2b2db728507fcabc0c2c0557612
sha256sum core-ruby-input-source.tgz
tar -xzf core-ruby-input-source.tgz -C /tmp/writebook-probe-compiler
# Download/extract the pinned codeload archive into /tmp/writebook-probe-app.
mise install ruby@3.4.7
# PATH must include mise for its RubyGems reshim hook; Gemfile is untouched.
env -i HOME="$HOME" PATH="$HOME/.local/share/mise/installs/ruby/3.4.7/bin:$HOME/.local/bin:/usr/bin:/bin" \
  BUNDLE_PATH=/tmp/writebook-gems BUNDLE_FROZEN=true bundle install --jobs 4
# Run this build inside the exact compiler worktree:
cargo build --locked --jobs 4 --bin roundhouse --bin dump_ir
```

`bundle install` runs in the app directory. The exact codeload URL is
`https://codeload.github.com/basecamp/writebook/tar.gz/f3fadd21907ad9b18cb23800d971c2cc25045e2a`.
The first install attempt omitted mise from PATH and its reshim hook failed;
adding mise to PATH completed the same frozen installation, with no app edits.

From the source checkout, run the unchanged app-specific contract/manifest:

```sh
~/.local/share/mise/installs/ruby/3.4.7/bin/ruby tools/boot-to-core/writebook/run.rb \
  /tmp/writebook-probe-compiler /tmp/writebook-probe-app /tmp/writebook-next /tmp/writebook-gems
```

That four-argument recipe is for the **initial** snapshot and archived initial
probe source. Round two requires the native observer and optional fifth path:

```sh
# Verify the supplied hash; extract only its intended files in COMPILER_WORKTREE.
sha256sum core-ruby-input-round2.tgz
tar -xzf core-ruby-input-round2.tgz -C /tmp/writebook-probe-compiler \
  tools/boot-to-core/capture.rb tools/boot-to-core/materialize.rb tools/native-observer \
  bin/rh tests/rh_materialize_test.rb
RUBY="$HOME/.local/share/mise/installs/ruby/3.4.7/bin/ruby"
source_dir=/tmp/writebook-probe-compiler/tools/native-observer
build_dir=/tmp/core-observer-round2
mkdir -p "$build_dir"
(cd "$build_dir" && "$RUBY" "$source_dir/extconf.rb" && make)
env -i HOME="$HOME" PATH="$(dirname "$RUBY"):/usr/bin:/bin" LANG=C.UTF-8 \
  RUBYLIB="$build_dir" "$RUBY" "$source_dir/contract.rb"
env -i HOME="$HOME" PATH="$(dirname "$RUBY"):/usr/bin:/bin" LANG=C.UTF-8 \
  RUBYLIB="$build_dir" RUBYOPT=--yjit "$RUBY" "$source_dir/contract.rb"
# CLI validation from the compiler worktree (the Rails 8.1.4 fixture skips here):
env -i HOME="$HOME" PATH="$(dirname "$RUBY"):/usr/bin:/bin" LANG=C.UTF-8 \
  RUBYLIB="$build_dir" BUNDLE_GEMFILE=/tmp/writebook-probe-app/Gemfile \
  BUNDLE_PATH=/tmp/writebook-gems BUNDLE_FROZEN=true \
  "$RUBY" -rbundler/setup tests/rh_materialize_test.rb --verbose
# App probe from this source checkout:
"$RUBY" tools/boot-to-core/writebook/run.rb /tmp/writebook-probe-compiler \
  /tmp/writebook-probe-app /tmp/writebook-next /tmp/writebook-gems "$build_dir"
```

Every original/manifest subprocess explicitly allowlists this `RUBYLIB`; private
environment is not propagated. Standalone Core/emitted subprocesses load neither
the app/bundle nor the observer. Reusable sources are `boot.rb`, `manifest.rb`,
`contract.rb`, `provenance.rb`, `run.rb`, `inventory.rb`, plus bounded `qr.rbs`.

The output path must be new. The runner exits successfully when the **experiment
is recorded**, including failures; only a lane's `contract_verified: true` means
strict check and all three fresh-process contracts passed. `materialized: false`
means later stages were blocked, never zero errors. Detailed commands, clean
environment, tool/probe hashes, generated method inventory and actual observations
are in `results.json`. Full subprocess logs and disposable databases remain in
the orb's output directory, not in this reusable source bundle.

Additional executed commands, inside the exact compiler worktree:

```sh
WRITEBOOK_ROOT=/tmp/writebook-probe-app WRITEBOOK_INVENTORY_REPORT=/tmp/writebook-inventory-initial.json \
  cargo test --locked --jobs 4 --test writebook pinned_writebook_inventory -- --ignored --nocapture
target/debug/dump_ir /tmp/writebook-initial-evidence/qr_focused-core --select QrCodeLink --format json
```

Run `inventory.rb PINNED_APP` with the locked bundle to regenerate source sites.
Private `evidence/decisive.log` includes boot,
original observations, representative failure stacks, strict diagnostics and actual
QR execution. Source audit and Ruby syntax checks pass. No CI matrix, complete
Writebook suite or deployment was run. Deliverables are local/uncommitted.

The orb retains the exact app, frozen gems, unchanged compiler, native build and
both full output directories for another **verified exporter snapshot**. Sources
and reports are local/uncommitted. This is the independently owned Writebook
probe; Campfire and real-blog experiments are not duplicated here. Do not treat a
later capture barrier as success or remove difficult roots to improve the headline.
