# Schema, routes, and seeds

Three Ruby files under a Rails app are not treated as general code
— they're recognized as declarative inputs and ingested into
dedicated IR structures: `db/schema.rb`, `config/routes.rb`, and
`db/seeds.rb`. This doc covers what each one contributes, and where
the resulting IR lands.

## `db/schema.rb` → `Schema`

**IR:** `src/schema.rs::Schema` — an `IndexMap<Symbol, Table>`. Each
`Table` carries its columns (typed via `ColumnType`), indexes, and
foreign-key declarations. Iteration order is source order, so
downstream consumers (schema DDL lowering, persistence lowering, model
attribute seeding) produce deterministic output.

**Ingest:** `src/ingest.rs::ingest_schema`. Recognizes the
`ActiveRecord::Schema[…].define do` DSL: `create_table`, `t.string`,
`t.integer`, `t.references`, `t.timestamps`, `add_index`,
`add_foreign_key`, etc.

**Downstream consumers:**

- **Analyzer** seeds each model's `attributes` row from its matching
  table — this is how `article.title : String` gets its type without
  any annotation in the model file.
- **`src/lower/schema_sql.rs`** produces a single `CREATE TABLE …`
  string per table (SQLite dialect today). Each emitter embeds it as a
  const so generated projects can initialize a fresh `:memory:` DB
  for tests or prepare a file-backed DB on first boot.
- **`src/lower/persistence.rs`** uses the column list to build
  INSERT / UPDATE / DELETE / SELECT strings per model.

**Known shape limits.** SQLite-only today. When Postgres or MySQL
demand per-engine DDL, a `Dialect` enum lands inside `schema_sql.rs`
without changing the `Schema` IR itself (it's already dialect-
neutral).

## `config/routes.rb` → `RouteTable`

**IR:** `src/dialect.rs::RouteTable` — a flat list of `RouteSpec`
entries. Each `RouteSpec` is one of: `HttpVerb { method, path, to }`,
`Root { to }`, `Resources { name, block }`, or `Unknown` for shapes
the recognizer can't classify yet.

**Ingest:** `src/ingest.rs::ingest_routes`. Finds the outer
`Rails.application.routes.draw do … end` and walks its statements.
Recognized: `get`/`post`/`patch`/`put`/`delete` verb shortcuts,
`root "c#a"`, `resources :name` (with optional nested block), and
`resource :name` (singular). Non-receiver sends inside the block are
the only statements considered — anything with an explicit receiver
is skipped so nested weird inputs don't re-enter.

**Downstream consumers:**

- **Analyzer** uses the controller/action pairings to wire up before-
  action and render edges.
- **`src/lower/routes.rs::flatten_routes`** expands the source-shape
  `RouteTable` into a flat `Vec<FlatRoute>`: one entry per
  `(method, path, controller, action)`, with a helper name (`article`
  → `article_path`, `edit_article` → `edit_article_path`) and the
  ordered list of path parameter names. Every pass-2 emitter consumes
  this flat form.

**Known shape limits.** Custom routes with `constraints:` or
`defaults:` are dropped; `namespace`/`scope` are not yet recognized.
Add to the `Unknown` path and widen the recognizer as fixtures force
the need.

## `db/seeds.rb` → `App::seeds`

**IR:** `src/app.rs::App::seeds: Option<Expr>` — the seeds file is
stored as a single top-level `Expr` (usually a `Seq` of AR-create
sends, frequently guarded by an early-return on "already populated").
No special dialect wrapping; it's just Ruby in IR form.

**Ingest:** `ingest_ruby_program` on the source. The analyzer types
the body against the model registry exactly as it does any controller
body — `Article.create!(...)` binds its argument types from the
`Article` class's attribute row.

**Downstream consumers:**

- **TypeScript emitter** wraps the seed expression in an
  `async function run()` and `main.ts` invokes it if the DB is fresh
  (empty on first boot).
- **Rust emitter** similarly emits a seed runner the server calls on
  first-boot after applying schema.
- Other targets: pending, treated as a no-op until the target's
  server runtime exists.

**Known shape limits.** No special handling today for `Rails.env`
gates or `unless` guards on seed records — whatever Ruby the file
contains is ingested as-is and the analyzer sorts out the types.

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

Not under `db/`, but worth naming here since it rounds out the
declarative-inputs picture. Each `<table>.yml` becomes a `Fixture`
entry in `App::fixtures`; `src/lower/fixtures.rs::lower_fixtures`
turns them into a per-target-renderable plan (which columns receive
literals, which are foreign-key references to another fixture's
eventual AUTOINCREMENT rowid). Values stay as strings in the IR;
emitters coerce per column type.

## Key files

| File | Role |
|------|------|
| `src/schema.rs` | `Schema` / `Table` / `Column` IR |
| `src/ingest.rs` | `ingest_schema`, `ingest_routes` (seeds go through `ingest_ruby_program`) |
| `src/dialect.rs` | `RouteTable`, `RouteSpec`, `Fixture` |
| `src/app.rs` | `App::seeds`, `App::fixtures` |
| `src/lower/schema_sql.rs` | Schema → CREATE TABLE DDL |
| `src/lower/routes.rs` | `RouteTable` → `Vec<FlatRoute>` |
| `src/lower/fixtures.rs` | YAML fixtures → loader plan |

## Related docs

- [`ruby-and-erb.md`](ruby-and-erb.md) — how the general-purpose Ruby
  ingest path works.
- [`catalog.md`](catalog.md) — the AR method catalog that lets the
  analyzer understand what `Article.create!(...)` means.
- [`../pipeline/lower.md`](../pipeline/lower.md) — detailed coverage
  of each lowering pass.
