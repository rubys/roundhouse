# Roda + Sequel front-end: mapping table and step-1 plan (issue #67)

Step 1 of the plan agreed with @jeremyevans on
[#67](https://github.com/rubys/roundhouse/issues/67): ingest the
Roda + Sequel exemplar (`fixtures/roda-blog`, vendored from
[rubys/roda-sequel-blog](https://github.com/rubys/roda-sequel-blog)) into the
**same** IR the Rails front-end produces, emit the CRuby target, and run the
fixture's own oracle suite (`test/blog_test.rb`, 18 checks) against the emitted
app. The deliverable besides a green oracle is the per-app ledger of what did
not lower.

Two acceptance tests:

1. **Oracle parity** — the emitted CRuby app passes `test/blog_test.rb`
   unchanged (gate: `tests/roda_toolchain.rs`, second test, lands with the
   ingest work; the first test pins the oracle against the fixture as-is).
2. **IR convergence** — `dump_ir` on `fixtures/roda-blog` vs
   `fixtures/real-blog` (domain-identical by construction) comes out
   near-isomorphic: same `Model`/`Association`/`Validation` records, same
   flat route set, same filter-chain shape. Divergence is either a real
   dialect difference (record it) or evidence the IR is secretly AR-shaped
   (fix it) — either way it is the research finding this step exists for.

Design stances (committed before ingest code):

- **(a)** Reuse the AR-derived IR (`dialect.rs`, `arel/ir.rs`) and extend only
  additively, where a construct in the fixture forces it.
- **(b)** Flatten the routing tree by **per-route linearization** onto the
  existing before-filter prologue mechanism (details below). This is the
  load-bearing decision.
- **(c)** Anything outside the recognized vocabulary goes to the ledger
  (`DiagnosticKind::Unsupported`), never into a corrupted IR record. Per the
  `ingest/routes.rs` convention: a dropped route is a ledger line, never a
  silently empty table.

## Routing: per-route linearization

A Roda routing block is a matcher tree. Each request takes exactly one
root→leaf path, so the tree flattens to one record per path:

- **Route** = the concatenation of matchers along a root→leaf path →
  `RouteSpec::Explicit { method, path, controller, action, constraints }`.
- **Prologue** = the interior statements along that path (everything in an
  enclosing block that executes before descending), in source order →
  synthesized `Filter { kind: Before, only: [actions under that node] }` on
  the synthesized controller, so `ControllerResolution.filter_chain` and the
  `PreambleStmt`/`halt_if_performed` machinery in
  `lower/controller_to_library/process_action.rs` are reused as-is.
  Duplicating prefix work per route is semantics-preserving precisely because
  each request runs one path.

Interior aborts — the seam Jeremy flagged — map onto the existing halt model:

| Roda construct | Semantics | IR |
|---|---|---|
| `next unless @article = Article[id]` at an interior node | block → nil → route unhandled → `not_found` handler renders, status 404 | before-filter: assign ivar; on nil, render `not_found` template with 404 + halt (`If performed? → Return`, the same shape Rails' `set_article`-raises-RecordNotFound lowers to) |
| `r.halt` at any node | explicit response, stop routing | filter/action body renders + halt check |
| `r.redirect` at any node | explicit redirect, stop routing | `RenderTarget::Redirect` + halt check |

Matcher vocabulary (recognized → IR; anything else → ledger):

| Matcher | IR |
|---|---|
| `r.on "literal"` | static path segment |
| `r.on Integer` (block arg `id`) | `:id` segment, `constraints: {id: Integer}`, block param typed `Integer` |
| `String` matcher | `:param` segment typed `String` |
| `r.is` / verb with args / `r.verb true` | path-termination check — **required**: a linearized route that never passes a termination check would match trailing garbage; such routes go to the ledger, not the table |
| bare `r.verb` (no args, outside `r.is`) | verb check only, no termination → ledger |
| `r.root` | `RouteSpec::Root` (here: redirect action) |
| array / regexp / proc / `class_matchers` / `symbol_matchers` | ledger (deliberately out of scope, per #67 discussion) |

Controller/action synthesis: the top-level `r.on "articles"` subtree becomes
the `Articles` controller (`ClassId`); action names come from a REST-shape
recognizer (GET collection → `index`, GET member → `show`, `"new"`/`"edit"`
literals → `new`/`edit`, POST collection → `create`, PATCH/DELETE member →
`update`/`destroy`; nested `r.on "comments"` → `Comments` controller). Routes
that don't match a REST shape get mechanical names (`get_articles_id_edit`).
The recognizer is cosmetic — it exists so acceptance test 2 diffs cleanly —
but the linearization beneath it is purely mechanical.

App-level plugin allowlist (unknown plugin → ledger): `render` (`escape:
true` assumed — see Views), `part`, `all_verbs`, `sessions`, `flash`,
`not_found` (its block = the app's 404 template), `use Rack::MethodOverride`
(same override Rails installs implicitly). View helpers defined on the app
class (`truncate`, `pluralize`) land where Rails helper modules do.

`r.params` is `Hash[String, …]` natively — no indifferent-access seam at all
(cleaner than Rails; ties into the params-indifferent-access design, which
already commits to string keys).

## Sequel → model IR

`db.rb` is the config vocabulary, recognized statement-by-statement:
`Sequel.sqlite(...)` (adapter), `Sequel::Migrator.run` (fold `db/migrate`),
`Sequel::Model.raise_on_save_failure = false` (**required** in step 1: it
makes `#save` return nil/false like AR's `#save`; if absent, `#save` raises
and every `if model.save` lowering is wrong → ledger the app), plugins
`validation_helpers` and `timestamps, update_on_create: true` (→ the existing
Rails timestamp convention).

| Sequel | IR |
|---|---|
| `class Article < Sequel::Model` | `Model { table: articles }` (same underscore-pluralize convention as AR); `attributes: Row` folded from the migration-derived `Schema`, not from a live DB |
| `one_to_many :comments, order: Sequel.desc(:created_at)` | `Association::HasMany { foreign_key: article_id (default), scope: order desc }` |
| `many_to_one :article` | `Association::BelongsTo` (required — the FK is `null: false`) |
| `def validate; super; validates_presence [...]; end` | the imperative body is a closed vocabulary: each `validates_*` call → a declarative `Validation`; `validates_presence [:a, :b]` → `Presence` per attribute; `validates_min_length 10, :body` → `Length { min: 10 }` (check whether `ValidationRule` carries a custom message; if not, that's the first additive extension); unrecognized statements in `validate` → ledger |
| `Sequel.migration { change { create_table ... } }` | fold into `Schema` alongside `ingest_migration`: `primary_key` → pk `Integer`; `String :t` → `String`; `String :t, text: true` → `Text`; `DateTime` → `DateTime`; `foreign_key :article_id, :articles, on_delete: :cascade` → `Reference` + `ForeignKey` (cascade); `index` → `Index`; `null: false` → `nullable: false` |

## Sequel dataset → query IR

All of these are `Send` chains typed `Ty::Relation` folding into `ArelOp`
exactly like their AR counterparts — extend `AR_CATALOG`/`is_query_builder_method`
with the Sequel names (as `ReceiverContext`-appropriate entries), don't build
a parallel catalog:

| Sequel | AR equivalent | Query IR |
|---|---|---|
| `Article[id]` | `find_by(id:)` — returns nil, doesn't raise | `Select { Eq(id), limit 1 }`, `ReturnKind::SelfOrNil` |
| `Article.eager(:comments)` | `includes(:comments)` | `PreloadDirective` |
| `.reverse(:created_at)` | `order(created_at: :desc)` | `Order { Desc }` |
| `.all` | `.to_a` terminal | `ArrayOfSelf` |
| `article.comments_dataset` | `article.comments` (relation form) | association-scoped `Relation` |
| `.with_pk(x)` | `.find_by(id: x)` on the scoped relation | `Select { Eq(fk) ∧ Eq(id), limit 1 }`, `SelfOrNil` |
| `Model.new.set_fields(hash, %w[a b])` | `new(params.permit(:a, :b))` | per-field assignment from a **static** allow-list — no strong-params machinery |
| `#save` (with `raise_on_save_failure = false`) | `#save` | `DbWrite`, `Bool` |
| `#destroy`, `.count`, `add_comment(...)` (seeds) | same / `comments.create` | existing catalog entries |

Virtual-row blocks (`where { ... }`), dataset-level-only models, and the rest
of Sequel's surface: out of scope for step 1, ledger on sight.

## Views

Roda's `render`/`part` use erubi like Rails; with `escape: true` the escaping
semantics are identical to Rails ERB (`<%=` escapes, `<%==` raw). So
`ingest_view`/`erb.rs` are reused; the differences are wiring, not language:

| Roda | IR |
|---|---|
| `view "articles/index"` | `RenderTarget::Template` |
| `plugin :render, layout: "layout"` + `<%== yield %>` | `LayoutDecl::Name("layout")` |
| `part("articles/_form", article: ..., action: ...)` | partial render edge; the kwargs are exactly a strict-locals row (`View.strict_locals`) — Roda's `part` is the strict-locals pattern by construction |
| `flash["notice"]` in layout | same `Send`/`Index` shape as Rails flash |

## Build order (step 1)

1. **Recognizers** behind a front-end dispatch in `ingest/app.rs` (a
   `config.ru` + no `config/routes.rb` selects the Roda front-end):
   `ingest/roda_app.rs` (route-tree walk + linearization + controller
   synthesis), `ingest/sequel_model.rs`, `ingest/sequel_migration.rs`,
   `db.rb`/plugin config recognition. Ledger lines from day one.
2. **Catalog extension** for the dataset methods above.
3. **First route end-to-end**: `GET /articles` through analyze → `emit::ruby`,
   oracle test `test_index_lists_articles_newest_first` green.
4. **Full CRUD**: remaining routes; the oracle's 18 checks are the worklist.
5. **IR diff** vs `real-blog` (acceptance test 2) + write the ledger.
6. Report ledger + findings on #67.

Timebox: this is a spike, not a lane. If linearization or the dataset
lowering balloons, ledger the hard cases, post partial results on #67, and
return to the lobsters worklist.
