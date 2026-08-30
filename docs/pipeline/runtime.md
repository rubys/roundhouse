# Runtime

Each emitted project links against a per-target runtime that ships
in two layers: **target primitives** (hand-written) and the
**framework runtime** (transpiled from Ruby).

**Source:** `runtime/<target>/` for primitives; `runtime/ruby/` for
the framework runtime (consumed by all non-Ruby targets via the same
roundhouse pipeline that compiles user apps).

## The two-layer split

```
runtime/ruby/                    ← single source of framework Ruby
  active_record/                    transpiled into the emit pipeline
  action_controller/                  ↓
  action_view/                      framework-runtime files appear in
  action_dispatch/                  the emitted project (e.g. TS emit
  inflector.rb                      writes src/active_record_base.ts,
  json_builder.rb                   src/action_controller_base.ts, …)
  ...

runtime/<target>/                ← per-target primitives, hand-written
  db.<ext>                          DB connections, HTTP server,
  server.<ext>                      WebSocket plumbing, test harness —
  cable.<ext>                       genuinely target-idiomatic glue
  test_support.<ext>                that no IR-level lowering captures
  ...
```

### Why two layers?

- **Framework runtime is target-uniform Rails surface.** Models'
  `validates`, controller action helpers, view helpers like
  `link_to` / `form_with` / `pluralize` — these have one canonical
  Ruby implementation. Authoring them N times in N target
  languages is exactly the duplication the lowering layer was
  built to eliminate; transpiling them once subsumes it.
- **Target primitives are unavoidably idiomatic.** Wiring axum
  middleware vs. Node's `http` module vs. Plug's pipeline DSL vs.
  Crystal's `HTTP::Server` looks different in a way no IR
  captures. Hand-writing these stays cheap because each target's
  primitive layer is small (a few hundred lines).

The forcing function: **any target that compiles user Rails apps
must also compile `runtime/ruby/`**. If your emitter can transpile
ApplicationRecord + FormBuilder + Inflector, it can transpile a
real Rails app. Phase 1 of the unified-IR plan front-loaded this
risk by transpiling `runtime/ruby/` to TypeScript before any user-
app emission depended on the result.

## Target primitives — what's in each `runtime/<target>/`

Conventional file roles (file names vary by target):

| Role | Description |
|------|-------------|
| Model base / shims | `ActiveRecordAdapter` trait / adapter interface, validation error type |
| DB connection | Lifecycle (open, with_conn borrow, test-mode in-memory) |
| HTTP server | Production HTTP entry — listens on a port, dispatches through Router |
| Action Cable | WebSocket endpoint |
| View helpers | Delegates into transpiled framework Ruby (where present) or implements helpers directly (legacy) |
| Test support | TestClient + TestResponse + Rails-shaped assertions |

The exact file set varies per target and rots fast in prose; the
authoritative inventories are the `include_str!` tables in each
target's emitter (`src/emit/typescript.rs`, `src/emit/go.rs`,
`src/emit/elixir.rs`, …) and, for the Ruby family, the runtime walk
in `src/project.rs`. Shape notes worth knowing:

- `runtime/go/` and `runtime/elixir/` keep their primitives under a
  `v2/` sublayout (a strangler-era name the emitted tree still uses)
  — the Go/Elixir emitters copy from there.
- `runtime/typescript/` carries db/server variants selected by the
  `DeploymentProfile` (sync sqlite, libsql, worker), the worker
  bridge (`juntos*.ts`), and async/sync minitest adapters.
  Framework-runtime files (`active_record_base.ts`,
  `action_controller_base.ts`, …) are emitter-generated from
  `runtime/ruby/` and appear under `src/` in emitted projects, not in
  this directory.
- `runtime/spinel/` is by far the largest: per-target Ruby primitives
  (DB adapters per interpreter, CGI/ERB shims, message digests, …)
  plus `tep/` (an embedded HTTP/WebSocket server), `facades/`
  (hand-written typed stand-ins, each with an RBS sidecar), a
  `scaffold/` tree overlaid into
  every emitted Ruby/Spinel project, and a `test/` tree of
  target-specific test files.

## Framework runtime — `runtime/ruby/`

Ruby authoritative source for the Rails surface every emitted app
calls into:

```
runtime/ruby/
  active_record/        ApplicationRecord, validations, querying
  action_controller/    Base controller, Parameters
  action_view/          link_to, form_with, FormBuilder, pluralize, ...
  action_dispatch/      Routing helpers
  action_text.rb        Content (the has_rich_text coder), Attachment
  inflector.rb          camelize / pluralize / dasherize
  ...                   plus mailer/job/storage/params modules and the
                        runtime's own test suite — see the directory
```

`runtime/ruby/test/` is the framework runtime's own test suite, run
per target by the `framework-tests-<target>` CI jobs.

`action_text.rb` holds only Action Text's VALUE layer.
`ActionText::RichText` is absent because it has a table: it is
synthesized as an ordinary model by `lower::rich_text` and reaches
every target through the model machinery. That split — table-backed
things are models, values are framework Ruby — is the general rule,
not an Action Text special case.

Nearly every `.rb` ships with a `.rbs` sidecar declaring the public typed
surface (see `analyze.md` on RBS-paired ingestion). The `.rbs` is
what makes the framework runtime typeable without annotating every
internal expression — only the public boundary commits to a type.

## How files reach the emitted project

Target primitives ship via Rust's `include_str!` at emitter compile
time:

```rust
const RUNTIME_SOURCE: &str = include_str!("../../runtime/rust/runtime.rs");
const DB_SOURCE: &str = include_str!("../../runtime/rust/db.rs");
// ...etc.
```

These strings are written verbatim into the generated project as
`src/runtime.rs`, `src/db.rs`, etc.

Framework runtime files ship via the same emit pipeline that compiles
user apps — `runtime/ruby/active_record/` is ingested with its RBS
sidecar, lowered, and emitted into the generated project as
`src/active_record_base.ts` (etc.) by the same code path that
compiles user controllers and models. The driver is
`src/runtime_loader.rs`: a per-target `TargetEmit` hook set plus
per-target entry points (`typescript_units`, `crystal_units`,
`rust_units`, `go_units`, `elixir_units`, `kotlin_units`,
`swift_units`, `csharp_units`, `python_units`, …) — most targets have
one today. The parse layer underneath is `src/runtime_src.rs`. There
is no separate `bin/build-runtime` binary; emission runs inline as
part of `cargo run --bin roundhouse -- --target <t>` (or `--site` for
the full archive matrix).

The Ruby family is the asymmetry: Spinel / Ruby / JRuby receive the
framework Ruby VERBATIM — the Ruby-family assembly in
`src/project.rs` walks `runtime/ruby/` into the emitted tree as text,
and a text-level tree-shake (`emit::ruby::shake`, run from
`src/project.rs::target_files`) trims what the app doesn't reference
— while every other target gets the IR transpile described above.

## Why hand-write the primitives?

1. **Framework integration is language-idiomatic.** axum middleware,
   Node's event loop, Plug's pipeline, Crystal's `HTTP::Handler` chain
   each look different in a way no higher-level IR captures.
2. **Editability.** When a primitive needs to grow (new middleware,
   new helper hook), editing a normal `.rs` / `.ts` file with IDE
   tooling is faster than editing a string inside a `format!`-driven
   emitter.

The tradeoff: emitters and primitives stay in lockstep. If
`view_helpers.rs` adds a function, the corresponding emitter (or, for
helpers, the `runtime/ruby/action_view/` source) has to learn to call
it. Snapshot tests + toolchain tests catch drift.

## Emitter ↔ runtime contract

For each target:

- **Emitter assumes** specific function names, signatures, and
  imports from the runtime (both layers).
- **Runtime guarantees** those functions exist and behave.
- **Snapshot tests** catch drift in emitter output.
- **Toolchain tests** catch drift in the runtime — if it no longer
  compiles, or if `cargo test` / `tsc --strict` / `crystal build`
  fails, the gate blocks the merge.

When adding a new helper: land the runtime change and the emitter
change in the same commit. Runtime that ships without emitter uptake
is dead code; emitter output that references a non-existent runtime
function is a compile failure in the generated project.

## Key files

| Directory | Role |
|-----------|------|
| `runtime/ruby/` | Framework runtime — Ruby source + RBS sidecars |
| `runtime/rust/` | Rust primitives |
| `runtime/typescript/` | TS primitives (framework runtime is emitter-generated from `runtime/ruby/`) |
| `runtime/crystal/` | Crystal primitives |
| `runtime/{go,python,elixir,kotlin,swift,csharp,spinel}/` | Sibling targets (go/elixir under a `v2/` sublayout) |
| `src/emit/<target>.rs` | Emitter side that reads + embeds the runtime |
| `src/runtime_loader.rs` | Framework-transpile driver — `TargetEmit` hooks + per-target `*_units` entry points |
| `src/runtime_src.rs` | Parse layer: runtime Ruby + RBS → `MethodDef`s (no emission) |

## Deliberate divergences from Rails

Places where the runtime knowingly answers differently from Rails. Each
is a decision, not a gap — a gap belongs in the diagnostics ledger
instead. **A divergence must be recorded here when it is chosen**: an
undocumented one reads as intent to the next session precisely because
it is applied consistently, and the emit gives no signal that anyone
weighed it.

### `id` is `0` before save, not `nil`

`ActiveRecord::Base#initialize` seeds `@id = 0`. Rails answers
`Article.new.id == nil` (measured against Rails 8.1).

**Why.** A nullable primary key means `Option<i64>` in Rust and `Int?`
in Kotlin/Swift/C#, with an unwrap at every foreign-key comparison, path
helper and join. The sentinel keeps ids plain machine integers across
every target. Foreign keys follow the same convention — the
synthesized `belongs_to` readers test `@creator_id == 0`.

**What depends on it.** `ty_of_column_slot` excludes the primary key
from nullability, so the RBS declares `id: Integer` (non-null) while a
genuinely nullable column declares `String?`. The two must agree: a
write of `nil` into `@id` contradicts both the sentinel and the
signature, and spinel widens the slot on the *possibility* — one
unreachable nil arm in `[]=` boxed `@id` on every model in the corpus.

**Where it is visible.** Only where app code reads `id` on an unsaved
record. The framework never does: `form_with` picks its action from
`persisted?`, and so does `dom_id`. `record.id.nil?` is `false` here
where Rails says `true`.

### An attribute writer does not TYPE-CAST to the column's type

Rails casts on assignment: `Message.create!(client_message_id: 999)`
into a `t.string` column stores the String `"999"`, and
`record.client_message_id` reads back `"999"` before and after the
INSERT. The writer this runtime generates is a bare `@col = value`, so
the attribute holds the Integer `999` until the row is written, and the
adapter's `escape_string` is what finally renders it.

**Why.** A cast per write means every generated writer carries the
column's coercion, in every target — and the coercions do not agree
across them (`999.to_s` is not `String(999)` is not `"" + 999`). The
value reaches the database through one escaper per column type, which
is already the single place that knows the column's type, so a write
that goes straight to storage arrives correct without the writer
knowing anything.

**What it costs.** Anything that reads the attribute BETWEEN the
assignment and the INSERT sees the uncast value — which in practice
means a `before_save`/`before_create` callback, and app code that reads
back what it just assigned. campfire's suite writes
`create! client_message_id: 999` into a string column
(`test/controllers/users/sidebars_controller_test.rb:17`), and its
`before_create` guard reads that attribute.

**What depends on it.** Any synthesized guard over a string column must
branch on the VALUE, not on the column type — which is why
`lower::blank::synthesized_string_blank` grounds to
`ActiveSupport.blank?` rather than to the String form
`(r || "").strip.empty?`. The String form raises `undefined method
'strip' for an instance of Integer` on exactly the case above. **A
schema type describes the column, not what the attribute holds before
the INSERT**, and a lowering that reads `attributes.fields` is reading
the former.

The fix, when it is worth making, belongs in the writer — one cast per
string column, at the one site that already knows the type — and it
changes what every reader sees, so it is a change to make deliberately
rather than as a side effect.

### An absent UNSIGNED cookie reads as `""`, not `nil`

`cookies[:missing]` answers `""`; Rails answers `nil`.

**The SIGNED read is nullable and no longer diverges** —
`cookies.signed[:missing]` answers `nil`, and so does anything that does
not verify (a tampered payload, a bad signature, a value signed for a
different cookie name). That is the read where the difference was
visible to an app rather than absorbed by a `.to_s`: `if token =
cookies.signed[:session_token]` is campfire's `SessionLookup`, and an
empty String is TRUTHY in Ruby, so a signed-out request took the
signed-in branch and queried for `token: ""` — right by accident, one
query Rails never makes, and simply wrong for a call site that only
checked presence. Everything below is about the unsigned jar.

**Why.** The store is `Hash[String, String]`, and a nullable String puts
every read on spinel's nullable path: `cookies[k].to_s.split(",")`
yields a null array there, which is how lobsters'
`remove_unknown_cookies` first met this. Every call site in the corpus
coerces with `.to_s`, under which `""` and `nil` are identical.

**What depends on it.** `raw` returns `""` as its final fallback, and
`delete` records a cleared write as `""` rather than a tombstone, so
`@out` stays a plain String→String map — the harness and both
dispatchers read that empty as "expire this cookie". `SignedCookieJar#[]`
sits ON TOP of `raw` and maps its `""` back to nil, so the nullable read
costs the store nothing.

**Where it is still visible.** `cookies[:missing]` is truthy where Rails
is falsy. No corpus call site reads the unsigned jar for presence — every
one coerces with `.to_s` — which is why the signed read was closed and
this one was not.

### An enum attribute reader yields the STORED value

`user.status` answers `0` where Rails answers `"active"`. The generated
predicates and scopes carry the stored value too, which is what makes
them correct with no enum type at runtime. Fixing the reader means
mapping at every read; do it only if an app is found that reads the raw
attribute.

### An attachment sgid resolves only where the caller names its class

`ActionText::Content#attachables` — the untyped list of every record a
fragment's `<action-text-attachment sgid="…">` nodes point at — still
answers `[]`. Its per-model twin does not:
`attachable_ids("User")` verifies each node's sgid and answers the ids
minted for that model, and `lower::attachables_grep` rewrites the shape
app code actually writes into a query over them:

```text
body.attachables.grep(User)  →  User.where(id: body.attachable_ids("User")).to_a
```

**Why the split.** Dereferencing an arbitrary sgid needs a
name-to-class map, and building one means reflection or per-model
registration at load. `grep(User)` has already told the compiler which
class it wants, so that lookup never arises — the name becomes a
literal. A bare `attachables` has no such caller and keeps the `[]`.

**The wire format is ours, not Rails'.** `ActionText::SignedGlobalId`
signs `<Model>/<id>` through the same `MessageVerifier` envelope signed
cookies and `ActiveRecord::SignedId` use, rather than Rails'
`gid://<app>/<Model>/<id>` SignedGlobalID. One signing implementation
instead of three, at the cost that an sgid minted by a real Rails
process does not verify here and vice versa. Both ends of every round
trip in a transpiled app are this runtime, and nothing in the corpus
hands an sgid across that boundary.

**What is left.** A stale sgid (the record was deleted) drops out of
the `where` rather than materializing a MissingAttachable.
`ActionText::Attachment.from_node` and
`ActionText::Attachables::MissingAttachable` do not exist, so the tests
that build an attachment from a parsed node still fail there.
`RichText#to_trix_html` hands back the stored markup instead of
rendering attachment previews into it, so an editor loads the text and
shows attachment nodes bare.

**What always worked.** The PARSE: `#attachments` returns every node
with every attribute it carried (`sgid`, `content_type`, `caption`,
`filename`, `url`), and `to_plain_text` renders an attachment as its
caption or filename exactly as Rails does.

### Action Text decodes only the entities Rails' escaper emits

`Content#to_plain_text` decodes `&amp; &lt; &gt; &quot; &#39; &apos;
&nbsp;` and passes anything else through verbatim — `&lowast;` stays
`&lowast;` where Rails (via Nokogiri) yields `∗`.

**Why.** Decoding an arbitrary reference needs a codepoint-to-character
intrinsic the framework runtime does not carry, and a full named-entity
table is ~2000 entries for a case no corpus app produces.

**Where it is visible.** Plain-text projections only — search
indexing, the `to_plain_text.presence` fallbacks. The round trip that
matters is closed: every entity `ViewHelpers.html_escape` can emit is
in the table, so escape-then-extract recovers the original text.

### A preload scope on a RELATION is the identity, and now exists

`with_attached_<attr>` / `with_rich_text_<attr>` are generated as class
methods on the model. They are now ALSO generated as `ActiveRecord::
Relation` delegates that answer `self`, so a mid-chain call
(`find_autocompletable_users.with_attached_avatar.ordered`) resolves.

**Why.** These scopes are synthesized at emit time beside the
attachment macro, not declared by the app. So the class-side method
existed and the relation-side one did not, and a chain on a relation
VALUE was a NoMethodError on a method that plainly exists.

`build_scope_registry` carries them NOW — that entry is what lets the
scope-body rewriter thread `__rel` through a bare
`with_attached_attachment`, and without it the relation was silently
replaced by a fresh one and every accumulated `where` was lost.
campfire's `/rooms/1` served another room's messages on exactly that.
The delegate still bypasses the registry's general `__scope_` path and
stays identity, because a hop to a body that returns its argument is a
dispatch for nothing.

**Where it is visible.** Nowhere in the results: the delegate is
identity for the same reason the class-side body is (below).

### A rich-text preload scope is the identity

`with_rich_text_<attr>` and `with_rich_text_<attr>_and_embeds` return
the relation unchanged where Rails adds an `includes`.

**Why.** The synthesized reader fetches per record, so there is no
preloaded association for the hint to attach to.

**Where it is visible.** Query COUNT, not query results: a page
rendering N records issues N rich-text queries where Rails issues one.
The methods exist rather than being dropped so that call sites chaining
through them keep working.

MEASURED, so the cost is a number rather than a shrug: campfire's
`/rooms/1` with 40 messages makes 172 database round trips where Rails
makes 13. 161 of the 172 are four readers at ~40 each — the rich text
here, the message's `boosts`, and the two ActiveStorage lookups
(`attached?` and `filename`, two because each call builds a fresh
`Attached`). That is why the emitted tree serves that page at 1.34x
Rails' latency while beating it on every page with no message list
(`/searches`: 0.77 ms against 4.5 ms). `scripts/bench-campfire` is what
measures it; count CACHE MISSES, not `Db.prepare` calls, or the
per-request query cache flatters the number by half.

### A `has_json` column reads back as its stored TEXT

`has_json :settings, restrict: false` gives Rails a `DataAccessor`
object out of `account.settings`, and a decoded Hash out of
`account[:settings]` / `account.attributes`. Here the reader, `[]`, and
`attributes` all give the SERIALIZED JSON text; the schema's keys are
reached through the flat accessors `lower::has_json` synthesizes
(`account.settings_restrict?`), and the two-hop source spelling rewrites
to them. The seam itself is `runtime/spinel/schematized_json.rb` plus its
CRuby overlay twin — per-target like `TypedStore`, so the strict targets
carry the calls as one named unresolved seam until a native
implementation lands.

**Why.** The accessor object answers through `method_missing` and would
need a live back-reference into the record for a write through it to be
visible in the record — neither survives static resolution. The schema
is a compile-time fact, so it expands instead. `[]`/`attributes` are the
STORAGE view (`created_at` reads back through them as its raw ISO text
for the same reason), and the storage here is the serialized column.

**Where it is visible.** Three places. `record[:settings]` is a String
where Rails gives a Hash. Assigning the column a whole Hash —
`update!(settings: { … })`, which Rails casts key by key through the
schema — is UNMODELED and reported: the same writer is where hydration
lands (`from_row` assigns the stored column straight through it), so
telling a Hash of attributes from already-serialized text would need a
runtime type test over an untyped Hash, which no target's Hash surface
resolves. The per-key writer is the supported spelling and the whole-Hash
one is a diagnostic, never a silent `Hash#to_s` in the column. Third: an
integer key gets no `?` predicate, because Rails' `present?` on any
Integer — `0` included — is unconditionally true, and a method that
always answers the same thing is worse than an honest gap.

Analyze additionally types the column reader `untyped` where the emitted
reader returns `String`: that is the source-shaped accessor object, and
it exists only between the two hops the lowering erases.

### `insert_all` runs save callbacks and issues one INSERT per row

`Model.insert_all(rows)` is INLINED at the call site (Ruby family,
`scope_chain.rs`) as `rows.each { |a| Model.new(a)
.save_after_validation }`. Rails issues ONE multi-row INSERT and skips
validations *and* callbacks; this skips validations and their callbacks,
fills timestamps, and runs the save callbacks.

**Why `save_after_validation`.** It is the seam Rails' own
validation-skipping writes (`update_attribute`) already enter at, so
this reuses one definition of "write without validating" rather than
adding a second path that has to be kept in step.

**Why inlined, not a synthesized method.** A per-model `insert_all`
would land on every model of every app to serve the handful that call
it, and its parameter is an untyped attribute Hash — the shape the
has_json work established no target's Hash surface resolves portably. A
shared `ActiveRecord::Base` version is worse still: it would call
`new(attrs)` polymorphically against a `Base#initialize` taking no
attributes, in a file that prices every target. Inlining costs nothing
to an app that never calls it, and leaves strict targets an honest
unsupported diagnostic rather than a method that compiles and misbehaves.

**Conflicts are SKIPPED, and that took a fix.** Measured against
ActiveRecord 8.1.3: `insert_all` renders `INSERT … ON CONFLICT DO
NOTHING`, so a row that already exists is a silent no-op — only
`insert_all!` raises `RecordNotUnique`. A bare per-row save raises, i.e.
it implemented `insert_all!` under the other name, and campfire's
`memberships.revise` (re-granting a membership to a user who already has
one) died on a UNIQUE index where Rails does nothing at all. Each row is
now guarded by an existence check on the table's unique keys, built from
the schema by `lower::scope_chain::build_unique_keys`.

The guard is a pre-check, not the database's atomic `DO NOTHING` — the
same read-then-write shape `increment!` below already carries, and under
single-threaded dispatch the window it opens is not observable. A unique
index over a NULLABLE column is skipped when building the guard:
`where(col: nil)` asks whether a row holds SQL NULL, which is a
different question, and in SQLite such rows never conflict anyway.

**What it costs.** N statements instead of one, plus one SELECT per row
for the conflict check, and callbacks Rails would not run — visible on
any model whose `after_create` has side effects. The corpus caller
(campfire's `Room has_many :memberships do def grant_to … end end`)
inserts Membership rows whose callbacks are inert.

**And it answers a different value.** Rails returns an
`ActiveRecord::Result`; the inlined `rows.each { … }` returns `rows`,
the Array of attribute hashes it was given. The catalog says
`ArrayOfUntyped` for that reason — the type of what this pipeline
actually produces, not of what Rails produces. Both corpus call sites
discard the value. Saying nothing was not the neutral choice: a catalog
entry carrying no return kind falls through to the same place an
UNKNOWN method name does, so campfire's `Membership.insert_all(…)`
reported `no known method insert_all on Class { Membership }` while the
emitted code was the inlined loop, correct all along.

### `Current.reset` replaces the instance instead of nilling it

`ingest::current_attributes` turns `class Current <
ActiveSupport::CurrentAttributes` into a plain singleton, and `reset`
used to set the slot to nil. Rails' `CurrentAttributes.reset` puts every
attribute back to its default, which a fresh instance also does — so at
runtime the two agree, at the cost of one allocation per request.

They do not agree in the type system, and that is why it changed.
`@__instance`'s type is the union of what the class assigns it, so the
single `= nil` made `self.instance` answer `Current | Nil`; the
class-level forwarders are all `Current.instance.<name>`, so every one
of them registered `Untyped`; and campfire routes essentially all
per-request state through those forwarders. One `nil` in a synthesized
method, and `Current.user.rooms` — plus every ivar downstream of one —
had no shape.

**`Current.<attr>` keeps its Nil arm**, unlike the controller-wide ivar
seed, which strips it. The write sites are the seed (`Current.user =
bot` in the authentication concern says `User | Nil`), and something
reads that arm: `def signed_in?; Current.user.present?; end` folds to
the constant `true` against a non-nilable type — a correct fold of an
incorrect type, which signed every visitor in and stopped the join-code
page from 404ing. A lie the type system can act on is worse than a gap.

### `list.many?` becomes `ActiveSupport.many?(list)`

Another `Enumerable` core_ext reopen the transpiled runtimes cannot host
— same home and same rule as `index_by` beside it, the receiver
evaluated exactly once. Rails writes the no-block form as a
short-circuiting `any?` with a counter so it stops at the second hit;
the receivers that reach here are already materialized, so `length > 1`
answers the same question.

The BLOCK form (`many? { … }`) is not rewritten: it counts matches
rather than elements, which is a different question no corpus app asks,
so it stays visible. Neither is a receiver that is not an `Array` —
`ActiveSupport.many?` names an Array parameter, and a Relation has its
own surface.

### `hash.to_json` becomes `JSON.generate(hash)`

`Hash#to_json` is a core_ext reopen — Ruby's json library adds it,
ActiveSupport replaces it — and a reopened builtin is the one shape no
strict target can host and spinel cannot dispatch on. `lower::to_json`
rewrites it to the bundled JSON package every emitted tree already
requires, which takes the same collection and answers the same String.

The gate is the receiver's type: `Ty::Hash` or `Ty::Array` only. A MODEL
receiver is deliberately excluded — `record.to_json` is Rails'
`as_json`-then-encode, which the `as_json_*` passes own and which
answers the model's declared shape rather than its ivars — and so is an
untyped receiver, where the rewrite would be a guess.

**The divergence:** ActiveSupport walks `as_json` first, so a `Time`
value renders in Rails' ISO-8601 form where `JSON.generate` refuses it.
The corpus receiver (campfire's `Webhook#payload`, which is the entire
request body a bot receives) holds strings, integers and nested hashes
of the same; a value JSON cannot encode raises rather than rendering
wrongly.

### `ActiveStorage::Blob.service` and `create_and_upload!` raise

The last unnamed corner of Active Storage's bytes half. `runtime/ruby/
active_storage.rb` now defines `ActiveStorage::Blob.service` (answering
an `ActiveStorage::Service` whose `path_for` raises) and
`ActiveStorage::Blob.create_and_upload!` (which raises), in the same
voice and for the same reason as `Attached#url` and
`RouteHelpers.rails_blob_path` beside them: no storage service, no
processor, no signed ids, and the shared runtime does no file I/O
anywhere.

What changes is where the gap lands. campfire writes both —
`Blob.service.path_for(variant.key)` serves the custom account logo and
a bot's webp avatar, `create_and_upload!` stores a webhook's attachment
reply — and until these existed each was an unresolved call that stopped
the whole strict build, which reads like a compiler bug rather than the
gap it is. `Service` is a class rather than a module because
`Blob.service` is a singleton in Rails and app code chains off it, so a
real service can land later without touching a call site.

### `ActionCable.server` exists; the registry did not say so

`runtime/spinel/action_cable.rb` has had `ActionCable.server`,
`Server#broadcast`, `#remote_connections`, `RemoteConnections#where` and
`RemoteConnection#disconnect` since the cable work. None was in the
analyzer's class registry, so campfire's
`User#close_remote_connections` — the one caller — read out as
`no known method server on Class { ActionCable }` about a chain the
emitted tree resolves. Registered, not implemented: the divergence that
the selected connection set is always EMPTY is recorded with the cable
notes, unchanged.

### `IPAddr` is a port, and it rejects a prefix

`runtime/ruby/ipaddr.rb` implements the slice of Ruby's stdlib class the
corpus reaches: `IPAddr.new(str)`, `loopback?`, `private?`,
`link_local?`, `ipv4?`/`ipv6?`/`ipv4_mapped?`, `to_s`, and
`IPAddr::InvalidAddressError`. campfire's `Ban#ip_address_is_public`
is the caller.

**The rule table is the stdlib's, verbatim** — every prefix copied from
ipaddr.rb's own masks and comments (127.0.0.0/8; 10.0.0.0/8,
172.16.0.0/12, 192.168.0.0/16, fc00::/7; 169.254.0.0/16, fe80::/10; and
the IPv4-mapped form of each). Deriving them would be the inflector
mistake in a more expensive place: an address wrongly called public is a
ban that does not take, and nothing raises. `runtime/ruby/test/
ipaddr_test.rb` asserts the stdlib's own answers over 42 addresses that
sit on both sides of every boundary, generated by running Ruby's
`IPAddr` over the list.

**The representation is not the stdlib's.** Ruby keeps one Integer,
128 bits wide for IPv6; no strict target has that, and a bignum here
would cost nine runtimes a primitive to answer three predicates. The
port keeps octets — every rule in the table is a prefix test, so they
answer it exactly as well.

**What it does not do**, and where that shows: no prefix/netmask
(`IPAddr.new("10.0.0.0/8")` raises `InvalidAddressError` where Ruby
masks the address), no `include?`/`to_range`/`succ`/arithmetic, and
`to_s` renders IPv6 uncompressed. Rejecting a prefix loudly is the
deliberate choice over answering about `10.0.0.0` when the caller wrote
`10.0.0.0/8`.

**The CRuby and JRuby trees get Ruby's own instead.** `project::
ruby_runtime_files` rewrites the emitted tree's ipaddr.rb to a one-line
`require "ipaddr"` — the same "one require path, target-appropriate
implementation" split the emitted db.rb carries
(`runtime/spinel/db_cruby.rb` here). That is not tidiness:
something on the CRuby side already loads the stdlib's ipaddr, and two
definitions of `IPAddr::InvalidAddressError` with different superclasses
is a `TypeError: superclass mismatch` at REQUIRE time. campfire's suite
went 219/240 to 0/240 on it, every file dying on the same line.

### `Random.uuid` reads the CSPRNG, not the PRNG

`securerandom.rb` defines one module, `Random::Formatter`, and extends
BOTH `Random` and `SecureRandom` with it — `uuid` is the same code
either way, and only the byte source underneath differs. (It is also why
`Random.uuid` is undefined until something requires securerandom;
campfire's `Message#client_message_id ||= Random.uuid` works because
Rails already has.) Nothing on any target defines it, so
`lower::random_formatter` rewrites the receiver to `SecureRandom`, the
name the emitted tree already carries.

The divergence is the generator: Rails' call reads `Random`'s default
Mersenne Twister, ours reads the OS CSPRNG. The value is a v4 UUID
string either way and every consumer treats it as an opaque id, so this
is the safe direction — but an app that wanted a *reproducible* uuid out
of a seeded `Random` would not get one. Only the names `Random` cannot
answer on its own are rewritten: `Random.rand`, `Random.bytes`,
`Random.new_seed`, `Random.srand` and `Random.random_number` are real
methods on the PRNG class with their own meaning.

### `increment!` / `decrement!` are read-modify-write, not atomic

Rails issues `UPDATE … SET col = col + 1`, so two concurrent callers
both land. `lower::column_ops` rewrites the call site to `self.col =
self.col + 1; touch`, which reads, adds and writes — the same answer
with one writer, a LOST UPDATE with two.

**Why the call site at all.** The alternative is a shared
`increment!(name, by, touch:)`, whose column parameter means
`self[name] = …` — an index write through a variable key, the shape
that keeps `touch` no-arg (see below). At the call site the column is a
literal, so the write is an ordinary typed attribute assignment.

Only the `touch: true` spelling is claimed, which is what the corpus
writes. The bare `increment!(:col)` keeps its NoMethodError: reproducing
it would mean persisting the counter WITHOUT stamping `updated_at`, and
a silently wrong timestamp is worse than a missing method.

### A bad signed id raises `RecordNotFound`, not `InvalidSignature`

`record.signed_id(purpose:)` and `Model.find_signed(id, purpose:)` are
rewritten at the call site by `lower::signed_id` — Rails'
`combine_signed_id_purposes` reads `self.class.name`, and this runtime
is deliberately reflection-free, so the model name is folded into the
purpose string at compile time (`signed_id(purpose: :avatar)` in
`User::Avatar` becomes the literal `"user/avatar"`).

The WIRE FORMAT is not a divergence: `ActiveRecord::SignedId` signs
through the same envelope the cookie jar uses, and
`runtime/ruby/test/action_controller/message_verifier_test.rb` pins the
emitted bytes against tokens minted by a real ActiveSupport 8.1.3. A
token this runtime writes is one Rails reads, and vice versa — which is
what a migration needs, because campfire puts a signed id in the URL of
every rendered avatar.

What diverges is the FAILURE. `ActiveRecord::SignedId.verified_id`
answers `0` for a token that does not verify — tampered, wrong purpose,
or expired — the same non-nil-sentinel posture as the verifier's `""`.
The lowered `find_signed!` is therefore `Model.find(0)`, which raises
`ActiveRecord::RecordNotFound` where Rails raises
`ActiveSupport::MessageVerifier::InvalidSignature`.

**CLOSED for the BANG form.** `find_signed!` verifies through
`SignedId.verified_id!`, which raises
`ActiveSupport::MessageVerifier::InvalidSignature` for a token that does
not verify and leaves `RecordNotFound` to a token that DOES and names no
row — Rails' own split. The name is the whole point: campfire's
`Users::AvatarsController` rescues the signature error BY NAME over an
avatar URL carrying a signed id, and against a `RecordNotFound` that
rescue never fired.

**Still divergent:** the non-bang `find_signed` reads the same `0`
sentinel through `find_by(id: 0)`, which answers nil — the same answer
Rails gives, by a different route. A row whose id really is `0` would
be indistinguishable, and no schema here mints one.

Rails' `expires_at:` (an absolute instant) is not claimed; only
`expires_in:`. A call passing it is left alone rather than rewritten,
so it fails by name instead of silently minting a token that never
expires.

### `remote_connections.disconnect` selects an empty set

`ActionCable.server.remote_connections.where(current_user: user)
.disconnect` returns without closing anything. A remote connection is
selected by its connection *identifiers*, and no connection in this
runtime ever registers one: Turbo's streams subscribe by signed stream
name through `Cable::Connection#handle_message`, and channel
subscription dispatch — the half that would run `identified_by
:current_user` — is not implemented. The selected set is genuinely
empty, so the no-op is accurate rather than a stub.

**What it costs.** campfire calls this from `User#deactivate` and
`User#reset_remote_connections`: a deactivated or banned user's live
socket stays open. Nothing in the corpus's own tests can observe it
(they assert on the database rows the same method deletes), which is
exactly why it is written down here. When subscription dispatch lands,
`ActionCable::RemoteConnections#where` is where the real selection goes.

### A raw cable payload is JSON TEXT on spinel, a Hash on CRuby

`ActionCable.server.broadcast(stream, payload)` — the low-level publish
that is NOT the Turbo Stream family — carries its payload differently on
the two Action Cable substrates.

The CRuby overlay keeps the Ruby Hash the whole way to the transport and
lets it serialize. `runtime/spinel/action_cable.rb` cannot: a `payload`
parameter that must hold `{room_id: 1}` today and whatever the next app
writes tomorrow is exactly the untyped bag a strict target has no lane
for. So the Hash is rendered to JSON text at the boundary
(`ActionCable.payload_json`) and everything downstream carries String.
`Cable.publish_raw` splices that text UNQUOTED into the envelope's
`message` field, which is what keeps the wire shape a JSON *object*
rather than a JSON string — the silent failure the overlay's own header
warns about.

**What it costs.** `Broadcasts::LOG` records the rendered text where the
overlay records the Hash, so a test that reads `entry[:payload]` and
subscripts it passes on CRuby and does not on spinel. Both entries carry
`action: :message` and the stream, which is what `assert_broadcasts`
reads, so the test helper itself agrees across the two. The narrower
consequence is in the renderer: `payload_json` writes Integer values
only, because two call sites in one app is the whole surface anybody has
asked for. A String or nested value needs the renderer widened — and
that is a monomorphization decision to take deliberately, not a cast to
sneak in.

### `rails_blob_path` raises rather than returning a URL

Active Storage's engine-mounted route helpers are not in the app's
`config/routes.rb`, so the generator that reads it never emits them.
Until now that meant a view writing `rails_blob_path(message.attachment,
disposition: "attachment")` — campfire's download link, on every
attachment message — emitted a call to a method NOTHING defined: a
NoMethodError at render time on CRuby, and `unsupported call:
(CallNode 'rails_blob_path')` on a strict target, which reads like a
compiler bug rather than a missing feature.

They now exist, on `RouteHelpers` in `runtime/ruby/active_storage.rb`,
and they RAISE — the same voice and the same reason as
`ActiveStorage::Attached#url` beside them: the bytes half of Active
Storage is a storage service, a processor and a signed-id scheme, none
of which exist here, and a plausible-looking URL is a page that renders
a broken image. What changed is that the gap has one named home every
target compiles instead of a missing method.

**Still open, and visible in any campfire emit:** not every call site is
qualified. `lower::controller_to_library::rewrites::rewrite_route_helpers`
adds the `RouteHelpers.` receiver for controllers and tests; a library
class (campfire's `Messages::AttachmentPresentation`) and at least one
view interpolation context keep the bare `rails_blob_path(...)`, so the
same helper is spelled two ways in one emitted tree.

### A `has_secure_token` column fills at CREATE, not at initialize

Rails 7.1+ defaults `has_secure_token` to `on: :initialize`, so
`Session.new.token` already holds a token. `lower::secure_token`
expands the macro into a `before_create` default instead, so the token
is readable from the moment the record is SAVED. Every corpus call site
reads it after a save, which is what a session token is for. `on:
:create` is exactly this lowering, and `on: :initialize` gets it too —
keeping half of Rails' own default would be the worse divergence.

The generator is `SecureRandom.alphanumeric(<length>)`, not Rails'
`SecureRandom.base58`: base58 is not core Ruby (it arrives with
`active_support/core_ext/securerandom`, which the emitted app does not
load), while `alphanumeric` is core and already carried by every target.
Same length, different alphabet.

**Why it matters that this expands at all.** A dropped
`has_secure_token` is not an absent method, it is a wrong column value:
this runtime defaults a string slot to `""`, so every unsaved record
carries the same token and the second INSERT dies on the table's UNIQUE
index.

### `assert_enqueued_with` checks the job, not its arguments

Rails matches both `job:` and `args:`. This runtime's helper
(`runtime/spinel/test/test_helper.rb`) matches the job class only.

**Why.** Matching the arguments needs the enqueue log to carry them, and
a record argument would then compare by object identity — the test's
fixture and the controller's freshly-loaded row are different objects
where Rails compares serialized GlobalIDs. A check that fails for the
wrong reason is worse than a narrower one that says so.

**What it costs.** A job enqueued with the RIGHT class and the WRONG
arguments passes here and fails in Rails. Nothing in the corpus depends
on the distinction today; campfire's one site asserts a ban job for a
user, and the class is unique to that path.

### A job enqueues under test and runs inline in the app

Rails picks its adapter per environment: `:test` enqueues without
running, the app's runs the job for real. This runtime does the same
with one seam. `lower::job_class_side` synthesizes

```text
def self.perform_later(a, b)
  ActiveJob.record_performed("X")
  new.perform(a, b) if !ActiveJob.enqueue_only
  nil
end
```

`ActiveJob::ENQUEUE_ONLY` is an empty suspension stack by default, so a
running app dispatches at the call site — there is no queue daemon
in-process, which is the `:inline` adapter's semantics. The emitted
test harness pushes onto that stack at load, so the suite enqueues.

**Why the difference is load-bearing.** campfire's `Message` carries
`after_create_commit -> { room.receive(self) }`, whose tail is
`Room::PushMessageJob.perform_later`. Under inline dispatch every
message a FIXTURE loads runs `Room::MessagePusher`, and the suite dies
in that job's unresolvable nested join before a single assertion runs.
Rails' own suite never reaches the code, for exactly this reason.

**What it costs.** `perform_later` answers `nil` rather than the
perform's value. Rails answers the job (or `false`), never the result,
so nothing portable reads it — and a Nil return is what lets the
guarded call sit in statement position instead of forcing a
`<perform-return> | nil` union on the strict targets.

`perform_now` is ungated in both environments: its Rails semantics is
already "run now".

**AND A SERVED APP IS THE THIRD ENVIRONMENT, which this framing missed.**
Rails' production adapter does not run the job in the request either — it
enqueues, and a worker runs it. Inline dispatch is therefore closest to
Rails only for an app with no queue at all; for campfire it puts
`Room::PushMessageJob` inside `POST /rooms/:id/messages`, where it dies on
`uninitialized constant Net::HTTP::Persistent` and 500s a request whose
broadcast had already gone out. `scripts/campfire-cable-walk` splices
`ActiveJob.enqueue_without_running` for that reason and says so in its
header. What this runtime does not have is the third option Rails
actually uses: enqueue now, run elsewhere, later.

### ActiveJob's test helpers count NAMES, and `perform_enqueued_jobs` re-enters inline

`ActiveJob::PERFORMED` is the queue-inspection seam, appended by the
`perform_later` wrapper before the adapter gate — so it is an ENQUEUE
log in both environments. It holds class NAMES, not arguments (a class
is not a first-class value on the strict targets, and the call sites
that name one are rewritten to the string by `lower::job_test_only`).

`perform_enqueued_jobs { … }` therefore cannot replay a queue: it holds
no arguments. It switches back to inline dispatch for the block
instead, so the jobs the block enqueues run as it enqueues them — the
same observable behaviour for a block that enqueues and then asserts,
and different only for one that enqueued BEFORE the block opened.

### A terminal must leave the relation as it found it

`pluck` / `ids` / `pick` set a projection (or a limit) on the relation
to build their query. They now RESTORE it. This is a note about an
invariant rather than a divergence, because the alternative was not a
divergence either — it was silent corruption.

**What it looked like.** `pluck` wrote `@select_sql = "users.id AS v"`
and left it there. The next `to_a` on that same object hydrated whole
records out of a one-column row: every field blank, every id `0`, no
error raised, nothing logged. campfire's
`Rooms::Direct.find_or_create_for` plucks user ids in `find_for` and
then hands the SAME relation to `grant_to`, which duly created
memberships for user 0.

**The invariant.** This Relation is deliberately mutate-and-return-self
for CHAIN methods (`where`, `order`, `limit` …) — the class doc says so,
and lowered chains rely on it. A TERMINAL is the other kind: it runs a
query and answers a value, and Rails builds it a query of its own. Any
terminal that has to touch relation state to build its SQL must put that
state back.

`first` and `find(id)` were the same shape and now restore too — `first`
puts the limit back and drops the one-row `@records` (a cache holding
the single row it asked for would answer a later `each` with one row out
of many), `find` pops its id predicate. Neither moved a test; they are
here so the invariant above has no known live exceptions.

The restore is not exception-safe: a raise inside the terminal leaves
the state set, because `begin`/`ensure` is not available in this file
(see the note on `Model.connection` in base.rb — the rescue-carrying
surface lives in the ruby-family `connection.rb` reopen). The push and
the pop are symmetric on every path that returns.

### A `has_many :through` reader is a live Relation — only here

`user.rooms`, `room.users`, `user.reachable_messages`: a reader for an
association declared `through:` returns an `ActiveRecord::Relation`, not
an Array of rows. Its declared type says so, and the chains campfire
writes on top of it — `Current.user.rooms.find_by(id: …)`,
`room.users.where.not(id: …)`, `Current.user.rooms.original` — resolve
against the relation surface because of it.

**Why.** The direct-fk query the shared lowering synthesizes for every
has_many (`Room.where(user_id: @id)`) is simply wrong when the key lives
on the join table, so the Ruby family rebuilds the body as
`ActiveRecord::Relation.new(Room).joins("INNER JOIN memberships …")
.where("memberships.user_id = ?", @id)`
(`emit::ruby::library::apply_through_assoc_lowering`). That rebuilt body
returns a relation. Declaring `Array[Room]` over it was a signature that
disagreed with the method, and nothing raised: `Current.user.rooms`
typed as an Array means `.find_by` is not a known method, so the
controller method wrapping it registered `-> untyped`, so the
route-param lowering had no model type to see and left the RECORD in the
path — `/rooms/#<Room:0x…>` where Rails writes `/rooms/1`.

**What it costs — and who pays.** The rebuild is Ruby-family only. Every
other target still carries the direct-fk body, which on Rust reads
`Story::where(category_id: self.id)` for lobsters'
`has_many :stories, through: :tags` — a column `stories` does not have.
That emit compiled and returned the wrong rows. With the reader typed
`Ty::Relation`, those targets now meet a relation at emit and report
`relation_type` unsupported instead, which is the ledger doing its job:
a named gap where there was a silent wrong answer. Closing it means
moving the through rebuild into the shared lowering, where every target
gets the join.

### Conditional GET is ALWAYS FRESH

`fresh_when(record)` is a no-op and `stale?(etag:)` answers `true`, so
every conditional-GET request renders instead of ever answering 304.

**Why.** Both halves of Rails' comparison are missing. The controller
has no request object — only `@request_format` — so there is nothing to
read `If-None-Match` / `If-Modified-Since` FROM, and the extra-header
hash on the buffered response is never sent by the CGI harness, so
there is nothing to write `ETag` / `Last-Modified` TO. Wiring either
one is a change to the harness, not to this method.

**What it costs.** Bandwidth, not behavior. Always-render is the answer
Rails itself gives when a client sends no conditional header, so no
response is ever WRONG — the 304 is simply never earned. That is why
this can ship ahead of the plumbing, where a raise or a missing method
could not: three campfire controllers gate real work on `stale?`, and
without it they answered nothing at all.

**Shape.** Monomorphic on what the corpus writes — `fresh_when
@messages`, `stale?(etag: record)`. Rails accepts more
(`fresh_when(etag:, last_modified:)`, `stale?(record)`); each gets its
own method here when a call site asks, rather than one method with a
union parameter no strict target can narrow. `fresh_when`'s body is
EMPTY rather than a bare `nil` — a lone `nil` gives Rust an `Option`
with nothing to infer from (`E0282` on `None;`), where an empty body is
a plain `void`.

### `expires_in` records Cache-Control but emits no header

`expires_in 1.year, public: true` records the max-age and the
public/private flag on the controller and stops there. No
`Cache-Control` header is produced, so a real client is told nothing
about caching and re-fetches every time.

**Why.** The same unsent extra-header seam the conditional-GET entry
above describes, approached from the writing side rather than the
reading side: the CGI harness emits status, body and content type, and
nothing else. Composing the header string into the buffered `headers`
hash ahead of that would be work nothing reads — and `@headers[k] = v`
does not survive the Rust emitter, which renders a Hash index-assign as
`self.headers[k] = v` where `HashMap` wants `.insert()` (E0594:
`IndexMut` is not implemented for `HashMap`). That emitter gap is worth
closing on its own; it is not worth carrying dead code to reach.

**What it costs.** Bandwidth, and only bandwidth — an uncached response
is a correct response. Nothing an app does depends on the client
honoring it.

**Where a test differs from a client.** `response.cache_control` reads
the controller's recorded values directly, not a parsed header, so a
test asserting `cache_control[:max_age]` sees the right answer while an
HTTP client sees no header at all. That is the honest reading of what is
implemented: the VALUE is computed, the TRANSPORT is not.

`stale_while_revalidate:` is accepted and recorded nowhere for the same
reason — it exists so that campfire's logos and avatars actions, two of
its three call sites, do not raise ArgumentError on an option this
method would otherwise not know.

**Shape.** Rails' `response.cache_control` is `{public: true, max_age:
31556952}` — an Integer and a boolean in one Hash, the type bag every
strict target pays for. The controller keeps the two facts apart as
`cache_control_max_age` (Integer) and `cache_control_public` (bool), and
only the TEST harness reassembles Rails' Hash, since the subscript
spelling is what a test writes and the harness ships to the Ruby family
alone. The rest of Rails' options (`must_revalidate:`,
`stale_if_error:`) join `stale_while_revalidate:` when a call site asks,
rather than as a splat nothing can type. The seconds argument is
grounded at the CALL SITE by `lower::duration::rewrite_expires_in` —
the same `.to_i` unwrap `signed_id(expires_in:)` gets — so the runtime
signature stays `Integer` and no strict target pays for an `untyped`
parameter.

### `Relation#find` raises `RecordNotFound` — as Rails does

Recorded because it USED to answer `nil`, and code written against the
old behavior would now see the raise.

`find(id)` raises when there is no such row; `find_by(conditions)` still
answers nil. That is Rails' own split, and the raise is what turns a
missing record into a 404 rather than a nil that NoMethodErrors a few
frames later — campfire's `Current.user.rooms.find(params[:room_id])
.users` on a non-member room read "undefined method 'users' for nil",
against a test asserting `assert_raises ActiveRecord::RecordNotFound`.

**Still divergent:** the message. Rails names the model and the id
(`Couldn't find Room with 'id'=3`); this names the TABLE, because the
model is held as an untyped class value here and reading `.name` off it
would make the message a gradual site.

### `ActiveRecord::Relation` has no `new`

Rails builds a record through a relation — `User.active_bots.new`,
`room.memberships.new` — seeded from the relation's equality conditions.
There is no `Relation#new` here, and there cannot be one.

**Why.** Under spinel a class's constructor is already
`sp_<Class>_new`, so an instance method named `new` on `Relation`
compiles to a second `sp_Relation_new` and the C compiler rejects the
program outright (`conflicting types for 'sp_Relation_new'`). The name
is spoken for on any target that derives a constructor symbol from the
class name; a runtime method cannot claim it. Measured, not predicted:
one landed briefly and turned every spinel job red.

**What it costs.** Nothing at the call sites the corpus writes, because
the call never reaches a method: `lower::scope_chain` rewrites it away.
Inside an association-scoped class method, `new` becomes
`new(__rel.scope_attributes)` — that is where `user.sessions.create!`
gets its `user_id`. On a relation-valued RECEIVER, the same rewrite one
layer out moves the constructor to the model:

```text
User.active_bots.new         ->  User.new(User.active_bots.scope_attributes)
room.memberships.new(attrs)  ->  Membership.new(__rel.scope_attributes.merge(attrs))
```

The receiver stays where it is — a relation is lazy, so reading
`scope_attributes` off it runs no query — and the caller's own
attributes ride on the OUTSIDE of the merge, which is Rails' order.

**Still divergent:** the seed itself. Rails' `scope_for_create` is
`where_values_hash`, so EVERY equality condition on the relation
pre-fills the record; here only an association seed (`where_scope`)
writes the create-seed slot, so a plain scope's conditions filter reads
and do not seed writes. `User.active_bots.new` comes back without its
`role`. An argument shape the rewrite does not admit — a positional
value, a splat — is left alone and still raises.

### A scope-INDIFFERENT class method runs unscoped

Rails runs `User.active.find_by_transfer_id(id)` with the relation as
the current scope, so any query the body makes is filtered by it. Here
the call reaches a class method that some call site named through a
relation chain (`scope_chain::collect_relation_class_method_demand`),
and the method grows the same trailing `__rel` an association-scoped one
does — defaulted to `Relation.new(self)`, so a direct `Model.x` call is
unchanged.

**What it costs.** The parameter is only READ when the body's own shape
says it should be: a constructor merges `__rel.scope_attributes`, a
query at implicit self roots on `__rel`. A body that does neither —
campfire's `find_by_transfer_id`, which is `find_signed(id, purpose:
:transfer)` — takes the relation and ignores it, so its lookup runs
against the whole table. An inactive user found by a valid transfer id
comes back here where Rails would answer nil.

The classification is made at SURVEY time, before `find_signed` has been
lowered to the `find_by` it becomes, which is why this one reads as
indifferent. Recognizing the sugar earlier would close it.

### `send_file` reads the whole file, and only the options it names

Rails STREAMS the file at `path`; `lower::send_file` grounds the call to
`send_data File.binread(path)` — the whole file in memory, because this
controller IS its own buffered response and its body is a String, so
there is no handle to pass down. Every corpus call site sends an image
measured in kilobytes.

The read is at the CALL SITE rather than in `runtime/ruby/`, and that is
deliberate: the shared runtime does no file I/O anywhere, because every
file under it transpiles to every target. See the pass's own header.

**What it costs.** `:filename`, `:status`, `:url_based_filename`,
`:stream` and `:buffer_size` are not reproduced, and a call carrying one
is LEFT ALONE rather than silently stripped — it fails by name, which is
a ledger entry rather than a response quietly missing a header. A
missing path raises `Errno::ENOENT` from the read where Rails raises
`ActionController::MissingFile`.

### `Attached::One#destroy` purges the blob too

Rails' `attached.destroy` falls through to the ATTACHMENT record, whose
destroy removes the join row and leaves the blob to a `purge_later` job.
There is no job here and an orphaned blob row would make `attached?`
answer for a file no longer attached, so `destroy` is `purge` — the same
reasoning `attach`'s replace-first already carries.

### An account logo is served STOCK, never resized

`variable?` is false and `variant` raises (no blob store, no processor),
so `Current.account&.logo_variant(size)` answers nil and campfire's logo
endpoint falls through to its stock icon on every request — including
the ones where a custom logo IS attached.

Worth naming because of how it MEASURES: campfire's own tests assert the
response's pixel dimensions, and the stock icon is 512×512 and 192×192 —
exactly the sizes the custom-logo tests expect. They pass on the
fallback. A dimension assertion cannot see this divergence; only the
pixels could.

### A rich text materialized by a READ is not written through

`message.body` answers an `ActionText::RichText` whether or not a row
exists — that is what makes `body.to_plain_text` safe on a message with
none. Rails AUTOSAVES that built record, putting an empty row in the
table for a message nobody ever gave a body; here the autosave skips a
record that is still unsaved AND still blank.

**Why the difference shows up here and not in Rails.** Rails' fixture
loader inserts rows with raw SQL and runs no callbacks, so a `Message`
never reads its own `body` during a load. Ours loads THROUGH the model,
so campfire's search-index callback read `body`, the read materialized
an empty rich text, and `after_save` claimed the
`(record_type, record_id, name)` unique key — before the rich-text
fixture, whose thirteen records are every message's actual text, could
insert its own row.

**What it costs.** An app that reads `record.<rich_text>` and then saves,
without ever assigning, gets no row where Rails would leave a blank one.
Nothing can observe the difference through the reader (both answer a
blank content); a direct query for the row can.

### `destroy!` cannot fail

Rails raises `RecordNotDestroyed` when a `before_destroy` callback
throws `:abort`. `destroy!` here is `destroy`.

**Why.** This runtime has no abort channel: `before_destroy` returns
into the void and `destroy` always completes. The bang form is kept as
its own method so the raise has a home when the channel exists, rather
than aliasing the two names together.

**What it costs.** Nothing today — no corpus app halts a destroy from a
callback. An app that did would see the row deleted where Rails would
have raised.

### A recast row is a NEW OBJECT sharing the old row's id

`record.becomes!(Rooms::Open)` in Rails hands the sibling the SAME
attribute hash, so writes through either object are visible in both.
Here (`src/lower/sti_scope.rs`, which unrolls the copy column by
column into the synthesized `becomes_from`) the sibling gets a COPY.

**Why.** Shared mutable attribute state across two objects is the shape
the typed targets have no representation for — each carries its columns
as its own typed slots, not as a hash one can hand around.

**What it costs.** Code that keeps the pre-recast object and writes
through it loses those writes. The Rails idiom reassigns
(`@room = @room.becomes!(Rooms::Closed)`), which is what campfire's two
sites do; a site that kept both handles would diverge silently.

### `errors[:field]` re-derives its field from the message text

Rails' error accumulator keeps an attribute alongside every message, so
`errors[:url]` is an exact lookup. Here the accumulator is a plain
`Array[String]` of FULL messages (`"Url is not public"`), and
`src/lower/errors_index.rs` grounds the read as
`ActiveSupport.errors_for(errors, "Url ")` — a prefix match against the
humanized field name that `errors.add` / `validates` baked at lower
time.

**Why.** Adding an attribute column changes `@errors`' type in every
strict target — `Vec<String>` becomes a vector of pairs, and every
emitted `validate` body, every `errors <<`, and the `full_messages`
identity fold move with it. The projection is recoverable from the text
the runtime already stores, so the type stays.

**What it costs.** Two cases, both named rather than silent:

- One field's humanized name being a PREFIX of another's — `url` and
  `url_host` humanize to `"Url"` and `"Url host"` — makes
  `errors[:url]` also answer `"Url host can't be blank"`, with the
  wrong prefix stripped. No corpus app has such a pair; an app that did
  would diverge silently, and that is the case that would force the
  attribute column.
- `errors[:base]` DECLINES. Rails attaches `:base` messages to the
  record, so `errors_add` bakes them with no prefix and there is no
  text to match. Those sites stay dynamic and join the
  `errors_index` residue ledger.

### A Turbo stream name is not signed

`ActionView::ViewHelpers.turbo_stream_from` writes
`signed-stream-name="<base64-of-JSON>--unsigned"`, and both readers —
`Cable::Connection#decode_stream_name` and
`Turbo::Streams::StreamName.verified` — split on `--` and ignore the
suffix. Rails HMAC-signs the value with `Turbo.signed_stream_verifier`
and refuses a name that does not verify.

**Why.** Signing is not the hard part; agreeing on the key is. Our
`MessageVerifier` derives with 65_536 PBKDF2 iterations where Rails'
class default is 1_000, so a signature minted here would not verify
against a Rails-issued cookie or vice versa — and that question is
already open (see `message_digest.rbs` and spinel#3769). Shipping a
signature that is real but incompatible would be worse than one that is
absent and labelled.

**What it costs.** A stream name is tamperable: a client can subscribe
to any stream whose name it can spell. Signing would close that, and
nothing else — a verified name still carries no expiry and no binding
to a user, which is why campfire guards its room streams with a channel
rather than with the signature. That guard is a separate divergence,
and it is the one below.

Real signing lands in one commit across all three ends —
`turbo_stream_from`, `decode_stream_name`, and
`Turbo::Streams::StreamName` — or not at all: two of them agreeing and
the third not is the failure that looks like it works.

### A cable subscribe is not authorized on the SPINEL lane

The two runtimes have parted company here, so this entry now describes
one of them.

- **CRuby overlay — CLOSED.** `Cable::Connection#handle_message`
  (`runtime/spinel/scaffold/ruby_overlay/cable.rb`) reads the `channel`
  the identifier names, resolves it through
  `ActionCable::Channel::Base::REGISTRY`, and runs the app's own
  `subscribed` on a worker thread holding a database handle. Only what
  that method asked for through `stream_from`/`stream_for` is
  registered, and a `reject` registers nothing. `current_user` comes off
  the identity `Cable.identify` resolved from the handshake.
- **spinel — HALF CLOSED, and the half that remains is the one that
  costs.** The HANDSHAKE is now identified: `Cable.identify`
  (`runtime/spinel/cable.rb`) builds the app's own
  `ApplicationCable::Connection` against an
  `ActionController::CookieJar` over the handshake's `req.cookies`, runs
  its `connect`, and answers a `reject_unauthorized_connection` with
  **401 before `res.start_websocket`** rather than an anonymous socket.
  The identified connection is carried on the per-upgrade
  `Cable::WsMessage` handler, which the driver holds, so it lives
  exactly as long as the connection. The class is reached through a
  generated eager arm (`project::apply_cable_connection`), not
  `const_get` — an app with no `app/channels/` keeps the default arm and
  connects anonymously, because Turbo fan-out predates identity.
  **SUBSCRIBE is still unrouted:** `Cable.handle_message` reads
  `identifier["signed_stream_name"]`, decodes it, and calls
  `Tep::Broadcast.subscribe_ws(stream, ws.fd)` without instantiating a
  channel, so no `subscribed` runs and the `current_user` that now
  EXISTS on the connection is never consulted. **A client that can spell
  a stream name still receives that stream's fan-out** — identity
  without dispatch does not authorize anything by itself.

**Why the split.** The two lanes share `runtime/spinel/turbo_streams.rb`
and the channel classes, but not the transport: the overlay rides Puma's
rack-hijack plus a nio4r reactor, spinel rides tep's fiber-scheduled
server. Dispatch was built on the overlay first on purpose — if the
frames do not match between Rails and the Ruby emit they were never
going to match on spinel, and that is far cheaper to find while
debugging one runtime instead of two (rubys/roundhouse#71).

**What it costs on spinel.** The path implemented is precisely the path
an app's channel guard exists to close. campfire prepends
`RoomStreamsAreAuthorized` onto `Turbo::StreamsChannel`:

```ruby
def subscribed
  if RoomMessagesChannel.guarded_stream?(verified_stream_name_from_params)
    reject                 # ...so the stock channel isn't a way around
```

and its comment states the reason — "authorizing room messages only in
`RoomMessagesChannel` would leave the stock channel as a way around it:
same signed stream name, no membership check." On spinel that prepend is
now EMITTED (the constant exists) and still never reached, because
nothing routes a subscribe frame to a channel.

**The prepend is now IN THE SPINEL BINARY'S LOOKUP CHAIN**, where it used
to be a commented-out line in `boot.rb`: spinel refused
`X.prepend Y` through an explicit receiver, and the class-reopen form its
own diagnostic recommended compiled and did nothing. Fixed upstream in
matz/spinel `a7b6f726`, so `apply_module_mixins` emits the reopen for
that target rather than a comment. It changes no behaviour YET, and that
is the point of saying so here: the guard is installed and unreachable,
because `handle_message` still never builds a channel to run
`subscribed` on. Identity does not move this either — `connect` running
gives the guard a user to test against, and the guard is still not in any
path.

The name is not a secret either: it is a GlobalID
(`GlobalID::Locator.locate gid_param, only: Room`), an identifier rather
than a capability. That is literally true of the names this runtime
mints: a record streamable contributes `GlobalID.param("Room", id)` —
`Base64.urlsafe_encode64` of `gid://<app>/Room/<id>`, no padding — which
is byte-identical to what `to_gid_param` produces in a real Rails
process. It is spelled that way so the app's own channel code can read
it back, the same rule the `/cable` handshake follows by running the
app's `connect`.

**Not a mitigation, but bounds on the blast radius:** fan-out is
in-process and single-worker, and the only frames published are those an
`after_commit` hook records. A subscriber learns nothing about streams no
hook writes to.

**This is not fixed by signing.** Signing decides whether the name was
tampered with; authorization decides whether the named stream may be
joined. campfire's own channel comment makes the point — Turbo's stock
channel "verifies only the signature on the stream name. That name
carries no expiry and no binding to a user." Both ends of that need
closing; **neither lane signs**, and that half is the entry below.

### An open socket outlives the authorization that opened it

`ActionCable.server.remote_connections.where(current_user: user)
.disconnect(reconnect: true)` — campfire's `User#deactivate` and
`#reset_remote_connections` — selects an empty set and returns.

**Why.** Nothing indexes live connections by user. `Cable::Reactor`'s
table is keyed by socket, and the identity a connection carries is read
off it rather than looked up by it. Closing the gap means an index the
reactor maintains on attach and drops on close, plus a posted close per
hit.

**What it costs.** Membership is checked at SUBSCRIBE time, which is
exactly the window campfire's `RoomMessagesChannel` comment calls out:
"revoking a membership disconnects the user with `reconnect: true`, and
the client then replays its subscriptions on the fresh socket." The
replay is now authorized on the CRuby lane — a revoked member's
resubscribe is refused. The disconnect that would FORCE that replay is
what does not happen, so an already-open socket keeps delivering to a
user whose membership was just revoked, until they reconnect for some
other reason.

### Rich text renders EMPTY on a target with no safe-list sanitizer

campfire's message presentation ends in `ContentFilters::SanitizeAttributes`,
which calls `ActionText::ContentHelper.sanitizer.sanitize(html, tags:,
attributes:)`. That reaches `ActionView::ViewHelpers.sanitize_allowing`,
which on the ruby family is the real `rails-html-sanitizer` and on every
other target raises `NotImplementedError` for input containing markup —
the same limit `sanitize` above already carries, for the same reason: the
allow-list is a rule table, and both ways to fake it are wrong in a way
nobody would see.

**What it costs.** campfire wraps its filter chain in its own `rescue
Exception` and returns `""`, so on those targets a message body renders
EMPTY: the record has the text, the database has the text, the page
returns 200, and `<div id="presentation_message_N">` is blank. Nothing is
logged either — the app's own log line runs through a no-op
`Rails.logger`.

**Where it bites.** The spinel campfire binary. The ruby lane is correct:
`scripts/campfire-cable-walk` asserts the posted body arrives in the
broadcast frame, and the room page carries it too.

**How it was found, which is the part worth keeping.** Not here — behind
it. An inherited class-side `new` was binding to the LEXICAL class, so
`ContentFilters::*.apply` built the abstract `ActionText::Content::Filter`
and `applicable?` raised `NotImplementedError`, which the same `rescue
Exception` turned into the same `""`. Every lane rendered empty bodies,
under a green 255/288 suite, until a live `GET /rooms/1`. That one is
fixed (`lower::class_body_new` monomorphizes the method into each
descendant); this is what was standing behind it.

**The fix is the sanitizer**, not this seam: port the safe-list rule
table (42 tags, 13 attributes, per-attribute URL protocols, CSS
behaviour) the way the inflector tables were ported, rather than deriving
one.

### A plain has_many reader answers an Array, not a Relation

`Room#memberships` (`has_many :memberships`) lowers to a reader that runs
the query and hydrates: it answers `Array[Membership]`. `Room#users`
(`has_many :users, through: :memberships`) lowers to an
`ActiveRecord::Relation` over a joins chain. Both are spelled
`owner.name`, and only the second answers the Relation API.

**What still costs.** Every Relation terminal but `pluck` — `count`,
`exists?`, `ids`, `where` — is a `NoMethodError` on a plain has_many
reader, as is any scope (`room.users.without(x)` works only because
`users` is a `:through`). `pluck` is closed, and closed narrowly:
`lower::assoc_pluck` expands `room.memberships.pluck(:user_id)` into
`room.memberships.map { |__pluck| __pluck.user_id }`, which is the
projection it is. That was a 500 on every `POST /rooms/:id/messages` in
a served app — the broadcast still went out, because `after_create_commit`
runs before the line, so the message reached subscribers and the request
that made it then failed. The suite never saw it: under the test adapter
the job that reaches the line is enqueued rather than run.

**What the projection costs.** The reader hydrates every row, which is
what it ALREADY does — the projection is the only new work. Rails reads
one column. So this is a fix for the crash, not for the query.

**What would close the rest** is the association proxy: an association
reader that IS a chain root, so `room.memberships.pluck(:user_id)` folds
to `SELECT user_id FROM memberships WHERE room_id = ?` and every other
terminal follows. That is designed and not built. `lower::assoc_pluck`
is deliberately not a down payment on it — a half-built chain root that
handles one terminal is worse than none, because the next terminal to
arrive looks supported until it is not.

**A note on what the type says.** `Ty::Array` does NOT distinguish the
two: the analyzer types a `:through` reader `Array[Room]` as well, its
approximation of "a collection". The association KIND is what determines
which reader gets emitted, and it is what the pass reads.

### `config.x.<key> = <expr>` is re-evaluated on every read

`config.x.web_push_pool = WebPush::Pool.new(...)` in an initializer is an
ASSIGNMENT: Rails evaluates the right-hand side once at boot and every
`Rails.configuration.x.web_push_pool` reads that one object. Ingest lifts
it to a reader on the Application reopen:

```ruby
def x_web_push_pool
  WebPush::Pool.new(invalid_subscription_handler: ->(id) { ... })
end
```

which re-runs the expression on every read.

**What it costs.** For a literal (`config.x.vapid.public_key = "…"`, the
common case) nothing at all. For a constructor it is a new object per
read, and campfire's is the bad kind: `WebPush::Pool.new` builds a
50-thread executor, a 1-thread pool and a 150-connection HTTP pool, and
the `shutdown` the app's `at_exit` calls reaches only the last one. One
per message created.

**Not the reason campfire's push path fails today** — that is
`uninitialized constant Net::HTTP::Persistent`, which the pool's
constructor hits first. Memoizing the reader would not fix that; it is a
separate entry because it would still be wrong once the constant exists.

### An initializer's `prepend` is not performed on the spinel target

`config/initializers/turbo_streams_authorization.rb` — campfire's
`Turbo::StreamsChannel.prepend RoomStreamsAreAuthorized` — is emitted as
a live line at the end of the ruby family's `boot.rb` and as a COMMENT in
the spinel tree's.

**Why.** Spinel refuses the explicit-receiver form outright: "the class
graph, ancestor chain, and method/ivar layout are baked at compile time,
so a class cannot be restructured through an explicit receiver." Its
diagnostic recommends moving the call inside a `class X ... end` reopen,
and **that form compiles and does nothing** — a prepended `hello` calling
`super` prints `guarded hi` under CRuby and `hi` from the binary, with no
warning. Emitting it would put the guard back in the tree, tested, and
out of the lookup chain, which is the failure `lower::module_mixins`
exists to prevent, minus the report. Filed as matz/spinel#4200.

A `prepend` inside the class's ORIGINAL body works on spinel; only a
reopen is silent. That is no help here — the target class is turbo-rails'.

**What it costs today: nothing, and that is a fact about a second gap
rather than a defence.** The spinel lane does not dispatch a subscribe
frame to a channel at all (see above), so `Turbo::StreamsChannel
#subscribed` never runs and a guard prepended onto it would not run
either. The two have to close in that order.

**Where it is visible.** The emitted `boot.rb` carries the commented
line and the reason, so the absence names itself in the file someone
would read.

### A cable stream name is not signed, on either lane

`turbo_stream_from` writes `<base64-of-JSON>--unsigned` and
`Turbo::Streams::StreamName.verified` reads it back by splitting on
`--` and ignoring the suffix. Rails HMAC-signs the value.

**What it costs.** A client can subscribe to any stream it can SPELL,
and the names are guessable — they are GlobalIDs. On the CRuby lane the
app's own channel guard is what stands between a spelled name and its
fan-out (campfire's `RoomMessagesChannel` re-derives the room from the
name and asks `user.rooms.find_by`), which is a real check but an
app-supplied one: an app that guards nothing is wide open. On spinel
nothing stands there at all.

Real HMAC signing belongs in `runtime/spinel/turbo_streams.rb`, with
`turbo_stream_from` and `verified` changed in the same commit, once the
key-derivation question (`message_verifier.rb`'s iteration count vs
Rails') is settled.

### `strip_tags` leaves entity references alone

`ActionView::ViewHelpers.strip_tags` parses the HTML and serializes the
text, matching `Rails::HTML5::FullSanitizer` on 24 of 25 measured
probes — including the ones a regex gets wrong (`"a < b"` →
`"a &lt; b"`, a `>` inside a quoted attribute value, an unterminated
tag swallowing the rest, the CONTENT of a `<script>` surviving).

**Why.** The 25th is decoding: Rails turns `&eacute;` into `é`, which
needs HTML5's 2231-entry named-entity table. A well-formed reference
(`&name;`, `&#123;`, `&#xAB;`) passes through unchanged here instead,
and a bare `&` still escapes to `&amp;`.

**What it costs.** Nothing that renders: the two agree byte for byte on
every reference that round-trips (`&amp;`, `&lt;`, `&nbsp;`) and a
browser draws `&eacute;` and `é` the same. They part company only on
malformed input, where HTML5's legacy no-semicolon matching applies —
`&notanentity;` is `¬anentity;` to Rails and unchanged here.

`sanitize` is a separate matter and is NOT a divergence: the safe-list
sanitizer is unimplemented and raises on input containing markup,
serving only the tagless case that `sanitize(strip_tags(x))` produces.
That is a gap, and it names itself when reached.

### `strip_tags` drops `<script>` TEXT on JRuby and keeps it on CRuby

`ActionView::ViewHelpers.strip_tags("<b>Hi</b> &amp; <script>bad()
</script>there")` is `"Hi &amp; bad()there"` on the CRuby tree and
`"Hi &amp; there"` on the JRuby one. Same for `<style>`.

**Why.** Both trees serve `strip_tags` from the real
`rails-html-sanitizer`, and the gem's `best_supported_vendor` answers
`Rails::HTML5::Sanitizer` only where `Loofah.html5_support?` — which
needs an HTML5 parser in Nokogiri, and JRuby has none. So JRuby gets
`Rails::HTML4::Sanitizer`, whose full sanitizer removes the CONTENT of
`script` and `style` where the HTML5 one removes only the tags.

**Where it is visible.** This one probe and its `<style>` twin. Every
other sanitize / strip_tags / auto_link behaviour in the corpus was
checked against both vendors side by side and agrees, so the divergence
is exactly "the text inside a script or style element". It is Rails'
own difference, not ours: a Rails app on JRuby answers the same way.

**Where it is pinned.** `tests/overlay_sanitize_autolink.rb` asserts
BOTH values, branching on the vendor, so a runner whose nokogiri lacks
HTML5 reads as the other correct answer rather than as a regression.

### `auto_link` does NOT sanitize the body; Rails does

`ActionView::ViewHelpers.auto_link` on every target except the CRuby
overlay's finds and wraps the links exactly as `rails_autolink` does,
and hands back the body around them AS GIVEN. Rails runs the body
through the safe-list sanitizer first.

```text
input   a < b > c
Rails   a &lt; b &gt; c
here    a < b > c

input   addr <foo@bar.com> ok
Rails   addr  ok                      (the unknown tag is dropped)
here    addr <<a href="mailto:foo@bar.com">foo@bar.com</a>> ok

input   <a href='x'>t</a>
Rails   <a href="x">t</a>             (attributes renormalised)
here    <a href='x'>t</a>
```

**Why.** The safe-list pass is HTML5 tree construction, not filtering —
the argument is in the header of
`ruby_overlay/runtime/action_view_sanitize.rb` and is the same reason
the shared `sanitize` REFUSES markup instead of approximating it. That
refusal is the honest answer where the caller can be told; `auto_link`
is on campfire's read path, under a `rescue Exception` that returns
`""`, so raising there is a blank message body rather than an error.
Linking without the pass is the only remaining option, and it is stated
here rather than discovered.

**The size of it, measured.** Against `rails_autolink` 1.1.8 on
`actionview` 8.1.3, over 36 probes:

* **36 / 36** byte-identical to `auto_link(..., :sanitize => false)` —
  the gem minus this pass. Every linking decision is the gem's: the
  scheme list, where a URL ends, which trailing punctuation is the
  sentence's, the bracket rule, the e-mail local part, and both clauses
  of `auto_linked?`.
* **30 / 36** identical to the gem's default. All six differences are
  the three shapes above — escaped angle brackets, a dropped tag,
  renormalised quotes. Not one is a different link.

**What it costs, and what it does not.** The links this helper CREATES
are still safe by the gem's own rule table: the scheme list has no
`javascript:` in it and the `www.` branch is prefixed `http://`, so
`auto_link` cannot manufacture a scripting URL out of text. What is
lost is Rails' SECOND layer over markup that was ALREADY in the body.
campfire's is ActionText content that arrived through `h`, so the first
layer is the one doing the work — but an app that feeds `auto_link` raw
user HTML and leans on this pass to clean it gets no cleaning here.

One consequence follows from the same skip: Rails' body pass turns a
bare `&` into `&amp;` before the URL regex ever runs, so the gem's href
never carries one. Here it does — `https://x.co/a?b=1&c=2` reaches the
attribute as written. campfire's body arrives through `h`, so its `&`
is already an entity.

**A second, unrelated to sanitizing.** The gem strips trailing
`\p{Word}` — Unicode letters, marks, numbers and connector punctuation.
The port spells ASCII out and TAKES everything above it as a word
character. A URL ending in a non-ASCII letter agrees; one ending in
non-ASCII PUNCTUATION (`»`, `。`) keeps the character here and drops it
there.

**Where it is pinned.** `tests/shared_autolink.rb`, which asserts the
30 agreements AND the three divergent shapes, so a future change that
starts sanitizing says so in that file rather than in the campfire
suite. The CRuby overlay is unaffected: it redefines `auto_link` on the
real gem chain and is gated separately by
`tests/overlay_sanitize_autolink.rb`.

### `link_to` / `mail_to` put `href` FIRST; Rails puts it LAST

`ActionView::ViewHelpers.link_to("t", "/u", target: "_blank")` renders
`<a href="/u" target="_blank">`. Rails renders
`<a target="_blank" href="/u">` — measured against ActionView 8.2 for
both helpers, and `mail_to` and `link_to_raw` share the shape.

**Why.** Every one of these builds its attributes as
`{ href: href }.merge(opts.to_h)`, so the default lands ahead of the
caller's; Rails merges the other way round.

**Where it is visible.** ATTRIBUTE ORDER only — never in which
attributes, their values, or the element. `scripts/compare` is a DOM
comparison (see the note on its own output), so it cannot see this, and
the campfire tag tallies cannot either. It surfaced from the other
direction: the `auto_link` port in
`ruby_overlay/runtime/action_view_sanitize.rb` agrees with the real
`rails_autolink` gem on 13 of 14 probes, and the fourteenth is an email
address, where the anchor goes through `mail_to` and comes back with its
attributes transposed.

**Why it is still here, and what it would cost.** The fix is one `merge`
reversed in three helpers. The blast radius is MEASURED, not assumed:
**20 call sites total** — 6 in the campfire emit, 14 in lobsters, and
ZERO in the blog, because the view walker inlines most anchors as
literal strings and only reaches these helpers when the URL or the
attributes are dynamic. Every one of the 20 passes attributes, so every
one moves.

That is small enough to do, and it was left undone only because it
arrived at the tail of a session that had already changed the escape
surface twice. Do it with the golden dumps regenerated in the same
commit, and check `compare-*` on every target rather than assuming a
DOM comparison cannot see it.

## Related docs

- [`emit.md`](emit.md) — the universal IR contract; the consumers of
  the runtime.
- [`analyze.md`](analyze.md) — RBS-paired typing of `runtime/ruby/`.
- [`verification.md`](verification.md) — toolchain tests that
  exercise runtime + emitted project end-to-end.

### A `_path` helper turns `host:` into a query parameter

`RouteHelpers.room_at_message_path(1, 5, host: "once.campfire.test")`
emits `/rooms/1/@5?host=once.campfire.test`. Rails emits `/rooms/1/@5`.

**Why.** `:host` is a URL-GENERATION option, not a route segment and not
a query parameter. actionpack 8.1.1,
`ActionDispatch::Routing::RouteSet`:

```ruby
RESERVED_OPTIONS = [:host, :protocol, :port, :subdomain, :domain,
                    :tld_length, :trailing_slash, :anchor, :params,
                    :only_path, :script_name, :original_script_name]
```

`path_for` passes that list as `reserved`, so none of those names reaches
the query string. A `_path` helper drops `:host` entirely (`full_url_for`
is the only consumer); `:anchor` becomes `#frag`; `:params` becomes the
query. Our synthesis knows none of this and treats every leftover kwarg
as a query parameter.

**What it costs.** campfire's
`MessagesControllerTest#test_creating_a_message_broadcasts_the_message_to_the_room`
builds the expected copy-link URL with
`room_at_message_path(@room.id, Message.last.id, host: "once.campfire.test")`
and compares it to the rendered `data-copy-to-clipboard-content-value`.
Both sides are ours, so they would agree if the view rendered the same
wrong URL — the test fails because only the TEST passes `host:`. A page
that renders one of these is serving a URL with a bogus query on it.

**The fix is a rule table, not a special case.** Port `RESERVED_OPTIONS`
and give the four that MEAN something (`anchor`, `params`,
`trailing_slash`, and `host`/`protocol`/`port` for the `_url` family)
their actual behaviour, rather than dropping `host` alone and leaving
`anchor:` to become `?anchor=`.

### `ActionView::RecordIdentifier` is ruby-family only

Rails defines `dom_id` on `ActionView::RecordIdentifier` and includes
that module into its helpers. This runtime defines it on
`ActionView::ViewHelpers` and offers `RecordIdentifier.dom_id` as a
delegate — but only on the ruby family, from
`ruby_overlay/runtime/action_view_record_identifier.rb`.

**Why not `runtime/ruby/`, beside the function it delegates to.** That
directory prices all nine targets, and the ones with no module system
flatten `ActionView`'s modules into a single namespace. Kotlin emitted
both `domId`s into one `ViewHelpers.kt` and refused to build —
"Conflicting overloads", then "Overload resolution ambiguity" at the
call site. A delegate is exactly the shape that cannot survive
flattening: same name, same arity, same namespace.

**What it costs.** A strict target that meets
`ActionView::RecordIdentifier` gets an uninitialized constant. Nothing
does — the one caller is an app test helper, and those run on the ruby
family. A target that needs it wants the module split, not this file
copied.

### `assert_select`'s block does not scope its nested assertions

Rails runs an `assert_select` block against the MATCHED ELEMENTS: a
nested `assert_select` inside searches only what the outer one selected.
Ours yields with no scoping, so a nested assertion searches the last
response body again.

**What it costs, and the direction is the bad one.** A nested assertion
can PASS against markup the outer selector never matched. campfire's
broadcast test is the shape: the outer `assert_select` is scoped to a
Nokogiri fragment built from the pubsub queue, and the assertions inside
it look at the POST response instead. Both happen to contain the
message, so the inner assertion is answering about the wrong document
and agreeing anyway.

**Not fixed here.** Scoping means the block's assertions run against a
node set rather than a body, which is a change to every `assert_select`
call site's plumbing rather than to this one method.

### `try` narrows to the classes that answer, and cannot see every one

`recv.try(:name)` is Rails' `respond_to?(name) && public_send(name, …)`
— a DEFINEDNESS guard. It used to be grounded at ingest to
`recv && recv.name`, the `&.` desugar, which is a NILNESS guard: the two
agree whenever the receiver either is nil or does respond, and diverge on
the one case between. That cost campfire's own
`MessagesControllerTest#test_creating_a_message_broadcasts_the_message_to_the_room`,
where `streamble.try(:to_gid_param) || streamble` over `[room, :messages]`
raised on the Symbol instead of taking the fallback.

`lower::try_guard` now asks the TREE which classes answer the name and
emits a narrowing over the fewest `is_a?` tests that cover them:

```ruby
(s.to_gid_param if s.is_a?(ApplicationRecord) ||
                   s.is_a?(Opengraph::Location) ||
                   s.is_a?(Opengraph::Metadata)) || s
```

`nil.is_a?(X)` is false, so the narrowing does everything the nil guard
did and answers nil — rather than raising — for the non-nil receiver
that does not respond.

**WHAT IT STILL CANNOT SEE, and this is the divergence that remains.**
The pass reads methods DECLARED in the tree plus the short list the
pipeline synthesizes on every model. It does not see:

* **runtime methods** — `to_param`, `strip`, `id`. A `try(:strip)`
  therefore keeps the nil guard, because folding to nil would be wrong
  for exactly the names the runtime supplies.
* **column accessors**, which are synthesized from the schema after this
  pass runs. lobsters' 31 `try` sites are all of this shape
  (`user.try(:username)`), so they keep the nil guard too — correct
  there, since the receiver is a nilable `User` that does define it, but
  correct by accident rather than by decision.

So the divergence is narrower than it was and has not gone: a non-nil
receiver that does not answer a RUNTIME-supplied or COLUMN-backed name
still raises where Rails answers nil. Closing it means giving the pass
the schema and the runtime surface, both of which exist and neither of
which is wired to it.

**The earlier proposal, and why this is not it.** This entry used to say:
fold to nil when analysis knows the receiver's type has no such method,
"leaving the untyped-receiver case as it is today". The untyped receiver
IS the failing case — campfire's site is a block parameter over a mixed
array — so that plan would have closed nothing. The defining set is
knowable where the receiver's type is not.

### A conditional as a boolean operand needs its parens

`lower::try_guard` emits `x.m if cond`, and a modifier-`if` binds looser
than every boolean operator. Rendered bare as the left operand of `||`,
`x.m if cond || fallback` re-parses with the fallback INSIDE the
condition — the expression answers nil and the fallback never runs. The
Ruby emitter parenthesizes an `If` / `Case` / `RescueModifier` operand
for that reason, the same call `recv_needs_parens` already made for a
receiver.

Worth knowing because it is invisible in review: the emitted line is
valid Ruby either way, and only the parse changes.

### Four of Rails' reserved URL options are still query params

`url_for` splits a route helper's option hash in two: the twelve names in
`ActionDispatch::Routing::RouteSet::RESERVED_OPTIONS` (actionpack 8.1.3,
`route_set.rb:838`) it consumes itself, and everything else, which it
forwards to the path generator and which ends up in the query string. We
forwarded all twelve, so `room_at_message_path(1, 5, host: "x")` rendered
`/rooms/1/@5?host=x` where Rails renders `/rooms/1/@5` — and that extra
`?host=` was what failed campfire's own broadcast assertion.

`lower::route_url_options` now models the split as a table, and the table
has three filled cells and one empty one:

* **the seven host-only names** — `host`, `protocol`, `port`,
  `subdomain`, `domain`, `tld_length`, `only_path` — are dropped from a
  `_path` call site. `path_for` is `url_for(…, PATH, …)`, and the `PATH`
  strategy never calls `build_host_url`, so these contribute nothing to
  a path. Dropping them is EXACT, not an approximation.

  On the `_url` spelling they are not dropped, because there they are
  the answer: `x_url(…, host: h)` becomes
  `"http://#{h}#{RouteHelpers.x_path(…)}"` — the same shape the view
  lowerer grounds a hostless `_url` with (`Rails.application.domain` in
  place of `h`) and the same one
  `emit::ruby::library::rewrite_url_helpers_absolute` builds for the
  explicit `…routes.url_helpers.x_url(…, host:)` chain. `protocol:`
  replaces the `http` and rides bare (`"https"`, not `"https://"`),
  which is that older pass's convention; Rails' `normalize_protocol`
  accepts both spellings and we accept only the first.
  `x_url(…, only_path: true)` is Rails asking the URL spelling for a
  path, and gets one.
* **`anchor:`** is rendered, `#tag`, after the query string — the order
  `path_for` applies `add_params` and then `add_anchor` in.
* **`format:`** is `lower::route_format_suffix`'s, which monomorphizes
  the helper rather than widening its signature.
* **`script_name:`, `original_script_name:`, `trailing_slash:` and
  `params:` are NOT modeled.** Each genuinely changes the path — the
  first two prefix it, the third appends a `/`, and `params:` is a hash
  Rails merges into the query — and each is still treated as an ordinary
  query key, so `foo_path(trailing_slash: true)` renders
  `?trailing_slash=true`. Left visibly wrong rather than silently
  dropped: no corpus app writes one of them on an app route (campfire's
  one `params:` site is an integration-test POST), and a dropped option
  is a URL that looks right and is not.

A second, smaller divergence in the same place: Rails escapes a fragment
with `Journey::Router::Utils.escape_fragment`, which leaves `/`, `?` and
`:` alone. The generated helper reuses the `url_encode` its query keys
use, which percent-encodes them. Every anchor the corpus writes is a
slug, a tag or a `dom_id`, so nothing can tell the difference today.

### `ActionText::ContentHelper.allowed_attributes` answers the list, not `nil`

Rails declares it `mattr_accessor(:allowed_attributes)` with no default
(actiontext 8.1.3, `app/helpers/action_text/content_helper.rb:11`), so
in an app that never configured it the reader answers `nil` and every
caller falls through its own `||`. Ours answers the list that fallback
computes — the sanitizer's own set plus `ActionText::Attachment::
ATTRIBUTES`, which is exactly what Rails' `sanitizer_allowed_attributes`
builds from the same two pieces.

**Why, and it is a type problem rather than a behaviour one.** campfire's
`ContentFilters::SanitizeAttributes` copies Rails' expression verbatim:

```ruby
ActionText::ContentHelper.allowed_attributes ||
  (sanitizer_class.allowed_attributes + ActionText::Attachment::ATTRIBUTES).to_a
```

`sanitizer_class` is `ActionText::ContentHelper.sanitizer.class` — a
CLASS OBJECT. Neither we nor spinel can dispatch statically on one: our
`Ty` has no singleton variant (`analyze::body::send` collapses
`instance.class` onto the instance's own `Ty::Class`), and spinel's
`sp_Class` is a dynamic receiver. So the right arm is `Array[untyped]`
whatever the left says, and the result was handed twelve lines later to
a `sanitize` this runtime declares takes `Array[String]`. One list,
described two contradicting ways — and on spinel the contradiction
surfaced as far from its cause as it could get:

```
sanitize_attributes.rb:13: error: incompatible pointer types passing
  'sp_PolyArray *' to parameter of type 'sp_StrArray *'
```

from the C compiler, with the campfire binary failing to LINK.

Typing the LEFT arm closes it, because `||` now folds to a left that
cannot be falsy (Ruby's falsy set is `nil` and `false` and nothing else,
so it reads off the type) instead of unioning with the right. The values
are identical either way: campfire computes the same list from the same
two pieces. Only an app that asks whether the reader is `nil` can tell
the difference, and none does.

**What is still not modeled:** a method that returns a class object is
declared by its INSTANCE type, because that is the only type we have.
`def sanitizer_class: () -> Class` is what gets emitted — honest, and
what spinel's `sp_Class` wants — but a chain through it stays dynamic.
Closing that means a singleton variant in `Ty`, which every target's
exhaustive `match` would have to answer for.
