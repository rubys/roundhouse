# roda-sequel-blog

A small, idiomatic **Roda + Sequel** blog, built as the strawman exemplar for
[roundhouse#67](https://github.com/rubys/roundhouse/issues/67) — the RFC asking
whether Roundhouse (a Rails-to-many-languages transpiler) can take Roda + Sequel
as a second source framework.

It is deliberately **domain-identical** to Roundhouse's existing `real-blog`
Rails fixture — `Article has_many :comments`, index/show/new/edit/create/update/
destroy plus nested comment create/destroy — so the two can be run through the
same typed IR and emitters and diffed. The question the exemplar exists to answer
is *"where does the IR come out Roda/Sequel-shaped rather than Rails-shaped?"*,
and a held-constant *domain* is what makes that diff legible.

Held-constant means the **domain**, not the **code**: where idiomatic Roda/Sequel
and Rails diverge, this app takes the Roda/Sequel idiom and the mapping table
below notes the divergence. Transliterating Rails into Roda would make a
dishonest test — the whole point is to see the idiomatic stack's real shape.

> **Status:** reviewed strawman. @jeremyevans reviewed the first cut in the
> [RFC thread](https://github.com/rubys/roundhouse/issues/67); his notes are
> folded in (see *Idiomatic choices adopted from review* below).

## Run it

```sh
bundle install
bundle exec ruby seeds.rb      # runs migrations + inserts sample data
bundle exec rackup -p 9292     # http://localhost:9292
```

SQLite, no external services. Migrations run automatically on boot (`db.rb`), so
the app is runnable with no separate setup step.

## Test it

```sh
bundle exec ruby test/blog_test.rb
```

`test/blog_test.rb` is an in-process (minitest + rack-test) spec of the full
route surface: redirects, CRUD with valid and invalid input, flash messages via
the session, the method-override form pattern, the interior-node 404s, the
`r.post true` path-termination check, and default escaping. It doubles as the
behavioral oracle for [roundhouse#67](https://github.com/rubys/roundhouse/issues/67):
a transpiled version of this app must pass the same suite unchanged.

## Layout

```
app.rb                  Roda app: the routing tree + shared actions/helpers
db.rb                   Sequel connection, migrator, model-wide plugins
models/article.rb       Sequel::Model + one_to_many + validations
models/comment.rb       Sequel::Model + many_to_one + validations
db/migrate/*.rb         Sequel migration DSL (the schema)
views/**/*.erb          ERB templates (layout + partials), rendered by Roda
seeds.rb                sample data
config.ru               `run Blog.freeze.app`
test/blog_test.rb       in-process spec of the route surface (the oracle)
```

## Rails ↔ Roda/Sequel mapping

| Rails (`real-blog`) | This app |
|---|---|
| `config/routes.rb` `resources :articles` | `route do \|r\|` tree in `app.rb` |
| `ArticlesController#index` etc. | terminal blocks in the routing tree |
| `before_action :set_article` | `next unless @article = Article[id]` at the `r.on Integer` interior node |
| root + index both render index | `r.root { r.redirect "/articles" }` — one canonical path, not two |
| strong params `params.expect(article: […])` | `model.set_fields(r.params["article"], %w[title body])` |
| `ActiveRecord::Base` | `Sequel::Model` |
| `has_many`/`belongs_to` | `one_to_many`/`many_to_one` |
| `Article.includes(:comments).order(created_at: :desc)` | `Article.eager(:comments).reverse(:created_at)` |
| `save!` raises; controllers use `if save` | `Sequel::Model.raise_on_save_failure = false`, then `if model.save` (validates once) |
| `validates :title, presence: true` | `validate` + `validates_presence` (validation_helpers plugin) |
| DB defaults nullable; presence only in model | migrations declare `null: false` — Sequel leans on DB constraints |
| ERB auto-escapes `<%= %>` | `render escape: true`: `<%= %>` escapes, `<%== %>` raw (no manual `h`) |
| `render partial:`/`locals:` | `part("articles/_form", article: @a, …)` (part plugin) |
| `db/migrate` (AR migrations) | `db/migrate` (Sequel migrations) |
| `flash[:notice]`, `redirect_to` | `flash["notice"]`, `r.redirect` |
| implicit `_method` override | `use Rack::MethodOverride` + `all_verbs` plugin |

## What this exemplar deliberately exercises

Two routing-tree properties that @jeremyevans flagged in the RFC thread as things
a naive "split each terminal block into an independent handler" model does **not**
capture — both live at the `r.on Integer do |id|` node in `app.rb`:

1. **Shared interior state.** `@article` is loaded once at the interior node and
   consumed by every sub-branch (show, edit, update, destroy, and the nested
   comment routes). An ingest front-end that specializes the tree into per-route
   handlers has to thread this shared state into each, not just duplicate prefix
   *code*.

2. **An interior-node abort.** `next unless @article = Article[id]` abandons the
   whole subtree at the interior node when the record is missing: the block
   returns `nil`, Roda treats the route as unhandled, and the `not_found` handler
   renders a 404 — the "stop partway down the tree" case, before any terminal
   matcher runs. This is the idiomatic form; access-control failures use the same
   interior-node mechanism but with `r.halt`/`r.redirect` (returning an explicit
   response) instead of `next`.

It also exercises the friendlier parts: a typed `Integer` matcher (`id` is known
to be an integer at the call site — better inference input than a stringly
`params[:id]`), literal string matchers throughout, a nested resource, and
Sequel's explicit dataset algebra (`eager(:comments).order(...)`).

## What it deliberately leaves out (and why)

Priced on the "unsupported ledger", not solved here — kept out so the exemplar
stays a clean A/B rather than a feature tour:

- **Array matchers** like `[Integer, "foo", String]`, which @jeremyevans noted
  are the case where the match-block arg type *isn't* statically knowable (int /
  string / `nil`). The single-`Integer` matcher used here is the statically-typed
  case on purpose.
- **`class_matchers` / `symbol_matchers`, proc/custom matchers** — common in the
  large but not needed for this domain.
- **Virtual-row blocks** (`where{ … }`). Per the RFC thread these are largely a
  non-issue for transpilation (they build `Sequel::SQL::Identifier`/`Function`
  objects, usually statically determinable). This app has no query that needs
  one, and adding a filter the Rails fixture doesn't have would break the A/B, so
  none is included — the seam is acknowledged, not exercised.
- **Dataset-level-only models.** Sequel's dataset level has no ActiveRecord
  equivalent; staying at the model level keeps the lowering diff apples-to-apples.
- **Assets/JS, CSRF, real-time updates.** Orthogonal to whether the IR comes out
  clean; the Rails fixture's Turbo Streams / importmap have no bearing on the
  routing-tree and ORM shapes under study.

## Plugins used

The minimal honest subset of @jeremyevans's "core browser-app" list for what this
domain needs: `render` (with `escape: true`, so `<%= %>` HTML-escapes by default),
`part` (partials with keyword locals), `sessions`, `flash`, `not_found`, plus
`all_verbs` (so browser forms can PATCH/DELETE via `Rack::MethodOverride`).
`assets`, `public`, `common_logger`, `error_handler` are intentionally omitted as
runtime surface rather than IR-shape questions.

## Idiomatic choices adopted from review

Changes made after @jeremyevans's [review](https://github.com/rubys/roundhouse/issues/67),
each taking the Roda/Sequel idiom over the Rails transliteration:

- **`render escape: true`** — output escapes by default (`<%==` for raw), so the
  manual `h(...)` calls and the `h` plugin are gone.
- **`part` plugin** — `part("articles/_form", article: @a, …)` instead of
  `partial(..., locals: {…})`.
- **`set_fields`** — models take an explicit allow-list
  (`set_fields(r.params["article"], %w[title body])`) rather than a strong-params
  slice. (`typecast_params` for non-Hash param guarding is noted as available but
  left out as overkill for a blog.)
- **`raise_on_save_failure = false`** + `if model.save` — validates once, not
  twice (no separate `valid?` call).
- **`next unless` for missing records** — the idiomatic interior-node abort into
  the `not_found` handler, for both a missing article and a missing comment
  (the latter previously silently no-op'd; now a 404).
- **`r.post true`** in the nested comments route — path-termination check, so
  `POST /articles/1/comments/garbage` 404s instead of matching.
- **`reverse(:created_at)`** instead of `order(Sequel.desc(:created_at))`.
- **`with_pk`** instead of `where(id: …).first` for the comment lookup.
- **`null: false` DB constraints** in the migrations — Sequel leans on the
  database; validations remain in the model as defense in depth.
- **`r.root` redirects to `/articles`** — one canonical path instead of two
  identical pages.
- **`one_to_many :comments`** drops the redundant `key: :article_id` (the default).

## License

MIT — see [MIT-LICENSE](MIT-LICENSE).
