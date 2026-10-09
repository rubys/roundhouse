# Schema, routes, seeds, and importmap

A family of files under a Rails app are not treated as general code —
they're recognized as declarative inputs and ingested into dedicated
IR structures. The four covered in depth here are `db/schema.rb`,
`config/routes.rb`, `db/seeds.rb`, and `config/importmap.rb`; the
same family also takes in test fixtures (below), `db/migrate/`
migrations (below), `config/routes/` split files, and `sig/**/*.rbs`
RBS sidecars (`App::rbs_signatures`, parsed by `src/rbs.rs`). This
doc covers what each one contributes, the IR shape it produces, and
the `*_to_library` lowering pass that turns each one into a
`LibraryFunction` (or `LibraryClass` for fixtures) for the universal
post-lowering IR that emitters consume.

The pattern is consistent throughout:

```
file → dedicated IR (App::<field>) → *_to_library → LibraryFunction → emit
```

The lowered shape is the designed contract, and the preferred path is
for emitters to consume it rather than the source IR (`Schema`,
`RouteTable`, `Importmap`, `App::seeds`). Direct source-IR reads do
survive in emitters (e.g. `src/emit/typescript.rs` reads `app.schema`
directly; `src/emit/roda.rs` writes `db/migrate` files from
`Schema`) — those are candidates to migrate, not a second contract.
See [`../pipeline/lower.md`](../pipeline/lower.md) for the two-shape
contract.

## `db/schema.rb` → `Schema` → `Schema.statements`

**Source IR:** `src/schema.rs::Schema` — an `IndexMap<Symbol, Table>`.
Each `Table` carries its columns (typed via `ColumnType`), indexes,
and foreign-key declarations. Iteration order is source order, so
downstream consumers (schema DDL lowering, persistence lowering, model
attribute seeding) produce deterministic output.

**Ingest:** `src/ingest/schema.rs::ingest_schema`. Recognizes the
`ActiveRecord::Schema[…].define do` DSL: `create_table`, `t.string`,
`t.integer`, `t.references`, `t.timestamps`, `add_index`,
`add_foreign_key`, etc.

**Downstream consumers (analyze/lower):**

- **Analyzer** seeds each model's `attributes` row from its matching
  table — this is how `article.title : String` gets its type without
  any annotation in the model file.
- **`src/emit/shared/schema_sql.rs::render_schema_statements`**
  produces the `CREATE TABLE …` DDL statement list in the SQLite
  dialect, which every caller uses today (the joined-string
  `render_schema_sql` survives for some targets);
  `render_schema_statements_for(schema, Dialect::Postgres)` renders
  the same schema for Postgres (see the shape limits below). The
  sibling `src/emit/shared/seed_sql.rs` renders `db/seeds.rb` data to
  a `db/seed.sql` for text-only archives.
- **`src/lower/persistence.rs`** uses the column list to build
  INSERT / UPDATE / DELETE / SELECT strings per model.

**Lowered to LibraryFunction:** `src/lower/schema_to_library/`
produces a single `LibraryFunction` — `Schema.statements() ->
Array<Str>` under `module_path: ["Schema"]`. The body is an array
literal of the rendered DDL statements, one `Lit::Str` each. Empty
when `schema.tables` is empty (apps without persisted models don't
need a `Schema` artifact).

**Per-target emit:** TS writes `src/schema.ts` exporting
`statements`. `main.ts` passes `schemaStatements:
Schema.statements()` to the runtime's `startServer({ … })`.

**Known shape limits.** Every emitter that renders this DDL uses the
SQLite dialect. Per-engine DDL sits behind the `Dialect` enum in
`schema_sql.rs`, without changing the `Schema` IR itself (it's already
dialect-neutral) or the lowerer. `Dialect::Postgres` (stage (a) of
#91; no target emits it yet) spells column types as Rails' PostgreSQL
adapter creates them, quotes every identifier, and gives a key Rails'
default convention for its type: `bigserial`, `serial` for an
`integer` key, and `uuid … DEFAULT gen_random_uuid()` for a `uuid`
one. Neither dialect reproduces source column defaults, foreign keys
or CHECK constraints: Postgres synthesizes the key defaults above, so
a custom or suppressed one is not reproduced, and the model layer
applies supported literal defaults. A virtual table has no Postgres
DDL, so that dialect returns an error for it. Postgres renders what
ingest kept, so it shares the current ingest and IR limits.
`schema.rb` ingest drops `array: true`; an index's `order:` and
`opclass:`, and expression indexes; precision on `numeric`,
`datetime` and `time`; and schema qualifiers. An `integer` `limit:` of 1
or 2 still normalizes to `ColumnType::Integer` for ordinary typing, but
its original smallint width is retained as negative evidence for generated
int4 results; 3 and 4 are exact PostgreSQL `integer`, and 5 to 8 normalize
to `bigint`, as in Rails. The key forms the
PostgreSQL dumper writes are read as the keys they name: `id: :serial`
is an `integer` key, and a hash-valued `id: { type: :string, limit:
32 }` keeps its type and limit. And the folds below
apply (`timestamptz` renders `timestamp`). The IR preserves `json` and
`jsonb` separately so PostgreSQL DDL keeps the source type. SQLite stores both
as text, and both use the same Ruby `JsonColumn` encoding/decoding path; this
does not add PostgreSQL runtime support.
Roda's current Sequel schema mapping remains text-backed and records the
original type in a comment rather than claiming native JSONB support.
A partial index's predicate (`t.index … where:`,
`add_index … where:` in the migration fold, or `WHERE` in
`structure.sql`) is kept as the source database wrote it. Postgres
renders it on every index. SQLite renders it on a unique index, where
it decides which rows must be distinct, and so do the `insert_all`
conflict guard and `upsert_all`'s conflict target (`unique_by:` picks
the first unique index by name with those columns, as Rails does).
SQLite leaves it off a non-unique index, which then covers every row,
and off a unique one whose predicate falls outside the syntax both
engines read alike (`Dialect::index_predicate`: the table's columns,
literals, boolean and comparison operators, a few shared functions).
A Postgres dump's `::text` casts or `= ANY (ARRAY[…])` fall outside it;
that index is unique over every row, and the transpile names it in a
warning. The migration fold refuses to rename or remove a column a
predicate names.
An index's literal access method is kept from `schema.rb`'s `using:` or
`structure.sql`'s `USING` clause and quoted in PostgreSQL DDL. Schema Ruby
methods must be ASCII unquoted SQL identifiers and are folded to lowercase.
The `structure.sql` parser likewise folds ASCII unquoted methods and retains
the case of quoted methods. PostgreSQL DDL preserves these methods;
SQLite DDL uses SQLite's default index method, so query plans and index
performance can differ while row semantics stay the same. Custom PostgreSQL
methods are preserved as identifiers, but their extension must already be
installed by a PostgreSQL deployment.
Postgres column types map to their SQLite storage at
ingest (`uuid` → TEXT via `ColumnType::Uuid`, `json`/`jsonb` → the same
text-backed JSON representation, `citext` → text, `timestamptz` → datetime,
`inet`/`cidr`/`macaddr`/`enum` → string); a type with no mapping is an
ingest error (a ledger line under `--survey`), never a silent drop — its index would still be
emitted and the DDL would not apply. A non-integer primary key
(`create_table …, id: :uuid` / `primary_key: "identifier", id:
:string`) renders as `TEXT PRIMARY KEY` in SQLite and is carried end
to end by the ruby-shape emit: the analyzer
types `id`, `ids` and the key-taking finders from that column; the
emitted `find`/`exists?`/`update`/`delete`/`reload` primitives
compare it with the key's type; insert writes it (minting a blank
`uuid` key with `SecureRandom.uuid`, as the Postgres default would)
and answers it instead of the rowid; a key not called `id` gets
`id`/`id=` aliases; and a uuid foreign key's "no row" sentinel is
`""`, not `0`. Spinel carries it too: the shared `ActiveRecord::Base`
holds no `@id` slot (under Spinel a base-class ivar is the union of
every subclass's writes, so one String-keyed model would widen every
model's key to poly), its `id`/`id=` are a raise-bodied contract each
model's own typed accessors implement, and an app with a string key
ships the runtime sidecar with that contract widened to
`(Integer | String)` (`project::widen_key_contract`) while an
integer-only app ships it byte for byte. The compiled targets' model
layers still pin an integer id and report `non_integer_primary_key`
as an unsupported construct at emit time, per target, rather than as
an ingest gap that would be false of the ruby family (#90;
`fixtures/tiny-blog-uuid` + `tests/uuid_key_ruby.rs` run the shape
on CRuby, and the same fixture compiles and serves under Spinel).

### Generated columns

`Column.generated` retains a generated expression and its `stored` or
`virtual` mode separately from its declared result type. For example:

```ruby
t.string "first_name"
t.string "last_name"
t.virtual "display_name", type: :string,
  as: "first_name || ' ' || coalesce(last_name, '')", stored: true
```

The portable expression subset is deliberately bounded: unbounded string/text
columns, SQL string literals, parentheses, `||`, and `coalesce` with at least
two arguments. Expressions are validated against the complete table and kept
verbatim. A separate PostgreSQL DDL-only ingest API opts into the same subset
plus `::text`, `::varchar`, `::character varying`, and equivalent
`CAST(... AS ...)` forms over unbounded string/text expressions. The default
application ingest remains portable, so PostgreSQL-only casts still fail app
checking and cannot reach current SQLite project emission. Use
`ingest_schema_with_generated_expression_dialect` or the corresponding
`structure.sql`/migration entry point with
`schema::generated::GeneratedExpressionDialect::Postgres`, then call
`render_schema_statements_for(..., Dialect::Postgres)` to render DDL. This
source-expression mode is not a database selector and does not enable
PostgreSQL model persistence or a PostgreSQL application target.

That PostgreSQL DDL mode also accepts `::integer`, `::int4`, and
`CAST(... AS integer/int4)` only when the cast input is text, such as
`(payload ->> 'count'::text)::integer`. The declared generated result must
be an exact PostgreSQL int4. It does not admit integer-column operands,
integer literals, arithmetic, or other numeric casts; any `coalesce` input
must still satisfy the existing text-only rules. Because ordinary typing folds
several source widths into `ColumnType::Integer`,
`schema.rb` integer limits 1 and 2, and `structure.sql` `smallint`/`int2`,
`serial`/`serial4`, and integer types with typmods retain negative width
evidence and are refused as generated int4 results. Bare `integer`, `int`,
and `int4`, and Rails limits 3 and 4, remain eligible. This does not change
ordinary model typing or rendered SQLite types. In `structure.sql`, a
qualified integer type is considered exact only as unquoted `pg_catalog.int4`;
`integer` and `int` are unqualified SQL grammar aliases, not catalog type
names. The parser otherwise keeps its ordinary normalized type but refuses
generated int4 output because another schema can define a domain or type
with the same name.

That explicit PostgreSQL DDL mode also accepts `->` and `->>` when the left
operand is an exact `json` or `jsonb` column and the selector is a SQL string
literal (including Rails' `'key'::text` form) or a decimal array index
with an optional leading minus, within the full signed int4 range.
The operators can be chained; `->` may produce an intermediate JSON value,
and `->>` produces text. That text can be the generated result or feed the
explicit int4 cast described above.
Expressions remain verbatim, so PostgreSQL preserves the source JSON type's
behavior. The default application ingest remains portable, and SQLite DDL
validation rejects a schema imported in PostgreSQL mode. This adds no
PostgreSQL model persistence and leaves the shared serialized-text
`JsonColumn` model path unchanged.

Other PostgreSQL operators, casts to non-text types apart from the bounded
text-to-int4 casts above, other functions,
generated-column references, defaults, generated keys/timestamps,
length-limited casts or operand types remain explicit errors. A table must
contain an ordinary column. Source types normalized to text for ordinary model
typing (such as network types, enums, `citext`, and fixed-width characters)
remain unsupported here. Column names that are SQL keywords must be
double-quoted in the expression. Unresolved keyword splats in column options
are rejected because they can hide generated-column metadata.
Migration folding permits renaming or dropping an unindexed generated output
when the resulting table still validates. `change_column` on an existing
generated output and replacement of a generated output by an ordinary column
are rejected. Renaming or dropping a source column that a generated expression
uses is rejected without rewriting the source SQL; renaming or dropping an
indexed generated output is also an explicit error.
Assigning a default to a generated output is rejected before changing schema
state, including during survey recovery. Ordinary defaults and generated-column
nullability changes remain supported.
`remove_reference` and `remove_belongs_to` cannot remove a generated output;
their folded changes are also validated before altering generated expressions.
The same metadata is read from a complete `GENERATED ALWAYS AS (...) STORED`
or `VIRTUAL` clause in `structure.sql`; unsupported clauses cannot silently
become writable columns. Roda emission rejects generated columns. SQLite DDL
supports both modes; the separate PostgreSQL DDL renderer currently accepts
stored columns only, even though PostgreSQL 18 also supports virtual columns.
PostgreSQL text casts and JSON extraction require the explicit DDL-only ingest
mode described above; this does not enable a PostgreSQL runtime backend.

Normal model inserts and updates omit generated columns, while SELECT and
reload retain them. Generated attributes start nil, including a database
`NOT NULL` attribute. The `ruby`, `jruby`, and `spinel` persistence runtimes
use one `INSERT ... RETURNING` statement to read the inserted key and generated
values before create callbacks; there is no separate post-insert SELECT. The
SQLite persistence path requires SQLite 3.35 or newer, matching the
`Db.exec_returning` runtime gate. SQLite 3.31 through 3.34 support generated
columns in DDL but cannot run this model-persistence path. Project emission
also refuses targets whose runtime does not provide `Db.exec_returning`. These
runtime gates do not change the standalone schema DDL renderers: they continue
to preserve every accepted SQLite generated-column mode and stored-only
PostgreSQL generated columns. The single statement does not wrap the rest of
model save or callbacks in a transaction; callers that need rollback when
later application code raises must use their transaction.
Tables without a declared primary key can return SQLite's rowid from the
insert adapter primitive, but the existing model create/save lifecycle does
not support keyless models end to end.
An explicitly assigned nonnil value remains in memory through callbacks and
until reload. This create-time hydration does not add generated-column dirty
tracking or refresh generated values after later writes; updates leave the
previous value in memory until explicit reload, as observed with Rails 8.1.4
and SQLite. Existing
full-column update behavior also remains: saving only a generated-field
assignment on a mixed table may still write unchanged ordinary fields. A
table with no writable non-key fields uses `DEFAULT VALUES` on insert and
issues no UPDATE.

Direct writes that the current lowering would silently discard remain
unsupported: generated-model `insert_all`/`insert_all!`, and `update_column`
or named `touch` calls that can address a generated field, including
`belongs_to ..., touch: :generated_column` callbacks. Ordinary literal
column names retain their existing behavior; dynamic names on generated
models fail closed. Static seed SQL follows ordinary `create!` filtering.
Explicit generated values in YAML fixtures are rejected with fixture, record,
and column context. The regression suites are `tests/generated_columns*.rs`;
`generated_columns_spinel` executes the SQLite contract natively in the
selected Spinel CI suite.

## `config/routes.rb` → `RouteTable` → `RouteHelpers.<x>_path`

**Source IR:** `src/dialect.rs::RouteTable` — a list of `RouteSpec`
entries plus `direct_helpers` (`direct :name` custom URL helpers,
lowered by `src/lower/routes_to_library/direct.rs`). `RouteSpec` has
four variants — `Explicit`, `Root`, `Resources`, and `Scope`
(`namespace`/`scope` nesting); see `src/dialect.rs` for what each
carries (`Resources` knows about singular `resource` and `as:`
renames; `Explicit` records its `member`/`collection` scope).

**Ingest:** `src/ingest/routes.rs::ingest_routes` finds the outer
`Rails.application.routes.draw do … end` and walks its statements. Whole-app
ingest also supplies source-backed engine metadata to
`ingest_routes_with_engines` for the literal mount slice described below.
The recognizer covers the verb shortcuts (`get`/`post`/…), `match`,
`root`, `resources`/`resource` (with `only:`/`except:`/`as:`/
`controller:`/`param:`/`path:`, symbol or string spellings alike, as Rails
`to_sym`s them), `namespace`/`scope`,
`member`/`collection`/`constraints` blocks, `draw(:name)`
split files under `config/routes/`, and options like `defaults:`,
`on:`, and `via:` — `src/ingest/routes.rs` is the authority on the
current surface. Literal `redirect("/path")` targets on a verb or `root`
are synthesized into controller actions. Some block redirects are synthesized
too: `redirect_block` accepts bodies that pass its string-expression check,
including string literals and interpolations, conditionals whose branches
pass the check, sequences whose final expression passes it, and selected
method calls. It checks for at most two required block parameters, named
`_`, `params`, `request`, or `req`. Other dynamic targets outside this
recognizer remain a separate known gap: they are dropped and reported only
in survey mode. The engine-mount support below does not change those
non-mount target semantics.

One literal isolated-engine mount shape is composed into the shared route
scope: a top-level `mount Catalog::Engine, at: "/catalog"` where the engine
comes from a locked, in-tree `PATH` gem, declares one plain `Rails::Engine`
subclass under `lib/` with a direct literal `isolate_namespace`, and has one
source-backed `Catalog::Engine.routes.draw` block. The mount prefix must
be a static, absolute, non-root path without a trailing slash. Its routes are
flattened at the mount's source position with the engine controller
namespace, so earlier host routes, engine routes, and later host routes retain
their order. The prefix is matched as a path segment (`/catalogue` does not
match `/catalog`); the mounted root accepts both `/catalog` and
`/catalog/`.

The engine's `lib/` source must have a plain load shape: literal `require` or
`require_relative` calls may resolve only to another checked `.rb` file under
that engine's `lib/`, with `require "rails/engine"` (optionally `.rb`) as the
sole external bootstrap. Files may define namespace modules along the path
to the engine owner and below it, plus classes without custom superclasses,
instance methods, ordinary `self` methods, and scalar constants at the owner
namespace and below; Ruby load-hook methods are excluded. At file level, only
those declarations and the checked literal requires are accepted. The
`Engine` declaration itself may only set its literal isolated namespace.
Other load-time calls or computed declarations remain explicit mount errors,
including initializer or middleware registration through a gem entrypoint.

The engine route body is limited to bare, unconditional `root` and HTTP
shortcut declarations whose paths, `to:` targets, controller/action options,
and supported `as:`/`match via:` options are literal. Nonliteral targets
and all `redirect(...)` route targets, `resources`/`resource`,
`namespace`/`scope`, and other routing wrappers remain explicit errors; they
are not flattened into public routes. The engine's route helpers and the
host's named mount proxy helpers are not composed. Calls that could otherwise
fall through to a same-named host helper receive a located `route mount`
error. Rack applications and all other engine mount forms remain explicit
errors, including dynamic or patterned prefixes, `as:` aliases,
nested or repeated mounts, custom engine initializers and middleware, route
constraints or conditional branches, route prepends, Devise visibility
wrappers, and engine `direct` helpers. Strict emission refuses these errors;
`--allow-unsupported` can write an incomplete project, and survey mode records
the gap without clearing the diagnostic. Direct and literal reflective
(`send`, `public_send`, or `__send__`) uses of engine route helpers or the
mounted proxy are also located errors; engine helper proxies are not resolved
to the host route table.
The generic CRuby dispatch contract is exercised by
`tests/emit_and_run.rs`; `tests/spinel_toolchain.rs` carries the ignored native
Spinel HTTP witness.

The fixed runtime's top-level `mount ActionCable.server => "/cable"` (or
`at: "/cable"`) remains a separate exception. The existing CRuby/JRuby
pruning policy still omits Cable from apps without a live broadcast surface;
the literal engine support does not change that policy.


**Downstream consumers (analyze/lower):**

- **Analyzer** uses the controller/action pairings to wire up before-
  action and render edges.
- **`src/lower/routes.rs::flatten_routes`** expands the source-shape
  `RouteTable` into a flat `Vec<FlatRoute>`: one entry per
  `(method, path, controller, action)`, with `namespace`/`scope`
  prefixes composed in, a helper name (`article` → `article_path`,
  `edit_article` → `edit_article_path`), and the ordered list of path
  parameter names. `FlatRoute` also records whether the route is
  named (unnamed dynamic routes get no helper) and any route-forced
  response format.

**Lowered to LibraryFunction:** `src/lower/routes_to_library/`
produces one `LibraryFunction` per named route under `module_path:
["RouteHelpers"]`. Body is a typed `StringInterp` building the path
from path-params (`id` and `<x>_id` typed as `Int`, others as `Str`).
Multiple HTTP verbs on the same path collapse to a single helper
(e.g. `articles_path` covers both `GET /articles` and `POST
/articles`).

Route helpers are only half of what `src/lower/routes_to_library/`
emits: `lower_routes_to_dispatch_functions` builds the dispatch
surface under `module_path: ["RouteTable"]` (emitted to
`app/routes.ts` on TS) — the table that makes requests reach
controllers — and `lower_url_option_helpers` adds resolvers for
hash-form `url_for` options.

**Per-target emit:** TS writes `app/route_helpers.ts` with one
`export function` per helper plus the namespace const. Controller
and view bodies that call `RouteHelpers.article_path(id)` resolve
through the namespace import unchanged.

**Known shape limits.** Custom routes with `constraints:` are
preserved in the IR but the helper-emit ignores them.

## `db/seeds.rb` → `App::seeds` → `Seeds.run`

**Source IR:** `src/app.rs::App::seeds: Option<Expr>` — the seeds
file is stored as a single top-level `Expr` (usually a `Seq` of
AR-create sends, frequently guarded by an early-return on "already
populated"). No special dialect wrapping; it's just Ruby in IR form.

**Ingest:** `ingest_ruby_program` on the source.

**Analyze:** the body is typed against the model registry exactly
as any controller body — `Article.create!(...)` binds its argument
types from the `Article` class's attribute row.

**Lowered to LibraryFunction:** `src/lower/seeds_to_library/`
produces one `LibraryFunction` — `Seeds.run() -> nil` under
`module_path: ["Seeds"]`. The body is the seeds Expr verbatim;
analyze has already attached types and effects, so the walker
emits `Article.create!(...)` etc. the same way it would in any
other class context.

**Per-target emit:** TS writes `db/seeds.ts` with `export function
run()` plus the namespace const. `main.ts` passes `() =>
Seeds.run()` as the `seeds` callback to `startServer({ … })`; the
runtime invokes it on first boot when the DB is empty.

**Known shape limits.** No special handling today for `Rails.env`
gates or `unless` guards on seed records — whatever Ruby the file
contains is ingested as-is and the analyzer sorts out the types.

## `config/importmap.rb` → `Importmap` → `Importmap.{pins, entry}`

**Source IR:** `src/app.rs::App::importmap: Option<Importmap>` — a
list of `ImportmapPin { name, path }` in declaration order (Rails
preserves order for modulepreload link emission).

**Ingest:** `src/ingest/app.rs::ingest_importmap`. The DSL has three
common shapes: `pin "<name>"`, `pin "<name>", to: "<path>"`, and
`pin_all_from "<dir>", under: "<prefix>"` (which expands by walking
the named directory).

**Lowered to LibraryFunction:** `src/lower/importmap_to_library/`
produces two `LibraryFunction`s under `module_path: ["Importmap"]`:
`pins()` returns the structured pin list (an `Array` of
`Record{name, path}`), and `entry()` returns the name of the
importmap's entry module. The `<script type="importmap">…</script>`
element that Rails' view layer emits via `javascript_importmap_tags`
is built by the view-helper lowering, which consumes
`pins()`/`entry()` (see `src/lower/view_to_library/helpers.rs`).

**Per-target emit:** TS writes `app/importmap.ts` with one `export
function` per method plus the namespace const; the lowered layout
view reaches them through the `javascript_importmap_tags` helper.

**Known shape limits.** `pin_all_from` walks the local file system
at ingest time; if the source moves files between ingest and emit,
the resolved pins go stale. Today the ingest+emit cycle runs in
one process so this isn't an issue.

## What about migrations?

`db/schema.rb` is canonical whenever it exists, because:

1. `schema.rb` is the denormalized, authoritative snapshot — the same
   view every `rails db:prepare` would construct.
2. Migrations are imperative; schema.rb is declarative. Typing against
   the final shape is straightforward; replaying migrations to derive
   it is avoidable work.

When `schema.rb` is absent, Roundhouse next reads `db/structure.sql`
(`src/ingest/structure_sql.rs`), the SQL dump Rails writes under
`config.active_record.schema_format = :sql`. Its reader handles
PostgreSQL's `pg_dump` format. It skips the `\restrict` and
`\unrestrict` lines that pg_dump 18 (and the August 2025 minor
releases) brackets a dump with and that Rails before 7.2.3/8.0.3 keeps;
any other psql meta-command is ledgered as a statement it does not
model. When there is neither (never migrated locally, or gitignored),
the walk falls back to folding `db/migrate/*.rb` in filename order —
`src/ingest/schema.rs::ingest_migration`, called from
`src/ingest/app.rs`. Migration shapes it can't fold deterministically
(see `UNSUPPORTED_VERBS`) error with a pointer to `rails db:migrate`,
which materializes the schema.rb this fallback substitutes for. Roda
apps get the same fallback for Sequel-DSL migrations via
`src/ingest/sequel_migration.rs`.

The real-blog fixture generator (`scripts/create-blog`) runs
`rails db:prepare` after generating migrations, so `schema.rb`
always exists by the time ingest runs. See
[fixture setup](../development/testing.md#fixtures).

## Test fixtures: `test/fixtures/*.yml`

Not under `db/` or `config/`, but worth naming here since it rounds
out the declarative-inputs picture. Each `<table>.yml` becomes a
`Fixture` entry in `App::fixtures`; `src/lower/fixtures.rs::lower_fixtures`
turns them into a per-target-renderable load plan (which columns
receive literals, which are foreign-key references to another
fixture's eventual AUTOINCREMENT rowid). Record values are held as
`FixtureValue::Scalar(String)` or `FixtureValue::Ruby(Expr)` for
inline `<%= … %>` values, and a fixture file's ERB statement tags
land in the fixture's `preamble` of ingested Ruby (`src/dialect.rs`,
`src/ingest/fixture.rs`); emitters coerce scalars per column type.

**Lowered to LibraryClass** (not LibraryFunction — fixtures are
class-shaped because they have a state-like notion of "the loaded
records by label"). `src/lower/fixture_to_library/` produces one
`LibraryClass` per fixture file: `<Plural>Fixtures` with one class
method per label (`articles(:one)` → `ArticlesFixtures.one()`).

**Per-target emit:** TS writes `test/fixtures/<plural>.ts` with one
`export class <Plural>Fixtures` declaring all the labeled record
methods.

## Key files

| File | Role |
|------|------|
| `src/schema.rs` | `Schema` / `Table` / `Column` source IR |
| `src/ingest/` | `ingest_schema` + `ingest_migration` (`schema.rs`), `ingest_routes` (`routes.rs`), `ingest_importmap` (`app.rs`); seeds go through `ingest_ruby_program` (`expr.rs`) |
| `src/dialect.rs` | `RouteTable`, `RouteSpec`, `Fixture`, `LibraryClass`, `LibraryFunction` |
| `src/app.rs` | `App::seeds`, `App::fixtures`, `App::importmap`, `App::rbs_signatures` |
| `src/rbs.rs` | `sig/**/*.rbs` sidecars → `App::rbs_signatures` |
| `src/emit/shared/schema_sql.rs` | Schema → CREATE TABLE DDL statements (sibling `seed_sql.rs` renders seed rows) |
| `src/lower/routes.rs` | `RouteTable` → `Vec<FlatRoute>` |
| `src/lower/fixtures.rs` | YAML fixtures → loader plan |
| `src/lower/schema_to_library/` | Schema → `LibraryFunction` (`Schema.statements`) |
| `src/lower/routes_to_library/` | FlatRoutes → `Vec<LibraryFunction>` (RouteHelpers + the `RouteTable` dispatch surface + `direct` helpers) |
| `src/lower/seeds_to_library/` | App::seeds → `LibraryFunction` (`Seeds.run`) |
| `src/lower/importmap_to_library/` | Importmap → `Vec<LibraryFunction>` (`Importmap.{pins, entry}`) |
| `src/lower/fixture_to_library/` | Fixtures → `LibraryClass` per fixture file |

## Related docs

- [`ruby-and-erb.md`](ruby-and-erb.md) — how the general-purpose Ruby
  ingest path works (used by `db/seeds.rb`).
- [`catalog.md`](catalog.md) — the AR method catalog that lets the
  analyzer understand what `Article.create!(...)` means.
- [`../pipeline/lower.md`](../pipeline/lower.md) — the two-shape IR
  contract and detailed coverage of each `*_to_library` lowerer.
- [`../pipeline/emit.md`](../pipeline/emit.md) — how each shape is
  rendered per target (e.g. TS `export function` + namespace const).
