# real-blog fixture

Source of truth: the output of
[`ruby2js/test/blog/create-blog`](https://github.com/ruby2js/ruby2js/blob/master/test/blog/create-blog),
distributed as
[demo-blog.tar.gz](https://ruby2js.github.io/ruby2js/releases/demo-blog.tar.gz).

A modernized Rails 8 blog with:
- Article and Comment models with `has_many :comments, dependent: :destroy` / `belongs_to :article`
- Turbo Streams broadcasting via `broadcasts_to` and `turbo_stream_from`
- Tailwind CSS class attributes (string + hash-of-conditions)
- Rails 8 strong params using `params.expect(...)`
- Form helpers: `form_with`, `form.label`, `form.text_field`, `form.textarea`, `form.submit`
- View helpers: `pluralize`, `link_to`, `render @articles`, `content_for`
- Nested `resources :articles do resources :comments, only: [...] end` routes
- `respond_to do |format| ... format.html ... format.json ... end` controller pattern
- Migrations (no generated `db/schema.rb`; Rails 8 regenerates from migrations)

## What's included here

A subset of the full Rails app — just the files that matter for ingest:
- `app/models/*.rb`
- `app/controllers/*.rb`
- `app/views/articles/*.html.erb`, `app/views/comments/_comment.html.erb`
- `app/views/layouts/application.html.erb`
- `config/routes.rb`
- `db/migrate/*.rb`
- `Gemfile`

Deliberately excluded:
- `app/helpers/`, `app/jobs/`, `app/mailers/`, `app/javascript/`, `app/assets/`
- `.json.jbuilder` templates (separate template language)
- `config/` beyond `routes.rb` (Rails env + initializers; out of current scope)
- `test/`, `bin/`, `tmp/`, `log/`, `public/`, `storage/`, `vendor/`

## Why it's here

This fixture is the target for Phase 1 of the multi-target plan
(see `project_roundhouse_strategy.md` in the auto-memory). Ingest
against it identifies Rails dialect gaps, which drive priority for
ingest/emit/analyzer work. Expected failures on any given day; the
fixture moves from "mostly rejected" to "fully ingested" incrementally.

## Known gaps as of check-in (2026-04-17)

Not comprehensive — the first probe hit ArrayNode and stopped.
Known/expected gaps, in rough priority order:

1. **ArrayNode in expressions** (`[:title, :body]`, `%i[ show edit update ]`)
   — needed for controller strong-params and `before_action only:` lists.
2. **`params.expect(...)` pattern** — Rails 8 replacement for
   `params.require(...).permit(...)`. New idiom; current analyzer
   doesn't know this method shape.
3. **`broadcasts_to` / `broadcast_replace_to`** DSL — new class-body
   recognizer family.
4. **`respond_to do |format| ... end`** — CallNode with block;
   the `format.html { ... }` / `format.json { ... }` inside are
   method calls on `format` with blocks. Existing block-ingest should
   handle shape-wise; semantics need a recognizer for render targets.
5. **Migration ingest** — currently we read `db/schema.rb`; here we
   have `db/migrate/*.rb`. Need to either generate `schema.rb` from
   migrations or ingest migrations directly. Rails 8 doesn't create
   `schema.rb` until migrations run.
6. **`t.references`, `t.timestamps`** column shorthands in schema/
   migrations — not in current recognizer.
7. **Symbol table names** (`create_table :articles do |t|`) vs the
   string form we currently handle.
8. **View helpers**: `link_to`, `form_with`, `form.label`, `pluralize`,
   `content_for`, `render @articles`, `turbo_stream_from`. Each is a
   method call; emit works generically; semantic resolution for typed
   targets needs helper-specific translation rules (future).
9. **`%i[...]` symbol-array literal** — a separate AST node kind.
10. **Hash-of-conditional-classes**: `{"class-a": cond1, "class-b": cond2}`
    — should work via existing hash literal handling; worth verifying.
11. **Comments** (Ruby `#` and ERB `<%# %>`) — dropped silently today;
    ruby2js-style association work tracked in auto-memory.
12. **Private methods** (`private` marker in controller) — not
    currently recognized; all methods ingest as public.
13. **`%i`, string interpolation `"#{x}"`, `begin/rescue/ensure`** —
    core Ruby features we haven't needed yet.

## Workflow

Don't try to ingest this fixture as part of the regular test suite
yet — it would fail with one of the above. When working on any item
on the list above, add a probe test (or a targeted unit test against
the specific file), fix the gap, verify, and expand.

When ingest succeeds cleanly end-to-end, add it as a real fixture
test paired with `source_equivalence` (byte-for-byte round-trip)
and `round_trip_identity`, same as tiny-blog.
