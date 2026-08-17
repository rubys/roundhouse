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

Conventional file roles (file names vary by target — see inventory below):

| Role | Description |
|------|-------------|
| Model base / shims | `ActiveRecordAdapter` trait, validation error type, framework-test adapter |
| DB connection | Lifecycle (open, with_conn borrow, test-mode in-memory) |
| HTTP server | Production HTTP entry — listens on a port, dispatches through Router |
| Action Cable | WebSocket endpoint |
| View helpers | Delegates into transpiled framework Ruby (where present) or implements helpers directly (legacy) |
| Test support | TestClient + TestResponse + Rails-shaped assertions |

The shape varies per target — newer targets (Rust, Crystal) carry
adapter + framework-test scaffolding the older ones don't yet. The
current inventory:

| Target | Files |
|--------|-------|
| `runtime/rust/` | `active_record_adapter.rs`, `cable.rs`, `db.rs`, `errors_ext.rs`, `flash.rs`, `framework_test_adapter.rs`, `hash_ext.rs`, `http.rs`, `param_value.rs`, `runtime.rs`, `server.rs`, `session.rs`, `test_support.rs`, `view_helpers.rs` |
| `runtime/crystal/` | `broadcasts.cr`, `cable.cr`, `db.cr`, `framework_test_adapter.cr`, `http.cr`, `param_value.cr`, `server.cr`, `test_helper.cr`, `test_support.cr` |
| `runtime/typescript/` | DB / server / Cable primitives (`db.ts`, `db-libsql.ts`, `db_worker.ts`, `server.ts`, `server-libsql.ts`, `server-worker.ts`, `broadcasts.ts`, `client.ts`, `param_value.ts`, `sqlite_wasm_engine.ts`), the worker bridge (`juntos*.ts`), and async/sync minitest adapters (`minitest.ts`, `minitest-async.ts`). Framework-runtime files (`active_record_base.ts`, `action_controller_base.ts`, `inflector.ts`, `json_builder.ts`, …) are emitter-generated from `runtime/ruby/` and appear under `src/` in emitted projects, not in this directory. |
| `runtime/go/`, `runtime/python/`, `runtime/elixir/` | Conventional 7-file primitive set (`cable`, `db`, `http`, `runtime`, `server`, `test_support`, `view_helpers`) |
| `runtime/kotlin/`, `runtime/swift/`, `runtime/csharp/` | Newer-target primitives (per-target file set) |
| `runtime/spinel/` | Per-target primitives for the Ruby/Spinel target (`base64.rb`, `broadcasts.rb`, `cgi_io.rb`, `db.rb`, `db_cruby.rb`, `importmap.rb`, `json.rb`, `sqlite_adapter.rb`) plus a `scaffold/` tree (Gemfile, inner Makefile, main.rb, Tailwind config) overlaid into every emitted Ruby/Spinel project, and a `test/` tree of target-specific test files |

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
```

`action_text.rb` holds only Action Text's VALUE layer.
`ActionText::RichText` is absent because it has a table: it is
synthesized as an ordinary model by `lower::rich_text` and reaches
every target through the model machinery. That split — table-backed
things are models, values are framework Ruby — is the general rule,
not an Action Text special case.

Each `.rb` ships with a `.rbs` sidecar declaring the public typed
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

Framework runtime files (TS, eventually all targets) ship via the
same emit pipeline that compiles user apps — `runtime/ruby/active_record/`
is ingested with its RBS sidecar (`src/runtime_src.rs`), lowered, and
emitted into the generated project as `src/active_record_base.ts`
(etc.) by the same code path that compiles user controllers and
models. There is no separate `bin/build-runtime` binary; emission
runs inline as part of `cargo run --bin roundhouse -- --target <t>`
(or `--site` for the full archive matrix).

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
| `runtime/{go,python,elixir,kotlin,swift,csharp,spinel}/` | Sibling targets, partial |
| `src/emit/<target>.rs` | Emitter side that reads + embeds the runtime |
| `src/runtime_src.rs` | Framework-Ruby ingestion + transpile pipeline |

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
all thirteen targets. Foreign keys follow the same convention — the
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
to them.

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

## Related docs

- [`emit.md`](emit.md) — the universal IR contract; the consumers of
  the runtime.
- [`analyze.md`](analyze.md) — RBS-paired typing of `runtime/ruby/`.
- [`verification.md`](verification.md) — toolchain tests that
  exercise runtime + emitted project end-to-end.
