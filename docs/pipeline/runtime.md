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

### An absent cookie reads as `""`, not `nil`

`cookies[:missing]` and `cookies.signed[:missing]` both answer `""`.
Rails answers `nil`. The signed jar answers `""` for *anything that does
not verify* too — a tampered payload, a bad signature, a value signed
for a different cookie name — which is the same thing Rails does
semantically (it answers `nil` and the app reads as signed out), just
spelled with a different empty.

**Why.** The store is `Hash[String, String]`, and a nullable String puts
every read on spinel's nullable path: `cookies[k].to_s.split(",")`
yields a null array there, which is how lobsters'
`remove_unknown_cookies` first met this. Every call site in the corpus
coerces with `.to_s`, under which `""` and `nil` are identical.

**What depends on it.** `raw` returns `""` as its final fallback, and
`SignedCookieJar#[]` returns `""` on every verification failure. `delete`
records a cleared write as `""` rather than a tombstone, so `@out` stays
a plain String→String map — the harness and both dispatchers read that
empty as "expire this cookie".

**Where it is visible.** Two shapes, both real:

- `if token = cookies.signed[:session_token]` is truthy when signed out,
  because `""` is truthy in Ruby. campfire's `SessionLookup` has exactly
  this, and it still behaves correctly — the branch runs
  `Session.find_by(token: "")`, which finds nothing — but it costs a
  query Rails would not make, and a call site that *only* checked
  presence would be wrong.
- `assert_nil cookies.signed[:session_token]` fails where Rails passes.
  campfire's `sessions_controller_test` asserts this after a failed
  sign-in.

Closing it means making the signed read nullable and auditing every
`.to_s` coercion above; the truthiness shape is the one that would
justify it.

### An enum attribute reader yields the STORED value

`user.status` answers `0` where Rails answers `"active"`. The generated
predicates and scopes carry the stored value too, which is what makes
them correct with no enum type at runtime. Fixing the reader means
mapping at every read; do it only if an app is found that reads the raw
attribute.

### Action Text resolves no attachment

`ActionText::Content#attachables` answers `[]` where Rails answers the
records the fragment's `<action-text-attachment sgid="…">` nodes point
at.

**Why.** An `sgid` is a *signed* GlobalID. Turning one back into a
record needs SignedGlobalID verification against the app's secret,
a GlobalID URI parse, and a class registry to look the model up in —
three pieces that do not exist yet, and none of which the HTML scanner
can approximate.

**What still works.** The PARSE is complete: `#attachments` returns
every node with every attribute it carried (`sgid`, `content_type`,
`caption`, `filename`, `url`), and `to_plain_text` renders an
attachment as its caption or filename exactly as Rails does. The
boundary is dereferencing, not reading.

**Where it is visible.** Code that greps attachables for a model —
mention extraction is the canonical shape (campfire's
`Message::Mentionee` does `body.body.attachables.grep(User)`) — sees
no mentions rather than wrong ones. `RichText#to_trix_html` likewise
hands back the stored markup instead of rendering attachment previews
into it, so an editor loads the text and shows attachment nodes bare.

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

### A rich-text preload scope is the identity

`with_rich_text_<attr>` and `with_rich_text_<attr>_and_embeds` return
the relation unchanged where Rails adds an `includes`.

**Why.** The synthesized reader fetches per record, so there is no
preloaded association for the hint to attach to.

**Where it is visible.** Query COUNT, not query results: a page
rendering N records issues N rich-text queries where Rails issues one.
The methods exist rather than being dropped so that call sites chaining
through them keep working.

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

**What it costs.** Both are a 404 through the dispatcher, so a plain
app sees the same response. A controller that rescues the signature
error BY NAME does not catch this one — campfire's
`Users::AvatarsController` has exactly that
`rescue_from(ActiveSupport::MessageVerifier::InvalidSignature)`, and it
would go unfired.

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

### ActiveJob's test helpers count jobs that RAN

The adapter is inline (`lower::job_class_side` makes `perform_later`
run the job), so there is no queue to inspect — the same reason Rails'
own `:inline` adapter does not work with these helpers.
`ActiveJob::PERFORMED` is the seam instead, appended by the
`perform_later` wrapper.

**Why it is equivalent here.** "Enqueued during this block" and "ran
during this block" are the same set under inline dispatch. The shape
that separates them — a job enqueued and never run — is one inline
semantics cannot produce.

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

**What it costs.** `relation.new` reaches no method and raises. The
association form is already served without it — `lower::scope_chain`
rewrites `new` inside an association-scoped class method to
`new(__rel.scope_attributes)`, which is where `user.sessions.create!`
gets its `user_id`. The gap is the SCOPE form, and closing it means
another call-site rewrite in that same pass rather than a runtime
method.

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

## Related docs

- [`emit.md`](emit.md) — the universal IR contract; the consumers of
  the runtime.
- [`analyze.md`](analyze.md) — RBS-paired typing of `runtime/ruby/`.
- [`verification.md`](verification.md) — toolchain tests that
  exercise runtime + emitted project end-to-end.
