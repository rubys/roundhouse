# Schema, routes, seeds, and importmap

Four files under a Rails app are not treated as general code —
they're recognized as declarative inputs and ingested into dedicated
IR structures: `db/schema.rb`, `config/routes.rb`, `db/seeds.rb`,
and `config/importmap.rb`. This doc covers what each one
contributes, the IR shape it produces, and the `*_to_library`
lowering pass that turns each one into a `LibraryFunction` (or
`LibraryClass` for fixtures) for the universal post-lowering IR
that emitters consume.

The pattern is consistent across all four:

```
file → dedicated IR (App::<field>) → *_to_library → LibraryFunction → emit
```

Emitters never touch the source IR (`Schema`, `RouteTable`,
`Importmap`, `App::seeds`). They consume the lowered shape. See
[`../pipeline/lower.md`](../pipeline/lower.md) for the two-shape
contract.

## `db/schema.rb` → `Schema` → `Schema.create_tables`

**Source IR:** `src/schema.rs::Schema` — an `IndexMap<Symbol, Table>`.
Each `Table` carries its columns (typed via `ColumnType`), indexes,
and foreign-key declarations. Iteration order is source order, so
downstream consumers (schema DDL lowering, persistence lowering, model
attribute seeding) produce deterministic output.

**Ingest:** `src/ingest.rs::ingest_schema`. Recognizes the
`ActiveRecord::Schema[…].define do` DSL: `create_table`, `t.string`,
`t.integer`, `t.references`, `t.timestamps`, `add_index`,
`add_foreign_key`, etc.

**Downstream consumers (analyze/lower):**

- **Analyzer** seeds each model's `attributes` row from its matching
  table — this is how `article.title : String` gets its type without
  any annotation in the model file.
- **`src/emit/shared/schema_sql.rs::render_schema_sql`** produces the
  joined `CREATE TABLE …` DDL string (SQLite dialect today).
- **`src/lower/persistence.rs`** uses the column list to build
  INSERT / UPDATE / DELETE / SELECT strings per model.

**Lowered to LibraryFunction:** `src/lower/schema_to_library/`
produces a single `LibraryFunction` — `Schema.create_tables() -> string`
under `module_path: ["Schema"]`. The body is the rendered DDL as a
`Lit::Str`. Empty when `schema.tables` is empty (apps without
persisted models don't need a `Schema` artifact).

**Per-target emit:** TS writes `src/schema.ts` with one `export
function create_tables()` plus a trailing `export const Schema = {
create_tables }`. `main.ts` calls `Schema.create_tables()` and
hands the string to the runtime's `startServer({ schemaSql, … })`.

**Known shape limits.** SQLite-only today. When Postgres or MySQL
demand per-engine DDL, a `Dialect` enum lands inside `schema_sql.rs`
without changing the `Schema` IR itself (it's already dialect-
neutral) or the lowerer.

## `config/routes.rb` → `RouteTable` → `RouteHelpers.<x>_path`

**Source IR:** `src/dialect.rs::RouteTable` — a flat list of
`RouteSpec` entries. Each `RouteSpec` is one of: `Explicit { method,
path, controller, action, as_name?, constraints }`, `Root { target }`,
or `Resources { name, only, except, nested }`.

**Ingest:** `src/ingest/routes.rs::ingest_routes`. Finds the outer
`Rails.application.routes.draw do … end` and walks its statements.
Recognized: `get`/`post`/`patch`/`put`/`delete` verb shortcuts,
`root "c#a"`, `resources :name` (with `only:` / `except:` / nested
block), and `resource :name` (singular).

**Downstream consumers (analyze/lower):**

- **Analyzer** uses the controller/action pairings to wire up before-
  action and render edges.
- **`src/lower/routes.rs::flatten_routes`** expands the source-shape
  `RouteTable` into a flat `Vec<FlatRoute>`: one entry per
  `(method, path, controller, action)`, with a helper name (`article`
  → `article_path`, `edit_article` → `edit_article_path`) and the
  ordered list of path parameter names.

**Lowered to LibraryFunction:** `src/lower/routes_to_library/`
produces one `LibraryFunction` per named route under `module_path:
["RouteHelpers"]`. Body is a typed `StringInterp` building the path
from path-params (`id` and `<x>_id` typed as `Int`, others as `Str`).
Multiple HTTP verbs on the same path collapse to a single helper
(e.g. `articles_path` covers both `GET /articles` and `POST
/articles`).

**Per-target emit:** TS writes `app/route_helpers.ts` with one
`export function` per helper plus the namespace const. Controller
and view bodies that call `RouteHelpers.article_path(id)` resolve
through the namespace import unchanged.

**Known shape limits.** Custom routes with `constraints:` are
preserved in the IR but the helper-emit ignores them. `namespace` /
`scope` aren't yet recognized — add to the recognizer as fixtures
force the need.

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

## `config/importmap.rb` → `Importmap` → `Importmap.{json, tags}`

**Source IR:** `src/app.rs::App::importmap: Option<Importmap>` — a
list of `ImportmapPin { name, path }` in declaration order (Rails
preserves order for modulepreload link emission).

**Ingest:** `src/ingest/app.rs::ingest_importmap`. The DSL has three
common shapes: `pin "<name>"`, `pin "<name>", to: "<path>"`, and
`pin_all_from "<dir>", under: "<prefix>"` (which expands by walking
the named directory).

**Lowered to LibraryFunction:** `src/lower/importmap_to_library/`
produces two `LibraryFunction`s under `module_path: ["Importmap"]`:
`json()` returns the importmap as a JSON string, `tags()` wraps it
in the `<script type="importmap">…</script>` element that Rails'
view layer emits via `javascript_importmap_tags`. Both bodies are
static `Lit::Str` values pre-computed at lower time.

**Per-target emit:** TS writes `app/importmap.ts` with one `export
function` per method plus the namespace const. The view layer's
layout template can call `Importmap.tags()` to inject the
`<script>` block, or `Importmap.json()` if it wants the bare JSON
for some other purpose.

**Known shape limits.** `pin_all_from` walks the local file system
at ingest time; if the source moves files between ingest and emit,
the resolved pins go stale. Today the ingest+emit cycle runs in
one process so this isn't an issue.

## What about migrations?

`db/migrate/*.rb` files are **not** ingested today. The ingester reads
`db/schema.rb` directly because:

1. `schema.rb` is the denormalized, authoritative snapshot — the same
   view every `rails db:prepare` would construct.
2. Migrations are imperative; schema.rb is declarative. Typing against
   the final shape is straightforward; replaying migrations to derive
   it is unnecessary work.

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
fixture's eventual AUTOINCREMENT rowid). Values stay as strings in
the IR; emitters coerce per column type.

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
| `src/ingest.rs` + `src/ingest/`| `ingest_schema`, `ingest_routes`, `ingest_importmap` (seeds go through `ingest_ruby_program`) |
| `src/dialect.rs` | `RouteTable`, `RouteSpec`, `Fixture`, `LibraryClass`, `LibraryFunction` |
| `src/app.rs` | `App::seeds`, `App::fixtures`, `App::importmap` |
| `src/emit/shared/schema_sql.rs` | Schema → CREATE TABLE DDL |
| `src/lower/routes.rs` | `RouteTable` → `Vec<FlatRoute>` |
| `src/lower/fixtures.rs` | YAML fixtures → loader plan |
| `src/lower/schema_to_library/` | Schema → `LibraryFunction` (`Schema.create_tables`) |
| `src/lower/routes_to_library/` | FlatRoutes → `Vec<LibraryFunction>` (RouteHelpers) |
| `src/lower/seeds_to_library/` | App::seeds → `LibraryFunction` (`Seeds.run`) |
| `src/lower/importmap_to_library/` | Importmap → `Vec<LibraryFunction>` (`Importmap.{json, tags}`) |
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
