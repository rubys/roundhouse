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
  produces the `CREATE TABLE …` DDL statement list (SQLite dialect
  today; the joined-string `render_schema_sql` survives for some
  targets). The sibling `src/emit/shared/seed_sql.rs` renders
  `db/seeds.rb` data to a `db/seed.sql` for text-only archives.
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

**Known shape limits.** SQLite-only today. When Postgres or MySQL
demand per-engine DDL, a `Dialect` enum lands inside `schema_sql.rs`
without changing the `Schema` IR itself (it's already dialect-
neutral) or the lowerer.

## `config/routes.rb` → `RouteTable` → `RouteHelpers.<x>_path`

**Source IR:** `src/dialect.rs::RouteTable` — a list of `RouteSpec`
entries plus `direct_helpers` (`direct :name` custom URL helpers,
lowered by `src/lower/routes_to_library/direct.rs`). `RouteSpec` has
four variants — `Explicit`, `Root`, `Resources`, and `Scope`
(`namespace`/`scope` nesting); see `src/dialect.rs` for what each
carries (`Resources` knows about singular `resource` and `as:`
renames; `Explicit` records its `member`/`collection` scope).

**Ingest:** `src/ingest/routes.rs::ingest_routes`. Finds the outer
`Rails.application.routes.draw do … end` and walks its statements.
The recognizer covers the verb shortcuts (`get`/`post`/…), `match`,
`root`, `resources`/`resource`, `namespace`/`scope`,
`member`/`collection`/`constraints` blocks, `mount`, `draw(:name)`
split files under `config/routes/`, and options like `defaults:`,
`on:`, and `via:` — `src/ingest/routes.rs` is the authority on the
current surface.

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

When `schema.rb` is absent (never migrated locally, or gitignored),
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
[`../../DEVELOPMENT.md`](../../DEVELOPMENT.md#fixtures).

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
